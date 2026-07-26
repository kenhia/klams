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
    // Sprint 027 (#420): gate on the embedder's real token budget, not a
    // hardcoded 8192 characters. The old cap sat ~4x above what the model
    // accepts, so a chunk in the gap was accepted with 202, the scanner
    // advanced its cursor, and the worker then dropped the job when the
    // embed failed — silent corpus loss. Rejecting here happens BEFORE
    // the 202, so the scanner's cursor never advances past a chunk that
    // was never stored.
    //
    // The check asks the model's own tokenizer, not a character ratio:
    // measured live, punctuation-dense text hits the 512-token ceiling at
    // ~525 characters while base64 survives past 20,000, so no single
    // chars-per-token constant is both safe and usable.
    if let Err(e) = state
        .store
        .check_embed_size(&normalized, state.embed_limit)
        .await
    {
        return match e {
            klams_store::StoreError::PayloadTooLarge { oversize, .. } => {
                m::incr_writes_failed("knowledge", "too_large");
                Err(ApiError::TooLarge(oversize))
            }
            // The tokenizer was unreachable. Fail the write rather than
            // guess: a 503 leaves the scanner's cursor unadvanced so the
            // chunk is retried, which is the recoverable outcome.
            other => Err(ApiError::Internal {
                request_id: format!("check_embed_size: {other}"),
            }),
        };
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
        .find_knowledge_by_content_hash(&content_hash)
        .await
        .map_err(|e| ApiError::Internal {
            request_id: format!("store-error: {e}"),
        })?
    {
        // Sprint 028 (#642): content-only identity — the dedupe hit must
        // still record that THIS (machine, file) holds the content, or a
        // later delete for the original location would drop a point this
        // host still relies on. A failed attach is a failed write: 500
        // leaves the scanner's cursor unadvanced so the chunk retries.
        if req.machine.is_some() || req.file.is_some() {
            state
                .store
                .attach_knowledge_copy(
                    existing,
                    req.machine.as_deref(),
                    req.file.as_deref(),
                    req.repo.as_deref(),
                )
                .await
                .map_err(|e| ApiError::Internal {
                    request_id: format!("store-error: {e}"),
                })?;
        }
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
        // Sprint 029: lifecycle fields are MCP-surface verbs; the REST
        // ingest path (scanner) never declares volatility or supersedes.
        volatility: None,
        supersedes: None,
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
    ///
    /// Sprint 025 (#637): **required**. It was optional for back-compat,
    /// and omitting it deleted the path's chunks on *every* host — a
    /// hand-run cleanup for one machine silently wiped the others. The
    /// scanner has always sent it; there is no cross-host caller to
    /// preserve.
    #[serde(default)]
    pub machine: Option<String>,
}

/// `POST /memory/knowledge/delete?source_file=<abs_path>&machine=<host>`
/// — remove every chunk whose payload `source_file` matches, on that
/// host. Used by the scanner's vanished-file cleanup (FR-008).
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
    let machine = params.machine.as_deref().map(str::trim).unwrap_or_default();
    if machine.is_empty() {
        return Err(ApiError::Validation {
            field: "machine".into(),
            message: "machine is required — a delete without it would remove \
                      this source_file's chunks on every host"
                .into(),
        });
    }
    let deleted = state
        .store
        .delete_knowledge_by_source_file(&params.source_file, Some(machine))
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
