//! Validation error shapes used by `klams-core::validate` and surfaced
//! through the `ApiError.details` field on the wire.
//!
//! `ErrorDetail` is the canonical struct (matching the `OpenAPI`
//! `ErrorDetail` schema). `ValidationError` is a semantic alias used
//! by validator code paths; `ValidationResult` is the canonical
//! return type of a `Validator`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ErrorDetail {
    /// Dotted path of the offending field, e.g. `payload.hostname`.
    pub field: String,
    /// Machine-readable rule id, e.g. `hostname_shape`,
    /// `timestamp_range`, `required`, `expected_version_required`.
    pub rule: String,
    /// Human-readable description of the violation.
    pub message: String,
    /// The offending value when it is safe to echo back to the
    /// caller. `None` when echoing would risk leaking secrets or the
    /// value is structurally too large.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub value: Option<serde_json::Value>,
}

/// Semantic alias used by validator code paths.
pub type ValidationError = ErrorDetail;

/// Canonical return type of a `Validator`.
pub type ValidationResult = Result<(), Vec<ErrorDetail>>;
