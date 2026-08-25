//! rva CLI entry point. Phase 1 wired up the `diff` subcommand (load + diff + statistics).
//! Report / Batch / Symbols / Patch are implemented in Phase 2/3.
use anyhow::Context;
use clap::{Parser, Subcommand};
use rva_core::aligner::{AlignMode, Aligner, DefaultAligner};
use rva_core::apply;
use rva_core::diff_engine::{ChangeType, DefaultDiffEngine, DiffEngine, DiffStrategy};
use rva_core::file_loader::{DefaultFileLoader, FileLoader};
use rva_core::patch_engine::{DefaultPatchEngine, PatchEngine, PatchFormat as PatchFmt};
use rva_core::patch_pack::{build_revert_segments, sha256_of, PackedPatch, PatchMeta};
use rva_core::report_generator::{DefaultReportGenerator, ReportFormat, ReportGenerator};
use rva_core::signing;
use rva_core::symbol_resolver::{DefaultSymbolResolver, SymbolResolver};
use rva_core::{BatchComparator, BatchNode, BatchStatus, DefaultBatchComparator, DiffReport};
use std::io::Write;
use std::path::Path;

#[derive(Parser)]
#[command(name = "rva", about = "Lightweight binary diff tool for security/firmware RE")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Diff two files
    Diff {
        a: String,
        b: String,
        /// Strategy: chunked (block hash) | sliding (sliding window)
        #[arg(long, default_value = "chunked")]
        strategy: String,
        /// Block size for chunked hashing
        #[arg(long, default_value_t = 4096)]
        chunk_size: usize,
        /// Sliding window size
        #[arg(long, default_value_t = 64)]
        window: usize,
        /// Minimum match length for the sliding window
        #[arg(long, default_value_t = 64)]
        min_match: usize,
    },
    /// Generate report (html/txt/json)
    Report {
        a: String,
        b: String,
        /// Output format: html | txt | json
        #[arg(default_value = "html")]
        format: String,
        /// Merge sliding-window Removed+Added into Modified (alignment)
        #[arg(long, default_value_t = false)]
        align: bool,
    },
    /// Recursively compare two directories
    Batch { a: String, b: String },
    /// Resolve symbols for a binary (PDB/DWARF)
    Symbols { binary: String, debug_info: Option<String> },
    /// Patch ecosystem: generate / apply / rollback / verify
    #[command(subcommand)]
    Patch(PatchCmd),
    /// Manage signing keys (generate / list / delete / import)
    #[command(subcommand)]
    Key(KeyCmd),
}

#[derive(Subcommand)]
enum PatchCmd {
    /// Build a .rvapatch container (A->B diff, optionally signed)
    Generate {
        a: String,
        b: String,
        /// Output .rvapatch path
        #[arg(short, long)]
        out: String,
        /// Sign with the named key in the keystore (omit to leave unsigned)
        #[arg(long)]
        key: Option<String>,
        /// Diff strategy: chunked | sliding
        #[arg(long, default_value = "sliding")]
        strategy: String,
    },
    /// Apply patches: source + patches (multiple allowed, applied chained in order) -> out
    Apply {
        source: String,
        #[arg(required = true, num_args = 1..)]
        patches: Vec<String>,
        #[arg(short, long)]
        out: String,
    },
    /// Roll back a patch: patched file + patch -> out
    Rollback {
        current: String,
        patch: String,
        #[arg(short, long)]
        out: String,
    },
    /// Verify container integrity (magic/TLV/SHA256) and list valid signatures
    Verify { patch: String },
}

#[derive(Subcommand)]
enum KeyCmd {
    /// Generate a new key pair
    Generate { name: String },
    /// List all keys in the keystore
    List,
    /// Delete a key
    Delete { name: String },
    /// Import an external public key (trust only)
    Import { name: String, public_key_hex: String },
}

fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Diff { a, b, strategy, chunk_size, window, min_match } => {
            let fa = DefaultFileLoader::load(Path::new(&a))?;
            let fb = DefaultFileLoader::load(Path::new(&b))?;
            let st = match strategy.as_str() {
                "sliding" => DiffStrategy::SlidingWindow { window, min_match },
                _ => DiffStrategy::ChunkedHash { chunk_size },
            };
            let engine = DefaultDiffEngine;
            let entries = engine.diff(&fa, &fb, st)?;

            let added = entries.iter().filter(|e| e.change == ChangeType::Added).count();
            let removed = entries.iter().filter(|e| e.change == ChangeType::Removed).count();
            let modified = entries.iter().filter(|e| e.change == ChangeType::Modified).count();
            let bytes: u64 = entries.iter().map(|e| e.length).sum();

            println!("files   : A={} ({}B)  B={} ({}B)", a, fa.meta.size, b, fb.meta.size);
            println!(
                "format  : A={:?} B={:?}   arch A={:?}",
                fa.meta.format, fb.meta.format, fa.meta.arch
            );
            println!(
                "strategy: {}   diff_entries={}  (Added={} Removed={} Modified={})  total_bytes={}",
                strategy,
                entries.len(),
                added,
                removed,
                modified,
                bytes
            );
            for e in entries.iter().take(20) {
                println!("  {:?} @ off={} len={}", e.change, e.offset, e.length);
            }
            if entries.len() > 20 {
                println!("  ... and {} more", entries.len() - 20);
            }
        }
        Commands::Report { a, b, format, align } => {
            let fa = DefaultFileLoader::load(Path::new(&a))?;
            let fb = DefaultFileLoader::load(Path::new(&b))?;
            // Report defaults to the sliding window (fine-grained; most meaningful when combined with align)
            let engine = DefaultDiffEngine;
            let mut entries = engine.diff(&fa, &fb, DiffStrategy::SlidingWindow { window: 8, min_match: 8 })?;
            if align {
                entries = DefaultAligner::default().align(&fa, &fb, entries, AlignMode::Byte)?;
            }
            let summary = rva_core::report_generator::summarize(&entries);
            let report = DiffReport { entries, symbols: None, summary };
            let fmt = ReportFormat::from_str_checked(&format);
            let out = DefaultReportGenerator::generate(&report, fmt)?;

            // html is written to a file by default so the browser can open it; txt/json are printed directly
            match fmt {
                ReportFormat::Html => {
                    let path = std::env::current_dir()?.join("rva_report.html");
                    let mut f = std::fs::File::create(&path)?;
                    f.write_all(out.as_bytes())?;
                    println!("report written -> {}", path.display());
                }
                _ => print!("{}", out),
            }
        }
        Commands::Batch { a, b } => {
            let node = DefaultBatchComparator::compare_dirs(
                Path::new(&a),
                Path::new(&b),
                DiffStrategy::ChunkedHash { chunk_size: 4096 },
            )?;
            print_node(&node, 0);
        }
        Commands::Symbols { binary, debug_info } => {
            let di = debug_info.as_deref().map(Path::new);
            let map = DefaultSymbolResolver::resolve(Path::new(&binary), di)?;
            println!("symbols : {} entries from {}", map.0.len(), binary);
            let mut v: Vec<_> = map.0.values().collect();
            v.sort_by_key(|s| s.addr);
            for s in v.iter().take(20) {
                println!("  0x{:X}  {}  size={:?}", s.addr, s.name, s.size);
            }
            if v.len() > 20 {
                println!("  ... and {} more", v.len() - 20);
            }
        }
        Commands::Patch(cmd) => match cmd {
            PatchCmd::Generate { a, b, out, key, strategy } => {
                let fa = DefaultFileLoader::load(Path::new(&a))?;
                let fb = DefaultFileLoader::load(Path::new(&b))?;
                let st = match strategy.as_str() {
                    "sliding" => DiffStrategy::SlidingWindow { window: 8, min_match: 8 },
                    _ => DiffStrategy::ChunkedHash { chunk_size: 4096 },
                };
                let is_sliding = matches!(st, DiffStrategy::SlidingWindow { .. });
                let entries = DefaultDiffEngine.diff(&fa, &fb, st)?;
                let patch = DefaultPatchEngine::generate(&fa, &fb, &entries, PatchFmt::Custom)?;
                let revert = build_revert_segments(fa.data.as_ref(), &patch);
                let meta = PatchMeta {
                    source_sha256: sha256_of(fa.data.as_ref()),
                    target_sha256: sha256_of(fb.data.as_ref()),
                    timestamp: std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .map(|d| d.as_secs() as i64)
                        .unwrap_or(0),
                    engine_version: 1,
                    strategy: if is_sliding { 1 } else { 0 },
                    entry_count: entries.len() as u32,
                };
                let mut packed = PackedPatch::new(meta, &patch, revert)?;
                if let Some(name) = &key {
                    let sig = signing::sign_bytes(name, &packed.content_bytes())?;
                    packed.add_signature(sig);
                }
                std::fs::write(&out, packed.to_bytes())
                    .with_context(|| format!("写入补丁 {} 失败", out))?;
                println!("generate: {} ops, {}B, 签名={} -> {}", patch.ops.len(), packed.to_bytes().len(), key.as_deref().unwrap_or("无"), out);
            }
            PatchCmd::Apply { source, patches, out } => {
                let mut list: Vec<PackedPatch> = Vec::with_capacity(patches.len());
                for p in &patches {
                    let bytes = std::fs::read(p).with_context(|| format!("读取补丁 {} 失败", p))?;
                    list.push(PackedPatch::from_bytes(&bytes)?);
                }
                let refs: Vec<&PackedPatch> = list.iter().collect();
                let res = apply::apply_batch(Path::new(&source), &refs, Path::new(&out))?;
                println!("apply   : {} ({} 个补丁) -> {}", source, patches.len(), out);
                println!("result  : ok={} msg={}", res.ok, res.message);
            }
            PatchCmd::Rollback { current, patch, out } => {
                let bytes = std::fs::read(&patch).with_context(|| format!("读取补丁 {} 失败", patch))?;
                let packed = PackedPatch::from_bytes(&bytes)?;
                let res = apply::rollback_patch(Path::new(&current), &packed, Path::new(&out))?;
                println!("rollback: {} -> {}", current, out);
                println!("result  : ok={} msg={}", res.ok, res.message);
            }
            PatchCmd::Verify { patch } => {
                let bytes = std::fs::read(&patch).with_context(|| format!("读取补丁 {} 失败", patch))?;
                let packed = PackedPatch::from_bytes(&bytes)?;
                println!("verify  : {}  ({}B, 版本 {})", patch, bytes.len(), packed.version);
                println!("source  : {}", hex(&packed.metadata.source_sha256));
                println!("target  : {}", hex(&packed.metadata.target_sha256));
                println!("entries : {}  engine_v{}  strategy={}", packed.metadata.entry_count, packed.metadata.engine_version, packed.metadata.strategy);
                println!("revert  : {} 段", packed.revert_segments.len());
                if packed.signatures.is_empty() {
                    println!("signature: 无（未签名）");
                } else {
                    for s in &packed.signatures {
                        let valid = signing::verify_by_fingerprint(&packed.content_bytes(), s).unwrap_or(false);
                        println!("signature: {} {}", hex(&s.fingerprint), if valid { "VALID" } else { "INVALID/未信任" });
                    }
                }
            }
        },
        Commands::Key(cmd) => match cmd {
            KeyCmd::Generate { name } => {
                let k = signing::generate_keypair(&name)?;
                println!("key     : {} 指纹={} 私钥={}", k.name, k.fingerprint_hex, if k.has_private { "有" } else { "无" });
            }
            KeyCmd::List => {
                let keys = signing::list_keys()?;
                if keys.is_empty() {
                    println!("keystore: （空）");
                }
                for k in keys {
                    println!("  {}  指纹={}  私钥={}", k.name, k.fingerprint_hex, if k.has_private { "有" } else { "无" });
                }
            }
            KeyCmd::Delete { name } => {
                signing::delete_key(&name)?;
                println!("key     : 已删除 {name}");
            }
            KeyCmd::Import { name, public_key_hex } => {
                let raw = hex_decode(&public_key_hex).with_context(|| "公钥 hex 非法")?;
                let k = signing::import_public(&name, &raw)?;
                println!("key     : 已导入 {} 指纹={}", k.name, k.fingerprint_hex);
            }
        },
    }
    Ok(())
}

/// Recursively print the batch comparison result tree.
fn print_node(node: &BatchNode, depth: usize) {
    let indent = "  ".repeat(depth);
    let name = node
        .path_a
        .as_deref()
        .or(node.path_b.as_deref())
        .unwrap_or("?");
    let status = match node.status {
        BatchStatus::Identical => "IDENTICAL ",
        BatchStatus::Different => "DIFFERENT ",
        BatchStatus::OnlyInA => "ONLY-IN-A ",
        BatchStatus::OnlyInB => "ONLY-IN-B ",
        BatchStatus::Error => "ERROR     ",
    };
    let note = node
        .diffs
        .as_ref()
        .map(|d| format!("  [{} diffs]", d.len()))
        .unwrap_or_default();
    println!("{}{}{}{}", indent, status, name, note);
    for c in &node.children {
        print_node(c, depth + 1);
    }
}

/// Byte array -> lowercase hex.
fn hex(b: &[u8]) -> String {
    let mut s = String::with_capacity(b.len() * 2);
    for byte in b {
        s.push_str(&format!("{byte:02x}"));
    }
    s
}

/// Hex string -> byte array.
fn hex_decode(s: &str) -> anyhow::Result<Vec<u8>> {
    let clean: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    if clean.len() % 2 != 0 {
        anyhow::bail!("hex 长度必须为偶数");
    }
    (0..clean.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&clean[i..i + 2], 16).map_err(|e| anyhow::anyhow!("{e}")))
        .collect()
}
