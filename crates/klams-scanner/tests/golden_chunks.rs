//! Sprint 022 (#326) — golden chunker tests over realistic, multi-line
//! fixtures for each language, pinning the scanner-v2 behaviors so the
//! crossroads junk-hit classes can't come back.

use klams_scanner::chunk::{chunk, Lang};

const MARKDOWN: &str = "\
# klams

A homelab memory store.

## MCP tools

The server exposes `memory_search`, `memory_add`, and `dissent_propose`
over the streamable HTTP transport at `kubs0:7777`. Each tool is
scope-gated by the caller's bearer token.

### memory_search

Federates Postgres FTS over facts/events and Qdrant ANN over knowledge
into one ranked list. Returns ScoredMemory envelopes with per-source
rank so evals can see why a hit ranked where it did.

## Restore

# PHASE 8 — Restore Data

Restore proceeds by streaming the latest committed backup into a scratch
database, verifying table counts, then promoting it. The runbook lives
in docs/usage.md and is exercised once at setup.
";

const RUST: &str = "\
//! A small module with a couple of items.

use std::collections::HashMap;

/// Adds two numbers.
fn add(a: u32, b: u32) -> u32 {
    a + b
}

/// A little container.
struct Bag {
    items: HashMap<String, u32>,
}

impl Bag {
    fn insert(&mut self, k: String, v: u32) {
        self.items.insert(k, v);
    }
}
";

const PYTHON: &str = "\
# top-level comment that must NOT be treated as a heading
import os


def cwd() -> str:
    # inner comment, indentation must survive
    return os.getcwd()


class Store:
    def __init__(self):
        self.data = {}
";

const SHELL: &str = "\
#!/usr/bin/env bash
# deploy helper — the leading '#' lines are comments, not headings
set -euo pipefail

main() {
    echo \"deploying\"
    systemctl restart klams-service
}

main \"$@\"
";

const TOML: &str = "\
# klams service config
[server]
bind = \"127.0.0.1:7777\"

# auth tokens are hot-reloadable since sprint 018
[[auth.tokens]]
token = \"redacted\"
scopes = [\"read\", \"write\"]
";

#[test]
fn markdown_junk_headings_become_substantive_chunks_with_paths() {
    let chunks = chunk(MARKDOWN, Lang::Markdown);
    // None of the crossroads bare-heading hits appear as a chunk that is
    // just the heading text.
    for junk in ["## MCP tools", "# PHASE 8 — Restore Data", "## Restore"] {
        assert!(
            !chunks.iter().any(|c| c.text.trim() == junk),
            "bare heading `{junk}` leaked as a chunk: {:#?}",
            chunks.iter().map(|c| &c.text).collect::<Vec<_>>()
        );
    }
    // Real content carries its heading path and body.
    let mcp = chunks
        .iter()
        .find(|c| c.text.contains("streamable HTTP transport"))
        .expect("MCP tools body chunk");
    assert_eq!(mcp.heading_path.as_deref(), Some("klams > MCP tools"));
    assert!(mcp.text.starts_with("klams > MCP tools"));

    let search = chunks
        .iter()
        .find(|c| c.text.contains("Federates Postgres FTS"))
        .expect("memory_search body chunk");
    assert_eq!(
        search.heading_path.as_deref(),
        Some("klams > MCP tools > memory_search")
    );

    // "PHASE 8 — Restore Data" is a real section with a body, so it is a
    // substantive chunk (path present, body included), never bare.
    let restore = chunks
        .iter()
        .find(|c| c.text.contains("streaming the latest committed backup"))
        .expect("restore body chunk");
    assert!(restore
        .heading_path
        .as_deref()
        .unwrap()
        .contains("PHASE 8 — Restore Data"));

    // Every markdown chunk is labelled, and no chunk is just a heading
    // line (a real short section carries a heading path + body).
    for c in &chunks {
        assert_eq!(c.language.as_deref(), Some("markdown"));
        assert!(
            c.text.lines().count() >= 2 || c.heading_path.is_none(),
            "chunk is a lone heading line: {:?}",
            c.text
        );
    }
}

#[test]
fn rust_chunks_by_item_with_symbols_and_indentation() {
    let chunks = chunk(RUST, Lang::Rust);
    let syms: Vec<String> = chunks.iter().flat_map(|c| c.symbols.clone()).collect();
    assert!(syms.contains(&"add".to_string()), "syms={syms:?}");
    assert!(syms.contains(&"Bag".to_string()), "syms={syms:?}");
    assert!(chunks.iter().all(|c| c.language.as_deref() == Some("rust")));
    // Indentation preserved (#321).
    assert!(chunks.iter().any(|c| c.text.contains("\n    a + b")));
    // No heading paths on code.
    assert!(chunks.iter().all(|c| c.heading_path.is_none()));
}

#[test]
fn python_hash_comments_do_not_fragment() {
    let chunks = chunk(PYTHON, Lang::Python);
    // The `#` comment lines must not have split the file heading-style;
    // a small file stays a single coherent chunk.
    assert_eq!(chunks.len(), 1, "got {chunks:#?}");
    let c = &chunks[0];
    assert!(c.text.contains("def cwd()"));
    assert!(c.text.contains("class Store"));
    assert!(c.text.contains("# top-level comment"));
    assert!(c.text.contains("\n    return os.getcwd()")); // indent kept
    let syms = &c.symbols;
    assert!(syms.contains(&"cwd".to_string()) && syms.contains(&"Store".to_string()));
}

#[test]
fn shell_and_toml_hash_lines_are_not_headings() {
    for (src, lang, label) in [(SHELL, Lang::Shell, "shell"), (TOML, Lang::Toml, "toml")] {
        let chunks = chunk(src, lang);
        assert!(!chunks.is_empty());
        // No chunk is a lone `#`-comment line masquerading as a heading.
        for c in &chunks {
            assert!(c.heading_path.is_none(), "{label} got a heading path");
            assert!(
                !c.text.trim_start().starts_with("# ") || c.text.lines().count() > 1,
                "{label} chunk looks like a bare comment-heading: {:?}",
                c.text
            );
            assert_eq!(c.language.as_deref(), Some(label));
        }
    }
}

#[test]
fn chunk_indices_are_contiguous_from_zero() {
    let chunks = chunk(MARKDOWN, Lang::Markdown);
    for (i, c) in chunks.iter().enumerate() {
        assert_eq!(
            c.index as usize, i,
            "chunk index must be 0-based contiguous"
        );
    }
}
