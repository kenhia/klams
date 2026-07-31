//! Typed HTTP client for the klams service.
//!
//! Wraps `reqwest::Client` with bearer-token injection and typed
//! request/response handling for every documented endpoint.

// `ClientError::Api` carries the full `WireError` (extended with
// `details` + `current_version` in sprint 002) so callers can render
// structured field-level errors without a second hop. The variant is
// ~130 bytes which trips `result_large_err`; we accept the size cost
// here because every public method already returns `ClientResult`.
#![allow(clippy::result_large_err)]

use klams_core::PolicyTable;
use klams_types::{
    AcceptedId, ApiError as WireError, AppendEventRequest, AuthorMemoriesPage, AuthorPage,
    ContextBundle, ContextRequest, Dissent, DissentPage, DissentSubmittedResponse, EventPage, Fact,
    FactPage, FactWriteOutcome, HealthSnapshot, IndexKnowledgeRequest, IndexKnowledgeResponse,
    KnowledgeDeleteResponse, KnowledgeItem, ListAuthorMemoriesParams, ListAuthorsParams,
    ListDissentsParams, ListEventsParams, ListFactsParams, ListMemoriesParams, MemoriesPage,
    PublicAuthor, SearchRequest, SearchResults, SearchType, Source, UpsertFactRequest,
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
                details: None,
                current_version: None,
                window_max_days: None,
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
                details: None,
                current_version: None,
                window_max_days: None,
            });
            Err(ClientError::Api { status, body })
        }
    }

    /// Fetch the source-trust policy table (`GET /memory/policy`).
    /// Mirrors `contracts/memory_policy.md`.
    pub async fn policy(&self) -> ClientResult<PolicyTable> {
        self.get_json("/memory/policy").await
    }

    pub async fn upsert_fact(&self, req: &UpsertFactRequest) -> ClientResult<FactWriteOutcome> {
        let url = self.url("/memory/facts")?;
        let resp = self
            .http
            .post(url)
            .header(AUTHORIZATION, format!("Bearer {}", self.bearer))
            .header(CONTENT_TYPE, "application/json")
            .json(req)
            .send()
            .await?;
        let status = resp.status();
        let bytes = resp.bytes().await?;
        match status {
            StatusCode::OK => {
                let fact: Fact = serde_json::from_slice(&bytes)
                    .map_err(|e| ClientError::Decode(e.to_string()))?;
                Ok(FactWriteOutcome::Persisted { fact })
            }
            StatusCode::ACCEPTED => {
                let r: DissentSubmittedResponse = serde_json::from_slice(&bytes)
                    .map_err(|e| ClientError::Decode(e.to_string()))?;
                Ok(FactWriteOutcome::Dissented {
                    dissent_id: r.dissent_id,
                    fact_id: r.fact_id,
                })
            }
            _ => {
                let body: WireError = serde_json::from_slice(&bytes).unwrap_or(WireError {
                    code: "decode_error".into(),
                    message: format!("non-JSON body ({} bytes)", bytes.len()),
                    field: None,
                    request_id: None,
                    details: None,
                    current_version: None,
                    window_max_days: None,
                });
                if status == StatusCode::CONFLICT {
                    if let Some(current_version) = body.current_version {
                        let fact_id = uuid::Uuid::nil();
                        return Ok(FactWriteOutcome::VersionConflict {
                            current_version,
                            fact_id,
                        });
                    }
                }
                Err(ClientError::Api { status, body })
            }
        }
    }

    /// `GET /memory/dissents` with optional filters.
    pub async fn list_dissents(&self, params: &ListDissentsParams) -> ClientResult<DissentPage> {
        self.get_json_query("/memory/dissents", params).await
    }

    /// `GET /v1/authors` — sprint 007 author drilldown.
    pub async fn list_authors(&self, params: &ListAuthorsParams) -> ClientResult<AuthorPage> {
        self.get_json_query("/v1/authors", params).await
    }

    /// `GET /v1/authors/{id}`.
    pub async fn get_author(&self, id: uuid::Uuid) -> ClientResult<PublicAuthor> {
        self.get_json(&format!("/v1/authors/{id}")).await
    }

    /// `GET /v1/authors/{id}/memories`.
    pub async fn list_author_memories(
        &self,
        id: uuid::Uuid,
        params: &ListAuthorMemoriesParams,
    ) -> ClientResult<AuthorMemoriesPage> {
        self.get_json_query(&format!("/v1/authors/{id}/memories"), params)
            .await
    }

    /// `GET /v1/memories` — sprint 008 cross-author activity page.
    pub async fn list_memories(&self, params: &ListMemoriesParams) -> ClientResult<MemoriesPage> {
        self.get_json_query("/v1/memories", params).await
    }

    /// `GET /memory/dissents/{id}`.
    pub async fn get_dissent(&self, id: uuid::Uuid) -> ClientResult<Dissent> {
        self.get_json(&format!("/memory/dissents/{id}")).await
    }

    /// `POST /memory/dissents/{id}/promote`.
    pub async fn promote_dissent(
        &self,
        id: uuid::Uuid,
        source: Source,
        expected_version: i32,
    ) -> ClientResult<Fact> {
        #[derive(Serialize)]
        struct Body {
            source: Source,
            expected_version: i32,
        }
        self.post_json(
            &format!("/memory/dissents/{id}/promote"),
            &Body {
                source,
                expected_version,
            },
        )
        .await
    }

    /// `POST /memory/dissents/{id}/discard`.
    pub async fn discard_dissent(&self, id: uuid::Uuid, source: Source) -> ClientResult<Dissent> {
        #[derive(Serialize)]
        struct Body {
            source: Source,
        }
        self.post_json(&format!("/memory/dissents/{id}/discard"), &Body { source })
            .await
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

    /// `POST /memory/context` — sprint 005 hybrid retrieval bundle.
    pub async fn memory_context(&self, req: &ContextRequest) -> ClientResult<ContextBundle> {
        self.post_json("/memory/context", req).await
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

    /// `POST /memory/knowledge/delete?source_file=..&machine=..` —
    /// remove the (`machine`, `source_file`) copy from every point carrying
    /// it; a point is deleted only when its last copy goes (sprint 028
    /// #642). `machine` is required by the API (sprint 025 #637).
    pub async fn delete_knowledge_by_source_file(
        &self,
        source_file: &str,
        machine: Option<&str>,
    ) -> ClientResult<KnowledgeDeleteResponse> {
        let url = self.url("/memory/knowledge/delete")?;
        let mut query: Vec<(&str, &str)> = vec![("source_file", source_file)];
        if let Some(m) = machine {
            query.push(("machine", m));
        }
        let resp = self
            .http
            .post(url)
            .query(&query)
            .header(AUTHORIZATION, format!("Bearer {}", self.bearer))
            .send()
            .await?;
        Self::decode(resp).await
    }

    /// `GET /memory/facts/{id}` — single fact lookup including `dissent_count`.
    pub async fn get_fact(&self, id: uuid::Uuid) -> ClientResult<Fact> {
        self.get_json(&format!("/memory/facts/{id}")).await
    }

    /// `DELETE /memory/facts/{id}` — admin delete (`User`/`Controller`).
    pub async fn delete_fact(&self, id: uuid::Uuid) -> ClientResult<()> {
        let url = self.url(&format!("/memory/facts/{id}"))?;
        let resp = self
            .http
            .delete(url)
            .header(AUTHORIZATION, format!("Bearer {}", self.bearer))
            .send()
            .await?;
        let status = resp.status();
        if status.is_success() {
            return Ok(());
        }
        let bytes = resp.bytes().await?;
        let body: WireError = serde_json::from_slice(&bytes).unwrap_or(WireError {
            code: "decode_error".into(),
            message: format!("non-JSON body ({} bytes)", bytes.len()),
            field: None,
            request_id: None,
            details: None,
            current_version: None,
            window_max_days: None,
        });
        Err(ClientError::Api { status, body })
    }

    /// User-sourced edit through the canonical write path
    /// ([`Self::upsert_fact`]) with `source = Source::User`.
    pub async fn edit_fact(
        &self,
        id: uuid::Uuid,
        fact_type: klams_types::FactType,
        payload: serde_json::Value,
        expected_version: i32,
    ) -> ClientResult<FactWriteOutcome> {
        let req = UpsertFactRequest {
            fact_type,
            source: Source::User,
            payload,
            explicit_id: Some(id),
            expected_version: Some(expected_version),
        };
        self.upsert_fact(&req).await
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
            expected_version: None,
        };
        let outcome = c.upsert_fact(&req).await.unwrap();
        match outcome {
            klams_types::FactWriteOutcome::Persisted { fact } => {
                assert_eq!(fact.id, id);
                assert_eq!(fact.version, 1);
            }
            other => panic!("expected Persisted, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn policy_returns_default_table() {
        let server = MockServer::start().await;
        let expected = PolicyTable::default();
        Mock::given(method("GET"))
            .and(path("/memory/policy"))
            .and(header("authorization", "Bearer tok"))
            .respond_with(
                ResponseTemplate::new(200).set_body_json(serde_json::to_value(&expected).unwrap()),
            )
            .mount(&server)
            .await;

        let c = Client::new(&server.uri(), "tok").unwrap();
        let got = c.policy().await.unwrap();
        assert_eq!(got, expected);
    }
}
