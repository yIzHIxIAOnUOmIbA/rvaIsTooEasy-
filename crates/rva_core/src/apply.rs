//! Patch application and rollback (Task B: patch ecosystem moat).
//!
//! apply_patch verification chain: source SHA256 comparison -> at least one valid signature ->
//! rebuild target -> target SHA256 verification -> write history log. rollback_patch reverse-rebuilds
//! the source file. The history log keeps the most recent 50 entries.

use crate::patch_engine::{Patch, PatchOp};
use crate::patch_pack::{sha256_of, PackedPatch};
use crate::signing;
use anyhow::{bail, Context, Result};
use std::path::{Path, PathBuf};

/// Apply/rollback result (public DTO).
#[derive(Debug, Clone, serde::Serialize)]
pub struct ApplyResult {
    pub ok: bool,
    pub message: String,
    pub source_sha256: String,
    pub target_sha256: String,
    pub signed_by: Option<String>,
}

/// Apply a patch: source + packed -> out.
pub fn apply_patch(source: &Path, packed: &PackedPatch, out: &Path) -> Result<ApplyResult> {
    let old = std::fs::read(source).with_context(|| format!("failed to read source file {}", source.display()))?;
    let src_hash = sha256_of(&old);
    if src_hash != packed.metadata.source_sha256 {
        bail!("source verification failed: SHA256 does not match patch metadata (file may have been modified)");
    }

    let signed_by = verify_signed(packed)?;
    let patch = packed.to_patch()?;
    let new_bytes = rebuild_new(&old, &patch)?;
    if sha256_of(&new_bytes) != packed.metadata.target_sha256 {
        bail!("target rebuild verification failed: SHA256 does not match patch metadata");
    }
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out, &new_bytes).with_context(|| format!("failed to write target file {}", out.display()))?;

    let res = ApplyResult {
        ok: true,
        message: format!("patch applied successfully ({} signature(s), issuer: {})", packed.signatures.len(), signed_by),
        source_sha256: hex(&src_hash),
        target_sha256: hex(&packed.metadata.target_sha256),
        signed_by: Some(signed_by),
    };
    write_history("apply", source, out, &res)?;
    Ok(res)
}

/// Apply a batch patch chain: source + patches[0..] -> out.
///
/// Semantics:
/// - Chain verification: `patches[i].source_sha256` must equal the previous patch's target (or the source file);
/// - Idempotency: if the source file already equals the final target SHA256, return success immediately (no re-apply);
/// - Resume from breakpoint: if the source file exactly equals some intermediate patch target, continue applying the rest.
pub fn apply_batch(source: &Path, patches: &[&PackedPatch], out: &Path) -> Result<ApplyResult> {
    if patches.is_empty() {
        bail!("batch apply: patch list is empty");
    }
    let mut cur = std::fs::read(source).with_context(|| format!("failed to read source file {}", source.display()))?;
    let final_target = patches.last().unwrap().metadata.target_sha256;
    let src_hash = sha256_of(&cur);

    // Idempotency: already at the latest target
    if src_hash == final_target {
        let res = ApplyResult {
            ok: true,
            message: "already up to date, nothing to apply (idempotent skip)".to_string(),
            source_sha256: hex(&src_hash),
            target_sha256: hex(&final_target),
            signed_by: None,
        };
        return Ok(res);
    }

    // Chain continuity: patches[i].source must equal patches[i-1].target
    for (i, p) in patches.iter().enumerate().skip(1) {
        if p.metadata.source_sha256 != patches[i - 1].metadata.target_sha256 {
            bail!("patch chain discontinuity: patch {} source does not match patch {} target SHA256", i + 1, i);
        }
    }

    // Resume from breakpoint: locate the first patch to start (src matches chain head source, or some intermediate target)
    let start = if src_hash == patches[0].metadata.source_sha256 {
        0
    } else {
        let mut found = None;
        for i in 1..patches.len() {
            if src_hash == patches[i - 1].metadata.target_sha256 {
                found = Some(i);
                break;
            }
        }
        found.ok_or_else(|| anyhow::anyhow!("batch apply: source file does not match any state in this patch chain (SHA256 mismatch)"))?
    };

    let mut applied = 0usize;
    let mut signed_by = None;
    for p in &patches[start..] {
        let pb = p.to_patch()?;
        let new_bytes = rebuild_new(&cur, &pb)?;
        if sha256_of(&new_bytes) != p.metadata.target_sha256 {
            bail!("batch apply: target rebuild verification failed for patch {} in chain (SHA256 mismatch)", start + applied + 1);
        }
        signed_by = Some(verify_signed(p)?);
        cur = new_bytes;
        applied += 1;
    }

    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out, &cur).with_context(|| format!("failed to write target file {}", out.display()))?;

    let res = ApplyResult {
        ok: true,
        message: format!("batch apply succeeded: {} patch(es), issuer: {}", applied, signed_by.as_deref().unwrap_or_default()),
        source_sha256: hex(&src_hash),
        target_sha256: hex(&final_target),
        signed_by,
    };
    write_history("apply_batch", source, out, &res)?;
    Ok(res)
}

/// Rollback a patch: current(applied) + packed -> out(restored to source).
pub fn rollback_patch(current: &Path, packed: &PackedPatch, out: &Path) -> Result<ApplyResult> {
    let cur = std::fs::read(current).with_context(|| format!("failed to read current file {}", current.display()))?;
    let cur_hash = sha256_of(&cur);
    if cur_hash != packed.metadata.target_sha256 {
        bail!("rollback precondition failed: current file SHA256 does not match patch target (may have been modified again)");
    }

    let signed_by = verify_signed(packed)?;
    let patch = packed.to_patch()?;
    let old_bytes = rebuild_old(&cur, &patch, &packed.revert_segments)?;
    if sha256_of(&old_bytes) != packed.metadata.source_sha256 {
        bail!("rollback rebuild verification failed: SHA256 does not match patch source");
    }
    if let Some(dir) = out.parent() {
        std::fs::create_dir_all(dir)?;
    }
    std::fs::write(out, &old_bytes).with_context(|| format!("failed to write rollback file {}", out.display()))?;

    let res = ApplyResult {
        ok: true,
        message: format!("patch rollback succeeded (issuer: {signed_by})"),
        source_sha256: hex(&packed.metadata.source_sha256),
        target_sha256: hex(&cur_hash),
        signed_by: Some(signed_by),
    };
    write_history("rollback", current, out, &res)?;
    Ok(res)
}

/// Verify at least one valid signature exists; return the first valid issuer fingerprint.
fn verify_signed(packed: &PackedPatch) -> Result<String> {
    if packed.signatures.is_empty() {
        bail!("patch is not signed; refusing to apply (ecosystem requires signed patches)");
    }
    let content = packed.content_bytes();
    for sig in &packed.signatures {
        if signing::verify_by_fingerprint(&content, sig)? {
            return Ok(hex(&sig.fingerprint));
        }
    }
    bail!("patch signature verification failed: no valid signature matches any public key in the keystore");
}

/// Rebuild the target file forward (Copy passes through old bytes, Insert writes embedded new bytes).
fn rebuild_new(old: &[u8], patch: &Patch) -> Result<Vec<u8>> {
    let mut new = Vec::with_capacity(patch.new_size as usize);
    for op in &patch.ops {
        match op {
            PatchOp::Copy { from, len } => {
                let s = *from as usize;
                let l = *len as usize;
                if s + l > old.len() {
                    bail!("patch Copy out of bounds (from {s} len {l} > old {})", old.len());
                }
                new.extend_from_slice(&old[s..s + l]);
            }
            PatchOp::Insert { data } => new.extend_from_slice(data),
        }
    }
    if new.len() != patch.new_size as usize {
        bail!("target rebuild length mismatch: {} (expected {})", new.len(), patch.new_size);
    }
    Ok(new)
}

/// Reverse-rebuild the source file: Copy ranges are passed through from the current file (placed at old coordinates),
/// and old bytes of Modified/Removed are backfilled from the container revert snapshot (spec: Modified carries original/replaced bytes).
fn rebuild_old(cur: &[u8], patch: &Patch, revert: &[crate::patch_pack::RevertSegment]) -> Result<Vec<u8>> {
    let mut old = vec![0u8; patch.old_size as usize];
    let mut consume = 0usize;
    for op in &patch.ops {
        match op {
            PatchOp::Copy { from, len } => {
                let f = *from as usize;
                let l = *len as usize;
                if consume + l > cur.len() {
                    bail!("rollback Copy out of bounds (consume {consume} len {l} > cur {})", cur.len());
                }
                old[f..f + l].copy_from_slice(&cur[consume..consume + l]);
                consume += l;
            }
            PatchOp::Insert { data } => consume += data.len(),
        }
    }
    if consume != cur.len() {
        bail!("rollback consume position mismatch: {consume} (expected {})", cur.len());
    }
    for seg in revert {
        let s = seg.old_start as usize;
        let l = seg.len as usize;
        if s + l > old.len() || seg.bytes.len() != l {
            bail!("rollback snapshot segment out of bounds (start {s} len {l} > old {})", old.len());
        }
        old[s..s + l].copy_from_slice(&seg.bytes);
    }
    Ok(old)
}

// ---------- History log ----------

pub fn history_dir() -> PathBuf {
    if let Ok(dir) = std::env::var("RVA_PATCH_HISTORY") {
        return PathBuf::from(dir);
    }
    let base = if cfg!(windows) {
        std::env::var("APPDATA").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from("."))
    } else {
        std::env::var("HOME").map(PathBuf::from).unwrap_or_else(|_| PathBuf::from(".")).join(".rvacompare")
    };
    base.join("RVACompare").join("patch_history")
}

fn write_history(kind: &str, source: &Path, out: &Path, res: &ApplyResult) -> Result<()> {
    let dir = history_dir();
    std::fs::create_dir_all(&dir)?;
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0);
    let record = serde_json::json!({
        "kind": kind,
        "ts_ms": ts,
        "source": source.display().to_string(),
        "out": out.display().to_string(),
        "source_sha256": res.source_sha256,
        "target_sha256": res.target_sha256,
        "signed_by": res.signed_by,
        "ok": res.ok,
        "message": res.message,
    });
    let path = dir.join(format!("{}_{}.json", ts, kind));
    std::fs::write(&path, serde_json::to_vec_pretty(&record)?)?;
    trim_history(&dir, 50)?;
    Ok(())
}

/// Keep only the most recent `keep` log entries (sorted by filename timestamp prefix).
fn trim_history(dir: &Path, keep: usize) -> Result<()> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir)?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    files.sort();
    if files.len() > keep {
        for old in files.drain(..files.len() - keep) {
            let _ = std::fs::remove_file(&old);
        }
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
