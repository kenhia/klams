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
    extract::State,
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use klams_store::Store;
use klams_types::{HealthSnapshot, HealthStatus, QueueStatus, SubsystemStatus};
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

fn aggregate(parts: &[&SubsystemStatus]) -> HealthStatus {
    if parts.iter().any(|s| s.state == HealthStatus::Down) {
        HealthStatus::Down
    } else if parts.iter().any(|s| s.state == HealthStatus::Degraded) {
        HealthStatus::Degraded
    } else {
        HealthStatus::Ok
    }
}

pub async fn healthz<S: Store>(State(state): State<ApiState<S>>) -> Response {
    let (pg, qd, tei) = tokio::join!(
        probe_pg(state.store.as_ref()),
        probe_qdrant(state.store.as_ref()),
        probe_tei(state.store.as_ref()),
    );
    let agg = aggregate(&[&pg, &qd, &tei]);

    let snapshot = HealthSnapshot {
        status: agg,
        postgres: pg,
        qdrant: qd,
        embeddings: tei,
        queue: QueueStatus {
            depth: state.queue.depth(),
            capacity: state.queue_capacity,
            workers: state.workers,
        },
        version: env!("CARGO_PKG_VERSION").to_string(),
        uptime_seconds: state.started_at.elapsed().as_secs(),
    };

    let code = match agg {
        HealthStatus::Ok => StatusCode::OK,
        _ => StatusCode::SERVICE_UNAVAILABLE,
    };
    (code, Json(snapshot)).into_response()
}

#[cfg(test)]
#[allow(dead_code)] // used by integration tests under cfg(test) in adjacent files
pub(crate) fn clear_cache_for_tests() {
    for c in [&PG_CACHE, &QDRANT_CACHE, &TEI_CACHE] {
        if let Ok(mut g) = c.lock() {
            g.last = None;
        }
    }
}
