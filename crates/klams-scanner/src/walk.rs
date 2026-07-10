//! Filesystem walker built on the `ignore` crate. Honours `.gitignore`
//! and `.klamsignore`, always skips a handful of build/cache dirs,
//! and yields `(absolute_path, mtime_ns, file_size)` so the cursor
//! layer can short-circuit on unchanged files.

use ignore::WalkBuilder;
use std::path::{Path, PathBuf};

const ALWAYS_SKIP: &[&str] = &[
    "target",
    "node_modules",
    ".pnpm-store",
    ".git",
    "__pycache__",
    ".venv",
    "venv",
    ".mypy_cache",
    ".pytest_cache",
    ".ruff_cache",
    ".tox",
    ".gradle",
    ".idea",
    ".svelte-kit",
    ".next",
    ".nuxt",
    ".cache",
    ".terraform",
    ".obsidian",
    "dist",
    "build",
];

/// Extensions worth indexing — source code, docs/prose, and config
/// prose. Everything else (lockfiles, JSON fixtures, SVGs, images,
/// archives, binaries) is noise in a recall corpus and is dropped
/// before it reaches the chunker (sprint 021, #316). Aggressive on
/// purpose: a false negative is recoverable (add the extension, the
/// miss log will surface demand), a false positive costs tokens on
/// every retrieval. Compared case-insensitively.
const ALLOW_EXT: &[&str] = &[
    // Rust / systems
    "rs",
    "toml",
    "c",
    "h",
    "cc",
    "cpp",
    "cxx",
    "hpp",
    "go",
    "zig",
    // Python
    "py",
    "pyi", // JS / TS / web frameworks
    "js",
    "jsx",
    "mjs",
    "cjs",
    "ts",
    "tsx",
    "svelte",
    "vue",
    // Web
    "html",
    "htm",
    "css",
    "scss",
    "sass", // Shell / scripting
    "sh",
    "bash",
    "zsh",
    "fish",
    "ps1",
    "rb",
    "lua",
    "pl",
    // Docs / prose
    "md",
    "mdx",
    "markdown",
    "txt",
    "rst",
    "adoc",
    "asciidoc",
    "org",
    "tex",
    // Config prose (yaml/yml kept: compose, ansible, k8s, CI are real
    // homelab knowledge — lockfiles are filtered by name below)
    "yaml",
    "yml",
    "ini",
    "cfg",
    "conf",
    "env",
    "properties",
    "sql",
    "proto",
];

/// Extensionless filenames worth indexing — build/ops files that carry
/// real structure but have no extension for `ALLOW_EXT` to match.
/// Compared case-insensitively against the whole file name.
const ALLOW_NAMES: &[&str] = &[
    "dockerfile",
    "containerfile",
    "makefile",
    "gnumakefile",
    "justfile",
    "readme",
    "vagrantfile",
    "procfile",
];

/// Lockfile names that carry an otherwise-allowed extension (e.g.
/// `pnpm-lock.yaml`, `package-lock.json`) or none. Machine-generated,
/// enormous, zero recall value — always dropped even if the extension
/// passes. Compared case-insensitively.
const DENY_NAMES: &[&str] = &[
    "cargo.lock",
    "pnpm-lock.yaml",
    "package-lock.json",
    "yarn.lock",
    "poetry.lock",
    "pipfile.lock",
    "uv.lock",
    "composer.lock",
    "gemfile.lock",
    "flake.lock",
    "bun.lockb",
    "deno.lock",
];

/// Decide whether a file path is worth indexing: a lockfile is always
/// rejected; otherwise the extension must be on the allowlist, or the
/// (extensionless) file name must be a known-good ops file.
#[must_use]
pub fn is_indexable(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|s| s.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    if DENY_NAMES.contains(&name.as_str()) {
        return false;
    }
    if let Some(ext) = path.extension() {
        let ext = ext.to_string_lossy().to_ascii_lowercase();
        if ALLOW_EXT.contains(&ext.as_str()) {
            return true;
        }
    }
    ALLOW_NAMES.contains(&name.as_str())
}

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
        .add_custom_ignore_filename(".klamsignore")
        // Prune always-skip directories *before* descending into them.
        // Filtering only after the walker yields entries still forces a
        // full traversal of e.g. `.venv/site-packages` (hundreds of
        // thousands of files) just to discard them — pruning here means
        // the walker never enters those subtrees at all.
        .filter_entry(|entry| {
            let name = entry.file_name().to_string_lossy();
            !ALWAYS_SKIP.contains(&name.as_ref())
        });
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
        // File-type allowlist (sprint 021, #316): drop lockfiles, JSON
        // fixtures, SVGs, images and other non-content before it ever
        // reaches the chunker/embedder.
        if !is_indexable(path) {
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
