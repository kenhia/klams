//! The shared front half of a knowledge write: normalize, bound, hash.
//!
//! Sprint 031 (#645). Both surfaces accept knowledge, and until 031
//! only one of them enforced anything. REST normalized the text,
//! rejected it if normalization emptied it, capped tag count and tag
//! length, hashed the *normalized* form, and probed that hash for an
//! existing point. MCP did none of it: it hashed the raw text, accepted
//! any tags at all, and always inserted — so identical content written
//! by an agent became a second point, while the same content written by
//! the scanner deduped onto the first.
//!
//! The divergence was not a decision anyone made; it is what happens
//! when two call sites grow their own copy of a policy. This module is
//! the single copy. It deliberately stops short of the backend calls
//! (`find_knowledge_by_content_hash`, the enqueue-vs-embed choice)
//! because those genuinely differ: REST hands the write to a worker
//! queue and answers `202`, MCP embeds inline so it can return the
//! stored memory to the agent that asked for it.

use klams_types::ErrorDetail;

/// Maximum number of tags accepted on a knowledge write.
pub const MAX_TAGS: usize = 32;
/// Maximum length of any single tag.
pub const MAX_TAG_LEN: usize = 64;

/// Normalized text plus the content hash that identifies it.
#[derive(Debug, Clone)]
pub struct PreparedKnowledge {
    /// Text after `normalize_chunk_text` — this is what gets embedded
    /// and stored, so it is also what must be hashed.
    pub text: String,
    /// Lowercase hex SHA-256 of [`Self::text`].
    pub content_hash: String,
}

/// Normalize `text`, enforce the tag bounds, and hash the result.
///
/// The hash covers the **normalized** text, which is what makes dedupe
/// work across surfaces: two writers whose input differs only in
/// trailing whitespace must land on one point, not two.
///
/// # Errors
/// Returns the same [`ErrorDetail`] shape the REST validator path
/// produces, so each surface can render it in its own error envelope
/// without inventing a second vocabulary of rules.
pub fn prepare(text: &str, tags: &[String]) -> Result<PreparedKnowledge, Vec<ErrorDetail>> {
    let normalized = klams_types::normalize_chunk_text(text);
    if normalized.is_empty() {
        return Err(vec![ErrorDetail {
            field: "text".into(),
            rule: "required".into(),
            message: "text must be non-empty after normalization".into(),
            value: None,
        }]);
    }
    if tags.len() > MAX_TAGS {
        return Err(vec![ErrorDetail {
            field: "tags".into(),
            rule: "length".into(),
            message: format!("at most {MAX_TAGS} tags allowed"),
            value: None,
        }]);
    }
    if let Some(bad) = tags.iter().find(|t| t.is_empty() || t.len() > MAX_TAG_LEN) {
        return Err(vec![ErrorDetail {
            field: "tags".into(),
            rule: "length".into(),
            message: format!("tags must be non-empty and at most {MAX_TAG_LEN} chars"),
            value: Some(serde_json::Value::String(bad.clone())),
        }]);
    }
    let content_hash = sha256_hex(&normalized);
    Ok(PreparedKnowledge {
        text: normalized,
        content_hash,
    })
}

/// Lowercase hex SHA-256 — the content-hash spelling both surfaces and
/// the scanner already used, kept byte-identical so hashes written
/// before this module still match.
#[must_use]
pub fn sha256_hex(s: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(s.as_bytes());
    let digest = h.finalize();
    let mut out = String::with_capacity(digest.len() * 2);
    for b in digest {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{b:02x}");
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashes_the_normalized_form_not_the_raw_input() {
        // The whole point of sharing this: two surfaces handed
        // cosmetically different input must agree on one identity.
        let a = prepare("hello world\n\n", &[]).expect("a");
        let b = prepare("hello world", &[]).expect("b");
        assert_eq!(a.content_hash, b.content_hash);
        assert_eq!(a.text, b.text);
    }

    #[test]
    fn preserves_line_structure_and_indentation() {
        // Sprint 022 (#321): only trailing whitespace and blank-line
        // runs are cleaned up — code chunks must survive intact, or the
        // stored text stops matching what the file actually says.
        let p = prepare("fn main() {\n    let x = 1;  \n}\n", &[]).expect("code chunk");
        assert_eq!(p.text, "fn main() {\n    let x = 1;\n}");
    }

    #[test]
    fn rejects_text_that_normalization_empties() {
        let err = prepare("   \n\t  ", &[]).expect_err("whitespace-only");
        assert_eq!(err[0].field, "text");
        assert_eq!(err[0].rule, "required");
    }

    #[test]
    fn rejects_too_many_tags() {
        let tags: Vec<String> = (0..=MAX_TAGS).map(|i| format!("t{i}")).collect();
        let err = prepare("text", &tags).expect_err("tag count");
        assert_eq!(err[0].field, "tags");
    }

    #[test]
    fn rejects_empty_or_oversized_tag() {
        let err = prepare("text", &[String::new()]).expect_err("empty tag");
        assert_eq!(err[0].field, "tags");

        let long = "x".repeat(MAX_TAG_LEN + 1);
        let err = prepare("text", std::slice::from_ref(&long)).expect_err("long tag");
        assert_eq!(err[0].value, Some(serde_json::Value::String(long)));
    }

    #[test]
    fn hash_is_lowercase_hex_sha256() {
        // Pinned against the known digest so a future refactor cannot
        // silently re-hash the corpus.
        assert_eq!(
            sha256_hex("abc"),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
