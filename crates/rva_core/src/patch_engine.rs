//! Patch engine: generates a self-contained patch (custom format) from diff results and can apply it
//! to the target file.
//!
//! Design notes (deviation from the original blueprint stub): the stub `generate(diffs) -> Vec<u8>`
//! carried neither the original nor the target bytes, so it could not produce a self-contained patch
//! (apply needs the new file's incremental bytes to rebuild `new`). Following the Phase 3 flexibility
//! principle, this implementation changed to `generate(old, new, diffs, format)`, embedding each
//! Added/Modified incremental chunk into the patch so that `apply(old, patch)` can rebuild `new`
//! without the target file. Custom is the only current implementation; Xdelta3 is a TODO (the Rust
//! ecosystem bindings are immature, and the in-house format already satisfies end-to-end needs).

use crate::diff_engine::DiffEntry;
use crate::file_loader::LoadedFile;
use crate::RvaError;
use crate::Result;
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PatchFormat {
    Custom, // 自定义紧凑格式（内嵌增量字节）
    Xdelta3, // 标准 xdelta3 二进制差量（TODO）
}

/// Edit instruction: Copy preserves an old-file range; Insert writes embedded new bytes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PatchOp {
    Copy { from: u64, len: u64 },
    Insert { data: Vec<u8> },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Patch {
    pub format: PatchFormat,
    pub old_size: u64,
    pub new_size: u64,
    pub ops: Vec<PatchOp>,
}

pub trait PatchEngine {
    /// Generate a self-contained patch from the diff plus the old/target file contents.
    fn generate(old: &LoadedFile, new: &LoadedFile, diffs: &[DiffEntry], format: PatchFormat) -> Result<Patch>;
    /// Apply the patch to the old file, writing the rebuilt result to the out path.
    fn apply(old: &LoadedFile, patch: &Patch, out: &Path) -> Result<()>;
    /// Serialize to bytes (JSON, readable and verifiable).
    fn serialize(patch: &Patch) -> Result<Vec<u8>>;
    /// Deserialize from bytes.
    fn deserialize(bytes: &[u8]) -> Result<Patch>;
}

pub struct DefaultPatchEngine;

impl DefaultPatchEngine {
    /// Encode diffs into an ordered Copy/Insert sequence (advancing by old-file coordinates).
    /// Requires diffs to be "faithful": Removed = old bytes absent from new; Added = new bytes
    /// absent from old; Modified = replacement. The sliding window produces faithful diffs for
    /// head inserts / tail deletes / single modifications under a fine window.
    fn build_ops(old: &[u8], new: &[u8], diffs: &[DiffEntry]) -> Vec<PatchOp> {
        use crate::diff_engine::ChangeType;

        let mut items: Vec<(u64, &DiffEntry)> = diffs
            .iter()
            .map(|e| {
                let old_start = e.old.as_ref().map(|r| r.start).unwrap_or(e.offset);
                (old_start, e)
            })
            .collect();
        items.sort_by_key(|(s, _)| *s);

        let mut ops = Vec::new();
        let mut old_pos: u64 = 0;
        for (old_start, e) in items {
            if old_start > old_pos {
                ops.push(PatchOp::Copy { from: old_pos, len: old_start - old_pos });
            }
            match e.change {
                ChangeType::Removed => {
                    if let Some(r) = &e.old {
                        old_pos = r.end; // 丢弃旧区间
                    }
                }
                ChangeType::Modified => {
                    if let Some(r) = &e.old {
                        old_pos = r.end;
                    }
                    if let Some(nr) = &e.new {
                        ops.push(PatchOp::Insert {
                            data: new[nr.start as usize..nr.end as usize].to_vec(),
                        });
                    }
                }
                ChangeType::Added => {
                    // The insertion point's old-file coordinate is recorded by a zero-length old range;
                    // after an insert the old pointer advances to that point, so later Copy instructions
                    // never re-copy from 0 when multiple inserts exist.
                    if let Some(r) = &e.old {
                        old_pos = r.start;
                    }
                    if let Some(nr) = &e.new {
                        ops.push(PatchOp::Insert {
                            data: new[nr.start as usize..nr.end as usize].to_vec(),
                        });
                    }
                }
            }
        }
        if old_pos < old.len() as u64 {
            ops.push(PatchOp::Copy { from: old_pos, len: old.len() as u64 - old_pos });
        }
        ops
    }
}

impl PatchEngine for DefaultPatchEngine {
    fn generate(old: &LoadedFile, new: &LoadedFile, diffs: &[DiffEntry], format: PatchFormat) -> Result<Patch> {
        if format == PatchFormat::Xdelta3 {
            return Err(RvaError::Patch("Xdelta3 backend not implemented yet; use Custom".into()));
        }
        let ops = Self::build_ops(old.data.as_ref(), new.data.as_ref(), diffs);
        Ok(Patch {
            format,
            old_size: old.meta.size,
            new_size: new.meta.size,
            ops,
        })
    }

    fn apply(old: &LoadedFile, patch: &Patch, out: &Path) -> Result<()> {
        let mut result = Vec::with_capacity(patch.new_size as usize);
        for op in &patch.ops {
            match op {
                PatchOp::Copy { from, len } => {
                    let f = *from as usize;
                    let l = *len as usize;
                    if f + l > old.data.len() {
                        return Err(RvaError::Patch(format!(
                            "copy out of range: {}..{} > {}",
                            f,
                            f + l,
                            old.data.len()
                        )));
                    }
                    result.extend_from_slice(&old.data[f..f + l]);
                }
                PatchOp::Insert { data } => result.extend_from_slice(data),
            }
        }
        std::fs::write(out, &result)?;
        Ok(())
    }

    fn serialize(patch: &Patch) -> Result<Vec<u8>> {
        serde_json::to_vec(patch).map_err(|e| RvaError::Patch(e.to_string()))
    }

    fn deserialize(bytes: &[u8]) -> Result<Patch> {
        serde_json::from_slice(bytes).map_err(|e| RvaError::Patch(e.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_engine::{DefaultDiffEngine, DiffEngine, DiffStrategy};
    use crate::file_loader::{DefaultFileLoader, FileLoader};
    use std::ops::Range;

    fn load_tmp(bytes: &[u8], name: &str) -> LoadedFile {
        let p = std::env::temp_dir().join(name);
        std::fs::write(&p, bytes).unwrap();
        DefaultFileLoader::load(&p).unwrap()
    }

    fn roundtrip(name: &str, a: &[u8], b: &[u8]) {
        let fa = load_tmp(a, &format!("pe_a_{}.bin", name));
        let fb = load_tmp(b, &format!("pe_b_{}.bin", name));
        let engine = DefaultDiffEngine;
        let diffs = engine
            .diff(&fa, &fb, DiffStrategy::SlidingWindow { window: 8, min_match: 8 })
            .unwrap();
        let patch = DefaultPatchEngine::generate(&fa, &fb, &diffs, PatchFormat::Custom).unwrap();
        let out = std::env::temp_dir().join(format!("pe_out_{}.bin", name));
        DefaultPatchEngine::apply(&fa, &patch, &out).unwrap();
        let got = std::fs::read(&out).unwrap();
        assert_eq!(got, b, "roundtrip mismatch for {}", name);

        // serialize/deserialize roundtrip
        let bytes = DefaultPatchEngine::serialize(&patch).unwrap();
        let patch2 = DefaultPatchEngine::deserialize(&bytes).unwrap();
        let out2 = std::env::temp_dir().join(format!("pe_out2_{}.bin", name));
        DefaultPatchEngine::apply(&fa, &patch2, &out2).unwrap();
        let got2 = std::fs::read(&out2).unwrap();
        assert_eq!(got2, b, "roundtrip after serialize mismatch for {}", name);

        let _ = std::fs::remove_file(&out);
        let _ = std::fs::remove_file(&out2);
    }

    #[test]
    fn roundtrip_identical() {
        let a: Vec<u8> = (0..64).collect();
        roundtrip("ident", &a.clone(), &a);
    }

    #[test]
    fn roundtrip_head_insert() {
        let a: Vec<u8> = (0..64).collect();
        let mut b = vec![100, 101, 102, 103];
        b.extend_from_slice(&a);
        roundtrip("head", &a, &b);
    }

    #[test]
    fn roundtrip_tail_delete() {
        let a: Vec<u8> = (0..64).collect();
        let b = a[..60].to_vec();
        roundtrip("tail", &a, &b);
    }

    #[test]
    fn roundtrip_modify_one() {
        let a: Vec<u8> = (0..64).collect();
        let mut b = a.clone();
        b[30] = 255;
        roundtrip("mod", &a, &b);
    }

    #[test]
    fn xdelta3_rejected() {
        let a: Vec<u8> = (0..16).collect();
        let fa = load_tmp(&a, "pe_xd_a.bin");
        let fb = load_tmp(&a, "pe_xd_b.bin");
        let r = DefaultPatchEngine::generate(&fa, &fb, &[], PatchFormat::Xdelta3);
        assert!(r.is_err());
    }

    // Compile-time only check: ensure DiffEntry fields are accessible (old/new are Option<Range<u64>>)
    #[test]
    fn diff_entry_shape_compiles() {
        let _e = DiffEntry {
            offset: 0,
            length: 1,
            change: crate::diff_engine::ChangeType::Added,
            old: None,
            new: Some(Range { start: 0, end: 1 }),
        };
    }
}
