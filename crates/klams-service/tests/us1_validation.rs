//! US1 validation e2e tests (T014).
//!
//! Runs against the docker-compose test stack. Marked `#[ignore]`
//! so plain `cargo test` skips it; invoke with
//! `cargo test -p klams-service --test us1_validation -- --ignored`.

mod common;

use common::TestServer;
use klams_client::ClientError;
use klams_types::{AppendEventRequest, FactType, Source, UpsertFactRequest};
use serde_json::json;

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn missing_name_userfact_is_422() {
    let server = TestServer::spawn().await;
    let err = server
        .client
        .upsert_fact(&UpsertFactRequest {
            fact_type: FactType::UserFact,
            payload: json!({}),
            source: Source::Controller,
            explicit_id: None,
            expected_version: None,
        })
        .await
        .unwrap_err();
    match err {
        ClientError::Api { status, body } => {
            assert_eq!(status.as_u16(), 422);
            assert_eq!(body.code, "validation_error");
        }
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn hostname_shape_rejected_even_for_user_source() {
    let server = TestServer::spawn().await;
    let err = server
        .client
        .upsert_fact(&UpsertFactRequest {
            fact_type: FactType::UserFact,
            payload: json!({"name": "Ada", "hostname": "BAD HOST"}),
            source: Source::User,
            explicit_id: None,
            expected_version: None,
        })
        .await
        .unwrap_err();
    match err {
        ClientError::Api { status, .. } => assert_eq!(status.as_u16(), 422),
        other => panic!("expected Api error, got {other:?}"),
    }
}

#[tokio::test]
#[ignore = "requires docker-compose.test.yml"]
async fn far_future_event_rejected() {
    let server = TestServer::spawn().await;
    let err = server
        .client
        .append_event(&AppendEventRequest {
            task_id: None,
            category: "service".into(),
            payload: json!({
                "hostname": "kubs0",
                "name": "klams",
                "state": "up",
                "observed_at": "3030-01-01T00:00:00Z"
            }),
            source: Source::Controller,
        })
        .await
        .unwrap_err();
    match err {
        ClientError::Api { status, .. } => assert_eq!(status.as_u16(), 422),
        other => panic!("expected Api error, got {other:?}"),
    }
}
