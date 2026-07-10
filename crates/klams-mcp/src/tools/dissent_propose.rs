//! `dissent_propose` MCP tool (sprint 015 — companion enablement).
//!
//! File a dissent directly against a live canonical fact. This is the
//! external-agent path for semantic contradiction detection (e.g.
//! klams-mind): the write-path dissent trigger only fires on
//! same-fact trust conflicts, so contradictions found *after* the
//! fact need this tool. Proposals land as `Source::AgentProposal`
//! (lowest trust) and resolve only through the human promote/discard
//! flow in the viewport.

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    maintenance, metrics as mcp_metrics,
    tools::McpState,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

const MAX_REASON_CHARS: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct DissentProposeArgs {
    /// Optional: defaults to the author bound to the caller's bearer
    /// token (WI #62). Pass explicitly to write as a different
    /// registered author.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub author_id: Uuid,
    /// The canonical fact being disputed; must exist and be live.
    #[schemars(with = "String")]
    pub fact_id: Uuid,
    /// The corrected payload — same shape as the fact type's payload.
    /// Promoting the dissent overwrites the fact with this verbatim.
    pub proposed_payload: serde_json::Value,
    /// Why the proposer believes the fact is wrong (1..=2000 chars).
    pub reason: String,
    /// Optional: the memory (fact/knowledge/event id) that conflicts
    /// with `fact_id`. Recorded for the human reviewer; not validated
    /// against the store.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub contradicting_memory_id: Option<Uuid>,
}

#[derive(Debug, Clone, Serialize)]
pub struct DissentProposeOutput {
    #[serde(with = "uuid::serde::simple")]
    pub dissent_id: Uuid,
    #[serde(with = "uuid::serde::simple")]
    pub fact_id: Uuid,
    pub status: &'static str,
    /// `true` when this proposal matched an existing pending dissent
    /// for the same `(fact_id, payload)` and bumped its
    /// `submission_count` instead of creating a new row.
    pub deduped: bool,
}

/// Execute `dissent_propose`.
///
/// # Errors
/// Returns an [`ErrorEnvelope`] for `MAINTENANCE_WINDOW_ACTIVE`,
/// `MISSING_AUTHOR_ID`, `UNKNOWN_AUTHOR_ID`,
/// `SCHEMA_VALIDATION_FAILED` (empty/oversized reason, non-object
/// payload), `NOT_FOUND` (fact missing or soft-deleted), or
/// `INTERNAL_ERROR`.
pub async fn run(
    state: &McpState,
    args: DissentProposeArgs,
) -> Result<DissentProposeOutput, ErrorEnvelope> {
    if let Some(env) = maintenance::check(&state.maintenance) {
        return Err(env);
    }
    if args.author_id.is_nil() {
        return Err(envelope(errors::MISSING_AUTHOR_ID, "author_id is required"));
    }
    let reason = args.reason.trim();
    if reason.is_empty() || reason.chars().count() > MAX_REASON_CHARS {
        return Err(envelope(
            errors::SCHEMA_VALIDATION_FAILED,
            format!("reason must be 1..={MAX_REASON_CHARS} chars"),
        ));
    }
    if !args.proposed_payload.is_object() {
        return Err(envelope(
            errors::SCHEMA_VALIDATION_FAILED,
            "proposed_payload must be a JSON object",
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
    let _ = state
        .store
        .postgres
        .touch_author_last_seen_at(author.id)
        .await;

    let outcome = state
        .store
        .postgres
        .propose_dissent(
            args.fact_id,
            &args.proposed_payload,
            author.id,
            reason,
            args.contradicting_memory_id,
        )
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("propose_dissent: {e}")))?;
    let Some((dissent_id, deduped)) = outcome else {
        return Err(envelope(
            errors::NOT_FOUND,
            format!("fact {} not found or deleted", args.fact_id),
        ));
    };

    mcp_metrics::record_write(&author.agent_name, author.model.as_deref(), "dissent");
    Ok(DissentProposeOutput {
        dissent_id,
        fact_id: args.fact_id,
        status: "pending",
        deduped,
    })
}
