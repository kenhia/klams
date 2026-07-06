# Viewport Tauri Command Contract

The viewport's Svelte frontend talks to its Rust backend exclusively
through Tauri commands. Each command wraps a `klams-client` call,
maps errors to a uniform `ViewportError`, and returns DTOs that
mirror the shapes in [../contracts/openapi.yaml](openapi.yaml).

All commands are async. The Svelte side calls them via:

```ts
import { invoke } from "@tauri-apps/api/core";
const facts = await invoke<FactPage>("list_facts", { args: { ... } });
```

## Conventions

- Command names are `snake_case` Rust functions registered via
  `tauri::Builder::invoke_handler`.
- Each command takes a single `args` object so the Svelte caller can
  use a typed wrapper without long argument lists.
- All commands return `Result<T, ViewportError>`. Tauri surfaces
  `Err` as a JS exception on the frontend.
- DTOs (`FactPage`, `EventPage`, `KnowledgeItem`, `SearchResults`,
  `HealthSnapshot`) match the OpenAPI schemas exactly. They are
  re-exported from `klams-types` and mirrored in
  `viewport/src/lib/types.ts`.

## `ViewportError`

```rust
#[derive(serde::Serialize, thiserror::Error, Debug)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ViewportError {
    #[error("not configured: {0}")]
    NotConfigured(String),                          // missing URL or token
    #[error("network error: {0}")]
    Network(String),                                // reqwest transport failure
    #[error("auth failed")]
    Unauthorized,                                   // 401 from service
    #[error("server error {status}: {message}")]
    Server { status: u16, message: String },        // 4xx/5xx with body
    #[error("invalid response: {0}")]
    Deserialization(String),
}
```

## Commands

### `get_config`

Return the current viewport configuration (URL only; token presence
is reported as a boolean, the token itself is never returned to JS).

```rust
#[tauri::command]
async fn get_config() -> Result<ViewportConfig, ViewportError>;

pub struct ViewportConfig {
    pub klams_url: String,
    pub has_token: bool,
    pub refresh_interval_seconds: u32,
}
```

### `set_config`

Persist a new URL and/or token. Token is written to the OS credential
manager (`keyring` crate) under service `klams-viewport`, account
`bearer`. The TOML at `%APPDATA%/klams/viewport.toml` stores only the
URL and refresh interval.

```rust
#[tauri::command]
async fn set_config(args: SetConfigArgs) -> Result<(), ViewportError>;

pub struct SetConfigArgs {
    pub klams_url: Option<String>,
    pub bearer_token: Option<String>,
    pub refresh_interval_seconds: Option<u32>,
}
```

### `get_health`

Calls `GET /healthz`. Returns the parsed `HealthSnapshot` even when
the service returns 503 (degraded). Only transport failure produces
`Err`.

```rust
#[tauri::command]
async fn get_health() -> Result<HealthSnapshot, ViewportError>;
```

### `list_facts`

```rust
#[tauri::command]
async fn list_facts(args: ListFactsArgs) -> Result<FactPage, ViewportError>;

pub struct ListFactsArgs {
    pub fact_type:      Option<String>,    // -> ?type=
    pub source:         Option<String>,
    pub created_after:  Option<String>,    // RFC3339
    pub created_before: Option<String>,
    pub limit:          Option<u32>,
    pub cursor:         Option<String>,
}
```

### `list_events`

```rust
#[tauri::command]
async fn list_events(args: ListEventsArgs) -> Result<EventPage, ViewportError>;

pub struct ListEventsArgs {
    pub task_id:        Option<Uuid>,
    pub category:       Option<String>,
    pub created_after:  Option<String>,
    pub created_before: Option<String>,
    pub limit:          Option<u32>,
    pub cursor:         Option<String>,
}
```

### `search_knowledge`

Convenience wrapper over `POST /memory/search` with
`types: ["knowledge"]`. Returns the same `SearchResults` shape as
`search_unified` for UI uniformity.

```rust
#[tauri::command]
async fn search_knowledge(args: SearchArgs) -> Result<SearchResults, ViewportError>;
```

### `search_unified`

```rust
#[tauri::command]
async fn search_unified(args: SearchArgs) -> Result<SearchResults, ViewportError>;

pub struct SearchArgs {
    pub query:   String,
    pub types:   Option<Vec<String>>,                       // ["fact","event","knowledge"]
    pub filters: Option<serde_json::Value>,
    pub top_k:   Option<u32>,
}
```

### `get_fact`, `get_event`, `get_knowledge_item`

Detail-pane fetches by id. Used when a Svelte view opens the detail
panel for a single row. All three are direct passthroughs to the
service's by-id endpoints.

```rust
#[tauri::command] async fn get_fact(args: ByIdArgs) -> Result<Fact, ViewportError>;
#[tauri::command] async fn get_event(args: ByIdArgs) -> Result<Event, ViewportError>;
#[tauri::command] async fn get_knowledge_item(args: ByIdArgs) -> Result<KnowledgeItem, ViewportError>;

pub struct ByIdArgs { pub id: Uuid }
```

> Note: For Phase 1, `GET /memory/knowledge/{id}` exists in the
> OpenAPI contract (Qdrant point lookup by id). `get_fact` and
> `get_event` are implemented by calling the list endpoints with
> cursor paging until the id is found; dedicated `GET /memory/facts/{id}`
> and `GET /memory/events/{id}` endpoints can be added in Phase 2 if
> the workaround becomes a bottleneck.

## Events emitted to the frontend

The Rust backend emits two Tauri events to drive the connection
indicator without polling from JS:

- `klams://health` — payload is `HealthSnapshot`. Fired every
  `refresh_interval_seconds` (default 10 s) and on demand after
  `set_config`.
- `klams://config-changed` — payload is `ViewportConfig`. Fired after
  `set_config` completes successfully.

The Svelte `lib/stores.ts` subscribes to both via
`@tauri-apps/api/event`.
