//! Wraps `klams-client` with the two HTTP calls the scanner needs:
//! `index_knowledge` (one POST per chunk) and `delete_knowledge`
//! (one POST per vanished file).

use crate::metrics;
use anyhow::{Context, Result};
use klams_client::{Client, ClientError};
use klams_types::{IndexKnowledgeRequest, Source};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use std::time::Duration;

/// Aggressive, drain-oriented backoff for the service's `503 queue_full`
/// backpressure. The scanner is a background task, so completeness beats
/// latency: we wait long enough for the bounded write queue to actually
/// drain (the 4 workers each do embed + qdrant + postgres) rather than
/// racing to refill the few slots that just opened. Worst case ~3 min of
/// sleeping per chunk before we give up and leave the file's cursor
/// unadvanced so the next scan retries it.
const RETRY_MAX_ATTEMPTS: u32 = 12;
const RETRY_INITIAL_DELAY: Duration = Duration::from_secs(2);
const RETRY_MAX_DELAY: Duration = Duration::from_secs(60);

pub async fn publish_chunk(
    client: &Client,
    host: &str,
    repo: &str,
    source_file: &str,
    chunk: &crate::chunk::Chunk,
) -> Result<()> {
    let req = IndexKnowledgeRequest {
        text: chunk.text.clone(),
        source: Source::Task,
        tags: vec![],
        repo: Some(repo.to_owned()),
        file: Some(source_file.to_owned()),
        // Sprint 023 (#407): stamp the host so retrieval is
        // fully-qualified as (host, file) and deletes are host-scoped.
        machine: Some(host.to_owned()),
        // Sprint 022 (#322) — carry chunk structure to the store payload.
        chunk_index: Some(chunk.index),
        language: chunk.language.clone(),
        heading_path: chunk.heading_path.clone(),
        symbols: chunk.symbols.clone(),
    };

    let mut delay = RETRY_INITIAL_DELAY;
    for attempt in 1..=RETRY_MAX_ATTEMPTS {
        match client.index_knowledge(&req).await {
            Ok(_) => return Ok(()),
            Err(ClientError::Api { status, .. }) if status.as_u16() == 503 => {
                if attempt == RETRY_MAX_ATTEMPTS {
                    anyhow::bail!(
                        "POST /memory/knowledge/index: queue_full after {attempt} attempts"
                    );
                }
                metrics::incr_retry("queue_full");
                tracing::debug!(
                    path = %source_file,
                    attempt,
                    delay_ms = delay.as_millis(),
                    "queue_full; backing off to let the write queue drain"
                );
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(RETRY_MAX_DELAY);
            }
            Err(e) => {
                return Err(anyhow::Error::new(e).context("POST /memory/knowledge/index"));
            }
        }
    }
    // The loop returns or bails on the final attempt.
    unreachable!("publish_chunk retry loop exhausted without returning")
}

/// `POST /memory/knowledge/delete?source_file=<abs>&machine=<host>` —
/// used for vanished files and delete-before-reindex. The `machine`
/// scope (sprint 023 #408) keeps one host's delete from dropping another
/// host's chunk for the same path. Uses raw `reqwest` because
/// `klams-client` doesn't yet expose a dedicated helper; piggybacks on
/// the client's base URL + bearer.
pub async fn publish_delete(
    base_url: &str,
    bearer: &str,
    host: &str,
    source_file: &str,
) -> Result<u64> {
    let http = reqwest::Client::new();
    let resp = http
        .post(format!(
            "{}/memory/knowledge/delete",
            base_url.trim_end_matches('/')
        ))
        .query(&[("source_file", source_file), ("machine", host)])
        .header(AUTHORIZATION, format!("Bearer {bearer}"))
        .header(CONTENT_TYPE, "application/json")
        .send()
        .await
        .context("send delete")?;
    let status = resp.status();
    let bytes = resp.bytes().await.context("read delete body")?;
    if !status.is_success() {
        anyhow::bail!(
            "delete returned {status}: {}",
            String::from_utf8_lossy(&bytes)
        );
    }
    let body: serde_json::Value =
        serde_json::from_slice(&bytes).context("decode delete response")?;
    Ok(body
        .get("deleted")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0))
}
