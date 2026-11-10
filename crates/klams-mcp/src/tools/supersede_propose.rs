//! `supersede_propose` MCP tool (sprint 058, klams-mind WI 3331).
//!
//! The propose-merge path Ken chose on 2026-10-23. `memory_supersede`
//! needs the caller to own the old memory or hold `manage`; a
//! consolidation agent like klams-mind owns neither the records it finds
//! duplicated nor the scope, by design. It files a proposal here instead,
//! and an identity with `manage` accepts or rejects it with
//! `supersede_proposal_resolve`, which performs the supersede.
//!
//! One proposal names a **keeper** and one or more records to **retire**
//! into it (decision 2: multi-record retirement is native, so "A is
//! replaced by existing B" and "A and B merge into C" are both one
//! proposal). With `text`, acceptance writes the merged text as a new
//! memory that supersedes the keeper, and every retired record points at
//! it; without `text`, the keeper stays as it is and the retired records
//! point at the keeper.
//!
//! Every argument is a string or an array of strings, deliberately: the
//! untyped `payload` fields are what kept `dissent_propose` unreachable
//! from Claude Code (#850).

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    maintenance, metrics as mcp_metrics,
    tools::memory_add::{enforce_embed_size, sha256_hex},
    tools::memory_delete::DeleteCaller,
    tools::McpState,
};
use klams_store::Store;
use klams_types::{KnowledgeItem, NewSupersedeProposal, Source, SupersedeProposal};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Most records one proposal may retire.
pub const MAX_RETIRE_IDS: usize = 20;
const MAX_REASON_CHARS: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SupersedeProposeArgs {
    /// The record that survives. Without `text` it stays exactly as it
    /// is; with `text`, the merged replacement supersedes it. Must be
    /// live, agent-authored knowledge.
    #[schemars(with = "String")]
    pub keeper_id: Uuid,
    /// The records to retire into the survivor (1..=20, distinct, not
    /// the keeper). Each must be live, agent-authored knowledge.
    #[schemars(with = "Vec<String>")]
    pub retire_ids: Vec<Uuid>,
    /// Optional merged replacement text. Omit it when the keeper already
    /// says everything (a duplicate); pass it when the records combine
    /// into something new (a merge).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// Why these records should be consolidated (1..=2000 chars) — the
    /// reviewer reads this.
    pub reason: String,
}

#[derive(Debug, Clone, Serialize)]
pub struct SupersedeProposeOutput {
    pub proposal: SupersedeProposal,
    /// `true` when an identical pending proposal already existed and
    /// this call bumped its `submission_count` instead of queueing a
    /// second copy.
    pub deduped: bool,
}

/// Load `id` and check it may take part in a supersede: it exists, it
/// is live, and it is agent-authored knowledge. Shared by propose time
/// and acceptance time (decision 3 re-validates at acceptance).
///
/// # Errors
/// `NOT_FOUND` (unknown, or superseded/deleted), `NOT_AGENT_AUTHORED`,
/// or the mapped store error.
pub(crate) async fn live_agent_knowledge<S: Store>(
    state: &McpState<S>,
    id: Uuid,
) -> Result<KnowledgeItem, ErrorEnvelope> {
    let item = state
        .store
        .get_knowledge(id)
        .await
        .map_err(|e| errors::from_store_error("get_knowledge", &e))?
        .ok_or_else(|| {
            envelope(
                errors::NOT_FOUND,
                format!("no knowledge memory with id {id}"),
            )
        })?;
    let deleted = state
        .store
        .point_is_soft_deleted(id)
        .await
        .map_err(|e| errors::from_store_error("point_is_soft_deleted", &e))?;
    if deleted == Some(true) {
        return Err(envelope(
            errors::NOT_FOUND,
            format!("memory {id} is already superseded or deleted"),
        ));
    }
    if item.source != Source::AgentProposal {
        return Err(envelope(
            errors::NOT_AGENT_AUTHORED,
            format!(
                "memory {id} is scanner-ingested knowledge; it is derived from a \
                 file, so fix the file and let the re-scan update it"
            ),
        ));
    }
    Ok(item)
}

/// The dedupe key: keeper, sorted retire set, and text. Two proposals
/// that would do the same thing hash the same.
fn proposal_hash(keeper: Uuid, retire_sorted: &[Uuid], text: Option<&str>) -> String {
    let retire: Vec<String> = retire_sorted.iter().map(Uuid::to_string).collect();
    let text = text.map_or_else(|| "-".to_string(), |t| format!("+{t}"));
    sha256_hex(&format!("{keeper}\n{}\n{text}", retire.join(",")))
}

/// Execute `supersede_propose`.
///
/// # Errors
/// [`ErrorEnvelope`] for `MAINTENANCE_WINDOW_ACTIVE`,
/// `MISSING_AUTHOR_ID`, `UNKNOWN_AUTHOR_ID`, `SCHEMA_VALIDATION_FAILED`
/// (reason, retire set or text out of shape), `NOT_FOUND`,
/// `NOT_AGENT_AUTHORED`, `PAYLOAD_TOO_LARGE`, or `INTERNAL_ERROR`.
pub async fn run<S: Store>(
    state: &McpState<S>,
    args: SupersedeProposeArgs,
    caller: Option<&DeleteCaller>,
) -> Result<SupersedeProposeOutput, ErrorEnvelope> {
    if let Some(env) = maintenance::check(&state.maintenance) {
        return Err(env);
    }
    let caller = caller.ok_or_else(|| {
        envelope(
            errors::MISSING_AUTHOR_ID,
            "no author is bound to this bearer token; add `agent_name` to \
             its [[auth.tokens]] grant",
        )
    })?;
    let reason = args.reason.trim();
    if reason.is_empty() || reason.chars().count() > MAX_REASON_CHARS {
        return Err(envelope(
            errors::SCHEMA_VALIDATION_FAILED,
            format!("reason must be 1..={MAX_REASON_CHARS} chars"),
        ));
    }
    let mut retire = args.retire_ids.clone();
    retire.sort_unstable();
    retire.dedup();
    if retire.is_empty() || retire.len() > MAX_RETIRE_IDS {
        return Err(envelope(
            errors::SCHEMA_VALIDATION_FAILED,
            format!("retire_ids must name 1..={MAX_RETIRE_IDS} distinct records"),
        ));
    }
    if retire.len() != args.retire_ids.len() {
        return Err(envelope(
            errors::SCHEMA_VALIDATION_FAILED,
            "retire_ids contains a duplicate",
        ));
    }
    if retire.contains(&args.keeper_id) {
        return Err(envelope(
            errors::SCHEMA_VALIDATION_FAILED,
            "keeper_id must not also appear in retire_ids",
        ));
    }
    let text = match args.text {
        Some(t) => {
            let t = t.trim().to_string();
            if t.is_empty() {
                return Err(envelope(
                    errors::SCHEMA_VALIDATION_FAILED,
                    "text must be non-empty when present (omit it to keep the keeper as is)",
                ));
            }
            Some(t)
        }
        None => None,
    };

    let author = state
        .store
        .get_author_by_id(caller.author_id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("get_author_by_id: {e}")))?
        .ok_or_else(|| {
            envelope(
                errors::UNKNOWN_AUTHOR_ID,
                format!("author_id {} not found", caller.author_id),
            )
        })?;
    let _ = state.store.touch_author_last_seen_at(author.id).await;

    live_agent_knowledge(state, args.keeper_id).await?;
    for id in &retire {
        live_agent_knowledge(state, *id).await?;
    }
    // Refuse oversize text now rather than at acceptance, where the
    // proposer is no longer in the loop to shorten it.
    if let Some(t) = &text {
        enforce_embed_size(state, author.id, &author.agent_name, t).await?;
    }

    let new = NewSupersedeProposal {
        id: Uuid::now_v7(),
        author_id: author.id,
        keeper_id: args.keeper_id,
        proposal_hash: proposal_hash(args.keeper_id, &retire, text.as_deref()),
        retire_ids: retire,
        text,
        reason: reason.to_string(),
    };
    let (proposal, deduped) = state
        .store
        .insert_supersede_proposal(&new)
        .await
        .map_err(|e| errors::from_store_error("insert_supersede_proposal", &e))?;

    mcp_metrics::record_write(
        &author.agent_name,
        author.model.as_deref(),
        "supersede_proposal",
    );
    Ok(SupersedeProposeOutput { proposal, deduped })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_distinguishes_no_text_from_empty_text_and_orders_retire() {
        let k = Uuid::now_v7();
        let a = Uuid::now_v7();
        let b = Uuid::now_v7();
        let mut ab = vec![a, b];
        ab.sort_unstable();
        assert_eq!(
            proposal_hash(k, &ab, None),
            proposal_hash(k, &ab, None),
            "deterministic"
        );
        assert_ne!(proposal_hash(k, &ab, None), proposal_hash(k, &ab, Some("")));
        assert_ne!(proposal_hash(k, &ab, None), proposal_hash(k, &[a], None));
    }
}
