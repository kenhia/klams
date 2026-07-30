//! `GET /healthz` handler.
//!
//! Probes Postgres, Qdrant, and the embedder service in parallel,
//! reads queue depth/capacity/workers from `ApiState`, computes
//! aggregate status (Ok if all Ok; Degraded if some are Degraded but
//! none Down; Down otherwise), and returns 200 + `HealthSnapshot` on
//! Ok or 503 + `HealthSnapshot` otherwise.
//!
//! Probe results are cached for ~2s in `ProbeCache` so health
//! storms (typical with k8s liveness/readiness) don't hammer the
//! backends.

use crate::router::ApiState;
use axum::{
    extract::{Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use klams_store::Store;
use klams_types::{
    HealthSnapshot, HealthStatus, MaintenanceSnapshot, QueueStatus, SubsystemStatus,
};
use serde::Deserialize;
use std::sync::Mutex;
use std::time::{Duration, Instant};

const CACHE_TTL: Duration = Duration::from_secs(2);

#[derive(Default)]
struct CachedProbe {
    last: Option<(Instant, SubsystemStatus)>,
}

static PG_CACHE: Mutex<CachedProbe> = Mutex::new(CachedProbe { last: None });
static QDRANT_CACHE: Mutex<CachedProbe> = Mutex::new(CachedProbe { last: None });
static TEI_CACHE: Mutex<CachedProbe> = Mutex::new(CachedProbe { last: None });

fn cached(cache: &Mutex<CachedProbe>) -> Option<SubsystemStatus> {
    let guard = cache.lock().ok()?;
    let (when, ref val) = guard.last.as_ref()?;
    if when.elapsed() < CACHE_TTL {
        Some(val.clone())
    } else {
        None
    }
}

fn store_probe(cache: &Mutex<CachedProbe>, value: SubsystemStatus) {
    if let Ok(mut guard) = cache.lock() {
        guard.last = Some((Instant::now(), value));
    }
}

fn ok() -> SubsystemStatus {
    SubsystemStatus {
        state: HealthStatus::Ok,
        message: None,
    }
}

fn down(msg: String) -> SubsystemStatus {
    SubsystemStatus {
        state: HealthStatus::Down,
        message: Some(msg),
    }
}

async fn probe_pg<S: Store>(store: &S) -> SubsystemStatus {
    if let Some(v) = cached(&PG_CACHE) {
        return v;
    }
    let v = match store.health_postgres().await {
        Ok(()) => ok(),
        Err(e) => down(e.to_string()),
    };
    store_probe(&PG_CACHE, v.clone());
    v
}

async fn probe_qdrant<S: Store>(store: &S) -> SubsystemStatus {
    if let Some(v) = cached(&QDRANT_CACHE) {
        return v;
    }
    let v = match store.health_qdrant().await {
        Ok(()) => ok(),
        Err(e) => down(e.to_string()),
    };
    store_probe(&QDRANT_CACHE, v.clone());
    v
}

async fn probe_tei<S: Store>(store: &S) -> SubsystemStatus {
    if let Some(v) = cached(&TEI_CACHE) {
        return v;
    }
    let v = match store.health_embedder().await {
        Ok(()) => ok(),
        Err(e) => down(e.to_string()),
    };
    store_probe(&TEI_CACHE, v.clone());
    v
}

/// Sprint 036 (#731): probe the second-stage reranker when configured.
/// `TeiReranker::health()` had zero callers before this — the stage
/// could sit dead for a week with only the `rerank_skipped` counter as
/// a signal. Returns `None` when the stage is off (the field is then
/// omitted from the snapshot entirely, so an unconfigured deployment
/// doesn't dangle a permanently-"down" subsystem).
///
/// The cache is keyed by the reranker's URL: the other probes cache
/// per-process because a process has exactly one store, but tests spin
/// up several routers with different mock rerankers in one process, and
/// a URL-blind cache would serve one server's verdict for another.
static RERANKER_CACHE: Mutex<Option<(Instant, String, SubsystemStatus)>> = Mutex::new(None);

async fn probe_reranker(
    reranker: Option<&std::sync::Arc<klams_store::TeiReranker>>,
) -> Option<SubsystemStatus> {
    let reranker = reranker?;
    let url = reranker.base_url().to_string();
    if let Ok(guard) = RERANKER_CACHE.lock() {
        if let Some((when, cached_url, v)) = guard.as_ref() {
            if *cached_url == url && when.elapsed() < CACHE_TTL {
                return Some(v.clone());
            }
        }
    }
    let v = match reranker.health().await {
        Ok(()) => ok(),
        Err(e) => down(e.to_string()),
    };
    if let Ok(mut guard) = RERANKER_CACHE.lock() {
        *guard = Some((Instant::now(), url, v.clone()));
    }
    Some(v)
}

/// Sprint 032 (#335): the `Degraded` arm below is currently
/// unreachable — `probe_pg`/`probe_qdrant`/`probe_tei` only ever build
/// `ok()` or `down()`. It is kept deliberately rather than deleted: it
/// is the correct aggregation rule, and dropping it would mean a probe
/// that later grows a partial-failure state silently aggregates to
/// `Ok`. Two lines of correct branch beat a latent wrong answer.
fn aggregate(parts: &[&SubsystemStatus]) -> HealthStatus {
    if parts.iter().any(|s| s.state == HealthStatus::Down) {
        HealthStatus::Down
    } else if parts.iter().any(|s| s.state == HealthStatus::Degraded) {
        HealthStatus::Degraded
    } else {
        HealthStatus::Ok
    }
}

pub async fn healthz<S: Store>(
    State(state): State<ApiState<S>>,
    Query(params): Query<HealthzParams>,
) -> Response {
    let (pg, qd, tei, reranker) = tokio::join!(
        probe_pg(state.store.as_ref()),
        probe_qdrant(state.store.as_ref()),
        probe_tei(state.store.as_ref()),
        probe_reranker(state.reranker.as_ref()),
    );
    // The reranker is deliberately absent from the aggregate: the stage
    // is best-effort (searches serve the un-reranked order when it is
    // sick), so it must be visible here without ever flipping overall
    // status or the HTTP code (#731).
    let agg = aggregate(&[&pg, &qd, &tei]);

    let snapshot = HealthSnapshot {
        status: agg,
        postgres: pg,
        qdrant: qd,
        embeddings: tei,
        reranker,
        queue: QueueStatus {
            depth: state.queue.depth(),
            capacity: state.queue_capacity,
            workers: state.workers,
        },
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: state.started_at.elapsed().as_secs(),
        contract: params.contract.filter(|v| v == "v1"),
        maintenance: Some(MaintenanceSnapshot::from_state(&state.maintenance)),
    };

    let code = match agg {
        HealthStatus::Ok => StatusCode::OK,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    (code, Json(snapshot)).into_response()
}

#[derive(Debug, Default, Deserialize)]
pub struct HealthzParams {
    #[serde(default)]
    pub contract: Option<String>,
}
