//! Memory-browsing Tauri commands.
//!
//! Every command takes a single `args` envelope, returns a typed DTO
//! re-exported from `klams_types`, and surfaces errors as
//! `ViewportError`. The actual HTTP work happens behind the
//! `ClientFactory` trait so unit tests can supply a mock.

use crate::commands::{AppState, ViewportError};
use crate::config;
use klams_types::{
    ContextBundle, ContextRequest, Dissent, DissentPage, EventPage, Fact, FactPage, FactType,
    FactWriteOutcome, KnowledgeItem, ListDissentsParams, ListEventsParams, ListFactsParams,
    ListMemoriesParams, MemoriesPage, SearchRequest, SearchResults, SearchType, Source,
    UpsertFactRequest,
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
        task_id: args.task_id.map(|u| u.to_string()),
        category: args.category,
        created_after: args.created_after.as_deref().and_then(parse_rfc3339_opt),
        created_before: args.created_before.as_deref().and_then(parse_rfc3339_opt),
        limit: args.limit,
        cursor: args.cursor,
        ..ListEventsParams::default()
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

/// Sprint 005 — `POST /memory/context`. Args envelope mirrors the
/// wire `ContextRequest` exactly.
#[tauri::command]
pub async fn memory_context(
    state: tauri::State<'_, AppState>,
    args: ContextRequest,
) -> Result<ContextBundle, ViewportError> {
    state.factory.memory_context(args).await
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

// ---------------------------------------------------------------------------
// Sprint 002: dissents + canonical writes (US4 viewport)
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize, Default)]
pub struct ListDissentsArgs {
    #[serde(default)]
    pub fact_id: Option<Uuid>,
    #[serde(default)]
    pub status: Option<String>,
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

impl From<ListDissentsArgs> for ListDissentsParams {
    fn from(a: ListDissentsArgs) -> Self {
        ListDissentsParams {
            fact_id: a.fact_id,
            status: a.status,
            source: a.source,
            created_after: a.created_after,
            created_before: a.created_before,
            limit: a.limit,
            cursor: a.cursor,
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct PromoteDissentArgs {
    pub dissent_id: Uuid,
    pub caller_source: Source,
    pub expected_version: i32,
}

#[derive(Debug, Deserialize)]
pub struct DiscardDissentArgs {
    pub dissent_id: Uuid,
    pub caller_source: Source,
}

#[derive(Debug, Deserialize)]
pub struct UpsertFactArgs {
    pub fact_type: FactType,
    pub payload: serde_json::Value,
    pub source: Source,
    #[serde(default)]
    pub explicit_id: Option<Uuid>,
    #[serde(default)]
    pub expected_version: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct EditFactArgs {
    pub id: Uuid,
    pub fact_type: FactType,
    pub payload: serde_json::Value,
    pub expected_version: i32,
}

#[tauri::command]
pub async fn list_dissents(
    state: tauri::State<'_, AppState>,
    args: ListDissentsArgs,
) -> Result<DissentPage, ViewportError> {
    state.factory.list_dissents(args.into()).await
}

#[tauri::command]
pub async fn get_dissent(
    state: tauri::State<'_, AppState>,
    args: ByIdArgs,
) -> Result<Dissent, ViewportError> {
    state.factory.get_dissent(args.id).await
}

#[tauri::command]
pub async fn promote_dissent(
    state: tauri::State<'_, AppState>,
    args: PromoteDissentArgs,
) -> Result<Fact, ViewportError> {
    state
        .factory
        .promote_dissent(args.dissent_id, args.caller_source, args.expected_version)
        .await
}

#[tauri::command]
pub async fn discard_dissent(
    state: tauri::State<'_, AppState>,
    args: DiscardDissentArgs,
) -> Result<Dissent, ViewportError> {
    state
        .factory
        .discard_dissent(args.dissent_id, args.caller_source)
        .await
}

#[tauri::command]
pub async fn upsert_fact(
    state: tauri::State<'_, AppState>,
    args: UpsertFactArgs,
) -> Result<FactWriteOutcome, ViewportError> {
    let req = UpsertFactRequest {
        fact_type: args.fact_type,
        payload: args.payload,
        source: args.source,
        explicit_id: args.explicit_id,
        expected_version: args.expected_version,
    };
    state.factory.upsert_fact(req).await
}

#[tauri::command]
pub async fn edit_fact(
    state: tauri::State<'_, AppState>,
    args: EditFactArgs,
) -> Result<FactWriteOutcome, ViewportError> {
    state
        .factory
        .edit_fact(args.id, args.fact_type, args.payload, args.expected_version)
        .await
}

#[tauri::command]
pub async fn delete_fact(
    state: tauri::State<'_, AppState>,
    args: ByIdArgs,
) -> Result<(), ViewportError> {
    state.factory.delete_fact(args.id).await
}

#[derive(Debug, Deserialize, Default)]
pub struct ListAuthorsArgs {
    #[serde(default)]
    pub agent_name: Option<String>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
}

impl From<ListAuthorsArgs> for klams_types::ListAuthorsParams {
    fn from(a: ListAuthorsArgs) -> Self {
        klams_types::ListAuthorsParams {
            agent_name: a.agent_name,
            since: a.since,
            limit: a.limit,
            cursor: a.cursor,
        }
    }
}

#[derive(Debug, Deserialize, Default)]
pub struct ListAuthorMemoriesArgs {
    pub id: Uuid,
    #[serde(default)]
    pub kinds: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct ListMemoriesArgs {
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default)]
    pub kinds: Option<String>,
    #[serde(default)]
    pub state: Option<String>,
    #[serde(default)]
    pub authors: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[tauri::command]
pub async fn list_authors(
    state: tauri::State<'_, AppState>,
    args: ListAuthorsArgs,
) -> Result<klams_types::AuthorPage, ViewportError> {
    state.factory.list_authors(args.into()).await
}

#[tauri::command]
pub async fn get_author(
    state: tauri::State<'_, AppState>,
    args: ByIdArgs,
) -> Result<klams_types::PublicAuthor, ViewportError> {
    state.factory.get_author(args.id).await
}

#[tauri::command]
pub async fn list_author_memories(
    state: tauri::State<'_, AppState>,
    args: ListAuthorMemoriesArgs,
) -> Result<klams_types::AuthorMemoriesPage, ViewportError> {
    let id = args.id;
    let params = klams_types::ListAuthorMemoriesParams {
        kinds: args.kinds,
        state: args.state,
        limit: args.limit,
        cursor: args.cursor,
    };
    state.factory.list_author_memories(id, params).await
}

#[tauri::command]
pub async fn list_memories(
    state: tauri::State<'_, AppState>,
    args: ListMemoriesArgs,
) -> Result<MemoriesPage, ViewportError> {
    let params = ListMemoriesParams {
        since: args.since,
        until: args.until,
        kinds: args.kinds,
        state: args.state,
        authors: args.authors,
        limit: args.limit,
        cursor: args.cursor,
    };
    state.factory.list_memories(params).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::commands::ClientFactory;
    use async_trait::async_trait;
    use klams_types::{
        DissentStatus, FactType, HealthSnapshot, HealthStatus, QueueStatus, SubsystemStatus,
    };
    use std::sync::{Arc, Mutex};

    #[derive(Debug, Default)]
    struct MockFactory {
        last_facts: Mutex<Option<ListFactsParams>>,
        last_events: Mutex<Option<ListEventsParams>>,
        last_search: Mutex<Option<SearchRequest>>,
        last_dissents: Mutex<Option<ListDissentsParams>>,
        last_promote: Mutex<Option<(Uuid, Source, i32)>>,
        last_discard: Mutex<Option<(Uuid, Source)>>,
        last_upsert: Mutex<Option<UpsertFactRequest>>,
        last_edit: Mutex<Option<(Uuid, FactType, serde_json::Value, i32)>>,
        last_deleted: Mutex<Option<Uuid>>,
        last_memories: Mutex<Option<ListMemoriesParams>>,
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
        async fn memory_context(
            &self,
            _req: ContextRequest,
        ) -> Result<ContextBundle, ViewportError> {
            Err(ViewportError::Server {
                status: 501,
                message: "mock".into(),
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
                contract: None,
                maintenance: None,
            })
        }
        async fn list_dissents(&self, p: ListDissentsParams) -> Result<DissentPage, ViewportError> {
            if let Some(e) = self.fail.lock().unwrap().clone() {
                return Err(e);
            }
            *self.last_dissents.lock().unwrap() = Some(p);
            Ok(DissentPage {
                items: vec![],
                next_cursor: None,
            })
        }
        async fn get_dissent(&self, _id: Uuid) -> Result<Dissent, ViewportError> {
            if let Some(e) = self.fail.lock().unwrap().clone() {
                return Err(e);
            }
            Err(ViewportError::Server {
                status: 404,
                message: "no".into(),
            })
        }
        async fn promote_dissent(
            &self,
            id: Uuid,
            caller_source: Source,
            expected_version: i32,
        ) -> Result<Fact, ViewportError> {
            if let Some(e) = self.fail.lock().unwrap().clone() {
                return Err(e);
            }
            *self.last_promote.lock().unwrap() = Some((id, caller_source, expected_version));
            Err(ViewportError::Server {
                status: 404,
                message: "no".into(),
            })
        }
        async fn discard_dissent(
            &self,
            id: Uuid,
            caller_source: Source,
        ) -> Result<Dissent, ViewportError> {
            if let Some(e) = self.fail.lock().unwrap().clone() {
                return Err(e);
            }
            *self.last_discard.lock().unwrap() = Some((id, caller_source));
            Ok(Dissent {
                id,
                fact_id: Uuid::nil(),
                proposed_payload: serde_json::Value::Null,
                source: caller_source,
                status: DissentStatus::Discarded,
                submitted_at: time::OffsetDateTime::UNIX_EPOCH,
                last_seen_at: time::OffsetDateTime::UNIX_EPOCH,
                submission_count: 1,
                resolved_at: Some(time::OffsetDateTime::UNIX_EPOCH),
                resolved_by_source: Some(caller_source),
            })
        }
        async fn upsert_fact(
            &self,
            req: UpsertFactRequest,
        ) -> Result<FactWriteOutcome, ViewportError> {
            if let Some(e) = self.fail.lock().unwrap().clone() {
                return Err(e);
            }
            *self.last_upsert.lock().unwrap() = Some(req);
            Ok(FactWriteOutcome::VersionConflict {
                current_version: 1,
                fact_id: Uuid::nil(),
            })
        }
        async fn delete_fact(&self, id: Uuid) -> Result<(), ViewportError> {
            if let Some(e) = self.fail.lock().unwrap().clone() {
                return Err(e);
            }
            *self.last_deleted.lock().unwrap() = Some(id);
            Ok(())
        }
        async fn edit_fact(
            &self,
            id: Uuid,
            fact_type: FactType,
            payload: serde_json::Value,
            expected_version: i32,
        ) -> Result<FactWriteOutcome, ViewportError> {
            if let Some(e) = self.fail.lock().unwrap().clone() {
                return Err(e);
            }
            *self.last_edit.lock().unwrap() = Some((id, fact_type, payload, expected_version));
            Ok(FactWriteOutcome::VersionConflict {
                current_version: 1,
                fact_id: id,
            })
        }
        async fn list_authors(
            &self,
            _params: klams_types::ListAuthorsParams,
        ) -> Result<klams_types::AuthorPage, ViewportError> {
            Ok(klams_types::AuthorPage {
                authors: vec![],
                next_cursor: None,
            })
        }
        async fn get_author(&self, _id: Uuid) -> Result<klams_types::PublicAuthor, ViewportError> {
            Err(ViewportError::Server {
                status: 404,
                message: "no".into(),
            })
        }
        async fn list_author_memories(
            &self,
            _id: Uuid,
            _params: klams_types::ListAuthorMemoriesParams,
        ) -> Result<klams_types::AuthorMemoriesPage, ViewportError> {
            Ok(klams_types::AuthorMemoriesPage {
                memories: vec![],
                next_cursor: None,
            })
        }
        async fn list_memories(
            &self,
            params: ListMemoriesParams,
        ) -> Result<MemoriesPage, ViewportError> {
            if let Some(e) = self.fail.lock().unwrap().clone() {
                return Err(e);
            }
            *self.last_memories.lock().unwrap() = Some(params);
            Ok(MemoriesPage {
                memories: vec![],
                next_cursor: None,
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
            ..ListEventsParams::default()
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

    #[tokio::test]
    async fn list_dissents_forwards_filter_args() {
        let mock = Arc::new(MockFactory::default());
        let args = ListDissentsArgs {
            status: Some("pending".into()),
            source: Some("AgentProposal".into()),
            limit: Some(20),
            ..Default::default()
        };
        let _ = mock.list_dissents(args.into()).await.unwrap();
        let captured = mock.last_dissents.lock().unwrap().clone().unwrap();
        assert_eq!(captured.status.as_deref(), Some("pending"));
        assert_eq!(captured.source.as_deref(), Some("AgentProposal"));
        assert_eq!(captured.limit, Some(20));
    }

    #[tokio::test]
    async fn promote_dissent_forwards_caller_source_and_version() {
        let mock = Arc::new(MockFactory::default());
        let id = Uuid::now_v7();
        let _ = mock.promote_dissent(id, Source::User, 7).await;
        let (cid, src, ver) = (*mock.last_promote.lock().unwrap()).unwrap();
        assert_eq!(cid, id);
        assert!(matches!(src, Source::User));
        assert_eq!(ver, 7);
    }

    #[tokio::test]
    async fn promote_dissent_surfaces_403_trust_required() {
        let mock = Arc::new(MockFactory::default());
        *mock.fail.lock().unwrap() = Some(ViewportError::Server {
            status: 403,
            message: "trust_required".into(),
        });
        let err = mock
            .promote_dissent(Uuid::now_v7(), Source::AgentProposal, 1)
            .await
            .unwrap_err();
        match err {
            ViewportError::Server { status, .. } => assert_eq!(status, 403),
            other => panic!("expected Server 403, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn discard_dissent_returns_discarded_row() {
        let mock = Arc::new(MockFactory::default());
        let id = Uuid::now_v7();
        let out = mock.discard_dissent(id, Source::User).await.unwrap();
        assert_eq!(out.id, id);
        assert!(matches!(out.status, klams_types::DissentStatus::Discarded));
    }

    #[tokio::test]
    async fn upsert_fact_returns_outcome_variant() {
        let mock = Arc::new(MockFactory::default());
        let req = UpsertFactRequest {
            fact_type: FactType::UserFact,
            payload: serde_json::json!({"k": "v"}),
            source: Source::User,
            explicit_id: None,
            expected_version: Some(0),
        };
        let out = mock.upsert_fact(req).await.unwrap();
        match out {
            FactWriteOutcome::VersionConflict {
                current_version, ..
            } => assert_eq!(current_version, 1),
            other => panic!("expected VersionConflict, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn edit_fact_records_id_and_version() {
        let mock = Arc::new(MockFactory::default());
        let id = Uuid::now_v7();
        let _ = mock
            .edit_fact(id, FactType::UserFact, serde_json::json!({"k": "v2"}), 3)
            .await
            .unwrap();
        let (cid, _ft, _p, ver) = mock.last_edit.lock().unwrap().clone().unwrap();
        assert_eq!(cid, id);
        assert_eq!(ver, 3);
    }

    #[tokio::test]
    async fn delete_fact_records_id_and_propagates_410() {
        let mock = Arc::new(MockFactory::default());
        let id = Uuid::now_v7();
        mock.delete_fact(id).await.unwrap();
        assert_eq!(*mock.last_deleted.lock().unwrap(), Some(id));

        *mock.fail.lock().unwrap() = Some(ViewportError::Server {
            status: 410,
            message: "gone".into(),
        });
        let err = mock.delete_fact(Uuid::now_v7()).await.unwrap_err();
        match err {
            ViewportError::Server { status, .. } => assert_eq!(status, 410),
            other => panic!("expected Server 410, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn list_memories_forwards_activity_filters() {
        let mock = Arc::new(MockFactory::default());
        let params = ListMemoriesParams {
            since: Some("2026-05-25T00:00:00Z".into()),
            until: Some("2026-05-26T00:00:00Z".into()),
            kinds: Some("event,knowledge".into()),
            state: Some("all".into()),
            authors: Some(
                "00000000-0000-0000-0000-000000000001,00000000-0000-0000-0000-000000000002".into(),
            ),
            limit: Some(75),
            cursor: Some("opaque-cursor".into()),
        };

        let _ = mock.list_memories(params.clone()).await.unwrap();
        let captured = mock.last_memories.lock().unwrap().clone().unwrap();
        assert_eq!(captured.kinds.as_deref(), Some("event,knowledge"));
        assert_eq!(captured.state.as_deref(), Some("all"));
        assert_eq!(captured.limit, Some(75));
        assert_eq!(captured.cursor.as_deref(), Some("opaque-cursor"));
    }
}
