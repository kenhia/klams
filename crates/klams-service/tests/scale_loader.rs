//! Sprint 006 T014 (R-009) — Day-0 sizing fixture loader.
//!
//! Loads the `large` fixture preset (~10k facts / ~20k knowledge
//! chunks / ~50k events) into the live `docker-compose.test.yml`
//! stack. Multi-minute runtime — gated by the `scale-fixture` Cargo
//! feature so it never runs with `cargo test --workspace`.
//!
//! Usage:
//!
//! ```bash
//! docker compose -f tests/docker-compose.test.yml up -d
//! cargo test -p klams-service --features scale-fixture \
//!     --test scale_loader -- --ignored --nocapture
//! ```
//!
//! Consumed by `just backup-size` (T015) and Phase 4's
//! `restore_roundtrip` integration test (T029).

#![cfg(feature = "scale-fixture")]

mod common;

use common::{
    fixture::{generate_with_seed, FixtureScale},
    seed, TestServer,
};

#[tokio::test(flavor = "multi_thread")]
#[ignore = "multi-minute; run explicitly with --features scale-fixture --ignored"]
async fn load_large_fixture() {
    let server = TestServer::spawn().await;
    let fixture = generate_with_seed(FixtureScale::large(), 0xDA70_0006_BACC_0006);

    let started = std::time::Instant::now();
    let report = seed::load(&server.store, &fixture).await;
    let elapsed = started.elapsed();

    println!(
        "scale-fixture loaded: facts={} knowledge={} events={} in {:.1}s",
        report.facts,
        report.knowledge,
        report.events,
        elapsed.as_secs_f64(),
    );

    assert_eq!(report.facts, 10_000);
    assert_eq!(report.knowledge, 20_000);
    assert_eq!(report.events, 50_000);
}
