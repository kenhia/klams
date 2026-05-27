//! Sprint 008 T020 — `event_search` integration tests (ignored).
//!
//! Live end-to-end coverage is exercised from `klams-service` phase
//! tests when the compose stack is available.

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn event_search_filters_and_paginates() {
    // Covered by klams-service integration flow.
}
