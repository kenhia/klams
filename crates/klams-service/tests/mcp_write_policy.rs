//! Sprint 031 (#645) — the MCP write path enforces the same policy as
//! REST.
//!
//! Until 031 it enforced none of it. `memory_add` called `upsert_fact`
//! (v1) with no `ValidatorRegistry` and no way to target an existing
//! fact, so on the surface agents actually use:
//!
//!   * a payload REST rejects was stored, and
//!   * the documented safety property — "agents can't overwrite
//!     canonical facts; they disagree via dissent" — had no mechanism
//!     behind it at all.
//!
//! These tests are the WI's stated acceptance. They are docker-gated
//! because the trust/dissent divert lives in the Postgres transaction;
//! the hermetic mock-store coverage of the surrounding handler logic is
//! in `crates/klams-mcp/tests/`.

mod common;

use common::{make_author, mcp_state_from, TestServer};
use klams_mcp::tools::memory_add::{run as memory_add, FactTypeArg, MemoryAddArgs};
use klams_store::{DissentQuery, Store};
use klams_types::{DissentStatus, PublicMemoryContent, Source};

/// Pull `payload.value` off a fact projection.
fn fact_value(m: &klams_types::PublicMemory) -> &serde_json::Value {
    match &m.content {
        PublicMemoryContent::Fact { payload, .. } => &payload["value"],
        other => panic!("expected a fact projection, got {other:?}"),
    }
}

/// A `UserFact` with no `name` is rejected by `UserFactValidator`. REST
/// has always answered 422; MCP stored it.
#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn mcp_fact_write_runs_the_same_validators_as_rest() {
    let server = TestServer::spawn_isolated().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "s031-validator").await;

    let err = memory_add(
        &state,
        MemoryAddArgs::fact(author, FactTypeArg::UserFact, serde_json::json!({})),
    )
    .await
    .expect_err("a UserFact with no name must be refused");
    assert_eq!(err.meta.error_code, "SCHEMA_VALIDATION_FAILED", "{err:?}");
    assert!(
        err.content[0].text.contains("payload.name"),
        "the rejection must name the offending field so the agent can \
         fix it without guessing: {err:?}"
    );

    // And the shape the validator *does* accept still lands.
    let ok = memory_add(
        &state,
        MemoryAddArgs::fact(
            author,
            FactTypeArg::UserFact,
            serde_json::json!({"name": "Ken"}),
        ),
    )
    .await
    .expect("a valid UserFact must still be accepted");
    assert!(ok.write_path.is_none(), "valid write is canonical: {ok:?}");

    server.cleanup().await;
}

/// The headline property. An agent amending a fact written by a
/// higher-trust source must NOT overwrite it — the write becomes a
/// dissent, exactly as it does over REST.
#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn agent_amending_a_higher_trust_fact_lands_as_a_dissent() {
    let server = TestServer::spawn_isolated().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "s031-dissent").await;

    // Canonical fact from a trusted source, written the way the REST
    // surface would write it.
    let canonical = match state
        .store
        .upsert_fact_v2(klams_types::UpsertFact {
            fact_type: klams_types::FactType::EnvFact,
            payload: serde_json::json!({"key": "S031_TRUST", "value": "operator-set"}),
            source: Source::User,
            explicit_id: None,
            expected_version: None,
            author_id: author,
        })
        .await
        .expect("seed canonical fact")
    {
        klams_types::FactWriteOutcome::Persisted { fact } => fact,
        other => panic!("seed expected Persisted, got {other:?}"),
    };

    // The agent disagrees and amends it.
    let out = memory_add(
        &state,
        MemoryAddArgs {
            amends: Some(canonical.id),
            ..MemoryAddArgs::fact(
                author,
                FactTypeArg::EnvFact,
                serde_json::json!({"key": "S031_TRUST", "value": "agent-thinks-otherwise"}),
            )
        },
    )
    .await
    .expect("an amendment is accepted, it just may not become canonical");

    assert_eq!(
        out.write_path,
        Some("dissent"),
        "an AgentProposal amending a User-sourced fact must divert: {out:?}"
    );
    let dissent_id = out.dissent_id.expect("dissent path carries its id");

    // What comes back is what the store HOLDS, not what was submitted —
    // an agent that read its own payload back here would conclude it
    // had won the disagreement.
    assert_eq!(
        *fact_value(&out.memory),
        "operator-set",
        "the canonical value must be unchanged and reported: {out:?}"
    );

    let (dissents, _) = state
        .store
        .list_dissents(DissentQuery {
            fact_id: Some(canonical.id),
            ..DissentQuery::default()
        })
        .await
        .expect("list_dissents");
    let recorded = dissents
        .iter()
        .find(|d| d.id == dissent_id)
        .unwrap_or_else(|| panic!("dissent {dissent_id} must be queryable: {dissents:?}"));
    assert_eq!(recorded.status, DissentStatus::Pending);

    server.cleanup().await;
}

/// The other half of the trust rule: amending a fact at equal-or-lower
/// trust applies, and bumps the version. Without this the test above
/// would also pass if the divert fired unconditionally.
#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn agent_amending_its_own_proposal_applies_and_bumps_version() {
    let server = TestServer::spawn_isolated().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "s031-amend").await;

    let first = memory_add(
        &state,
        MemoryAddArgs::fact(
            author,
            FactTypeArg::EnvFact,
            serde_json::json!({"key": "S031_AMEND", "value": "first"}),
        ),
    )
    .await
    .expect("first write");

    let out = memory_add(
        &state,
        MemoryAddArgs {
            amends: Some(first.id),
            ..MemoryAddArgs::fact(
                author,
                FactTypeArg::EnvFact,
                serde_json::json!({"key": "S031_AMEND", "value": "corrected"}),
            )
        },
    )
    .await
    .expect("amend");

    assert!(
        out.write_path.is_none(),
        "equal-trust amendment is canonical, not a dissent: {out:?}"
    );
    assert_eq!(*fact_value(&out.memory), "corrected", "{out:?}");
    assert_eq!(out.id, first.id, "an amendment updates in place: {out:?}");

    server.cleanup().await;
}

/// The knowledge half: identical content is one point on both surfaces.
#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn mcp_knowledge_write_dedupes_on_content_hash_like_rest() {
    let server = TestServer::spawn_isolated().await;
    let state = mcp_state_from(&server);
    let author = make_author(&state, "s031-dedupe").await;

    let text = "s031: the reranker container listens on port 7071";
    let first = memory_add(&state, MemoryAddArgs::knowledge(author, text))
        .await
        .expect("first knowledge write");

    // Trailing whitespace only — normalization must make these one
    // point, which is the whole reason the hash covers the NORMALIZED
    // text rather than the raw input.
    let second = memory_add(
        &state,
        MemoryAddArgs::knowledge(author, format!("{text}  \n\n")),
    )
    .await
    .expect("second knowledge write");

    assert_eq!(
        second.id, first.id,
        "content differing only in trailing whitespace must dedupe onto \
         one point, not create a twin: {second:?}"
    );

    server.cleanup().await;
}
