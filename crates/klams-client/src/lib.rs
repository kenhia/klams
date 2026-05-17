//! Typed HTTP client for the klams service.
//!
//! Wraps `reqwest::Client` with bearer-token injection and typed
//! request/response handling for every documented endpoint.

//! Async Rust client for klams-service HTTP API.
//!
//! Phase-2 scope: typed surface + bearer-token injection wired
//! through `reqwest`. Concrete endpoint implementations land with
//! their owning user-story handler tasks.

use klams_types::{
    AcceptedId, ApiError as WireError, AppendEventRequest, EventPage, Fact, FactPage,
    HealthSnapshot, IndexKnowledgeRequest, IndexKnowledgeResponse, KnowledgeItem, ListEventsParams,
    ListFactsParams, SearchRequest, SearchResults, SearchType, UpsertFactRequest,
};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::Serialize;
use thiserror::Error;
use url::Url;

#[derive(Debug, Error)]
pub enum ClientError {
    #[error("invalid base URL: {0}")]
    InvalidUrl(String),
    #[error("HTTP transport error: {0}")]
    Transport(#[from] reqwest::Error),
    #[error("API error {status}: {body:?}")]
    Api { status: StatusCode, body: WireError },
    #[error("response decode error: {0}")]
    Decode(String),
    #[error("not implemented: {0}")]
    NotImplemented(&'static str),
}

pub type ClientResult<T> = Result<T, ClientError>;

#[derive(Debug, Clone)]
pub struct Client {
    base: Url,
    http: reqwest::Client,
    bearer: String,
}

impl Client {
    pub fn new(base_url: &str, bearer_token: impl Into<String>) -> ClientResult<Self> {
        let base = Url::parse(base_url).map_err(|e| ClientError::InvalidUrl(e.to_string()))?;
        Ok(Self {
            base,
            http: reqwest::Client::new(),
            bearer: bearer_token.into(),
        })
    }

    fn url(&self, path: &str) -> ClientResult<Url> {
        self.base
            .join(path)
            .map_err(|e| ClientError::InvalidUrl(e.to_string()))
    }

    async fn post_json<B: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> ClientResult<R> {
        let url = self.url(path)?;
        let resp = self
            .http
            .post(url)
            .header(AUTHORIZATION, format!("Bearer {}", self.bearer))
            .header(CONTENT_TYPE, "application/json")
            .json(body)
            .send()
            .await?;
        Self::decode(resp).await
    }

    async fn get_json<R: DeserializeOwned>(&self, path: &str) -> ClientResult<R> {
        let url = self.url(path)?;
        let resp = self
            .http
            .get(url)
            .header(AUTHORIZATION, format!("Bearer {}", self.bearer))
            .send()
            .await?;
        Self::decode(resp).await
    }

    async fn get_json_query<Q: Serialize, R: DeserializeOwned>(
        &self,
        path: &str,
        query: &Q,
    ) -> ClientResult<R> {
        let url = self.url(path)?;
        let resp = self
            .http
            .get(url)
            .query(query)
            .header(AUTHORIZATION, format!("Bearer {}", self.bearer))
            .send()
            .await?;
        Self::decode(resp).await
    }

    async fn decode<R: DeserializeOwned>(resp: reqwest::Response) -> ClientResult<R> {
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if status.is_success() {
            serde_json::from_slice::<R>(&bytes).map_err(|e| ClientError::Decode(e.to_string()))
        } else {
            let body: WireError = serde_json::from_slice(&bytes).unwrap_or(WireError {
                code: "decode_error".into(),
                message: format!("non-JSON body ({} bytes)", bytes.len()),
                field: None,
                request_id: None,
            });
            Err(ClientError::Api { status, body })
        }
    }

    /// `GET /healthz` — returns the `HealthSnapshot` regardless of
    /// whether the server replied 200 or 503 so callers can render
    /// degraded subsystems instead of seeing only a transport error.
    pub async fn health(&self) -> ClientResult<HealthSnapshot> {
        let url = self.url("/healthz")?;
        let resp = self.http.get(url).send().await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        if status.is_success() || status == reqwest::StatusCode::SERVICE_UNAVAILABLE {
            serde_json::from_slice::<HealthSnapshot>(&bytes)
                .map_err(|e| ClientError::Decode(e.to_string()))
        } else {
            let body: WireError = serde_json::from_slice(&bytes).unwrap_or(WireError {
                code: "decode_error".into(),
                message: format!("non-JSON body ({} bytes)", bytes.len()),
                field: None,
                request_id: None,
            });
            Err(ClientError::Api { status, body })
        }
    }

    #[deprecated(note = "use `health` which tolerates 503")]
    pub async fn healthz(&self) -> ClientResult<HealthSnapshot> {
        self.get_json("/healthz").await
    }

    pub async fn upsert_fact(&self, req: &UpsertFactRequest) -> ClientResult<Fact> {
        self.post_json("/memory/facts", req).await
    }

    pub async fn append_event(&self, req: &AppendEventRequest) -> ClientResult<AcceptedId> {
        self.post_json("/memory/events", req).await
    }

    pub async fn index_knowledge(
        &self,
        req: &IndexKnowledgeRequest,
    ) -> ClientResult<IndexKnowledgeResponse> {
        self.post_json("/memory/knowledge/index", req).await
    }

    pub async fn list_facts(&self) -> ClientResult<FactPage> {
        self.get_json("/memory/facts").await
    }

    /// `GET /memory/facts` with filters and cursor paging.
    pub async fn list_facts_with(&self, params: &ListFactsParams) -> ClientResult<FactPage> {
        self.get_json_query("/memory/facts", params).await
    }

    pub async fn list_events(&self, params: &ListEventsParams) -> ClientResult<EventPage> {
        self.get_json_query("/memory/events", params).await
    }

    pub async fn search(&self, req: &SearchRequest) -> ClientResult<SearchResults> {
        self.post_json("/memory/search", req).await
    }

    /// Convenience wrapper: knowledge-only search via the unified
    /// `/memory/search` endpoint.
    pub async fn search_knowledge(&self, query: &str, top_k: u32) -> ClientResult<SearchResults> {
        let req = SearchRequest {
            query: query.into(),
            types: Some(vec![SearchType::Knowledge]),
            filters: None,
            top_k,
        };
        self.search(&req).await
    }

    pub async fn get_knowledge(&self, id: uuid::Uuid) -> ClientResult<KnowledgeItem> {
        self.get_json(&format!("/memory/knowledge/{id}")).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn bearer_token_is_attached() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/memory/facts"))
            .and(header("authorization", "Bearer s3cret"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "items": [],
                "next_cursor": null
            })))
            .mount(&server)
            .await;

        let c = Client::new(&server.uri(), "s3cret").unwrap();
        let page = c.list_facts().await.unwrap();
        assert!(page.items.is_empty());
    }

    #[tokio::test]
    async fn api_error_is_surfaced() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/memory/facts"))
            .respond_with(ResponseTemplate::new(401).set_body_json(serde_json::json!({
                "code": "unauthorized",
                "message": "missing bearer token"
            })))
            .mount(&server)
            .await;

        let c = Client::new(&server.uri(), "anything").unwrap();
        let err = c.list_facts().await.unwrap_err();
        match err {
            ClientError::Api { status, body } => {
                assert_eq!(status, StatusCode::UNAUTHORIZED);
                assert_eq!(body.code, "unauthorized");
            }
            other => panic!("expected Api error, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn upsert_fact_posts_to_correct_path() {
        let server = MockServer::start().await;
        let id = uuid::Uuid::now_v7();
        let now = "2026-05-16T12:00:00Z";
        Mock::given(method("POST"))
            .and(path("/memory/facts"))
            .and(header("authorization", "Bearer tok"))
            .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
                "id": id,
                "type": "UserFact",
                "payload": {"key": "value"},
                "version": 1,
                "source": "User",
                "confidence": 1.0,
                "decay_weight": 1.0,
                "use_count": 0,
                "last_used_at": null,
                "created_at": now,
                "updated_at": now
            })))
            .mount(&server)
            .await;

        let c = Client::new(&server.uri(), "tok").unwrap();
        let req = klams_types::UpsertFactRequest {
            fact_type: klams_types::FactType::UserFact,
            payload: serde_json::json!({"key": "value"}),
            source: klams_types::Source::User,
            explicit_id: None,
        };
        let fact = c.upsert_fact(&req).await.unwrap();
        assert_eq!(fact.id, id);
        assert_eq!(fact.version, 1);
    }
}
