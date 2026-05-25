//! Tauri command surface for klams-viewport.
//!
//! Each command is a thin wrapper over `klams_client::Client`, going
//! through the [`ClientFactory`] trait so unit tests can swap in a
//! mock. All commands return `Result<T, ViewportError>`.

pub mod health;
pub mod memory;

use async_trait::async_trait;
use klams_client::{Client, ClientError};
use klams_types::{
    AuthorMemoriesPage, AuthorPage, ContextBundle, ContextRequest, Dissent, DissentPage,
    EventPage, Fact, FactPage, FactType, FactWriteOutcome, HealthSnapshot, KnowledgeItem,
    ListAuthorMemoriesParams, ListAuthorsParams, ListDissentsParams, ListEventsParams,
    ListFactsParams, PublicAuthor, SearchRequest, SearchResults, Source, UpsertFactRequest,
};
use serde::Serialize;
use std::sync::Arc;
use thiserror::Error;
use uuid::Uuid;

#[derive(Serialize, Debug, Error, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ViewportError {
    #[error("not configured: {message}")]
    NotConfigured { message: String },
    #[error("network error: {message}")]
    Network { message: String },
    #[error("auth failed")]
    Unauthorized,
    #[error("server error {status}: {message}")]
    Server { status: u16, message: String },
    #[error("invalid response: {message}")]
    Deserialization { message: String },
}

impl From<ClientError> for ViewportError {
    fn from(e: ClientError) -> Self {
        match e {
            ClientError::InvalidUrl(m) => ViewportError::NotConfigured { message: m },
            ClientError::Transport(t) => ViewportError::Network { message: t.to_string() },
            ClientError::Decode(m) => ViewportError::Deserialization { message: m },
            ClientError::NotImplemented(m) => ViewportError::Server {
                status: 501,
                message: m.into(),
            },
            ClientError::Api { status, body } => {
                if status.as_u16() == 401 {
                    ViewportError::Unauthorized
                } else {
                    ViewportError::Server {
                        status: status.as_u16(),
                        message: body.message,
                    }
                }
            }
        }
    }
}

/// Abstraction over [`klams_client::Client`] for unit tests. Each
/// command takes an `&dyn ClientFactory` and pulls a fresh handle so
/// tests can swap in a mock returning canned responses or errors.
#[async_trait]
pub trait ClientFactory: Send + Sync + std::fmt::Debug {
    async fn list_facts(&self, params: ListFactsParams) -> Result<FactPage, ViewportError>;
    async fn list_events(&self, params: ListEventsParams) -> Result<EventPage, ViewportError>;
    async fn search(&self, req: SearchRequest) -> Result<SearchResults, ViewportError>;
    async fn get_knowledge(&self, id: Uuid) -> Result<KnowledgeItem, ViewportError>;
    async fn health(&self) -> Result<HealthSnapshot, ViewportError>;

    /// Sprint 005 — `POST /memory/context` typed call.
    async fn memory_context(
        &self,
        req: ContextRequest,
    ) -> Result<ContextBundle, ViewportError>;

    // -- Sprint 002: dissents + canonical writes ---------------------
    async fn list_dissents(
        &self,
        params: ListDissentsParams,
    ) -> Result<DissentPage, ViewportError>;
    async fn get_dissent(&self, id: Uuid) -> Result<Dissent, ViewportError>;
    async fn promote_dissent(
        &self,
        id: Uuid,
        caller_source: Source,
        expected_version: i32,
    ) -> Result<Fact, ViewportError>;
    async fn discard_dissent(
        &self,
        id: Uuid,
        caller_source: Source,
    ) -> Result<Dissent, ViewportError>;
    async fn upsert_fact(
        &self,
        req: UpsertFactRequest,
    ) -> Result<FactWriteOutcome, ViewportError>;
    async fn delete_fact(&self, id: Uuid) -> Result<(), ViewportError>;
    async fn edit_fact(
        &self,
        id: Uuid,
        fact_type: FactType,
        payload: serde_json::Value,
        expected_version: i32,
    ) -> Result<FactWriteOutcome, ViewportError>;

    // -- Sprint 007: viewport `/v1/authors` drilldown -----------------
    async fn list_authors(
        &self,
        params: ListAuthorsParams,
    ) -> Result<AuthorPage, ViewportError>;
    async fn get_author(&self, id: Uuid) -> Result<PublicAuthor, ViewportError>;
    async fn list_author_memories(
        &self,
        id: Uuid,
        params: ListAuthorMemoriesParams,
    ) -> Result<AuthorMemoriesPage, ViewportError>;

    /// Default impl walks pages of `/memory/facts` looking for `id`.
    /// Override in production once a `GET /memory/facts/{id}` lands.
    async fn get_fact(&self, id: Uuid) -> Result<Fact, ViewportError> {
        let mut cursor: Option<String> = None;
        for _ in 0..50 {
            let params = ListFactsParams {
                limit: Some(200),
                cursor: cursor.clone(),
                ..Default::default()
            };
            let page = self.list_facts(params).await?;
            if let Some(f) = page.items.iter().find(|f| f.id == id).cloned() {
                return Ok(f);
            }
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Err(ViewportError::Server {
            status: 404,
            message: format!("fact {id} not found"),
        })
    }

    async fn get_event(&self, id: Uuid) -> Result<klams_types::Event, ViewportError> {
        let mut cursor: Option<String> = None;
        for _ in 0..50 {
            let params = ListEventsParams {
                limit: Some(200),
                cursor: cursor.clone(),
                ..Default::default()
            };
            let page = self.list_events(params).await?;
            if let Some(e) = page.items.iter().find(|e| e.id == id).cloned() {
                return Ok(e);
            }
            match page.next_cursor {
                Some(c) => cursor = Some(c),
                None => break,
            }
        }
        Err(ViewportError::Server {
            status: 404,
            message: format!("event {id} not found"),
        })
    }
}

/// Live factory backed by a real `klams_client::Client` built from
/// the persisted viewport config.
#[derive(Debug, Default)]
pub struct LiveClientFactory;

impl LiveClientFactory {
    #[allow(clippy::unused_self)] // method form is ergonomic for the factory trait
    fn client(&self) -> Result<Client, ViewportError> {
        let cfg = crate::config::load();
        let token = crate::config::read_token()
            .ok_or_else(|| ViewportError::NotConfigured { message: "no bearer token in keyring".into() })?;
        Client::new(&cfg.klams_url, token).map_err(Into::into)
    }
}

#[async_trait]
impl ClientFactory for LiveClientFactory {
    async fn list_facts(&self, params: ListFactsParams) -> Result<FactPage, ViewportError> {
        Ok(self.client()?.list_facts_with(&params).await?)
    }
    async fn list_events(&self, params: ListEventsParams) -> Result<EventPage, ViewportError> {
        Ok(self.client()?.list_events(&params).await?)
    }
    async fn search(&self, req: SearchRequest) -> Result<SearchResults, ViewportError> {
        Ok(self.client()?.search(&req).await?)
    }
    async fn get_knowledge(&self, id: Uuid) -> Result<KnowledgeItem, ViewportError> {
        Ok(self.client()?.get_knowledge(id).await?)
    }
    async fn health(&self) -> Result<HealthSnapshot, ViewportError> {
        Ok(self.client()?.health().await?)
    }
    async fn memory_context(
        &self,
        req: ContextRequest,
    ) -> Result<ContextBundle, ViewportError> {
        Ok(self.client()?.memory_context(&req).await?)
    }
    async fn list_dissents(
        &self,
        params: ListDissentsParams,
    ) -> Result<DissentPage, ViewportError> {
        Ok(self.client()?.list_dissents(&params).await?)
    }
    async fn get_dissent(&self, id: Uuid) -> Result<Dissent, ViewportError> {
        Ok(self.client()?.get_dissent(id).await?)
    }
    async fn promote_dissent(
        &self,
        id: Uuid,
        caller_source: Source,
        expected_version: i32,
    ) -> Result<Fact, ViewportError> {
        Ok(self
            .client()?
            .promote_dissent(id, caller_source, expected_version)
            .await?)
    }
    async fn discard_dissent(
        &self,
        id: Uuid,
        caller_source: Source,
    ) -> Result<Dissent, ViewportError> {
        Ok(self.client()?.discard_dissent(id, caller_source).await?)
    }
    async fn upsert_fact(
        &self,
        req: UpsertFactRequest,
    ) -> Result<FactWriteOutcome, ViewportError> {
        Ok(self.client()?.upsert_fact(&req).await?)
    }
    async fn delete_fact(&self, id: Uuid) -> Result<(), ViewportError> {
        Ok(self.client()?.delete_fact(id).await?)
    }
    async fn edit_fact(
        &self,
        id: Uuid,
        fact_type: FactType,
        payload: serde_json::Value,
        expected_version: i32,
    ) -> Result<FactWriteOutcome, ViewportError> {
        Ok(self
            .client()?
            .edit_fact(id, fact_type, payload, expected_version)
            .await?)
    }
    async fn list_authors(
        &self,
        params: ListAuthorsParams,
    ) -> Result<AuthorPage, ViewportError> {
        Ok(self.client()?.list_authors(&params).await?)
    }
    async fn get_author(&self, id: Uuid) -> Result<PublicAuthor, ViewportError> {
        Ok(self.client()?.get_author(id).await?)
    }
    async fn list_author_memories(
        &self,
        id: Uuid,
        params: ListAuthorMemoriesParams,
    ) -> Result<AuthorMemoriesPage, ViewportError> {
        Ok(self.client()?.list_author_memories(id, &params).await?)
    }
}

/// Shared application state passed to every command via Tauri's
/// `State<AppState>`.
#[derive(Debug, Clone)]
pub struct AppState {
    pub factory: Arc<dyn ClientFactory>,
}

impl AppState {
    pub fn live() -> Self {
        Self {
            factory: Arc::new(LiveClientFactory),
        }
    }
}
