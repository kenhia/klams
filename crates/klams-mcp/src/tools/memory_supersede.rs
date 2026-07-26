//! `memory_supersede` MCP tool (sprint 029, #638 / review F-1.1).
//!
//! The correction verb agents actually need: "this is now wrong, here
//! is the replacement" — one call, not a delete-then-add an agent has
//! to sequence itself. Writes the replacement memory (carrying
//! `supersedes: <old>`), then marks the old one superseded: the
//! soft-delete payload pair plus a `superseded_by: <new>` pointer, so
//! every existing retrieval filter hides it while the admin surface
//! can still inspect and restore it.
//!
//! Only agent-authored knowledge can be superseded. Scanner chunks are
//! derived data (the file is the source of truth, re-scan is their
//! update path); facts have versioned amendments; events are
//! append-only.
//!
//! Authorization is the delete capability (a supersede *is* a delete
//! plus a write): the caller must own the old memory or hold
//! [`Scope::Manage`] — see `authorize_curation`.
//!
//! Atomicity: Qdrant has no transactions, so the write pair is ordered
//! new-first. If marking the old point fails, the just-written
//! replacement is removed again (best-effort) and the call errors —
//! the store is never left claiming both memories are current without
//! telling the caller.

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    maintenance, metrics as mcp_metrics, projection,
    tools::memory_add::{enforce_embed_size, sha256_hex, MemoryAddOutput, VolatilityArg},
    tools::memory_delete::{authorize_curation, DeleteCaller},
    tools::McpState,
};
use klams_types::{IndexKnowledge, PublicAuthorRef, Source};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemorySupersedeArgs {
    /// The memory being replaced (agent-authored knowledge only).
    #[schemars(with = "String")]
    pub id: Uuid,
    /// The replacement text. Embedded and written as a new memory
    /// authored by you.
    pub text: String,
    /// Tags for the replacement. Omitted = the old memory's tags are
    /// inherited.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// Volatility declaration for the replacement. Omitted = inherited
    /// from the old memory.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volatility: Option<VolatilityArg>,
}

/// Execute `memory_supersede`: write the replacement, hide the old
/// memory behind the soft-delete filter with a `superseded_by` pointer.
/// Returns the replacement memory (its projection carries
/// `supersedes`).
///
/// # Errors
/// [`ErrorEnvelope`] for `MAINTENANCE_WINDOW_ACTIVE`,
/// `MISSING_AUTHOR_ID`, `NOT_FOUND` (unknown id, or already
/// superseded/deleted), `NOT_AGENT_AUTHORED`, `INSUFFICIENT_SCOPE`,
/// `PAYLOAD_TOO_LARGE`, `EMBEDDING_UNAVAILABLE`, or `INTERNAL_ERROR`.
#[allow(clippy::too_many_lines)]
pub async fn run(
    state: &McpState,
    args: MemorySupersedeArgs,
    caller: Option<&DeleteCaller>,
) -> Result<MemoryAddOutput, ErrorEnvelope> {
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
    let text = args.text.trim().to_string();
    if text.is_empty() {
        return Err(envelope(
            errors::SCHEMA_VALIDATION_FAILED,
            "replacement text must be non-empty",
        ));
    }

    // The target must exist, be live, and be agent-authored knowledge.
    let old = state
        .store
        .qdrant
        .get_knowledge(args.id)
        .await
        .map_err(|e| errors::from_store_error("get_knowledge", &e))?
        .ok_or_else(|| {
            envelope(
                errors::NOT_FOUND,
                format!(
                    "no knowledge memory with id {}; only knowledge can be \
                     superseded (facts amend, events append)",
                    args.id
                ),
            )
        })?;
    if state
        .store
        .qdrant
        .point_is_soft_deleted(args.id)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
    {
        return Err(envelope(
            errors::NOT_FOUND,
            format!(
                "memory {} is already superseded or deleted; supersede its \
                 replacement instead (see `superseded_by` via the admin \
                 surface)",
                args.id
            ),
        ));
    }
    if old.source != Source::AgentProposal {
        return Err(envelope(
            errors::NOT_AGENT_AUTHORED,
            "this is scanner-ingested knowledge; it is derived from a file, \
             so fix the file (or remove it from scanning) and let the \
             re-scan update the store. Supersession is for agent-authored \
             memories.",
        ));
    }

    let owner = state
        .store
        .qdrant
        .knowledge_authors_by_ids(&[args.id])
        .await
        .map_err(|e| errors::from_store_error("knowledge_authors_by_ids", &e))?
        .get(&args.id)
        .copied();
    authorize_curation(caller, owner, "superseding it")?;

    let author = state
        .store
        .postgres
        .get_author_by_id(caller.author_id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("get_author_by_id: {e}")))?
        .ok_or_else(|| {
            envelope(
                errors::UNKNOWN_AUTHOR_ID,
                format!("author_id {} not found", caller.author_id),
            )
        })?;
    let _ = state
        .store
        .postgres
        .touch_author_last_seen_at(author.id)
        .await;

    enforce_embed_size(state, author.id, &author.agent_name, &text).await?;

    // Write the replacement first: worst case on a mid-flight failure
    // is a duplicate-ish new memory, never a hidden old one with no
    // replacement.
    let new_id = Uuid::now_v7();
    let req = IndexKnowledge {
        id: new_id,
        content_hash: sha256_hex(&text),
        text,
        source: Source::AgentProposal,
        tags: args.tags.unwrap_or_else(|| old.tags.clone()),
        repo: old.repo.clone(),
        file: old.file.clone(),
        machine: None,
        author_id: author.id,
        chunk_index: None,
        language: None,
        heading_path: None,
        symbols: Vec::new(),
        volatility: args
            .volatility
            .map(|v| v.as_str().to_string())
            .or_else(|| old.volatility.clone()),
        supersedes: Some(old.id),
    };
    let embedding = state
        .store
        .embedder
        .embed(&req.text)
        .await
        .map_err(|e| errors::from_store_error("embedding", &e))?;
    let item = state
        .store
        .qdrant
        .index_knowledge(req, embedding)
        .await
        .map_err(|e| errors::from_store_error("qdrant", &e))?;

    if let Err(e) = state
        .store
        .qdrant
        .mark_superseded(old.id, new_id, author.id, time::OffsetDateTime::now_utc())
        .await
    {
        // Undo the replacement so the store is not left with both
        // memories live. Best-effort: if the cleanup also fails, say
        // exactly what state things are in.
        let cleanup = state.store.qdrant.hard_delete_point(new_id).await;
        let detail = match cleanup {
            Ok(()) => format!(
                "marking {} superseded failed ({e}); the replacement was \
                 rolled back — nothing changed, retry the call",
                old.id
            ),
            Err(c) => format!(
                "marking {} superseded failed ({e}) AND rolling back the \
                 replacement {new_id} failed ({c}); both memories are \
                 currently live — retry `memory_supersede`, or \
                 `memory_delete` one of them",
                old.id
            ),
        };
        return Err(envelope(errors::INTERNAL_ERROR, detail));
    }

    mcp_metrics::record_write(
        &author.agent_name,
        author.model.as_deref(),
        mcp_metrics::KIND_KNOWLEDGE,
    );
    Ok(MemoryAddOutput {
        memory: projection::project_knowledge(&item, PublicAuthorRef::from_record(&author)),
        similar_existing: Vec::new(),
    })
}
