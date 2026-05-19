//! Wraps `klams-client` with the two HTTP calls the scanner needs:
//! `index_knowledge` (one POST per chunk) and `delete_knowledge`
//! (one POST per vanished file).

use anyhow::{Context, Result};
use klams_client::Client;
use klams_types::{IndexKnowledgeRequest, Source};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};

pub async fn publish_chunk(
    client: &Client,
    repo: &str,
    source_file: &str,
    chunk_text: &str,
) -> Result<()> {
    let req = IndexKnowledgeRequest {
        text: chunk_text.to_owned(),
        source: Source::Task,
        tags: vec![],
        repo: Some(repo.to_owned()),
        file: Some(source_file.to_owned()),
        machine: None,
    };
    client
        .index_knowledge(&req)
        .await
        .context("POST /memory/knowledge/index")?;
    Ok(())
}

/// `POST /memory/knowledge/delete?source_file=<abs>` — used at the
/// end of a walk for every path that has vanished since the last run.
/// Uses raw `reqwest` because `klams-client` doesn't yet expose a
/// dedicated helper; piggybacks on the client's base URL + bearer.
pub async fn publish_delete(base_url: &str, bearer: &str, source_file: &str) -> Result<u64> {
    let http = reqwest::Client::new();
    let resp = http
        .post(format!(
            "{}/memory/knowledge/delete",
            base_url.trim_end_matches('/')
        ))
        .query(&[("source_file", source_file)])
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
