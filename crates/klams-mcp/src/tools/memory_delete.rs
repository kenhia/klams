//! `memory_delete` MCP tool (sprint 007 T046, US4).
//!
//! Soft-delete a fact or knowledge item by id. Idempotent (FR-014):
//! a second call on an already-soft-deleted item returns success
//! without rewriting `deleted_at` / `deleted_by_author_id`. Events
//! are append-only (FR-015) — return `EVENTS_NOT_DELETABLE`.
//!
//! Sprint 025 (#633) — **ownership is enforced here.** Before this
//! sprint `author_id` was a required-but-unvalidated argument: any
//! authenticated caller could pass any well-formed author id (minting
//! one via `register_author` if needed) and delete anything in the
//! store. The acting identity is now always the bearer-bound author,
//! and deleting somebody else's memory requires [`Scope::Manage`].

use crate::{
    errors::{self, envelope, ErrorEnvelope},
    maintenance, metrics as mcp_metrics,
    tools::McpState,
};
use chrono::Utc;
use klams_types::Scope;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct MemoryDeleteArgs {
    /// Optional since sprint 025 (#633): the delete is attributed to the
    /// author bound to your bearer token. If supplied it must *equal*
    /// that author — passing somebody else's id is the backdoor this
    /// sprint closed, not a way to act as them.
    #[serde(default)]
    #[schemars(with = "Option<String>")]
    pub author_id: Option<Uuid>,
    #[schemars(with = "String")]
    pub id: Uuid,
}

/// The authenticated caller, as resolved from the bearer token by
/// `require_bearer` and forwarded through the rmcp request extensions.
#[derive(Debug, Clone)]
pub struct DeleteCaller {
    pub author_id: Uuid,
    pub scopes: Vec<Scope>,
}

/// Decide whether `caller` may curate a memory owned by `owner` —
/// the shared ownership gate for `memory_delete`, `memory_supersede`,
/// and `memory_update` (sprint 029, #638: supersession *is* a delete
/// plus a write, and update is a rewrite, so all three ride one
/// authorization decision, per F-1.1). Pure so the policy is testable
/// without a live backend — the surrounding `run`s are
/// integration-tested against docker.
///
/// `owner == None` means the record carries no `author_id` (legacy
/// knowledge points predating attribution). Those are treated as
/// *not* owned by the caller, so curating them requires `Manage` —
/// the conservative reading.
///
/// # Errors
/// `INSUFFICIENT_SCOPE` when the caller neither owns the memory nor
/// holds [`Scope::Manage`].
pub(crate) fn authorize_curation(
    caller: &DeleteCaller,
    owner: Option<Uuid>,
    verb: &str,
) -> Result<(), ErrorEnvelope> {
    if owner == Some(caller.author_id) {
        return Ok(());
    }
    if caller.scopes.iter().any(|s| s.satisfies(Scope::Manage)) {
        return Ok(());
    }
    Err(envelope(
        errors::INSUFFICIENT_SCOPE,
        format!(
            "this memory belongs to another author; {verb} requires the \
             `manage` scope. Your token can act on memories it wrote itself."
        ),
    ))
}

/// Resolve the identity the delete acts as. Always the bearer-bound
/// author; an explicit `author_id` may only confirm it.
///
/// # Errors
/// `MISSING_AUTHOR_ID` when the request carries no bound author, and
/// `INSUFFICIENT_SCOPE` when `args.author_id` names somebody else.
fn acting_author(
    caller: Option<&DeleteCaller>,
    requested: Option<Uuid>,
) -> Result<&DeleteCaller, ErrorEnvelope> {
    let caller = caller.ok_or_else(|| {
        envelope(
            errors::MISSING_AUTHOR_ID,
            "no author is bound to this bearer token; add `agent_name` to \
             its [[auth.tokens]] grant",
        )
    })?;
    match requested {
        Some(id) if !id.is_nil() && id != caller.author_id => Err(envelope(
            errors::INSUFFICIENT_SCOPE,
            "author_id must match the author bound to your bearer token; \
             deletes act as your own identity and cannot be performed on \
             behalf of another author",
        )),
        _ => Ok(caller),
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MemoryDeleteOutput {
    #[serde(with = "uuid::serde::simple")]
    pub id: Uuid,
    pub deleted_at: chrono::DateTime<Utc>,
}

/// Execute `memory_delete` as `caller`'s bound author.
///
/// # Errors
/// Returns an [`ErrorEnvelope`] for `MAINTENANCE_WINDOW_ACTIVE`,
/// `MISSING_AUTHOR_ID`, `UNKNOWN_AUTHOR_ID`, `INSUFFICIENT_SCOPE`
/// (sprint 025 — not the owner and no `manage` scope), `NOT_FOUND`,
/// `EVENTS_NOT_DELETABLE`, or `INTERNAL_ERROR`.
pub async fn run(
    state: &McpState,
    args: MemoryDeleteArgs,
    caller: Option<&DeleteCaller>,
) -> Result<MemoryDeleteOutput, ErrorEnvelope> {
    if let Some(env) = maintenance::check(&state.maintenance) {
        return Err(env);
    }
    let caller = acting_author(caller, args.author_id)?;
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

    if let Some(out) = delete_fact(state, args.id, caller, &author).await? {
        return Ok(out);
    }
    if let Some(out) = delete_knowledge(state, args.id, caller, &author).await? {
        return Ok(out);
    }

    Err(envelope(
        errors::NOT_FOUND,
        format!("memory {} not found", args.id),
    ))
}

/// Soft-delete `id` if it is a fact. `Ok(None)` means "not a fact" —
/// the caller falls through to the knowledge store.
async fn delete_fact(
    state: &McpState,
    id: Uuid,
    caller: &DeleteCaller,
    author: &klams_types::AuthorRecord,
) -> Result<Option<MemoryDeleteOutput>, ErrorEnvelope> {
    // `fact_owner` doubles as the existence probe: `facts.author_id` is
    // NOT NULL, so `Some(_)` means the row is there.
    let Some(owner) = state
        .store
        .postgres
        .fact_owner(id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("fact_owner: {e}")))?
    else {
        return Ok(None);
    };
    authorize_curation(caller, Some(owner), "deleting it")?;
    // soft_delete_fact returns false if already soft-deleted; we treat
    // that as a success per FR-014 idempotency.
    let _ = state
        .store
        .postgres
        .soft_delete_fact(id, author.id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("soft_delete_fact: {e}")))?;
    Ok(Some(deleted(id, author)))
}

/// Soft-delete `id` if it is a knowledge point. `Ok(None)` means "no
/// such point".
async fn delete_knowledge(
    state: &McpState,
    id: Uuid,
    caller: &DeleteCaller,
    author: &klams_types::AuthorRecord,
) -> Result<Option<MemoryDeleteOutput>, ErrorEnvelope> {
    if !state
        .store
        .qdrant
        .point_exists_any(id)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("point_exists_any: {e}")))?
    {
        return Ok(None);
    }
    let owner = state
        .store
        .qdrant
        .knowledge_authors_by_ids(&[id])
        .await
        .map_err(|e| {
            envelope(
                errors::INTERNAL_ERROR,
                format!("knowledge_authors_by_ids: {e}"),
            )
        })?
        .get(&id)
        .copied();
    authorize_curation(caller, owner, "deleting it")?;
    state
        .store
        .qdrant
        .soft_delete_payload(id, author.id, time::OffsetDateTime::now_utc())
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("soft_delete_payload: {e}")))?;
    Ok(Some(deleted(id, author)))
}

/// Record the delete metric and build the success output.
fn deleted(id: Uuid, author: &klams_types::AuthorRecord) -> MemoryDeleteOutput {
    mcp_metrics::record_delete(
        &author.agent_name,
        author.model.as_deref(),
        mcp_metrics::MODE_SOFT,
    );
    MemoryDeleteOutput {
        id,
        deleted_at: Utc::now(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caller(scopes: &[Scope]) -> DeleteCaller {
        DeleteCaller {
            author_id: Uuid::from_u128(1),
            scopes: scopes.to_vec(),
        }
    }

    const OTHER: Uuid = Uuid::from_u128(2);

    #[test]
    fn owner_may_delete_own_memory_with_write_only() {
        let c = caller(&[Scope::Read, Scope::Write]);
        authorize_curation(&c, Some(c.author_id), "deleting it")
            .expect("self-management needs no manage scope");
    }

    /// The sprint's headline invariant: this is the exact call that
    /// succeeded on 2026-07-25 and should not have.
    #[test]
    fn write_only_caller_cannot_delete_another_authors_memory() {
        let c = caller(&[Scope::Read, Scope::Write]);
        let err = authorize_curation(&c, Some(OTHER), "deleting it").expect_err("must be refused");
        assert_eq!(err.meta.error_code, errors::INSUFFICIENT_SCOPE);
    }

    #[test]
    fn manage_scope_permits_cross_author_delete() {
        let c = caller(&[Scope::Read, Scope::Write, Scope::Manage]);
        authorize_curation(&c, Some(OTHER), "deleting it")
            .expect("manage is the cross-author tier");
    }

    /// `admin` is a peer scope, not a superset — an admin-only token
    /// does not inherit cross-author curation.
    #[test]
    fn admin_alone_does_not_confer_cross_author_delete() {
        let c = caller(&[Scope::Read, Scope::Write, Scope::Admin]);
        assert!(authorize_curation(&c, Some(OTHER), "deleting it").is_err());
    }

    /// Legacy knowledge points with no `author_id` in their payload are
    /// nobody's to self-manage; curating them needs `manage`.
    #[test]
    fn unowned_record_requires_manage() {
        assert!(authorize_curation(&caller(&[Scope::Write]), None, "deleting it").is_err());
        authorize_curation(&caller(&[Scope::Write, Scope::Manage]), None, "deleting it")
            .expect("manage curates");
    }

    #[test]
    fn omitted_author_id_resolves_to_the_bound_author() {
        let c = caller(&[Scope::Write]);
        let resolved = acting_author(Some(&c), None).expect("omitted is the documented path");
        assert_eq!(resolved.author_id, c.author_id);
    }

    #[test]
    fn explicit_matching_author_id_is_accepted() {
        let c = caller(&[Scope::Write]);
        acting_author(Some(&c), Some(c.author_id)).expect("confirming your own id is fine");
    }

    /// Closes the `register_author` backdoor: minting a fresh identity
    /// and passing its id no longer changes who the delete acts as.
    #[test]
    fn foreign_author_id_is_refused_even_with_manage() {
        let c = caller(&[Scope::Read, Scope::Write, Scope::Manage]);
        let err = acting_author(Some(&c), Some(OTHER)).expect_err("cannot act as another author");
        assert_eq!(err.meta.error_code, errors::INSUFFICIENT_SCOPE);
    }

    #[test]
    fn nil_author_id_is_treated_as_omitted() {
        let c = caller(&[Scope::Write]);
        acting_author(Some(&c), Some(Uuid::nil())).expect("nil means 'use my binding'");
    }

    #[test]
    fn unbound_token_cannot_delete() {
        let err = acting_author(None, None).expect_err("no bound author");
        assert_eq!(err.meta.error_code, errors::MISSING_AUTHOR_ID);
    }
}
