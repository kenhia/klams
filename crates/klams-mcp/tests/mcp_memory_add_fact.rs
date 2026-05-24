//! Sprint 007 T030 — `memory_add` fact path integration test (ignored).
//!
//! See `crates/klams-service/tests/mcp_phase3.rs` for the live
//! end-to-end variant.

#[ignore = "requires docker compose stack; see crates/klams-service/tests/mcp_phase3.rs"]
#[tokio::test]
async fn memory_add_fact_persists_with_author_attribution_and_rejects_unknown_author() {
    // Covered by crates/klams-service/tests/mcp_phase3.rs::memory_add_fact_smoke
}
