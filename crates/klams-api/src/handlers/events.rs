//! HTTP handlers for `/memory/events` (POST append + GET list).
//!
//! Append is fire-and-forget: the handler assigns a UUID v7, enqueues
//! a `MemoryWrite::AppendEvent`, and returns `202` with `{id}` without
//! awaiting persistence. Failures during the worker stage increment a
//! metric (defined in US5) and are logged.

use crate::auth::AuthenticatedAuthor;
use crate::router::ApiState;
use crate::ApiError;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::IntoResponse,
    Extension, Json,
};
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use klams_core::{metrics as m, WriteJob};
use klams_store::{EventQuery, Store};
use klams_types::{
    AppendEvent, AppendEventRequest, EventPage, EventWriteResponse, ListEventsParams, MemoryWrite,
    WritePath,
};
use time::OffsetDateTime;
use uuid::Uuid;

/// `POST /memory/events` — enqueue an append, return 202 with the
/// freshly assigned id. Fire-and-forget; no oneshot reply.
pub async fn append<S: Store>(
    State(state): State<ApiState<S>>,
    Extension(author): Extension<AuthenticatedAuthor>,
    Json(req): Json<AppendEventRequest>,
) -> Result<(StatusCode, Json<EventWriteResponse>), ApiError> {
    validate(&req)?;
    let _guard = m::LatencyGuard::with_type(m::WRITE_LATENCY, "event");
    let id = Uuid::now_v7();
    let append = AppendEvent {
        id,
        task_id: req.task_id,
        category: req.category,
        payload: req.payload,
        source: req.source,
        author_id: author.author_id,
    };
    let req_source = append.source;
    let probe = MemoryWrite::AppendEvent(append.clone());
    if let Err(details) = state.validators.validate_write(&probe) {
        let rules: Vec<String> = details.iter().map(|d| d.rule.clone()).collect();
        m::incr_validation_rejections_owned(&rules);
        return Err(ApiError::validation_error(details));
    }
    let job = WriteJob::append_event(append);
    state.queue.try_enqueue(job).map_err(|_| {
        m::incr_writes_failed("event", "queue_full");
        ApiError::QueueFull { retry_after: 1 }
    })?;
    m::incr_writes_accepted("event");
    m::incr_writes_total("event", req_source, WritePath::Canonical);
    m::record_queue(state.queue.depth(), state.queue_capacity, state.workers);
    Ok((
        StatusCode::ACCEPTED,
        Json(EventWriteResponse {
            id,
            path: WritePath::Canonical,
        }),
    ))
}

/// `GET /memory/events` — paginated listing with optional filters.
/// Results are ordered ascending by `created_at`.
pub async fn list<S: Store>(
    State(state): State<ApiState<S>>,
    Query(params): Query<ListEventsParams>,
) -> Result<Json<EventPage>, ApiError> {
    let limit = params.limit.unwrap_or(50).clamp(1, 500);
    let task_id_uuid = params
        .task_id
        .as_deref()
        .and_then(|s| Uuid::parse_str(s).ok());
    let created_after = params.created_after.or(params.since);
    let q = EventQuery {
        task_id: task_id_uuid,
        payload_task_id: params.task_id.clone(),
        category: params.category.clone(),
        service: params.service.clone(),
        created_after,
        created_before: params.created_before,
        limit,
        cursor: params.cursor.clone(),
    };
    let (items, _cursor) = state
        .store
        .list_events(q)
        .await
        .map_err(|e| ApiError::Internal {
            request_id: format!("store-error: {e}"),
        })?;
    let next_cursor = items.last().map(|e| encode_cursor(e.created_at, e.id));
    Ok(Json(EventPage { items, next_cursor }))
}

fn validate(req: &AppendEventRequest) -> Result<(), ApiError> {
    if req.category.trim().is_empty() {
        return Err(ApiError::Validation {
            field: "category".into(),
            message: "category must be non-empty".into(),
        });
    }
    if !req.payload.is_object() {
        return Err(ApiError::Validation {
            field: "payload".into(),
            message: "payload must be a JSON object".into(),
        });
    }
    Ok(())
}

fn encode_cursor(ts: OffsetDateTime, id: Uuid) -> String {
    let raw = format!("{}:{}", ts.unix_timestamp_nanos(), id);
    URL_SAFE_NO_PAD.encode(raw.as_bytes())
}

// IntoResponse for the tuple is provided by axum.
#[allow(dead_code, clippy::used_underscore_items)]
fn _assert_into_response() {
    fn _f<T: IntoResponse>() {}
    _f::<(StatusCode, Json<EventWriteResponse>)>();
}
