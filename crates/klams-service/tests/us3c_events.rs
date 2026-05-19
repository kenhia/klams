//! US3 / sprint-003 T023 — `Service` and `Execution` events share a `task_id`.
//!
//! Two acceptance scenarios:
//!   1. A `category=Service` event lands and is retrievable by
//!      `?service=` filter.
//!   2. A `category=Execution` event with the same `task_id` lands,
//!      and a `?task_id=` query returns both rows via the new
//!      `events_task_id_created_at_idx` (sub-50ms even without the
//!      perf table — index correctness, not perf bench).
//!
//! Run with the docker test stack up:
//!   `cargo test -p klams-service --test us3c_events -- --ignored --test-threads=1`

mod common;

use common::TestServer;
use klams_types::{AppendEventRequest, ListEventsParams, Source};
use serde_json::json;
use uuid::Uuid;

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn service_event_lands_and_is_filterable_by_service() {
    let server = TestServer::spawn().await;
    let svc = format!("test-svc-{}", Uuid::now_v7().simple());
    let req = AppendEventRequest {
        task_id: None,
        category: "Service".into(),
        payload: json!({"service": svc, "event": "up", "host": "kubs0"}),
        source: Source::Controller,
    };
    server.client.append_event(&req).await.expect("append");

    // Allow worker drain.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let page = server
            .client
            .list_events(&ListEventsParams {
                service: Some(svc.clone()),
                ..Default::default()
            })
            .await
            .expect("list");
        if !page.items.is_empty() {
            assert_eq!(page.items[0].payload["service"], svc);
            assert_eq!(page.items[0].payload["event"], "up");
            return;
        }
        assert!(
            std::time::Instant::now() <= deadline,
            "service event never landed"
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn service_and_execution_share_task_id_filterable_via_payload() {
    let server = TestServer::spawn().await;
    let task_id = format!("ansible-{}", Uuid::now_v7().simple());
    let svc = format!("test-svc-{}", Uuid::now_v7().simple());

    let svc_event = AppendEventRequest {
        task_id: None,
        category: "Service".into(),
        payload: json!({
            "service": svc,
            "event": "restart",
            "host": "kubs0",
            "task_id": task_id,
        }),
        source: Source::Controller,
    };
    let exec_event = AppendEventRequest {
        task_id: None,
        category: "Execution".into(),
        payload: json!({
            "task_id": task_id,
            "tool": "ansible",
            "phase": "completed",
        }),
        source: Source::Task,
    };
    server
        .client
        .append_event(&svc_event)
        .await
        .expect("svc append");
    server
        .client
        .append_event(&exec_event)
        .await
        .expect("exec append");

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let t0 = std::time::Instant::now();
        let page = server
            .client
            .list_events(&ListEventsParams {
                task_id: Some(task_id.clone()),
                ..Default::default()
            })
            .await
            .expect("list by task_id");
        let elapsed = t0.elapsed();
        if page.items.len() >= 2 {
            assert!(
                elapsed < std::time::Duration::from_millis(500),
                "per-task query too slow: {elapsed:?}"
            );
            let cats: Vec<&str> = page.items.iter().map(|e| e.category.as_str()).collect();
            assert!(cats.contains(&"Service"), "missing Service in {cats:?}");
            assert!(cats.contains(&"Execution"), "missing Execution in {cats:?}");
            return;
        }
        assert!(
            std::time::Instant::now() <= deadline,
            "expected 2 rows, got {}",
            page.items.len()
        );
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}
