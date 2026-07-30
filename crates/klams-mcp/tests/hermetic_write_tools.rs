//! Hermetic behavioural tests for the MCP write tools.
//!
//! Sprint 031 (#646). Replaces the empty `#[ignore]`d stubs
//! `mcp_register_author.rs`, `mcp_memory_add_fact.rs`,
//! `mcp_memory_add_knowledge.rs` and `mcp_memory_append_event.rs`,
//! whose bodies were comments pointing at klams-service integration
//! tests. Those integration tests still exist and still run — what was
//! missing is coverage that runs on `cargo test` with no stack at all,
//! so a broken tool surfaces in seconds rather than only under docker.
//!
//! Everything here goes through the real handler; only the store is
//! substituted (see `support::MemStore`).

mod support;

use klams_mcp::tools::{
    memory_add::{run as memory_add, FactTypeArg, MemoryAddArgs},
    memory_append_event::{run as append_event, MemoryAppendEventArgs},
    register_author::run as register_author,
};
use support::{author_input as input, state};
use uuid::Uuid;

// ---------------------------------------------------------------- authors

#[tokio::test]
async fn register_author_rejects_an_invalid_agent_name() {
    let (st, _) = state();
    // `validate_agent_name` requires lowercase kebab; an uppercase name
    // is the case the config validator also refuses.
    let err = register_author(&st, input("Alice"))
        .await
        .expect_err("uppercase agent_name must be refused");
    assert_eq!(err.meta.error_code, "INVALID_AGENT_NAME", "{err:?}");
}

#[tokio::test]
async fn register_author_returns_the_existing_row_for_a_known_name() {
    let (st, store) = state();
    let first = register_author(&st, input("s031-agent"))
        .await
        .expect("first register");
    let second = register_author(&st, input("s031-agent"))
        .await
        .expect("second register");

    assert_eq!(
        first.author_id, second.author_id,
        "re-registering a name must return the SAME author, not mint a \
         second identity — attribution and ownership both key on this id"
    );
    assert_eq!(store.list_all_authors_len(), 1);
}

#[tokio::test]
async fn register_author_touches_last_seen_on_a_repeat_call() {
    let (st, store) = state();
    let out = register_author(&st, input("s031-touch"))
        .await
        .expect("first");
    let before = store.touch_count(out.author_id);
    register_author(&st, input("s031-touch"))
        .await
        .expect("second");
    assert!(
        store.touch_count(out.author_id) > before,
        "FR-005: a repeat registration is activity and must bump \
         last_seen_at, or the authors view reports live agents as stale"
    );
}

// ------------------------------------------------------------ memory_add

#[tokio::test]
async fn memory_add_rejects_an_unknown_author_id() {
    let (st, _) = state();
    let err = memory_add(
        &st,
        MemoryAddArgs::fact(
            Uuid::now_v7(),
            FactTypeArg::EnvFact,
            serde_json::json!({"key": "S031_X", "value": "v"}),
        ),
    )
    .await
    .expect_err("an unregistered author must not be able to write");
    assert_eq!(err.meta.error_code, "UNKNOWN_AUTHOR_ID", "{err:?}");
}

#[tokio::test]
async fn memory_add_rejects_a_nil_author_id() {
    let (st, _) = state();
    let err = memory_add(
        &st,
        MemoryAddArgs::fact(
            Uuid::nil(),
            FactTypeArg::EnvFact,
            serde_json::json!({"key": "S031_X", "value": "v"}),
        ),
    )
    .await
    .expect_err("nil author_id must be refused");
    assert_eq!(err.meta.error_code, "MISSING_AUTHOR_ID", "{err:?}");
}

#[tokio::test]
async fn memory_add_fact_persists_and_attributes_to_the_author() {
    let (st, store) = state();
    let author = store.seed_author("s031-writer");

    let out = memory_add(
        &st,
        MemoryAddArgs::fact(
            author,
            FactTypeArg::EnvFact,
            serde_json::json!({"key": "S031_OK", "value": "v"}),
        ),
    )
    .await
    .expect("valid fact");

    assert_eq!(store.fact_count(), 1);
    assert_eq!(
        out.author.agent_name, "s031-writer",
        "the projection must name the writer: {out:?}"
    );
    assert!(store.touch_count(author) > 0, "FR-005: a write is activity");
}

#[tokio::test]
async fn memory_add_fact_runs_the_validator_registry() {
    // Sprint 031 (#645) wired REST's `ValidatorRegistry` into this path.
    // Hermetic coverage matters here specifically: the check is pure
    // and needs no backend, so there is no excuse for it to be
    // docker-gated — and a regression that silently drops validation is
    // exactly the class that went unnoticed for 24 sprints.
    let (st, store) = state();
    let author = store.seed_author("s031-validator");

    let err = memory_add(
        &st,
        MemoryAddArgs::fact(author, FactTypeArg::EnvFact, serde_json::json!({})),
    )
    .await
    .expect_err("EnvFact with no key/value must be refused");
    assert_eq!(err.meta.error_code, "SCHEMA_VALIDATION_FAILED", "{err:?}");
    assert_eq!(store.fact_count(), 0, "nothing may be stored on rejection");

    let err = memory_add(
        &st,
        MemoryAddArgs::fact(
            author,
            FactTypeArg::EnvFact,
            serde_json::json!({"key": "lowercase", "value": "v"}),
        ),
    )
    .await
    .expect_err("EnvFact key must match ^[A-Z][A-Z0-9_]*$");
    assert!(
        err.content[0].text.contains("payload.key"),
        "the rejection must name the field: {err:?}"
    );
}

#[tokio::test]
async fn memory_add_knowledge_stores_the_normalized_text() {
    let (st, store) = state();
    let author = store.seed_author("s031-knowledge");

    let out = memory_add(
        &st,
        MemoryAddArgs::knowledge(author, "s031 hermetic knowledge write  \n\n"),
    )
    .await
    .expect("knowledge write");

    assert_eq!(store.knowledge_count(), 1);
    let stored = store.knowledge_text(out.id).expect("point stored");
    assert_eq!(
        stored, "s031 hermetic knowledge write",
        "the NORMALIZED text is what gets stored and hashed (#645); \
         storing raw input is how the same content got two hashes"
    );

    // Leading indentation survives on purpose (sprint 022 #321): code
    // chunks must match the file they came from. Only trailing
    // whitespace and blank-line runs are cleaned.
    let indented = memory_add(
        &st,
        MemoryAddArgs::knowledge(author, "    fn main() {}   \n"),
    )
    .await
    .expect("indented write");
    assert_eq!(
        store.knowledge_text(indented.id).as_deref(),
        Some("    fn main() {}")
    );
}

#[tokio::test]
async fn memory_add_knowledge_dedupes_identical_content() {
    let (st, store) = state();
    let author = store.seed_author("s031-dedupe");
    let text = "s031 the reranker listens on port 7071";

    let first = memory_add(&st, MemoryAddArgs::knowledge(author, text))
        .await
        .expect("first");
    let second = memory_add(&st, MemoryAddArgs::knowledge(author, format!("{text}\n")))
        .await
        .expect("second");

    assert_eq!(second.id, first.id, "identical content is one point");
    assert_eq!(store.knowledge_count(), 1);
}

#[tokio::test]
async fn memory_add_knowledge_rejects_empty_and_whitespace_only_text() {
    let (st, store) = state();
    let author = store.seed_author("s031-empty");

    for text in ["", "   \n\t  "] {
        let err = memory_add(&st, MemoryAddArgs::knowledge(author, text))
            .await
            .expect_err("text that normalizes to nothing must be refused");
        assert_eq!(
            err.meta.error_code, "SCHEMA_VALIDATION_FAILED",
            "for {text:?}: {err:?}"
        );
    }
    assert_eq!(store.knowledge_count(), 0);
}

#[tokio::test]
async fn memory_add_knowledge_rejects_too_many_tags() {
    let (st, store) = state();
    let author = store.seed_author("s031-tags");

    let mut args = MemoryAddArgs::knowledge(author, "s031 tagged");
    args.tags = (0..40).map(|i| format!("t{i}")).collect();

    let err = memory_add(&st, args)
        .await
        .expect_err("40 tags exceeds the 32-tag cap");
    assert_eq!(err.meta.error_code, "SCHEMA_VALIDATION_FAILED", "{err:?}");
    assert_eq!(store.knowledge_count(), 0);
}

// --------------------------------------------------------------- events

#[tokio::test]
async fn memory_append_event_round_trips_through_the_projection() {
    let (st, store) = state();
    let author = store.seed_author("s031-events");

    let out = append_event(
        &st,
        MemoryAppendEventArgs {
            author_id: author,
            category: "test.s031".into(),
            payload: serde_json::json!({"step": 1}),
            task_id: None,
        },
    )
    .await
    .expect("append_event");

    assert_eq!(store.event_count(), 1);
    assert_eq!(out.author.agent_name, "s031-events", "{out:?}");
}

#[tokio::test]
async fn memory_append_event_rejects_an_unknown_author() {
    let (st, _) = state();
    let err = append_event(
        &st,
        MemoryAppendEventArgs {
            author_id: Uuid::now_v7(),
            category: "test.s031".into(),
            payload: serde_json::json!({}),
            task_id: None,
        },
    )
    .await
    .expect_err("unknown author must be refused");
    assert_eq!(err.meta.error_code, "UNKNOWN_AUTHOR_ID", "{err:?}");
}
