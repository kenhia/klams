//! TEI embedding client.
//!
//! Calls the Hugging Face Text Embeddings Inference HTTP API
//! (`POST /embed`) and returns the 384-dim vector for a single input.

use crate::{StoreError, StoreResult};
use std::time::Duration;

const MAX_RETRIES: usize = 3;

#[derive(Debug, Clone)]
pub struct TeiEmbedder {
    base_url: String,
    client: reqwest::Client,
    expected_dim: usize,
}

impl TeiEmbedder {
    pub fn new(base_url: impl Into<String>, expected_dim: usize) -> StoreResult<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(30))
            .build()
            .map_err(|e| StoreError::Embedding(format!("client build: {e}")))?;
        Ok(Self {
            base_url: base_url.into(),
            client,
            expected_dim,
        })
    }

    pub fn expected_dim(&self) -> usize {
        self.expected_dim
    }

    pub async fn embed(&self, text: &str) -> StoreResult<Vec<f32>> {
        let url = format!("{}/embed", self.base_url.trim_end_matches('/'));
        let body = serde_json::json!({ "inputs": [text] });

        let mut last_err = String::new();
        let mut backoff = Duration::from_millis(200);
        for attempt in 0..MAX_RETRIES {
            match self.client.post(&url).json(&body).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let bytes = resp
                            .bytes()
                            .await
                            .map_err(|e| StoreError::Embedding(format!("read body: {e}")))?;
                        let vectors: Vec<Vec<f32>> = serde_json::from_slice(&bytes)
                            .map_err(|e| StoreError::Embedding(format!("parse: {e}")))?;
                        let v = vectors.into_iter().next().ok_or_else(|| {
                            StoreError::Embedding("empty vectors response".into())
                        })?;
                        if v.len() != self.expected_dim {
                            return Err(StoreError::Embedding(format!(
                                "expected dim {}, got {}",
                                self.expected_dim,
                                v.len()
                            )));
                        }
                        return Ok(v);
                    }
                    last_err = format!("HTTP {}", resp.status());
                }
                Err(e) => last_err = format!("send: {e}"),
            }
            if attempt + 1 < MAX_RETRIES {
                tokio::time::sleep(backoff).await;
                backoff *= 2;
            }
        }
        Err(StoreError::Embedding(format!(
            "TEI request failed after {MAX_RETRIES} attempts: {last_err}"
        )))
    }

    /// Cheap liveness probe. Hits the TEI `/health` endpoint with a
    /// 1s timeout so the health handler never blocks for long.
    pub async fn health(&self) -> StoreResult<()> {
        let url = format!("{}/health", self.base_url.trim_end_matches('/'));
        let resp = self
            .client
            .get(&url)
            .timeout(Duration::from_secs(1))
            .send()
            .await
            .map_err(|e| StoreError::Embedding(format!("tei health: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(StoreError::Embedding(format!(
                "tei health HTTP {}",
                resp.status()
            )))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Integration test against the live TEI compose service.
    /// Set `TEST_TEI_URL` to enable; defaults are off.
    #[tokio::test]
    #[ignore = "requires TEST_TEI_URL pointing at a running TEI server"]
    async fn embed_returns_384_dim_vector() {
        let url = std::env::var("TEST_TEI_URL").expect("TEST_TEI_URL not set");
        let embedder = TeiEmbedder::new(url, 384).unwrap();
        let v = embedder.embed("hello world").await.unwrap();
        assert_eq!(v.len(), 384);
    }
}
