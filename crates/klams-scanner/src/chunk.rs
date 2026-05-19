//! Chunk text from a file into ~800-char paragraphs with 200-char
//! overlap, treating Markdown headings as hard breaks. Each chunk
//! is content-hashed (sha256) over the post-normalization text so
//! whitespace-only edits don't churn the cursor.

use sha2::{Digest, Sha256};

/// Target chunk size (post-normalization). Soft target — we'll exceed
/// it to avoid splitting a paragraph or a heading mid-line.
pub const TARGET_CHARS: usize = 800;

/// Overlap between adjacent chunks. Keeps semantic continuity across
/// the boundary for the embedder.
pub const OVERLAP_CHARS: usize = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub index: u32,
    pub text: String,
    pub content_hash: String,
}

/// Collapse runs of internal whitespace and trim each line.
/// Idempotent: `normalize(normalize(x)) == normalize(x)`.
#[must_use]
pub fn normalize(input: &str) -> String {
    input
        .lines()
        .map(|l| {
            let mut out = String::with_capacity(l.len());
            let mut last_space = false;
            for c in l.chars() {
                if c.is_whitespace() {
                    if !last_space {
                        out.push(' ');
                    }
                    last_space = true;
                } else {
                    out.push(c);
                    last_space = false;
                }
            }
            out.trim().to_owned()
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[must_use]
pub fn sha256_hex(s: &str) -> String {
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    format!("{:x}", h.finalize())
}

/// Split `body` into chunks. Hard-breaks on `#`/`##`/etc. markdown
/// headings (line starts with one or more `#` followed by space).
/// Other boundaries: blank lines (paragraph breaks). Within a span
/// that exceeds `TARGET_CHARS`, falls back to a sliding window of
/// `TARGET_CHARS` with `OVERLAP_CHARS` overlap so no chunk grows
/// unboundedly.
#[must_use]
pub fn chunk(body: &str) -> Vec<Chunk> {
    let normalized = normalize(body);
    if normalized.is_empty() {
        return Vec::new();
    }

    // Split into "sections" on heading lines. Each section is then
    // split into paragraphs on blank lines.
    let mut sections: Vec<String> = Vec::new();
    let mut buf = String::new();
    for line in normalized.lines() {
        if is_heading(line) && !buf.is_empty() {
            sections.push(std::mem::take(&mut buf));
        }
        if !buf.is_empty() {
            buf.push('\n');
        }
        buf.push_str(line);
    }
    if !buf.is_empty() {
        sections.push(buf);
    }

    let mut out: Vec<Chunk> = Vec::new();
    let mut idx: u32 = 0;
    for section in sections {
        // Build chunks within this section by aggregating paragraphs
        // until we cross TARGET_CHARS, then emit + slide.
        let paragraphs: Vec<&str> = section.split("\n\n").filter(|p| !p.is_empty()).collect();
        let mut current = String::new();
        for p in paragraphs {
            if current.len() + p.len() + 2 > TARGET_CHARS && !current.is_empty() {
                push_chunk(&mut out, &mut idx, &current);
                // Carry overlap.
                current = tail(&current, OVERLAP_CHARS);
            }
            if !current.is_empty() {
                current.push_str("\n\n");
            }
            current.push_str(p);
            // If a single paragraph is bigger than TARGET, slide-window it.
            while current.len() > TARGET_CHARS * 2 {
                let head: String = current.chars().take(TARGET_CHARS).collect();
                push_chunk(&mut out, &mut idx, &head);
                let drop_n: usize = TARGET_CHARS - OVERLAP_CHARS;
                current = current.chars().skip(drop_n).collect();
            }
        }
        if !current.is_empty() {
            push_chunk(&mut out, &mut idx, &current);
        }
    }
    out
}

fn push_chunk(out: &mut Vec<Chunk>, idx: &mut u32, text: &str) {
    let t = text.trim().to_owned();
    if t.is_empty() {
        return;
    }
    let hash = sha256_hex(&t);
    out.push(Chunk {
        index: *idx,
        text: t,
        content_hash: hash,
    });
    *idx += 1;
}

fn tail(s: &str, n: usize) -> String {
    let chars: Vec<char> = s.chars().collect();
    let start = chars.len().saturating_sub(n);
    chars[start..].iter().collect()
}

fn is_heading(line: &str) -> bool {
    let trimmed = line.trim_start();
    let mut chars = trimmed.chars();
    let mut hashes = 0;
    for c in chars.by_ref() {
        if c == '#' {
            hashes += 1;
            if hashes > 6 {
                return false;
            }
        } else {
            return hashes > 0 && (c == ' ' || c == '\t');
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_collapses_whitespace_and_is_idempotent() {
        let raw = "  hello   world\n\nfoo\t bar  \n";
        let once = normalize(raw);
        assert_eq!(once, "hello world\n\nfoo bar");
        assert_eq!(normalize(&once), once);
    }

    #[test]
    fn sha256_stable_across_whitespace_normalization() {
        let a = chunk("hello   world\n\n");
        let b = chunk("  hello world  ");
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].content_hash, b[0].content_hash);
    }

    #[test]
    fn empty_input_returns_no_chunks() {
        assert!(chunk("").is_empty());
        assert!(chunk("    \n\n   ").is_empty());
    }

    #[test]
    fn markdown_heading_is_hard_break() {
        let body = "intro paragraph\n\n# Heading\n\nbody of section";
        let chunks = chunk(body);
        assert_eq!(chunks.len(), 2, "got {chunks:?}");
        assert!(chunks[0].text.contains("intro paragraph"));
        assert!(chunks[1].text.starts_with("# Heading"));
    }

    #[test]
    fn paragraph_under_target_collapses_to_one_chunk() {
        let body = "para one\n\npara two\n\npara three";
        let chunks = chunk(body);
        assert_eq!(chunks.len(), 1);
    }

    #[test]
    fn large_section_splits_with_overlap() {
        let para = "lorem ipsum ".repeat(80); // ~960 chars
        let body = format!("{para}\n\n{para}\n\n{para}");
        let chunks = chunk(&body);
        assert!(
            chunks.len() >= 2,
            "expected >=2 chunks, got {}",
            chunks.len()
        );
        // Overlap: tail of chunk[0] should appear at head of chunk[1].
        let tail_n = OVERLAP_CHARS / 2;
        let t: String = chunks[0].text.chars().rev().take(tail_n).collect();
        let tail_fwd: String = t.chars().rev().collect();
        assert!(
            chunks[1].text.contains(&tail_fwd[..tail_fwd.len().min(40)]),
            "no overlap between chunks"
        );
    }

    #[test]
    fn heading_detection_excludes_hashtags() {
        assert!(is_heading("# Title"));
        assert!(is_heading("###  Sub"));
        assert!(!is_heading("#hashtag"));
        assert!(!is_heading("not a heading"));
    }
}
