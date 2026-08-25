//! Batch comparator: recursively walks two directories, runs the Diff Engine on each matching file, and shows a tree.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::diff_engine::{DefaultDiffEngine, DiffEngine, DiffEntry, DiffStrategy};
use crate::file_loader::{DefaultFileLoader, FileLoader};
use crate::Result;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BatchStatus {
    Identical,
    Different,
    OnlyInA,
    OnlyInB,
    Error,
}

#[derive(Debug, Clone)]
pub struct BatchNode {
    pub path_a: Option<String>,
    pub path_b: Option<String>,
    pub status: BatchStatus,
    pub diffs: Option<Vec<DiffEntry>>,
    pub children: Vec<BatchNode>,
}

pub trait BatchComparator {
    fn compare_dirs(a: &Path, b: &Path, strategy: DiffStrategy) -> Result<BatchNode>;
}

pub struct DefaultBatchComparator;

impl BatchComparator for DefaultBatchComparator {
    fn compare_dirs(a: &Path, b: &Path, strategy: DiffStrategy) -> Result<BatchNode> {
        // Recursively collect a "relative path -> absolute path" map of every file under both dirs.
        let mut files_a: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
        let mut files_b: BTreeMap<PathBuf, PathBuf> = BTreeMap::new();
        collect_files(a, a, &mut files_a)?;
        collect_files(b, b, &mut files_b)?;

        // Build a tree mirroring the directory structure keyed by relative path (root is a virtual root).
        let mut root = BatchNode {
            path_a: Some(a.to_string_lossy().into_owned()),
            path_b: Some(b.to_string_lossy().into_owned()),
            status: BatchStatus::Different,
            diffs: None,
            children: Vec::new(),
        };
        build_tree(&mut root, &files_a, &files_b, &strategy);

        // Root status is Identical only if all children are Identical (including both dirs being empty).
        finalize_dir(&mut root);
        Ok(root)
    }
}

/// Recursively walk all files under `dir`, keyed by paths relative to `base`.
fn collect_files(base: &Path, dir: &Path, out: &mut BTreeMap<PathBuf, PathBuf>) -> Result<()> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_dir() {
            collect_files(base, &path, out)?;
        } else {
            let rel = path.strip_prefix(base).map_err(|_| {
                crate::err(format!("path {} not under base {}", path.display(), base.display()))
            })?;
            out.insert(rel.to_path_buf(), path);
        }
    }
    Ok(())
}

/// Take entries of `files` whose first component equals `comp`, dropping the first component (one level down).
fn sub_map(files: &BTreeMap<PathBuf, PathBuf>, comp: &Path) -> BTreeMap<PathBuf, PathBuf> {
    files
        .iter()
        .filter(|(k, _)| k.components().next().map(|c| c.as_os_str()) == Some(comp.as_os_str()))
        .map(|(k, v)| (strip_first(k), v.clone()))
        .collect()
}

/// Drop the first path component; a single-component path becomes an empty path (meaning "leaf file").
fn strip_first(p: &Path) -> PathBuf {
    let mut it = p.components();
    it.next();
    it.as_path().to_path_buf()
}

/// Recursively build tree nodes by the first-level component of the relative path.
fn build_tree(
    node: &mut BatchNode,
    files_a: &BTreeMap<PathBuf, PathBuf>,
    files_b: &BTreeMap<PathBuf, PathBuf>,
    strategy: &DiffStrategy,
) {
    // Collect all top-level components of this layer (dir names or file names); BTreeMap is naturally ordered, guaranteeing deterministic output.
    let mut components: Vec<PathBuf> = Vec::new();
    for key in files_a.keys().chain(files_b.keys()) {
        if let Some(first) = key.components().next() {
            let p = PathBuf::from(first.as_os_str());
            if !components.contains(&p) {
                components.push(p);
            }
        }
    }

    for comp in components {
        let sub_a = sub_map(files_a, &comp);
        let sub_b = sub_map(files_b, &comp);
        // An empty key (path empty after dropping the first component) means the component itself is a file.
        let a_is_file = sub_a.contains_key(Path::new(""));
        let b_is_file = sub_b.contains_key(Path::new(""));

        if a_is_file && b_is_file {
            // Same-named file on both sides: diff file by file.
            let abs_a = &sub_a[Path::new("")];
            let abs_b = &sub_b[Path::new("")];
            let (status, diffs) = compare_files(abs_a, abs_b, strategy);
            node.children.push(BatchNode {
                path_a: Some(abs_a.to_string_lossy().into_owned()),
                path_b: Some(abs_b.to_string_lossy().into_owned()),
                status,
                diffs,
                children: Vec::new(),
            });
        } else if a_is_file {
            // Only A side is a file.
            let abs = &sub_a[Path::new("")];
            node.children.push(BatchNode {
                path_a: Some(abs.to_string_lossy().into_owned()),
                path_b: None,
                status: BatchStatus::OnlyInA,
                diffs: None,
                children: Vec::new(),
            });
            // If the same name on the B side is a directory, recurse to show the B dir.
            if !sub_b.is_empty() {
                let mut dir = dir_node(&comp);
                build_tree(&mut dir, &empty_map(), &sub_b, strategy);
                node.children.push(dir);
            }
        } else if b_is_file {
            // Only B side is a file.
            let abs = &sub_b[Path::new("")];
            node.children.push(BatchNode {
                path_a: None,
                path_b: Some(abs.to_string_lossy().into_owned()),
                status: BatchStatus::OnlyInB,
                diffs: None,
                children: Vec::new(),
            });
            if !sub_a.is_empty() {
                let mut dir = dir_node(&comp);
                build_tree(&mut dir, &sub_a, &empty_map(), strategy);
                node.children.push(dir);
            }
        } else {
            // Both sides are directories: recurse and aggregate status from children.
            let mut dir = dir_node(&comp);
            build_tree(&mut dir, &sub_a, &sub_b, strategy);
            finalize_dir(&mut dir);
            node.children.push(dir);
        }
    }
}

/// A directory node is Identical only if all children are Identical; otherwise Different.
fn finalize_dir(node: &mut BatchNode) {
    node.status = if node.children.iter().all(|c| c.status == BatchStatus::Identical) {
        BatchStatus::Identical
    } else {
        BatchStatus::Different
    };
}

fn dir_node(comp: &Path) -> BatchNode {
    BatchNode {
        path_a: Some(comp.to_string_lossy().into_owned()),
        path_b: Some(comp.to_string_lossy().into_owned()),
        status: BatchStatus::Different,
        diffs: None,
        children: Vec::new(),
    }
}

fn empty_map() -> BTreeMap<PathBuf, PathBuf> {
    BTreeMap::new()
}

/// Diff a single file pair, returning (status, diffs).
fn compare_files(
    abs_a: &Path,
    abs_b: &Path,
    strategy: &DiffStrategy,
) -> (BatchStatus, Option<Vec<DiffEntry>>) {
    let fa = match DefaultFileLoader::load(abs_a) {
        Ok(f) => f,
        Err(_) => return (BatchStatus::Error, None),
    };
    let fb = match DefaultFileLoader::load(abs_b) {
        Ok(f) => f,
        Err(_) => return (BatchStatus::Error, None),
    };
    if fa.data.as_ref() == fb.data.as_ref() {
        return (BatchStatus::Identical, None);
    }
    let diffs = DefaultDiffEngine.diff(&fa, &fb, strategy.clone()).ok();
    (BatchStatus::Different, diffs)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn setup(dir: &Path) {
        fs::create_dir_all(dir.join("sub")).unwrap();
        fs::write(dir.join("same.bin"), vec![0u8; 16]).unwrap();
        fs::write(dir.join("sub/changed.bin"), vec![1u8, 2, 3, 4]).unwrap();
    }

    fn temp_base(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("rva_batch_{}_{}", tag, std::process::id()))
    }

    #[test]
    fn compare_identical_dirs() {
        let base = temp_base("same");
        let a = base.join("a");
        let b = base.join("b");
        let _ = fs::remove_dir_all(&base);
        setup(&a);
        setup(&b);
        let node = DefaultBatchComparator::compare_dirs(
            &a,
            &b,
            DiffStrategy::ChunkedHash { chunk_size: 4096 },
        )
        .unwrap();
        assert_eq!(node.status, BatchStatus::Identical);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn compare_different_dirs() {
        let base = temp_base("diff");
        let a = base.join("a");
        let b = base.join("b");
        let _ = fs::remove_dir_all(&base);
        setup(&a);
        setup(&b);
        fs::write(b.join("sub/changed.bin"), vec![9u8, 9, 9, 9]).unwrap();
        let node = DefaultBatchComparator::compare_dirs(
            &a,
            &b,
            DiffStrategy::ChunkedHash { chunk_size: 4096 },
        )
        .unwrap();
        assert_eq!(node.status, BatchStatus::Different);
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn only_in_one_side() {
        let base = temp_base("only");
        let a = base.join("a");
        let b = base.join("b");
        let _ = fs::remove_dir_all(&base);
        setup(&a);
        setup(&b);
        fs::write(a.join("only_a.bin"), vec![7u8; 8]).unwrap();
        let node = DefaultBatchComparator::compare_dirs(
            &a,
            &b,
            DiffStrategy::ChunkedHash { chunk_size: 4096 },
        )
        .unwrap();
        assert!(node.children.iter().any(|c| c.status == BatchStatus::OnlyInA));
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn multi_file_subdir_recurses() {
        let base = temp_base("multi");
        let a = base.join("a");
        let b = base.join("b");
        let _ = fs::remove_dir_all(&base);
        setup(&a);
        setup(&b);
        // Add another file under the sub directory to verify multi-file directories recurse correctly instead of being treated as files.
        fs::write(a.join("sub/extra.bin"), vec![5u8; 4]).unwrap();
        fs::write(b.join("sub/extra.bin"), vec![5u8; 4]).unwrap();
        let node = DefaultBatchComparator::compare_dirs(
            &a,
            &b,
            DiffStrategy::ChunkedHash { chunk_size: 4096 },
        )
        .unwrap();
        assert_eq!(node.status, BatchStatus::Identical);
        let _ = fs::remove_dir_all(&base);
    }
}
