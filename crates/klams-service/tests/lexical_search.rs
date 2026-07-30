//! Sprint 037 (#333) — the lexical candidate list against real Qdrant.
//!
//! Pins the store-level semantics the unit tests can only mock: the
//! full-text index built at `connect`, `matches_text`'s AND-over-tokens
//! filtering, and its lowercase tokenizer. Hand-supplied vectors keep
//! it deterministic — no TEI involved.
//!
//! Marked `#[ignore]`; run with `just test-integration` (or
//! `cargo test -p klams-service --test lexical_search -- --ignored`).

mod common;

use common::{ensure_collection, test_qdrant_grpc_url, TEST_EMBED_DIM};
use klams_store::QdrantStore;
use uuid::Uuid;

fn unit_vec(hot: &[(usize, f32)]) -> Vec<f32> {
    let mut v = vec![0.0_f32; TEST_EMBED_DIM];
    for (i, x) in hot {
        v[*i] = *x;
    }
    v
}

async fn seed(store: &QdrantStore, text: &str, embedding: Vec<f32>) -> Uuid {
    let id = Uuid::now_v7();
    store
        .index_knowledge(
            klams_types::IndexKnowledge {
                id,
                text: text.to_string(),
                content_hash: format!("lexical-{id}"),
                source: klams_types::Source::Task,
                tags: vec![],
                repo: None,
                file: None,
                machine: None,
                author_id: klams_types::SYSTEM_AUTHOR_ID,
                chunk_index: None,
                language: None,
                heading_path: None,
                symbols: vec![],
                volatility: None,
                supersedes: None,
            },
            embedding,
        )
        .await
        .expect("seed knowledge point");
    id
}

#[tokio::test]
#[ignore = "needs the docker test stack (just test-integration)"]
async fn lexical_search_filters_by_all_query_tokens_case_insensitively() {
    let collection = "lexical_search_test";
    ensure_collection(collection, true).await;
    let store = QdrantStore::connect(&test_qdrant_grpc_url(), collection, TEST_EMBED_DIM as u64)
        .await
        .expect("connect");

    // The measured live shape: the distractor's cosine (1.0) dwarfs the
    // target's (0.6), but only the target carries the query's tokens —
    // uppercased, to pin the lowercase tokenizer.
    let query_vec = unit_vec(&[(0, 1.0)]);
    let target = seed(
        &store,
        "GOTCHA — the Zanzibar Flywheel caveat lives in this text",
        unit_vec(&[(0, 0.6), (1, 0.8)]),
    )
    .await;
    let distractor = seed(
        &store,
        "a perfectly ordinary chunk with none of those words",
        unit_vec(&[(0, 1.0)]),
    )
    .await;

    // Sanity: plain ANN prefers the distractor.
    let ann = store
        .search_knowledge(query_vec.clone(), 10)
        .await
        .expect("ann search");
    assert_eq!(ann[0].0.id, distractor, "distractor must top plain ANN");

    // The lexical list keeps only the token-matching point, whatever
    // its cosine.
    let lexical = store
        .search_knowledge_lexical("zanzibar flywheel", query_vec.clone(), 10)
        .await
        .expect("lexical search");
    let ids: Vec<Uuid> = lexical.iter().map(|(item, _)| item.id).collect();
    assert_eq!(ids, vec![target], "only the token match may survive");

    // AND semantics: one absent token empties the list.
    let none = store
        .search_knowledge_lexical("zanzibar absenttoken", query_vec, 10)
        .await
        .expect("lexical search");
    assert!(none.is_empty(), "all tokens must be present: {none:?}");
}
