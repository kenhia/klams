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
