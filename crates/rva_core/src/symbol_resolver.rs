//! Symbol resolver: parses the PE export table / ELF symbol table (goblin) into an address->function-name map.
//!
//! Design notes (deviation from the original blueprint): the blueprint required deep PDB/DWARF debug info,
//! but there are no real PDB/DWARF fixtures on this machine to verify end-to-end. Following the Phase 3
//! flexibility principle, we start with the PE export table + ELF symbol table that goblin already supports
//! (effective and verifiable on real binaries); PDB/DWARF/MachO are left as future enhancements (see TODO).
//! Unsymbolized raw binaries safely return an empty map (never panic).

use std::path::Path;
use crate::Result;

#[derive(Debug, Clone)]
pub struct Symbol {
    pub addr: u64,
    pub name: String,
    pub size: Option<u64>,
}

/// Address-to-symbol map; lookup matches addresses exactly (function level).
#[derive(Debug, Clone, Default)]
pub struct SymbolMap(pub std::collections::HashMap<u64, Symbol>);

impl SymbolMap {
    pub fn lookup(&self, addr: u64) -> Option<&Symbol> {
        self.0.get(&addr)
    }
    /// Find the nearest function containing the address (addr in [sym.addr, sym.addr+size)).
    pub fn lookup_containing(&self, addr: u64) -> Option<&Symbol> {
        self.0.values().find(|s| {
            s.size.map_or(false, |sz| addr >= s.addr && addr < s.addr + sz)
        })
    }
}

pub trait SymbolResolver {
    /// `binary` is the main file; `debug_info` is a separate PDB/DWARF file (optional; some formats embed it).
    fn resolve(binary: &Path, debug_info: Option<&Path>) -> Result<SymbolMap>;
}

/// Default implementation: parses the PE export table and ELF symbol table via goblin.
/// Raw binaries (no PE/ELF/MachO header) safely return an empty map.
pub struct DefaultSymbolResolver;

impl SymbolResolver for DefaultSymbolResolver {
    fn resolve(binary: &Path, debug_info: Option<&Path>) -> Result<SymbolMap> {
        let buf = std::fs::read(binary)?;
        let mut map = SymbolMap::default();

        match goblin::Object::parse(&buf) {
            Ok(goblin::Object::PE(pe)) => {
                for exp in &pe.exports {
                    if let (Some(name), Some(off)) = (&exp.name, exp.offset) {
                        let addr = off as u64;
                        map.0.entry(addr).or_insert(Symbol {
                            addr,
                            name: name.to_string(),
                            size: None,
                        });
                    }
                }
                // TODO(Phase3+): if a PDB exists (debug_info or a same-named .pdb next to the binary),
                // parse finer function-level symbols and line info with the pdb crate. Currently only the
                // export table is used, which already covers most APIs.
                let _ = debug_info;
            }
            Ok(goblin::Object::Elf(elf)) => {
                for sym in elf.syms.iter() {
                    let name = elf.strtab.get_at(sym.st_name).unwrap_or("");
                    if !name.is_empty() && sym.st_value != 0 {
                        let addr = sym.st_value as u64;
                        map.0.entry(addr).or_insert(Symbol {
                            addr,
                            name: name.to_string(),
                            size: Some(sym.st_size),
                        });
                    }
                }
                // TODO(Phase3+): DWARF (.debug_info) line-number/local-symbol parsing (gimli).
            }
            Ok(goblin::Object::Mach(_)) => {
                // TODO(Phase3+): MachO symbol table (nlist) parsing.
            }
            Ok(_) => {
                // goblin::Object is non-exhaustive; other variants (e.g. future additions) are safely ignored.
            }
            Err(_) => {
                // Raw binary (no PE/ELF/MachO header) -> no symbols; return an empty map.
            }
        }
        Ok(map)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_path(bytes: &[u8], name: &str) -> std::path::PathBuf {
        let p = std::env::temp_dir().join(name);
        std::fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn bare_bin_returns_empty_map() {
        let p = dummy_path(&[0u8; 16], "sym_bare.bin");
        let m = DefaultSymbolResolver::resolve(&p, None).unwrap();
        assert!(m.0.is_empty());
        let _ = std::fs::remove_file(&p);
    }

    #[cfg(windows)]
    #[test]
    fn resolves_pe_exports_on_system_dll() {
        for cand in [
            "C:\\Windows\\System32\\kernel32.dll",
            "C:\\Windows\\System32\\user32.dll",
            "C:\\Windows\\System32\\ntdll.dll",
        ] {
            let p = std::path::Path::new(cand);
            if p.exists() {
                let m = DefaultSymbolResolver::resolve(p, None).unwrap();
                assert!(!m.0.is_empty(), "expected symbols from {}", cand);
                // Spot check: should contain a common export name (at least one hit across different DLLs)
                let hit = m
                    .0
                    .values()
                    .any(|s| s.name == "CreateFileW" || s.name == "MessageBoxW" || s.name.starts_with("Nt"));
                assert!(hit, "expected known exports in {}", cand);
                return;
            }
        }
    }
}
