//! Sprint 008 T034 — Activity command round-trip integration scaffold.
//!
//! This target is intentionally ignored by default because it needs a
//! running mocked klams-service endpoint and a Tauri harness process.
//! The command-level argument mapping is covered in unit tests in
//! `src/commands/memory.rs` and in frontend invoke tests in
//! `viewport/src/lib/api.test.ts`.

#[test]
#[ignore = "requires tauri integration harness + mocked klams-service"]
fn viewport_activity_command_roundtrip_scaffold() {
    // Reserved for a full Tauri integration harness.
}
