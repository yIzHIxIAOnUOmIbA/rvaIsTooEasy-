//! File loader: supports .bin/.exe/.dll/.elf/.macho, auto-detects file headers,
//! extracts architecture/entry point/section table; **must be memory-mapped to support >2GB**.

pub use memmap2::Mmap;
use std::path::{Path, PathBuf};
use crate::{Result, RvaError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileFormat {
    Bin,
    PE,
    ELF,
    MachO,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arch {
    X86,
    X86_64,
    Arm,
    Aarch64,
    Mips,
    Unknown,
}

impl Arch {
    pub fn from_pe_machine(m: u16) -> Arch {
        match m {
            0x014c => Arch::X86,
            0x8664 => Arch::X86_64,
            0x01c0 | 0x01c2 | 0x01c4 => Arch::Arm,
            0xaa64 => Arch::Aarch64,
            _ => Arch::Unknown,
        }
    }
    pub fn from_elf_machine(m: u16) -> Arch {
        match m {
            0x03 => Arch::X86,
            0x3e => Arch::X86_64,
            0x28 => Arch::Arm,
            0xb7 => Arch::Aarch64,
            0x08 => Arch::Mips,
            _ => Arch::Unknown,
        }
    }
}

#[derive(Debug, Clone)]
pub struct Segment {
    pub name: String,
    pub file_offset: u64,
    pub vaddr: u64,
    pub size: u64,
    pub is_executable: bool,
    pub is_writable: bool,
}

#[derive(Debug, Clone)]
pub struct FileMeta {
    pub format: FileFormat,
    pub arch: Arch,
    pub entry_point: Option<u64>,
    pub segments: Vec<Segment>,
    pub size: u64,
}

/// The mmap handle is bound to the LoadedFile lifetime: memmap2's Mmap holds the
/// file-mapping object on Windows, so the source File can be dropped safely without extra holding.
pub struct LoadedFile {
    pub path: PathBuf,
    pub meta: FileMeta,
    pub data: Mmap,
}

pub trait FileLoader {
    /// Memory-map the file and parse its metadata (architecture, entry point, section table).
    fn load(path: &Path) -> Result<LoadedFile>;

    /// Identify the format from magic bytes only, without full parsing.
    fn detect_format(path: &Path) -> Result<FileFormat> {
        let data = std::fs::read(path)?;
        Self::detect_format_bytes(&data)
    }

    /// Identify the format from the byte header.
    fn detect_format_bytes(data: &[u8]) -> Result<FileFormat> {
        if data.len() >= 2 && &data[0..2] == b"MZ" {
            Ok(FileFormat::PE)
        } else if data.len() >= 4 && &data[0..4] == b"\x7fELF" {
            Ok(FileFormat::ELF)
        } else if data.len() >= 4 {
            let magic = u32::from_le_bytes([data[0], data[1], data[2], data[3]]);
            if matches!(magic, 0xfeedface | 0xfeedfacf | 0xcafebabe | 0xcafebabf) {
                Ok(FileFormat::MachO)
            } else {
                Ok(FileFormat::Bin)
            }
        } else {
            Err(RvaError::UnsupportedFormat)
        }
    }
}

pub struct DefaultFileLoader;

impl FileLoader for DefaultFileLoader {
    fn load(path: &Path) -> Result<LoadedFile> {
        let file = std::fs::File::open(path)?;
        let size = file.metadata()?.len();
        // mmap on a static read-only file is safe; the unsafe is only required by the memmap2 API.
        let data = unsafe { Mmap::map(&file)? };
        let format = DefaultFileLoader::detect_format_bytes(&data)?;
        let (arch, entry_point, segments) = parse_meta(format, &data)
            .unwrap_or_else(|_| (Arch::Unknown, None, vec![raw_segment(size)]));
        Ok(LoadedFile {
            path: path.to_path_buf(),
            meta: FileMeta { format, arch, entry_point, segments, size },
            data,
        })
    }
}

fn raw_segment(size: u64) -> Segment {
    Segment {
        name: "<raw>".into(),
        file_offset: 0,
        vaddr: 0,
        size,
        is_executable: false,
        is_writable: false,
    }
}

// MachO segment extraction is inlined into the MachO branch (the `extract` closure) to avoid goblin's Binary type-path differences.


fn parse_meta(format: FileFormat, data: &[u8]) -> Result<(Arch, Option<u64>, Vec<Segment>)> {
    match format {
        FileFormat::Bin => Ok((Arch::Unknown, None, vec![raw_segment(data.len() as u64)])),
        FileFormat::PE => {
            let pe = goblin::pe::PE::parse(data).map_err(|e| RvaError::Parse(e.to_string()))?;
            let arch = Arch::from_pe_machine(pe.header.coff_header.machine);
            let entry = Some(pe.entry as u64);
            const EXEC: u32 = 0x20000000; // IMAGE_SCN_MEM_EXECUTE
            const WRITE: u32 = 0x80000000; // IMAGE_SCN_MEM_WRITE
            let mut segments = Vec::new();
            for s in &pe.sections {
                let name = s.name().unwrap_or("<unknown>").to_string();
                segments.push(Segment {
                    name,
                    file_offset: s.pointer_to_raw_data as u64,
                    vaddr: s.virtual_address as u64,
                    size: s.size_of_raw_data as u64,
                    is_executable: s.characteristics & EXEC != 0,
                    is_writable: s.characteristics & WRITE != 0,
                });
            }
            Ok((arch, entry, segments))
        }
        FileFormat::ELF => {
            let elf = goblin::elf::Elf::parse(data).map_err(|e| RvaError::Parse(e.to_string()))?;
            let arch = Arch::from_elf_machine(elf.header.e_machine);
            let entry = Some(elf.entry);
            const EXEC: u64 = 0x4; // SHF_EXECINSTR
            const WRITE: u64 = 0x1; // SHF_WRITE
            let mut segments = Vec::new();
            for sh in &elf.section_headers {
                let name = elf.shdr_strtab.get_at(sh.sh_name).unwrap_or("<unknown>").to_string();
                segments.push(Segment {
                    name,
                    file_offset: sh.sh_offset,
                    vaddr: sh.sh_addr,
                    size: sh.sh_size,
                    is_executable: sh.sh_flags & EXEC != 0,
                    is_writable: sh.sh_flags & WRITE != 0,
                });
            }
            Ok((arch, entry, segments))
        }
        FileFormat::MachO => {
            use goblin::mach::Mach;
            let mach = Mach::parse(data).map_err(|e| RvaError::Parse(e.to_string()))?;
            let extract = |bin: &goblin::mach::Mach| -> (Arch, Option<u64>, Vec<Segment>) {
                let mut segments = Vec::new();
                if let Mach::Binary(b) = bin {
                    for seg in b.segments.iter() {
                        let name = seg.name().unwrap_or("<unknown>").to_string();
                        segments.push(Segment {
                            name,
                            file_offset: seg.fileoff,
                            vaddr: seg.vmaddr,
                            size: seg.vmsize,
                            is_executable: false,
                            is_writable: false,
                        });
                    }
                }
                // Note: resolving the MachO entry point requires parsing LC_MAIN/thread state, which goblin does not expose directly; left as None for the prototype stage.
                (Arch::Unknown, None, segments)
            };
            match mach {
                Mach::Binary(_) => Ok(extract(&mach)),
                Mach::Fat(_) => {
                    // TODO(Phase 1+): per-architecture slice parsing for multi-arch MachO(Fat) to be added;
                    // the prototype returns an empty section table; single-arch MachO still parses fine (see the Binary branch above).
                    Ok((Arch::Unknown, None, vec![]))
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_pe() {
        let mut d = vec![0u8; 64];
        d[0] = b'M';
        d[1] = b'Z';
        assert_eq!(DefaultFileLoader::detect_format_bytes(&d).unwrap(), FileFormat::PE);
    }

    #[test]
    fn detect_elf() {
        let mut d = vec![0u8; 64];
        d[0] = 0x7f;
        d[1] = b'E';
        d[2] = b'L';
        d[3] = b'F';
        assert_eq!(DefaultFileLoader::detect_format_bytes(&d).unwrap(), FileFormat::ELF);
    }

    #[test]
    fn detect_macho() {
        let mut d = vec![0u8; 64];
        d[0..4].copy_from_slice(&0xfeedfacfu32.to_le_bytes());
        assert_eq!(DefaultFileLoader::detect_format_bytes(&d).unwrap(), FileFormat::MachO);
    }

    #[test]
    fn detect_bin_fallback() {
        let d = vec![0u8; 64];
        assert_eq!(DefaultFileLoader::detect_format_bytes(&d).unwrap(), FileFormat::Bin);
    }
}
