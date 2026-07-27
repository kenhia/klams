//! `memory_admin_hard_delete` MCP tool (sprint 007 T048, US4).
//!
//! Permanently removes a fact row (Postgres) or a knowledge point
//! (Qdrant). Events are append-only (FR-015) and return
//! `EVENTS_NOT_DELETABLE`.

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    metrics as mcp_metrics,
    tools::McpState,
};
use chrono::Utc;
use klams_store::Store;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryAdminHardDeleteArgs {
    #[schemars(with = "String")]
    pub id: Uuid,
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryAdminHardDeleteOutput {
    #[serde(with = "uuid::serde::simple")]
    pub id: Uuid,
    pub hard_deleted_at: chrono::DateTime<Utc>,
}

/// Execute `memory_admin_hard_delete`.
///
/// # Errors
/// Returns an [`ErrorEnvelope`] for `NOT_FOUND`, `EVENTS_NOT_DELETABLE`,
/// or `INTERNAL_ERROR`.
pub async fn run<S: Store>(
    state: &McpState<S>,
    args: MemoryAdminHardDeleteArgs,
) -> Result<MemoryAdminHardDeleteOutput, ErrorEnvelope> {
    if state
        .store
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
        .hard_delete_fact(args.id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("hard_delete_fact: {e}")))?
    {
        mcp_metrics::record_delete("admin", None, mcp_metrics::MODE_HARD);
        return Ok(MemoryAdminHardDeleteOutput {
            id: args.id,
            hard_deleted_at: Utc::now(),
        });
    }

    if state
        .store
        .point_exists_any(args.id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("point_exists_any: {e}")))?
    {
        state
            .store
            .hard_delete_point(args.id)
            .await
            .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("hard_delete_point: {e}")))?;
        mcp_metrics::record_delete("admin", None, mcp_metrics::MODE_HARD);
        return Ok(MemoryAdminHardDeleteOutput {
            id: args.id,
            hard_deleted_at: Utc::now(),
        });
    }

    Err(envelope(
        errors::NOT_FOUND,
        format!("memory {} not found", args.id),
    ))
}
