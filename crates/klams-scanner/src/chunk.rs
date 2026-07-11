//! Split a file into retrieval-worthy chunks (sprint 022 scanner v2).
//!
//! Chunking is **language-aware** ([`Lang`], derived from the file
//! extension), which fixes the two junk-hit classes the crossroads
//! review found live:
//!
//! - **Markdown** is split on ATX headings, but a chunk always carries
//!   its heading *path* ("H1 > H2") as a breadcrumb and a bare heading
//!   never becomes its own chunk — no more `"## MCP tools"` hits.
//! - **Everything else** (code, config, plain text) is split on blank
//!   lines only; `#` is never treated as a heading, so a Python/shell/
//!   TOML comment no longer fragments the file.
//!
//! Text is normalized (newline- and indentation-preserving, sprint 022
//! #321) before splitting, and each chunk is content-hashed over the
//! post-normalization text so whitespace-only edits don't churn.
//! Sprint 022 #323 upgrades the code path to tree-sitter; until then
//! code uses the blank-line splitter below.

use sha2::{Digest, Sha256};

/// Target chunk size (post-normalization). Soft — we exceed it rather
/// than split a paragraph mid-line.
pub const TARGET_CHARS: usize = 800;

/// Overlap between adjacent slide-window chunks of an oversized block.
pub const OVERLAP_CHARS: usize = 200;

/// A packed piece below this size merges into the previous piece *of
/// the same heading path* rather than shipping as a tiny fragment
/// (sprint 022 #320). Bare headings are eliminated structurally (a
/// heading with no body never becomes a block), so this only catches
/// trailing scraps, never mislabels cross-section content.
pub const MIN_CHARS: usize = 64;

/// Source language, chosen by file extension. Drives the chunking
/// strategy and is surfaced as chunk metadata (sprint 022 #322).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Lang {
    Markdown,
    Rust,
    Python,
    Shell,
    Toml,
    Text,
}

impl Lang {
    /// Classify by file extension (case-insensitive). Unknown → `Text`.
    #[must_use]
    pub fn from_path(path: &str) -> Self {
        let ext = std::path::Path::new(path)
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "md" | "markdown" | "mdx" => Lang::Markdown,
            "rs" => Lang::Rust,
            "py" | "pyi" => Lang::Python,
            "sh" | "bash" | "zsh" => Lang::Shell,
            "toml" => Lang::Toml,
            _ => Lang::Text,
        }
    }

    /// Stable label for chunk metadata / payload, or `None` for plain
    /// text (no meaningful language to record).
    #[must_use]
    pub fn label(self) -> Option<&'static str> {
        match self {
            Lang::Markdown => Some("markdown"),
            Lang::Rust => Some("rust"),
            Lang::Python => Some("python"),
            Lang::Shell => Some("shell"),
            Lang::Toml => Some("toml"),
            Lang::Text => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub index: u32,
    pub text: String,
    pub content_hash: String,
    /// Markdown heading breadcrumb ("H1 > H2 > H3") for this chunk's
    /// section, or `None` for non-markdown / pre-heading content.
    pub heading_path: Option<String>,
    /// Source language label (`Lang::label`), or `None` for plain text.
    pub language: Option<String>,
    /// Code symbol names in this chunk (tree-sitter, sprint 022 #323);
    /// empty for prose or unparsed languages.
    pub symbols: Vec<String>,
}

/// Normalize file text before chunking, preserving line structure and
/// indentation (sprint 022 #321). Delegates to the shared
/// [`klams_types::normalize_chunk_text`] so the scanner and the API
/// ingest path agree — the API re-normalizes each received chunk, and
/// identical normalization keeps the dedupe content-hash stable.
#[must_use]
pub fn normalize(input: &str) -> String {
    klams_types::normalize_chunk_text(input)
}

#[must_use]
pub fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

/// Split `body` into chunks using the strategy for `lang`.
#[must_use]
pub fn chunk(body: &str, lang: Lang) -> Vec<Chunk> {
    let norm = normalize(body);
    if norm.is_empty() {
        return Vec::new();
    }

    // (heading_path, body_text) pieces, packed to ~TARGET_CHARS.
    let pieces: Vec<(Option<String>, String)> = match lang {
        Lang::Markdown => pack_blocks(markdown_blocks(&norm)),
        // Code/config/text: blank-line paragraphs, no heading breaks.
        // Sprint 022 #323 upgrades Rust/Python to tree-sitter here.
        _ => pack_blocks(plain_blocks(&norm)),
    };

    let language = lang.label().map(str::to_owned);
    let mut out = Vec::new();
    let mut idx: u32 = 0;
    for (path, body) in pieces {
        // Prepend the heading breadcrumb so the chunk is self-describing
        // in retrieval (and the section context is embedded), never a
        // bare heading. Path is also kept as metadata (#322).
        let text = match &path {
            Some(p) => format!("{p}\n\n{body}"),
            None => body,
        };
        let text = text.trim().to_owned();
        if text.is_empty() {
            continue;
        }
        let content_hash = sha256_hex(&text);
        out.push(Chunk {
            index: idx,
            text,
            content_hash,
            heading_path: path,
            language: language.clone(),
            symbols: Vec::new(),
        });
        idx += 1;
    }
    out
}

/// Parse an ATX markdown heading: 1–6 leading `#` followed by a space,
/// then the title. Returns `(level, title)`. Rejects `#hashtag`,
/// shebangs, and `#` code comments (no space, or >6 hashes).
fn md_heading(line: &str) -> Option<(usize, &str)> {
    let t = line.trim_start();
    let hashes = t.chars().take_while(|&c| c == '#').count();
    if hashes == 0 || hashes > 6 {
        return None;
    }
    let rest = &t[hashes..];
    if !rest.starts_with(' ') && !rest.starts_with('\t') {
        return None;
    }
    Some((hashes, rest.trim()))
}

/// Split normalized markdown into `(heading_path, body)` blocks. A
/// heading only updates the heading stack; the body that follows it
/// carries the stack as its path. A heading with no body contributes
/// context to the next body but never a block of its own.
fn markdown_blocks(norm: &str) -> Vec<(Option<String>, String)> {
    let mut blocks: Vec<(Option<String>, String)> = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut body = String::new();
    let mut body_path: Option<String> = None;

    for line in norm.lines() {
        if let Some((level, title)) = md_heading(line) {
            if body.trim().is_empty() {
                body.clear();
            } else {
                blocks.push((body_path.clone(), std::mem::take(&mut body)));
            }
            while stack.last().is_some_and(|&(l, _)| l >= level) {
                stack.pop();
            }
            stack.push((level, title.to_owned()));
            body_path = Some(
                stack
                    .iter()
                    .map(|(_, t)| t.as_str())
                    .collect::<Vec<_>>()
                    .join(" > "),
            );
        } else {
            if !body.is_empty() {
                body.push('\n');
            }
            body.push_str(line);
        }
    }
    if !body.trim().is_empty() {
        blocks.push((body_path, body));
    }
    blocks
}

/// Split normalized non-markdown text into `(None, paragraph)` blocks
/// on blank lines. No heading detection — a `#` comment stays with its
/// paragraph.
fn plain_blocks(norm: &str) -> Vec<(Option<String>, String)> {
    norm.split("\n\n")
        .map(str::trim)
        .filter(|p| !p.is_empty())
        .map(|p| (None, p.to_owned()))
        .collect()
}

/// Pack blocks into ~`TARGET_CHARS` pieces. Consecutive blocks are
/// merged only while they share a heading path (so B's and C's content
/// is never mislabelled under A's breadcrumb) and stay under
/// `TARGET_CHARS`. An oversized block is slide-windowed on its own.
fn pack_blocks(blocks: Vec<(Option<String>, String)>) -> Vec<(Option<String>, String)> {
    let mut out: Vec<(Option<String>, String)> = Vec::new();
    let mut cur = String::new();
    let mut cur_path: Option<String> = None;

    for (path, body) in blocks {
        let body = body.trim().to_owned();
        if body.is_empty() {
            continue;
        }
        if body.len() > TARGET_CHARS {
            if !cur.is_empty() {
                let p = cur_path.take();
                push_piece(&mut out, p, std::mem::take(&mut cur));
            }
            for w in slide_windows(&body) {
                out.push((path.clone(), w));
            }
            continue;
        }
        let mergeable =
            !cur.is_empty() && path == cur_path && cur.len() + body.len() + 2 <= TARGET_CHARS;
        if mergeable {
            cur.push_str("\n\n");
            cur.push_str(&body);
        } else {
            if !cur.is_empty() {
                let p = cur_path.take();
                push_piece(&mut out, p, std::mem::take(&mut cur));
            }
            cur_path = path;
            cur = body;
        }
    }
    if !cur.is_empty() {
        push_piece(&mut out, cur_path, cur);
    }
    out
}

/// Emit a packed piece, merging a sub-`MIN_CHARS` scrap into the
/// previous piece only when they share a heading path (safe — never
/// mislabels cross-section content).
fn push_piece(out: &mut Vec<(Option<String>, String)>, path: Option<String>, text: String) {
    if text.len() < MIN_CHARS {
        if let Some(last) = out.last_mut() {
            if last.0 == path {
                last.1.push_str("\n\n");
                last.1.push_str(&text);
                return;
            }
        }
    }
    out.push((path, text));
}

/// Slide a `TARGET_CHARS` window with `OVERLAP_CHARS` overlap over an
/// oversized block, on char boundaries.
fn slide_windows(s: &str) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= TARGET_CHARS {
        return vec![s.to_owned()];
    }
    let step = TARGET_CHARS - OVERLAP_CHARS;
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + TARGET_CHARS).min(chars.len());
        out.push(chars[start..end].iter().collect());
        if end == chars.len() {
            break;
        }
        start += step;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_returns_no_chunks() {
        assert!(chunk("", Lang::Markdown).is_empty());
        assert!(chunk("    \n\n   ", Lang::Text).is_empty());
    }

    #[test]
    fn lang_from_path_classifies() {
        assert_eq!(Lang::from_path("a/b/readme.md"), Lang::Markdown);
        assert_eq!(Lang::from_path("src/lib.rs"), Lang::Rust);
        assert_eq!(Lang::from_path("x.py"), Lang::Python);
        assert_eq!(Lang::from_path("deploy.sh"), Lang::Shell);
        assert_eq!(Lang::from_path("Cargo.TOML"), Lang::Toml);
        assert_eq!(Lang::from_path("data.bin"), Lang::Text);
    }

    #[test]
    fn markdown_heading_becomes_breadcrumb_not_a_bare_chunk() {
        let body = "# klams\n\n## MCP tools\n\nThe server exposes memory_search, memory_add, and friends over the streamable HTTP transport at kubs0:7777.";
        let chunks = chunk(body, Lang::Markdown);
        assert_eq!(chunks.len(), 1, "got {chunks:?}");
        let c = &chunks[0];
        assert_eq!(c.heading_path.as_deref(), Some("klams > MCP tools"));
        assert!(c.text.starts_with("klams > MCP tools"));
        assert!(c.text.contains("memory_search"));
        // The bare heading text is never a standalone chunk.
        assert!(!chunks.iter().any(|c| c.text.trim() == "## MCP tools"));
    }

    #[test]
    fn bare_heading_with_no_body_yields_no_chunk() {
        // A heading immediately followed by another heading (or EOF)
        // contributes context but never its own chunk.
        let body = "## MCP tools\n\n# PHASE 8 — Restore Data";
        let chunks = chunk(body, Lang::Markdown);
        assert!(
            chunks.is_empty(),
            "bare headings must not become chunks: {chunks:?}"
        );
    }

    #[test]
    fn code_hash_comment_is_not_a_heading() {
        // Sprint 022 (#320): `#` comments in Python/shell/TOML must not
        // be treated as markdown headings and fragment the file.
        let body = "# module docstring comment\nimport os\n\n\ndef go():\n    return os.getcwd()";
        let chunks = chunk(body, Lang::Python);
        assert_eq!(chunks.len(), 1, "python must not split on # comment");
        assert_eq!(chunks[0].language.as_deref(), Some("python"));
        assert!(chunks[0].heading_path.is_none());
        assert!(chunks[0].text.contains("def go():"));
        // indentation preserved (#321)
        assert!(chunks[0].text.contains("\n    return os.getcwd()"));
    }

    #[test]
    fn heading_nesting_pops_stack() {
        let body = "# A\n\nalpha body long enough to matter here padding padding\n\n## B\n\nbeta body also long enough padding padding padding\n\n# C\n\ngamma body enough padding padding padding padding";
        let chunks = chunk(body, Lang::Markdown);
        let paths: Vec<_> = chunks
            .iter()
            .filter_map(|c| c.heading_path.as_deref())
            .collect();
        assert!(paths.contains(&"A"));
        assert!(paths.contains(&"A > B"), "got {paths:?}");
        assert!(paths.contains(&"C"), "C must pop B and A: {paths:?}");
    }

    #[test]
    fn large_section_slides_with_overlap() {
        let para = "lorem ipsum ".repeat(120); // ~1440 chars
        let chunks = chunk(&para, Lang::Text);
        assert!(chunks.len() >= 2, "expected slide, got {}", chunks.len());
        let head: String = chunks[0].text.chars().rev().take(40).collect();
        let tail_fwd: String = head.chars().rev().collect();
        assert!(chunks[1].text.contains(&tail_fwd[..tail_fwd.len().min(30)]));
    }

    #[test]
    fn md_heading_parses_and_rejects() {
        assert_eq!(md_heading("# Title"), Some((1, "Title")));
        assert_eq!(md_heading("###  Sub  "), Some((3, "Sub")));
        assert_eq!(md_heading("#hashtag"), None);
        assert_eq!(md_heading("#!/bin/bash"), None);
        assert_eq!(md_heading("not a heading"), None);
        assert_eq!(md_heading("####### too many"), None);
    }

    #[test]
    fn normalize_preserves_structure_and_is_idempotent() {
        let raw = "  hello   world\n\nfoo\t bar  \n";
        let once = normalize(raw);
        assert_eq!(once, "  hello   world\n\nfoo\t bar");
        assert_eq!(normalize(&once), once);
    }

    #[test]
    fn sha256_stable_across_leading_trailing_blank_lines() {
        let a = chunk("hello world\n\n\n", Lang::Text);
        let b = chunk("\n\nhello world", Lang::Text);
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].content_hash, b[0].content_hash);
    }
}
