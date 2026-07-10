//! `memory_append_event` MCP tool (sprint 007 T040, US3).
//!
//! Append-only event log entry attributed to an `author_id`. Honours
//! the maintenance window (FR-007 / R-007). Events are not deletable
//! (FR-015) — the corresponding error path lives in `memory_delete`.

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    maintenance, metrics as mcp_metrics, projection,
    tools::McpState,
};
use klams_types::{AppendEvent, PublicAuthorRef, PublicMemory, Source};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const CATEGORY_MAX_LEN: usize = 64;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryAppendEventArgs {
    /// Optional: defaults to the author bound to the caller's bearer
    /// token (WI #62). Pass explicitly to write as a different
    /// registered author.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub author_id: Uuid,
    pub category: String,
    /// Rendered as an object schema, NOT a bare `serde_json::Value` —
    /// schemars turns the latter into the boolean schema `true`, which
    /// Claude Code rejects, discarding the whole tool list (WI #309).
    #[schemars(with = "serde_json::Map<String, serde_json::Value>")]
    pub payload: serde_json::Value,
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub task_id: Option<Uuid>,
}

/// Execute `memory_append_event`.
///
/// # Errors
/// Returns an [`ErrorEnvelope`] for `MAINTENANCE_WINDOW_ACTIVE`,
/// `MISSING_AUTHOR_ID`, `UNKNOWN_AUTHOR_ID`, `INVALID_CATEGORY`,
/// `SCHEMA_VALIDATION_FAILED`, or `INTERNAL_ERROR`.
pub async fn run(
    state: &McpState,
    args: MemoryAppendEventArgs,
) -> Result<PublicMemory, ErrorEnvelope> {
    if let Some(env) = maintenance::check(&state.maintenance) {
        return Err(env);
    }
    if args.author_id.is_nil() {
        return Err(envelope(errors::MISSING_AUTHOR_ID, "author_id is required"));
    }
    let category = args.category.trim().to_string();
    if category.is_empty() || category.len() > CATEGORY_MAX_LEN {
        return Err(envelope(
            errors::INVALID_CATEGORY,
            format!("category must be 1..={CATEGORY_MAX_LEN} characters"),
        ));
    }
    if !args.payload.is_object() {
        return Err(envelope(
            errors::SCHEMA_VALIDATION_FAILED,
            "payload must be a JSON object",
        ));
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

    // FR-005: touch last_seen_at on every authenticated reference.
    let _ = state
        .store
        .postgres
        .touch_author_last_seen_at(author.id)
        .await;

    let req = AppendEvent {
        id: Uuid::now_v7(),
        task_id: args.task_id,
        category,
        payload: args.payload,
        source: Source::AgentProposal,
        author_id: author.id,
    };
    let event = state
        .store
        .postgres
        .append_event(req)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("append_event: {e}")))?;

    mcp_metrics::record_write(
        &author.agent_name,
        author.model.as_deref(),
        mcp_metrics::KIND_EVENT,
    );

    let author_ref = PublicAuthorRef {
        agent_name: author.agent_name,
        model: author.model,
        repo: author.repo,
    };
    Ok(projection::project_event(&event, author_ref))
}
