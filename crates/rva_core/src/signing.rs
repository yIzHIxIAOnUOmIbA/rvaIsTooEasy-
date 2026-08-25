//! Signing and the keystore (Task B patch-ecosystem moat).
//!
//! - Keystore dir: Windows `%APPDATA%/RVACompare/keystore`, other platforms `$HOME/.rvacompare/keystore`.
//! - Private keys `<name>.key` are stored OS-DPAPI-encrypted (Windows); public keys `<name>.pub` are plaintext.
//! - Fingerprint = SHA256(public key)[0..32], used as the patch signature identifier and lookup key.
//! - Verification flow (spec): the patch carries a fingerprint -> locate the public key in the keystore
//!   (including imported trusted keys) -> verify the signature.

use crate::patch_pack::PatchSignature;
use anyhow::{bail, Context, Result};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};
use rand_core::OsRng;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

/// Key entry info (external DTO).
#[derive(Debug, Clone, serde::Serialize)]
pub struct KeyInfo {
    pub name: String,
    pub fingerprint_hex: String,
    pub has_private: bool,
}

pub fn keystore_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("RVA_KEYSTORE") {
        return PathBuf::from(dir);
    }
    let base = if cfg!(windows) {
        std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
    } else {
        std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(".")).join(".rvacompare")
    };
    base.join("RVACompare").join("keystore")
}

/// Generate a new key pair and write it into the keystore (private key DPAPI-encrypted).
pub fn generate_keypair(name: &str) -> Result<KeyInfo> {
    validate_name(name)?;
    let sk = SigningKey::generate(&mut OsRng);
    let vk = sk.verifying_key();
    let fp = fingerprint(vk.as_bytes());
    write_private(&private_path(name), &sk.to_bytes())?;
    std::fs::write(public_path(name), vk.as_bytes())
        .with_context(|| format!("写入公钥 {} 失败", public_path(name).display()))?;
    Ok(KeyInfo { name: name.to_string(), fingerprint_hex: hex(&fp), has_private: true })
}

/// Import an external public key (trust only; no private key produced). Overwrites on name conflict.
pub fn import_public(name: &str, public_key: &[u8]) -> Result<KeyInfo> {
    validate_name(name)?;
    if public_key.len() != 32 {
        bail!("公钥长度非法: {} (期望 32)", public_key.len());
    }
    std::fs::write(public_path(name), public_key)
        .with_context(|| format!("写入公钥 {} 失败", public_path(name).display()))?;
    Ok(KeyInfo { name: name.to_string(), fingerprint_hex: hex(&fingerprint(public_key)), has_private: false })
}

/// List all keys in the keystore.
pub fn list_keys() -> Result<Vec<KeyInfo>> {
    let dir = keystore_dir();
    let mut out = Vec::new();
    if !dir.exists() {
        return Ok(out);
    }
    let mut names: Vec<String> = Vec::new();
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let fname = entry.file_name().to_string_lossy().to_string();
        if let Some(stem) = fname.strip_suffix(".pub") {
            names.push(stem.to_string());
        }
    }
    names.sort();
    for name in names {
        let has_private = private_path(&name).exists();
        if let Ok(pub_bytes) = std::fs::read(public_path(&name)) {
            if pub_bytes.len() == 32 {
                out.push(KeyInfo {
                    name,
                    fingerprint_hex: hex(&fingerprint(&pub_bytes)),
                    has_private,
                });
            }
        }
    }
    Ok(out)
}

/// Delete a key (private key + public key).
pub fn delete_key(name: &str) -> Result<()> {
    for p in [private_path(name), public_path(name)] {
        if p.exists() {
            std::fs::remove_file(&p).with_context(|| format!("删除 {} 失败", p.display()))?;
        }
    }
    Ok(())
}

/// Sign content with the named key in the keystore (patch signing).
pub fn sign_bytes(name: &str, content: &[u8]) -> Result<PatchSignature> {
    let seed = read_private(&private_path(name))?;
    let sk = SigningKey::from_bytes(&seed);
    let vk = sk.verifying_key();
    let sig: Signature = sk.sign(content);
    Ok(PatchSignature {
        fingerprint: fingerprint(vk.as_bytes()),
        signature: sig.to_bytes(),
    })
}

/// Export the raw public key bytes (for distribution).
pub fn public_key_bytes(name: &str) -> Result<[u8; 32]> {
    let data = std::fs::read(public_path(name))
        .with_context(|| format!("读取公钥 {} 失败", public_path(name).display()))?;
    if data.len() != 32 {
        bail!("公钥长度非法");
    }
    let mut k = [0u8; 32];
    k.copy_from_slice(&data);
    Ok(k)
}

/// Locate the public key by fingerprint in the keystore and verify the signature (including imported trusted keys).
pub fn verify_by_fingerprint(content: &[u8], sig: &PatchSignature) -> Result<bool> {
    let Some(vk) = find_public_by_fingerprint(&sig.fingerprint)? else {
        return Ok(false); // 无匹配公钥：视为未知签发者
    };
    let signature = Signature::from_bytes(&sig.signature);
    Ok(vk.verify(content, &signature).is_ok())
}

fn find_public_by_fingerprint(fp: &[u8; 32]) -> Result<Option<VerifyingKey>> {
    let dir = keystore_dir();
    if !dir.exists() {
        return Ok(None);
    }
    for entry in std::fs::read_dir(&dir)? {
        let entry = entry?;
        let fname = entry.file_name().to_string_lossy().to_string();
        if !fname.ends_with(".pub") {
            continue;
        }
        let data = std::fs::read(entry.path())?;
        if data.len() != 32 {
            continue;
        }
        if fingerprint(&data) == *fp {
            let arr: [u8; 32] = data.as_slice().try_into().unwrap();
            let vk = VerifyingKey::from_bytes(&arr).ok();
            if vk.is_some() {
                return Ok(vk);
            }
        }
    }
    Ok(None)
}

fn fingerprint(pub_key: &[u8]) -> [u8; 32] {
    Sha256::digest(pub_key).into()
}

fn validate_name(name: &str) -> Result<()> {
    if name.is_empty() || name.len() > 64 || name.contains(['/', '\\', ':', '*', '?', '"', '<', '>', '|']) {
        bail!("密钥名非法（1-64 字符，禁止路径字符）");
    }
    Ok(())
}

fn private_path(name: &str) -> PathBuf {
    keystore_dir().join(format!("{name}.key"))
}
fn public_path(name: &str) -> PathBuf {
    keystore_dir().join(format!("{name}.pub"))
}

fn write_private(path: &Path, seed: &[u8; 32]) -> Result<()> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let blob = dpapi_protect(seed)?;
    std::fs::write(path, blob).with_context(|| format!("写入私钥 {} 失败", path.display()))
}

fn read_private(path: &Path) -> Result<[u8; 32]> {
    let blob = std::fs::read(path).with_context(|| format!("读取私钥 {} 失败", path.display()))?;
    let seed = dpapi_unprotect(&blob)?;
    if seed.len() != 32 {
        bail!("私钥长度非法");
    }
    let mut k = [0u8; 32];
    k.copy_from_slice(&seed);
    Ok(k)
}

// ---------- OS DPAPI ----------

/// DPAPI encryption (Windows only; other platforms store plaintext, functionally equivalent but with no at-rest protection).
#[cfg(windows)]
fn dpapi_protect(plain: &[u8]) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CryptProtectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
    };
    use windows::core::PCWSTR;

    let in_blob = CRYPT_INTEGER_BLOB { cbData: plain.len() as u32, pbData: plain.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(&in_blob, PCWSTR::null(), None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            .map_err(|e| anyhow::anyhow!("DPAPI CryptProtectData 失败: {e}"))?;
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    unsafe { let _ = LocalFree(HLOCAL(out.pbData as *mut _)); }
    Ok(bytes)
}

/// DPAPI decryption.
#[cfg(windows)]
fn dpapi_unprotect(blob: &[u8]) -> Result<Vec<u8>> {
    use windows::Win32::Foundation::{HLOCAL, LocalFree};
    use windows::Win32::Security::Cryptography::{
        CryptUnprotectData, CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN,
    };

    let in_blob = CRYPT_INTEGER_BLOB { cbData: blob.len() as u32, pbData: blob.as_ptr() as *mut u8 };
    let mut out = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(&in_blob, None, None, None, None, CRYPTPROTECT_UI_FORBIDDEN, &mut out)
            .map_err(|e| anyhow::anyhow!("DPAPI CryptUnprotectData 失败: {e}"))?;
    }
    let bytes = unsafe { std::slice::from_raw_parts(out.pbData, out.cbData as usize) }.to_vec();
    unsafe { let _ = LocalFree(HLOCAL(out.pbData as *mut _)); }
    Ok(bytes)
}

#[cfg(not(windows))]
fn dpapi_protect(plain: &[u8]) -> Result<Vec<u8>> {
    Ok(plain.to_vec()) // 非 Windows 平台降级明文（无 DPAPI）
}

#[cfg(not(windows))]
fn dpapi_unprotect(blob: &[u8]) -> Result<Vec<u8>> {
    Ok(blob.to_vec())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
