# rvaIsTooEasy

A lightweight binary diff / compare tool for **security analysis and firmware reverse engineering**.

- Memory-mapped loading of large files (> 2 GB)
- Multi-format parsing (.bin / .exe / .dll / .elf / .macho)
- Chunked hashing + sliding-window diff detection
- Three-level smart alignment (byte / instruction / function)
- PDB / DWARF symbol recovery
- Custom / xdelta3 patch generation and application
- HTML / TXT / JSON reports + a Tauri dual-pane hex viewer

## Quick Start

```bash
# 1. Build
cargo build --release

# 2. Diff two binaries from the CLI (prints the diff to the console)
cargo run --release --bin rva -- diff old.bin new.bin

# 3. Generate an HTML report
cargo run --release --bin rva -- report old.bin new.bin html

# 4. Generate a patch (.rvapatch container)
cargo run --release --bin rva -- patch generate -o update.rvapatch old.bin new.bin

# 5. Apply a patch
cargo run --release --bin rva -- patch apply -o restored.bin old.bin update.rvapatch

# 6. Launch the GUI (Tauri dual-pane viewer)
cargo run --release -p rva_gui
```

## Performance

| Scenario     | 64 MB elapsed | Throughput |
| ------------ | ------------- | ---------- |
| chunked-4K   | ~115 ms       | ~554 MB/s  |
| sliding-16   | ~208 ms       | ~306 MB/s  |

## Tech Stack

- Language: **Rust**
- GUI: **Tauri v2** (web frontend, lightweight and fast)
- Core crates: `goblin`, `memmap2`, `blake3` / `xxhash`, `serde`, `clap`, `symbolic`, `diffy`

## Status

| Phase | Scope                                        | Status |
| ----- | -------------------------------------------- | ------ |
| 1     | File Loader + Diff Engine + CLI `diff`       | ✅     |
| 2     | Aligner + Report Generator + CLI `report`    | ✅     |
| 3     | Symbol Resolver + Patch Engine               | ✅     |
| 4     | Tauri GUI dual-pane viewer                   | ✅     |
| 5     | Batch Comparator + tree results              | ✅     |

Core trait interfaces are frozen in `crates/rva_core/src/*.rs`. 51+ tests are green
(including accuracy false-positive rate / robustness / patch round-trip / rolling-hash equivalence).
