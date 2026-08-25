//! Benchmark runner: false-positive rate, robustness, performance, patch roundtrip.

use crate::gen::{self, Rng, Truth};
use rva_core::aligner::{AlignMode, Aligner, DefaultAligner};
use rva_core::diff_engine::{ChangeType, DefaultDiffEngine, DiffEngine, DiffStrategy, DiffEntry};
use rva_core::file_loader::{Arch, DefaultFileLoader, FileLoader, LoadedFile};
use rva_core::patch_engine::{DefaultPatchEngine, PatchEngine, PatchFormat};
use std::collections::HashSet;
use std::path::PathBuf;
use std::time::Instant;

const KB: usize = 1024;
const MB: usize = 1024 * 1024;

/// Wraps a pair of already-mmap'd LoadedFiles; on Drop, releases the mmaps before deleting temp files.
struct Pair {
    a: Option<LoadedFile>,
    b: Option<LoadedFile>,
    pa: PathBuf,
    pb: PathBuf,
}

impl Pair {
    fn new(a: &[u8], b: &[u8], tag: &str) -> Pair {
        let pa = std::env::temp_dir().join(format!("rva_bench_{}_a.bin", tag));
        let pb = std::env::temp_dir().join(format!("rva_bench_{}_b.bin", tag));
        std::fs::write(&pa, a).unwrap();
        std::fs::write(&pb, b).unwrap();
        let la = DefaultFileLoader::load(&pa).unwrap();
        let lb = DefaultFileLoader::load(&pb).unwrap();
        Pair { a: Some(la), b: Some(lb), pa, pb }
    }
    fn a(&self) -> &LoadedFile {
        self.a.as_ref().unwrap()
    }
    fn b(&self) -> &LoadedFile {
        self.b.as_ref().unwrap()
    }
}

impl Drop for Pair {
    fn drop(&mut self) {
        drop(self.a.take());
        drop(self.b.take());
        let _ = std::fs::remove_file(&self.pa);
        let _ = std::fs::remove_file(&self.pb);
    }
}

/// Byte-level evaluation metrics (A/B sides merged).
struct Metrics {
    reported: u64,
    truth: u64,
    inter: u64,
    b_fp: u64,
}

impl Metrics {
    fn f1(&self) -> f64 {
        let denom = self.reported + self.truth;
        if denom == 0 {
            1.0
        } else {
            2.0 * self.inter as f64 / denom as f64
        }
    }
    fn fp(&self) -> u64 {
        self.reported - self.inter
    }
    fn fneg(&self) -> u64 {
        self.truth - self.inter
    }
}

fn eval(truth: &Truth, diffs: &[DiffEntry]) -> Metrics {
    let mut rb: HashSet<u64> = HashSet::new();
    let mut ra: HashSet<u64> = HashSet::new();
    for e in diffs {
        match e.change {
            ChangeType::Modified => {
                if let Some(r) = &e.old {
                    for i in r.clone() {
                        ra.insert(i);
                    }
                }
                if let Some(r) = &e.new {
                    for i in r.clone() {
                        rb.insert(i);
                    }
                }
            }
            ChangeType::Removed => {
                if let Some(r) = &e.old {
                    for i in r.clone() {
                        ra.insert(i);
                    }
                }
            }
            ChangeType::Added => {
                if let Some(r) = &e.new {
                    for i in r.clone() {
                        rb.insert(i);
                    }
                }
            }
        }
    }
    let rb_inter = rb.iter().filter(|o| truth.b_truth.contains(o)).count() as u64;
    let ra_inter = ra.iter().filter(|o| truth.a_truth.contains(o)).count() as u64;
    Metrics {
        reported: (rb.len() + ra.len()) as u64,
        truth: (truth.b_truth.len() + truth.a_truth.len()) as u64,
        inter: rb_inter + ra_inter,
        b_fp: rb.len() as u64 - rb_inter,
    }
}

fn strategies() -> Vec<(&'static str, DiffStrategy)> {
    vec![
        ("chunked-4K", DiffStrategy::ChunkedHash { chunk_size: 4096 }),
        ("chunked-64", DiffStrategy::ChunkedHash { chunk_size: 64 }),
        ("sliding-16", DiffStrategy::SlidingWindow { window: 16, min_match: 16 }),
        ("sliding-64", DiffStrategy::SlidingWindow { window: 64, min_match: 64 }),
    ]
}

fn align_modes() -> Vec<(&'static str, AlignMode)> {
    vec![
        ("sliding-16+align:byte", AlignMode::Byte),
        ("sliding-16+align:instr", AlignMode::Instruction),
        ("sliding-16+align:func", AlignMode::Function),
    ]
}

// ---------------------------------------------------------------------------
// 1. False-positive-rate benchmark
// ---------------------------------------------------------------------------

pub fn accuracy(size: usize) {
    println!("\n================ 假阳性率基准 ================");
    println!("样本: 类固件 {} KB  |  F1=2*inter/(reported+truth), FP=误报字节, FN=漏报字节\n", size / KB);

    let a = gen::gen_firmware(0xACC_0001, size);

    let cases: Vec<(&str, Truth)> = vec![
        ("point-100  (100处点改<=4B)", gen::t_point(&a, 1, 100, 4)),
        ("insert-1KB@25% (单点插入)", gen::t_insert(&a, 2, size as u64 / 4, KB)),
        ("multi-insert-20 (地址漂移)", gen::t_multi_insert(&a, 3, 20, 8, 64)),
        ("multi-insert-50 (密集漂移)", gen::t_multi_insert(&a, 4, 50, 8, 64)),
        ("delete-1KB@50% (单点删除)", gen::t_delete(&a, size as u64 / 2, KB)),
    ];

    for (name, truth) in cases {
        println!("--- 场景: {} (真值差异 {} B) ---", name, truth.a_truth.len() + truth.b_truth.len());
        println!("{:<26} {:>10} {:>10} {:>10} {:>10} {:>8} {:>10}", "方法", "reported", "truth", "FP", "FN", "F1", "B侧FP");
        let pair = Pair::new(&a, &truth.b, "acc");
        let engine = DefaultDiffEngine;
        for (label, strat) in strategies() {
            let diffs = engine.diff(pair.a(), pair.b(), strat).unwrap();
            let m = eval(&truth, &diffs);
            println!(
                "{:<26} {:>10} {:>10} {:>10} {:>10} {:>8.3} {:>10}",
                label, m.reported, m.truth, m.fp(), m.fneg(), m.f1(), m.b_fp
            );
        }
        for (label, mode) in align_modes() {
            let diffs = engine
                .diff(pair.a(), pair.b(), DiffStrategy::SlidingWindow { window: 16, min_match: 16 })
                .unwrap();
            let aligned = DefaultAligner::default().align(pair.a(), pair.b(), diffs, mode).unwrap();
            let m = eval(&truth, &aligned);
            println!(
                "{:<26} {:>10} {:>10} {:>10} {:>10} {:>8.3} {:>10}",
                label, m.reported, m.truth, m.fp(), m.fneg(), m.f1(), m.b_fp
            );
        }
        println!();
    }
}

// ---------------------------------------------------------------------------
// 2. Robustness (no panic + sane results)
// ---------------------------------------------------------------------------

pub fn robustness() {
    println!("\n================ 鲁棒性 ================");
    let engine = DefaultDiffEngine;
    let mut ok = true;

    // All zeros 4096, flip 1 byte
    {
        let a = vec![0u8; 4096];
        let mut b = a.clone();
        b[2048] = 1;
        let pair = Pair::new(&a, &b, "rz");
        let d = engine.diff(pair.a(), pair.b(), DiffStrategy::SlidingWindow { window: 16, min_match: 16 }).unwrap();
        let pass = !d.is_empty();
        ok &= pass;
        println!("[{}] 全零 4096 改1字节 -> {} 条差异", flag(pass), d.len());
    }

    // Highly repetitive (period 16) 64KB
    {
        let mut rng = Rng::new(7);
        let pat: Vec<u8> = (0..16).map(|_| rng.byte()).collect();
        let a: Vec<u8> = (0..64 * KB).map(|i| pat[i % 16]).collect();
        let mut b = a.clone();
        b[10000] ^= 0xFF;
        let pair = Pair::new(&a, &b, "rep");
        let t = Instant::now();
        let d = engine.diff(pair.a(), pair.b(), DiffStrategy::SlidingWindow { window: 16, min_match: 16 }).unwrap();
        let pass = !d.is_empty() && t.elapsed().as_secs() < 10;
        ok &= pass;
        println!("[{}] 周期重复 64KB -> {} 条差异, 耗时 {:?}", flag(pass), d.len(), t.elapsed());
    }

    // Head insertion of 4/16/64 bytes (sliding should report only Added = inserted length, no Removed)
    {
        let a: Vec<u8> = (0..4096u32).map(|x| (x % 251) as u8).collect();
        for ins in [4usize, 16, 64] {
            let mut b = vec![0xEEu8; ins];
            b.extend_from_slice(&a);
            let pair = Pair::new(&a, &b, "ins");
            let d = engine.diff(pair.a(), pair.b(), DiffStrategy::SlidingWindow { window: 16, min_match: 16 }).unwrap();
            let added: u64 = d.iter().filter(|e| e.change == ChangeType::Added).map(|e| e.length).sum();
            let removed: u64 = d.iter().filter(|e| e.change == ChangeType::Removed).map(|e| e.length).sum();
            let pass = added == ins as u64 && removed == 0;
            ok &= pass;
            println!("[{}] 头部插入 {}B -> Added={}, Removed={}", flag(pass), ins, added, removed);
        }
    }

    // Pure random, no difference -> empty result
    {
        let mut rng = Rng::new(11);
        let a: Vec<u8> = (0..4096).map(|_| rng.byte()).collect();
        let pair = Pair::new(&a, &a, "same");
        let d = engine.diff(pair.a(), pair.b(), DiffStrategy::SlidingWindow { window: 16, min_match: 16 }).unwrap();
        let pass = d.is_empty();
        ok &= pass;
        println!("[{}] 纯随机同文件 -> {} 条差异(应为0)", flag(pass), d.len());
    }

    // Truncation (tail deletion)
    {
        let a: Vec<u8> = (0..4096u32).map(|x| (x % 251) as u8).collect();
        let b = &a[..a.len() - 100];
        let pair = Pair::new(&a, b, "trunc");
        let d = engine.diff(pair.a(), pair.b(), DiffStrategy::SlidingWindow { window: 16, min_match: 16 }).unwrap();
        let removed: u64 = d.iter().filter(|e| e.change == ChangeType::Removed).map(|e| e.length).sum();
        let pass = removed == 100;
        ok &= pass;
        println!("[{}] 尾部截断 100B -> Removed={} (应为100)", flag(pass), removed);
    }

    println!("鲁棒性总评: {}", if ok { "全部通过" } else { "存在失败项" });
}

// ---------------------------------------------------------------------------
// 3. Performance (large files + timing)
// ---------------------------------------------------------------------------

pub fn perf(size_mb: usize) {
    println!("\n================ 性能基准 ================");
    let size = size_mb * MB;
    println!("生成 {} MB 类固件样本...", size_mb);
    let a = gen::gen_firmware(0xFEED, size);
    let mut b = a.clone();
    b[size / 2] ^= 0xFF;
    // Also insert a segment simulating a local change
    b.splice(size / 4..size / 4, vec![0xABu8; 1024]);

    let pa = std::env::temp_dir().join("rva_bench_perf_a.bin");
    let pb = std::env::temp_dir().join("rva_bench_perf_b.bin");
    std::fs::write(&pa, &a).unwrap();
    std::fs::write(&pb, &b).unwrap();

    let t = Instant::now();
    let la = DefaultFileLoader::load(&pa).unwrap();
    let dt_load = t.elapsed();
    let t = Instant::now();
    let lb = DefaultFileLoader::load(&pb).unwrap();
    let dt_load2 = t.elapsed();

    println!("{:<20} {:>12}", "load A (mmap)", fmt_dur(dt_load));
    println!("{:<20} {:>12}", "load B (mmap)", fmt_dur(dt_load2));

    let engine = DefaultDiffEngine;
    let t = Instant::now();
    let _d = engine.diff(&la, &lb, DiffStrategy::ChunkedHash { chunk_size: 4096 }).unwrap();
    let dt_chunk = t.elapsed();
    println!("{:<20} {:>12}  ({:.1} MB/s)", "chunked-4K", fmt_dur(dt_chunk), size as f64 / dt_chunk.as_secs_f64() / MB as f64);

    let t = Instant::now();
    let d = engine.diff(&la, &lb, DiffStrategy::SlidingWindow { window: 16, min_match: 16 }).unwrap();
    let dt_slide = t.elapsed();
    println!("{:<20} {:>12}  ({:.1} MB/s, {} 条差异)", "sliding-16", fmt_dur(dt_slide), size as f64 / dt_slide.as_secs_f64() / MB as f64, d.len());

    drop(la);
    drop(lb);
    let _ = std::fs::remove_file(&pa);
    let _ = std::fs::remove_file(&pb);
}

// ---------------------------------------------------------------------------
// 6. Sample set (deterministically generated; for GUI demo / manual retest / docs tutorials)
// ---------------------------------------------------------------------------

/// Generate a deterministic set of sample files into the given directory (default samples/).
/// Each group outputs `{name}_a.bin` / `{name}_b.bin`, with a summary written to `samples/README.md`.
pub fn gen_samples(dir: &str) {
    use std::fmt::Write as _;
    std::fs::create_dir_all(dir).unwrap();

    struct Case {
        name: &'static str,
        desc: &'static str,
        a: Vec<u8>,
        b: Vec<u8>,
        truth_a: usize,
        truth_b: usize,
    }

    let mut cases: Vec<Case> = Vec::new();

    // 64KB firmware-like: 20 point modifications (<=4B) + one 256B insertion at 25%
    {
        let a = gen::gen_firmware(0xD00D_0001, 64 * KB);
        let t1 = gen::t_point(&a, 31, 20, 4);
        let a2 = t1.b.clone();
        let t2 = gen::t_insert(&a2, 32, (a2.len() as u64) / 4, 256);
        let truth_a = t1.a_truth.len() + t2.a_truth.len();
        let truth_b = t1.b_truth.len() + t2.b_truth.len();
        cases.push(Case { name: "toy", desc: "64KB 类固件：20 处点改(<=4B) + 1 处 256B 插入", a: a.clone(), b: t2.b, truth_a, truth_b });
    }

    // 1MB firmware-like: address drift (30 insertions of 8-64B) + one 4KB deletion at 50%
    {
        let a = gen::gen_firmware(0xD00D_0002, MB);
        let t1 = gen::t_multi_insert(&a, 33, 30, 8, 64);
        let a2 = t1.b.clone();
        let t2 = gen::t_delete(&a2, (a2.len() as u64) / 2, 4 * KB);
        let truth_a = t1.a_truth.len() + t2.a_truth.len();
        let truth_b = t1.b_truth.len() + t2.b_truth.len();
        cases.push(Case { name: "drift", desc: "1MB 类固件：30 处 8-64B 地址漂移插入 + 1 处 4KB 删除", a: a.clone(), b: t2.b, truth_a, truth_b });
    }

    // 16MB firmware-like: one 1KB insertion at 25% + one 1-byte modification at 50% (large/highly repetitive; tests perf and UI)
    {
        let a = gen::gen_firmware(0xD00D_0003, 16 * MB);
        let mut b = a.clone();
        let half = b.len() / 2;
        b[half] ^= 0xFF;
        let q = b.len() / 4;
        b.splice(q..q, vec![0xAB; 1024]);
        cases.push(Case { name: "large", desc: "16MB 类固件：1 处 1KB 插入@25% + 1 字节修改@50%（大文件/高重复）", a: a.clone(), b, truth_a: 1, truth_b: 1025 });
    }

    // No-difference control (A == B)
    {
        let a = gen::gen_firmware(0xD00D_0004, 256 * KB);
        cases.push(Case { name: "identical", desc: "256KB 类固件：A==B 无差异（对照组）", a: a.clone(), b: a.clone(), truth_a: 0, truth_b: 0 });
    }

    // Write files + summary README
    let mut md = String::from("# rvaIsTooEasy 示例样本集\n\n");
    md.push_str("由 `cargo run -p rva_bench --release -- --gen-samples` 确定性生成，可复现。\n\n");
    md.push_str("| 样本对 | 大小 | 内容 | A 侧真值差异 | B 侧真值差异 |\n|---|---|---|---|---|\n");
    for c in &cases {
        let pa = format!("{}/{}_a.bin", dir, c.name);
        let pb = format!("{}/{}_b.bin", dir, c.name);
        std::fs::write(&pa, &c.a).unwrap();
        std::fs::write(&pb, &c.b).unwrap();
        let _ = writeln!(md, "| {} | A {} B {} | {} | {} B | {} B |",
            c.name, c.a.len(), c.b.len(), c.desc, c.truth_a, c.truth_b);
    }
    md.push_str("\n## 用法\n\n- GUI：在「比对」页选择 `toy_a.bin` 与 `toy_b.bin`（A/B 路径输入框各填一个，再点「比对」按钮）查看差异。\n");
    md.push_str("- CLI：`rva diff toy_a.bin toy_b.bin --out report.html`\n");
    md.push_str("- 压测：`cargo run -p rva_bench --release -- --samples toy_a.bin toy_b.bin`\n\n");
    md.push_str("真值说明：A 侧真值差异 = 仅在 A 中出现的字节（删除/修改的旧值）；B 侧真值差异 = 仅在 B 中出现的字节（新增内容）。\n");
    std::fs::write(format!("{}/README.md", dir), md).unwrap();
    println!("示例样本集已生成到 {} （4 组：toy / drift / large / identical）", dir);
}

fn fmt_dur(d: std::time::Duration) -> String {
    if d.as_secs() >= 1 {
        format!("{:.2}s", d.as_secs_f64())
    } else {
        format!("{}ms", d.as_millis())
    }
}

fn flag(b: bool) -> &'static str {
    if b {
        "PASS"
    } else {
        "FAIL"
    }
}

// ---------------------------------------------------------------------------
// 4. Patch roundtrip (patch -> apply -> verify)
// ---------------------------------------------------------------------------

pub fn patch_roundtrip() {
    println!("\n================ 补丁往返 ================");
    let a: Vec<u8> = gen::gen_firmware(0xBEEF, 128 * KB);
    let cases: Vec<(&str, Truth)> = vec![
        ("头插1KB", gen::t_insert(&a, 20, 0, KB)),
        ("尾删1KB", gen::t_delete(&a, (a.len() - KB) as u64, KB)),
        ("点改200处", gen::t_point(&a, 21, 200, 4)),
        ("地址漂移40插", gen::t_multi_insert(&a, 22, 40, 8, 64)),
    ];
    for (name, truth) in cases {
        let pair = Pair::new(&a, &truth.b, "patch");
        let engine = DefaultDiffEngine;
        let diffs = engine
            .diff(pair.a(), pair.b(), DiffStrategy::SlidingWindow { window: 16, min_match: 16 })
            .unwrap();
        let patch = DefaultPatchEngine::generate(pair.a(), pair.b(), &diffs, PatchFormat::Custom).unwrap();
        let out = std::env::temp_dir().join("rva_bench_patch_out.bin");
        DefaultPatchEngine::apply(pair.a(), &patch, &out).unwrap();
        let got = std::fs::read(&out).unwrap();
        let ok = got == truth.b;
        let patch_bytes = DefaultPatchEngine::serialize(&patch).unwrap().len();
        let ratio = patch_bytes as f64 / truth.b.len() as f64 * 100.0;
        let _ = std::fs::remove_file(&out);
        println!("[{}] {} -> 补丁 {} B (占新文件 {:.1}%)", flag(ok), name, patch_bytes, ratio);
    }
}

// ---------------------------------------------------------------------------
// 5. Real samples (two files provided by the user)
// ---------------------------------------------------------------------------

pub fn samples(a_path: &str, b_path: &str) {
    println!("\n================ 真实样本对比 ================");
    let pa = PathBuf::from(a_path);
    let pb = PathBuf::from(b_path);
    let la = match DefaultFileLoader::load(&pa) {
        Ok(l) => l,
        Err(e) => {
            println!("加载 A 失败: {}", e);
            return;
        }
    };
    let lb = match DefaultFileLoader::load(&pb) {
        Ok(l) => l,
        Err(e) => {
            println!("加载 B 失败: {}", e);
            return;
        }
    };
    println!("A: {} ({} B, {:?})", a_path, la.meta.size, la.meta.format);
    println!("B: {} ({} B, {:?})", b_path, lb.meta.size, lb.meta.format);
    let engine = DefaultDiffEngine;
    for (label, strat) in strategies() {
        let t = Instant::now();
        let d = engine.diff(&la, &lb, strat).unwrap();
        let bytes: u64 = d.iter().map(|e| e.length).sum();
        println!("{:<16} {} 条差异, 覆盖 {} B, 耗时 {:?}", label, d.len(), bytes, t.elapsed());
    }
    // structural (function-level structure matching, x86/x64 only)
    if matches!(la.meta.arch, Arch::X86 | Arch::X86_64)
        && matches!(lb.meta.arch, Arch::X86 | Arch::X86_64)
    {
        let t = Instant::now();
        match engine.diff(&la, &lb, DiffStrategy::Structural { min_run: 8 }) {
            Ok(d) => {
                let bytes: u64 = d.iter().map(|e| e.length).sum();
                let added = d.iter().filter(|e| e.change == ChangeType::Added).count();
                let removed = d.iter().filter(|e| e.change == ChangeType::Removed).count();
                println!(
                    "{:<16} {} 条差异(Added {} / Removed {}), 覆盖 {} B, 耗时 {:?}",
                    "structural", d.len(), added, removed, bytes, t.elapsed()
                );
            }
            Err(e) => println!("{:<16} 失败: {}", "structural", e),
        }
    }
}
