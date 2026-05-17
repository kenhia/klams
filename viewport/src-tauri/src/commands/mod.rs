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
    EventPage, Fact, FactPage, HealthSnapshot, KnowledgeItem, ListEventsParams, ListFactsParams,
    SearchRequest, SearchResults,
};
use serde::Serialize;
use std::sync::Arc;
use thiserror::Error;
use uuid::Uuid;

#[derive(Serialize, Debug, Error, Clone)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ViewportError {
    #[error("not configured: {0}")]
    NotConfigured(String),
    #[error("network error: {0}")]
    Network(String),
    #[error("auth failed")]
    Unauthorized,
    #[error("server error {status}: {message}")]
    Server { status: u16, message: String },
    #[error("invalid response: {0}")]
    Deserialization(String),
}

impl From<ClientError> for ViewportError {
    fn from(e: ClientError) -> Self {
        match e {
            ClientError::InvalidUrl(m) => ViewportError::NotConfigured(m),
            ClientError::Transport(t) => ViewportError::Network(t.to_string()),
            ClientError::Decode(m) => ViewportError::Deserialization(m),
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
            .ok_or_else(|| ViewportError::NotConfigured("no bearer token in keyring".into()))?;
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
