//! US3 — index, dedupe, search, restart of knowledge items.
//!
//! Marked `#[ignore]`; run with
//! `cargo test -p klams-service --test us3_knowledge -- --ignored --test-threads=1`.

mod common;

use common::TestServer;
use klams_types::{IndexKnowledgeRequest, SearchRequest, SearchType, Source};
use uuid::Uuid;

fn req(text: &str, tags: Vec<&str>) -> IndexKnowledgeRequest {
    IndexKnowledgeRequest {
        text: text.into(),
        source: Source::Controller,
        tags: tags.into_iter().map(String::from).collect(),
        repo: None,
        file: None,
        machine: None,
        chunk_index: None,
        language: None,
        heading_path: None,
        symbols: vec![],
    }
}

async fn poll_get_knowledge(server: &TestServer, id: Uuid) -> klams_types::KnowledgeItem {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        if let Ok(item) = server.client.get_knowledge(id).await {
            return item;
        }
        assert!(
            std::time::Instant::now() <= deadline,
            "get_knowledge({id}) never succeeded"
        );
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
}

async fn poll_knowledge_search(server: &TestServer, query: &str, expected: usize) -> usize {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        let r = server
            .client
            .search(&SearchRequest {
                query: query.into(),
                types: Some(vec![SearchType::Knowledge]),
                filters: None,
                top_k: 10,
            })
            .await
            .expect("search");
        if r.results.len() >= expected || std::time::Instant::now() > deadline {
            return r.results.len();
        }
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn scenario_1_indexed_item_is_searchable_within_10s() {
    let server = TestServer::spawn().await;
    let nonce = Uuid::now_v7().to_string();
    let text = format!("zebras enjoy melted brie cheese {nonce}");
    let resp = server
        .client
        .index_knowledge(&req(&text, vec!["test"]))
        .await
        .expect("index");
    assert!(!resp.deduped);

    let found = poll_knowledge_search(&server, &text, 1).await;
    assert!(
        found >= 1,
        "indexed knowledge must be searchable within 10s"
    );
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn scenario_2_identical_text_is_deduped() {
    let server = TestServer::spawn().await;
    let text = format!("dedupe-me-exactly {}", Uuid::now_v7());
    let r1 = server
        .client
        .index_knowledge(&req(&text, vec![]))
        .await
        .expect("r1");
    assert!(!r1.deduped);

    // Wait for first to land in qdrant so content-hash lookup hits.
    let _ = poll_get_knowledge(&server, r1.knowledge_id).await;

    let r2 = server
        .client
        .index_knowledge(&req(&text, vec![]))
        .await
        .expect("r2");
    assert!(r2.deduped, "second identical submission must be deduped");
    assert_eq!(r1.knowledge_id, r2.knowledge_id);
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn scenario_3_get_by_id_returns_indexed_item() {
    let server = TestServer::spawn().await;
    let text = format!("retrievable knowledge {}", Uuid::now_v7());
    let resp = server
        .client
        .index_knowledge(&req(&text, vec!["x"]))
        .await
        .expect("index");

    // Poll get_knowledge until the worker persists it.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    loop {
        match server.client.get_knowledge(resp.knowledge_id).await {
            Ok(item) => {
                assert_eq!(item.id, resp.knowledge_id);
                assert!(item.text.contains("retrievable knowledge"));
                return;
            }
            Err(_) if std::time::Instant::now() < deadline => {
                tokio::time::sleep(std::time::Duration::from_millis(150)).await;
            }
            Err(e) => panic!("get_knowledge failed: {e}"),
        }
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn scenario_4_knowledge_survives_restart() {
    let nonce = Uuid::now_v7().to_string();
    let text = format!("persistence test {nonce}");
    let id = {
        let server = TestServer::spawn().await;
        let r = server
            .client
            .index_knowledge(&req(&text, vec![]))
            .await
            .expect("index");
        let _ = poll_knowledge_search(&server, &text, 1).await;
        r.knowledge_id
    };

    let server2 = TestServer::spawn().await;
    // Allow time for any in-flight worker.
    let _ = poll_knowledge_search(&server2, &text, 1).await;
    let item = server2
        .client
        .get_knowledge(id)
        .await
        .expect("get after restart");
    assert_eq!(item.id, id);
}
