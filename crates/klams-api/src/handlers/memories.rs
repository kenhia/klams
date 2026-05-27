//! Sprint 008 (US3) — `GET /v1/memories` cross-author listing.

use crate::error::ApiError;
use crate::router::ApiState;
use axum::{
    extract::{Query, State},
    Json,
};
use chrono::{DateTime, Duration, Utc};
use klams_store::{
    ListMemoriesQuery, ListMemoriesRow, MemoryKindFilter, MemoryStateFilter, MemoryStateOut, Store,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub struct ListMemoriesParams {
    pub since: Option<String>,
    pub until: Option<String>,
    pub kinds: Option<String>,
    pub state: Option<String>,
    pub authors: Option<String>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MemoryWire {
    #[serde(flatten)]
    pub memory: Value,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<DateTime<Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_by: Option<klams_types::PublicAuthorRef>,
}

#[derive(Debug, Serialize)]
pub struct MemoriesPage {
    pub memories: Vec<MemoryWire>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// `GET /v1/memories`
pub async fn list<S: Store>(
    State(state): State<ApiState<S>>,
    Query(params): Query<ListMemoriesParams>,
) -> Result<Json<MemoriesPage>, ApiError> {
    let now = Utc::now();
    let until = parse_rfc3339(params.until.as_deref(), "until")?.unwrap_or(now);
    let since = parse_rfc3339(params.since.as_deref(), "since")?
        .unwrap_or_else(|| until - Duration::hours(24));

    if since > until {
        return Err(ApiError::InvalidWindow {
            message: format!(
                "window is inverted: since ({}) is after until ({})",
                since.to_rfc3339(),
                until.to_rfc3339()
            ),
        });
    }

    let max_days = state.api.memories_max_window_days;
    if (until - since) > Duration::days(i64::from(max_days)) {
        return Err(ApiError::WindowTooLarge { max_days });
    }

    let kinds = parse_kinds(params.kinds.as_deref())?;
    let state_filter = parse_state(params.state.as_deref())?;
    let authors = parse_authors(params.authors.as_deref())?;
    let limit = params.limit.unwrap_or(50).clamp(1, 200);

    let q = ListMemoriesQuery {
        since,
        until,
        kinds,
        state: state_filter,
        authors,
        limit,
        cursor: params.cursor,
    };

    let (rows, next_cursor) = state
        .store
        .list_memories(q)
        .await
        .map_err(|e| map_store(&e))?;
    let memories = rows
        .into_iter()
        .map(row_to_wire)
        .collect::<Result<Vec<_>, _>>()?;

    tracing::info!(
        token_hash = "unknown",
        requested_window_hours = (until - since).num_hours(),
        kinds = memories_kind_label(params.kinds.as_deref()),
        state = params.state.as_deref().unwrap_or("live"),
        authors_count = memories_author_count(params.authors.as_deref()),
        result_count = memories.len(),
        "api.v1.memories.list"
    );

    Ok(Json(MemoriesPage {
        memories,
        next_cursor,
    }))
}

fn row_to_wire(row: ListMemoriesRow) -> Result<MemoryWire, ApiError> {
    let memory = serde_json::to_value(&row.memory).map_err(|e| ApiError::Internal {
        request_id: format!("serialize memory: {e}"),
    })?;
    Ok(MemoryWire {
        memory,
        state: match row.state {
            MemoryStateOut::Live => "live",
            MemoryStateOut::Deleted => "deleted",
        },
        deleted_at: row.deleted_at,
        deleted_by: row.deleted_by,
    })
}

fn parse_rfc3339(
    raw: Option<&str>,
    field: &'static str,
) -> Result<Option<DateTime<Utc>>, ApiError> {
    raw.map(|s| {
        DateTime::parse_from_rfc3339(s)
            .map(|t| t.with_timezone(&Utc))
            .map_err(|e| ApiError::Validation {
                field: field.into(),
                message: format!("invalid RFC3339 timestamp: {e}"),
            })
    })
    .transpose()
}

fn parse_kinds(raw: Option<&str>) -> Result<Vec<MemoryKindFilter>, ApiError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for tok in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        out.push(match tok {
            "fact" => MemoryKindFilter::Fact,
            "knowledge" => MemoryKindFilter::Knowledge,
            "event" => MemoryKindFilter::Event,
            other => {
                return Err(ApiError::Validation {
                    field: "kinds".into(),
                    message: format!("unknown kind `{other}` (expected fact|knowledge|event)"),
                })
            }
        });
    }
    Ok(out)
}

fn parse_state(raw: Option<&str>) -> Result<MemoryStateFilter, ApiError> {
    match raw.unwrap_or("live") {
        "live" => Ok(MemoryStateFilter::Live),
        "deleted" => Ok(MemoryStateFilter::Deleted),
        "all" => Ok(MemoryStateFilter::All),
        other => Err(ApiError::Validation {
            field: "state".into(),
            message: format!("unknown state `{other}` (expected live|deleted|all)"),
        }),
    }
}

fn parse_authors(raw: Option<&str>) -> Result<Vec<Uuid>, ApiError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for tok in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        out.push(Uuid::parse_str(tok).map_err(|e| ApiError::Validation {
            field: "authors".into(),
            message: format!("invalid uuid `{tok}`: {e}"),
        })?);
    }
    out.sort_unstable();
    out.dedup();
    Ok(out)
}

fn map_store(e: &klams_store::StoreError) -> ApiError {
    ApiError::Internal {
        request_id: format!("store-error: {e}"),
    }
}

fn memories_kind_label(raw: Option<&str>) -> String {
    raw.filter(|s| !s.is_empty())
        .map_or_else(|| "all".to_string(), ToString::to_string)
}

fn memories_author_count(raw: Option<&str>) -> usize {
    raw.map_or(0, |s| {
        s.split(',')
            .map(str::trim)
            .filter(|v| !v.is_empty())
            .count()
    })
}
