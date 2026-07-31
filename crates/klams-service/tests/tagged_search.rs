//! Sprint 041 (#799) — tag-filtered knowledge search against real Qdrant.
//!
//! Pins the thing the old code could not do: surface a tagged memory
//! that the *global* ANN page never contained. The bug was not a tuning
//! miss — `tags` was applied as a `retain` over an already-fetched pool,
//! so a tagged point outside that pool was unreachable at any
//! over-fetch factor. Measured live before the fix, a `gotcha`-filtered
//! search returned 4, 1 and 0 hits for three ordinary queries against a
//! 36-point stratum.
//!
//! The seed below reproduces that shape deterministically: a crowd of
//! untagged points sitting exactly on the query vector, and tagged
//! points deliberately far from it. Any unfiltered ANN returns the
//! crowd; only a server-side filter reaches the tagged ones.
//!
//! Hand-supplied vectors keep it deterministic — no TEI involved.
//!
//! Marked `#[ignore]`; run with `just test-integration` (or
//! `cargo test -p klams-service --test tagged_search -- --ignored`).

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

async fn seed(store: &QdrantStore, text: &str, tags: Vec<String>, embedding: Vec<f32>) -> Uuid {
    let id = Uuid::now_v7();
    store
        .index_knowledge(
            klams_types::IndexKnowledge {
                id,
                text: text.to_string(),
                content_hash: format!("tagged-{id}"),
                source: klams_types::Source::Task,
                tags,
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
async fn tagged_search_reaches_points_the_global_ann_page_never_contains() {
    let collection = "tagged_search_test";
    ensure_collection(collection, true).await;
    let store = QdrantStore::connect(&test_qdrant_grpc_url(), collection, TEST_EMBED_DIM as u64)
        .await
        .expect("connect");

    let marker = format!("s041-{}", Uuid::now_v7().simple());

    // The query direction, and the crowd sitting right on top of it.
    let query = unit_vec(&[(0, 1.0)]);
    for i in 0..60u8 {
        seed(
            &store,
            &format!("{marker} bulk chunk {i}"),
            vec![],
            unit_vec(&[(0, 1.0), (1, 0.001 * f32::from(i))]),
        )
        .await;
    }

    // The tagged points, deliberately orthogonal to the query — exactly
    // the memory that "should have been found" but never ranks.
    let mut tagged = Vec::new();
    for i in 0..3u8 {
        tagged.push(
            seed(
                &store,
                &format!("{marker} tagged note {i}"),
                vec![marker.clone(), "s041-tag".into()],
                unit_vec(&[(2, 1.0), (3, 0.01 * f32::from(i))]),
            )
            .await,
        );
    }

    // The old path: rank globally, then discard whatever is untagged.
    // Over-fetch generously — the point is that no factor helps.
    let global = store
        .search_knowledge(query.clone(), 30)
        .await
        .expect("global ann");
    let global_tagged = global
        .iter()
        .filter(|(item, _)| item.tags.iter().any(|t| t == &marker))
        .count();
    assert_eq!(
        global_tagged, 0,
        "seed is wrong: the global ANN page already contains the tagged \
         points, so this test would pass without the fix"
    );

    // The new path: rank over the tagged subset.
    let hits = store
        .search_knowledge_tagged(query.clone(), std::slice::from_ref(&marker), 30)
        .await
        .expect("tagged ann");
    let found: Vec<Uuid> = hits.iter().map(|(item, _)| item.id).collect();
    for id in &tagged {
        assert!(
            found.contains(id),
            "tagged search must surface {id}, which the global page never held"
        );
    }
    assert_eq!(
        hits.len(),
        tagged.len(),
        "tagged search must return the tagged subset and nothing else"
    );
}

#[tokio::test]
#[ignore = "needs the docker test stack (just test-integration)"]
async fn tagged_search_requires_every_tag() {
    // Its own collection: the sibling test seeds 60 points on the query
    // vector, and a shared collection would let those bleed in.
    let collection = "tagged_search_and_test";
    ensure_collection(collection, true).await;
    let store = QdrantStore::connect(&test_qdrant_grpc_url(), collection, TEST_EMBED_DIM as u64)
        .await
        .expect("connect");

    let a = format!("s041a-{}", Uuid::now_v7().simple());
    let b = format!("s041b-{}", Uuid::now_v7().simple());
    let query = unit_vec(&[(0, 1.0)]);

    let both = seed(
        &store,
        "carries both tags",
        vec![a.clone(), b.clone()],
        unit_vec(&[(0, 1.0)]),
    )
    .await;
    let only_a = seed(
        &store,
        "carries one tag",
        vec![a.clone()],
        unit_vec(&[(0, 0.9)]),
    )
    .await;

    // AND, not OR — matching the post-projection retain that remains the
    // definition of what the filter means.
    let hits = store
        .search_knowledge_tagged(query, &[a.clone(), b.clone()], 10)
        .await
        .expect("tagged ann");
    let ids: Vec<Uuid> = hits.iter().map(|(item, _)| item.id).collect();
    assert!(
        ids.contains(&both),
        "the point carrying both tags must match"
    );
    assert!(
        !ids.contains(&only_a),
        "a point carrying only one of two requested tags must NOT match \
         — the filter is AND"
    );
}
