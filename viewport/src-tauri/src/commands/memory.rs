//! Memory-browsing Tauri commands.
//!
//! Every command takes a single `args` envelope, returns a typed DTO
//! re-exported from `klams_types`, and surfaces errors as
//! `ViewportError`. The actual HTTP work happens behind the
//! `ClientFactory` trait so unit tests can supply a mock.

use crate::commands::{AppState, ViewportError};
use crate::config;
use klams_types::{
    EventPage, Fact, FactPage, KnowledgeItem, ListEventsParams, ListFactsParams, SearchRequest,
    SearchResults, SearchType,
};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Deserialize, Default)]
pub struct ListFactsArgs {
    #[serde(default)]
    pub fact_type: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub created_after: Option<String>,
    #[serde(default)]
    pub created_before: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
}

impl From<ListFactsArgs> for ListFactsParams {
    fn from(a: ListFactsArgs) -> Self {
        ListFactsParams {
            fact_type: a.fact_type,
            source: a.source,
            created_after: a.created_after,
            created_before: a.created_before,
            limit: a.limit,
            cursor: a.cursor,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ListEventsArgs {
    #[serde(default)]
    pub task_id: Option<Uuid>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub created_after: Option<String>,
    #[serde(default)]
    pub created_before: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct SearchArgs {
    pub query: String,
    #[serde(default)]
    pub types: Option<Vec<String>>,
    #[serde(default)]
    pub filters: Option<serde_json::Value>,
    #[serde(default)]
    pub top_k: Option<u32>,
}

#[derive(Debug, Deserialize)]
pub struct ByIdArgs {
    pub id: Uuid,
}

#[derive(Debug, Deserialize, Default)]
pub struct SetConfigArgs {
    #[serde(default)]
    pub klams_url: Option<String>,
    #[serde(default)]
    pub bearer_token: Option<String>,
    #[serde(default)]
    pub refresh_interval_seconds: Option<u32>,
}

fn parse_types(input: Option<&Vec<String>>) -> Option<Vec<SearchType>> {
    let v = input?;
    let mut out = Vec::with_capacity(v.len());
    for s in v {
        out.push(match s.to_ascii_lowercase().as_str() {
            "fact" => SearchType::Fact,
            "event" => SearchType::Event,
            "knowledge" => SearchType::Knowledge,
            _ => return None,
        });
    }
    Some(out)
}

fn into_search_request(args: SearchArgs, default_types: Option<Vec<SearchType>>) -> SearchRequest {
    SearchRequest {
        query: args.query,
        types: parse_types(args.types.as_ref()).or(default_types),
        filters: args.filters,
        top_k: args.top_k.unwrap_or(10),
    }
}

#[tauri::command]
pub async fn list_facts(
    state: tauri::State<'_, AppState>,
    args: ListFactsArgs,
) -> Result<FactPage, ViewportError> {
    state.factory.list_facts(args.into()).await
}

#[tauri::command]
pub async fn list_events(
    state: tauri::State<'_, AppState>,
    args: ListEventsArgs,
) -> Result<EventPage, ViewportError> {
    let params = ListEventsParams {
        task_id: args.task_id,
        category: args.category,
        created_after: args.created_after.as_deref().and_then(parse_rfc3339_opt),
        created_before: args.created_before.as_deref().and_then(parse_rfc3339_opt),
        limit: args.limit,
        cursor: args.cursor,
    };
    state.factory.list_events(params).await
}

fn parse_rfc3339_opt(s: &str) -> Option<time::OffsetDateTime> {
    time::OffsetDateTime::parse(s, &time::format_description::well_known::Rfc3339).ok()
}

#[tauri::command]
pub async fn search_unified(
    state: tauri::State<'_, AppState>,
    args: SearchArgs,
) -> Result<SearchResults, ViewportError> {
    let req = into_search_request(args, None);
    state.factory.search(req).await
}

#[tauri::command]
pub async fn search_knowledge(
    state: tauri::State<'_, AppState>,
    args: SearchArgs,
) -> Result<SearchResults, ViewportError> {
    let req = into_search_request(args, Some(vec![SearchType::Knowledge]));
    state.factory.search(req).await
}

#[tauri::command]
pub async fn get_fact(
    state: tauri::State<'_, AppState>,
    args: ByIdArgs,
) -> Result<Fact, ViewportError> {
    state.factory.get_fact(args.id).await
}

#[tauri::command]
pub async fn get_event(
    state: tauri::State<'_, AppState>,
    args: ByIdArgs,
) -> Result<klams_types::Event, ViewportError> {
    state.factory.get_event(args.id).await
}

#[tauri::command]
pub async fn get_knowledge_item(
    state: tauri::State<'_, AppState>,
    args: ByIdArgs,
) -> Result<KnowledgeItem, ViewportError> {
    state.factory.get_knowledge(args.id).await
}

#[derive(Debug, Serialize)]
pub struct ConfigSummary {
    pub klams_url: String,
    pub has_token: bool,
    pub refresh_interval_seconds: u32,
}

#[tauri::command]
pub async fn get_config() -> Result<ConfigSummary, ViewportError> {
    let snap = config::snapshot();
    Ok(ConfigSummary {
        klams_url: snap.klams_url,
        has_token: snap.has_token,
        refresh_interval_seconds: snap.refresh_interval_seconds,
    })
}

#[tauri::command]
pub async fn set_config(
    app: tauri::AppHandle,
    args: SetConfigArgs,
) -> Result<ConfigSummary, ViewportError> {
    use tauri::Emitter;
    let mut stored = config::load();
    if let Some(url) = args.klams_url {
        stored.klams_url = url;
    }
    if let Some(iv) = args.refresh_interval_seconds {
        stored.refresh_interval_seconds = iv;
    }
    config::save(&stored)?;
    if let Some(token) = args.bearer_token {
        if !token.is_empty() {
            config::write_token(&token)?;
        }
    }
    let summary = get_config().await?;
    let _ = app.emit(crate::commands::health::CONFIG_CHANGED_EVENT, &summary);
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::ClientFactory;
    use async_trait::async_trait;
    use klams_types::{HealthSnapshot, HealthStatus, QueueStatus, SubsystemStatus};
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Default)]
    struct MockFactory {
        last_facts: Mutex<Option<ListFactsParams>>,
        last_events: Mutex<Option<ListEventsParams>>,
        last_search: Mutex<Option<SearchRequest>>,
        fail: Mutex<Option<ViewportError>>,
    }

    impl MockFactory {
        fn ok() -> SubsystemStatus {
            SubsystemStatus {
                state: HealthStatus::Ok,
                message: None,
            }
        }
    }

    #[async_trait]
    impl ClientFactory for MockFactory {
        async fn list_facts(&self, p: ListFactsParams) -> Result<FactPage, ViewportError> {
            if let Some(e) = self.fail.lock().unwrap().clone() {
                return Err(e);
            }
            *self.last_facts.lock().unwrap() = Some(p);
            Ok(FactPage {
                items: vec![],
                next_cursor: None,
            })
        }
        async fn list_events(&self, p: ListEventsParams) -> Result<EventPage, ViewportError> {
            if let Some(e) = self.fail.lock().unwrap().clone() {
                return Err(e);
            }
            *self.last_events.lock().unwrap() = Some(p);
            Ok(EventPage {
                items: vec![],
                next_cursor: None,
            })
        }
        async fn search(&self, req: SearchRequest) -> Result<SearchResults, ViewportError> {
            if let Some(e) = self.fail.lock().unwrap().clone() {
                return Err(e);
            }
            *self.last_search.lock().unwrap() = Some(req.clone());
            Ok(SearchResults {
                query: req.query,
                results: vec![],
                total: 0,
                degraded: false,
            })
        }
        async fn get_knowledge(&self, _id: Uuid) -> Result<KnowledgeItem, ViewportError> {
            Err(ViewportError::Server {
                status: 404,
                message: "no".into(),
            })
        }
        async fn health(&self) -> Result<HealthSnapshot, ViewportError> {
            Ok(HealthSnapshot {
                status: HealthStatus::Ok,
                postgres: MockFactory::ok(),
                qdrant: MockFactory::ok(),
                embeddings: MockFactory::ok(),
                queue: QueueStatus {
                    depth: 0,
                    capacity: 1,
                    workers: 1,
                },
                version: "test".into(),
                uptime_seconds: 0,
            })
        }
    }

    #[tokio::test]
    async fn list_facts_forwards_args() {
        let mock = Arc::new(MockFactory::default());
        let args = ListFactsArgs {
            fact_type: Some("UserFact".into()),
            source: Some("User".into()),
            limit: Some(25),
            ..Default::default()
        };
        let _ = mock.list_facts(args.into()).await.unwrap();
        let captured = mock.last_facts.lock().unwrap().clone().unwrap();
        assert_eq!(captured.fact_type.as_deref(), Some("UserFact"));
        assert_eq!(captured.source.as_deref(), Some("User"));
        assert_eq!(captured.limit, Some(25));
    }

    #[tokio::test]
    async fn search_unified_defaults_to_top_k_10() {
        let mock = Arc::new(MockFactory::default());
        let req = into_search_request(
            SearchArgs {
                query: "hello".into(),
                ..Default::default()
            },
            None,
        );
        assert_eq!(req.top_k, 10);
        assert!(req.types.is_none());
        let _ = mock.search(req).await.unwrap();
    }

    #[tokio::test]
    async fn search_knowledge_forces_knowledge_type() {
        let mock = Arc::new(MockFactory::default());
        let req = into_search_request(
            SearchArgs {
                query: "x".into(),
                top_k: Some(5),
                ..Default::default()
            },
            Some(vec![SearchType::Knowledge]),
        );
        assert_eq!(req.types.as_deref(), Some(&[SearchType::Knowledge][..]));
        assert_eq!(req.top_k, 5);
        mock.search(req).await.unwrap();
    }

    #[tokio::test]
    async fn list_events_parses_rfc3339_timestamps() {
        let mock = Arc::new(MockFactory::default());
        let params = ListEventsParams {
            task_id: None,
            category: Some("agent.activity".into()),
            created_after: parse_rfc3339_opt("2025-01-01T00:00:00Z"),
            created_before: None,
            limit: Some(50),
            cursor: None,
        };
        assert!(params.created_after.is_some());
        let _ = mock.list_events(params).await.unwrap();
        let captured = mock.last_events.lock().unwrap().clone().unwrap();
        assert_eq!(captured.category.as_deref(), Some("agent.activity"));
    }

    #[tokio::test]
    async fn errors_are_propagated() {
        let mock = Arc::new(MockFactory::default());
        *mock.fail.lock().unwrap() = Some(ViewportError::Unauthorized);
        let err = mock
            .list_facts(ListFactsParams::default())
            .await
            .unwrap_err();
        assert!(matches!(err, ViewportError::Unauthorized));
    }

    #[tokio::test]
    async fn get_fact_walks_pages_and_404s_when_missing() {
        let mock = Arc::new(MockFactory::default());
        // MockFactory's list_facts always returns empty + no cursor,
        // so get_fact (default impl) should 404 after one page.
        let err = mock.get_fact(Uuid::now_v7()).await.unwrap_err();
        match err {
            ViewportError::Server { status, .. } => assert_eq!(status, 404),
            other => panic!("expected Server 404, got {other:?}"),
        }
    }
}
