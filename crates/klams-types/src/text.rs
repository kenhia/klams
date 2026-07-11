//! Shared text normalization for the knowledge chunk pipeline.
//!
//! Both the scanner (before it splits a file into chunks) and the API
//! ingest path call [`normalize_chunk_text`], so the round trip is
//! idempotent and the content-hash used for dedupe is stable across
//! re-scans. Before sprint 022 the two crates normalized differently —
//! the scanner trimmed every line (killing indentation) and the API
//! then collapsed newlines to spaces, so stored chunks were one long
//! line. This preserves line structure and indentation, which code,
//! YAML, and structured prose depend on.

use unicode_normalization::UnicodeNormalization;

/// Normalize chunk text for storage and content-hashing, preserving
/// line structure and indentation (sprint 022 #321).
///
/// - NFC-normalize; convert CRLF / CR line endings to LF.
/// - Right-trim each line (drop trailing whitespace) but KEEP leading
///   indentation.
/// - Collapse runs of blank lines to a single blank line.
/// - Trim leading and trailing blank lines.
///
/// Idempotent: `normalize_chunk_text(normalize_chunk_text(x)) ==
/// normalize_chunk_text(x)`.
#[must_use]
pub fn normalize_chunk_text(input: &str) -> String {
    let nfc: String = input.nfc().collect();
    let mut out: Vec<&str> = Vec::new();
    let mut prev_blank = true; // seeded true so leading blank lines drop
    for raw in nfc.split('\n') {
        let line = raw.strip_suffix('\r').unwrap_or(raw).trim_end();
        let blank = line.is_empty();
        if blank && prev_blank {
            continue; // collapse blank-line runs; drop leading blanks
        }
        out.push(line);
        prev_blank = blank;
    }
    while out.last().is_some_and(|l| l.is_empty()) {
        out.pop(); // drop trailing blank line(s)
    }
    out.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn preserves_indentation_and_newlines() {
        let src = "fn main() {\n    let x = 1;\n    dbg!(x);\n}";
        assert_eq!(normalize_chunk_text(src), src);
    }

    #[test]
    fn rtrims_each_line_but_keeps_leading_indent() {
        assert_eq!(
            normalize_chunk_text("  indented   \n\ttabbed  "),
            "  indented\n\ttabbed"
        );
    }

    #[test]
    fn collapses_blank_runs_and_trims_edges() {
        assert_eq!(
            normalize_chunk_text("\n\n# Title\n\n\n\nbody\n\n\n"),
            "# Title\n\nbody"
        );
    }

    #[test]
    fn crlf_becomes_lf() {
        assert_eq!(normalize_chunk_text("a\r\nb\r\n"), "a\nb");
    }

    #[test]
    fn is_idempotent() {
        let src = "  a  \r\n\n\n   b\nc   ";
        let once = normalize_chunk_text(src);
        assert_eq!(normalize_chunk_text(&once), once);
    }

    #[test]
    fn all_whitespace_is_empty() {
        assert_eq!(normalize_chunk_text("   \n\n\t\n  "), "");
    }
}
