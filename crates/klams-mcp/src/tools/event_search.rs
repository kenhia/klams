//! `event_search` MCP tool (sprint 008, US1).
//!
//! Event-only read path with optional filters by author/category/date
//! window and payload JSON containment.

use crate::{
    errors::{self, envelope, envelope_with_window_max, ErrorEnvelope},
    metrics as mcp_metrics,
    tools::McpState,
};
use chrono::{DateTime, Duration, Utc};
use klams_store::{EventOrder, EventSearchQuery, Store};
use klams_types::PublicMemory;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

const DEFAULT_LIMIT: u32 = 50;
const MAX_LIMIT: u32 = 500;
const DEFAULT_WINDOW_HOURS: i64 = 24;

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum EventOrderInput {
    Desc,
    Asc,
}

impl From<EventOrderInput> for EventOrder {
    fn from(value: EventOrderInput) -> Self {
        match value {
            EventOrderInput::Desc => EventOrder::Desc,
            EventOrderInput::Asc => EventOrder::Asc,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum UuidOrMany {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(untagged)]
pub enum StringOrMany {
    One(String),
    Many(Vec<String>),
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct EventSearchArgs {
    #[serde(default)]
    pub author_id: Option<UuidOrMany>,
    #[serde(default)]
    pub category: Option<StringOrMany>,
    #[serde(default)]
    pub since: Option<String>,
    #[serde(default)]
    pub until: Option<String>,
    #[serde(default)]
    pub payload_match: Option<serde_json::Map<String, Value>>,
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub order: Option<EventOrderInput>,
    #[serde(default)]
    pub cursor: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EventSearchOutput {
    pub events: Vec<PublicMemory>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// Execute `event_search` with read-scope constraints already enforced
/// by the tool registry.
pub async fn run<S: Store>(
    state: &McpState<S>,
    args: EventSearchArgs,
    caller: Option<&str>,
) -> Result<EventSearchOutput, ErrorEnvelope> {
    let now = Utc::now();
    let until = parse_or_default(args.until.as_deref(), now, "until")?;
    let since = parse_or_default(
        args.since.as_deref(),
        until - Duration::hours(DEFAULT_WINDOW_HOURS),
        "since",
    )?;

    // Sprint 031 (#645): one copy of the rule, in klams-types. This
    // and `GET /v1/memories` had character-identical checks and
    // messages, which is two places to fix and one to forget — and the
    // two surfaces read the same `memories_max_window_days`, so they
    // are supposed to answer identically.
    let max_days = state.api.memories_max_window_days;
    if let Err(e) = klams_types::validate_window(since, until, max_days) {
        return Err(match e {
            klams_types::WindowError::Inverted { .. } => {
                envelope(errors::INVALID_WINDOW, e.message())
            }
            // The MCP envelope carries the ceiling in `_meta` so an
            // agent can retry with a legal window without guessing.
            klams_types::WindowError::TooLarge { max_days } => {
                envelope_with_window_max(errors::WINDOW_TOO_LARGE, e.message(), max_days)
            }
        });
    }

    let author_ids = normalize_uuids(args.author_id);
    let categories = normalize_categories(args.category)?;
    let limit = args.limit.unwrap_or(DEFAULT_LIMIT).clamp(1, MAX_LIMIT);
    let query = EventSearchQuery {
        author_ids,
        categories,
        since: Some(since),
        until: Some(until),
        payload_match: args.payload_match.map(Value::Object),
        limit,
        order: args.order.unwrap_or(EventOrderInput::Desc).into(),
        cursor: args.cursor,
    };

    let (events, next_cursor) = state
        .store
        .event_search(query)
        .await
        .map_err(|e| envelope(errors::INTERNAL_ERROR, format!("event_search: {e}")))?;

    // Keep span labels bounded; author_id is intentionally excluded.
    // Sprint 033 (#692): attribute to the caller — the 026 fix that
    // gave memory_search real agent labels never reached this tool,
    // so its metric series stayed a single "anonymous".
    let agent = crate::tools::memory_search::caller_label(caller);
    tracing::info!(
        token_hash = "unknown",
        agent_name = agent,
        model = "unknown",
        requested_window_hours = (until - since).num_hours(),
        result_count = events.len(),
        "mcp.event_search complete"
    );
    mcp_metrics::record_search(agent, None);

    Ok(EventSearchOutput {
        events,
        next_cursor,
    })
}

fn parse_or_default(
    raw: Option<&str>,
    default: DateTime<Utc>,
    field: &'static str,
) -> Result<DateTime<Utc>, ErrorEnvelope> {
    match raw {
        Some(v) => DateTime::parse_from_rfc3339(v)
            .map(|t| t.with_timezone(&Utc))
            .map_err(|e| {
                envelope(
                    errors::INVALID_WINDOW,
                    format!("invalid RFC3339 `{field}` timestamp: {e}"),
                )
            }),
        None => Ok(default),
    }
}

fn normalize_uuids(raw: Option<UuidOrMany>) -> Vec<uuid::Uuid> {
    let mut out = match raw {
        Some(UuidOrMany::One(v)) => vec![v],
        Some(UuidOrMany::Many(v)) => v,
        None => Vec::new(),
    }
    .into_iter()
    .filter_map(|v| uuid::Uuid::parse_str(v.trim()).ok())
    .collect::<Vec<_>>();
    out.sort_unstable();
    out.dedup();
    out
}

fn normalize_categories(raw: Option<StringOrMany>) -> Result<Vec<String>, ErrorEnvelope> {
    let mut out: Vec<String> = match raw {
        Some(StringOrMany::One(v)) => vec![v],
        Some(StringOrMany::Many(v)) => v,
        None => Vec::new(),
    }
    .into_iter()
    .map(|s| s.trim().to_string())
    .filter(|s| !s.is_empty())
    .collect();

    if out.iter().any(|s| s.len() > 128) {
        return Err(envelope(
            errors::INVALID_CATEGORY,
            "category entries must be 1..=128 characters",
        ));
    }

    out.sort();
    out.dedup();
    Ok(out)
}
