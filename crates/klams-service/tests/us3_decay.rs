//! Sprint 002 US3 (decay-aware ranking) — DB-touching integration
//! tests. Marked `#[ignore]` because they require the test
//! `docker-compose.test.yml` stack (Postgres + TEI + Qdrant).
//!
//! Run with `cargo test --test us3_decay -- --ignored` once the
//! stack is up.

use klams_core::{DecayConfig, DecayTask};
use klams_store::{FactQuery, PostgresStore};
use klams_types::{FactType, Source, UpsertFact};
use serde_json::json;
use std::sync::Arc;
use time::{Duration as TDuration, OffsetDateTime};

async fn connect() -> PostgresStore {
    let url = std::env::var("KLAMS_POSTGRES_URL")
        .or_else(|_| std::env::var("TEST_DATABASE_URL"))
        .unwrap_or_else(|_| "postgres://klams:klams_test@127.0.0.1:55432/klams".to_string());
    PostgresStore::connect(&url, 4)
        .await
        .expect("connect to test postgres")
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml stack"]
async fn task_fact_decays_faster_than_user_fact() {
    let store = Arc::new(connect().await);
    // Seed two facts of different types sharing a search term.
    let user = store
        .upsert_fact(UpsertFact {
            fact_type: FactType::UserFact,
            payload: json!({"name": "alpha-shared-decay-term"}),
            source: Source::User,
            explicit_id: None,
            expected_version: None,
        })
        .await
        .expect("user upsert");
    let task = store
        .upsert_fact(UpsertFact {
            fact_type: FactType::TaskFact,
            payload: json!({
                "title": "alpha-shared-decay-term",
                "status": "in_progress",
                "task_id": uuid::Uuid::now_v7(),
            }),
            source: Source::Task,
            explicit_id: None,
            expected_version: None,
        })
        .await
        .expect("task upsert");
    // Backdate last_used_at by 7 days for both.
    let seven_days_ago = OffsetDateTime::now_utc() - TDuration::days(7);
    sqlx::query("UPDATE facts SET last_used_at = $1 WHERE id IN ($2, $3)")
        .bind(seven_days_ago)
        .bind(user.id)
        .bind(task.id)
        .execute(store.pool())
        .await
        .expect("backdate");

    let cfg = DecayConfig::default();
    let mut decay = DecayTask::new(cfg, Arc::clone(&store));
    let updated = decay.tick_once().await.expect("tick_once");
    assert!(
        updated >= 2,
        "expected at least 2 rows updated, got {updated}"
    );

    // Re-read both facts and confirm TaskFact weight < UserFact weight.
    let (rows, _) = store
        .list_facts(FactQuery {
            fact_type: None,
            source: None,
            created_after: None,
            created_before: None,
            limit: 100,
            cursor: None,
        })
        .await
        .expect("list");
    let user_w = rows
        .iter()
        .find(|f| f.id == user.id)
        .map(|f| f.decay_weight)
        .unwrap();
    let task_w = rows
        .iter()
        .find(|f| f.id == task.id)
        .map(|f| f.decay_weight)
        .unwrap();
    assert!(
        task_w < user_w,
        "TaskFact decay_weight ({task_w}) should be < UserFact ({user_w})"
    );
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml stack"]
async fn tick_is_monotonically_non_increasing() {
    let store = Arc::new(connect().await);
    let _ = store
        .upsert_fact(UpsertFact {
            fact_type: FactType::TaskFact,
            payload: json!({
                "title": "monotone-decay-test",
                "status": "todo",
                "task_id": uuid::Uuid::now_v7(),
            }),
            source: Source::Task,
            explicit_id: None,
            expected_version: None,
        })
        .await
        .expect("seed");
    let cfg = DecayConfig::default();
    let mut decay = DecayTask::new(cfg, Arc::clone(&store));
    decay.tick_once().await.expect("tick 1");
    let (snap1, _) = store
        .list_facts(FactQuery {
            fact_type: Some(FactType::TaskFact),
            source: None,
            created_after: None,
            created_before: None,
            limit: 100,
            cursor: None,
        })
        .await
        .expect("list");
    decay.tick_once().await.expect("tick 2");
    let (snap2, _) = store
        .list_facts(FactQuery {
            fact_type: Some(FactType::TaskFact),
            source: None,
            created_after: None,
            created_before: None,
            limit: 100,
            cursor: None,
        })
        .await
        .expect("list");
    for f1 in &snap1 {
        let f2 = snap2.iter().find(|x| x.id == f1.id).expect("same id");
        assert!(
            f2.decay_weight <= f1.decay_weight,
            "decay_weight must be non-increasing across ticks: {} -> {}",
            f1.decay_weight,
            f2.decay_weight
        );
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml stack"]
async fn search_orders_user_above_task_for_shared_term() {
    let store = Arc::new(connect().await);
    let term = "shared-search-decay-term-9213";
    let user = store
        .upsert_fact(UpsertFact {
            fact_type: FactType::UserFact,
            payload: json!({"name": term}),
            source: Source::User,
            explicit_id: None,
            expected_version: None,
        })
        .await
        .expect("user");
    let task = store
        .upsert_fact(UpsertFact {
            fact_type: FactType::TaskFact,
            payload: json!({
                "title": term,
                "status": "in_progress",
                "task_id": uuid::Uuid::now_v7(),
            }),
            source: Source::Task,
            explicit_id: None,
            expected_version: None,
        })
        .await
        .expect("task");
    let seven_days_ago = OffsetDateTime::now_utc() - TDuration::days(7);
    sqlx::query("UPDATE facts SET last_used_at = $1 WHERE id IN ($2, $3)")
        .bind(seven_days_ago)
        .bind(user.id)
        .bind(task.id)
        .execute(store.pool())
        .await
        .expect("backdate");
    let cfg = DecayConfig::default();
    let mut decay = DecayTask::new(cfg, Arc::clone(&store));
    decay.tick_once().await.expect("tick");

    let (facts, _) = store.search_text(term, 10).await.expect("search");
    let user_pos = facts
        .iter()
        .position(|h| h.id == user.id)
        .expect("user hit");
    let task_pos = facts
        .iter()
        .position(|h| h.id == task.id)
        .expect("task hit");
    assert!(
        user_pos < task_pos,
        "UserFact (pos {user_pos}) should rank ahead of TaskFact (pos {task_pos})"
    );
}
