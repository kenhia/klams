//! Sprint 006 T017 (US1) — Qdrant snapshot integration. Requires
//! `docker-compose.test.yml` (REST API on :56333).

use klams_store::backup::qdrant;
use klams_store::QdrantStore;
use tempfile::tempdir;

fn qdrant_grpc_url() -> String {
    std::env::var("TEST_QDRANT_URL").unwrap_or_else(|_| "http://127.0.0.1:56334".into())
}

fn qdrant_rest_url() -> String {
    // gRPC is 56334; REST is the port one lower (56333) per
    // docker-compose.test.yml.
    std::env::var("TEST_QDRANT_REST_URL").unwrap_or_else(|_| "http://127.0.0.1:56333".into())
}

const TEST_COLLECTION: &str = "klams_backup_snapshot_test";

async fn ensure_collection() {
    // Use the gRPC client to create the collection if absent; the
    // snapshot REST API requires the collection to exist.
    QdrantStore::connect(&qdrant_grpc_url(), TEST_COLLECTION, 384)
        .await
        .expect("create test collection");
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn snapshot_writes_committed_artifact() {
    ensure_collection().await;
    let dir = tempdir().unwrap();
    let artifact = qdrant::snapshot(
        dir.path(),
        &qdrant_rest_url(),
        TEST_COLLECTION,
        "2026-05-23",
        None,
        true, // drop remote copy after download
    )
    .await
    .expect("snapshot succeeded");
    assert_eq!(artifact.path, dir.path().join("qdrant-2026-05-23.snapshot"));
    assert!(artifact.path.exists());
    assert!(
        !dir.path()
            .join("qdrant-2026-05-23.snapshot.partial")
            .exists(),
        ".partial removed after success"
    );
    assert!(artifact.ok);
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn snapshot_bad_collection_returns_err_and_no_committed_file() {
    let dir = tempdir().unwrap();
    let err = qdrant::snapshot(
        dir.path(),
        &qdrant_rest_url(),
        "this-collection-does-not-exist-006",
        "2026-05-23",
        None,
        false,
    )
    .await
    .expect_err("bad collection should fail");
    let msg = format!("{err}");
    assert!(
        msg.contains("404") || msg.contains("Not Found") || msg.contains("snapshot"),
        "unexpected error: {msg}"
    );
    assert!(
        !dir.path().join("qdrant-2026-05-23.snapshot").exists(),
        "no committed file on failure"
    );
}
