//! HTTP handlers for `/memory/knowledge/index` (POST) and
//! `/memory/knowledge/{id}` (GET).
//!
//! Indexing is fire-and-forget *after* a synchronous content-hash
//! probe: the handler normalizes the text, hashes it, checks Qdrant
//! for an existing point with the same `content_hash`, and either
//! returns the existing id (`deduped: true`) or enqueues the
//! embed-and-index job (`deduped: false`). Either way the response
//! is `202 Accepted` per the `OpenAPI` contract.

use crate::auth::AuthenticatedAuthor;
use crate::router::ApiState;
use crate::ApiError;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Extension, Json,
};
use klams_core::{metrics as m, WriteJob};
use klams_store::Store;
use klams_types::{
    IndexKnowledge, IndexKnowledgeRequest, IndexKnowledgeResponse, KnowledgeDeleteResponse,
    KnowledgeItem, WritePath,
};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use uuid::Uuid;

/// Maximum normalized-text length accepted by the API, per `OpenAPI`
/// (`413` is returned for anything longer).
const MAX_TEXT_LEN: usize = 8192;
/// Maximum number of tags accepted per request.
const MAX_TAGS: usize = 32;
/// Maximum length of any single tag.
const MAX_TAG_LEN: usize = 64;

pub async fn index<S: Store>(
    State(state): State<ApiState<S>>,
    Extension(author): Extension<AuthenticatedAuthor>,
    Json(req): Json<IndexKnowledgeRequest>,
) -> Result<(StatusCode, Json<IndexKnowledgeResponse>), ApiError> {
    let normalized = normalize_text(&req.text)?;
    if normalized.len() > MAX_TEXT_LEN {
        m::incr_writes_failed("knowledge", "too_large");
        return Err(ApiError::TooLarge);
    }
    validate_tags(&req.tags)?;
    let _guard = m::LatencyGuard::with_type(m::WRITE_LATENCY, "knowledge");
    let content_hash = sha256_hex(&normalized);

    // Sprint 010: bump the author's last_seen_at on every authenticated
    // HTTP write (fire-and-forget), mirroring the MCP write path so
    // daemons writing over HTTP (scanner/monitor) show fresh activity.
    let _ = state
        .store
        .touch_author_last_seen_at(author.author_id)
        .await;

    if let Some(existing) = state
        .store
        .find_knowledge_by_content_hash(&content_hash, req.file.as_deref(), req.machine.as_deref())
        .await
        .map_err(|e| ApiError::Internal {
            request_id: format!("store-error: {e}"),
        })?
    {
        m::incr_writes_total("knowledge", req.source, WritePath::Canonical);
        return Ok((
            StatusCode::ACCEPTED,
            Json(IndexKnowledgeResponse {
                knowledge_id: existing,
                deduped: true,
                path: WritePath::Canonical,
            }),
        ));
    }

    let id = Uuid::now_v7();
    let job = WriteJob::index_knowledge(IndexKnowledge {
        id,
        text: normalized,
        content_hash,
        source: req.source,
        tags: req.tags,
        repo: req.repo,
        file: req.file,
        machine: req.machine,
        author_id: author.author_id,
        chunk_index: req.chunk_index,
        language: req.language,
        heading_path: req.heading_path,
        symbols: req.symbols,
    });
    state.queue.try_enqueue(job).map_err(|_| {
        m::incr_writes_failed("knowledge", "queue_full");
        ApiError::QueueFull { retry_after: 1 }
    })?;
    m::incr_writes_accepted("knowledge");
    m::incr_writes_total("knowledge", req.source, WritePath::Canonical);
    m::record_queue(state.queue.depth(), state.queue_capacity, state.workers);

    Ok((
        StatusCode::ACCEPTED,
        Json(IndexKnowledgeResponse {
            knowledge_id: id,
            deduped: false,
            path: WritePath::Canonical,
        }),
    ))
}

#[derive(Debug, Deserialize)]
pub struct DeleteParams {
    pub source_file: String,
    /// Sprint 023 (#408): scope the delete to a host so one host's
    /// re-index can't drop another host's chunk for the same path.
    /// Optional for back-compat with callers that omit it.
    #[serde(default)]
    pub machine: Option<String>,
}

/// `POST /memory/knowledge/delete?source_file=<abs_path>` — remove
/// every chunk whose payload `source_file` matches. Used by the
/// scanner's vanished-file cleanup (FR-008).
pub async fn delete<S: Store>(
    State(state): State<ApiState<S>>,
    Query(params): Query<DeleteParams>,
) -> Result<(StatusCode, Json<KnowledgeDeleteResponse>), ApiError> {
    if params.source_file.trim().is_empty() {
        return Err(ApiError::Validation {
            field: "source_file".into(),
            message: "source_file must be non-empty".into(),
        });
    }
    let deleted = state
        .store
        .delete_knowledge_by_source_file(&params.source_file, params.machine.as_deref())
        .await
        .map_err(|e| ApiError::Internal {
            request_id: format!("store-error: {e}"),
        })?;
    Ok((
        StatusCode::OK,
        Json(KnowledgeDeleteResponse {
            deleted,
            path: WritePath::Canonical,
        }),
    ))
}

pub async fn get<S: Store>(
    State(state): State<ApiState<S>>,
    Path(id): Path<Uuid>,
) -> Result<Json<KnowledgeItem>, ApiError> {
    state
        .store
        .get_knowledge(id)
        .await
        .map_err(|e| ApiError::Internal {
            request_id: format!("store-error: {e}"),
        })?
        .map(Json)
        .ok_or_else(|| ApiError::NotFound {
            resource: "knowledge item".into(),
        })
}

/// Normalize chunk text for storage + content-hashing, preserving line
/// structure and indentation (sprint 022 #321 — delegates to the shared
/// [`klams_types::normalize_chunk_text`] so the scanner and the API
/// agree and the dedupe hash is stable). Returns Err if empty.
pub fn normalize_text(input: &str) -> Result<String, ApiError> {
    let out = klams_types::normalize_chunk_text(input);
    if out.is_empty() {
        return Err(ApiError::Validation {
            field: "text".into(),
            message: "text must be non-empty after normalization".into(),
        });
    }
    Ok(out)
}

fn validate_tags(tags: &[String]) -> Result<(), ApiError> {
    if tags.len() > MAX_TAGS {
        return Err(ApiError::Validation {
            field: "tags".into(),
            message: format!("at most {MAX_TAGS} tags allowed"),
        });
    }
    for t in tags {
        if t.is_empty() || t.len() > MAX_TAG_LEN {
            return Err(ApiError::Validation {
                field: "tags".into(),
                message: format!("tags must be non-empty and at most {MAX_TAG_LEN} chars"),
            });
        }
    }
    Ok(())
}

fn sha256_hex(s: &str) -> String {
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
    fn normalize_preserves_newlines_and_indentation() {
        // Sprint 022 (#321): line structure + indentation survive; only
        // trailing whitespace and blank-line runs are cleaned up.
        assert_eq!(
            normalize_text("fn main() {\n    let x = 1;  \n}\n").unwrap(),
            "fn main() {\n    let x = 1;\n}"
        );
    }

    #[test]
    fn normalize_empty_rejected() {
        assert!(matches!(
            normalize_text("   \n\t"),
            Err(ApiError::Validation { .. })
        ));
    }

    #[test]
    fn sha256_is_stable() {
        let a = sha256_hex("hello");
        let b = sha256_hex("hello");
        assert_eq!(a, b);
        assert_eq!(a.len(), 64);
    }
}
