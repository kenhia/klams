//! The shared embedder size gate (sprint 027, WI #420).
//!
//! One definition of "will the embedder accept this text", used by every
//! path that can produce an embed call:
//!
//! * the scanner chunker, so it never *emits* an over-budget chunk;
//! * the REST ingest handler ([`crate::IndexKnowledgeRequest`]), so an
//!   over-budget chunk is refused before the `202` and before the
//!   scanner advances its cursor;
//! * MCP `memory_add`, which before this sprint had no length check at
//!   all.
//!
//! ## Why this exists
//!
//! Before 027 nothing in klams knew the embedder's real ceiling. The REST
//! path capped text at 8192 *characters* while the deployed model
//! (`BAAI/bge-small-en-v1.5`) accepts 512 *tokens* — roughly 4× apart. A
//! chunk in that gap passed validation, was accepted with `202`, and then
//! failed at the worker's embed call, which logged and dropped it. The
//! scanner had already advanced its cursor on the `202`, so the chunk was
//! never retried: silent corpus loss (~30k occurrences in one 2h window
//! on kai). Reconciling the two limits against one token-aware estimate is
//! the fix.
//!
//! ## This is an approximation, and the exact answer lives elsewhere
//!
//! **Measured against the live model, no character-based estimate can be
//! both safe and useful.** Binary-searching TEI's real ceiling by content
//! shape (kubs0, `bge-small-en-v1.5`, 512 tokens) gives:
//!
//! | shape | chars accepted | chars/token |
//! |---|---|---|
//! | punctuation-dense | 525 | 1.03 |
//! | minified JSON | 788 | 1.55 |
//! | URLs | 797 | 1.56 |
//! | random identifiers | 819 | 1.61 |
//! | markdown tables | 1054 | 2.07 |
//! | Rust code | 1490 | 2.92 |
//! | English prose | 1691 | 3.32 |
//! | base64 / hex | >20000 | >39 |
//!
//! A divisor safe for the top of that table (~1) would split ordinary
//! 800-character prose chunks in half and wreck retrieval; a divisor that
//! leaves prose alone (~3) under-counts punctuation-dense text by 3×. So
//! the authoritative check is
//! [`Store::check_embed_size`](../../klams_store/trait.Store.html), which
//! asks the model's own tokenizer via TEI's `/tokenize` — cheap, since it
//! runs no forward pass.
//!
//! This module remains the estimate used where that is not reachable:
//! the scanner (which talks only to the klams API), tokenizer-less
//! backends, and as documentation of the ceiling's rough shape.
//!
//! `tiktoken-rs` is deliberately not used even though it is already a
//! workspace dependency: `cl100k_base` is `OpenAI`'s BPE vocabulary,
//! which matches neither bge-family `WordPiece` (the 027 model this
//! table was measured on) nor the Qwen3 BPE vocabulary deployed since
//! 028, so it would produce confidently wrong numbers — worse than an
//! estimate that is honest about being one.
//!
//! ## Direction of error
//!
//! [`estimate_tokens`] leans toward **over**-estimating: it charges
//! non-ASCII at one token per character (matching the measured CJK case
//! exactly) and takes a word-count floor. It still under-counts
//! punctuation-dense ASCII, which is why it is not the last word.

use serde::{Deserialize, Serialize};
use std::fmt;

/// Fallback token ceiling when config doesn't say otherwise.
///
/// 512 was `BAAI/bge-small-en-v1.5`'s measured limit (the model this
/// gate was built against in 027). Sprint 028 swapped production to
/// `Qwen/Qwen3-Embedding-0.6B` (32768 tokens), set explicitly via
/// `[embeddings] max_input_tokens` — the deployed config never uses
/// this default. It stays at 512 deliberately: a too-small fallback
/// refuses loudly at the boundary, a too-large one recreates the
/// silent worker-drop this module exists to prevent.
pub const DEFAULT_MAX_INPUT_TOKENS: usize = 512;

/// `WordPiece` wraps every input in `[CLS]` … `[SEP]`, which count against
/// `max_input_length`. Reserved off the top so the budget we advertise is
/// the budget callers actually get.
const SPECIAL_TOKENS: usize = 2;

/// Conservative characters-per-token divisor for ASCII text.
///
/// `WordPiece` over English prose averages ~4 chars/token; source code and
/// dense punctuation run nearer 3. Using 3 keeps the estimate above the
/// real count for both.
const ASCII_CHARS_PER_TOKEN: usize = 3;

/// Estimate the token count the embedder will see, biased high.
///
/// Two independent lower bounds are computed and the larger wins:
///
/// * **Density** — ASCII characters at [`ASCII_CHARS_PER_TOKEN`] each,
///   plus one token per non-ASCII character. Non-ASCII is charged at 1:1
///   because bge's English `WordPiece` vocabulary has no multi-character
///   pieces for CJK and most symbols, so each becomes its own token (or
///   `[UNK]`).
/// * **Word count** — `WordPiece` never merges across a word boundary, so
///   the number of whitespace-separated words is a hard floor. This is
///   what catches text of many short words (`"a b c d e"`), where the
///   density estimate alone would undercount.
#[must_use]
pub fn estimate_tokens(text: &str) -> usize {
    let mut ascii = 0usize;
    let mut non_ascii = 0usize;
    for c in text.chars() {
        if c.is_ascii() {
            ascii += 1;
        } else {
            non_ascii += 1;
        }
    }
    let density = ascii.div_ceil(ASCII_CHARS_PER_TOKEN) + non_ascii;
    let words = text.split_whitespace().count();
    SPECIAL_TOKENS + density.max(words)
}

/// A text that the embedder would reject, with the numbers a caller needs
/// to fix it on the first retry rather than by bisection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Oversize {
    /// The ceiling in force, in tokens.
    pub limit_tokens: usize,
    /// What [`estimate_tokens`] made of the submitted text.
    pub estimated_tokens: usize,
    /// Submitted length in characters — the unit a caller can act on
    /// directly, since they cannot run the tokenizer themselves.
    pub submitted_chars: usize,
    /// The largest character count that is guaranteed to fit, so the
    /// error can say "split below this" rather than just "too big".
    pub max_chars: usize,
}

impl fmt::Display for Oversize {
    /// Phrased as an instruction, not a complaint: every number a caller
    /// needs to split correctly on the *first* retry is present, so
    /// nobody has to bisect for the ceiling the way #629 and #632 did.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "{} characters (~{} tokens) exceeds the embedder's {}-token limit; \
             split into pieces of at most {} characters",
            self.submitted_chars, self.estimated_tokens, self.limit_tokens, self.max_chars,
        )
    }
}

/// The embedder's accepted-input ceiling, shared by every ingest path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbedLimit {
    max_input_tokens: usize,
}

impl Default for EmbedLimit {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_INPUT_TOKENS)
    }
}

impl EmbedLimit {
    /// Build a gate for a model accepting `max_input_tokens` per input.
    ///
    /// Clamped to at least [`SPECIAL_TOKENS`] + 1 so a nonsensical config
    /// yields a gate that rejects everything rather than one that panics
    /// or silently accepts everything.
    #[must_use]
    pub fn new(max_input_tokens: usize) -> Self {
        Self {
            max_input_tokens: max_input_tokens.max(SPECIAL_TOKENS + 1),
        }
    }

    /// The configured ceiling, in tokens.
    #[must_use]
    pub fn max_input_tokens(self) -> usize {
        self.max_input_tokens
    }

    /// A character budget to quote to callers, who cannot run the
    /// tokenizer themselves and need *some* actionable number.
    ///
    /// This is guidance, **not a guarantee**: it holds for prose and code
    /// but not for punctuation-dense content, which can exhaust the same
    /// token budget in a third of the characters (see the module docs).
    /// Where an exact count is available the error quotes a figure scaled
    /// to the caller's own text instead, which is strictly better advice.
    #[must_use]
    pub fn max_chars(self) -> usize {
        (self.max_input_tokens - SPECIAL_TOKENS) * ASCII_CHARS_PER_TOKEN
    }

    /// Check one text against the ceiling.
    ///
    /// # Errors
    /// Returns [`Oversize`] carrying the limit and the submitted size when
    /// the text would be rejected by the embedder.
    pub fn check(self, text: &str) -> Result<(), Oversize> {
        let estimated_tokens = estimate_tokens(text);
        if estimated_tokens <= self.max_input_tokens {
            return Ok(());
        }
        Err(Oversize {
            limit_tokens: self.max_input_tokens,
            estimated_tokens,
            submitted_chars: text.chars().count(),
            max_chars: self.max_chars(),
        })
    }

    /// Whether `text` fits, for callers that only need the predicate
    /// (the chunker, deciding whether to split again).
    #[must_use]
    pub fn fits(self, text: &str) -> bool {
        self.check(text).is_ok()
    }

    /// Reject only what is **provably** over the ceiling, with no
    /// possibility of a false rejection (sprint 027, #420).
    ///
    /// [`Self::check`] is an estimate and may over-count — which is the
    /// right bias for the chunker (it just splits more) but the wrong one
    /// anywhere a rejection is final. A base64 blob, for example, is
    /// ~39 characters per token, so the estimate would refuse text the
    /// model accepts comfortably.
    ///
    /// The bound used here is a fact about `WordPiece` rather than a
    /// guess: tokenization never merges across whitespace, so a text of
    /// *n* whitespace-separated words yields at least *n* tokens, plus
    /// the two special tokens. If even that minimum exceeds the ceiling,
    /// the text cannot possibly fit.
    #[must_use]
    pub fn certainly_exceeds(self, text: &str) -> Option<Oversize> {
        let words = text.split_whitespace().count();
        let floor = SPECIAL_TOKENS + words;
        if floor <= self.max_input_tokens {
            return None;
        }
        Some(Oversize {
            limit_tokens: self.max_input_tokens,
            estimated_tokens: floor,
            submitted_chars: text.chars().count(),
            max_chars: self.max_chars(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_text_costs_only_special_tokens() {
        assert_eq!(estimate_tokens(""), SPECIAL_TOKENS);
    }

    #[test]
    fn ascii_prose_is_charged_three_chars_per_token() {
        // 12 ASCII chars, 2 words -> density 4 wins over word count 2.
        assert_eq!(estimate_tokens("hello world!"), SPECIAL_TOKENS + 4);
    }

    #[test]
    fn many_short_words_use_the_word_count_floor() {
        // "a b c d e" is 9 chars -> density 3, but `WordPiece` cannot merge
        // across whitespace, so 5 words is the real floor.
        assert_eq!(estimate_tokens("a b c d e"), SPECIAL_TOKENS + 5);
    }

    #[test]
    fn non_ascii_is_charged_one_token_per_char() {
        // No multi-char `WordPiece` pieces exist for CJK in an English
        // vocabulary, so each character is its own token.
        assert_eq!(estimate_tokens("日本語テキスト"), SPECIAL_TOKENS + 7);
    }

    #[test]
    fn estimate_exceeds_naive_char_division() {
        // The whole point: the estimate must sit ABOVE a naive
        // chars/4 reading, or the gate lets through what TEI rejects.
        let text = "x".repeat(2000);
        assert!(estimate_tokens(&text) > 2000 / 4);
    }

    #[test]
    fn default_limit_matches_the_deployed_model() {
        assert_eq!(
            EmbedLimit::default().max_input_tokens(),
            DEFAULT_MAX_INPUT_TOKENS
        );
    }

    #[test]
    fn max_chars_is_a_size_that_actually_fits() {
        // The advertised character ceiling must survive its own gate --
        // otherwise the docs tell callers to submit something we reject.
        let limit = EmbedLimit::default();
        let text = "a".repeat(limit.max_chars());
        assert!(
            limit.fits(&text),
            "advertised max_chars={} did not fit",
            limit.max_chars()
        );
    }

    #[test]
    fn one_char_over_the_advertised_ceiling_is_rejected() {
        let limit = EmbedLimit::default();
        let text = "a".repeat(limit.max_chars() + ASCII_CHARS_PER_TOKEN);
        let err = limit.check(&text).unwrap_err();
        assert_eq!(err.limit_tokens, DEFAULT_MAX_INPUT_TOKENS);
        assert_eq!(err.submitted_chars, text.chars().count());
        assert!(err.estimated_tokens > err.limit_tokens);
    }

    #[test]
    fn the_25kb_memory_add_from_629_is_rejected() {
        // The regression that opened WI #629: a ~2.5 KB memory_add that
        // TEI refused with 413. It must now fail the gate up front,
        // locally, with numbers attached.
        let text = "word ".repeat(500); // 2500 chars
        let err = EmbedLimit::default().check(&text).unwrap_err();
        assert_eq!(err.submitted_chars, 2500);
        assert!(err.max_chars < 2500);
    }

    #[test]
    fn the_hand_split_pieces_from_629_are_accepted() {
        // ...and the ~780 and ~1180 char pieces it was split into, which
        // TEI accepted, must still pass. A gate that rejects these would
        // be over-conservative to the point of being wrong.
        let limit = EmbedLimit::default();
        assert!(limit.fits(&"a".repeat(780)));
        assert!(limit.fits(&"a".repeat(1180)));
    }

    #[test]
    fn scanner_target_chunk_size_fits_with_room_to_spare() {
        // The chunker targets ~800 chars; if that did not fit, the gate
        // would reject the corpus wholesale.
        assert!(EmbedLimit::default().fits(&"lorem ipsum ".repeat(70)));
    }

    #[test]
    fn absurd_config_rejects_rather_than_panicking() {
        let limit = EmbedLimit::new(0);
        assert!(limit.max_input_tokens() >= SPECIAL_TOKENS);
        assert!(limit.check("anything at all").is_err());
    }
}
