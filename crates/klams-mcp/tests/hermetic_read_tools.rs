//! Hermetic behavioural tests for the MCP read and lifecycle tools.
//!
//! Sprint 031 (#646). Replaces the empty `#[ignore]`d stubs
//! `mcp_memory_search.rs`, `mcp_memory_related.rs`,
//! `mcp_event_search.rs`, `mcp_event_search_window.rs`,
//! `mcp_memory_delete.rs`, `mcp_memory_admin_restore.rs`,
//! `mcp_memory_admin_hard_delete.rs` and
//! `mcp_memory_admin_list_deleted.rs`.
//!
//! Scope: argument validation, error codes, ownership refusals, and the
//! projection shape — everything a caller can observe that does not
//! depend on Postgres or Qdrant semantics. RANKING is deliberately not
//! asserted here (`MemStore` returns a constant score; pretending
//! otherwise would produce a test that passes while retrieval breaks).
//! Ranking lives in `klams-service`'s docker-gated suites and in the
//! klams-mind eval.

mod support;

use klams_mcp::tools::{
    event_search::{run as event_search, EventSearchArgs},
    memory_admin_hard_delete::{run as hard_delete, MemoryAdminHardDeleteArgs},
    memory_admin_list_deleted::{run as list_deleted, MemoryAdminListDeletedArgs},
    memory_admin_restore::{run as restore, MemoryAdminRestoreArgs},
    memory_delete::{run as memory_delete, MemoryDeleteArgs},
    memory_related::{run as memory_related, MemoryRelatedArgs},
    memory_search::{run as memory_search, MemorySearchArgs},
};
use support::{curator, state, writer};
use uuid::Uuid;

fn search_args(query: &str) -> MemorySearchArgs {
    MemorySearchArgs {
        query: query.to_string(),
        kinds: None,
        tags: None,
        top_k: None,
    }
}

fn delete_args(id: Uuid) -> MemoryDeleteArgs {
    MemoryDeleteArgs {
        author_id: None,
        id,
    }
}

// -------------------------------------------------------- memory_search

#[tokio::test]
async fn memory_search_rejects_an_empty_query() {
    let (st, _) = state();
    let err = memory_search(&st, search_args("   "), None)
        .await
        .expect_err("a whitespace-only query must be refused");
    assert_eq!(err.meta.error_code, "EMPTY_QUERY", "{err:?}");
}

#[tokio::test]
async fn memory_search_rejects_an_out_of_range_top_k() {
    let (st, _) = state();
    let mut args = search_args("s031");
    args.top_k = Some(0);
    let err = memory_search(&st, args, None)
        .await
        .expect_err("top_k=0 must be refused");
    assert_eq!(err.meta.error_code, "INVALID_TOP_K", "{err:?}");
}

#[tokio::test]
async fn memory_search_projects_live_knowledge_with_its_author() {
    let (st, store) = state();
    let author = store.seed_author("s031-search");
    store.seed_knowledge(author, "s031 the scanner runs on a timer");

    let hits = memory_search(&st, search_args("scanner"), None)
        .await
        .expect("search");

    assert_eq!(hits.len(), 1, "{hits:?}");
    assert_eq!(hits[0].memory.author.agent_name, "s031-search");
}

#[tokio::test]
async fn memory_search_omits_soft_deleted_memories() {
    let (st, store) = state();
    let author = store.seed_author("s031-search-deleted");
    let id = store.seed_knowledge(author, "s031 soon to be retracted");

    memory_delete(&st, delete_args(id), Some(&writer(author)))
        .await
        .expect("owner may delete own memory");

    let hits = memory_search(&st, search_args("retracted"), None)
        .await
        .expect("search");
    assert!(
        hits.is_empty(),
        "a retracted memory must not come back in search — that is the \
         whole point of retracting it: {hits:?}"
    );
}

// ------------------------------------------------------- memory_related

#[tokio::test]
async fn memory_related_rejects_an_out_of_range_top_k() {
    let (st, _) = state();
    let err = memory_related(
        &st,
        MemoryRelatedArgs {
            id: Uuid::now_v7(),
            top_k: Some(0),
        },
    )
    .await
    .expect_err("top_k=0 must be refused");
    assert_eq!(err.meta.error_code, "INVALID_TOP_K", "{err:?}");
}

#[tokio::test]
async fn memory_related_reports_not_found_for_an_unknown_id() {
    let (st, _) = state();
    let err = memory_related(
        &st,
        MemoryRelatedArgs {
            id: Uuid::now_v7(),
            top_k: None,
        },
    )
    .await
    .expect_err("an unknown id must not return an empty list");
    assert_eq!(err.meta.error_code, "NOT_FOUND", "{err:?}");
}

#[tokio::test]
async fn memory_related_excludes_the_anchor_itself() {
    let (st, store) = state();
    let author = store.seed_author("s031-related");
    let anchor = store.seed_knowledge(author, "s031 anchor memory");
    store.seed_knowledge(author, "s031 neighbour memory");

    let out = memory_related(
        &st,
        MemoryRelatedArgs {
            id: anchor,
            top_k: None,
        },
    )
    .await
    .expect("related");

    assert!(
        !out.iter().any(|m| m.id == anchor),
        "the anchor is not its own neighbour: {out:?}"
    );
}

// --------------------------------------------------------- memory_delete

#[tokio::test]
async fn memory_delete_refuses_another_authors_memory_without_manage() {
    let (st, store) = state();
    let owner = store.seed_author("s031-owner");
    let intruder = store.seed_author("s031-intruder");
    let id = store.seed_knowledge(owner, "s031 not yours to retract");

    let err = memory_delete(&st, delete_args(id), Some(&writer(intruder)))
        .await
        .expect_err("a write-scoped peer must not delete somebody else's memory");
    assert_eq!(err.meta.error_code, "INSUFFICIENT_SCOPE", "{err:?}");
    assert!(
        err.content[0].text.contains("manage"),
        "the refusal must name the scope that would allow it, or the \
         caller cannot tell a policy from a bug: {err:?}"
    );
    assert!(
        !store.is_knowledge_soft_deleted(id),
        "a refused delete must not have taken effect"
    );
}

#[tokio::test]
async fn memory_delete_allows_a_manage_scoped_caller_across_authors() {
    let (st, store) = state();
    let owner = store.seed_author("s031-owner2");
    let curator_id = store.seed_author("s031-curator");
    let id = store.seed_knowledge(owner, "s031 curated away");

    memory_delete(&st, delete_args(id), Some(&curator(curator_id)))
        .await
        .expect("manage scope is exactly the cross-author curation tier");
    assert!(store.is_knowledge_soft_deleted(id));
}

#[tokio::test]
async fn memory_delete_is_idempotent() {
    let (st, store) = state();
    let author = store.seed_author("s031-idem");
    let id = store.seed_knowledge(author, "s031 delete me twice");
    let caller = writer(author);

    memory_delete(&st, delete_args(id), Some(&caller))
        .await
        .expect("first delete");
    memory_delete(&st, delete_args(id), Some(&caller))
        .await
        .expect("a second delete must not be an error — retries happen");
    assert!(store.is_knowledge_soft_deleted(id));
}

// ------------------------------------------------------- admin lifecycle

#[tokio::test]
async fn restore_reports_not_found_for_an_unknown_id() {
    let (st, _) = state();
    let err = restore(&st, MemoryAdminRestoreArgs { id: Uuid::now_v7() })
        .await
        .expect_err("unknown id");
    assert_eq!(err.meta.error_code, "NOT_FOUND", "{err:?}");
}

#[tokio::test]
async fn restore_refuses_a_memory_that_is_not_deleted() {
    let (st, store) = state();
    let author = store.seed_author("s031-live");
    let id = store.seed_knowledge(author, "s031 never deleted");

    let err = restore(&st, MemoryAdminRestoreArgs { id })
        .await
        .expect_err("restoring a live memory is a caller mistake, not a no-op");
    assert_eq!(err.meta.error_code, "NOT_SOFT_DELETED", "{err:?}");
}

#[tokio::test]
async fn delete_then_restore_round_trips() {
    let (st, store) = state();
    let author = store.seed_author("s031-roundtrip");
    let id = store.seed_knowledge(author, "s031 retract then reinstate");

    memory_delete(&st, delete_args(id), Some(&writer(author)))
        .await
        .expect("delete");
    assert!(store.is_knowledge_soft_deleted(id));

    restore(&st, MemoryAdminRestoreArgs { id })
        .await
        .expect("restore");
    assert!(!store.is_knowledge_soft_deleted(id));

    let hits = memory_search(&st, search_args("reinstate"), None)
        .await
        .expect("search");
    assert_eq!(hits.len(), 1, "a restored memory is findable again");
}

#[tokio::test]
async fn hard_delete_removes_the_point_entirely() {
    let (st, store) = state();
    let author = store.seed_author("s031-hard");
    let id = store.seed_knowledge(author, "s031 gone for good");

    hard_delete(&st, MemoryAdminHardDeleteArgs { id })
        .await
        .expect("hard delete");
    assert_eq!(store.knowledge_count(), 0);
}

#[tokio::test]
async fn hard_delete_reports_not_found_for_an_unknown_id() {
    let (st, _) = state();
    let err = hard_delete(&st, MemoryAdminHardDeleteArgs { id: Uuid::now_v7() })
        .await
        .expect_err("unknown id");
    assert_eq!(err.meta.error_code, "NOT_FOUND", "{err:?}");
}

#[tokio::test]
async fn list_deleted_rejects_an_out_of_range_limit() {
    let (st, _) = state();
    let err = list_deleted(
        &st,
        MemoryAdminListDeletedArgs {
            kinds: None,
            since: None,
            author_id: None,
            limit: Some(0),
            cursor: None,
        },
    )
    .await
    .expect_err("limit=0 must be refused");
    assert_eq!(err.meta.error_code, "INVALID_LIMIT", "{err:?}");
}

fn empty_event_search() -> EventSearchArgs {
    EventSearchArgs {
        author_id: None,
        category: None,
        since: None,
        until: None,
        payload_match: None,
        limit: None,
        order: None,
        cursor: None,
    }
}

// ---------------------------------------------------------- event_search

#[tokio::test]
async fn event_search_refuses_an_inverted_window() {
    let (st, _) = state();
    let err = event_search(
        &st,
        EventSearchArgs {
            since: Some("2026-07-20T00:00:00Z".into()),
            until: Some("2026-07-19T00:00:00Z".into()),
            ..empty_event_search()
        },
        Some("test-agent"),
    )
    .await
    .expect_err("since after until must be refused");
    assert_eq!(err.meta.error_code, "INVALID_WINDOW", "{err:?}");
}

#[tokio::test]
async fn event_search_refuses_a_window_past_the_configured_ceiling() {
    let (st, _) = state();
    let max_days = klams_types::ApiConfig::default().memories_max_window_days;
    let err = event_search(
        &st,
        EventSearchArgs {
            since: Some("2020-01-01T00:00:00Z".into()),
            until: Some("2026-01-01T00:00:00Z".into()),
            ..empty_event_search()
        },
        Some("test-agent"),
    )
    .await
    .expect_err("a six-year window must be refused");
    assert_eq!(err.meta.error_code, "WINDOW_TOO_LARGE", "{err:?}");
    assert_eq!(
        err.meta.window_max_days,
        Some(max_days),
        "the envelope must carry the ceiling so the agent can retry with \
         a legal window instead of guessing: {err:?}"
    );
}

#[tokio::test]
async fn event_search_rejects_an_unparseable_timestamp() {
    let (st, _) = state();
    let err = event_search(
        &st,
        EventSearchArgs {
            since: Some("last tuesday".into()),
            ..empty_event_search()
        },
        Some("test-agent"),
    )
    .await
    .expect_err("free text is not RFC3339");
    assert_eq!(err.meta.error_code, "INVALID_WINDOW", "{err:?}");
    assert!(
        err.content[0].text.contains("since"),
        "the refusal must name WHICH timestamp failed: {err:?}"
    );
}
