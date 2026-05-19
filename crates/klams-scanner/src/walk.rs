//! Filesystem walker built on the `ignore` crate. Honours `.gitignore`
//! and `.klamsignore`, always skips a handful of build/cache dirs,
//! and yields `(absolute_path, mtime_ns, file_size)` so the cursor
//! layer can short-circuit on unchanged files.

use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

const ALWAYS_SKIP: &[&str] = &[
    "target",
    "node_modules",
    ".git",
    "__pycache__",
    ".venv",
    "dist",
    "build",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkedFile {
    pub absolute_path: PathBuf,
    pub mtime_ns: i64,
    pub file_size: u64,
}

/// Walk `root`, returning every file (not directory) whose path is
/// not excluded by ignore files or the always-skip list.
#[must_use]
pub fn walk(root: &Path) -> Vec<WalkedFile> {
    let mut out = Vec::new();
    let mut b = WalkBuilder::new(root);
    b.standard_filters(true)
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false)
        .hidden(false)
        .add_custom_ignore_filename(".klamsignore");
    let walker = b.build();
    for entry in walker {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path == root {
            continue;
        }
        if is_in_skip_list(path) {
            continue;
        }
        let Some(ft) = entry.file_type() else {
            continue;
        };
        if !ft.is_file() {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let mtime_ns = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX));
        out.push(WalkedFile {
            absolute_path: path.to_path_buf(),
            mtime_ns,
            file_size: meta.len(),
        });
    }
    out
}

fn is_in_skip_list(path: &Path) -> bool {
    path.components().any(|c| {
        let s = c.as_os_str().to_string_lossy();
        ALWAYS_SKIP.contains(&s.as_ref())
    })
}
