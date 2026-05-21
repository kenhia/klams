//! Token-cost estimator: tiktoken `cl100k_base` + `chars/4` fallback.
//!
//! Sprint 005 (Phase 4) — T017. The estimator is used by the
//! `ContextBuilder` to fit retrieved items inside the request's
//! `token_budget`. The chosen encoder is reported back in the
//! response so callers know which counter produced the numbers.

use klams_types::TokenEncoderId;
use std::sync::Arc;
use tiktoken_rs::{cl100k_base, CoreBPE};
use tracing::warn;

/// Configured token-counting mode. Mirrors `[tokens] mode` in the
/// service config (`"tiktoken"` or `"chars_div4"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TokenMode {
    Tiktoken,
    CharsDiv4,
}

impl TokenMode {
    /// Parse the wire value from `[tokens] mode`. Unknown values
    /// fall back to `CharsDiv4` with a `warn!`.
    #[must_use]
    pub fn from_config_str(value: &str) -> Self {
        match value {
            "tiktoken" => TokenMode::Tiktoken,
            "chars_div4" => TokenMode::CharsDiv4,
            other => {
                warn!(value = other, "unknown [tokens] mode; falling back to chars_div4");
                TokenMode::CharsDiv4
            }
        }
    }
}

/// Counts tokens for context-budget accounting. Cheap to clone
/// (the `CoreBPE` is held behind `Arc`).
#[derive(Clone)]
pub struct TokenCounter {
    encoder: TokenEncoderId,
    bpe: Option<Arc<CoreBPE>>,
}

impl std::fmt::Debug for TokenCounter {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TokenCounter")
            .field("encoder", &self.encoder)
            .field("bpe", &self.bpe.as_ref().map(|_| "<cl100k_base>"))
            .finish()
    }
}

impl TokenCounter {
    /// Build a `TokenCounter` for the requested mode. If
    /// `mode == Tiktoken` but the cl100k tables fail to load (should
    /// not happen — they ship with the crate) we log a warning and
    /// degrade to `chars/4`.
    #[must_use]
    pub fn new(mode: TokenMode) -> Self {
        match mode {
            TokenMode::Tiktoken => match cl100k_base() {
                Ok(bpe) => Self {
                    encoder: TokenEncoderId::Cl100kBase,
                    bpe: Some(Arc::new(bpe)),
                },
                Err(err) => {
                    warn!(error = %err, "cl100k_base load failed; degrading to chars_div4");
                    Self {
                        encoder: TokenEncoderId::CharsDiv4,
                        bpe: None,
                    }
                }
            },
            TokenMode::CharsDiv4 => Self {
                encoder: TokenEncoderId::CharsDiv4,
                bpe: None,
            },
        }
    }

    /// Returns the encoder actually in use. Surfaced through the
    /// `ContextBundle.token_encoder` field so callers can normalise
    /// counts across budgets.
    #[must_use]
    pub fn encoder_id(&self) -> TokenEncoderId {
        self.encoder
    }

    /// Estimate the token cost of `text`. Never panics; on the
    /// tiktoken path falls back to `chars/4` when encoding fails.
    #[must_use]
    pub fn count(&self, text: &str) -> u32 {
        match (&self.bpe, self.encoder) {
            (Some(bpe), TokenEncoderId::Cl100kBase) => {
                let tokens = bpe.encode_with_special_tokens(text).len();
                u32::try_from(tokens).unwrap_or(u32::MAX)
            }
            _ => chars_div4(text),
        }
    }

    /// Estimate the token cost of a JSON payload by serializing and
    /// counting. Used for structured `payload` blobs on
    /// `ContextItem`.
    #[must_use]
    pub fn count_json(&self, value: &serde_json::Value) -> u32 {
        let s = serde_json::to_string(value).unwrap_or_default();
        self.count(&s)
    }
}

fn chars_div4(text: &str) -> u32 {
    let chars = text.chars().count();
    u32::try_from(chars.div_ceil(4)).unwrap_or(u32::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiktoken_path_counts_known_text() {
        let c = TokenCounter::new(TokenMode::Tiktoken);
        assert_eq!(c.encoder_id(), TokenEncoderId::Cl100kBase);
        // "hello world" is 2 cl100k tokens (verified against
        // tiktoken upstream).
        assert_eq!(c.count("hello world"), 2);
    }

    #[test]
    fn fallback_path_counts_chars_div4() {
        let c = TokenCounter::new(TokenMode::CharsDiv4);
        assert_eq!(c.encoder_id(), TokenEncoderId::CharsDiv4);
        // 11 chars / 4 = 3 (ceil)
        assert_eq!(c.count("hello world"), 3);
        // empty
        assert_eq!(c.count(""), 0);
        // multi-byte chars counted by chars, not bytes
        assert_eq!(c.count("aé"), 1);
    }

    #[test]
    fn from_config_str_handles_known_and_unknown() {
        assert_eq!(TokenMode::from_config_str("tiktoken"), TokenMode::Tiktoken);
        assert_eq!(
            TokenMode::from_config_str("chars_div4"),
            TokenMode::CharsDiv4
        );
        assert_eq!(
            TokenMode::from_config_str("bogus"),
            TokenMode::CharsDiv4
        );
    }

    #[test]
    fn large_payload_does_not_panic() {
        let big = "a".repeat(50_000);
        let c = TokenCounter::new(TokenMode::Tiktoken);
        let n = c.count(&big);
        assert!(n > 0);
        // tiktoken should give roughly ~6k-13k tokens for 50k 'a's
        // but we only assert non-degenerate output here.
    }

    #[test]
    fn count_json_round_trips() {
        let c = TokenCounter::new(TokenMode::CharsDiv4);
        let v = serde_json::json!({"k": "v", "n": 42});
        assert!(c.count_json(&v) > 0);
    }
}
