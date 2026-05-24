//! `memory_delete` MCP tool (sprint 007 T046, US4).
//!
//! Soft-delete a fact or knowledge item by id. Idempotent (FR-014):
//! a second call on an already-soft-deleted item returns success
//! without rewriting `deleted_at` / `deleted_by_author_id`. Events
//! are append-only (FR-015) — return `EVENTS_NOT_DELETABLE`.

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    maintenance, metrics as mcp_metrics,
    tools::McpState,
};
use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryDeleteArgs {
    #[schemars(with = "String")]
    pub author_id: Uuid,
    #[schemars(with = "String")]
    pub id: Uuid,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryDeleteOutput {
    #[serde(with = "uuid::serde::simple")]
    pub id: Uuid,
    pub deleted_at: chrono::DateTime<Utc>,
}

/// Execute `memory_delete`.
///
/// # Errors
/// Returns an [`ErrorEnvelope`] for `MAINTENANCE_WINDOW_ACTIVE`,
/// `MISSING_AUTHOR_ID`, `UNKNOWN_AUTHOR_ID`, `NOT_FOUND`,
/// `EVENTS_NOT_DELETABLE`, or `INTERNAL_ERROR`.
pub async fn run(
    state: &McpState,
    args: MemoryDeleteArgs,
) -> Result<MemoryDeleteOutput, ErrorEnvelope> {
    if let Some(env) = maintenance::check(&state.maintenance) {
        return Err(env);
    }
    if args.author_id.is_nil() {
        return Err(envelope(errors::MISSING_AUTHOR_ID, "author_id is required"));
    }
    let author = state
        .store
        .postgres
        .get_author_by_id(args.author_id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("get_author_by_id: {e}")))?
        .ok_or_else(|| {
            envelope(
                errors::UNKNOWN_AUTHOR_ID,
                format!("author_id {} not found", args.author_id),
            )
        })?;
    let _ = state
        .store
        .postgres
        .touch_author_last_seen_at(author.id)
        .await;

    // Events first: append-only.
    if state
        .store
        .postgres
        .event_exists(args.id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("event_exists: {e}")))?
    {
        return Err(envelope(
            errors::EVENTS_NOT_DELETABLE,
            "events are append-only (FR-015)",
        ));
    }

    // Facts (Postgres).
    if state
        .store
        .postgres
        .fact_exists_any(args.id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("fact_exists_any: {e}")))?
    {
        // soft_delete_fact returns false if already soft-deleted; we
        // treat that as a success per FR-014 idempotency.
        let _ = state
            .store
            .postgres
            .soft_delete_fact(args.id, author.id)
            .await
            .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("soft_delete_fact: {e}")))?;
        mcp_metrics::record_delete(
            &author.agent_name,
            author.model.as_deref(),
            mcp_metrics::MODE_SOFT,
        );
        return Ok(MemoryDeleteOutput {
            id: args.id,
            deleted_at: Utc::now(),
        });
    }

    // Knowledge (Qdrant).
    if state
        .store
        .qdrant
        .point_exists_any(args.id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("point_exists_any: {e}")))?
    {
        let now = time::OffsetDateTime::now_utc();
        state
            .store
            .qdrant
            .soft_delete_payload(args.id, author.id, now)
            .await
            .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("soft_delete_payload: {e}")))?;
        mcp_metrics::record_delete(
            &author.agent_name,
            author.model.as_deref(),
            mcp_metrics::MODE_SOFT,
        );
        return Ok(MemoryDeleteOutput {
            id: args.id,
            deleted_at: Utc::now(),
        });
    }

    Err(envelope(
        errors::NOT_FOUND,
        format!("memory {} not found", args.id),
    ))
}
