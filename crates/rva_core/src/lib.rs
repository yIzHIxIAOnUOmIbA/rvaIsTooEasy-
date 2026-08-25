//! rva_core - the rvaIsTooEasy core library (UI-independent).
//! All modules are defined as traits; implementations live in each submodule file.
//! The types/errors defined in this file are the single cross-module authority; subagent
//! implementations must not change the signatures.

pub mod file_loader;
pub mod diff_engine;
pub mod aligner;
pub mod symbol_resolver;
pub mod patch_engine;
pub mod report_generator;
pub mod batch_comparator;
pub mod disasm;
pub mod structural_matcher;
pub mod patch_pack;
pub mod signing;
pub mod apply;

use std::path::PathBuf;

#[derive(Debug, thiserror::Error)]
pub enum RvaError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("unsupported format")]
    UnsupportedFormat,
    #[error("symbol resolve failed: {0}")]
    Symbol(String),
    #[error("patch error: {0}")]
    Patch(String),
    #[error("{0}")]
    Other(String),
}

pub type Result<T> = std::result::Result<T, RvaError>;

/// Convenience constructor for Other errors.
pub fn err(msg: impl Into<String>) -> RvaError {
    RvaError::Other(msg.into())
}

/// Unified intermediate model: diff results + optional symbols + summary, consumed by Report/Patch/GUI.
pub use diff_engine::DiffEntry;
pub use report_generator::{DiffReport, ReportSummary};
pub use file_loader::LoadedFile;
pub use symbol_resolver::SymbolMap;
pub use batch_comparator::{BatchComparator, BatchNode, BatchStatus, DefaultBatchComparator};

#[allow(dead_code)]
fn _assert_pathbuf(_: PathBuf) {}
