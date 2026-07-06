//! OpenAI-compatible chat client for LLM-fallback summarization.
//!
//! Sprint 014 — replaces the Ollama-native client so the serving
//! engine (Ollama `/v1`, vLLM, kvllm on kai, …) is a config choice.
//! One-shot `probe()` at task start disables fallback for the cycle
//! on failure (research.md D-010, unchanged). The client is
//! intentionally minimal — no streaming, no retries — since the
//! surrounding `SummarizationTask` already handles graceful
//! degradation.

use serde::{Deserialize, Serialize};
use std::time::Duration;
use thiserror::Error;

#[derive(Debug, Error)]
pub enum ChatError {
    #[error("chat endpoint transport error: {0}")]
    Transport(String),
    #[error("chat endpoint returned status {status}: {body}")]
    Status { status: u16, body: String },
    #[error("chat response missing message content")]
    MissingResponse,
}

pub type ChatResult<T> = Result<T, ChatError>;

#[derive(Debug, Clone)]
pub struct OpenAiChatClient {
    /// OpenAI-compat base *including* `/v1`, e.g.
    /// `http://127.0.0.1:11434/v1` (Ollama) or `http://kai:8000/v1` (vLLM).
    base_url: String,
    model: String,
    api_key: Option<String>,
    http: reqwest::Client,
}

impl OpenAiChatClient {
    #[must_use]
    pub fn new(
        base_url: impl Into<String>,
        model: impl Into<String>,
        api_key: Option<String>,
    ) -> Self {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()
            .unwrap_or_default();
        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            model: model.into(),
            api_key,
            http,
        }
    }

    fn authed(&self, req: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        match &self.api_key {
            Some(k) => req.bearer_auth(k),
            None => req,
        }
    }

    /// One-shot probe: `GET {base}/models` and check the configured
    /// model is present. Used to gate the cycle's LLM fallback flag.
    pub async fn probe(&self) -> ChatResult<()> {
        let url = format!("{}/models", self.base_url);
        let resp = self
            .authed(self.http.get(&url))
            .send()
            .await
            .map_err(|e| ChatError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ChatError::Status {
                status: status.as_u16(),
                body,
            });
        }
        let models: ModelsResponse = resp
            .json()
            .await
            .map_err(|e| ChatError::Transport(e.to_string()))?;
        if models
            .data
            .iter()
            .any(|m| m.id == self.model || m.id.starts_with(&format!("{}:", self.model)))
        {
            Ok(())
        } else {
            Err(ChatError::Status {
                status: 404,
                body: format!("model `{}` not present", self.model),
            })
        }
    }

    /// Generate a single completion via `POST {base}/chat/completions`.
    pub async fn generate(&self, prompt: &str) -> ChatResult<String> {
        let url = format!("{}/chat/completions", self.base_url);
        let body = ChatRequest {
            model: &self.model,
            messages: vec![ChatMessage {
                role: "user",
                content: prompt,
            }],
            stream: false,
        };
        let resp = self
            .authed(self.http.post(&url).json(&body))
            .send()
            .await
            .map_err(|e| ChatError::Transport(e.to_string()))?;
        let status = resp.status();
        if !status.is_success() {
            let body = resp.text().await.unwrap_or_default();
            return Err(ChatError::Status {
                status: status.as_u16(),
                body,
            });
        }
        let parsed: ChatResponse = resp
            .json()
            .await
            .map_err(|e| ChatError::Transport(e.to_string()))?;
        let content = parsed
            .choices
            .into_iter()
            .next()
            .map(|c| c.message.content)
            .unwrap_or_default();
        if content.trim().is_empty() {
            Err(ChatError::MissingResponse)
        } else {
            Ok(content)
        }
    }
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: Vec<ChatMessage<'a>>,
    stream: bool,
}

#[derive(Serialize)]
struct ChatMessage<'a> {
    role: &'a str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ChatResponse {
    #[serde(default)]
    choices: Vec<ChatChoice>,
}

#[derive(Deserialize)]
struct ChatChoice {
    message: ChatChoiceMessage,
}

#[derive(Deserialize)]
struct ChatChoiceMessage {
    #[serde(default)]
    content: String,
}

#[derive(Deserialize)]
struct ModelsResponse {
    #[serde(default)]
    data: Vec<ModelEntry>,
}

#[derive(Deserialize)]
struct ModelEntry {
    id: String,
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{body_partial_json, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn probe_to_unreachable_host_returns_transport_error() {
        // 127.0.0.1:1 is closed on virtually any host → immediate
        // ECONNREFUSED, no waiting for a SYN ACK.
        let c = OpenAiChatClient::new("http://127.0.0.1:1/v1", "phi3:medium", None);
        let err = tokio::time::timeout(Duration::from_secs(5), c.probe())
            .await
            .expect("probe should not hang")
            .expect_err("unreachable host must error");
        assert!(matches!(err, ChatError::Transport(_)), "got {err:?}");
    }

    #[test]
    fn new_strips_trailing_slash() {
        let c = OpenAiChatClient::new("http://host:11434/v1/", "m", None);
        assert_eq!(c.base_url, "http://host:11434/v1");
    }

    #[tokio::test]
    async fn probe_accepts_listed_model_and_rejects_absent() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/v1/models"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "object": "list",
                "data": [ { "id": "phi3:medium", "object": "model" } ]
            })))
            .mount(&server)
            .await;

        let present = OpenAiChatClient::new(format!("{}/v1", server.uri()), "phi3:medium", None);
        present.probe().await.unwrap();

        let absent = OpenAiChatClient::new(format!("{}/v1", server.uri()), "nope", None);
        let err = absent.probe().await.unwrap_err();
        assert!(
            matches!(err, ChatError::Status { status: 404, .. }),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn generate_parses_chat_completion() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .and(body_partial_json(serde_json::json!({
                "model": "phi3:medium",
                "messages": [ { "role": "user", "content": "summarize this" } ]
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [ { "message": { "role": "assistant", "content": "a summary" } } ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let c = OpenAiChatClient::new(format!("{}/v1", server.uri()), "phi3:medium", None);
        assert_eq!(c.generate("summarize this").await.unwrap(), "a summary");
    }

    #[tokio::test]
    async fn generate_empty_content_is_missing_response() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/chat/completions"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "choices": [ { "message": { "role": "assistant", "content": "  " } } ]
            })))
            .mount(&server)
            .await;

        let c = OpenAiChatClient::new(format!("{}/v1", server.uri()), "m", None);
        let err = c.generate("x").await.unwrap_err();
        assert!(matches!(err, ChatError::MissingResponse), "{err:?}");
    }
}
