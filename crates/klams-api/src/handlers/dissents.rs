//! HTTP handlers for `/memory/dissents` (sprint 002, US2).
//!
//! These wire the persistence layer in `klams_store::Store` to the
//! REST surface defined in `sprints/002-safety-and-write-ops/contracts/openapi.yaml`.

use crate::router::ApiState;
use crate::ApiError;
use axum::{
    extract::{Path, Query, State},
    Json,
};
use klams_core::metrics as m;
use klams_store::{DissentQuery, Store, StoreError};
use klams_types::{Dissent, DissentPage, DissentStatus, Fact, Source};
use serde::Deserialize;
use time::OffsetDateTime;
use uuid::Uuid;

#[derive(Debug, Deserialize, Default)]
pub struct ListParams {
    pub fact_id: Option<Uuid>,
    pub status: Option<DissentStatus>,
    pub source: Option<Source>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub created_after: Option<OffsetDateTime>,
    #[serde(default, with = "time::serde::rfc3339::option")]
    pub created_before: Option<OffsetDateTime>,
    pub limit: Option<u32>,
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct PromoteRequest {
    pub source: Source,
    pub expected_version: i32,
}

#[derive(Debug, Deserialize)]
pub struct DiscardRequest {
    pub source: Source,
}

pub async fn list<S: Store>(
    State(state): State<ApiState<S>>,
    Query(p): Query<ListParams>,
) -> Result<Json<DissentPage>, ApiError> {
    let q = DissentQuery {
        fact_id: p.fact_id,
        status: p.status,
        source: p.source,
        created_after: p.created_after,
        created_before: p.created_before,
        limit: p.limit.unwrap_or(50),
        cursor: p.cursor,
    };
    let (items, next_cursor) = state.store.list_dissents(q).await.map_err(map_store_err)?;
    Ok(Json(DissentPage { items, next_cursor }))
}

pub async fn get<S: Store>(
    State(state): State<ApiState<S>>,
    Path(id): Path<Uuid>,
) -> Result<Json<Dissent>, ApiError> {
    let dissent = state.store.get_dissent(id).await.map_err(map_store_err)?;
    dissent.map(Json).ok_or_else(|| ApiError::NotFound {
        resource: format!("dissent {id}"),
    })
}

pub async fn promote<S: Store>(
    State(state): State<ApiState<S>>,
    Path(id): Path<Uuid>,
    Json(req): Json<PromoteRequest>,
) -> Result<Json<Fact>, ApiError> {
    require_trust(req.source)?;
    let fact = state
        .store
        .promote_dissent(id, req.source, req.expected_version)
        .await
        .map_err(map_store_err)?;
    m::incr_dissent_outcome("promoted");
    Ok(Json(fact))
}

pub async fn discard<S: Store>(
    State(state): State<ApiState<S>>,
    Path(id): Path<Uuid>,
    Json(req): Json<DiscardRequest>,
) -> Result<Json<Dissent>, ApiError> {
    require_trust(req.source)?;
    let dissent = state
        .store
        .discard_dissent(id, req.source)
        .await
        .map_err(map_store_err)?;
    m::incr_dissent_outcome("discarded");
    Ok(Json(dissent))
}

fn require_trust(source: Source) -> Result<(), ApiError> {
    match source {
        Source::User | Source::Controller => Ok(()),
        other => Err(ApiError::trust_required(format!(
            "promote/discard requires User or Controller, got {}",
            other.as_str()
        ))),
    }
}

fn map_store_err(e: StoreError) -> ApiError {
    match e {
        StoreError::VersionConflict { current_version } => {
            m::incr_version_conflict();
            ApiError::version_conflict(current_version, Uuid::nil())
        }
        StoreError::Gone(what) => ApiError::Gone { what },
        other => ApiError::Internal {
            request_id: format!("store-error: {other}"),
        },
    }
}
