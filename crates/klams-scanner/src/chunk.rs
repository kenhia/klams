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
///
/// `limit` is the embedder's input ceiling (sprint 027 #420). It does not
/// drive normal chunking — [`TARGET_CHARS`] still does, because ~800
/// characters is the size that retrieves well — it is the guarantee that
/// no chunk the scanner publishes can be refused by the embedder.
#[must_use]
pub fn chunk(body: &str, lang: Lang, limit: klams_types::EmbedLimit) -> Vec<Chunk> {
    let norm = normalize(body);
    if norm.is_empty() {
        return Vec::new();
    }

    // Each piece: (heading_path, symbols, body_text).
    let pieces: Vec<(Option<String>, Vec<String>, String)> = match lang {
        Lang::Markdown => pack_blocks(markdown_blocks(&norm))
            .into_iter()
            .map(|(p, b)| (p, Vec::new(), b))
            .collect(),
        // Code-aware chunking (sprint 022 #323); falls back to the plain
        // blank-line splitter if tree-sitter can't parse or has no items.
        Lang::Rust | Lang::Python => crate::code::code_blocks(&norm, lang).map_or_else(
            || {
                pack_blocks(plain_blocks(&norm))
                    .into_iter()
                    .map(|(_, b)| (None, Vec::new(), b))
                    .collect()
            },
            |cbs| cbs.into_iter().map(|(syms, b)| (None, syms, b)).collect(),
        ),
        // Shell/TOML/text: blank-line paragraphs, no heading breaks.
        _ => pack_blocks(plain_blocks(&norm))
            .into_iter()
            .map(|(_, b)| (None, Vec::new(), b))
            .collect(),
    };

    let language = lang.label().map(str::to_owned);
    let mut out = Vec::new();
    let mut idx: u32 = 0;
    for (path, symbols, body) in pieces {
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
        // Sprint 027 (#420): the final gate, applied to the text that
        // will actually be embedded.
        //
        // Three things upstream can push a piece past the model's token
        // budget even though `TARGET_CHARS` is only 800: the heading
        // breadcrumb is prepended *after* packing; the tree-sitter code
        // path bypasses `pack_blocks` entirely and emits whole functions;
        // and 800 characters of dense content (CJK, minified code, wide
        // tables) is far more than 800 characters of prose is in tokens.
        //
        // A chunk that exceeds the ceiling used to be published anyway,
        // accepted with 202, and then dropped at the worker when the
        // embed failed. Splitting here means the scanner never offers one.
        for piece in enforce_limit(&text, limit) {
            let content_hash = sha256_hex(&piece);
            out.push(Chunk {
                index: idx,
                text: piece,
                content_hash,
                heading_path: path.clone(),
                language: language.clone(),
                symbols: symbols.clone(),
            });
            idx += 1;
        }
    }
    out
}

/// Split `text` until every piece fits the embedder's ceiling.
///
/// Returns `text` untouched in the overwhelming majority of cases — an
/// 800-character prose chunk is nowhere near a 512-token budget. The
/// splitting path exists for the dense content that provoked #420.
fn enforce_limit(text: &str, limit: klams_types::EmbedLimit) -> Vec<String> {
    if limit.fits(text) {
        return vec![text.to_owned()];
    }
    // Halve the window until the densest piece fits. Character-count
    // windows cannot express a token budget directly (that is the whole
    // problem), so converge on one instead of guessing a ratio.
    let mut window = limit.max_chars();
    loop {
        let pieces = split_on_char_window(text, window);
        if pieces.iter().all(|p| limit.fits(p)) {
            return pieces;
        }
        window /= 2;
        if window < MIN_CHARS {
            // Pathological input (e.g. one enormous CJK run). Emit the
            // per-character floor rather than looping forever; the gate
            // downstream is still authoritative.
            return split_on_char_window(text, MIN_CHARS);
        }
    }
}

/// Split into `window`-character pieces with the usual overlap, on char
/// boundaries so multi-byte content is never cut mid-character.
fn split_on_char_window(s: &str, window: usize) -> Vec<String> {
    let chars: Vec<char> = s.chars().collect();
    if chars.len() <= window {
        return vec![s.to_owned()];
    }
    let overlap = OVERLAP_CHARS.min(window / 4);
    let step = (window - overlap).max(1);
    let mut out = Vec::new();
    let mut start = 0;
    while start < chars.len() {
        let end = (start + window).min(chars.len());
        out.push(chars[start..end].iter().collect());
        if end == chars.len() {
            break;
        }
        start += step;
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
        assert!(chunk("", Lang::Markdown, klams_types::EmbedLimit::default()).is_empty());
        assert!(chunk(
            "    \n\n   ",
            Lang::Text,
            klams_types::EmbedLimit::default()
        )
        .is_empty());
    }

    // -----------------------------------------------------------------
    // Sprint 027 (#420) — no chunk the scanner emits may exceed the
    // embedder's ceiling. Before this, an over-budget chunk was published
    // anyway, accepted with 202 (advancing the cursor), and then dropped
    // at the worker when the embed failed: silent, unretried data loss.

    #[test]
    fn every_emitted_chunk_fits_the_embedder_ceiling() {
        let limit = klams_types::EmbedLimit::default();
        // Dense CJK: ~1 token per character, so a piece well under
        // TARGET_CHARS in *characters* is far over budget in *tokens*.
        // This is exactly the "well under 8192 chars but over 512 tokens"
        // shape #420 describes.
        let body = "日本語のテキストが延々と続く段落です。".repeat(200);
        let chunks = chunk(&body, Lang::Text, limit);

        assert!(!chunks.is_empty(), "dense input must still produce chunks");
        for c in &chunks {
            assert!(
                limit.fits(&c.text),
                "chunk {} is over budget ({} chars, ~{} tokens)",
                c.index,
                c.text.chars().count(),
                klams_types::estimate_tokens(&c.text),
            );
        }
    }

    #[test]
    fn long_markdown_with_breadcrumbs_stays_within_budget() {
        // The breadcrumb is prepended *after* packing, so a piece that
        // fit before can stop fitting. Deep heading paths make that worst.
        let limit = klams_types::EmbedLimit::default();
        let heading = "# Very Long Top Level Heading\n\n## And A Nested One\n\n";
        let body = format!("{heading}{}", "prose about the homelab. ".repeat(400));
        for c in chunk(&body, Lang::Markdown, limit) {
            assert!(limit.fits(&c.text), "chunk {} over budget", c.index);
        }
    }

    #[test]
    fn oversized_code_blocks_are_split_rather_than_published_whole() {
        // The tree-sitter code path bypasses pack_blocks entirely and can
        // emit a whole function, so the final gate is what protects it.
        let limit = klams_types::EmbedLimit::default();
        let huge_fn = format!(
            "fn enormous() {{\n{}\n}}",
            "    let x = compute_something_with_a_long_name(alpha, beta);\n".repeat(60)
        );
        let chunks = chunk(&huge_fn, Lang::Rust, limit);
        assert!(!chunks.is_empty());
        for c in &chunks {
            assert!(limit.fits(&c.text), "chunk {} over budget", c.index);
        }
    }

    #[test]
    fn chunk_indices_stay_contiguous_after_a_split() {
        // Splitting happens inside the emit loop, so the index counter
        // has to keep running rather than repeat or skip.
        let limit = klams_types::EmbedLimit::default();
        let body = "文字".repeat(3000);
        let chunks = chunk(&body, Lang::Text, limit);
        assert!(chunks.len() > 1, "fixture should force a split");
        for (i, c) in chunks.iter().enumerate() {
            assert_eq!(c.index as usize, i, "indices must be contiguous");
        }
    }

    #[test]
    fn ordinary_prose_is_untouched_by_the_gate() {
        // The gate must be invisible in the common case — an 800-char
        // prose chunk is nowhere near 512 tokens, and re-splitting it
        // would wreck retrieval quality.
        let limit = klams_types::EmbedLimit::default();
        let body = "The klams service runs on kubs0 behind tailscale.\n\n\
                    Retrieval fuses Qdrant and Postgres results with RRF.";
        let with_gate = chunk(body, Lang::Text, limit);
        let with_huge_gate = chunk(body, Lang::Text, klams_types::EmbedLimit::new(100_000));
        assert_eq!(with_gate, with_huge_gate);
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
        let chunks = chunk(body, Lang::Markdown, klams_types::EmbedLimit::default());
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
        let chunks = chunk(body, Lang::Markdown, klams_types::EmbedLimit::default());
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
        let chunks = chunk(body, Lang::Python, klams_types::EmbedLimit::default());
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
        let chunks = chunk(body, Lang::Markdown, klams_types::EmbedLimit::default());
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
        let chunks = chunk(&para, Lang::Text, klams_types::EmbedLimit::default());
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
        let a = chunk(
            "hello world\n\n\n",
            Lang::Text,
            klams_types::EmbedLimit::default(),
        );
        let b = chunk(
            "\n\nhello world",
            Lang::Text,
            klams_types::EmbedLimit::default(),
        );
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].content_hash, b[0].content_hash);
    }
}
