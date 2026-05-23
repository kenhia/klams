//! Bulk seeder that loads a [`Fixture`] into the live test stack via
//! direct `Store` calls (bypassing the HTTP layer for throughput).

use std::sync::Arc;

use klams_store::Store;

use super::fixture::Fixture;

/// Insert every fact / knowledge / event in `fixture` into `store`.
/// Returns counts written. Aborts on the first error.
pub async fn load(store: &Arc<super::TestStore>, fixture: &Fixture) -> SeedReport {
    let mut report = SeedReport::default();

    for fact in &fixture.facts {
        store.upsert_fact(fact.clone()).await.expect("seed fact");
        report.facts += 1;
    }

    for k in &fixture.knowledge {
        store
            .index_knowledge(k.clone())
            .await
            .expect("seed knowledge");
        report.knowledge += 1;
    }

    for e in &fixture.events {
        store.append_event(e.clone()).await.expect("seed event");
        report.events += 1;
    }

    report
}

#[derive(Debug, Default, Clone, Copy)]
pub struct SeedReport {
    pub facts: usize,
    pub knowledge: usize,
    pub events: usize,
}

/// Wipe `facts`, `events`, and `summaries` so a test starts from a
/// known-empty Postgres state. Reads `TEST_DATABASE_URL` (same
/// default as the harness). Knowledge in the shared Qdrant test
/// collection is left in place — tests that care about knowledge
/// counts should query by repo/file filters seeded for that test.
pub async fn truncate_pg() {
    let url = std::env::var("TEST_DATABASE_URL")
        .unwrap_or_else(|_| "postgres://klams:klams_test@127.0.0.1:55432/klams".into());
    let pool = sqlx::PgPool::connect(&url).await.expect("pg connect");
    sqlx::query("TRUNCATE facts, events, summaries CASCADE")
        .execute(&pool)
        .await
        .expect("truncate");
}
