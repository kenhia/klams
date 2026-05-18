//! Background decay task + `last_used_at` write coalescer
//! (sprint 002 US3).
//!
//! Reads send fact ids over a bounded `tokio::mpsc` channel; the
//! decay loop drains the channel, flushes a single
//! `UPDATE … WHERE id = ANY(...)` for the bumps, then walks the
//! `facts` table in batches and recomputes
//! `decay_weight = base × (1 / (1 + λ_type × age_seconds))`.

use crate::metrics as m;
use klams_store::{DecayStore, StoreResult};
pub use klams_types::DecayConfig;
use std::sync::Arc;
use tokio::sync::mpsc;
use tokio::task::yield_now;
use tracing::{error, info, warn};
use uuid::Uuid;

/// Bounded sender used by every fact-returning read to flag the row
/// for a `last_used_at` bump. Capacity is fixed at 1024 per
/// research §5; over-capacity sends increment a metric and are
/// silently dropped (the bump is best-effort).
#[derive(Debug, Clone)]
pub struct LastUsedBumper {
    tx: mpsc::Sender<Uuid>,
}

impl LastUsedBumper {
    pub const CAPACITY: usize = 1024;

    /// Construct the sender/receiver pair. Hand the receiver to
    /// `DecayTask::with_bumper_rx` (or drain it manually in tests).
    #[must_use]
    pub fn channel() -> (Self, mpsc::Receiver<Uuid>) {
        let (tx, rx) = mpsc::channel(Self::CAPACITY);
        (Self { tx }, rx)
    }

    /// Fire-and-forget enqueue. Drops on full channel and bumps
    /// `klams_last_used_bumps_dropped_total`.
    pub fn send_lossy(&self, id: Uuid) {
        if self.tx.try_send(id).is_err() {
            m::incr_last_used_bumps_dropped();
        }
    }

    /// Borrow the inner sender so the store crate (which cannot
    /// depend on `klams-core`) can flag fact reads without
    /// duplicating the wrapper type.
    #[must_use]
    pub fn sender(&self) -> mpsc::Sender<Uuid> {
        self.tx.clone()
    }
}

/// Drain whatever bumps are immediately available (no `await` past
/// the first item once data has been collected) and apply them as
/// one batched UPDATE. Returns the number of unique ids flushed.
async fn drain_into_batch<S: DecayStore + ?Sized>(
    rx: &mut mpsc::Receiver<Uuid>,
    store: &S,
) -> StoreResult<usize> {
    let mut ids = Vec::new();
    while let Ok(id) = rx.try_recv() {
        ids.push(id);
    }
    if ids.is_empty() {
        return Ok(0);
    }
    ids.sort_unstable();
    ids.dedup();
    let n = ids.len();
    store.apply_last_used_bumps(&ids).await?;
    Ok(n)
}

/// Compute the decayed weight from a base, type-specific lambda, and
/// elapsed age in seconds. Always returns a value in `(0, base]`.
#[must_use]
pub fn score(base: f32, lambda: f32, age_seconds: f32) -> f32 {
    let age = age_seconds.max(0.0);
    let denom = 1.0_f32 + lambda * age;
    base / denom
}

/// Per-row update payload accepted by `Store::apply_decay_batch`.
pub type DecayUpdate = (Uuid, f32);

pub struct DecayTask<S: DecayStore + ?Sized> {
    cfg: DecayConfig,
    store: Arc<S>,
    bumps_rx: Option<mpsc::Receiver<Uuid>>,
}

impl<S: DecayStore + ?Sized> std::fmt::Debug for DecayTask<S> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DecayTask")
            .field("cfg", &self.cfg)
            .field("has_bumps_rx", &self.bumps_rx.is_some())
            .finish_non_exhaustive()
    }
}

impl<S: DecayStore + ?Sized> DecayTask<S> {
    pub fn new(cfg: DecayConfig, store: Arc<S>) -> Self {
        Self {
            cfg,
            store,
            bumps_rx: None,
        }
    }

    #[must_use]
    pub fn with_bumps_rx(mut self, rx: mpsc::Receiver<Uuid>) -> Self {
        self.bumps_rx = Some(rx);
        self
    }

    /// Long-running loop. Sleeps for `task_interval`, then runs one
    /// `tick_once`. Errors are logged and the loop continues so a
    /// transient backend hiccup never takes the task down.
    pub async fn run(mut self) {
        info!(
            interval_seconds = self.cfg.task_interval_seconds,
            batch_size = self.cfg.batch_size,
            "decay task started"
        );
        loop {
            tokio::time::sleep(self.cfg.task_interval()).await;
            if let Err(e) = self.tick_once().await {
                error!(error = %e, "decay tick failed");
            }
        }
    }

    /// Single pass: flush any pending `last_used_at` bumps then walk
    /// the `facts` table in batches and apply recomputed weights.
    /// Returns the total number of facts updated this tick.
    pub async fn tick_once(&mut self) -> StoreResult<u64> {
        if let Some(rx) = self.bumps_rx.as_mut() {
            match drain_into_batch(rx, self.store.as_ref()).await {
                Ok(n) if n > 0 => info!(bumps = n, "flushed last_used_at bumps"),
                Ok(_) => {}
                Err(e) => warn!(error = %e, "last_used_at flush failed"),
            }
        }

        let batch_size = self.cfg.batch_size.max(1);
        let mut after_id: Option<Uuid> = None;
        let mut total: u64 = 0;
        loop {
            let rows = self.store.select_decay_batch(after_id, batch_size).await?;
            if rows.is_empty() {
                break;
            }
            let updates: Vec<DecayUpdate> = rows
                .iter()
                .map(|r| {
                    let lambda = self.cfg.lambda_for(r.fact_type);
                    (r.id, score(1.0, lambda, r.age_seconds))
                })
                .collect();
            self.store.apply_decay_batch(&updates).await?;
            #[allow(clippy::cast_possible_truncation)]
            let n = rows.len() as u64;
            total += n;
            m::incr_decay_facts_updated(n);
            after_id = rows.last().map(|r| r.id);
            yield_now().await;
            if rows.len() < batch_size as usize {
                break;
            }
        }
        m::incr_decay_run();
        Ok(total)
    }
}

/// Test-only seam: drain the bumps channel synchronously without a
/// full tick. Useful for assertions about coalescing behaviour.
pub async fn flush_bumps_for_test<S: DecayStore + ?Sized>(
    rx: &mut mpsc::Receiver<Uuid>,
    store: &S,
) -> StoreResult<usize> {
    drain_into_batch(rx, store).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn score_is_monotonically_non_increasing_in_age() {
        let lambda = 1e-6_f32;
        let a = score(1.0, lambda, 0.0);
        let b = score(1.0, lambda, 86_400.0);
        let c = score(1.0, lambda, 7.0 * 86_400.0);
        assert!(a >= b && b >= c, "{a} >= {b} >= {c}");
        assert!(a > 0.0 && c > 0.0);
    }

    #[test]
    fn score_clamps_negative_age_to_zero() {
        let s = score(1.0, 1e-6, -100.0);
        assert!((s - 1.0).abs() < f32::EPSILON);
    }

    #[tokio::test]
    async fn bumper_drops_on_full_channel() {
        let (bumper, mut rx) = LastUsedBumper::channel();
        for _ in 0..LastUsedBumper::CAPACITY {
            bumper.send_lossy(Uuid::now_v7());
        }
        // Channel is full; this one must be dropped (no panic).
        bumper.send_lossy(Uuid::now_v7());
        // Drain just to release the receiver cleanly.
        let mut drained = 0;
        while rx.try_recv().is_ok() {
            drained += 1;
        }
        assert_eq!(drained, LastUsedBumper::CAPACITY);
    }
}
