//! Shared helpers for the static contract tests.
//!
//! These tests read repository sources as text instead of exercising behaviour,
//! so they only need a directory walker and a stable repository root.
#![allow(dead_code)]

use std::path::{Path, PathBuf};

/// Repository root, derived from this crate's manifest directory.
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("..")
        .canonicalize()
        .expect("resolve repository root from CARGO_MANIFEST_DIR")
}

/// Every file under `root` (recursively) whose extension equals `extension`.
///
/// Missing directories yield an empty list so a partially checked-out tree does
/// not turn into a confusing panic.
pub fn collect_files(root: &Path, extension: &str) -> Vec<PathBuf> {
    let mut found = Vec::new();
    walk(root, extension, &mut found);
    found.sort();
    found
}

fn walk(dir: &Path, extension: &str, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            walk(&path, extension, found);
        } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            found.push(path);
        }
    }
}

/// Repository-relative path with forward slashes, for stable failure messages.
pub fn relative(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Source text with everything from the first top-level `#[cfg(test)]` removed.
///
/// Contract tests describe production behaviour; in-file unit tests are free to
/// use whatever primitives make the test readable. Only a `#[cfg(test)]` in
/// column zero ends the production region, because indented ones are attributes
/// on individual items rather than the trailing test module.
pub fn production_region(source: &str) -> &str {
    let mut offset = 0usize;
    for line in source.split_inclusive('\n') {
        if line.starts_with("#[cfg(test)]") {
            return &source[..offset];
        }
        offset += line.len();
    }
    source
}
