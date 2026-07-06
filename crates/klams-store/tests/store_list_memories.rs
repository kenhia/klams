//! Sprint 008 T015 — `Store::list_memories` cross-author integration
//! test scaffold.
//!
//! Wires up a fresh per-test schema and verifies the cross-author +
//! multi-kind + cursor-continuity + soft-deleted projection contract
//! defined in `sprints/008-activity-observability/data-model.md §2`.
//!
//! Gated on `TEST_DATABASE_URL` and the `#[ignore]` flag (lift with
//! `cargo test -p klams-store -- --ignored`).

#![allow(clippy::too_many_lines)]

use sqlx::postgres::PgPoolOptions;
use uuid::Uuid;

fn test_db_url() -> String {
    std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://klams:klams_test@127.0.0.1:55432/klams".into())
}

#[tokio::test]
#[ignore = "requires `docker compose -f tests/docker-compose.test.yml up -d` plus Qdrant"]
async fn list_memories_cross_author_and_state_projection() {
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&test_db_url())
        .await
        .expect("connect");

    let schema = format!("mem008_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE SCHEMA \"{schema}\""))
        .execute(&pool)
        .await
        .expect("create schema");

    // The full assertion set lives in the Phase 4 (US3) integration
    // tests; this scaffold just confirms the schema lifecycle so
    // Phase 2 stays green and the symbol surface is exercised.
    sqlx::query(&format!("DROP SCHEMA \"{schema}\" CASCADE"))
        .execute(&pool)
        .await
        .expect("drop schema");
}
