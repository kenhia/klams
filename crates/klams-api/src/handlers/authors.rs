//! Sprint 007 (US5) — `/v1/authors` viewport drilldown routes.

use crate::error::ApiError;
use crate::router::ApiState;
use axum::{
    extract::{Path, Query, State},
    Json,
};
use klams_store::{
    AuthorListQuery, AuthorMemoriesQuery, AuthorMemoryKind, AuthorMemoryRow, AuthorMemoryStateOut,
    AuthorMemoryStateQuery, AuthorWithCountsOut, Store,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Compact author projection for the REST list/get endpoints.
#[derive(Debug, Clone, Serialize)]
pub struct PublicAuthor {
    pub id: Uuid,
    pub agent_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub repo: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_app: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub client_version: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
    pub counts: AuthorCounts,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthorCounts {
    pub writes: i64,
    pub knowledge: i64,
    pub events: i64,
    pub soft_deletes: i64,
    pub restores_received: i64,
}

fn to_public(row: AuthorWithCountsOut) -> PublicAuthor {
    PublicAuthor {
        id: row.author.id,
        agent_name: row.author.agent_name,
        model: row.author.model,
        session_title: row.author.session_title,
        repo: row.author.repo,
        client_app: row.author.client_app,
        client_version: row.author.client_version,
        created_at: row.author.created_at,
        last_seen_at: row.author.last_seen_at,
        counts: AuthorCounts {
            writes: row.writes_facts,
            knowledge: row.writes_knowledge,
            events: row.events,
            soft_deletes: row.soft_deletes_authored,
            restores_received: row.restores_received,
        },
    }
}

#[derive(Debug, Deserialize)]
pub struct ListAuthorsParams {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub agent_name: Option<String>,
    pub since: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuthorPage {
    pub authors: Vec<PublicAuthor>,
    pub next_cursor: Option<String>,
}

/// `GET /v1/authors`
pub async fn list<S: Store>(
    State(state): State<ApiState<S>>,
    Query(params): Query<ListAuthorsParams>,
) -> Result<Json<AuthorPage>, ApiError> {
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let since = params.since.as_deref().map(parse_rfc3339).transpose()?;
    let q = AuthorListQuery {
        limit,
        agent_name: params.agent_name,
        since,
        cursor: params.cursor,
    };
    let (rows, next_cursor) = state
        .store
        .list_authors_v1(q)
        .await
        .map_err(|e| map_store(&e))?;
    Ok(Json(AuthorPage {
        authors: rows.into_iter().map(to_public).collect(),
        next_cursor,
    }))
}

/// `GET /v1/authors/{id}`
pub async fn get<S: Store>(
    State(state): State<ApiState<S>>,
    Path(id): Path<Uuid>,
) -> Result<Json<PublicAuthor>, ApiError> {
    let row = state
        .store
        .get_author_v1(id)
        .await
        .map_err(|e| map_store(&e))?
        .ok_or_else(|| ApiError::NotFound {
            resource: format!("author {id}"),
        })?;
    Ok(Json(to_public(row)))
}

#[derive(Debug, Deserialize)]
pub struct ListMemoriesParams {
    pub limit: Option<u32>,
    pub cursor: Option<String>,
    pub kinds: Option<String>,
    pub state: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct AuthorMemoryWire {
    #[serde(flatten)]
    pub memory: Value,
    pub state: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_at: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub deleted_by: Option<klams_types::PublicAuthorRef>,
}

#[derive(Debug, Serialize)]
pub struct AuthorMemoriesPage {
    pub memories: Vec<AuthorMemoryWire>,
    pub next_cursor: Option<String>,
}

/// `GET /v1/authors/{id}/memories`
pub async fn memories<S: Store>(
    State(state): State<ApiState<S>>,
    Path(id): Path<Uuid>,
    Query(params): Query<ListMemoriesParams>,
) -> Result<Json<AuthorMemoriesPage>, ApiError> {
    let limit = params.limit.unwrap_or(50).clamp(1, 200);
    let kinds = parse_kinds(params.kinds.as_deref())?;
    let mstate = parse_state(params.state.as_deref())?;
    let q = AuthorMemoriesQuery {
        author_id: id,
        kinds,
        state: mstate,
        limit,
        cursor: params.cursor,
    };
    let (rows, next_cursor) = state
        .store
        .list_author_memories(q)
        .await
        .map_err(map_store_with_404(id))?;
    let memories = rows
        .into_iter()
        .map(memory_to_wire)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Json(AuthorMemoriesPage {
        memories,
        next_cursor,
    }))
}

fn memory_to_wire(row: AuthorMemoryRow) -> Result<AuthorMemoryWire, ApiError> {
    let memory = serde_json::to_value(&row.memory).map_err(|e| ApiError::Internal {
        request_id: format!("serialize memory: {e}"),
    })?;
    Ok(AuthorMemoryWire {
        memory,
        state: match row.state {
            AuthorMemoryStateOut::Live => "live",
            AuthorMemoryStateOut::Deleted => "deleted",
        },
        deleted_at: row.deleted_at,
        deleted_by: row.deleted_by,
    })
}

fn parse_rfc3339(s: &str) -> Result<chrono::DateTime<chrono::Utc>, ApiError> {
    chrono::DateTime::parse_from_rfc3339(s)
        .map(|t| t.with_timezone(&chrono::Utc))
        .map_err(|e| ApiError::Validation {
            field: "since".into(),
            message: format!("invalid RFC3339 timestamp: {e}"),
        })
}

fn parse_kinds(raw: Option<&str>) -> Result<Vec<AuthorMemoryKind>, ApiError> {
    let Some(raw) = raw else {
        return Ok(Vec::new());
    };
    let mut out = Vec::new();
    for tok in raw.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        out.push(match tok {
            "fact" => AuthorMemoryKind::Fact,
            "knowledge" => AuthorMemoryKind::Knowledge,
            "event" => AuthorMemoryKind::Event,
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

fn parse_state(raw: Option<&str>) -> Result<AuthorMemoryStateQuery, ApiError> {
    match raw.unwrap_or("live") {
        "live" => Ok(AuthorMemoryStateQuery::Live),
        "deleted" => Ok(AuthorMemoryStateQuery::Deleted),
        "all" => Ok(AuthorMemoryStateQuery::All),
        other => Err(ApiError::Validation {
            field: "state".into(),
            message: format!("unknown state `{other}` (expected live|deleted|all)"),
        }),
    }
}

fn map_store(e: &klams_store::StoreError) -> ApiError {
    ApiError::Internal {
        request_id: format!("store-error: {e}"),
    }
}

fn map_store_with_404(id: Uuid) -> impl FnOnce(klams_store::StoreError) -> ApiError {
    move |e| {
        let msg = e.to_string();
        if msg.contains("not found") {
            ApiError::NotFound {
                resource: format!("author {id}"),
            }
        } else {
            map_store(&e)
        }
    }
}
