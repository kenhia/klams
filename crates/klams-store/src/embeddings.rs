//! Embedding clients.
//!
//! Sprint 014 — the `Embedder` trait decouples the store from any one
//! serving engine. Two implementations:
//!
//! * [`TeiEmbedder`] — Hugging Face Text Embeddings Inference native
//!   API (`POST /embed`). The production default on `kubs0`.
//! * [`OpenAiCompatEmbedder`] — the OpenAI-compatible surface
//!   (`POST {url}/embeddings`) served by vLLM, TEI's `/v1` route,
//!   Ollama, etc. Selected via `[embeddings] api = "openai"`.

use crate::{StoreError, StoreResult};
use async_trait::async_trait;
use std::time::Duration;

const MAX_RETRIES: usize = 3;

/// A text-embedding backend. Object-safe so `CompositeStore` can hold
/// whichever engine the config selected.
#[async_trait]
pub trait Embedder: Send + Sync + std::fmt::Debug {
    /// Embed a single input; the returned vector length must equal
    /// [`Embedder::expected_dim`].
    async fn embed(&self, text: &str) -> StoreResult<Vec<f32>>;
    /// Cheap liveness probe for the health handler (≤1s).
    async fn health(&self) -> StoreResult<()>;
    fn expected_dim(&self) -> usize;
}

fn build_http_client() -> StoreResult<reqwest::Client> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| StoreError::Embedding(format!("client build: {e}")))
}

fn check_dim(v: Vec<f32>, expected: usize) -> StoreResult<Vec<f32>> {
    if v.len() == expected {
        Ok(v)
    } else {
        Err(StoreError::Embedding(format!(
            "expected dim {expected}, got {}",
            v.len()
        )))
    }
}

// ---------------------------------------------------------------------
// TEI (native API)

#[derive(Debug, Clone)]
pub struct TeiEmbedder {
    base_url: String,
    client: reqwest::Client,
    expected_dim: usize,
}

impl TeiEmbedder {
    pub fn new(base_url: impl Into<String>, expected_dim: usize) -> StoreResult<Self> {
        Ok(Self {
            base_url: base_url.into(),
            client: build_http_client()?,
            expected_dim,
        })
    }
}

#[async_trait]
impl Embedder for TeiEmbedder {
    async fn embed(&self, text: &str) -> StoreResult<Vec<f32>> {
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
                        return check_dim(v, self.expected_dim);
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

    /// Hits the TEI `/health` endpoint with a 1s timeout so the health
    /// handler never blocks for long.
    async fn health(&self) -> StoreResult<()> {
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

    fn expected_dim(&self) -> usize {
        self.expected_dim
    }
}

// ---------------------------------------------------------------------
// OpenAI-compatible (vLLM, TEI `/v1`, Ollama `/v1`, …)

/// `base_url` is the OpenAI-compat base **including** the version
/// segment, e.g. `http://127.0.0.1:7070/v1` (TEI) or
/// `http://kai:8000/v1` (vLLM). Endpoints used: `POST {base}/embeddings`,
/// `GET {base}/models`.
#[derive(Debug, Clone)]
pub struct OpenAiCompatEmbedder {
    base_url: String,
    model: String,
    client: reqwest::Client,
    expected_dim: usize,
    api_key: Option<String>,
}

#[derive(serde::Deserialize)]
struct EmbeddingsResponse {
    data: Vec<EmbeddingsDatum>,
}

#[derive(serde::Deserialize)]
struct EmbeddingsDatum {
    embedding: Vec<f32>,
}

impl OpenAiCompatEmbedder {
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        expected_dim: usize,
        api_key: Option<String>,
    ) -> StoreResult<Self> {
        Ok(Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            client: build_http_client()?,
            expected_dim,
            api_key,
        })
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(k) => req.bearer_auth(k),
            None => req,
        }
    }
}

#[async_trait]
impl Embedder for OpenAiCompatEmbedder {
    async fn embed(&self, text: &str) -> StoreResult<Vec<f32>> {
        let url = format!("{}/embeddings", self.base_url);
        let body = serde_json::json!({ "model": self.model, "input": [text] });

        let mut last_err = String::new();
        let mut backoff = Duration::from_millis(200);
        for attempt in 0..MAX_RETRIES {
            match self.authed(self.client.post(&url).json(&body)).send().await {
                Ok(resp) => {
                    if resp.status().is_success() {
                        let bytes = resp
                            .bytes()
                            .await
                            .map_err(|e| StoreError::Embedding(format!("read body: {e}")))?;
                        let parsed: EmbeddingsResponse = serde_json::from_slice(&bytes)
                            .map_err(|e| StoreError::Embedding(format!("parse: {e}")))?;
                        let v = parsed
                            .data
                            .into_iter()
                            .next()
                            .map(|d| d.embedding)
                            .ok_or_else(|| StoreError::Embedding("empty data response".into()))?;
                        return check_dim(v, self.expected_dim);
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
            "embeddings request failed after {MAX_RETRIES} attempts: {last_err}"
        )))
    }

    /// Probes `GET {base}/models` with a 1s timeout. Any 2xx counts as
    /// alive; model presence is not asserted here (some engines gate
    /// the list behind auth scopes the embed path doesn't need).
    async fn health(&self) -> StoreResult<()> {
        let url = format!("{}/models", self.base_url);
        let resp = self
            .authed(self.client.get(&url).timeout(Duration::from_secs(1)))
            .send()
            .await
            .map_err(|e| StoreError::Embedding(format!("openai-compat health: {e}")))?;
        if resp.status().is_success() {
            Ok(())
        } else {
            Err(StoreError::Embedding(format!(
                "openai-compat health HTTP {}",
                resp.status()
            )))
        }
    }

    fn expected_dim(&self) -> usize {
        self.expected_dim
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    /// Integration test against the live TEI compose service.
    /// Set `TEST_TEI_URL` to enable; defaults are off.
    #[tokio::test]
    #[ignore = "requires TEST_TEI_URL pointing at a running TEI server"]
    async fn tei_embed_returns_configured_dim_vector() {
        let url = std::env::var("TEST_TEI_URL").expect("TEST_TEI_URL not set");
        let embedder = TeiEmbedder::new(url, 384).unwrap();
        let v = embedder.embed("hello world").await.unwrap();
        assert_eq!(v.len(), 384);
    }

    /// Integration test against a live OpenAI-compatible endpoint
    /// (e.g. TEI's `/v1` route: `TEST_OPENAI_EMBED_URL=http://127.0.0.1:7070/v1`).
    #[tokio::test]
    #[ignore = "requires TEST_OPENAI_EMBED_URL pointing at an OpenAI-compat /v1 base"]
    async fn openai_embed_returns_configured_dim_vector() {
        let url = std::env::var("TEST_OPENAI_EMBED_URL").expect("TEST_OPENAI_EMBED_URL not set");
        let model = std::env::var("TEST_OPENAI_EMBED_MODEL")
            .unwrap_or_else(|_| "BAAI/bge-small-en-v1.5".into());
        let embedder = OpenAiCompatEmbedder::new(url, model, 384, None).unwrap();
        let v = embedder.embed("hello world").await.unwrap();
        assert_eq!(v.len(), 384);
    }

    #[tokio::test]
    async fn openai_embed_parses_response_and_sends_model() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(body_partial_json(
                serde_json::json!({ "model": "test-model", "input": ["hi"] }),
            ))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "object": "list",
                "data": [ { "object": "embedding", "embedding": [0.1, 0.2, 0.3] } ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let e = OpenAiCompatEmbedder::new(format!("{}/v1", server.uri()), "test-model", 3, None)
            .unwrap();
        let v = e.embed("hi").await.unwrap();
        assert_eq!(v, vec![0.1, 0.2, 0.3]);
    }

    #[tokio::test]
    async fn openai_embed_rejects_dim_mismatch() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [ { "embedding": [0.1, 0.2] } ]
            })))
            .mount(&server)
            .await;

        let e = OpenAiCompatEmbedder::new(format!("{}/v1", server.uri()), "m", 3, None).unwrap();
        let err = e.embed("hi").await.unwrap_err();
        assert!(err.to_string().contains("expected dim 3"), "{err}");
    }

    #[tokio::test]
    async fn openai_embed_sends_bearer_when_key_configured() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(header("authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [ { "embedding": [1.0] } ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let e = OpenAiCompatEmbedder::new(
            format!("{}/v1", server.uri()),
            "m",
            1,
            Some("sk-test".into()),
        )
        .unwrap();
        assert_eq!(e.embed("hi").await.unwrap(), vec![1.0]);
    }

    #[tokio::test]
    async fn openai_embed_retries_5xx_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(500))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "data": [ { "embedding": [1.0] } ]
            })))
            .mount(&server)
            .await;

        let e = OpenAiCompatEmbedder::new(format!("{}/v1", server.uri()), "m", 1, None).unwrap();
        assert_eq!(e.embed("hi").await.unwrap(), vec![1.0]);
    }

    #[tokio::test]
    async fn openai_health_probes_models_endpoint() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "object": "list", "data": []
            })))
            .expect(1)
            .mount(&server)
            .await;

        let e = OpenAiCompatEmbedder::new(format!("{}/v1", server.uri()), "m", 1, None).unwrap();
        e.health().await.unwrap();
    }
}
