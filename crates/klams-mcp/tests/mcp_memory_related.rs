//! Sprint 007 T036 — `memory_related` integration test (ignored).
//!
//! See `crates/klams-service/tests/mcp_phase4.rs` for the live
//! end-to-end variant.

#[ignore = "requires docker compose stack; see crates/klams-service/tests/mcp_phase4.rs"]
#[tokio::test]
async fn memory_related_returns_neighbours_excluding_self() {
    // Covered by crates/klams-service/tests/mcp_phase4.rs::memory_related_smoke
}
