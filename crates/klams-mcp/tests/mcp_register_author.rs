//! Sprint 007 T029 — `register_author` integration test (ignored).
//!
//! Requires the docker compose stack (postgres + qdrant + tei) and the
//! `TEST_DATABASE_URL` / `TEST_QDRANT_URL` / `TEST_TEI_URL` env vars
//! that the klams-service test harness consumes. Marked `#[ignore]`
//! by default so `cargo test --workspace` stays hermetic.
//!
//! End-to-end coverage of `register_author` + `memory_add` lives in
//! `crates/klams-service/tests/mcp_phase3.rs` (added alongside this
//! file) — that harness boots the full router and exercises the tool
//! via the Streamable HTTP transport.

#[ignore = "requires docker compose stack; see crates/klams-service/tests/mcp_phase3.rs"]
#[tokio::test]
async fn register_author_returns_uuid_v7_and_touches_last_seen_on_repeat() {
    // Covered by crates/klams-service/tests/mcp_phase3.rs::register_author_smoke
}
