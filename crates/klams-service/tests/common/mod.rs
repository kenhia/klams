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
//!
//! # Run the ignored suite with `--test-threads=1`
//!
//! [`TestServer::spawn_isolated`] TRUNCATEs the shared Postgres tables,
//! which wipes rows belonging to any **concurrently running** test built
//! on the non-truncating [`TestServer::spawn`]. Run the suite in
//! parallel and you get failures that vanish on re-run serially — most
//! visibly in `phase4_summarization_pipeline`, whose bundles lose their
//! events mid-test.
//!
//! CI already does the right thing (`--ignored --test-threads=1`), so
//! this is a local-run footgun rather than a live problem. Properly
//! fixing it means giving each isolated test its own schema instead of
//! sharing one database — deliberately out of scope for sprint 025,
//! which only removed the `authors` half of the same hazard (that half
//! had to go, because it broke tests that were *not* running in
//! parallel with a truncating one).

#![allow(dead_code)]

use klams_api::{build_router_with_auth, ApiState};
use klams_client::Client;
use klams_core::{spawn_workers, MemoryQueue};
use klams_store::{CompositeStore, PostgresStore, QdrantStore, TeiEmbedder};
use std::net::SocketAddr;
use std::sync::Arc;
use tokio::net::TcpListener;
use uuid::Uuid;

pub mod fixture;
pub mod seed;

pub type TestStore = CompositeStore;

/// Look up (or create) an author row for a test-token binding, mirroring
/// `main.rs`'s `resolve_token_author`. Idempotent across `spawn_inner`
/// calls that share the Postgres fixture.
async fn resolve_test_author(store: &Arc<CompositeStore>, agent_name: &str) -> Uuid {
    if let Some(a) = store
        .postgres
        .get_author_by_agent_name(agent_name)
        .await
        .expect("lookup bound author")
    {
        return a.id;
    }
    let args = klams_types::RegisterAuthorArgs {
        agent_name: agent_name.to_string(),
        model: None,
        session_title: Some("test harness".into()),
        repo: None,
        client_app: Some("klams-service-tests".into()),
        client_version: None,
        extra: serde_json::json!({}),
    };
    store
        .postgres
        .insert_author(args, None)
        .await
        .expect("insert bound author")
        .id
}

pub struct TestServer {
    pub client: Client,
    pub addr: SocketAddr,
    pub bearer_token: String,
    /// Read-only scope token (sprint 007 T066).
    pub read_token: String,
    /// Write+Read scope token (sprint 007 T066).
    pub write_token: String,
    /// Token bound to a named author (sprint 018 / WI #62) — writes
    /// through it may omit `author_id`.
    pub author_token: String,
    /// The `agent_name` the author-bound token attributes writes to.
    pub author_agent_name: String,
    /// The author id `author_token` is bound to.
    pub bound_author_id: Uuid,
    /// Sprint 025 (#633) — token bound to a *different* author with
    /// only `[read, write]`. Used to prove a write-scoped caller cannot
    /// delete somebody else's memory.
    pub other_write_token: String,
    /// The author id `other_write_token` is bound to.
    pub other_author_id: Uuid,
    /// Sprint 025 (#633) — token bound to a third author carrying
    /// `[read, write, manage]`: cross-author curation is permitted.
    pub manage_token: String,
    /// The author id `manage_token` is bound to.
    pub manage_author_id: Uuid,
    pub store: Arc<TestStore>,
    /// gRPC Qdrant URL — retained so `cleanup()` can drop the
    /// per-test collection on teardown (sprint 009 T039 / FR-021).
    qdrant_url: String,
    /// Qdrant collection name this server bound to. For
    /// `spawn_isolated` this is `klams_test_{uuid}`; for the shared
    /// helpers it is `knowledge_items_test`.
    qdrant_collection: String,
}

impl std::fmt::Debug for TestServer {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TestServer")
            .field("addr", &self.addr)
            .field("bearer_token", &"<redacted>")
            .field("read_token", &"<redacted>")
            .field("write_token", &"<redacted>")
            .field("client", &"<klams_client>")
            .field("store", &"<CompositeStore>")
            .finish_non_exhaustive()
    }
}

impl TestServer {
    pub async fn spawn() -> Self {
        Self::spawn_with_summary_store(false).await
    }

    /// Like `spawn`, but wires `SummaryStore` into `ContextBuilder`
    /// so the events section can substitute raw rows for summaries
    /// (sprint 005 T039 + T033).
    pub async fn spawn_with_summary_store(with_summary_store: bool) -> Self {
        Self::spawn_inner(
            with_summary_store,
            "knowledge_items_test".to_string(),
            false,
        )
        .await
    }

    /// Sprint 009 T039 (FR-021 / SC-008) — per-test isolation.
    /// Creates an ephemeral Qdrant collection `klams_test_{uuid}`
    /// (dropped via [`Self::cleanup`]) and TRUNCATEs the shared
    /// Postgres test tables so concurrent tests cannot observe
    /// each other's facts/events/summaries/dissents/authors.
    /// Seeded `system` + `lost-author` authors are preserved.
    pub async fn spawn_isolated() -> Self {
        let collection = format!("klams_test_{}", Uuid::new_v4().simple());
        Self::spawn_inner(false, collection, true).await
    }

    #[allow(clippy::too_many_lines)]
    async fn spawn_inner(
        with_summary_store: bool,
        qdrant_collection: String,
        truncate_postgres: bool,
    ) -> Self {
        let pg_url = std::env::var("TEST_DATABASE_URL")
            .unwrap_or_else(|_| "postgres://klams:klams_test@127.0.0.1:55432/klams".into());
        let qdrant_url =
            std::env::var("TEST_QDRANT_URL").unwrap_or_else(|_| "http://127.0.0.1:56334".into());
        let tei_url =
            std::env::var("TEST_TEI_URL").unwrap_or_else(|_| "http://127.0.0.1:57070".into());
        let bearer = "test-token-do-not-use-in-prod".to_string();
        let read_token = "test-token-read-only".to_string();
        let write_token = "test-token-write".to_string();

        let postgres = PostgresStore::connect(&pg_url, 4)
            .await
            .expect("postgres connect");
        if truncate_postgres {
            // Wipe per-test mutable state.
            //
            // Sprint 025: the companion `DELETE FROM authors WHERE
            // agent_name NOT IN ('system','lost-author')` was removed.
            // It deleted rows belonging to *other* tests running
            // concurrently, so a second test could have its author
            // pulled out from under it between `register_author` and
            // its first write — surfacing as a `facts_author_id_fkey`
            // violation. The race was always there; it only became
            // likely once this sprint added more `spawn_isolated`
            // tests. Nothing depends on a clean `authors` table: the
            // v1 listing test filters by `agent_name` and matches on
            // id, and every test picks a unique name.
            sqlx::query(
                "TRUNCATE TABLE facts, events, summaries, dissents RESTART IDENTITY CASCADE",
            )
            .execute(postgres.pool())
            .await
            .expect("truncate postgres");
        }
        let qdrant = QdrantStore::connect(&qdrant_url, &qdrant_collection, 384)
            .await
            .expect("qdrant connect");
        let embedder = Arc::new(TeiEmbedder::new(tei_url, 384).expect("tei client"));
        let store = Arc::new(CompositeStore::new(postgres, qdrant, embedder));

        // Sprint 018 (WI #62) — mirror main.rs's resolve_token_author:
        // bind a test token to a named author so bearer-fallback
        // attribution is exercisable.
        let author_token = "test-token-author-bound".to_string();
        let author_agent_name = "bearer-bound-test-agent".to_string();
        let bound_author_id = resolve_test_author(&store, &author_agent_name).await;

        // Sprint 025 (#633): two more bound identities so ownership can
        // be exercised — one write-only peer, one manage-scoped curator.
        let other_write_token = "test-token-other-write".to_string();
        let other_author_id = resolve_test_author(&store, "other-write-test-agent").await;
        let manage_token = "test-token-manage".to_string();
        let manage_author_id = resolve_test_author(&store, "manage-test-agent").await;

        let (queue, rx) = MemoryQueue::new(256);
        let _workers = spawn_workers(2, rx, Arc::clone(&store));

        let mut builder = klams_core::context::ContextBuilder::new(
            klams_core::tokens::TokenCounter::new(klams_core::tokens::TokenMode::CharsDiv4),
            100,
        );
        if with_summary_store {
            builder = builder
                .with_summary_store(Arc::clone(&store) as Arc<dyn klams_store::SummaryStore>);
        }

        let state = ApiState {
            store: Arc::clone(&store),
            api: klams_types::ApiConfig::default(),
            queue,
            queue_capacity: 256,
            workers: 2,
            started_at: std::time::Instant::now(),
            validators: Arc::new(klams_core::ValidatorRegistry::with_defaults()),
            context_builder: Arc::new(builder),
            maintenance: klams_types::MaintenanceState::default(),
            embed_limit: klams_types::EmbedLimit::default(),
        };
        let router = {
            // Mirror main.rs's wiring: single AuthState gates both REST
            // and the nested /mcp router. Sprint 007 T064.
            let grants = vec![
                klams_api::auth::TokenGrant::new(
                    bearer.clone(),
                    vec![
                        klams_types::Scope::Read,
                        klams_types::Scope::Write,
                        klams_types::Scope::Manage,
                        klams_types::Scope::Admin,
                    ],
                    Some("legacy".into()),
                ),
                klams_api::auth::TokenGrant::new(
                    read_token.clone(),
                    vec![klams_types::Scope::Read],
                    Some("read-only".into()),
                ),
                klams_api::auth::TokenGrant::new(
                    write_token.clone(),
                    vec![klams_types::Scope::Read, klams_types::Scope::Write],
                    Some("write".into()),
                ),
                klams_api::auth::TokenGrant::new_with_author(
                    author_token.clone(),
                    vec![klams_types::Scope::Read, klams_types::Scope::Write],
                    Some("author-bound".into()),
                    bound_author_id,
                    author_agent_name.clone(),
                ),
                klams_api::auth::TokenGrant::new_with_author(
                    other_write_token.clone(),
                    vec![klams_types::Scope::Read, klams_types::Scope::Write],
                    Some("other-write".into()),
                    other_author_id,
                    "other-write-test-agent",
                ),
                klams_api::auth::TokenGrant::new_with_author(
                    manage_token.clone(),
                    vec![
                        klams_types::Scope::Read,
                        klams_types::Scope::Write,
                        klams_types::Scope::Manage,
                    ],
                    Some("manage".into()),
                    manage_author_id,
                    "manage-test-agent",
                ),
            ];
            let auth_state = klams_api::auth::AuthState::with_grants(grants);
            let mcp_state = klams_mcp::tools::McpState::new(
                Arc::clone(&store),
                std::sync::Arc::new(klams_types::MaintenanceState::default()),
                klams_types::ApiConfig::default(),
            );
            let mcp_router = klams_mcp::router(mcp_state, Vec::new()).layer(
                axum::middleware::from_fn_with_state(auth_state.clone(), klams_api::require_bearer),
            );
            build_router_with_auth(state, auth_state).nest("/mcp", mcp_router)
        };

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
            read_token,
            write_token,
            author_token,
            author_agent_name,
            bound_author_id,
            other_write_token,
            other_author_id,
            manage_token,
            manage_author_id,
            store,
            qdrant_url,
            qdrant_collection,
        }
    }

    /// Sprint 009 T039 — drop the per-test Qdrant collection.
    /// Safe to call on a `spawn()`-built server too; it will skip
    /// the shared `knowledge_items_test` collection.
    pub async fn cleanup(self) {
        if self.qdrant_collection == "knowledge_items_test" {
            return;
        }
        let client = qdrant_client::Qdrant::from_url(&self.qdrant_url)
            .build()
            .expect("qdrant client for cleanup");
        let _ = client.delete_collection(self.qdrant_collection).await;
    }
}

/// A live MCP Streamable-HTTP session against the spawned test server.
///
/// Sprint 025: hoisted here from `mcp_bearer_author.rs` so the
/// authorization tests can drive the *real* transport — the ownership
/// checks read caller identity out of request extensions that only the
/// HTTP path populates, so calling `tools::*::run` directly would test
/// the wrong thing.
pub struct McpSession {
    client: reqwest::Client,
    base: String,
    token: String,
    session_id: String,
}

const INIT_BODY: &str = r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2024-11-05","capabilities":{},"clientInfo":{"name":"probe","version":"0"}}}"#;

/// Parse a Streamable-HTTP SSE (or bare-JSON) body into its JSON-RPC payload.
fn parse_sse_json(body: &str) -> serde_json::Value {
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(body.trim()) {
        return v;
    }
    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("data: ") {
            if let Ok(v) = serde_json::from_str::<serde_json::Value>(rest) {
                return v;
            }
        }
    }
    panic!("no JSON / data: line in body (len={}):\n{body}", body.len());
}

impl McpSession {
    pub async fn handshake(addr: SocketAddr, token: &str) -> Self {
        let client = reqwest::Client::new();
        let base = format!("http://{addr}/mcp");
        let init = client
            .post(&base)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Authorization", format!("Bearer {token}"))
            .body(INIT_BODY)
            .send()
            .await
            .expect("initialize");
        assert_eq!(init.status(), reqwest::StatusCode::OK, "initialize ok");
        let session_id = init
            .headers()
            .get("mcp-session-id")
            .expect("mcp-session-id header")
            .to_str()
            .unwrap()
            .to_string();
        let _ = init.text().await;
        let notif = client
            .post(&base)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Authorization", format!("Bearer {token}"))
            .header("mcp-session-id", &session_id)
            .body(r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#)
            .send()
            .await
            .expect("initialized notify");
        assert!(notif.status().is_success(), "initialized notify ok");
        Self {
            client,
            base,
            token: token.to_string(),
            session_id,
        }
    }

    /// Call a tool; returns the parsed JSON the tool put in
    /// `result.content[0].text`. Errors come back as tool results too
    /// (with an `error_code` in `meta`), not as transport failures.
    pub async fn call_tool(&self, name: &str, arguments: serde_json::Value) -> serde_json::Value {
        let body = serde_json::json!({
            "jsonrpc": "2.0",
            "id": 2,
            "method": "tools/call",
            "params": {"name": name, "arguments": arguments},
        });
        let resp = self
            .client
            .post(&self.base)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("mcp-session-id", &self.session_id)
            .body(body.to_string())
            .send()
            .await
            .expect("tools/call");
        assert!(resp.status().is_success(), "tools/call http ok");
        let rpc = parse_sse_json(&resp.text().await.expect("body"));
        let text = rpc["result"]["content"][0]["text"]
            .as_str()
            .unwrap_or_else(|| panic!("unexpected tools/call response: {rpc}"));
        serde_json::from_str(text).expect("tool result JSON")
    }

    /// The `error_code` of a tool result that failed, or `None` if it
    /// succeeded. The envelope serializes the meta block as `_meta` on
    /// the wire, so read that.
    pub fn error_code(v: &serde_json::Value) -> Option<&str> {
        v["_meta"]["error_code"].as_str()
    }

    /// Names of the tools this session's token may see.
    pub async fn list_tool_names(&self) -> Vec<String> {
        let body = serde_json::json!({
            "jsonrpc": "2.0", "id": 3, "method": "tools/list", "params": {}
        });
        let resp = self
            .client
            .post(&self.base)
            .header("Content-Type", "application/json")
            .header("Accept", "application/json, text/event-stream")
            .header("Authorization", format!("Bearer {}", self.token))
            .header("mcp-session-id", &self.session_id)
            .body(body.to_string())
            .send()
            .await
            .expect("tools/list");
        let rpc = parse_sse_json(&resp.text().await.expect("body"));
        rpc["result"]["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|t| t["name"].as_str().unwrap_or_default().to_string())
            .collect()
    }
}
