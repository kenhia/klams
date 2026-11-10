//! `supersede_proposal_resolve` MCP tool (sprint 058, klams-mind WI 3331).
//!
//! The acceptance half of the propose-merge path: an identity holding
//! `manage` accepts or rejects one or more `supersede_propose` proposals
//! in one call (Ken's "or Ken in bulk").
//!
//! **Accepting** re-validates every record the proposal names (decision
//! 3: overlapping clusters are settled at acceptance, not at filing — a
//! proposal whose keeper or retirees were consumed by a sibling accepted
//! first becomes `stale` and changes nothing). Then it performs the
//! supersede:
//!
//! * with `text`, a new memory carrying it is written **as the
//!   proposer** (it is their wording; `supersedes` = the keeper), and the
//!   keeper and every retiree are marked superseded by it;
//! * without `text`, every retiree is marked superseded by the keeper.
//!
//! Each retired record carries `superseded_by` — the lineage — and
//! `deleted_by_author_id` names the **resolver**, who made the call.
//!
//! The row is claimed (`pending` → `accepted`) before anything is
//! written, so two resolvers cannot apply one proposal twice. Qdrant has
//! no transactions: if a write fails part-way, what was marked is
//! restored, the replacement is removed, and the proposal goes back to
//! `pending` with the failure in its note.

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    maintenance, metrics as mcp_metrics,
    tools::memory_add::sha256_hex,
    tools::memory_delete::DeleteCaller,
    tools::supersede_propose::live_agent_knowledge,
    tools::McpState,
};
use klams_store::Store;
use klams_types::{IndexKnowledge, Source, SupersedeProposal, SupersedeProposalStatus};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::fmt::Write as _;
use uuid::Uuid;

/// Most proposals one call may resolve.
pub const MAX_PROPOSALS_PER_CALL: usize = 50;
const MAX_NOTE_CHARS: usize = 2000;

/// What to do with the named proposals.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ResolveDecision {
    Accept,
    Reject,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SupersedeProposalResolveArgs {
    /// The proposals to resolve (1..=50), as returned in
    /// `supersede_propose`'s `proposal.id`. Applied in this order.
    #[schemars(with = "Vec<String>")]
    pub proposal_ids: Vec<Uuid>,
    /// `accept` performs the supersede; `reject` closes the proposal
    /// and changes no memory.
    pub decision: ResolveDecision,
    /// Optional note recorded on every resolved proposal (<=2000 chars).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

/// Per-proposal result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ResolveOutcome {
    Accepted,
    Rejected,
    /// A named record was no longer live agent-authored knowledge; the
    /// proposal is closed as `stale` and nothing changed.
    Stale,
    /// Already resolved (by someone else, or an earlier call).
    NotPending,
    NotFound,
    /// A write failed part-way; it was rolled back and the proposal is
    /// `pending` again. Retry.
    Failed,
}

#[derive(Debug, Clone, Serialize)]
pub struct ResolveResult {
    pub proposal_id: Uuid,
    pub outcome: ResolveOutcome,
    /// On `accepted`: the live record every retired id now points at.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result_memory_id: Option<Uuid>,
    /// On `stale`, `not_pending` and `failed`: why.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct SupersedeProposalResolveOutput {
    pub results: Vec<ResolveResult>,
}

impl ResolveResult {
    fn new(proposal_id: Uuid, outcome: ResolveOutcome) -> Self {
        Self {
            proposal_id,
            outcome,
            result_memory_id: None,
            detail: None,
        }
    }

    fn detail(mut self, d: impl Into<String>) -> Self {
        self.detail = Some(d.into());
        self
    }
}

/// Execute `supersede_proposal_resolve`. Scope (`manage`) is enforced by
/// the dispatcher; this needs only the resolver's identity.
///
/// # Errors
/// Whole-call failures only: `MAINTENANCE_WINDOW_ACTIVE`,
/// `MISSING_AUTHOR_ID`, `SCHEMA_VALIDATION_FAILED`. Everything about an
/// individual proposal is reported in its [`ResolveResult`].
pub async fn run<S: Store>(
    state: &McpState<S>,
    args: SupersedeProposalResolveArgs,
    caller: Option<&DeleteCaller>,
) -> Result<SupersedeProposalResolveOutput, ErrorEnvelope> {
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
    if args.proposal_ids.is_empty() || args.proposal_ids.len() > MAX_PROPOSALS_PER_CALL {
        return Err(envelope(
            errors::SCHEMA_VALIDATION_FAILED,
            format!("proposal_ids must name 1..={MAX_PROPOSALS_PER_CALL} proposals"),
        ));
    }
    let note = args
        .note
        .as_deref()
        .map(str::trim)
        .filter(|n| !n.is_empty());
    if note.is_some_and(|n| n.chars().count() > MAX_NOTE_CHARS) {
        return Err(envelope(
            errors::SCHEMA_VALIDATION_FAILED,
            format!("note must be at most {MAX_NOTE_CHARS} chars"),
        ));
    }
    let _ = state
        .store
        .touch_author_last_seen_at(caller.author_id)
        .await;

    let mut results = Vec::with_capacity(args.proposal_ids.len());
    for id in &args.proposal_ids {
        let r = resolve_one(state, *id, args.decision, caller.author_id, note).await;
        results.push(r);
    }
    Ok(SupersedeProposalResolveOutput { results })
}

async fn resolve_one<S: Store>(
    state: &McpState<S>,
    id: Uuid,
    decision: ResolveDecision,
    resolver: Uuid,
    note: Option<&str>,
) -> ResolveResult {
    use SupersedeProposalStatus as St;
    let proposal = match state.store.get_supersede_proposal(id).await {
        Ok(Some(p)) => p,
        Ok(None) => return ResolveResult::new(id, ResolveOutcome::NotFound),
        Err(e) => {
            return ResolveResult::new(id, ResolveOutcome::Failed)
                .detail(format!("get_supersede_proposal: {e}"))
        }
    };
    if proposal.status != St::Pending {
        return ResolveResult::new(id, ResolveOutcome::NotPending)
            .detail(format!("proposal is {}", proposal.status.as_str()));
    }
    let to = match decision {
        ResolveDecision::Accept => St::Accepted,
        ResolveDecision::Reject => St::Rejected,
    };
    // The claim: only one resolver moves the row out of `pending`.
    match state
        .store
        .transition_supersede_proposal(id, St::Pending, to, Some(resolver), note, None)
        .await
    {
        Ok(true) => {}
        Ok(false) => {
            return ResolveResult::new(id, ResolveOutcome::NotPending)
                .detail("resolved concurrently by another caller")
        }
        Err(e) => {
            return ResolveResult::new(id, ResolveOutcome::Failed)
                .detail(format!("transition_supersede_proposal: {e}"))
        }
    }
    if decision == ResolveDecision::Reject {
        return ResolveResult::new(id, ResolveOutcome::Rejected);
    }

    // Decision 3: re-validate at acceptance.
    for rid in std::iter::once(&proposal.keeper_id).chain(&proposal.retire_ids) {
        if let Err(env) = live_agent_knowledge(state, *rid).await {
            let why = env
                .content
                .first()
                .map_or_else(String::new, |c| c.text.clone());
            let stale_note = format!("stale at acceptance: {why}");
            let _ = state
                .store
                .transition_supersede_proposal(
                    id,
                    St::Accepted,
                    St::Stale,
                    Some(resolver),
                    Some(&stale_note),
                    None,
                )
                .await;
            return ResolveResult::new(id, ResolveOutcome::Stale).detail(why);
        }
    }

    match apply(state, &proposal, resolver).await {
        Ok(survivor) => {
            let _ = state
                .store
                .transition_supersede_proposal(
                    id,
                    St::Accepted,
                    St::Accepted,
                    Some(resolver),
                    note,
                    Some(survivor),
                )
                .await;
            let mut r = ResolveResult::new(id, ResolveOutcome::Accepted);
            r.result_memory_id = Some(survivor);
            r
        }
        Err(detail) => {
            let failed_note = format!("apply failed, rolled back: {detail}");
            let _ = state
                .store
                .transition_supersede_proposal(
                    id,
                    St::Accepted,
                    St::Pending,
                    None,
                    Some(&failed_note),
                    None,
                )
                .await;
            ResolveResult::new(id, ResolveOutcome::Failed).detail(detail)
        }
    }
}

/// Perform the supersede. Returns the survivor's id, or a description
/// of what failed after rolling back what it could.
async fn apply<S: Store>(
    state: &McpState<S>,
    p: &SupersedeProposal,
    resolver: Uuid,
) -> Result<Uuid, String> {
    let mut written: Option<Uuid> = None;
    let (survivor, to_retire): (Uuid, Vec<Uuid>) = match &p.text {
        None => (p.keeper_id, p.retire_ids.clone()),
        Some(text) => {
            let keeper = state
                .store
                .get_knowledge(p.keeper_id)
                .await
                .map_err(|e| format!("get_knowledge {}: {e}", p.keeper_id))?
                .ok_or_else(|| format!("keeper {} vanished", p.keeper_id))?;
            let new_id = Uuid::now_v7();
            let req = IndexKnowledge {
                id: new_id,
                content_hash: sha256_hex(text),
                text: text.clone(),
                source: Source::AgentProposal,
                tags: keeper.tags.clone(),
                repo: keeper.repo.clone(),
                file: keeper.file.clone(),
                machine: None,
                author_id: p.author_id,
                chunk_index: None,
                language: None,
                heading_path: None,
                symbols: Vec::new(),
                volatility: keeper.volatility.clone(),
                supersedes: Some(keeper.id),
            };
            let embedding = state
                .store
                .embed_document(&req.text)
                .await
                .map_err(|e| format!("embedding: {e}"))?;
            state
                .store
                .index_knowledge_with_embedding(req, embedding)
                .await
                .map_err(|e| format!("writing the replacement: {e}"))?;
            written = Some(new_id);
            let mut all = vec![p.keeper_id];
            all.extend(&p.retire_ids);
            (new_id, all)
        }
    };

    let now = time::OffsetDateTime::now_utc();
    let mut marked = Vec::with_capacity(to_retire.len());
    for rid in &to_retire {
        if let Err(e) = state
            .store
            .mark_superseded(*rid, survivor, resolver, now)
            .await
        {
            let mut detail = format!("marking {rid} superseded: {e}");
            for m in &marked {
                if let Err(re) = state.store.restore_payload(*m).await {
                    let _ = write!(detail, "; restoring {m} ALSO failed ({re})");
                }
            }
            if let Some(w) = written {
                if let Err(he) = state.store.hard_delete_point(w).await {
                    let _ = write!(detail, "; removing replacement {w} ALSO failed ({he})");
                }
            }
            return Err(detail);
        }
        marked.push(*rid);
    }

    if written.is_some() {
        if let Ok(Some(author)) = state.store.get_author_by_id(p.author_id).await {
            mcp_metrics::record_write(
                &author.agent_name,
                author.model.as_deref(),
                mcp_metrics::KIND_KNOWLEDGE,
            );
        }
    }
    Ok(survivor)
}
