//! Sprint 007 T042 — `memory_admin_restore` (ignored).

#[ignore = "requires docker compose stack; see crates/klams-service/tests/mcp_phase6.rs"]
#[tokio::test]
async fn memory_admin_restore_clears_soft_delete() {
    // Covered by crates/klams-service/tests/mcp_phase6.rs::memory_admin_restore_smoke
}
