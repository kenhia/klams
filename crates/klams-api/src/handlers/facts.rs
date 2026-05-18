//! HTTP handlers for `/memory/facts` (POST upsert + GET list).

use crate::router::ApiState;
use crate::ApiError;
use axum::{
    extract::{Query, State},
    Json,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use klams_core::{metrics as m, WriteJob};
use klams_store::{FactQuery, Store};
use klams_types::{Fact, FactPage, FactType, Source, UpsertFact, UpsertFactRequest};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

/// `POST /memory/facts` — enqueue an upsert and await the persisted
/// fact via a oneshot reply channel.
pub async fn upsert<S: Store>(
    State(state): State<ApiState<S>>,
    Json(req): Json<UpsertFactRequest>,
) -> Result<Json<Fact>, ApiError> {
    validate_payload(&req.payload)?;
    let _guard = m::LatencyGuard::with_type(m::WRITE_LATENCY, "fact");

    let (job, rx) = WriteJob::upsert_fact_with_reply(UpsertFact {
        fact_type: req.fact_type,
        payload: req.payload,
        source: req.source,
        explicit_id: req.explicit_id,
    });
    state.queue.try_enqueue(job).map_err(|_| {
        m::incr_writes_failed("fact", "queue_full");
        ApiError::QueueFull { retry_after: 1 }
    })?;
    m::incr_writes_accepted("fact");
    m::record_queue(state.queue.depth(), state.queue_capacity, state.workers);

    match rx.await {
        Ok(klams_core::WriteReply::Fact(Ok(fact))) => Ok(Json(fact)),
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
