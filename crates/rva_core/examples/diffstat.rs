//! Characterize two binaries: find the DOMINANT SHIFT (including large ones near size_diff).
//! Usage: cargo run --release --example diffstat -- <A> <B>
use std::env;
use std::fs;

fn byte_identity(a: &[u8], b: &[u8], delta: i64) -> f64 {
    // compare a[i] with b[i+delta]; sample every 32KB for speed, over the valid overlap.
    let n = a.len().min(b.len());
    let mut tot = 0u64;
    let mut eq = 0u64;
    let mut i = 0usize;
    while i < n {
        let bi = i as i64 + delta;
        if bi >= 0 && bi < b.len() as i64 {
            tot += 1;
            if a[i] == b[bi as usize] {
                eq += 1;
            }
        }
        i += 32768;
    }
    if tot == 0 { 0.0 } else { eq as f64 / tot as f64 }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let a = fs::read(&args[1]).unwrap();
    let b = fs::read(&args[2]).unwrap();
    let size_diff = b.len() as i64 - a.len() as i64;

    println!("A size={}  B size={}  size_diff={}", a.len(), b.len(), size_diff);

    // explicit candidate deltas
    println!("\n=== byte-identity at candidate deltas ===");
    for d in [0i64, size_diff, -size_diff] {
        println!("  delta={:>10}  identity={:.2}%", d, 100.0 * byte_identity(&a, &b, d));
    }

    // grid scan for dominant shift in [-2MB, +2MB]
    println!("\n=== grid scan for dominant shift [-2MB,+2MB] step 16384 ===");
    let mut best: (i64, f64) = (0, 0.0);
    let mut runner_up: (i64, f64) = (0, 0.0);
    let mut d = -2_000_000i64;
    while d <= 2_000_000 {
        let r = byte_identity(&a, &b, d);
        if r > best.1 {
            runner_up = best;
            best = (d, r);
        } else if r > runner_up.1 {
            runner_up = (d, r);
        }
        d += 16384;
    }
    println!("  best delta={} identity={:.2}%", best.0, 100.0 * best.1);
    println!("  2nd  delta={} identity={:.2}%", runner_up.0, 100.0 * runner_up.1);

    // refine best delta ±16384 step 256
    println!("\n=== refine best delta +-16384 step 256 ===");
    let base = best.0;
    let mut rb = (base, best.1);
    let mut rd = base - 16384;
    while rd <= base + 16384 {
        let r = byte_identity(&a, &b, rd);
        if r > rb.1 {
            rb = (rd, r);
        }
        rd += 256;
    }
    println!("  refined delta={} identity={:.2}%", rb.0, 100.0 * rb.1);
}
