//! Sprint 007 T031 — `memory_add` knowledge path integration test (ignored).

#[ignore = "requires docker compose stack; see crates/klams-service/tests/mcp_phase3.rs"]
#[tokio::test]
async fn memory_add_knowledge_embeds_via_tei_and_returns_retry_envelope_on_outage() {
    // Covered by crates/klams-service/tests/mcp_phase3.rs::memory_add_knowledge_smoke
}
