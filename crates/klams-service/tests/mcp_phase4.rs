//! Sprint 007 Phase 4 — `memory_search` + `memory_related` end-to-end
//! against the in-process docker compose stack.
//!
//! Marked `#[ignore]` by default — run with `cargo test -- --ignored`
//! after bringing up `docker compose -f tests/docker-compose.test.yml`.

mod common;

use common::TestServer;
use klams_mcp::tools::{
    memory_add::{run as memory_add, FactTypeArg, MemoryAddArgs},
    memory_related::{run as memory_related, MemoryRelatedArgs},
    memory_search::{run as memory_search, MemoryKindFilter, MemorySearchArgs},
    register_author::{run as register, RegisterAuthorInput},
    McpState,
};
use klams_types::{MaintenanceState, MemoryKind};
use std::sync::Arc;

fn mcp_state_from(server: &TestServer) -> McpState {
    McpState::new(
        Arc::clone(&server.store),
        Arc::new(MaintenanceState::default()),
        klams_types::ApiConfig::default(),
    )
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
#[allow(clippy::too_many_lines)] // comprehensive smoke: shape + filters + leak
async fn memory_search_smoke() {
    let server = TestServer::spawn().await;
    let state = mcp_state_from(&server);
    let author = register(
        &state,
        RegisterAuthorInput {
            agent_name: "GHCP-test-search".into(),
            model: None,
            session_title: None,
            repo: None,
            client_app: None,
            client_version: None,
            extra: serde_json::Value::Null,
        },
    )
    .await
    .expect("register_author");

    // Seed a fact + a knowledge item so both backends contribute.
    memory_add(
        &state,
        MemoryAddArgs::fact(
            author.author_id,
            FactTypeArg::EnvFact,
            serde_json::json!({
                "key": "phase4-search-target",
                "value": "needle phase4 search"
            }),
        ),
    )
    .await
    .expect("seed fact");

    memory_add(
        &state,
        MemoryAddArgs {
            tags: vec!["phase4".into()],
            source_path: None,
            repo: None,
            ..MemoryAddArgs::knowledge(author.author_id, "phase4 search needle knowledge body")
        },
    )
    .await
    .expect("seed knowledge");

    // Sprint 017: a second knowledge item so the knowledge-only query
    // below returns >=2 hits with distinct, real per-source ranks.
    memory_add(
        &state,
        MemoryAddArgs {
            tags: vec!["phase4".into()],
            source_path: None,
            repo: None,
            ..MemoryAddArgs::knowledge(
                author.author_id,
                "a second needle knowledge entry for ranking",
            )
        },
    )
    .await
    .expect("seed knowledge 2");

    let hits = memory_search(
        &state,
        MemorySearchArgs {
            query: "needle".into(),
            kinds: None,
            tags: None,
            top_k: Some(20),
        },
    )
    .await
    .expect("memory_search");
    assert!(!hits.is_empty(), "expected at least one search hit");
    // Sprint 016: each result is a ScoredMemory envelope. The score is
    // present (finite) and source_rank is a real per-source rank.
    let hit = serde_json::to_value(&hits[0]).expect("serialize");
    let obj = hit.as_object().expect("object");
    assert!(obj.contains_key("score"), "ScoredMemory missing score");
    assert!(
        obj.contains_key("source_rank"),
        "ScoredMemory missing source_rank"
    );
    assert!(obj.contains_key("memory"), "ScoredMemory missing memory");
    // source_kind would duplicate memory.kind — must NOT be a field.
    assert!(!obj.contains_key("source_kind"));
    assert!(hits[0].score.is_finite(), "score must be finite");
    // Sprint 017: the merged result list is ordered by score descending
    // — the cross-source fusion invariant the eval harness relies on.
    // Assert the behavior, not just that the field exists.
    assert!(
        hits.windows(2).all(|w| w[0].score >= w[1].score),
        "merged results must be sorted by score descending"
    );
    // FR-011: the wrapped projection must not leak internal fields.
    let mem = &obj["memory"];
    let mem_obj = mem.as_object().expect("memory object");
    for forbidden in [
        "version",
        "confidence",
        "decay_weight",
        "use_count",
        "deleted_at",
    ] {
        assert!(
            !mem_obj.contains_key(forbidden),
            "PublicMemory leaked `{forbidden}`"
        );
    }

    // Kind filter narrows the result.
    let only_knowledge = memory_search(
        &state,
        MemorySearchArgs {
            query: "needle".into(),
            kinds: Some(vec![MemoryKindFilter::Knowledge]),
            tags: None,
            top_k: Some(20),
        },
    )
    .await
    .expect("memory_search knowledge-only");
    assert!(only_knowledge
        .iter()
        .all(|h| h.memory.kind() == MemoryKind::Knowledge));
    // Sprint 017: with two seeded knowledge items the single-source
    // result carries real per-source ranks. `source_rank` must reflect
    // that source's own ordering: the ranks form a 0-based contiguous
    // set, and a lower rank carries a higher-or-equal score.
    assert!(
        only_knowledge.len() >= 2,
        "expected >=2 knowledge hits to exercise per-source ranking"
    );
    let mut rank_score: Vec<(usize, f32)> = only_knowledge
        .iter()
        .map(|h| (h.source_rank as usize, h.score))
        .collect();
    rank_score.sort_by_key(|(r, _)| *r);
    assert_eq!(
        rank_score.iter().map(|(r, _)| *r).collect::<Vec<_>>(),
        (0..only_knowledge.len()).collect::<Vec<_>>(),
        "single-source `source_rank`s must be 0-based and contiguous"
    );
    assert!(
        rank_score.windows(2).all(|w| w[0].1 >= w[1].1),
        "a lower source_rank must carry a higher-or-equal score"
    );

    // Tag filter is honored.
    let tagged = memory_search(
        &state,
        MemorySearchArgs {
            query: "needle".into(),
            kinds: Some(vec![MemoryKindFilter::Knowledge]),
            tags: Some(vec!["phase4".into()]),
            top_k: Some(20),
        },
    )
    .await
    .expect("memory_search tagged");
    assert!(tagged
        .iter()
        .all(|h| h.memory.tags.iter().any(|t| t == "phase4")));
}

#[ignore = "requires docker compose stack"]
#[tokio::test]
async fn memory_related_smoke() {
    let server = TestServer::spawn().await;
    let state = mcp_state_from(&server);
    let author = register(
        &state,
        RegisterAuthorInput {
            agent_name: "GHCP-test-related".into(),
            model: None,
            session_title: None,
            repo: None,
            client_app: None,
            client_version: None,
            extra: serde_json::Value::Null,
        },
    )
    .await
    .expect("register_author");

    // Seed several similar knowledge points.
    let mut ids = Vec::new();
    for i in 0..4 {
        let mem = memory_add(
            &state,
            MemoryAddArgs {
                tags: vec!["related-seed".into()],
                source_path: None,
                repo: None,
                ..MemoryAddArgs::knowledge(
                    author.author_id,
                    format!("phase4 related seed point number {i} about kittens"),
                )
            },
        )
        .await
        .expect("seed knowledge");
        ids.push(mem.id);
    }

    let neighbours = memory_related(
        &state,
        MemoryRelatedArgs {
            id: ids[0],
            top_k: Some(3),
        },
    )
    .await
    .expect("memory_related");
    assert!(neighbours.len() <= 3, "respects top_k");
    assert!(
        neighbours.iter().all(|m| m.id != ids[0]),
        "seed point excluded from results"
    );
    assert!(
        neighbours.iter().all(|m| m.kind() == MemoryKind::Knowledge),
        "only knowledge returned"
    );
}
