//! SC-008 rogue-agent drill, hermetically.
//!
//! Sprint 031 (#646). Replaces the empty `mcp_rogue_agent_drill.rs`
//! stub, whose body was a comment saying the drill was "composed from
//! the per-tool flows in `crates/klams-service/tests/mcp_phase6.rs`" —
//! which is to say the drill as a *scenario* was never run anywhere.
//! Individual tools passing does not establish that the containment
//! story holds end to end.
//!
//! The drill: an agent writes a batch of memories, one turns out to be
//! wrong, and an operator has to contain it — find it, retract it,
//! confirm it stopped surfacing, and be able to reverse that if the
//! retraction was itself a mistake.

mod support;

use klams_mcp::tools::{
    memory_add::{run as memory_add, MemoryAddArgs},
    memory_admin_hard_delete::{run as hard_delete, MemoryAdminHardDeleteArgs},
    memory_admin_restore::{run as restore, MemoryAdminRestoreArgs},
    memory_delete::{run as memory_delete, MemoryDeleteArgs},
    memory_search::{run as memory_search, MemorySearchArgs},
};
use klams_types::Scope;
use support::{caller, state};

fn search(query: &str) -> MemorySearchArgs {
    MemorySearchArgs {
        query: query.to_string(),
        kinds: None,
        tags: None,
        top_k: Some(50),
        // Asserts on retrieval semantics, not the wire shape (#1178).
        full: Some(true),
    }
}

#[tokio::test]
async fn a_rogue_memory_can_be_found_retracted_and_reinstated() {
    let (st, store) = state();

    let rogue_agent = store.seed_author("s031-rogue");
    let operator = store.seed_author("s031-operator");

    // 1. The agent writes a batch. One of them is wrong.
    let mut written = Vec::new();
    for text in [
        "s031 drill: backups land in /gratch/klams-backup",
        "s031 drill: the reranker listens on port 7071",
        "s031 drill: WRONG the service listens on port 9999",
    ] {
        let out = memory_add(&st, MemoryAddArgs::knowledge(rogue_agent, text))
            .await
            .expect("agent write");
        written.push(out.id);
    }
    let rogue = written[2];
    assert_eq!(store.knowledge_count(), 3);

    // 2. It is findable — which is the problem.
    let before = memory_search(&st, search("port"), None)
        .await
        .expect("search")
        .into_full()
        .expect("full: true was requested");
    assert!(
        before.iter().any(|h| h.memory.id == rogue),
        "pre-condition: the bad memory is in the corpus: {before:?}"
    );

    // 3. A peer agent must NOT be able to retract it. Containment that
    //    any caller can perform is not containment.
    let peer = store.seed_author("s031-peer");
    let err = memory_delete(
        &st,
        MemoryDeleteArgs {
            author_id: None,
            id: rogue,
        },
        Some(&caller(peer, vec![Scope::Read, Scope::Write])),
    )
    .await
    .expect_err("a write-scoped peer may not retract another author's memory");
    assert_eq!(err.meta.error_code, "INSUFFICIENT_SCOPE", "{err:?}");

    // 4. An operator holding `manage` retracts it.
    memory_delete(
        &st,
        MemoryDeleteArgs {
            author_id: None,
            id: rogue,
        },
        Some(&caller(
            operator,
            vec![Scope::Read, Scope::Write, Scope::Manage],
        )),
    )
    .await
    .expect("manage scope may curate across authors");

    // 5. It stops surfacing, and the agent's OTHER memories survive —
    //    containment must be surgical, not a purge of the author.
    let after = memory_search(&st, search("port"), None)
        .await
        .expect("search")
        .into_full()
        .expect("full: true was requested");
    assert!(
        !after.iter().any(|h| h.memory.id == rogue),
        "the retracted memory must be gone from search: {after:?}"
    );
    assert!(
        after.iter().any(|h| h.memory.id == written[1]),
        "the agent's correct memories must be untouched: {after:?}"
    );

    // 6. The retraction is reversible — an operator who retracts the
    //    wrong thing has to be able to undo it.
    restore(&st, MemoryAdminRestoreArgs { id: rogue })
        .await
        .expect("restore");
    let restored = memory_search(&st, search("port"), None)
        .await
        .expect("search")
        .into_full()
        .expect("full: true was requested");
    assert!(
        restored.iter().any(|h| h.memory.id == rogue),
        "a restored memory is findable again: {restored:?}"
    );

    // 7. And when the operator is sure, hard delete is final.
    hard_delete(&st, MemoryAdminHardDeleteArgs { id: rogue })
        .await
        .expect("hard delete");
    assert_eq!(store.knowledge_count(), 2, "the point is gone, not hidden");
    let finally = memory_search(&st, search("port"), None)
        .await
        .expect("search")
        .into_full()
        .expect("full: true was requested");
    assert!(!finally.iter().any(|h| h.memory.id == rogue));
}
