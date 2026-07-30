//! Sprint 018 (WI #305) — live rmcp session termination returns 204.
//!
//! Drives a real MCP handshake against the in-process server, then
//! terminates the session with HTTP DELETE and asserts the status the
//! mcp python-sdk requires (200/204 — we standardize on 204). Without
//! the `delete_status_compat` layer, rmcp answers 202 and every
//! python client logs `Session termination failed: 202` on close.
//!
//! Marked `#[ignore]` like the rest of the integration suite — run via
//!   `cargo test -p klams-service --test mcp_session_delete -- --ignored`
//! after `docker compose -f tests/docker-compose.test.yml up -d`.

mod common;

use common::{TestServer, INIT_BODY};

#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn session_termination_delete_returns_204() {
    let server = TestServer::spawn().await;
    let client = reqwest::Client::new();
    let base = format!("http://{}/mcp", server.addr);

    let init = client
        .post(&base)
        .header("Content-Type", "application/json")
        .header("Accept", "application/json, text/event-stream")
        .header("Authorization", format!("Bearer {}", server.bearer_token))
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

    let del = client
        .delete(&base)
        .header("mcp-session-id", &session_id)
        .header("Authorization", format!("Bearer {}", server.bearer_token))
        .send()
        .await
        .expect("DELETE /mcp");
    assert_eq!(
        del.status(),
        reqwest::StatusCode::NO_CONTENT,
        "python-sdk accepts only 200/204 for session termination"
    );
    let body = del.text().await.expect("body");
    assert!(body.is_empty(), "204 must have an empty body, got: {body}");
}

#[ignore = "requires docker compose test stack"]
#[tokio::test]
async fn session_delete_without_session_id_stays_400() {
    let server = TestServer::spawn().await;
    let del = reqwest::Client::new()
        .delete(format!("http://{}/mcp", server.addr))
        .header("Authorization", format!("Bearer {}", server.bearer_token))
        .send()
        .await
        .expect("DELETE /mcp");
    assert_eq!(del.status(), reqwest::StatusCode::BAD_REQUEST);
}
