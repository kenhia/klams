//! `memory_update` MCP tool (sprint 029, #638 / review F-1.1).
//!
//! Author-fixes-own-record without ceremony: edit the text, tags, or
//! volatility of an agent-authored knowledge memory in place. The id
//! stays stable (unlike `memory_supersede`, which is for "this belief
//! was wrong" and leaves a trail); this verb is for typos, small
//! amendments, and volatility declarations, where minting a new id and
//! a pointer would be pure ceremony.
//!
//! Text changes re-embed and re-hash. Authorship never changes: a
//! `manage`-tier caller editing another author's memory edits *their*
//! record, it does not adopt it.
//!
//! Same target restriction as supersede: agent-authored knowledge only
//! (scanner chunks update via re-scan; facts amend; events append).

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    maintenance, metrics as mcp_metrics, projection,
    tools::memory_add::{enforce_embed_size, sha256_hex, VolatilityArg},
    tools::memory_delete::{authorize_curation, DeleteCaller},
    tools::McpState,
};
use klams_store::Store;
use klams_types::{PublicAuthorRef, PublicMemory, Source};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryUpdateArgs {
    /// The memory to edit (agent-authored knowledge only).
    #[schemars(with = "String")]
    pub id: Uuid,
    /// New text. Omitted = unchanged. Re-embeds when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
    /// New tag set (replaces the old set). Omitted = unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tags: Option<Vec<String>>,
    /// New volatility declaration. Omitted = unchanged.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub volatility: Option<VolatilityArg>,
}

/// Execute `memory_update`. Returns the updated memory.
///
/// # Errors
/// [`ErrorEnvelope`] for `MAINTENANCE_WINDOW_ACTIVE`,
/// `MISSING_AUTHOR_ID`, `SCHEMA_VALIDATION_FAILED` (no change
/// requested, or empty text), `NOT_FOUND` (unknown id, or
/// superseded/deleted), `NOT_AGENT_AUTHORED`, `INSUFFICIENT_SCOPE`,
/// `PAYLOAD_TOO_LARGE`, `EMBEDDING_UNAVAILABLE`, or `INTERNAL_ERROR`.
#[allow(clippy::too_many_lines)]
pub async fn run<S: Store>(
    state: &McpState<S>,
    args: MemoryUpdateArgs,
    caller: Option<&DeleteCaller>,
) -> Result<PublicMemory, ErrorEnvelope> {
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
    if args.text.is_none() && args.tags.is_none() && args.volatility.is_none() {
        return Err(envelope(
            errors::SCHEMA_VALIDATION_FAILED,
            "nothing to update: pass at least one of text, tags, volatility",
        ));
    }
    let new_text = match args.text {
        Some(t) => {
            let t = t.trim().to_string();
            if t.is_empty() {
                return Err(envelope(
                    errors::SCHEMA_VALIDATION_FAILED,
                    "text must be non-empty when present (use memory_delete \
                     to remove a memory)",
                ));
            }
            Some(t)
        }
        None => None,
    };

    let mut item = state
        .store
        .get_knowledge(args.id)
        .await
        .map_err(|e| errors::from_store_error("get_knowledge", &e))?
        .ok_or_else(|| {
            envelope(
                errors::NOT_FOUND,
                format!(
                    "no knowledge memory with id {}; only knowledge can be \
                     updated (facts amend, events append)",
                    args.id
                ),
            )
        })?;
    if state
        .store
        .point_is_soft_deleted(args.id)
        .await
        .ok()
        .flatten()
        .unwrap_or(false)
    {
        return Err(envelope(
            errors::NOT_FOUND,
            format!(
                "memory {} is superseded or deleted; update its replacement, \
                 or restore it first",
                args.id
            ),
        ));
    }
    if item.source != Source::AgentProposal {
        return Err(envelope(
            errors::NOT_AGENT_AUTHORED,
            "this is scanner-ingested knowledge; it is derived from a file, \
             so fix the file and let the re-scan update the store. \
             memory_update is for agent-authored memories.",
        ));
    }

    // Ownership: the record's author, not the caller, stays on the
    // point — resolve both.
    let owner = state
        .store
        .knowledge_authors_by_ids(&[args.id])
        .await
        .map_err(|e| errors::from_store_error("knowledge_authors_by_ids", &e))?
        .get(&args.id)
        .copied();
    authorize_curation(caller, owner, "updating it")?;
    let owner_id = owner.unwrap_or(caller.author_id);

    // The caller's identity is what gets metric-attributed and
    // last-seen-touched; the projection shows the record's author.
    let caller_author = state
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
    let _ = state
        .store
        .touch_author_last_seen_at(caller_author.id)
        .await;

    // Apply the edit to the item.
    let text_changed = new_text.is_some();
    if let Some(t) = new_text {
        item.content_hash = sha256_hex(&t);
        item.text = t;
    }
    if let Some(tags) = args.tags {
        item.tags = tags;
    }
    if let Some(v) = args.volatility {
        item.volatility = Some(v.as_str().to_string());
    }
    item.updated_at = time::OffsetDateTime::now_utc();

    // Vector: re-embed on text change, else keep the stored one.
    let embedding = if text_changed {
        enforce_embed_size(
            state,
            caller_author.id,
            &caller_author.agent_name,
            &item.text,
        )
        .await?;
        state
            .store
            .embed_document(&item.text)
            .await
            .map_err(|e| errors::from_store_error("embedding", &e))?
    } else {
        state
            .store
            .get_point_vector(item.id)
            .await
            .map_err(|e| errors::from_store_error("get_point_vector", &e))?
            .ok_or_else(|| {
                envelope(
                    errors::INTERNAL_ERROR,
                    format!("memory {} has no stored vector", item.id),
                )
            })?
    };

    state
        .store
        .upsert_knowledge_item(&item, owner_id, embedding)
        .await
        .map_err(|e| errors::from_store_error("qdrant", &e))?;

    mcp_metrics::record_write(
        &caller_author.agent_name,
        caller_author.model.as_deref(),
        mcp_metrics::KIND_KNOWLEDGE,
    );

    // Project with the *record's* author, fetched fresh; fall back to
    // the unknown ref if the row vanished (legacy points).
    let author_ref = match state.store.get_author_by_id(owner_id).await {
        Ok(Some(a)) => PublicAuthorRef::from_record(&a),
        _ => PublicAuthorRef::unknown(),
    };
    Ok(projection::project_knowledge(&item, author_ref))
}
