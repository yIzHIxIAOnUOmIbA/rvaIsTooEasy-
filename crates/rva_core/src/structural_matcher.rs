//! Function-level structure matching: disassembly normalization + sliding match over instruction sequences.
//!
//! For "recompilation causes global byte drift" scenarios (where byte-level chunked/sliding fails):
//! disassemble the executable segments, produce a normalization signature for every instruction with
//! relocation-sensitive fields masked, then sliding-match over the signature sequences and output
//! code-segment-level diffs.
//! Note: only executable segments (.text etc.) are covered; data-segment diffs need chunked/sliding as well.

use crate::diff_engine::{ChangeType, DiffEntry};
use crate::disasm::{disassemble, Insn};
use crate::file_loader::{Arch, Segment};
use crate::{err, LoadedFile, Result};

fn xxh(buf: &[u8]) -> u64 {
    xxhash_rust::xxh64::xxh64(buf, 0x9E37_79B9_7F4A_7C15)
}

/// Structure-matching entry point. `min_run` is the shortest match anchor (instruction count, suggested >= 4).
pub fn structural_diff(a: &LoadedFile, b: &LoadedFile, min_run: usize) -> Result<Vec<DiffEntry>> {
    let bitness = match (a.meta.arch, b.meta.arch) {
        (Arch::X86_64, Arch::X86_64) => 64,
        (Arch::X86, Arch::X86) => 32,
        _ => return Err(err("structural diff 仅支持两文件同为 x86 或 x64")),
    };
    let k = min_run.max(1);

    let a_exec = exec_segments(&a.meta.segments);
    let b_exec = exec_segments(&b.meta.segments);

    let mut out = Vec::new();

    // Pair up executable segments with the same name for matching
    for (name, a_seg) in &a_exec {
        if let Some((_, b_seg)) = b_exec.iter().find(|(n, _)| n == name) {
            let a_insns = disassemble_seg(a, a_seg, bitness);
            let b_insns = disassemble_seg(b, b_seg, bitness);
            let matches = match_insns(&a_insns, &b_insns, k);
            out.extend(build_entries(&a_insns, &b_insns, &matches, a_seg, b_seg));
        } else {
            // Executable segment only in a -> Removed
            out.push(DiffEntry {
                offset: a_seg.file_offset,
                length: a_seg.size,
                change: ChangeType::Removed,
                old: Some(a_seg.file_offset..a_seg.file_offset + a_seg.size),
                new: None,
            });
        }
    }
    // Executable segment only in b -> Added
    for (name, b_seg) in &b_exec {
        if !a_exec.iter().any(|(n, _)| n == name) {
            out.push(DiffEntry {
                offset: b_seg.file_offset,
                length: b_seg.size,
                change: ChangeType::Added,
                old: Some(b_seg.file_offset..b_seg.file_offset),
                new: Some(b_seg.file_offset..b_seg.file_offset + b_seg.size),
            });
        }
    }

    out.sort_by_key(|e| e.offset);
    Ok(out)
}

fn exec_segments(segs: &[Segment]) -> Vec<(String, &Segment)> {
    let mut v: Vec<(String, &Segment)> = segs
        .iter()
        .filter(|s| s.is_executable && s.size > 0)
        .map(|s| (s.name.trim_matches('\0').to_ascii_lowercase(), s))
        .collect();
    v.sort_by(|x, y| x.0.cmp(&y.0).then(x.1.file_offset.cmp(&y.1.file_offset)));
    v
}

fn disassemble_seg(f: &LoadedFile, seg: &Segment, bitness: u32) -> Vec<Insn> {
    let s = seg.file_offset as usize;
    let e = ((seg.file_offset + seg.size) as usize).min(f.data.len());
    if s >= e {
        return Vec::new();
    }
    disassemble(&f.data[s..e], seg.vaddr, s, bitness)
}

/// Instruction-level sliding match: find the longest matching block over normalized signature sequences.
/// Returns `(a_insn_idx, b_insn_idx, len_in_insns)`.
fn match_insns(a: &[Insn], b: &[Insn], k: usize) -> Vec<(usize, usize, usize)> {
    if a.len() < k || b.len() < k {
        return Vec::new();
    }
    let a_sigs: Vec<u64> = a.iter().map(|i| i.sig).collect();
    let b_sigs: Vec<u64> = b.iter().map(|i| i.sig).collect();

    // Sorted-array index (reusing the performance lesson from byte-level sliding; avoids HashMap heap allocation/rehash)
    let n = a_sigs.len() - k + 1;
    let mut index: Vec<(u64, u32)> = Vec::with_capacity(n);
    for i in 0..n {
        index.push((win_hash(&a_sigs, i, k), i as u32));
    }
    index.sort_unstable();

    let mut matches: Vec<(usize, usize, usize)> = Vec::new();
    let mut i = 0;
    while i + k <= b_sigs.len() {
        let h = win_hash(&b_sigs, i, k);
        let lo = index.partition_point(|e| e.0 < h);
        let hi = index.partition_point(|e| e.0 <= h);
        if lo < hi {
            let mut best: Option<(usize, usize)> = None;
            for m in lo..hi.min(lo + 128) {
                let op = index[m].1 as usize;
                let mut len = k;
                while i + len < b_sigs.len()
                    && op + len < a_sigs.len()
                    && b_sigs[i + len] == a_sigs[op + len]
                {
                    len += 1;
                }
                if best.map_or(true, |(_, bl)| len > bl) {
                    best = Some((op, len));
                }
            }
            if let Some((op, len)) = best {
                let (mut op, mut np, mut len) = (op, i, len);
                while op > 0 && np > 0 && a_sigs[op - 1] == b_sigs[np - 1] {
                    op -= 1;
                    np -= 1;
                    len += 1;
                }
                matches.push((op, np, len));
                i = np + len;
                continue;
            }
        }
        i += 1;
    }

    // Greedily select non-overlapping matches (sorted by new end, maximizing coverage)
    matches.sort_by_key(|m| m.1 + m.2);
    let mut selected: Vec<(usize, usize, usize)> = Vec::new();
    let (mut last_o, mut last_n) = (0usize, 0usize);
    for m in matches {
        if m.1 >= last_n && m.0 >= last_o {
            selected.push(m);
            last_n = m.1 + m.2;
            last_o = m.0 + m.2;
        }
    }
    selected
}

/// Map instruction-level matches back to file byte offsets; gaps between matched blocks are the diffs.
fn build_entries(
    a_insns: &[Insn],
    b_insns: &[Insn],
    selected: &[(usize, usize, usize)],
    a_seg: &Segment,
    b_seg: &Segment,
) -> Vec<DiffEntry> {
    let a_end = a_seg.file_offset + a_seg.size;
    let b_end = b_seg.file_offset + b_seg.size;
    let mut out = Vec::new();
    let (mut o_cur, mut n_cur) = (0usize, 0usize);

    for &(op, np, len) in selected {
        if o_cur < op {
            let (s, e) = insn_byte_range(a_insns, o_cur, op, a_end);
            out.push(DiffEntry {
                offset: s,
                length: e - s,
                change: ChangeType::Removed,
                old: Some(s..e),
                new: None,
            });
        }
        if np > n_cur {
            let (s, e) = insn_byte_range(b_insns, n_cur, np, b_end);
            // A zero-length old range records the insertion point (in replacement scenarios it falls right after the deletion)
            let ins_point = a_insns.get(op).map(|i| i.offset as u64).unwrap_or(a_end);
            out.push(DiffEntry {
                offset: s,
                length: e - s,
                change: ChangeType::Added,
                old: Some(ins_point..ins_point),
                new: Some(s..e),
            });
        }
        o_cur = op + len;
        n_cur = np + len;
    }
    if n_cur < b_insns.len() {
        let (s, e) = insn_byte_range(b_insns, n_cur, b_insns.len(), b_end);
        let ins_point = a_insns.get(o_cur).map(|i| i.offset as u64).unwrap_or(a_end);
        out.push(DiffEntry {
            offset: s,
            length: e - s,
            change: ChangeType::Added,
            old: Some(ins_point..ins_point),
            new: Some(s..e),
        });
    }
    if o_cur < a_insns.len() {
        let (s, e) = insn_byte_range(a_insns, o_cur, a_insns.len(), a_end);
        out.push(DiffEntry {
            offset: s,
            length: e - s,
            change: ChangeType::Removed,
            old: Some(s..e),
            new: None,
        });
    }
    out
}

fn insn_byte_range(insns: &[Insn], start: usize, end: usize, seg_end: u64) -> (u64, u64) {
    if start >= end {
        return (0, 0);
    }
    let s = insns[start].offset as u64;
    let last = &insns[end - 1];
    let e = ((last.offset + last.len) as u64).min(seg_end);
    (s, e)
}

/// Window hash: xxh64 over k consecutive instruction signatures (8 bytes LE each).
fn win_hash(sigs: &[u64], start: usize, k: usize) -> u64 {
    let mut buf = [0u8; 64];
    let k = k.min(8);
    for j in 0..k {
        buf[j * 8..(j + 1) * 8].copy_from_slice(&sigs[start + j].to_le_bytes());
    }
    xxh(&buf[..k * 8])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn win_hash_stable() {
        let sigs = [1u64, 2, 3, 4, 5];
        assert_eq!(win_hash(&sigs, 0, 4), win_hash(&sigs, 0, 4));
        assert_eq!(win_hash(&sigs, 0, 4), win_hash(&[1, 2, 3, 4], 0, 4));
    }

    #[test]
    fn match_insns_identical() {
        // Build 8 instructions with identical signatures
        let mk = |off: usize| Insn { offset: off, len: 2, sig: 7 };
        let a: Vec<Insn> = (0..8).map(|i| mk(i * 2)).collect();
        let b: Vec<Insn> = (0..8).map(|i| mk(i * 2)).collect();
        let m = match_insns(&a, &b, 4);
        assert_eq!(m.len(), 1);
        assert_eq!(m[0], (0, 0, 8));
    }

    #[test]
    fn match_insns_insertion() {
        let mk = |off: usize, sig: u64| Insn { offset: off, len: 2, sig };
        let a: Vec<Insn> = (0..8).map(|i| mk(i * 2, i as u64)).collect();
        // b inserts 2 different instructions in the middle
        let mut b: Vec<Insn> = Vec::new();
        for i in 0..4 {
            b.push(mk(i * 2, i as u64));
        }
        b.push(mk(8, 100));
        b.push(mk(10, 101));
        for i in 4..8 {
            b.push(mk(12 + (i - 4) * 2, i as u64));
        }
        let m = match_insns(&a, &b, 2);
        // Should find two matched runs: the first 4 + the last 4
        assert!(!m.is_empty());
    }
}
