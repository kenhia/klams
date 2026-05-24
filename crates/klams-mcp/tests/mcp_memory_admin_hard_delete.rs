//! Sprint 007 T043 — `memory_admin_hard_delete` (ignored).

#[ignore = "requires docker compose stack; see crates/klams-service/tests/mcp_phase6.rs"]
#[tokio::test]
async fn memory_admin_hard_delete_removes_row_and_point() {
    // Covered by crates/klams-service/tests/mcp_phase6.rs::memory_admin_hard_delete_smoke
}
