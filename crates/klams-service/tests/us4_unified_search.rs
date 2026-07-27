//! US4 — unified search across facts, events, and knowledge.
//!
//! Marked `#[ignore]`; run with
//! `cargo test -p klams-service --test us4_unified_search -- --ignored --test-threads=1`.

mod common;

use common::TestServer;
use klams_types::{
    AppendEventRequest, FactType, IndexKnowledgeRequest, SearchRequest, SearchType, Source,
    UpsertFactRequest,
};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn scenario_1_mixed_results_with_all_types() {
    let server = TestServer::spawn().await;
    let nonce = Uuid::now_v7().simple().to_string();
    let needle = format!("orangutan{nonce}");

    // Seed one fact, one event, one knowledge item all containing the
    // unique needle so each backend has at least one matching row.
    server
        .client
        .upsert_fact(&UpsertFactRequest {
            fact_type: FactType::UserFact,
            payload: json!({"name": "us4-search-needle", "note": format!("{needle} fact body")}),
            source: Source::Controller,
            explicit_id: None,
            expected_version: None,
        })
        .await
        .expect("fact");

    server
        .client
        .append_event(&AppendEventRequest {
            task_id: Some(Uuid::now_v7()),
            category: "started".into(),
            payload: json!({"summary": format!("{needle} event body")}),
            source: Source::Controller,
        })
        .await
        .expect("event");

    server
        .client
        .index_knowledge(&IndexKnowledgeRequest {
            text: format!("{needle} knowledge body about primates"),
            source: Source::Controller,
            tags: vec![],
            repo: None,
            file: None,
            machine: None,
            chunk_index: None,
            language: None,
            heading_path: None,
            symbols: vec![],
        })
        .await
        .expect("knowledge");

    // Poll until all three are queryable (workers are async).
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let mut last_kinds: Vec<String> = vec![];
    loop {
        let res = server
            .client
            .search(&SearchRequest {
                query: needle.clone(),
                types: None,
                filters: None,
                top_k: 30,
            })
            .await
            .expect("search");
        last_kinds = res
            .results
            .iter()
            .map(|h| {
                serde_json::to_string(&h.kind)
                    .unwrap()
                    .trim_matches('"')
                    .to_string()
            })
            .collect();
        let has_all = ["fact", "event", "knowledge"]
            .iter()
            .all(|k| last_kinds.iter().any(|x| x == k));
        if has_all || std::time::Instant::now() > deadline {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    for k in ["fact", "event", "knowledge"] {
        assert!(
            last_kinds.iter().any(|x| x == k),
            "unified search missing type {k}; got {last_kinds:?}"
        );
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn scenario_2_types_filter_excludes_other_kinds() {
    let server = TestServer::spawn().await;
    let nonce = Uuid::now_v7().simple().to_string();
    let needle = format!("badger{nonce}");

    server
        .client
        .upsert_fact(&UpsertFactRequest {
            fact_type: FactType::UserFact,
            payload: json!({"name": "us4-search-needle", "note": format!("{needle} fact body")}),
            source: Source::Controller,
            explicit_id: None,
            expected_version: None,
        })
        .await
        .expect("fact");

    server
        .client
        .index_knowledge(&IndexKnowledgeRequest {
            text: format!("{needle} knowledge body"),
            source: Source::Controller,
            tags: vec![],
            repo: None,
            file: None,
            machine: None,
            chunk_index: None,
            language: None,
            heading_path: None,
            symbols: vec![],
        })
        .await
        .expect("knowledge");

    // Wait briefly for ingestion.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;

    let res = server
        .client
        .search(&SearchRequest {
            query: needle.clone(),
            types: Some(vec![SearchType::Knowledge]),
            filters: None,
            top_k: 10,
        })
        .await
        .expect("search");

    assert!(
        !res.results.is_empty(),
        "knowledge filter should return at least one hit"
    );
    for hit in res.results {
        assert!(
            matches!(hit.kind, SearchType::Knowledge),
            "filter=knowledge returned non-knowledge hit"
        );
    }
}
