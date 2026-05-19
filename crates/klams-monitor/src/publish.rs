//! Thin shim over [`klams_client::Client`] that converts a
//! [`crate::state::ServiceEventPayload`] into an `AppendEventRequest`
//! and posts it to `POST /memory/events`. Kept in its own module so
//! the diff layer and the HTTP plumbing stay independently testable.

use crate::state::ServiceEventPayload;
use anyhow::{Context, Result};
use klams_client::Client;
use klams_types::{AppendEventRequest, Source};

pub async fn publish(client: &Client, payload: &ServiceEventPayload) -> Result<()> {
    let req = AppendEventRequest {
        task_id: None,
        category: "Service".into(),
        payload: serde_json::to_value(payload).context("serialize ServiceEventPayload")?,
        source: Source::Controller,
    };
    client
        .append_event(&req)
        .await
        .context("POST /memory/events")?;
    Ok(())
}
