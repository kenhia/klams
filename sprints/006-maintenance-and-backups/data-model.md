# Phase 1 Data Model — Maintenance, Backups, and Ops

No new Postgres tables, no new Qdrant collections, no new persistent
state. All structures are runtime-only inside `klams-service`.

## In-process structures

### `BackupConfig` (`klams-types::config::BackupConfig`)

```rust
#[derive(Debug, Clone, serde::Deserialize)]
pub struct BackupConfig {
    #[serde(default)] pub enabled: bool,
    pub backup_dir: PathBuf,
    /// 24h UTC, "HH:MM". UTC-only to dodge DST.
    pub window_start_utc: WindowStartUtc,
    #[serde(default = "default_daily_count")]  pub daily_count: u32,   // 14
    #[serde(default = "default_weekly_count")] pub weekly_count: u32,  // 4
    #[serde(default)] pub same_day_strategy: SameDayStrategy,          // Suffix
    pub status_hook: Option<PathBuf>,
    #[serde(default = "default_hook_timeout", with = "humantime_serde")]
    pub status_hook_timeout: Duration,                                 // 10s
}

#[derive(Debug, Clone, Copy)]
pub struct WindowStartUtc { pub hour: u8, pub minute: u8 }

#[derive(Debug, Clone, Copy, serde::Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SameDayStrategy { #[default] Suffix, Overwrite }
```

Validation at service start (FR-012):

- If `enabled == true` and `backup_dir` is unset, missing, or not
  writable → exit code `2`.
- If `window_start_utc` doesn't parse as `HH:MM` with `0 ≤ hh ≤ 23`,
  `0 ≤ mm ≤ 59` → exit code `2`.
- If `status_hook` is `Some(path)` and path is not an executable
  file → exit code `2`.

### `BackupRun` (`klams-service::backup::lifecycle::BackupRun`)

```rust
pub struct BackupRun {
    pub run_id: Ulid,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub ok: Option<bool>,                  // None while in progress
    pub artifacts: Vec<BackupArtifact>,    // appended as each completes
    pub error: Option<String>,             // first failure, if any
}
```

State machine:

```
                  start_window()
   Idle ──────────────────────────▶ Running { run_id, started_at }
                                       │ artifact_done(BackupArtifact { ok: true })
                                       │ artifact_done(BackupArtifact { ok: false, error: ... })
                                       ▼
                                    Running with ≥1 artifact
                                       │ finish()
              ┌────────────────────────┴────────────────────────┐
              ▼                                                 ▼
   Finished { ok: true, … }                          Finished { ok: false, error }
```

State transitions emit a corresponding `status_hook` invocation
(`started`, then exactly one of `finished` | `failed` at end).

### `BackupArtifact`

```rust
pub struct BackupArtifact {
    pub kind: ArtifactKind,           // Postgres | Qdrant
    pub path: PathBuf,                // final (post-rename) path
    pub bytes: u64,                   // file size after success; 0 on failure
    pub duration_ms: u64,
    pub ok: bool,
    pub error: Option<String>,
}

pub enum ArtifactKind { Postgres, Qdrant }
```

### `MaintenanceState` (`klams-service::backup::MaintenanceState`)

```rust
#[derive(Clone)]
pub struct MaintenanceState {
    active: Arc<AtomicBool>,
    inflight: Arc<RwLock<Option<RunningSnapshot>>>,
}

pub struct RunningSnapshot {
    pub run_id: Ulid,
    pub started_at: DateTime<Utc>,
    pub expected_end_at: Option<DateTime<Utc>>, // mean of last 5 successful durations
}
```

- `active.load(Relaxed)` is the hot-path read used by the middleware.
- `inflight` is only read by `/healthz` and a future operator
  endpoint; it's a `RwLock` because writes happen ~2× per backup
  (`start`, `clear`) and reads are infrequent.

### `BackupHookEvent` (`klams-service::backup::hook::BackupHookEvent`)

The Rust struct that serializes to the contract in
`contracts/backup-status-hook.schema.json`.

```rust
#[derive(serde::Serialize)]
pub struct BackupHookEvent<'a> {
    pub schema_version: u32,                     // always 1 this sprint
    pub run_id: Ulid,
    pub event: HookEventKind,                    // started | finished | failed
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub artifacts: &'a [BackupArtifact],         // empty on `started`
    pub ok: bool,                                // false on `started`
    pub error: Option<&'a str>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum HookEventKind { Started, Finished, Failed }
```

## HTTP envelope additions

### 503 maintenance-mode response

```http
HTTP/1.1 503 Service Unavailable
Retry-After: 600
Content-Type: application/json

{
  "error": "maintenance_window_active",
  "retry_after_seconds": 600
}
```

`retry_after_seconds` is the seconds remaining until
`expected_end_at` (clamped to a configurable floor of 30s). If the
window has overrun, the value is the configured floor.

### `/healthz` extension

Additive. Existing fields unchanged.

```json
{
  "status": "ok",
  "version": "...",
  "uptime_seconds": ...,
  "maintenance": {
    "active": true,
    "run_id": "01HZP...",
    "started_at": "2026-05-23T10:00:01Z",
    "expected_end_at": "2026-05-23T10:04:30Z"
  }
}
```

`maintenance` is `{"active": false}` when no backup is running.

## File-system layout in `backup_dir`

```
<backup_dir>/
├── postgres-2026-05-22.dump
├── postgres-2026-05-23.dump
├── postgres-2026-05-23.dump.partial    # absent unless a backup is mid-flight or crashed
├── qdrant-2026-05-22.snapshot
├── qdrant-2026-05-23.snapshot
└── lockfile                            # holds {pid, run_id, started_at}; absent when idle
```

`lockfile` exists for the lifetime of a `BackupRun`. On service
startup, klams checks for a stale lockfile (pid not alive); if found,
it fires the hook with `event: "failed"` /
`error: "service_restarted_mid_backup"`, increments the failure
counter, and deletes the lockfile + any `.partial` files dated
later than the most recent committed snapshot.

## Prometheus series schema

| Series | Type | Labels | Source |
|--------|------|--------|--------|
| `klams_backup_last_success_timestamp_seconds` | gauge | _none_ | written on transition Finished{ok:true} |
| `klams_backup_duration_seconds` | histogram | `kind ∈ {postgres, qdrant}` | observed per-artifact on completion |
| `klams_backup_runs_total` | counter | `ok ∈ {true, false}` | incremented on transition to Finished |
| `klams_backup_hook_invocations_total` | counter | `event ∈ {started, finished, failed}, ok ∈ {true, false}` | incremented per hook invocation |
| `klams_maintenance_mode_active` | gauge | _none_ | mirrors `MaintenanceState::active` |

Histogram buckets for `klams_backup_duration_seconds`: default
`prometheus::DEFAULT_BUCKETS` augmented with `[300, 600, 1800, 3600]`
(homelab backups can legitimately take minutes; the default top
bucket of 10s is useless here).

## Relationships

```
Configuration → drives → BackupTask (one per service)
BackupTask    → owns   → MaintenanceState (shared with axum middleware)
              → emits  → BackupHookEvent (3× per run when status_hook set)
              → writes → files in backup_dir
              → updates→ Prometheus series
axum middleware reads MaintenanceState on every non-GET request
```

## Invariants

- Exactly one `BackupRun` is in flight at any time (enforced by
  lockfile + `Mutex<Option<BackupRun>>`).
- For each successful `BackupRun`, both `BackupArtifact` entries
  exist on disk under their final (non-`.partial`) names by the
  time the `finished` hook fires.
- `klams_maintenance_mode_active == 1` iff there is an in-flight
  `BackupRun`.
- `schema_version` in every hook payload this sprint is `1`. Future
  sprints bump it.
