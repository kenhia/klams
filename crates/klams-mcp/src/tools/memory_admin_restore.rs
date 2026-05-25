//! `memory_admin_restore` MCP tool (sprint 007 T047, US4).
//!
//! Clears `deleted_at` / `deleted_by_author_id` on a soft-deleted
//! fact (Postgres) or knowledge point (Qdrant). Returns `NOT_FOUND`
//! when the id is unknown and `NOT_SOFT_DELETED` when the item is
//! already live.

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    metrics as mcp_metrics,
    tools::McpState,
};
use chrono::Utc;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryAdminRestoreArgs {
    #[schemars(with = "String")]
    pub id: Uuid,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryAdminRestoreOutput {
    #[serde(with = "uuid::serde::simple")]
    pub id: Uuid,
    pub restored_at: chrono::DateTime<Utc>,
}

/// Execute `memory_admin_restore`.
///
/// # Errors
/// Returns an [`ErrorEnvelope`] for `NOT_FOUND`, `NOT_SOFT_DELETED`,
/// `EVENTS_NOT_DELETABLE`, or `INTERNAL_ERROR`.
pub async fn run(
    state: &McpState,
    args: MemoryAdminRestoreArgs,
) -> Result<MemoryAdminRestoreOutput, ErrorEnvelope> {
    // Events are never deletable so they can't be restored.
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

    if state
        .store
        .postgres
        .fact_exists_any(args.id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("fact_exists_any: {e}")))?
    {
        let restored = state
            .store
            .postgres
            .restore_fact(args.id)
            .await
            .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("restore_fact: {e}")))?;
        if !restored {
            return Err(envelope(
                errors::NOT_SOFT_DELETED,
                format!("fact {} is not soft-deleted", args.id),
            ));
        }
        mcp_metrics::record_delete("admin", None, mcp_metrics::MODE_RESTORED);
        return Ok(MemoryAdminRestoreOutput {
            id: args.id,
            restored_at: Utc::now(),
        });
    }

    if let Some(deleted) = state
        .store
        .qdrant
        .point_is_soft_deleted(args.id)
        .await
        .map_err(|e| {
            envelope(
                errors::INTERNAL_ERROR,
                format!("point_is_soft_deleted: {e}"),
            )
        })?
    {
        if !deleted {
            return Err(envelope(
                errors::NOT_SOFT_DELETED,
                format!("knowledge {} is not soft-deleted", args.id),
            ));
        }
        state
            .store
            .qdrant
            .restore_payload(args.id)
            .await
            .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("restore_payload: {e}")))?;
        mcp_metrics::record_delete("admin", None, mcp_metrics::MODE_RESTORED);
        return Ok(MemoryAdminRestoreOutput {
            id: args.id,
            restored_at: Utc::now(),
        });
    }

    Err(envelope(
        errors::NOT_FOUND,
        format!("memory {} not found", args.id),
    ))
}
