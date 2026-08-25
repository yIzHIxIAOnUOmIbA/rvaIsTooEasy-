//! Benchmark sample generation and "known-truth" transforms. Fully deterministic (xorshift64), reproducible.

use std::collections::HashSet;
use std::ops::Range;

/// xorshift64 PRNG (deterministic, no external dependencies).
pub struct Rng(pub u64);

impl Rng {
    pub fn new(seed: u64) -> Self {
        Rng(seed.max(1))
    }
    pub fn next(&mut self) -> u64 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        self.0 = x;
        x
    }
    pub fn byte(&mut self) -> u8 {
        (self.next() >> 32) as u8
    }
    pub fn range(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// Generate "firmware-like" data: function blocks (4 header bytes 0x55 + random body + 0xC3 ret),
/// with 16-byte aligned padding (0x00) between blocks and occasional 0xCC int3 sleds. Mimics real
/// binary structure + highly repetitive padding.
pub fn gen_firmware(seed: u64, total: usize) -> Vec<u8> {
    let mut rng = Rng::new(seed);
    let mut out: Vec<u8> = Vec::with_capacity(total);
    'outer: loop {
        for _ in 0..4 {
            out.push(0x55);
            if out.len() >= total {
                break 'outer;
            }
        }
        let body = 32 + rng.range(96) as usize;
        for _ in 0..body {
            out.push(rng.byte());
            if out.len() >= total {
                break 'outer;
            }
        }
        out.push(0xC3);
        while out.len() % 16 != 0 {
            out.push(0x00);
            if out.len() >= total {
                break 'outer;
            }
        }
        if rng.range(8) == 0 {
            for _ in 0..16 {
                out.push(0xCC);
                if out.len() >= total {
                    break 'outer;
                }
            }
        }
    }
    out.truncate(total);
    out
}

/// Truth: a_truth = byte offsets of "real differences" in old file A; b_truth = byte offsets of
/// "real differences" in new file B.
pub struct Truth {
    pub a_truth: HashSet<u64>,
    pub b_truth: HashSet<u64>,
    pub b: Vec<u8>,
}

fn mark(set: &mut HashSet<u64>, r: Range<u64>) {
    for i in r {
        set.insert(i);
    }
}

/// Point modification: n spots, each randomly rewritten by 1..=maxlen bytes. A/B coordinates are
/// unchanged; both sides are marked.
pub fn t_point(a: &[u8], seed: u64, n: usize, maxlen: u64) -> Truth {
    let mut rng = Rng::new(seed);
    let mut b = a.to_vec();
    let mut at = HashSet::new();
    let mut bt = HashSet::new();
    let _ = &mut at;
    for _ in 0..n {
        let len = 1 + rng.range(maxlen);
        let pos = rng.range(a.len() as u64 - len);
        for i in 0..len {
            b[(pos + i) as usize] = rng.byte();
            at.insert(pos + i);
            bt.insert(pos + i);
        }
    }
    Truth { a_truth: at, b_truth: bt, b }
}

/// Single insertion: insert len bytes at pos. Only the B side is marked.
pub fn t_insert(a: &[u8], seed: u64, pos: u64, len: usize) -> Truth {
    let mut rng = Rng::new(seed);
    let mut b = a.to_vec();
    let ins: Vec<u8> = (0..len).map(|_| rng.byte()).collect();
    b.splice(pos as usize..pos as usize, ins);
    let mut bt = HashSet::new();
    mark(&mut bt, pos..pos + len as u64);
    Truth { a_truth: HashSet::new(), b_truth: bt, b }
}

/// Multi-insertion (simulating recompilation address drift): n spots, each minlen..=maxlen bytes.
/// Insert in ascending order and accumulate offsets so that `b_truth` marks the real coordinates of
/// the inserted bytes in the final B.
pub fn t_multi_insert(a: &[u8], seed: u64, n: usize, minlen: usize, maxlen: usize) -> Truth {
    let mut rng = Rng::new(seed);
    let mut spots: Vec<(u64, usize)> = Vec::new();
    for _ in 0..n {
        let len = minlen + rng.range((maxlen - minlen + 1) as u64) as usize;
        let pos = rng.range(a.len() as u64 - 1);
        spots.push((pos, len));
    }
    // Ascending insertion: each prior insertion shifts later points to the right; accumulating with
    // shift yields the real B coordinates.
    spots.sort_by(|x, y| x.0.cmp(&y.0));
    let mut b = a.to_vec();
    let mut bt = HashSet::new();
    let mut shift = 0u64;
    for (pos, len) in spots {
        let ins: Vec<u8> = (0..len).map(|_| rng.byte()).collect();
        let actual = pos + shift;
        b.splice(actual as usize..actual as usize, ins);
        mark(&mut bt, actual..actual + len as u64);
        shift += len as u64;
    }
    Truth { a_truth: HashSet::new(), b_truth: bt, b }
}

/// Deletion: remove [pos, pos+len). Only the A side is marked.
pub fn t_delete(a: &[u8], pos: u64, len: usize) -> Truth {
    let mut b = a.to_vec();
    b.drain(pos as usize..pos as usize + len);
    let mut at = HashSet::new();
    mark(&mut at, pos..pos + len as u64);
    Truth { a_truth: at, b_truth: HashSet::new(), b }
}
