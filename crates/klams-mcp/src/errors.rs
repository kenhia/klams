//! Canonical MCP error codes (sprint 007).
//!
//! Mirrors `sprints/007-mcp-server/contracts/error-codes.md`. These
//! constants form the machine-readable contract surfaced via
//! `_meta.error_code` in the standard MCP error envelope.

#![allow(non_upper_case_globals)]

pub const MISSING_AUTHOR_ID: &str = "MISSING_AUTHOR_ID";
pub const UNKNOWN_AUTHOR_ID: &str = "UNKNOWN_AUTHOR_ID";
pub const INVALID_AGENT_NAME: &str = "INVALID_AGENT_NAME";
pub const INVALID_REPO_PATH: &str = "INVALID_REPO_PATH";
pub const EXTRA_TOO_LARGE: &str = "EXTRA_TOO_LARGE";
pub const INVALID_KIND: &str = "INVALID_KIND";
pub const INVALID_CATEGORY: &str = "INVALID_CATEGORY";
pub const INVALID_TOP_K: &str = "INVALID_TOP_K";
pub const INVALID_LIMIT: &str = "INVALID_LIMIT";
pub const EMPTY_QUERY: &str = "EMPTY_QUERY";
pub const SCHEMA_VALIDATION_FAILED: &str = "SCHEMA_VALIDATION_FAILED";
pub const EMBEDDING_UNAVAILABLE: &str = "EMBEDDING_UNAVAILABLE";
pub const NOT_FOUND: &str = "NOT_FOUND";
pub const NOT_SOFT_DELETED: &str = "NOT_SOFT_DELETED";
pub const EVENTS_NOT_DELETABLE: &str = "EVENTS_NOT_DELETABLE";
/// Sprint 025 (#636) — author removal refused because the row still
/// owns memories. Merge it into another author first.
pub const AUTHOR_HAS_MEMORIES: &str = "AUTHOR_HAS_MEMORIES";
pub const INSUFFICIENT_SCOPE: &str = "INSUFFICIENT_SCOPE";
pub const MAINTENANCE_WINDOW_ACTIVE: &str = "MAINTENANCE_WINDOW_ACTIVE";
pub const INTERNAL_ERROR: &str = "INTERNAL_ERROR";
// Sprint 008 — `event_search` (and `GET /v1/memories`) window validation.
pub const WINDOW_TOO_LARGE: &str = "WINDOW_TOO_LARGE";
pub const INVALID_WINDOW: &str = "INVALID_WINDOW";

/// Standard MCP error envelope body.
///
/// Tool handlers return this serialized as JSON inside the MCP
/// `CallToolResult` shape. The `retry_after_seconds` slot is populated
/// only for [`MAINTENANCE_WINDOW_ACTIVE`] and [`EMBEDDING_UNAVAILABLE`].
#[derive(Debug, Clone, serde::Serialize)]
pub struct ErrorEnvelope {
    #[serde(rename = "isError")]
    pub is_error: bool,
    pub content: Vec<ErrorContent>,
    #[serde(rename = "_meta")]
    pub meta: ErrorMeta,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ErrorContent {
    #[serde(rename = "type")]
    pub kind: &'static str,
    pub text: String,
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct ErrorMeta {
    pub error_code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub retry_after_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub window_max_days: Option<u32>,
}

/// Build a standard error envelope.
#[must_use]
pub fn envelope(code: &'static str, message: impl Into<String>) -> ErrorEnvelope {
    ErrorEnvelope {
        is_error: true,
        content: vec![ErrorContent {
            kind: "text",
            text: message.into(),
        }],
        meta: ErrorMeta {
            error_code: code,
            retry_after_seconds: None,
            window_max_days: None,
        },
    }
}

/// Build an error envelope that carries a retry hint
/// (`_meta.retry_after_seconds`). Only valid for
/// [`MAINTENANCE_WINDOW_ACTIVE`] and [`EMBEDDING_UNAVAILABLE`].
#[must_use]
pub fn envelope_with_retry(
    code: &'static str,
    message: impl Into<String>,
    retry_after_seconds: u64,
) -> ErrorEnvelope {
    ErrorEnvelope {
        is_error: true,
        content: vec![ErrorContent {
            kind: "text",
            text: message.into(),
        }],
        meta: ErrorMeta {
            error_code: code,
            retry_after_seconds: Some(retry_after_seconds),
            window_max_days: None,
        },
    }
}

/// Build an error envelope carrying `_meta.window_max_days`. Used by
/// sprint 008 window-cap validation (`WINDOW_TOO_LARGE`).
#[must_use]
pub fn envelope_with_window_max(
    code: &'static str,
    message: impl Into<String>,
    window_max_days: u32,
) -> ErrorEnvelope {
    ErrorEnvelope {
        is_error: true,
        content: vec![ErrorContent {
            kind: "text",
            text: message.into(),
        }],
        meta: ErrorMeta {
            error_code: code,
            retry_after_seconds: None,
            window_max_days: Some(window_max_days),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn envelope_serializes_with_is_error_true() {
        let env = envelope(MISSING_AUTHOR_ID, "no author");
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["isError"], true);
        assert_eq!(v["_meta"]["error_code"], "MISSING_AUTHOR_ID");
        assert!(v["_meta"].get("retry_after_seconds").is_none());
    }

    #[test]
    fn retry_envelope_includes_seconds() {
        let env = envelope_with_retry(MAINTENANCE_WINDOW_ACTIVE, "wait", 30);
        let v = serde_json::to_value(&env).unwrap();
        assert_eq!(v["_meta"]["retry_after_seconds"], 30);
    }
}
