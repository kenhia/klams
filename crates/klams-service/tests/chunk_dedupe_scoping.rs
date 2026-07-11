//! Sprint 022 (#324) — content-hash dedupe is scoped per source file.
//!
//! An identical chunk in two different files must become two distinct
//! points, so deleting one file can't drop a chunk still live in the
//! other. Before the fix the probe was global and the second file's
//! chunk deduped onto the first file's point.
//!
//! Run with the docker test stack up:
//!   `cargo test -p klams-service --test chunk_dedupe_scoping -- --ignored --test-threads=1`

mod common;

use common::TestServer;
use klams_types::{IndexKnowledgeRequest, Source};
use std::time::Duration;
use uuid::Uuid;

fn req(text: &str, file: &str) -> IndexKnowledgeRequest {
    IndexKnowledgeRequest {
        text: text.to_string(),
        source: Source::Task,
        tags: vec![],
        repo: Some("repo".into()),
        file: Some(file.to_string()),
        machine: None,
        chunk_index: Some(0),
        language: None,
        heading_path: None,
        symbols: vec![],
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn identical_chunk_in_two_files_is_two_points() {
    let server = TestServer::spawn_isolated().await;
    let nonce = Uuid::now_v7().simple().to_string();
    let text = format!("shared boilerplate chunk {nonce} present in two files verbatim");

    // File A.
    let a = server
        .client
        .index_knowledge(&req(&text, "/src/a.rs"))
        .await
        .expect("index A");
    assert!(!a.deduped, "first insert must not dedupe");

    // Wait until A's point is committed to Qdrant so the dedupe probe
    // for B can actually observe it — otherwise the test would pass
    // even with a (broken) global probe, by race.
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if server.client.get_knowledge(a.knowledge_id).await.is_ok() {
            break;
        }
        assert!(std::time::Instant::now() < deadline, "A never committed");
        tokio::time::sleep(Duration::from_millis(100)).await;
    }

    // File B, identical text. With per-file scoping this is a NEW point.
    let b = server
        .client
        .index_knowledge(&req(&text, "/src/b.rs"))
        .await
        .expect("index B");
    assert!(
        !b.deduped,
        "identical chunk in a different file must not dedupe (#324)"
    );
    assert_ne!(
        a.knowledge_id, b.knowledge_id,
        "the two files must own distinct points"
    );

    server.cleanup().await;
}
