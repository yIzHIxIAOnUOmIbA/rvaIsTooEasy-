//! Task B patch-ecosystem end-to-end tests: signing -> container roundtrip -> apply -> rollback -> tampered/unsigned rejection.

use rva_core::apply::{apply_batch, apply_patch, rollback_patch};
use rva_core::diff_engine::{DefaultDiffEngine, DiffEngine, DiffStrategy};
use rva_core::file_loader::{DefaultFileLoader, FileLoader};
use rva_core::patch_engine::{DefaultPatchEngine, Patch, PatchEngine, PatchFormat, PatchOp};
use rva_core::patch_pack::{build_revert_segments, sha256_of, PackedPatch, PatchMeta, RevertSegment};
use rva_core::signing;
use std::path::PathBuf;
use std::sync::{Mutex, MutexGuard, OnceLock};

/// RVA_KEYSTORE / RVA_PATCH_HISTORY are process-level global env vars; parallel tests would pollute
/// each other, so every test touching the keystore must run serially.
fn env_lock() -> MutexGuard<'static, ()> {
    static LOCK: OnceLock<Mutex<()>> = OnceLock::new();
    LOCK.get_or_init(|| Mutex::new(())).lock().unwrap_or_else(|e| e.into_inner())
}

fn tmp_root(tag: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("rva_eco_{tag}_{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

/// Build a patch replacing the [10,20) range (everything else is a pass-through Copy).
fn make_patch(old: &[u8], new: &[u8]) -> Patch {
    Patch {
        format: PatchFormat::Custom,
        old_size: old.len() as u64,
        new_size: new.len() as u64,
        ops: vec![
            PatchOp::Copy { from: 0, len: 10 },
            PatchOp::Insert { data: new[10..20].to_vec() },
            PatchOp::Copy { from: 20, len: (old.len() - 20) as u64 },
        ],
    }
}

fn make_packed(old: &[u8], new: &[u8], patch: &Patch) -> PackedPatch {
    let meta = PatchMeta {
        source_sha256: sha256_of(old),
        target_sha256: sha256_of(new),
        timestamp: 1_752_000_000,
        engine_version: 1,
        strategy: 0,
        entry_count: patch.ops.len() as u32,
    };
    // Snapshot of the old bytes in the replaced [10,20) range (Modified original bytes, for rollback)
    let revert = vec![RevertSegment { old_start: 10, len: 10, bytes: old[10..20].to_vec() }];
    PackedPatch::new(meta, patch, revert).unwrap()
}

#[test]
fn pack_tlv_roundtrip() {
    let old = (0u8..100).collect::<Vec<_>>();
    let mut new = old.clone();
    new[10..20].fill(0xFF);
    let patch = make_patch(&old, &new);
    let packed = make_packed(&old, &new, &patch);

    let bytes = packed.to_bytes();
    assert_eq!(&bytes[0..4], b"RVPT");

    let back = PackedPatch::from_bytes(&bytes).unwrap();
    assert_eq!(back.version, 1);
    assert_eq!(back.metadata.target_sha256, sha256_of(&new));
    assert_eq!(back.metadata.entry_count, 3);
    let patch2 = back.to_patch().unwrap();
    assert_eq!(patch2.ops.len(), patch.ops.len());

    // content_bytes naturally excludes the signature area: adding a signature leaves content unchanged
    let c0 = packed.content_bytes();
    let mut signed = packed;
    signed.add_signature(rva_core::patch_pack::PatchSignature {
        fingerprint: [0u8; 32],
        signature: [0u8; 64],
    });
    assert_eq!(c0, signed.content_bytes());

    // Corrupted data is rejected
    let mut bad = bytes.clone();
    bad[2] ^= 0xFF;
    assert!(PackedPatch::from_bytes(&bad).is_err());
    let trunc = bytes[..bytes.len() - 10].to_vec();
    assert!(PackedPatch::from_bytes(&trunc).is_err());
}

#[test]
fn sign_verify_apply_rollback_chain() {
    let _guard = env_lock();
    let root = tmp_root("chain");
    std::env::set_var("RVA_KEYSTORE", root.join("keystore"));
    std::env::set_var("RVA_PATCH_HISTORY", root.join("history"));

    // 1. Key generation and listing
    let k1 = signing::generate_keypair("dev").unwrap();
    assert!(k1.has_private);
    assert_eq!(k1.fingerprint_hex.len(), 64);
    let keys = signing::list_keys().unwrap();
    assert_eq!(keys.len(), 1);
    assert_eq!(keys[0].name, "dev");

    // 2. Import a trusted public key (simulating a distributor importing the publisher's key)
    let pub_bytes = signing::public_key_bytes("dev").unwrap();
    let imported = signing::import_public("vendor", &pub_bytes).unwrap();
    assert!(!imported.has_private);
    assert_eq!(imported.fingerprint_hex, k1.fingerprint_hex);
    assert_eq!(signing::list_keys().unwrap().len(), 2);

    // 3. Data and patch
    let old = (0u8..100).collect::<Vec<_>>();
    let mut new = old.clone();
    new[10..20].fill(0xFF);
    let patch = make_patch(&old, &new);
    let mut packed = make_packed(&old, &new, &patch);

    // 4. Sign (the fingerprint must equal dev's public-key fingerprint)
    let sig = signing::sign_bytes("dev", &packed.content_bytes()).unwrap();
    let mut dev_fp = [0u8; 32];
    hex_decode(&k1.fingerprint_hex, &mut dev_fp);
    assert_eq!(sig.fingerprint, dev_fp);
    packed.add_signature(sig);

    // 5. Byte roundtrip + signature verification (fingerprint lookup in the keystore, including the imported-key path)
    let bytes = packed.to_bytes();
    let packed2 = PackedPatch::from_bytes(&bytes).unwrap();
    assert_eq!(packed2.signatures.len(), 1);
    let content = packed2.content_bytes();
    assert!(signing::verify_by_fingerprint(&content, &packed2.signatures[0]).unwrap());

    // 6. Tamper detection: flipping any content byte fails verification
    let mut tampered = content.clone();
    tampered[6] ^= 1;
    assert!(!signing::verify_by_fingerprint(&tampered, &packed2.signatures[0]).unwrap());

    // 7. Unknown issuer: after clearing the keystore, verification fails (no matching public key)
    signing::delete_key("dev").unwrap();
    signing::delete_key("vendor").unwrap();
    assert!(!signing::verify_by_fingerprint(&content, &packed2.signatures[0]).unwrap());
    // Re-import the original public key to restore trust so the later apply/rollback passes verification
    signing::import_public("dev", &pub_bytes).unwrap();

    // 8. Apply the patch -> rebuild target and verify SHA256
    let src = root.join("a.bin");
    let out = root.join("b.bin");
    std::fs::write(&src, &old).unwrap();
    let res = apply_patch(&src, &packed2, &out).unwrap();
    assert!(res.ok, "{}", res.message);
    assert_eq!(std::fs::read(&out).unwrap(), new);

    // 9. Roll back the patch -> restore the source
    let back = root.join("a2.bin");
    let res2 = rollback_patch(&out, &packed2, &back).unwrap();
    assert!(res2.ok, "{}", res2.message);
    assert_eq!(std::fs::read(&back).unwrap(), old);

    // 10. History log written (apply + rollback >= 2 entries)
    let hist = std::fs::read_dir(root.join("history")).unwrap().count();
    assert!(hist >= 2, "历史日志条数 = {hist}");

    // 11. Unsigned patches are rejected from applying
    let mut unsigned = packed2.clone();
    unsigned.signatures.clear();
    let res3 = apply_patch(&src, &unsigned, &root.join("c.bin"));
    assert!(res3.is_err());

    // 12. Apply is rejected after the source file is tampered with
    std::fs::write(&src, b"tampered content that breaks sha256").unwrap();
    assert!(apply_patch(&src, &packed2, &root.join("d.bin")).is_err());

    // 13. Rollback precondition check: rejected when the current file does not match the target
    assert!(rollback_patch(&src, &packed2, &root.join("e.bin")).is_err());
}

/// P0 patch-chain roundtrip: real diff engine -> patch generation -> container -> apply;
/// re-diffing (apply output, target) must be empty; after rollback, re-diffing (restored, source)
/// must also be empty.
#[test]
fn patch_chain_rediff_is_empty() {
    let _guard = env_lock();
    let root = tmp_root("rediff");
    let kd = root.join("keystore");
    let hd = root.join("history");
    std::env::set_var("RVA_KEYSTORE", &kd);
    std::env::set_var("RVA_PATCH_HISTORY", &hd);

    // 1. Build old/new with a real difference (4 bytes inserted at the head -> the rest shifts as a whole)
    let old: Vec<u8> = (0u8..=255).cycle().take(512).collect();
    let mut new = old.clone();
    new.splice(32..32, [0xEE, 0xEE, 0xEE, 0xEE]);
    new[100..120].fill(0xAB);
    assert_ne!(old, new);

    let src = root.join("a.bin");
    let target = root.join("b.bin");
    std::fs::write(&src, &old).unwrap();
    std::fs::write(&target, &new).unwrap();

    // 2. Real pipeline: diff -> generate -> container -> sign
    let fa = DefaultFileLoader::load(&src).unwrap();
    let fb = DefaultFileLoader::load(&target).unwrap();
    let diffs = DefaultDiffEngine
        .diff(&fa, &fb, DiffStrategy::SlidingWindow { window: 8, min_match: 8 })
        .unwrap();
    assert!(!diffs.is_empty(), "构造的差异不应为空");

    let patch = DefaultPatchEngine::generate(&fa, &fb, &diffs, PatchFormat::Custom).unwrap();
    let meta = PatchMeta {
        source_sha256: sha256_of(&old),
        target_sha256: sha256_of(&new),
        timestamp: 1_752_100_000,
        engine_version: 1,
        strategy: 0,
        entry_count: patch.ops.len() as u32,
    };
    let revert = build_revert_segments(&old, &patch);
    assert!(!revert.is_empty(), "回滚段不应为空");
    let mut packed = PackedPatch::new(meta, &patch, revert).unwrap();
    signing::generate_keypair("dev").unwrap();
    let sig = signing::sign_bytes("dev", &packed.content_bytes()).unwrap();
    packed.add_signature(sig);
    assert!(signing::verify_by_fingerprint(&packed.content_bytes(), &packed.signatures[0]).unwrap());

    // 3. apply -> rebuild the target; re-diffing (output, target) must be empty
    let out = root.join("out.bin");
    let res = apply_patch(&src, &packed, &out).unwrap();
    assert!(res.ok, "{}", res.message);
    let fo = DefaultFileLoader::load(&out).unwrap();
    let rediff = DefaultDiffEngine
        .diff(&fo, &fb, DiffStrategy::SlidingWindow { window: 8, min_match: 8 })
        .unwrap();
    assert!(
        rediff.is_empty(),
        "apply 后重新 diff 应无差异，实际 {} 条: {rediff:?}",
        rediff.len()
    );

    // 4. Rollback -> restore the source; re-diffing (restored, source) must be empty
    let back = root.join("back.bin");
    let res2 = rollback_patch(&out, &packed, &back).unwrap();
    assert!(res2.ok, "{}", res2.message);
    let fr = DefaultFileLoader::load(&back).unwrap();
    let rediff2 = DefaultDiffEngine
        .diff(&fr, &fa, DiffStrategy::SlidingWindow { window: 8, min_match: 8 })
        .unwrap();
    assert!(
        rediff2.is_empty(),
        "回滚后重新 diff 应无差异，实际 {} 条: {rediff2:?}",
        rediff2.len()
    );
}

/// P1 batch apply: a two-stage A->B->C patch chain, verifying
/// (1) sequential apply rebuilds the final target; (2) idempotency (skipped when already latest);
/// (3) resume from a mid-state; (4) rejection when the chain is broken.
#[test]
fn patch_batch_apply_chain() {
    let _guard = env_lock();
    let root = tmp_root("batch");
    let kd = root.join("keystore");
    let hd = root.join("history");
    std::env::set_var("RVA_KEYSTORE", &kd);
    std::env::set_var("RVA_PATCH_HISTORY", &hd);
    signing::generate_keypair("dev").unwrap();

    // Three-state files: A base -> B (tail edit + head insert) -> C (middle edit)
    let a: Vec<u8> = (0u8..=255).cycle().take(1024).collect();
    let mut b = a.clone();
    b[1000..1004].fill(0x11);
    b.splice(0..0, [0xEE, 0xEE, 0xEE]);
    let mut c = b.clone();
    c[500..540].fill(0xAB);

    // Real diff -> two signed patches
    let src_a = root.join("a.bin");
    let src_b = root.join("b.bin");
    let src_c = root.join("c.bin");
    std::fs::write(&src_a, &a).unwrap();
    std::fs::write(&src_b, &b).unwrap();
    std::fs::write(&src_c, &c).unwrap();

    let make_signed = |old: &[u8], new: &[u8], old_path: &std::path::Path, new_path: &std::path::Path| -> PackedPatch {
        let fo = DefaultFileLoader::load(old_path).unwrap();
        let fnw = DefaultFileLoader::load(new_path).unwrap();
        let diffs = DefaultDiffEngine
            .diff(&fo, &fnw, DiffStrategy::SlidingWindow { window: 8, min_match: 8 })
            .unwrap();
        assert!(!diffs.is_empty());
        let patch = DefaultPatchEngine::generate(&fo, &fnw, &diffs, PatchFormat::Custom).unwrap();
        let meta = PatchMeta {
            source_sha256: sha256_of(old),
            target_sha256: sha256_of(new),
            timestamp: 1_752_200_000,
            engine_version: 1,
            strategy: 0,
            entry_count: patch.ops.len() as u32,
        };
        let revert = build_revert_segments(old, &patch);
        let mut packed = PackedPatch::new(meta, &patch, revert).unwrap();
        let sig = signing::sign_bytes("dev", &packed.content_bytes()).unwrap();
        packed.add_signature(sig);
        packed
    };
    let p1 = make_signed(&a, &b, &src_a, &src_b);
    let p2 = make_signed(&b, &c, &src_b, &src_c);

    // (1) Sequential apply of A + [p1, p2] -> C
    let out = root.join("out.bin");
    let res = apply_batch(&src_a, &[&p1, &p2], &out).unwrap();
    assert!(res.ok, "{}", res.message);
    assert_eq!(std::fs::read(&out).unwrap(), c, "批量应用结果应为 C");

    // (2) Idempotency: already the latest target; succeeds without overwriting the file
    let before = std::fs::read(&out).unwrap();
    let res2 = apply_batch(&out, &[&p1, &p2], &out).unwrap();
    assert!(res2.ok, "幂等应成功: {}", res2.message);
    assert!(res2.message.contains("幂等"), "应提示幂等跳过, 实际: {}", res2.message);
    assert_eq!(std::fs::read(&out).unwrap(), before, "幂等不应改变文件");

    // (3) Resume: continue from the mid-state B with [p1, p2] -> C
    let out2 = root.join("out2.bin");
    let res3 = apply_batch(&src_b, &[&p1, &p2], &out2).unwrap();
    assert!(res3.ok, "{}", res3.message);
    assert_eq!(std::fs::read(&out2).unwrap(), c, "断点续传结果应为 C");
    assert!(res3.message.contains("1 个补丁"), "应从 p2 续传, 实际: {}", res3.message);

    // (4) Broken chain: applying [p2, p1] out of order to A -> rejected
    let out3 = root.join("out3.bin");
    let res4 = apply_batch(&src_a, &[&p2, &p1], &out3);
    assert!(res4.is_err(), "乱序补丁链必须拒绝");
}

fn hex_decode(s: &str, out: &mut [u8; 32]) {
    for i in 0..32 {
        out[i] = u8::from_str_radix(&s[i * 2..i * 2 + 2], 16).unwrap();
    }
}
