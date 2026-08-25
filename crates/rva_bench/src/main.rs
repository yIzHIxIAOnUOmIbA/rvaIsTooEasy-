//! rva_bench - the rvaIsTooEasy stress benchmark.
//! Usage:
//!   cargo run -p rva_bench --release
//!   cargo run -p rva_bench --release -- --size 256          # performance test with 256MB
//!   cargo run -p rva_bench --release -- --samples A.bin B.bin  # compare real samples
//!   cargo run -p rva_bench --release -- --gen-samples [dir]   # generate a sample set (default samples/)

mod gen;
mod run;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    // Sample-set generation mode
    if let Some(i) = args.iter().position(|a| a == "--gen-samples") {
        let dir = if args.len() > i + 1 { args[i + 1].clone() } else { "samples".to_string() };
        run::gen_samples(&dir);
        return;
    }

    // Real-samples mode
    if let Some(i) = args.iter().position(|a| a == "--samples") {
        if args.len() > i + 2 {
            run::samples(&args[i + 1], &args[i + 2]);
        } else {
            println!("用法: --samples <A路径> <B路径>");
        }
        return;
    }

    // Performance test size (MB)
    let mut size_mb = 64usize;
    if let Some(i) = args.iter().position(|a| a == "--size") {
        if args.len() > i + 1 {
            size_mb = args[i + 1].parse().unwrap_or(64);
        }
    }

    run::robustness();
    run::accuracy(256 * 1024);
    run::patch_roundtrip();
    run::perf(size_mb);

    println!("\n压测完成。");
}
