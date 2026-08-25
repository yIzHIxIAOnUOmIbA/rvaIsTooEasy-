use rva_core::aligner::{AlignMode, Aligner, DefaultAligner};
use rva_core::batch_comparator::{BatchComparator, BatchNode, BatchStatus, DefaultBatchComparator};
use rva_core::diff_engine::{ChangeType, DefaultDiffEngine, DiffEngine, DiffEntry, DiffStrategy};
use rva_core::file_loader::{Arch, DefaultFileLoader, FileFormat, FileLoader, LoadedFile, Mmap};
use rva_core::patch_engine::{DefaultPatchEngine, PatchEngine, PatchFormat};
use rva_core::patch_pack;
use rva_core::signing;
use rva_core::report_generator::{
    summarize, DefaultReportGenerator, DiffReport, ReportFormat, ReportGenerator,
};
use rva_core::symbol_resolver::{DefaultSymbolResolver, SymbolResolver};
use serde::Serialize;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock};
use std::path::Path;
use tauri::Emitter;

// build trigger bump 0822-2
/// In-memory cache of loaded files (path -> shared read-only mmap slice).
/// Written by `diff_files` after loading; `read_bytes`/`search_bytes` hit it first to avoid repeated disk I/O.
/// Keyed by path only (the number of files involved in a comparison is bounded), so it cannot grow unboundedly; `write_bytes` evicts the entry after writing.
fn file_cache() -> &'static Mutex<HashMap<PathBuf, Arc<Mmap>>> {
    static CACHE: OnceLock<Mutex<HashMap<PathBuf, Arc<Mmap>>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Serializable file metadata DTO (LoadedFile owns an Mmap and cannot cross IPC).
#[derive(Serialize)]
struct FileInfoDto {
    path: String,
    size: u64,
    format: String,
    arch: String,
    entry_point: Option<u64>,
}

#[derive(Serialize)]
struct SummaryDto {
    added: u64,
    removed: u64,
    modified: u64,
    total_bytes: u64,
}

/// DiffEntry's old/new are Option<Range<u64>>, and Range does not implement Serialize, so they are expanded into start/end.
#[derive(Serialize)]
struct DiffEntryDto {
    offset: u64,
    length: u64,
    change: String,
    old_start: Option<u64>,
    old_end: Option<u64>,
    new_start: Option<u64>,
    new_end: Option<u64>,
}

#[derive(Serialize)]
struct DiffResultDto {
    file_a: FileInfoDto,
    file_b: FileInfoDto,
    summary: SummaryDto,
    entries: Vec<DiffEntryDto>,
    /// Actual total number of diffs (may exceed entries.len() when truncated)
    entries_total: u64,
    /// For performance protection, entries only keeps the first N; the frontend should hint "showing first N / of M"
    entries_truncated: bool,
    /// The strategy actually applied (may be rewritten by the fallback logic)
    strategy_used: String,
    /// Whether an automatic strategy fallback occurred (e.g. sliding divergence fell back to chunked)
    strategy_fallback: bool,
}

/// Symbol table entry (address -> function name).
#[derive(Serialize)]
struct SymbolDto {
    addr: u64,
    name: String,
    size: Option<u64>,
}

/// Batch comparison tree node (recursive).
#[derive(Serialize)]
struct BatchNodeDto {
    path_a: Option<String>,
    path_b: Option<String>,
    status: String,
    diff_count: Option<usize>,
    children: Vec<BatchNodeDto>,
}

fn fmt_arch(a: Arch) -> &'static str {
    match a {
        Arch::X86 => "x86",
        Arch::X86_64 => "x86_64",
        Arch::Arm => "arm",
        Arch::Aarch64 => "aarch64",
        Arch::Mips => "mips",
        Arch::Unknown => "unknown",
    }
}

fn fmt_format(f: FileFormat) -> &'static str {
    match f {
        FileFormat::Bin => "bin",
        FileFormat::PE => "PE",
        FileFormat::ELF => "ELF",
        FileFormat::MachO => "Mach-O",
    }
}

fn fmt_change(c: ChangeType) -> &'static str {
    match c {
        ChangeType::Added => "Added",
        ChangeType::Removed => "Removed",
        ChangeType::Modified => "Modified",
    }
}

fn fmt_batch_status(s: &BatchStatus) -> &'static str {
    match s {
        BatchStatus::Identical => "Identical",
        BatchStatus::Different => "Different",
        BatchStatus::OnlyInA => "OnlyInA",
        BatchStatus::OnlyInB => "OnlyInB",
        BatchStatus::Error => "Error",
    }
}

fn file_info(lf: &LoadedFile) -> FileInfoDto {
    FileInfoDto {
        path: lf.path.display().to_string(),
        size: lf.meta.size,
        format: fmt_format(lf.meta.format).to_string(),
        arch: fmt_arch(lf.meta.arch).to_string(),
        entry_point: lf.meta.entry_point,
    }
}

/// Byte-level refinement: ChunkedHash reports Modified per 4096-byte block, which is too coarse.
/// Here each Modified block is compared byte by byte; identical prefixes/suffixes are stripped and only
/// truly different contiguous ranges are kept, so the hex viewer can highlight precisely.
fn refine_byte_level(
    a: &[u8],
    b: &[u8],
    diffs: Vec<DiffEntry>,
    progress: Option<&dyn Fn(u32)>,
) -> Vec<DiffEntry> {
    let mut out = Vec::with_capacity(diffs.len());
    let total = diffs.len();
    for (idx, d) in diffs.into_iter().enumerate() {
        if let Some(p) = progress {
            if total > 0 {
                // Refinement phase covers 70-90; the last 10% is reserved for result assembly (summarize/DTO/serialization/IPC)
                p(70 + (20 * idx / total) as u32);
            }
        }
        if d.change != ChangeType::Modified {
            out.push(d);
            continue;
        }
        let (os, oe) = match &d.old {
            Some(r) => (r.start as usize, r.end as usize),
            None => {
                out.push(d);
                continue;
            }
        };
        let (ns, ne) = match &d.new {
            Some(r) => (r.start as usize, r.end as usize),
            None => {
                out.push(d);
                continue;
            }
        };
        let alen = oe - os;
        let blen = ne - ns;
        let common = alen.min(blen);
        // Merge adjacent diff ranges into one Modified when the gap is ≤ MIN_GAP (16 bytes).
        // Otherwise, byte-by-byte comparison of two misaligned blocks would split into millions of tiny entries
        // (e.g. an insertion shifting a whole region), inflating the summary counts and pushing
        // Added/Removed entries out of the truncation window.
        const MIN_GAP: usize = 16;
        let mut runs: Vec<(usize, usize)> = Vec::new();
        let mut i = 0;
        while i < common {
            if a[os + i] != b[ns + i] {
                let start = i;
                while i < common && a[os + i] != b[ns + i] {
                    i += 1;
                }
                if let Some(last) = runs.last_mut() {
                    if start - last.1 <= MIN_GAP {
                        last.1 = i;
                    } else {
                        runs.push((start, i));
                    }
                } else {
                    runs.push((start, i));
                }
            } else {
                i += 1;
            }
        }
        for (start, end) in runs {
            // In-place modification: keep it as a single Modified (with both old/new ranges) to preserve diff precision and keep the list bounded.
            // "Only red/green, no yellow modifications" is implemented on the frontend: HexPanel splits each Modified
            // per panel via buildRanges into A-side red (Removed) + B-side green (Added); list rows render as a neutral "change" plus red/green byte comparison.
            out.push(DiffEntry {
                offset: (os + start) as u64,
                length: (end - start) as u64,
                change: ChangeType::Modified,
                old: Some((os + start) as u64..(os + end) as u64),
                new: Some((ns + start) as u64..(ns + end) as u64),
            });
        }
        // Excess bytes when the two ranges differ in length: pure removal / pure addition
        if alen > blen {
            out.push(DiffEntry {
                offset: (os + blen) as u64,
                length: (alen - blen) as u64,
                change: ChangeType::Removed,
                old: Some((os + blen) as u64..oe as u64),
                new: None,
            });
        } else if blen > alen {
            out.push(DiffEntry {
                offset: (ns + alen) as u64,
                length: (blen - alen) as u64,
                change: ChangeType::Added,
                old: None,
                new: Some((ns + alen) as u64..ne as u64),
            });
        }
    }
    out
}

// Task 4: sliding window size is configurable (settings page "sliding window size"), default 8, range 4-64
fn parse_strategy(s: Option<&str>, sliding_window: Option<u32>) -> DiffStrategy {
    match s {
        Some("sliding") => DiffStrategy::SlidingWindow {
            window: sliding_window.unwrap_or(8).clamp(4, 64) as usize,
            min_match: 8,
        },
        Some("structural") => DiffStrategy::Structural { min_run: 8 },
        _ => DiffStrategy::ChunkedHash { chunk_size: 4096 },
    }
}

fn parse_align_mode(s: Option<&str>) -> AlignMode {
    match s {
        Some("instruction") => AlignMode::Instruction,
        Some("function") => AlignMode::Function,
        _ => AlignMode::Byte,
    }
}

/// Loads both files and runs diff + align + byte-level refinement, reused by diff/report/patch.
fn load_and_diff(
    path_a: &str,
    path_b: &str,
    strategy: DiffStrategy,
    align_mode: AlignMode,
    progress: Option<&dyn Fn(u32)>,
) -> Result<(LoadedFile, LoadedFile, Vec<DiffEntry>), String> {
    if let Some(p) = progress {
        p(5);
    }
    let a = DefaultFileLoader::load(Path::new(path_a)).map_err(|e| e.to_string())?;
    if let Some(p) = progress {
        p(20);
    }
    let b = DefaultFileLoader::load(Path::new(path_b)).map_err(|e| e.to_string())?;
    if let Some(p) = progress {
        p(30);
    }
    let diffs = DefaultDiffEngine
        .diff(&a, &b, strategy)
        .map_err(|e| e.to_string())?;
    if let Some(p) = progress {
        p(60);
    }
    let diffs = DefaultAligner::default()
        .align(&a, &b, diffs, align_mode)
        .map_err(|e| e.to_string())?;
    if let Some(p) = progress {
        p(70);
    }
    let diffs = refine_byte_level(&a.data[..], &b.data[..], diffs, progress);
    // Computation ends here; the part after 90% corresponds to summarize/DTO serialization/IPC transfer,
    // which the frontend tops up after invoke returns, avoiding a "progress full but still waiting" state
    if let Some(p) = progress {
        p(90);
    }
    Ok((a, b, diffs))
}

fn entries_to_dto(diffs: Vec<DiffEntry>) -> Vec<DiffEntryDto> {
    diffs
        .into_iter()
        .map(|e| DiffEntryDto {
            offset: e.offset,
            length: e.length,
            change: fmt_change(e.change).to_string(),
            old_start: e.old.as_ref().map(|r| r.start),
            old_end: e.old.as_ref().map(|r| r.end),
            new_start: e.new.as_ref().map(|r| r.start),
            new_end: e.new.as_ref().map(|r| r.end),
        })
        .collect()
}

fn batch_node_to_dto(node: &BatchNode) -> BatchNodeDto {
    BatchNodeDto {
        path_a: node.path_a.clone(),
        path_b: node.path_b.clone(),
        status: fmt_batch_status(&node.status).to_string(),
        diff_count: node.diffs.as_ref().map(|d| d.len()),
        children: node.children.iter().map(batch_node_to_dto).collect(),
    }
}

#[tauri::command]
async fn diff_files(
    app: tauri::AppHandle,
    path_a: String,
    path_b: String,
    strategy: Option<String>,
    sliding_window: Option<u32>,
    align_mode: Option<String>,
) -> Result<DiffResultDto, String> {
    let app2 = app.clone();
    let strat = parse_strategy(strategy.as_deref(), sliding_window);
    let strat_for_fallback = strat.clone(); // the closure moves strat, so keep a copy here for the fallback check
    let amode = parse_align_mode(align_mode.as_deref());
    // Engine computation is heavy synchronous work for large files; run it on the blocking thread pool
    // to avoid blocking the Tauri main thread (otherwise the whole UI freezes during large-file diffs:
    // clicks, scrolling, and the progress bar all become unresponsive).
    // Progress is still reported to the frontend in real time via events.
    let (a, b, diffs) = tauri::async_runtime::spawn_blocking(move || {
        load_and_diff(
            &path_a,
            &path_b,
            strat,
            amode,
            Some(&|pct| {
                let _ = app2.emit("diff-progress", pct);
            }),
        )
    })
    .await
    .map_err(|e| format!("diff 任务执行失败: {e}"))??;

    // Strategy fallback: the sliding heuristic alignment diverges on binaries where an insertion
    // shifts a whole region, producing a flood of overlapping Added+Removed entries
    // (total_bytes far exceeding file size), which overwhelms the frontend and inflates byte stats.
    // If divergence is detected, automatically fall back to chunked and recompute (the files are already
    // mmap'd in memory; only the engine + align + refine rerun, so the cost is bounded),
    // and mark the DTO so the frontend can inform the user.
    let mut strategy_used = fmt_strategy_name(&strat_for_fallback).to_string();
    let mut strategy_fallback = false;
    let (a, b, diffs) = if matches!(strat_for_fallback, DiffStrategy::SlidingWindow { .. })
        && summarize(&diffs).total_bytes > (a.meta.size + b.meta.size) / 2
    {
        strategy_fallback = true;
        strategy_used = "chunked (sliding 发散自动回退)".to_string();
        let strat2 = DiffStrategy::ChunkedHash { chunk_size: 4096 };
        let d2 = DefaultDiffEngine
            .diff(&a, &b, strat2)
            .map_err(|e| e.to_string())?;
        let d2 = DefaultAligner::default()
            .align(&a, &b, d2, amode)
            .map_err(|e| e.to_string())?;
        let d2 = refine_byte_level(&a.data[..], &b.data[..], d2, None);
        (a, b, d2)
    } else {
        (a, b, diffs)
    };

    Ok(build_diff_dto(a, b, diffs, strategy_used, strategy_fallback))
}

/// Strategy name (for frontend display), corresponds to parse_strategy.
fn fmt_strategy_name(s: &DiffStrategy) -> &'static str {
    match s {
        DiffStrategy::SlidingWindow { .. } => "sliding",
        DiffStrategy::Structural { .. } => "structural",
        DiffStrategy::ChunkedHash { .. } => "chunked",
    }
}

/// Builds the DTO from (a, b, diffs): writes the file cache, summarizes, truncates, serializes.
/// Runs synchronously inside spawn_blocking, so there is no risk of blocking the main thread.
fn build_diff_dto(
    a: LoadedFile,
    b: LoadedFile,
    diffs: Vec<DiffEntry>,
    strategy_used: String,
    strategy_fallback: bool,
) -> DiffResultDto {
    // Take metadata first (a/b are still intact here), then move out data into the cache
    let fa = file_info(&a);
    let fb = file_info(&b);

    // The full file is already mmap'd in memory; write it to the cache so HexPanel's chunked reads hit it directly (zero disk I/O)
    {
        let mut cache = file_cache().lock().unwrap();
        cache.insert(a.path.clone(), Arc::new(a.data));
        cache.insert(b.path.clone(), Arc::new(b.data));
    }

    let summary = summarize(&diffs);
    // Performance guard: diff entries can be extremely numerous (byte-level diffs after a large-file
    // recompile can reach millions), and serializing/deserializing all of them over Tauri IPC would
    // block the frontend main thread for a long time and freeze the UI.
    // Here the number of entries sent to the frontend is truncated, but summary is still computed from
    // the full diffs so the overview stays accurate.
    const MAX_ENTRIES: usize = 50_000;
    let entries_total = diffs.len() as u64;
    let truncated = diffs.len() > MAX_ENTRIES;
    // Truncation strategy: Modified may use at most 70% of the budget; the rest is reserved for Added/Removed.
    // Otherwise a flood of Modified entries would push Added/Removed out of the first N, causing the
    // dropdown to show "no diff entries of this type".
    let mod_budget = MAX_ENTRIES * 7 / 10;
    let mut kept: Vec<DiffEntry> = Vec::with_capacity(MAX_ENTRIES.min(diffs.len()));
    let mut mod_count = 0usize;
    for d in diffs {
        if kept.len() >= MAX_ENTRIES {
            break;
        }
        if d.change == ChangeType::Modified {
            if mod_count >= mod_budget {
                continue; // drop Modified entries over the budget to make room for later Added/Removed
            }
            mod_count += 1;
        }
        kept.push(d);
    }
    let entries = entries_to_dto(kept);
    DiffResultDto {
        file_a: fa,
        file_b: fb,
        summary: SummaryDto {
            added: summary.added,
            removed: summary.removed,
            modified: summary.modified,
            total_bytes: summary.total_bytes,
        },
        entries,
        entries_total,
        entries_truncated: truncated,
        strategy_used,
        strategy_fallback,
    }
}

/// Reads file bytes on demand for the virtualized-scrolling hex viewer's chunked loading.
/// Hits the in-memory cache first (the mmap slice loaded by diff), zero disk I/O; falls back to disk reads on a miss.
#[tauri::command]
fn read_bytes(path: String, offset: u64, length: u64) -> Result<Vec<u8>, String> {
    let len = (length.min(1 << 20)) as usize; // per-call cap of 1MB
    if let Some(arc) = file_cache().lock().unwrap().get(Path::new(&path)) {
        let start = (offset as usize).min(arc.len());
        let end = (start + len).min(arc.len());
        if start < end {
            return Ok(arc.as_ref()[start..end].to_vec());
        }
    }
    use std::io::{Read, Seek, SeekFrom};
    let mut f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; len];
    let n = f.read(&mut buf).map_err(|e| e.to_string())?;
    buf.truncate(n);
    Ok(buf)
}

/// Writes bytes at the given offset for the hex editor to modify the file in place.
#[tauri::command]
fn write_bytes(path: String, offset: u64, data: Vec<u8>) -> Result<(), String> {
    use std::io::{Seek, SeekFrom, Write};
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .map_err(|e| e.to_string())?;
    f.seek(SeekFrom::Start(offset)).map_err(|e| e.to_string())?;
    f.write_all(&data).map_err(|e| e.to_string())?;
    // The file has been modified; invalidate the cache so the next read goes back to disk and rebuilds it
    file_cache().lock().unwrap().remove(Path::new(&path));
    Ok(())
}

/// Searches for a byte sequence in a file and returns matching offsets (at most max_matches; 0 means unlimited).
#[tauri::command]
fn search_bytes(path: String, pattern: Vec<u8>, max_matches: u64) -> Result<Vec<u64>, String> {
    use std::io::Read;
    if pattern.is_empty() {
        return Ok(Vec::new());
    }
    // If the in-memory cache is hit, search the mmap slice directly to avoid reading the whole file from disk
    let buf: Vec<u8> = if let Some(arc) = file_cache().lock().unwrap().get(Path::new(&path)) {
        arc.as_ref().to_vec()
    } else {
        let mut f = std::fs::File::open(&path).map_err(|e| e.to_string())?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf).map_err(|e| e.to_string())?;
        buf
    };
    let mut out = Vec::new();
    let mut i = 0usize;
    while i + pattern.len() <= buf.len() {
        if &buf[i..i + pattern.len()] == &pattern[..] {
            out.push(i as u64);
            if out.len() as u64 >= max_matches.max(1) {
                break;
            }
        }
        i += 1;
    }
    Ok(out)
}

/// Copies a file (save-as / backup).
#[tauri::command]
fn copy_file(src: String, dst: String) -> Result<(), String> {
    std::fs::copy(&src, &dst).map_err(|e| e.to_string())?;
    Ok(())
}

/// Parses a binary's symbol table (PE export table / ELF .symtab / Mach-O symbols).
#[tauri::command]
fn symbols(path: String) -> Result<Vec<SymbolDto>, String> {
    let map = DefaultSymbolResolver::resolve(Path::new(&path), None).map_err(|e| e.to_string())?;
    let mut v: Vec<SymbolDto> = map
        .0
        .into_values()
        .map(|s| SymbolDto {
            addr: s.addr,
            name: s.name,
            size: s.size,
        })
        .collect();
    v.sort_by_key(|s| s.addr);
    Ok(v)
}

/// Recursively compares two directories and returns a tree-shaped result.
#[tauri::command]
fn batch_compare(dir_a: String, dir_b: String) -> Result<BatchNodeDto, String> {
    let node = DefaultBatchComparator::compare_dirs(
        Path::new(&dir_a),
        Path::new(&dir_b),
        DiffStrategy::ChunkedHash { chunk_size: 4096 },
    )
    .map_err(|e| e.to_string())?;
    Ok(batch_node_to_dto(&node))
}

/// Generates a patch and returns a readable JSON string (saved by the frontend as a .rva patch file).
#[tauri::command]
fn patch_generate(path_a: String, path_b: String) -> Result<String, String> {
    let (a, b, diffs) = load_and_diff(&path_a, &path_b, parse_strategy(None, None), parse_align_mode(None), None)?;
    let patch = <DefaultPatchEngine as PatchEngine>::generate(&a, &b, &diffs, PatchFormat::Custom)
        .map_err(|e| e.to_string())?;
    let bytes = DefaultPatchEngine::serialize(&patch).map_err(|e| e.to_string())?;
    String::from_utf8(bytes).map_err(|e| e.to_string())
}

/// Applies a patch: old + patch -> out.
#[tauri::command]
fn patch_apply(path_old: String, path_patch: String, path_out: String) -> Result<(), String> {
    let old = DefaultFileLoader::load(Path::new(&path_old)).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&path_patch).map_err(|e| e.to_string())?;
    let patch = DefaultPatchEngine::deserialize(&bytes).map_err(|e| e.to_string())?;
    DefaultPatchEngine::apply(&old, &patch, Path::new(&path_out)).map_err(|e| e.to_string())
}

/// Generates a report (html/txt/json) and returns the string (saved to a file by the frontend).
#[tauri::command]
fn export_report(path_a: String, path_b: String, format: String) -> Result<String, String> {
    let (_a, _b, diffs) = load_and_diff(&path_a, &path_b, parse_strategy(None, None), parse_align_mode(None), None)?;
    let summary = summarize(&diffs);
    let report = DiffReport {
        entries: diffs,
        symbols: None,
        summary,
    };
    let fmt = ReportFormat::from_str_checked(&format);
    <DefaultReportGenerator as ReportGenerator>::generate(&report, fmt).map_err(|e| e.to_string())
}

/// Writes a text file (for patch / report export).
#[tauri::command]
fn write_text_file(path: String, content: String) -> Result<(), String> {
    std::fs::write(&path, content).map_err(|e| e.to_string())
}

// ---------- Task B: patch ecosystem (.rvapatch signed container / keystore / apply & rollback) ----------

#[derive(Serialize)]
struct PackedVerifyDto {
    ok: bool,
    message: String,
    source_sha256: String,
    target_sha256: String,
    timestamp: i64,
    engine_version: u32,
    strategy: String,
    entry_count: u32,
    signatures: Vec<SignatureDto>,
}

#[derive(Serialize)]
struct SignatureDto {
    fingerprint: String,
    valid: bool,
}

fn strategy_from_u8(v: u8) -> DiffStrategy {
    match v {
        1 => DiffStrategy::SlidingWindow { window: 8, min_match: 8 },
        2 => DiffStrategy::Structural { min_run: 8 },
        _ => DiffStrategy::ChunkedHash { chunk_size: 4096 },
    }
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Generates a new signing key pair (private key encrypted with OS DPAPI).
#[tauri::command]
fn keystore_generate(name: String) -> Result<signing::KeyInfo, String> {
    signing::generate_keypair(&name).map_err(|e| e.to_string())
}

/// Lists all keys in the keystore.
#[tauri::command]
fn keystore_list() -> Result<Vec<signing::KeyInfo>, String> {
    signing::list_keys().map_err(|e| e.to_string())
}

/// Deletes a key (private key + public key).
#[tauri::command]
fn keystore_delete(name: String) -> Result<(), String> {
    signing::delete_key(&name).map_err(|e| e.to_string())
}

/// Exports the public key (hex, distributed to verifiers to import as trusted).
#[tauri::command]
fn keystore_export_pub(name: String) -> Result<String, String> {
    let k = signing::public_key_bytes(&name).map_err(|e| e.to_string())?;
    Ok(hex_encode(&k))
}

/// Imports an external public key (trust only; no private key is produced).
#[tauri::command]
fn keystore_import_pub(name: String, pub_hex: String) -> Result<signing::KeyInfo, String> {
    if pub_hex.len() != 64 || !pub_hex.chars().all(|c| c.is_ascii_hexdigit()) {
        return Err("公钥须为 64 位十六进制".into());
    }
    let bytes = (0..32)
        .map(|i| u8::from_str_radix(&pub_hex[i * 2..i * 2 + 2], 16).unwrap())
        .collect::<Vec<_>>();
    signing::import_public(&name, &bytes).map_err(|e| e.to_string())
}

/// Generates a patch + signature and outputs the `.rvapatch` container bytes (saved to a file by the frontend).
#[tauri::command]
fn patch_pack_sign(path_a: String, path_b: String, strategy: u8, signer_name: String) -> Result<Vec<u8>, String> {
    let (a, b, diffs) = load_and_diff(&path_a, &path_b, strategy_from_u8(strategy), parse_align_mode(None), None)?;
    let patch = <DefaultPatchEngine as PatchEngine>::generate(&a, &b, &diffs, PatchFormat::Custom)
        .map_err(|e| e.to_string())?;
    let old = std::fs::read(&path_a).map_err(|e| e.to_string())?;
    let new = std::fs::read(&path_b).map_err(|e| e.to_string())?;
    let revert = patch_pack::build_revert_segments(&old, &patch);
    let meta = patch_pack::PatchMeta {
        source_sha256: patch_pack::sha256_of(&old),
        target_sha256: patch_pack::sha256_of(&new),
        timestamp: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        engine_version: 1,
        strategy,
        entry_count: patch.ops.len() as u32,
    };
    let mut packed = patch_pack::PackedPatch::new(meta, &patch, revert).map_err(|e| e.to_string())?;
    let sig = signing::sign_bytes(&signer_name, &packed.content_bytes()).map_err(|e| e.to_string())?;
    packed.add_signature(sig);
    Ok(packed.to_bytes())
}

/// Parses and verifies a `.rvapatch` container (returns metadata + validity of each signature).
#[tauri::command]
fn patch_pack_verify(packed: Vec<u8>) -> Result<PackedVerifyDto, String> {
    let p = patch_pack::PackedPatch::from_bytes(&packed).map_err(|e| e.to_string())?;
    let content = p.content_bytes();
    let mut sigs = Vec::new();
    for s in &p.signatures {
        let valid = signing::verify_by_fingerprint(&content, s).map_err(|e| e.to_string())?;
        sigs.push(SignatureDto { fingerprint: hex_encode(&s.fingerprint), valid });
    }
    let any_valid = sigs.iter().any(|s| s.valid);
    let strategy = match p.metadata.strategy {
        1 => "滑动窗口".to_string(),
        2 => "函数级结构匹配".to_string(),
        _ => "分块哈希".to_string(),
    };
    Ok(PackedVerifyDto {
        ok: !p.signatures.is_empty() && any_valid,
        message: if p.signatures.is_empty() {
            "补丁未签名".into()
        } else if any_valid {
            "补丁有效：签名验证通过".into()
        } else {
            "签名验证失败：无匹配可信公钥".into()
        },
        source_sha256: hex_encode(&p.metadata.source_sha256),
        target_sha256: hex_encode(&p.metadata.target_sha256),
        timestamp: p.metadata.timestamp,
        engine_version: p.metadata.engine_version,
        strategy,
        entry_count: p.metadata.entry_count,
        signatures: sigs,
    })
}

/// Applies a `.rvapatch` (verifies source SHA256 + signature + rebuilds target SHA256).
#[tauri::command]
fn patch_apply_packed(packed: Vec<u8>, source_path: String, out_path: String) -> Result<rva_core::apply::ApplyResult, String> {
    let p = patch_pack::PackedPatch::from_bytes(&packed).map_err(|e| e.to_string())?;
    rva_core::apply::apply_patch(Path::new(&source_path), &p, Path::new(&out_path)).map_err(|e| e.to_string())
}

/// Rolls back a `.rvapatch` (verifies current SHA256 + signature + restores source from snapshot).
#[tauri::command]
fn patch_rollback_packed(packed: Vec<u8>, current_path: String, out_path: String) -> Result<rva_core::apply::ApplyResult, String> {
    let p = patch_pack::PackedPatch::from_bytes(&packed).map_err(|e| e.to_string())?;
    rva_core::apply::rollback_patch(Path::new(&current_path), &p, Path::new(&out_path)).map_err(|e| e.to_string())
}

/// Writes a binary file (for saving .rvapatch).
#[tauri::command]
fn write_binary_file(path: String, data: Vec<u8>) -> Result<(), String> {
    std::fs::write(&path, data).map_err(|e| e.to_string())
}

/// Reads a binary file (for .rvapatch verification / appending signatures).
#[tauri::command]
fn read_binary_file(path: String) -> Result<Vec<u8>, String> {
    std::fs::read(&path).map_err(|e| e.to_string())
}

/// Reads patch apply/rollback history (newest first).
#[tauri::command]
fn patch_history() -> Result<Vec<serde_json::Value>, String> {
    let dir = rva_core::apply::history_dir();
    let mut out = Vec::new();
    if dir.exists() {
        let mut files = std::fs::read_dir(&dir)
            .map_err(|e| e.to_string())?
            .filter_map(|e| e.ok())
            .map(|e| e.path())
            .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
            .collect::<Vec<_>>();
        files.sort();
        for f in files.into_iter().rev() {
            if let Ok(text) = std::fs::read_to_string(&f) {
                if let Ok(v) = serde_json::from_str::<serde_json::Value>(&text) {
                    out.push(v);
                }
            }
        }
    }
    Ok(out)
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .invoke_handler(tauri::generate_handler![
            diff_files,
            read_bytes,
            write_bytes,
            search_bytes,
            copy_file,
            symbols,
            batch_compare,
            patch_generate,
            patch_apply,
            export_report,
            write_text_file,
            keystore_generate,
            keystore_list,
            keystore_delete,
            keystore_export_pub,
            keystore_import_pub,
            patch_pack_sign,
            patch_pack_verify,
            patch_apply_packed,
            patch_rollback_packed,
            patch_history,
            write_binary_file,
            read_binary_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
