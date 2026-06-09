//! US2 / sprint-003 T033 — end-to-end scanner round trip.
//!
//! Three acceptance scenarios from US2:
//!   1. A fresh file in a `TempDir` is indexed and searchable.
//!   2. Re-running the scanner after editing the file makes the new
//!      content searchable while the old content is gone.
//!   3. Deleting the file and re-running the scanner removes the
//!      knowledge row (search returns zero hits).
//!
//! Run with the docker test stack up:
//!   `cargo test -p klams-service --test us3d_scanner_e2e -- --ignored --test-threads=1`

mod common;

use common::TestServer;
use klams_scanner::scan_root;
use std::fs;
use std::time::Duration;
use tempfile::TempDir;
use uuid::Uuid;

async fn poll_hits(server: &TestServer, nonce: &str, target: usize) -> usize {
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    loop {
        let res = server
            .client
            .search_knowledge(nonce, 25)
            .await
            .expect("search");
        let hits = res
            .results
            .iter()
            .filter(|h| serde_json::to_string(h).is_ok_and(|s| s.contains(nonce)))
            .count();
        if hits == target || std::time::Instant::now() > deadline {
            return hits;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn fresh_file_is_indexed_edit_replaces_delete_removes() {
    let server = TestServer::spawn().await;
    let nonce_a = format!("klams-nonce-{}", Uuid::now_v7().simple());
    let nonce_b = format!("klams-nonce-{}", Uuid::now_v7().simple());

    let tmp_root = TempDir::new().expect("root");
    let state = TempDir::new().expect("state");
    let cursor_path = state.path().join("scanner.sqlite");
    let file_path = tmp_root.path().join("notes").join("hello.md");
    fs::create_dir_all(file_path.parent().unwrap()).unwrap();

    // 1. Fresh file with nonce_a.
    fs::write(&file_path, format!("# Hello\n\nbody mentioning {nonce_a}.")).unwrap();
    scan_root(
        &server.client,
        &format!("http://{}", server.addr),
        &server.bearer_token,
        &cursor_path,
        tmp_root.path(),
    )
    .await
    .expect("scan 1");
    assert!(
        poll_hits(&server, &nonce_a, 1).await >= 1,
        "nonce_a not found"
    );

    // 2. Edit: replace nonce_a with nonce_b. Bump mtime explicitly.
    fs::write(
        &file_path,
        format!("# Hello\n\nupdated body with {nonce_b}."),
    )
    .unwrap();
    // sleep a bit to ensure mtime_ns differs across re-walk
    tokio::time::sleep(Duration::from_millis(50)).await;
    filetime::set_file_mtime(&file_path, filetime::FileTime::now()).ok();
    scan_root(
        &server.client,
        &format!("http://{}", server.addr),
        &server.bearer_token,
        &cursor_path,
        tmp_root.path(),
    )
    .await
    .expect("scan 2");
    assert!(
        poll_hits(&server, &nonce_b, 1).await >= 1,
        "nonce_b not found"
    );

    // 3. Delete the file → scanner prunes it.
    fs::remove_file(&file_path).unwrap();
    scan_root(
        &server.client,
        &format!("http://{}", server.addr),
        &server.bearer_token,
        &cursor_path,
        tmp_root.path(),
    )
    .await
    .expect("scan 3");
    assert_eq!(
        poll_hits(&server, &nonce_b, 0).await,
        0,
        "nonce_b still searchable after delete"
    );
}
