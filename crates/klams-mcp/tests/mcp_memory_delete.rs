//! Sprint 007 T041 — `memory_delete` soft-delete (ignored).
//!
//! See `crates/klams-service/tests/mcp_phase6.rs` for the live variant.

#[ignore = "requires docker compose stack; see crates/klams-service/tests/mcp_phase6.rs"]
#[tokio::test]
async fn memory_delete_soft_marks_row_and_is_idempotent() {
    // Covered by crates/klams-service/tests/mcp_phase6.rs::memory_delete_soft_smoke
}
