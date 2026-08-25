//! Smart aligner: three-level alignment (byte / instruction / function) that merges adjacent diffs and aligns disassembly boundaries to reduce false positives.

use crate::{diff_engine::ChangeType, diff_engine::DiffEntry, file_loader::LoadedFile, Result};
#[cfg(test)]
use crate::file_loader::{DefaultFileLoader, FileLoader};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AlignMode {
    Byte,       // uses the byte as the smallest unit
    Instruction, // aligns to disassembled instruction boundaries (requires disasm)
    Function,   // coarse alignment at function granularity
}

pub trait Aligner {
    fn align(
        &self,
        a: &LoadedFile,
        b: &LoadedFile,
        diffs: Vec<DiffEntry>,
        mode: AlignMode,
    ) -> Result<Vec<DiffEntry>>;
}

/// Default aligner: merges adjacent "Removed + Added" diffs produced by a diff engine (e.g. SlidingWindow)
/// into a single Modified entry, presenting the user's expected "in-place modification" (yellow).
/// Pure insertion (Added) and pure deletion (Removed) sequences are kept as-is.
///
/// Rationale: SlidingWindow represents a "single-byte modification" as a Removed segment (old-file range)
/// immediately followed by an Added segment (new-file range). The Aligner merges these "adjacent/small-gap"
/// Removed+Added pairs into one Modified; alternating R/A/R/A sequences expand into a single Modified
/// covering the whole range. Instruction/Function modes use larger approximate gap thresholds.
pub struct DefaultAligner {
    /// Maximum allowed gap (in bytes) between the Removed and Added coordinates; beyond this they are not merged.
    pub merge_gap: usize,
}

impl Default for DefaultAligner {
    fn default() -> Self {
        Self { merge_gap: 16 }
    }
}

impl DefaultAligner {
    pub fn new(merge_gap: usize) -> Self {
        Self { merge_gap }
    }

    /// Returns the merge gap threshold per alignment mode (heuristic): Byte uses the configured value,
    /// Instruction 16, Function 256. This is an approximate heuristic, not exact disasm/symbol boundary alignment.
    fn gap_threshold(&self, mode: AlignMode) -> i64 {
        match mode {
            AlignMode::Byte => self.merge_gap as i64,
            AlignMode::Instruction => 16,
            AlignMode::Function => 256,
        }
    }
}

/// Sort key: ascending file coordinates (Removed uses old.start, Added uses new.start, Modified uses offset).
fn coord_key(e: &DiffEntry) -> u64 {
    match e.change {
        ChangeType::Removed => e.old.as_ref().map(|r| r.start).unwrap_or(e.offset),
        ChangeType::Added => e.new.as_ref().map(|r| r.start).unwrap_or(e.offset),
        ChangeType::Modified => e.offset,
    }
}

impl Aligner for DefaultAligner {
    fn align(
        &self,
        _a: &LoadedFile,
        _b: &LoadedFile,
        diffs: Vec<DiffEntry>,
        mode: AlignMode,
    ) -> Result<Vec<DiffEntry>> {
        let mut v = diffs;
        v.sort_by_key(coord_key);
        let gap = self.gap_threshold(mode);
        let mut out: Vec<DiffEntry> = Vec::with_capacity(v.len());
        let mut i = 0;
        while i < v.len() {
            let e = &v[i];
            if e.change == ChangeType::Removed {
                // Scan forward for an Added that is adjacent or within a small gap, and merge it into a Modified.
                let mut found: Option<usize> = None;
                let j = i + 1;
                while j < v.len() {
                    match v[j].change {
                        ChangeType::Added => {
                            let rem_end = e.old.as_ref().map(|r| r.end).unwrap_or(e.offset);
                            let add_start = v[j].new.as_ref().map(|r| r.start).unwrap_or(v[j].offset);
                            let close = (add_start as i64 - rem_end as i64).abs() <= gap;
                            if close {
                                found = Some(j);
                            }
                            break; // Stop scanning at the first Added, whether or not it was merged.
                        }
                        ChangeType::Modified => break,
                        ChangeType::Removed => break, // Consecutive Removed entries form a pure-deletion segment, so do not merge.
                    }
                }
                if let Some(j) = found {
                    let removed = &v[i];
                    let added = &v[j];
                    let old = removed.old.clone().unwrap();
                    let new = added.new.clone().unwrap();
                    out.push(DiffEntry {
                        offset: old.start.min(new.start),
                        length: (old.end - old.start).max(new.end - new.start),
                        change: ChangeType::Modified,
                        old: Some(old),
                        new: Some(new),
                    });
                    i = j + 1;
                } else {
                    out.push(v[i].clone());
                    i += 1;
                }
            } else {
                // Added (not merged by a preceding Removed) or Modified: keep as-is.
                out.push(v[i].clone());
                i += 1;
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff_engine::{DefaultDiffEngine, DiffEngine, DiffStrategy};
    use std::ops::Range;

    fn md(
        change: ChangeType,
        offset: u64,
        length: u64,
        old: Option<Range<u64>>,
        new: Option<Range<u64>>,
    ) -> DiffEntry {
        DiffEntry { offset, length, change, old, new }
    }

    use std::sync::atomic::{AtomicU64, Ordering};
    static DUMMY_CTR: AtomicU64 = AtomicU64::new(0);

    /// Test LoadedFile (align does not depend on its contents; it only satisfies the signature); writes a 4-byte bin to trigger the Bin fallback.
    /// Each call uses a unique filename to avoid parallel tests writing the same temp file and causing mmap contention (ERROR_USER_MAPPED_FILE).
    fn dummy() -> LoadedFile {
        let n = DUMMY_CTR.fetch_add(1, Ordering::SeqCst);
        let p = std::env::temp_dir().join(format!("align_dummy_{}_{}.bin", std::process::id(), n));
        std::fs::write(&p, [0u8; 4]).unwrap();
        let lf = DefaultFileLoader::load(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        lf
    }

    #[test]
    fn merges_adjacent_removed_added_into_modified() {
        let diffs = vec![
            md(ChangeType::Removed, 30, 1, Some(30..31), None),
            md(ChangeType::Added, 34, 1, None, Some(34..35)),
        ];
        let r = DefaultAligner::default()
            .align(&dummy(), &dummy(), diffs, AlignMode::Byte)
            .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].change, ChangeType::Modified);
        assert_eq!(r[0].old, Some(30..31));
        assert_eq!(r[0].new, Some(34..35));
    }

    #[test]
    fn head_insert_stays_added() {
        let diffs = vec![md(ChangeType::Added, 0, 4, None, Some(0..4))];
        let r = DefaultAligner::default()
            .align(&dummy(), &dummy(), diffs, AlignMode::Byte)
            .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].change, ChangeType::Added);
    }

    #[test]
    fn tail_delete_stays_removed() {
        let diffs = vec![md(ChangeType::Removed, 60, 4, Some(60..64), None)];
        let r = DefaultAligner::default()
            .align(&dummy(), &dummy(), diffs, AlignMode::Byte)
            .unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r[0].change, ChangeType::Removed);
    }

    #[test]
    fn two_far_modifications_stay_separate() {
        let diffs = vec![
            md(ChangeType::Removed, 10, 1, Some(10..11), None),
            md(ChangeType::Added, 10, 1, None, Some(10..11)),
            md(ChangeType::Removed, 200, 1, Some(200..201), None),
            md(ChangeType::Added, 200, 1, None, Some(200..201)),
        ];
        let r = DefaultAligner::default()
            .align(&dummy(), &dummy(), diffs, AlignMode::Byte)
            .unwrap();
        let mods: Vec<_> = r.iter().filter(|e| e.change == ChangeType::Modified).collect();
        assert_eq!(mods.len(), 2);
    }

    #[test]
    fn identical_files_no_diff() {
        let a: Vec<u8> = (0..50).collect();
        let b = a.clone();
        let eng = DefaultDiffEngine;
        let d = eng
            .diff(
                &dummy_a(&a),
                &dummy_a(&b),
                DiffStrategy::SlidingWindow { window: 8, min_match: 8 },
            )
            .unwrap();
        let r = DefaultAligner::default()
            .align(&dummy(), &dummy(), d, AlignMode::Byte)
            .unwrap();
        assert!(r.is_empty());
    }

    fn dummy_a(b: &[u8]) -> LoadedFile {
        let dir = std::env::temp_dir();
        let p = dir.join(format!("align_dummy_a_{}.bin", b.len()));
        std::fs::write(&p, b).unwrap();
        let lf = DefaultFileLoader::load(&p).unwrap();
        let _ = std::fs::remove_file(&p);
        lf
    }
}
