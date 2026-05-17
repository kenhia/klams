//! Shared integration-test harness.
//!
//! Boots klams-service in-process against the local docker-compose
//! test stack, returning a configured `klams_client::Client` that
//! integration tests can drive.
//!
//! Activate by running `docker compose -f tests/docker-compose.test.yml up -d`
//! and setting environment variables:
//!   `TEST_DATABASE_URL`  (default: <postgres://klams:klams_test@127.0.0.1:55432/klams>)
//!   `TEST_QDRANT_URL`    (default: <http://127.0.0.1:56334>)
//!   `TEST_TEI_URL`       (default: <http://127.0.0.1:57070>)
//!
//! Tests that depend on this harness should be marked `#[ignore]`
//! by default and run explicitly via `cargo test -- --ignored`.

#![allow(dead_code)]

use klams_api::{build_router, ApiState};
use klams_client::Client;
use klams_core::{spawn_workers, MemoryQueue};
use klams_store::{CompositeStore, PostgresStore, QdrantStore, TeiEmbedder};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;

pub struct TestServer {
    pub client: Client,
    pub addr: SocketAddr,
    pub bearer_token: String,
}

impl std::fmt::Debug for TestServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestServer")
            .field("addr", &self.addr)
            .field("bearer_token", &"<redacted>")
            .field("client", &"<klams_client>")
            .finish()
    }
}

impl TestServer {
    pub async fn spawn() -> Self {
        let pg_url = std::env::var("TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://klams:klams_test@127.0.0.1:55432/klams".into());
        let qdrant_url =
            std::env::var("TEST_QDRANT_URL").unwrap_or_else(|_| "http://127.0.0.1:56334".into());
        let tei_url =
            std::env::var("TEST_TEI_URL").unwrap_or_else(|_| "http://127.0.0.1:57070".into());
        let bearer = "test-token-do-not-use-in-prod".to_string();

        let postgres = PostgresStore::connect(&pg_url, 4)
            .await
            .expect("postgres connect");
        let qdrant = QdrantStore::connect(&qdrant_url, "knowledge_items_test", 384)
            .await
            .expect("qdrant connect");
        let embedder = TeiEmbedder::new(tei_url, 384).expect("tei client");
        let store = Arc::new(CompositeStore::new(postgres, qdrant, embedder));

        let (queue, rx) = MemoryQueue::new(256);
        let _workers = spawn_workers(2, rx, Arc::clone(&store));

        let state = ApiState {
            store: Arc::clone(&store),
            queue,
            queue_capacity: 256,
            workers: 2,
            started_at: std::time::Instant::now(),
        };
        let router = build_router(state, bearer.clone());

        let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
        let addr = listener.local_addr().expect("local_addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });

        let client = Client::new(&format!("http://{addr}"), &bearer).expect("client");
        Self {
            client,
            addr,
            bearer_token: bearer,
        }
    }
}
