//! Sprint 046 (WI #853) — the MCP `instructions` block must describe
//! the post-sprint-025 world.
//!
//! This block is delivered on every connection to every agent and
//! cannot be checked by its reader, so a wrong sentence in it is paid
//! for constantly and silently. Two sentences had gone stale and one of
//! them caused a real misdiagnosis (the 2026-07-25 rpidash3 incident,
//! handoff korg:635), so the claims are pinned here rather than left to
//! the next reader to notice.
//!
//! These assertions are deliberately about *claims*, not phrasing —
//! rewording the block is fine, re-acquiring a retracted claim is not.

use klams_mcp::tools::SERVER_INSTRUCTIONS;

/// Sprint 025 made `register_author` idempotent on `agent_name`: it
/// dedupes and returns the token-bound author. The old text warned that
/// calling it would create "a separate per-session identity" — a
/// consequence that can no longer occur, and the exact reading that made
/// a session conclude its author id was undiscoverable.
#[test]
fn does_not_warn_about_a_separate_identity_register_author_cannot_create() {
    let lowered = SERVER_INSTRUCTIONS.to_lowercase();
    assert!(
        !lowered.contains("separate per-session identity"),
        "instructions still warn about the pre-025 `register_author` behaviour:\n{SERVER_INSTRUCTIONS}"
    );
    assert!(
        lowered.contains("idempotent"),
        "instructions should say `register_author` is idempotent on agent_name, \
         so a session that needs its author id knows calling it is the way through:\n{SERVER_INSTRUCTIONS}"
    );
}

/// `similar_existing` rides on the response to a write that has ALREADY
/// happened. "Instead of" describes an action the caller no longer has.
#[test]
fn describes_similar_existing_as_after_the_fact() {
    let lowered = SERVER_INSTRUCTIONS.to_lowercase();
    assert!(
        !lowered.contains("instead of writing a near-duplicate"),
        "instructions still advise an action that is no longer available — the \
         near-duplicate exists by the time `similar_existing` is read:\n{SERVER_INSTRUCTIONS}"
    );
    assert!(
        lowered.contains("already written"),
        "instructions should say the duplicate is already written:\n{SERVER_INSTRUCTIONS}"
    );
    assert!(
        lowered.contains("memory_delete") && lowered.contains("memory_supersede"),
        "instructions should name the actual remedy (delete the new record, \
         supersede the original):\n{SERVER_INSTRUCTIONS}"
    );
}

/// The block is what `get_info` serves — not a constant that drifted
/// out of use. Without this, the two tests above could pass against a
/// string no client ever receives.
#[test]
fn get_info_serves_the_pinned_block() {
    let src = include_str!("../src/tools/mod.rs");
    assert!(
        src.contains("info.instructions = Some(SERVER_INSTRUCTIONS.into());"),
        "get_info no longer serves SERVER_INSTRUCTIONS — the pinned block is dead text"
    );
}
