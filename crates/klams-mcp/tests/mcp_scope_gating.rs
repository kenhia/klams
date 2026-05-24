//! Sprint 007 T026 (foundational) — scope-gated tool registry smoke test.
//!
//! The Phase 2 foundational tool registry is empty (no tools defined
//! yet), so this test only proves the surface compiles, the router
//! mounts, and `tools/list` returns an empty result for every scope
//! mix. Per-tool scope-visibility coverage lands in Phase 3+ tests
//! once tools exist.

use klams_mcp::tools::{McpState, ToolRegistry};
use rmcp::ServerHandler;

#[tokio::test]
async fn tools_list_is_empty_for_phase2_foundation() {
    let registry = ToolRegistry::new(McpState::empty());
    let info = registry.get_info();
    assert_eq!(info.server_info.name, "klams-mcp");
}
