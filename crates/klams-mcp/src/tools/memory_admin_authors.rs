//! Sprint 025 (#636) — author lifecycle: list, remove, merge.
//!
//! Before this sprint `register_author` was the only verb the author
//! registry had. Rows accumulated forever — a live census found 44, of
//! which 8 were `klams-mind` and 6 `kyac` — and duplicates sharing an
//! `agent_name` were indistinguishable in every response an agent could
//! see. There was no way to remove even a row that owned nothing.
//!
//! These sit at [`Scope::Admin`] alongside the other `memory_admin_*`
//! tools: they rewrite attribution across the whole store, which is a
//! stronger capability than the cross-author curation `manage` grants.
//!
//! **Removal is never quietly destructive.** `remove` refuses while the
//! author owns anything (facts, events, knowledge points, or recorded
//! soft-deletes) and names the counts; reassigning is `merge`'s job, and
//! it is explicit.

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    tools::McpState,
};
use klams_store::Store;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct ListAuthorsArgs {
    /// Substring match on `agent_name`. Omit to list every author.
    #[serde(default)]
    pub agent_name: Option<String>,
    /// Only authors owning nothing — the removal candidates.
    #[serde(default)]
    pub only_empty: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct AuthorSummary {
    pub author_id: Uuid,
    pub agent_name: String,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
    pub facts: i64,
    pub events: i64,
    pub knowledge: u64,
    pub soft_deletes_authored: i64,
    /// True when this author owns nothing and can be removed outright.
    pub removable: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct ListAuthorsOutput {
    pub authors: Vec<AuthorSummary>,
    /// Names held by more than one author row — the merge candidates.
    pub duplicate_agent_names: Vec<String>,
}

/// Execute `memory_admin_list_authors`.
///
/// # Errors
/// `INTERNAL_ERROR` if either store cannot be queried.
pub async fn list<S: Store>(
    state: &McpState<S>,
    args: ListAuthorsArgs,
) -> Result<ListAuthorsOutput, ErrorEnvelope> {
    let rows = state
        .store
        .list_all_authors()
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("list_all_authors: {e}")))?;

    let needle = args.agent_name.as_deref().map(str::to_ascii_lowercase);
    let mut authors = Vec::new();
    for a in rows {
        if let Some(n) = &needle {
            if !a.agent_name.to_ascii_lowercase().contains(n.as_str()) {
                continue;
            }
        }
        let summary = summarize(state, &a).await?;
        if args.only_empty && !summary.removable {
            continue;
        }
        authors.push(summary);
    }

    let mut seen: std::collections::HashMap<&str, usize> = std::collections::HashMap::new();
    for a in &authors {
        *seen.entry(a.agent_name.as_str()).or_default() += 1;
    }
    let mut duplicate_agent_names: Vec<String> = seen
        .into_iter()
        .filter(|(_, n)| *n > 1)
        .map(|(name, _)| name.to_string())
        .collect();
    duplicate_agent_names.sort();

    Ok(ListAuthorsOutput {
        authors,
        duplicate_agent_names,
    })
}

async fn summarize<S: Store>(
    state: &McpState<S>,
    a: &klams_types::AuthorRecord,
) -> Result<AuthorSummary, ErrorEnvelope> {
    let (facts, events, soft_deletes_authored) = state
        .store
        .count_author_rows(a.id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("count_author_rows: {e}")))?;
    let knowledge = state
        .store
        .count_knowledge_by_author_any(a.id)
        .await
        .map_err(|e| {
            envelope(
                errors::INTERNAL_ERROR,
                format!("count_knowledge_by_author_any: {e}"),
            )
        })?;
    Ok(AuthorSummary {
        author_id: a.id,
        agent_name: a.agent_name.clone(),
        created_at: a.created_at,
        last_seen_at: a.last_seen_at,
        facts,
        events,
        knowledge,
        soft_deletes_authored,
        removable: facts == 0 && events == 0 && knowledge == 0 && soft_deletes_authored == 0,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct RemoveAuthorArgs {
    #[schemars(with = "String")]
    pub author_id: Uuid,
}

#[derive(Debug, Clone, Serialize)]
pub struct RemoveAuthorOutput {
    pub author_id: Uuid,
    pub agent_name: String,
    pub removed: bool,
}

/// Execute `memory_admin_remove_author`.
///
/// # Errors
/// `NOT_FOUND` for an unknown id, `INSUFFICIENT_SCOPE`-adjacent
/// `AUTHOR_HAS_MEMORIES` when the author still owns something, or
/// `INTERNAL_ERROR`.
pub async fn remove<S: Store>(
    state: &McpState<S>,
    args: RemoveAuthorArgs,
) -> Result<RemoveAuthorOutput, ErrorEnvelope> {
    let author = load_author(state, args.author_id).await?;
    let summary = summarize(state, &author).await?;
    if !summary.removable {
        return Err(envelope(
            errors::AUTHOR_HAS_MEMORIES,
            format!(
                "author {} ({}) still owns {} fact(s), {} event(s), {} knowledge point(s) \
                 and {} recorded soft-delete(s); merge it into another author first",
                author.id,
                author.agent_name,
                summary.facts,
                summary.events,
                summary.knowledge,
                summary.soft_deletes_authored,
            ),
        ));
    }
    let removed = state
        .store
        .delete_author(author.id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("delete_author: {e}")))?;
    Ok(RemoveAuthorOutput {
        author_id: author.id,
        agent_name: author.agent_name,
        removed,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MergeAuthorsArgs {
    /// The author to drain and remove.
    #[schemars(with = "String")]
    pub from_author_id: Uuid,
    /// The author that inherits everything.
    #[schemars(with = "String")]
    pub into_author_id: Uuid,
}

#[derive(Debug, Clone, Serialize)]
pub struct MergeAuthorsOutput {
    pub from_author_id: Uuid,
    pub into_author_id: Uuid,
    pub agent_name: String,
    pub facts_moved: u64,
    pub events_moved: u64,
    pub knowledge_moved: u64,
    pub soft_deletes_moved: u64,
}

/// Execute `memory_admin_merge_authors`: reassign everything `from`
/// owns to `into`, then drop `from`.
///
/// Ordering matters and is deliberate. Qdrant has no transactions, so
/// the knowledge repoint runs **first**; if it fails, Postgres is
/// untouched and the merge can simply be retried. The Postgres half —
/// facts, events, soft-delete attribution, and the author row itself —
/// is one transaction.
///
/// # Errors
/// `NOT_FOUND` for either unknown id, `INVALID_KIND` when both ids are
/// the same, or `INTERNAL_ERROR`.
pub async fn merge<S: Store>(
    state: &McpState<S>,
    args: MergeAuthorsArgs,
) -> Result<MergeAuthorsOutput, ErrorEnvelope> {
    if args.from_author_id == args.into_author_id {
        return Err(envelope(
            errors::INVALID_KIND,
            "from_author_id and into_author_id must differ",
        ));
    }
    let from = load_author(state, args.from_author_id).await?;
    // Load the target too: merging into a nonexistent author would
    // silently orphan everything the source owned.
    let _into = load_author(state, args.into_author_id).await?;

    let knowledge_moved = state
        .store
        .reassign_knowledge_author(from.id, args.into_author_id)
        .await
        .map_err(|e| {
            envelope(
                errors::INTERNAL_ERROR,
                format!("reassign_knowledge_author: {e} (nothing in Postgres changed; retry)"),
            )
        })?;
    let (facts_moved, events_moved, soft_deletes_moved) = state
        .store
        .merge_author_rows(from.id, args.into_author_id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("merge_author_rows: {e}")))?;

    Ok(MergeAuthorsOutput {
        from_author_id: from.id,
        into_author_id: args.into_author_id,
        agent_name: from.agent_name,
        facts_moved,
        events_moved,
        knowledge_moved,
        soft_deletes_moved,
    })
}

async fn load_author<S: Store>(
    state: &McpState<S>,
    id: Uuid,
) -> Result<klams_types::AuthorRecord, ErrorEnvelope> {
    state
        .store
        .get_author_by_id(id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("get_author_by_id: {e}")))?
        .ok_or_else(|| envelope(errors::NOT_FOUND, format!("author {id} not found")))
}
