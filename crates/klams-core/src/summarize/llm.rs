//! Ollama HTTP client for LLM-fallback summarization (Phi-3-medium on kubs0).
//!
//! Sprint 005 (Phase 4) — T035. Direct `reqwest` POST to
//! `[summarization] ollama_url`; one-shot `probe()` at task start
//! that disables fallback for the cycle on failure (research.md
//! D-010). The client is intentionally minimal — no streaming,
//! no retries — since the surrounding `SummarizationTask` already
//! handles graceful degradation.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum OllamaError {
    #[error("ollama transport error: {0}")]
    Transport(String),
    #[error("ollama returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("ollama response missing required field `response`")]
    MissingResponse,
}

pub type OllamaResult<T> = Result<T, OllamaError>;

#[derive(Debug, Clone)]
pub struct OllamaClient {
    base_url: String,
    model: String,
    http: reqwest::Client,
}

impl OllamaClient {
    /// Build a new client. `base_url` is e.g. `http://kubs0:11434`.
    #[must_use]
    pub fn new(base_url: impl Into<String>, model: impl Into<String>) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            http,
        }
    }

    /// One-shot probe: `GET /api/tags` and check the configured model
    /// is present. Used to gate the cycle's LLM fallback flag.
    pub async fn probe(&self) -> OllamaResult<()> {
        let url = format!("{}/api/tags", self.base_url);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .map_err(|e| OllamaError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OllamaError::Status {
                status: status.as_u16(),
                body,
            });
        }
        let tags: TagsResponse = resp
            .json()
            .await
            .map_err(|e| OllamaError::Transport(e.to_string()))?;
        if tags
            .models
            .iter()
            .any(|m| m.name == self.model || m.name.starts_with(&format!("{}:", self.model)))
        {
            Ok(())
        } else {
            Err(OllamaError::Status {
                status: 404,
                body: format!("model `{}` not present", self.model),
            })
        }
    }

    /// Generate a single completion via `POST /api/generate`.
    pub async fn generate(&self, prompt: &str) -> OllamaResult<String> {
        let url = format!("{}/api/generate", self.base_url);
        let body = GenerateRequest {
            model: &self.model,
            prompt,
            stream: false,
        };
        let resp = self
            .http
            .post(&url)
            .json(&body)
            .send()
            .await
            .map_err(|e| OllamaError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(OllamaError::Status {
                status: status.as_u16(),
                body,
            });
        }
        let parsed: GenerateResponse = resp
            .json()
            .await
            .map_err(|e| OllamaError::Transport(e.to_string()))?;
        if parsed.response.trim().is_empty() {
            Err(OllamaError::MissingResponse)
        } else {
            Ok(parsed.response)
        }
    }
}

#[derive(Serialize)]
struct GenerateRequest<'a> {
    model: &'a str,
    prompt: &'a str,
    stream: bool,
}

#[derive(Deserialize)]
struct GenerateResponse {
    #[serde(default)]
    response: String,
}

#[derive(Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagModel>,
}

#[derive(Deserialize)]
struct TagModel {
    name: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn probe_to_unreachable_host_returns_transport_error() {
        // 127.0.0.1:1 is closed on virtually any host → immediate
        // ECONNREFUSED, no waiting for a SYN ACK. Avoids relying
        // on RFC 5737 ranges that may behave differently per CI.
        let c = OllamaClient::new("http://127.0.0.1:1", "phi3:medium");
        let err = tokio::time::timeout(Duration::from_secs(5), c.probe())
            .await
            .expect("probe should not hang")
            .expect_err("unreachable host must error");
        assert!(matches!(err, OllamaError::Transport(_)), "got {err:?}");
    }

    #[test]
    fn new_strips_trailing_slash() {
        let c = OllamaClient::new("http://host:11434/", "m");
        assert_eq!(c.base_url, "http://host:11434");
    }
}
