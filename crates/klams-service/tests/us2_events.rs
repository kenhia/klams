//! US2: append and query task events end-to-end.
//!
//! Marked `#[ignore]`; run with
//! `cargo test -p klams-service --test us2_events -- --ignored`.

mod common;

use common::TestServer;
use klams_types::{AppendEventRequest, ListEventsParams, Source};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn scenario_1_filtered_query_returns_ordered_subset() {
    let server = TestServer::spawn().await;
    let task_a = Uuid::now_v7();
    let task_b = Uuid::now_v7();

    let mk = |task_id: Option<Uuid>, category: &str, n: u32| AppendEventRequest {
        task_id,
        category: category.into(),
        payload: json!({"n": n}),
        source: Source::Controller,
    };

    let _ = server
        .client
        .append_event(&mk(Some(task_a), "started", 1))
        .await
        .expect("e1");
    let _ = server
        .client
        .append_event(&mk(Some(task_b), "started", 2))
        .await
        .expect("e2");
    let _ = server
        .client
        .append_event(&mk(Some(task_a), "progress", 3))
        .await
        .expect("e3");
    let _ = server
        .client
        .append_event(&mk(Some(task_a), "completed", 4))
        .await
        .expect("e4");

    let page = poll_events(&server, task_a, 3).await;

    let mut ns: Vec<i64> = page
        .items
        .iter()
        .map(|e| e.payload["n"].as_i64().unwrap())
        .collect();
    ns.sort_unstable();
    assert_eq!(
        ns,
        vec![1, 3, 4],
        "task_a events only (filter excludes task_b)"
    );

    // task_b's event must NOT appear in this filtered view.
    assert!(
        !page.items.iter().any(|e| e.payload["n"] == 2),
        "task_b event leaked into task_a filter"
    );

    // Per spec: results ordered ascending by `created_at`. Verify the
    // returned slice is monotonically non-decreasing in that field.
    let times: Vec<_> = page.items.iter().map(|e| e.created_at).collect();
    assert!(
        times.windows(2).all(|w| w[0] <= w[1]),
        "events must be ordered by created_at ASC, got {times:?}"
    );
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn scenario_2_no_update_or_delete_endpoint() {
    let server = TestServer::spawn().await;
    // Direct HTTP — the typed client doesn't even expose these verbs.
    let url = format!("http://{}/memory/events", server.addr);
    let http = reqwest::Client::new();

    for method in [
        reqwest::Method::PUT,
        reqwest::Method::PATCH,
        reqwest::Method::DELETE,
    ] {
        let resp = http
            .request(method.clone(), &url)
            .bearer_auth(&server.bearer_token)
            .send()
            .await
            .expect("send");
        assert_eq!(
            resp.status(),
            reqwest::StatusCode::METHOD_NOT_ALLOWED,
            "{method} /memory/events must be 405"
        );
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn scenario_3_events_survive_restart() {
    let task_id = Uuid::now_v7();
    let nonce = Uuid::now_v7().to_string();

    {
        let server = TestServer::spawn().await;
        for n in 0..3 {
            server
                .client
                .append_event(&AppendEventRequest {
                    task_id: Some(task_id),
                    category: "progress".into(),
                    payload: json!({"n": n, "nonce": nonce}),
                    source: Source::Controller,
                })
                .await
                .expect("append");
        }
    }

    let server2 = TestServer::spawn().await;
    let page = poll_events(&server2, task_id, 3).await;

    let matching: Vec<_> = page
        .items
        .iter()
        .filter(|e| e.payload["nonce"] == nonce)
        .collect();
    assert_eq!(matching.len(), 3, "all three events should survive restart");
}

/// Poll `list_events(task_id=...)` up to ~2s waiting for `expected`
/// events to appear (compensates for fire-and-forget enqueue).
async fn poll_events(
    server: &TestServer,
    task_id: Uuid,
    expected: usize,
) -> klams_types::EventPage {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let page = server
            .client
            .list_events(&ListEventsParams {
                task_id: Some(task_id.to_string()),
                ..Default::default()
            })
            .await
            .expect("list");
        if page.items.len() >= expected || std::time::Instant::now() > deadline {
            return page;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
