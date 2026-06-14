//! HTTP handlers for `/memory/facts` (POST upsert + GET list).

use crate::auth::AuthenticatedAuthor;
use crate::router::ApiState;
use crate::ApiError;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Extension, Json,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use klams_core::{metrics as m, WriteJob};
use klams_store::{FactQuery, Store};
use klams_types::{
    DissentStatus, DissentSubmittedResponse, FactPage, FactType, FactWriteOutcome,
    FactWriteResponse, MemoryWrite, Source, UpsertFact, UpsertFactRequest, WritePath,
};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

/// `POST /memory/facts` — enqueue an upsert and await the persisted
/// fact via a oneshot reply channel.
pub async fn upsert<S: Store>(
    State(state): State<ApiState<S>>,
    Extension(author): Extension<AuthenticatedAuthor>,
    Json(req): Json<UpsertFactRequest>,
) -> Result<Response, ApiError> {
    validate_payload(&req.payload)?;
    let upsert = UpsertFact {
        fact_type: req.fact_type,
        payload: req.payload,
        source: req.source,
        explicit_id: req.explicit_id,
        expected_version: req.expected_version,
        author_id: author.author_id,
    };
    let probe = MemoryWrite::UpsertFact(upsert.clone());
    if let Err(details) = state.validators.validate_write(&probe) {
        let rules: Vec<String> = details.iter().map(|d| d.rule.clone()).collect();
        m::incr_validation_rejections_owned(&rules);
        return Err(ApiError::validation_error(details));
    }
    let _guard = m::LatencyGuard::with_type(m::WRITE_LATENCY, "fact");

    // Sprint 010: bump last_seen_at on authenticated HTTP writes
    // (fire-and-forget), mirroring the MCP write path.
    let _ = state
        .store
        .touch_author_last_seen_at(author.author_id)
        .await;

    let req_source = upsert.source;
    let (job, rx) = WriteJob::upsert_fact_with_reply(upsert);
    state.queue.try_enqueue(job).map_err(|_| {
        m::incr_writes_failed("fact", "queue_full");
        ApiError::QueueFull { retry_after: 1 }
    })?;
    m::incr_writes_accepted("fact");
    m::record_queue(state.queue.depth(), state.queue_capacity, state.workers);

    match rx.await {
        Ok(klams_core::WriteReply::Fact(Ok(outcome))) => {
            Ok(outcome_to_response(outcome, req_source))
        }
        Ok(klams_core::WriteReply::Fact(Err(e))) => {
            m::incr_writes_failed("fact", "store_error");
            Err(ApiError::Internal {
                request_id: format!("store-error: {e}"),
            })
        }
        Ok(_) => Err(ApiError::Internal {
            request_id: "wrong reply variant".into(),
        }),
        Err(_) => Err(ApiError::Internal {
            request_id: "worker dropped reply channel".into(),
        }),
    }
}

fn outcome_to_response(outcome: FactWriteOutcome, req_source: Source) -> Response {
    match outcome {
        FactWriteOutcome::Persisted { fact } => {
            m::incr_writes_total("fact", fact.source, WritePath::Canonical);
            Json(FactWriteResponse {
                fact,
                path: WritePath::Canonical,
                dissent_id: None,
            })
            .into_response()
        }
        FactWriteOutcome::Dissented {
            dissent_id,
            fact_id,
        } => {
            m::incr_dissent_outcome("accepted");
            m::incr_writes_total("fact", req_source, WritePath::Dissent);
            (
                StatusCode::ACCEPTED,
                Json(DissentSubmittedResponse {
                    dissent_id,
                    fact_id,
                    status: DissentStatus::Pending,
                    deduped: false,
                    path: WritePath::Dissent,
                }),
            )
                .into_response()
        }
        FactWriteOutcome::VersionConflict {
            current_version,
            fact_id,
        } => {
            m::incr_version_conflict();
            ApiError::version_conflict(current_version, fact_id).into_response()
        }
    }
}

/// `GET /memory/facts` — paginated listing with optional filters.
pub async fn list<S: Store>(
    State(state): State<ApiState<S>>,
    Query(params): Query<ListFactsParams>,
) -> Result<Json<FactPage>, ApiError> {
    let limit = params.limit.unwrap_or(50).clamp(1, 500);
    let q = FactQuery {
        fact_type: params.r#type.as_deref().and_then(parse_fact_type),
        source: params.source.as_deref().and_then(parse_source),
        created_after: params
            .created_after
            .as_deref()
            .map(parse_rfc3339)
            .transpose()?,
        created_before: params
            .created_before
            .as_deref()
            .map(parse_rfc3339)
            .transpose()?,
        limit,
        cursor: params.cursor.clone(),
    };
    let (items, _cursor) = state
        .store
        .list_facts(q)
        .await
        .map_err(|e| ApiError::Internal {
            request_id: format!("store-error: {e}"),
        })?;
    let next_cursor = items.last().map(|f| encode_cursor(f.created_at, f.id));
    Ok(Json(FactPage { items, next_cursor }))
}

#[derive(Debug, Deserialize)]
pub struct ListFactsParams {
    pub r#type: Option<String>,
    pub source: Option<String>,
    pub created_after: Option<String>,
    pub created_before: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

fn validate_payload(payload: &serde_json::Value) -> Result<(), ApiError> {
    if !payload.is_object() {
        return Err(ApiError::Validation {
            field: "payload".into(),
            message: "payload must be a JSON object".into(),
        });
    }
    Ok(())
}

fn parse_fact_type(s: &str) -> Option<FactType> {
    match s {
        "UserFact" => Some(FactType::UserFact),
        "TaskFact" => Some(FactType::TaskFact),
        "EnvFact" => Some(FactType::EnvFact),
        _ => None,
    }
}

fn parse_source(s: &str) -> Option<Source> {
    match s {
        "User" => Some(Source::User),
        "Controller" => Some(Source::Controller),
        "Task" => Some(Source::Task),
        "AgentProposal" => Some(Source::AgentProposal),
        _ => None,
    }
}

fn parse_rfc3339(s: &str) -> Result<OffsetDateTime, ApiError> {
    OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).map_err(|_| {
        ApiError::Validation {
            field: "created_after/before".into(),
            message: format!("invalid RFC3339 timestamp: {s}"),
        }
    })
}

fn encode_cursor(ts: OffsetDateTime, id: Uuid) -> String {
    let raw = format!("{}:{}", ts.unix_timestamp_nanos(), id);
    URL_SAFE_NO_PAD.encode(raw.as_bytes())
}
