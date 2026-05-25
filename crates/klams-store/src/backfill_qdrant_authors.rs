//! Sprint 007 T014 — one-shot Qdrant author backfill.
//!
//! Scrolls every point in the knowledge collection and stamps
//! `author_id = SYSTEM_AUTHOR_ID` on any point whose payload is missing
//! the field. The operation is idempotent: subsequent runs only touch
//! points still missing `author_id`, which (after the first complete
//! run) is the empty set.
//!
//! Run once at service startup, before `axum::serve()`, from
//! `klams-service::main`. A cancellation token allows the operator to
//! abort a long-running first-time backfill cleanly during shutdown.

use crate::QdrantStore;
use crate::{StoreError, StoreResult};
use klams_types::SYSTEM_AUTHOR_ID;
use qdrant_client::qdrant::{
    point_id::PointIdOptions, Condition, Filter, PointId, ScrollPointsBuilder,
    SetPayloadPointsBuilder, Value,
};
use tokio_util::sync::CancellationToken;
use tracing::{info, warn};

/// Run the backfill once. Returns the number of points patched. Safe to
/// call on a clean collection (no-op) and safe to re-run after a partial
/// pass (only points still missing `author_id` are touched).
///
/// # Errors
/// Bubbles any Qdrant scroll / `set_payload` error as
/// [`StoreError::Backend`].
pub async fn run_backfill(store: &QdrantStore, cancel: CancellationToken) -> StoreResult<u64> {
    let mut patched: u64 = 0;
    let mut offset: Option<PointId> = None;
    let page_size: u32 = 256;

    loop {
        if cancel.is_cancelled() {
            warn!(patched, "qdrant author backfill cancelled");
            return Ok(patched);
        }

        let filter = Filter {
            must: vec![Condition::is_empty("author_id")],
            ..Default::default()
        };

        let mut builder = ScrollPointsBuilder::new(store.collection().to_string())
            .filter(filter)
            .with_payload(false)
            .with_vectors(false)
            .limit(page_size);
        if let Some(off) = offset.clone() {
            builder = builder.offset(off);
        }

        let resp = store
            .client()
            .scroll(builder)
            .await
            .map_err(|e| StoreError::Backend(format!("qdrant backfill scroll: {e}")))?;

        if resp.result.is_empty() {
            break;
        }

        let ids: Vec<PointId> = resp.result.iter().filter_map(|p| p.id.clone()).collect();

        if !ids.is_empty() {
            let mut payload = std::collections::HashMap::new();
            payload.insert(
                "author_id".to_string(),
                Value::from(SYSTEM_AUTHOR_ID.to_string()),
            );
            store
                .client()
                .set_payload(
                    SetPayloadPointsBuilder::new(store.collection().to_string(), payload)
                        .points_selector(ids.clone())
                        .wait(true),
                )
                .await
                .map_err(|e| StoreError::Backend(format!("qdrant backfill set_payload: {e}")))?;
            patched = patched.saturating_add(ids.len() as u64);
        }

        offset = resp.next_page_offset;
        if offset.is_none() {
            break;
        }
    }

    if patched > 0 {
        info!(patched, "qdrant author backfill complete");
    }
    Ok(patched)
}

// Silence unused-import lint on `PointIdOptions` (kept for future extension
// to log skipped ids in verbose mode).
#[allow(dead_code)]
fn _keep_imports() -> PointIdOptions {
    PointIdOptions::Uuid(String::new())
}
