//! Wire `ApiError` shape shared across crates and clients.

use crate::validation::ErrorDetail;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiError {
    pub code: String,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
    /// Field-level breakdown populated for `code = "validation_error"`.
    /// New in sprint 002; omitted when empty so 001 clients keep
    /// round-tripping.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub details: Option<Vec<ErrorDetail>>,
    /// Populated for `code = "version_conflict"` so clients can
    /// retry against the current canonical version. New in sprint 002.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_version: Option<i32>,
}
