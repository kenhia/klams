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
/// The embedding backend is down or failing transiently. **Retryable** —
/// this code always carries `retry_after_seconds`.
pub const EMBEDDING_UNAVAILABLE: &str = "EMBEDDING_UNAVAILABLE";
/// Sprint 027 (#629/#632) — the submitted text exceeds the embedder's
/// token ceiling. Permanent for that text: the message names the limit
/// and the submitted size so the caller can split correctly on the first
/// retry. Never carries `retry_after_seconds`.
///
/// Before this code existed, a size rejection was reported as
/// `EMBEDDING_UNAVAILABLE` + `retry_after_seconds: 5` — every signal
/// telling the caller to wait and retry, which could never succeed. The
/// documented consequence was silent knowledge loss: agents concluded the
/// embedder was down and moved on without writing.
pub const PAYLOAD_TOO_LARGE: &str = "PAYLOAD_TOO_LARGE";
/// Sprint 027 (#629) — the embedder refused the request itself (a
/// permanent 4xx that is not a size problem). Distinct from
/// [`EMBEDDING_UNAVAILABLE`], which means the service is unwell rather
/// than the input being wrong. Never carries `retry_after_seconds`.
pub const EMBEDDING_REJECTED: &str = "EMBEDDING_REJECTED";
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

/// How long to advise waiting when the embedder is transiently down.
const EMBEDDING_RETRY_SECONDS: u64 = 5;
/// How long to advise waiting on transient backend trouble. Shorter than
/// the embedder's: an exhausted connection pool typically clears in well
/// under a second once in-flight queries drain.
const BACKEND_RETRY_SECONDS: u64 = 2;

/// Map a [`StoreError`] onto the MCP wire, preserving its
/// transient/permanent classification (sprint 027, WI #629).
///
/// This is the single place the taxonomy reaches callers, and it exists
/// because klams previously got the question wrong in both directions:
///
/// * a permanent HTTP 413 was reported as `EMBEDDING_UNAVAILABLE` with
///   `retry_after_seconds: 5` — advice that could never work; and
/// * a transient Postgres pool exhaustion was reported as a bare
///   `INTERNAL_ERROR` with no retry hint — advice to give up on
///   something that would have succeeded moments later.
///
/// The rule enforced here: `retry_after_seconds` appears if and only if
/// [`StoreError::is_transient`] is true.
#[must_use]
pub fn from_store_error(context: &str, e: &klams_store::StoreError) -> ErrorEnvelope {
    use klams_store::StoreError as SE;
    match e {
        SE::PayloadTooLarge { oversize, .. } => envelope(PAYLOAD_TOO_LARGE, oversize.to_string()),
        SE::EmbeddingRejected(m) => {
            envelope(EMBEDDING_REJECTED, format!("embedder rejected input: {m}"))
        }
        SE::Embedding(m) => envelope_with_retry(
            EMBEDDING_UNAVAILABLE,
            format!("embedding failed: {m}"),
            EMBEDDING_RETRY_SECONDS,
        ),
        SE::BackendUnavailable(m) => envelope_with_retry(
            INTERNAL_ERROR,
            format!("{context}: {m}"),
            BACKEND_RETRY_SECONDS,
        ),
        other => envelope(INTERNAL_ERROR, format!("{context}: {other}")),
    }
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
