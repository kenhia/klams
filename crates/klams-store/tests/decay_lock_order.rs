//! Sprint 040 (#811) — the `facts` batch writers must not deadlock.
//!
//! `apply_decay_batch` (`UPDATE … FROM UNNEST`) and
//! `apply_last_used_bumps` (`UPDATE … WHERE id = ANY`) both touch many
//! `facts` rows in one statement, and both run concurrently in
//! production: the decay task fires hourly while the read path keeps
//! flushing `last_used_at` bumps for whatever searches returned.
//!
//! Two multi-row `UPDATE`s over an overlapping row set deadlock (40P01)
//! whenever they acquire row locks in different orders, and neither
//! statement pins its order — that is the planner's choice, and the two
//! statements do not share a plan shape. Single-row writers are not part
//! of this: one statement taking one lock can never be the party that
//! holds A and waits for B.
//!
//! This is what took down `task_fact_decays_faster_than_user_fact`
//! during sprint 039's integration run.

use klams_store::PostgresStore;
use klams_types::{FactType, Source, UpsertFact};
use serde_json::json;
use std::sync::Arc;
use uuid::Uuid;

async fn connect() -> PostgresStore {
    let url = std::env::var("KLAMS_POSTGRES_URL")
        .or_else(|_| std::env::var("TEST_DATABASE_URL"))
        .unwrap_or_else(|_| "postgres://klams:klams_test@127.0.0.1:55432/klams".to_string());
    PostgresStore::connect(&url, 16)
        .await
        .expect("connect to test postgres")
}

/// Seed `n` facts and return their ids, sorted ascending.
async fn seed(store: &PostgresStore, n: usize, tag: &str) -> Vec<Uuid> {
    let mut ids = Vec::with_capacity(n);
    for i in 0..n {
        let out = store
            .upsert_fact_v2(UpsertFact {
                fact_type: FactType::TaskFact,
                payload: json!({ "name": format!("{tag}-{i}") }),
                source: Source::Task,
                explicit_id: None,
                expected_version: None,
                author_id: klams_types::SYSTEM_AUTHOR_ID,
            })
            .await
            .expect("seed");
        if let klams_types::FactWriteOutcome::Persisted { fact } = out {
            ids.push(fact.id);
        }
    }
    ids.sort_unstable();
    ids
}

#[tokio::test(flavor = "multi_thread", worker_threads = 8)]
#[ignore = "requires docker-compose.test.yml stack"]
async fn concurrent_facts_batch_updates_do_not_deadlock() {
    let store = Arc::new(connect().await);
    let ids = seed(&store, 400, "lockorder").await;
    assert!(!ids.is_empty(), "seeding produced no rows");

    // Both writers hammer the SAME row set. Before the sprint-040 fix
    // each picked its own lock order and this reported
    // `deadlock detected` within a few rounds.
    let mut tasks = Vec::new();
    for round in 0..12u32 {
        let decay_store = Arc::clone(&store);
        let fwd = ids.clone();
        // Vary the weight per round so every round is a real write.
        let weight = 0.5 + f32::from(u16::try_from(round % 7).expect("round % 7 fits u16")) / 100.0;
        tasks.push(tokio::spawn(async move {
            let updates: Vec<(Uuid, f32)> = fwd.iter().map(|id| (*id, weight)).collect();
            decay_store.apply_decay_batch(&updates).await
        }));

        let bump_store = Arc::clone(&store);
        // Reversed on purpose: if either statement's lock order followed
        // its input array, this is the ordering that collides.
        let mut rev = ids.clone();
        rev.reverse();
        tasks.push(tokio::spawn(async move {
            bump_store.apply_last_used_bumps(&rev).await
        }));
    }

    let mut deadlocks = Vec::new();
    for t in tasks {
        if let Err(e) = t.await.expect("task panicked") {
            let msg = e.to_string();
            assert!(
                !msg.contains("deadlock"),
                "concurrent facts batch updates deadlocked: {msg}"
            );
            deadlocks.push(msg);
        }
    }
    assert!(
        deadlocks.is_empty(),
        "batch updates failed (non-deadlock): {deadlocks:?}"
    );
}
