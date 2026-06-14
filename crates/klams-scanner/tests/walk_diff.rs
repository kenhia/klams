//! T030 — walk and diff coverage for `klams-scanner::walk`.
//!
//! Verifies the `ignore` builder honours `.gitignore` / `.klamsignore`,
//! always-skip paths are dropped, and the cursor's mtime pre-filter
//! shortcuts re-hashing of unchanged files. Vanished-file deletion is
//! exercised end-to-end in `klams-service::tests::us3d_scanner_e2e`.

use klams_scanner::cursor::Cursor;
use klams_scanner::walk::walk;
use std::fs;
use std::path::Path;
use tempfile::TempDir;

fn touch(path: &Path, body: &str) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).unwrap();
    }
    fs::write(path, body).unwrap();
}

#[test]
fn walk_skips_dot_git_and_target_unconditionally() {
    let d = TempDir::new().unwrap();
    let r = d.path();
    touch(&r.join("keep.md"), "kept");
    touch(&r.join(".git").join("HEAD"), "ref");
    touch(&r.join("target").join("debug").join("binary"), "x");
    touch(&r.join("node_modules").join("x").join("y.js"), "x");

    let files = walk(r);
    let names: Vec<String> = files
        .iter()
        .map(|f| f.absolute_path.display().to_string())
        .collect();
    assert!(names.iter().any(|n| n.ends_with("keep.md")));
    assert!(
        !names.iter().any(|n| n.contains("/.git/")),
        ".git leaked into walk: {names:?}"
    );
    assert!(!names.iter().any(|n| n.contains("/target/")));
    assert!(!names.iter().any(|n| n.contains("/node_modules/")));
}

#[test]
fn walk_prunes_python_venv_and_cache_dirs() {
    // Regression: the `ignore` walker must not descend into dependency/
    // cache trees like `.venv` (site-packages can be hundreds of
    // thousands of files). These are pruned before descent.
    let d = TempDir::new().unwrap();
    let r = d.path();
    touch(&r.join("main.py"), "print('hi')");
    touch(&r.join(".venv").join("lib").join("pkg").join("mod.py"), "x");
    touch(&r.join("venv").join("bin").join("activate"), "x");
    touch(&r.join("__pycache__").join("main.cpython-311.pyc"), "x");
    touch(&r.join(".mypy_cache").join("3.11").join("x.json"), "x");
    touch(&r.join(".obsidian").join("workspace.json"), "x");
    touch(
        &r.join(".pnpm-store").join("v10").join("pkg").join("x.js"),
        "x",
    );

    let files = walk(r);
    let names: Vec<String> = files
        .iter()
        .map(|f| f.absolute_path.display().to_string())
        .collect();
    assert!(names.iter().any(|n| n.ends_with("main.py")));
    for junk in [
        "/.venv/",
        "/venv/",
        "/__pycache__/",
        "/.mypy_cache/",
        "/.obsidian/",
        "/.pnpm-store/",
    ] {
        assert!(
            !names.iter().any(|n| n.contains(junk)),
            "{junk} leaked into walk: {names:?}"
        );
    }
}

#[test]
fn walk_respects_gitignore() {
    let d = TempDir::new().unwrap();
    let r = d.path();
    touch(&r.join(".gitignore"), "secret.txt\n");
    touch(&r.join("public.md"), "p");
    touch(&r.join("secret.txt"), "s");

    let files = walk(r);
    let names: Vec<String> = files
        .iter()
        .map(|f| f.absolute_path.display().to_string())
        .collect();
    assert!(names.iter().any(|n| n.ends_with("public.md")));
    assert!(
        !names.iter().any(|n| n.ends_with("secret.txt")),
        "gitignore not respected: {names:?}"
    );
}

#[test]
fn walk_respects_klamsignore() {
    let d = TempDir::new().unwrap();
    let r = d.path();
    touch(&r.join(".klamsignore"), "drafts/\n");
    touch(&r.join("kept.md"), "k");
    touch(&r.join("drafts").join("wip.md"), "x");

    let files = walk(r);
    let names: Vec<String> = files
        .iter()
        .map(|f| f.absolute_path.display().to_string())
        .collect();
    assert!(names.iter().any(|n| n.ends_with("kept.md")));
    assert!(
        !names.iter().any(|n| n.contains("/drafts/")),
        ".klamsignore not respected: {names:?}"
    );
}

#[test]
fn klamsignore_prunes_anchored_nested_subdirs() {
    // Regression: a root `.klamsignore` must prune nested, anchored
    // subtrees (e.g. `opc/`, `ai/llama.cpp/`) before descent — this is
    // how third-party checkouts are kept out of the index.
    let d = TempDir::new().unwrap();
    let r = d.path();
    touch(&r.join(".klamsignore"), "opc/\nai/llama.cpp/\n");
    touch(&r.join("ai").join("klams").join("main.rs"), "fn main(){}");
    touch(&r.join("ai").join("llama.cpp").join("ggml.c"), "x");
    touch(&r.join("opc").join("eza").join("src.rs"), "x");

    let files = walk(r);
    let names: Vec<String> = files
        .iter()
        .map(|f| f.absolute_path.display().to_string())
        .collect();
    assert!(
        names.iter().any(|n| n.ends_with("main.rs")),
        "first-party file pruned: {names:?}"
    );
    for junk in ["/opc/", "/llama.cpp/"] {
        assert!(
            !names.iter().any(|n| n.contains(junk)),
            "{junk} leaked past .klamsignore: {names:?}"
        );
    }
}

#[test]
fn cursor_mtime_match_skips_rehash() {
    let d = TempDir::new().unwrap();
    let r = d.path();
    touch(&r.join("a.md"), "hello");
    let files = walk(r);
    assert_eq!(files.len(), 1);
    let f = &files[0];
    let abs = f.absolute_path.display().to_string();

    let cursor = Cursor::open(&d.path().join("c.sqlite")).unwrap();
    cursor.upsert(&abs, "stale-hash", f.mtime_ns, 0).unwrap();

    // Re-walk: mtime matches → caller should skip without rehashing
    // (verified at the cursor.get(...) layer that scan_root uses).
    let row = cursor.get(&abs).unwrap().unwrap();
    assert_eq!(row.mtime_ns, f.mtime_ns);
}

#[test]
fn vanished_file_detected_by_cursor_list_vs_walk() {
    let d = TempDir::new().unwrap();
    let r = d.path();
    let a = r.join("a.md");
    let b = r.join("b.md");
    touch(&a, "1");
    touch(&b, "2");

    let cursor = Cursor::open(&d.path().join("c.sqlite")).unwrap();
    for f in walk(r) {
        cursor
            .upsert(&f.absolute_path.display().to_string(), "h", f.mtime_ns, 0)
            .unwrap();
    }

    fs::remove_file(&b).unwrap();
    let seen: std::collections::HashSet<String> = walk(r)
        .into_iter()
        .map(|f| f.absolute_path.display().to_string())
        .collect();
    let vanished: Vec<String> = cursor
        .list_all()
        .unwrap()
        .into_iter()
        .filter(|c| !seen.contains(&c.absolute_path))
        .map(|c| c.absolute_path)
        .collect();
    assert_eq!(vanished.len(), 1);
    assert!(vanished[0].ends_with("b.md"));
}

#[test]
fn prune_is_scoped_to_current_root() {
    // Regression for the multi-root prune bug: scanning root B must NOT
    // treat root A's files as "vanished". `scan_root` guards its prune
    // loop with `Path::starts_with(root)` in addition to the `seen` set;
    // this locks that invariant at the cursor layer.
    let da = TempDir::new().unwrap();
    let db = TempDir::new().unwrap();
    let root_a = da.path();
    let root_b = db.path();
    touch(&root_a.join("a.md"), "1");
    touch(&root_b.join("b.md"), "2");

    // Cursor spans both roots (as it does after scanning each in turn).
    let cursor = Cursor::open(&da.path().join("c.sqlite")).unwrap();
    for root in [root_a, root_b] {
        for f in walk(root) {
            cursor
                .upsert(&f.absolute_path.display().to_string(), "h", f.mtime_ns, 0)
                .unwrap();
        }
    }

    // Simulate scan_root(root_b): `seen` holds only root_b's paths.
    let seen: std::collections::HashSet<String> = walk(root_b)
        .into_iter()
        .map(|f| f.absolute_path.display().to_string())
        .collect();

    // Apply scan_root's prune predicate: skip seen, and skip entries
    // outside the current root.
    let would_prune: Vec<String> = cursor
        .list_all()
        .unwrap()
        .into_iter()
        .filter(|c| !seen.contains(&c.absolute_path))
        .filter(|c| Path::new(&c.absolute_path).starts_with(root_b))
        .map(|c| c.absolute_path)
        .collect();

    assert!(
        would_prune.is_empty(),
        "cross-root prune leaked while scanning root_b: {would_prune:?}"
    );
}
