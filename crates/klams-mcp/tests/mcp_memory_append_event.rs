//! Sprint 007 T039 — `memory_append_event` integration test (ignored).
//!
//! See `crates/klams-service/tests/mcp_phase5.rs` for the live
//! end-to-end variant.

#[ignore = "requires docker compose stack; see crates/klams-service/tests/mcp_phase5.rs"]
#[tokio::test]
async fn memory_append_event_round_trips_and_is_not_deletable() {
    // Covered by crates/klams-service/tests/mcp_phase5.rs::memory_append_event_smoke
}
