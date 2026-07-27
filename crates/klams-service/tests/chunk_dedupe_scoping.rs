//! Sprint 028 (#642) — content-only dedupe with copy bookkeeping.
//!
//! One point per content hash: identical content in two files, or on two
//! hosts, is ONE point whose `copies`/`machines` payload records every
//! location. Host-scoped delete became bookkeeping — removing a
//! (host, file) copy deletes the point only when it was the last copy.
//!
//! This file previously pinned the opposite (sprint 022 #324 / 023 #408
//! per-file/per-host point identity); those semantics were deliberately
//! unwound by Ken's ruling on #641/#642: metadata differences never keep
//! two copies apart, and duplicate storage wasted ~49% of the corpus.
//!
//! Run with the docker test stack up:
//!   `cargo test -p klams-service --test chunk_dedupe_scoping -- --ignored --test-threads=1`

mod common;

use common::TestServer;
use klams_types::{IndexKnowledgeRequest, Source};
use std::time::Duration;
use uuid::Uuid;

fn req_on(text: &str, file: &str, machine: Option<&str>) -> IndexKnowledgeRequest {
    IndexKnowledgeRequest {
        text: text.to_string(),
        source: Source::Task,
        tags: vec![],
        repo: Some("repo".into()),
        file: Some(file.to_string()),
        machine: machine.map(str::to_string),
        chunk_index: Some(0),
        language: None,
        heading_path: None,
        symbols: vec![],
    }
}

async fn wait_committed(server: &TestServer, id: Uuid) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        if server.client.get_knowledge(id).await.is_ok() {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "point never committed"
        );
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn identical_chunk_in_two_files_is_one_point() {
    let server = TestServer::spawn_isolated().await;
    let nonce = Uuid::now_v7().simple().to_string();
    let text = format!("shared boilerplate chunk {nonce} present in two files verbatim");

    let a = server
        .client
        .index_knowledge(&req_on(&text, "/src/a.rs", Some("kubs0")))
        .await
        .expect("index A");
    assert!(!a.deduped, "first insert must not dedupe");
    wait_committed(&server, a.knowledge_id).await;

    // File B, identical text: same point, deduped.
    let b = server
        .client
        .index_knowledge(&req_on(&text, "/src/b.rs", Some("kubs0")))
        .await
        .expect("index B");
    assert!(
        b.deduped,
        "identical content must collapse to one point (#642)"
    );
    assert_eq!(a.knowledge_id, b.knowledge_id);

    // Deleting file A's copy must NOT drop the point — file B still
    // holds the content. Deleting B's copy then removes it.
    let d1 = server
        .client
        .delete_knowledge_by_source_file("/src/a.rs", Some("kubs0"))
        .await
        .expect("delete A");
    assert_eq!(d1.deleted, 1, "one copy removed");
    assert!(
        server.client.get_knowledge(a.knowledge_id).await.is_ok(),
        "point must survive while another file holds the content"
    );

    let d2 = server
        .client
        .delete_knowledge_by_source_file("/src/b.rs", Some("kubs0"))
        .await
        .expect("delete B");
    assert_eq!(d2.deleted, 1);
    assert!(
        server.client.get_knowledge(a.knowledge_id).await.is_err(),
        "last copy gone — the point must be deleted"
    );

    server.cleanup().await;
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn same_path_on_two_hosts_is_one_point_listing_both_machines() {
    let server = TestServer::spawn_isolated().await;
    let nonce = Uuid::now_v7().simple().to_string();
    let text = format!("shared path chunk {nonce} on two hosts");
    let path = "/home/ken/src/shared.rs";

    let a = server
        .client
        .index_knowledge(&req_on(&text, path, Some("kubs0")))
        .await
        .expect("index kubs0");
    assert!(!a.deduped);
    wait_committed(&server, a.knowledge_id).await;

    let b = server
        .client
        .index_knowledge(&req_on(&text, path, Some("kai")))
        .await
        .expect("index kai");
    assert!(
        b.deduped,
        "same content on a second host must dedupe (#642)"
    );
    assert_eq!(a.knowledge_id, b.knowledge_id);

    let item = server
        .client
        .get_knowledge(a.knowledge_id)
        .await
        .expect("get shared point");
    assert_eq!(
        item.machines,
        vec!["kubs0".to_string(), "kai".to_string()],
        "the shared point must list every host holding the content"
    );

    server.cleanup().await;
}

/// The acceptance case from #642: a host removed from scanning (its file
/// vanished, or delete-before-reindex ran) disappears from `machines`
/// without disturbing the other host's listing.
#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn deleting_one_hosts_copy_keeps_the_other_hosts_listing() {
    let server = TestServer::spawn_isolated().await;
    let nonce = Uuid::now_v7().simple().to_string();
    let text = format!("cross host chunk {nonce} for delete scoping");
    let path = "/home/ken/src/lib.rs";

    let a = server
        .client
        .index_knowledge(&req_on(&text, path, Some("kubs0")))
        .await
        .expect("index kubs0");
    wait_committed(&server, a.knowledge_id).await;
    let b = server
        .client
        .index_knowledge(&req_on(&text, path, Some("kai")))
        .await
        .expect("index kai");
    assert_eq!(a.knowledge_id, b.knowledge_id);

    // kai edits the file → delete-before-reindex fires for kai only.
    let d = server
        .client
        .delete_knowledge_by_source_file(path, Some("kai"))
        .await
        .expect("delete kai copy");
    assert_eq!(d.deleted, 1);

    let item = server
        .client
        .get_knowledge(a.knowledge_id)
        .await
        .expect("point must survive for kubs0");
    assert_eq!(item.machines, vec!["kubs0".to_string()]);
    assert_eq!(item.machine.as_deref(), Some("kubs0"));

    // kubs0's copy is the last one; removing it deletes the point.
    let d = server
        .client
        .delete_knowledge_by_source_file(path, Some("kubs0"))
        .await
        .expect("delete kubs0 copy");
    assert_eq!(d.deleted, 1);
    assert!(server.client.get_knowledge(a.knowledge_id).await.is_err());

    server.cleanup().await;
}
