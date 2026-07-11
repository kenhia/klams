//! klams-scanner library surface.

pub mod chunk;
pub mod code;
pub mod cursor;
pub mod metrics;
pub mod publish;
pub mod walk;

use anyhow::Result;
use klams_client::Client;
use std::collections::HashSet;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[must_use]
pub fn banner() -> &'static str {
    "klams-scanner ready"
}

/// Walk a single root, diff against cursor, publish changes, prune
/// vanished files. Exposed at the library level so the integration
/// suite can drive it directly without spawning the binary.
pub async fn scan_root(
    client: &Client,
    base_url: &str,
    bearer: &str,
    cursor_path: &Path,
    root: &Path,
) -> Result<()> {
    use chunk::{chunk, sha256_hex, Lang};
    use cursor::Cursor;
    use publish::{publish_chunk, publish_delete};
    use walk::walk;

    let cursor = Cursor::open(cursor_path)?;
    let files = walk(root);
    let mut seen: HashSet<String> = HashSet::new();
    let repo = root.file_name().map_or_else(
        || root.display().to_string(),
        |s| s.to_string_lossy().into_owned(),
    );

    for f in files {
        let abs = f.absolute_path.display().to_string();
        seen.insert(abs.clone());

        let prev = cursor.get(&abs)?;
        if let Some(prev) = &prev {
            if prev.mtime_ns == f.mtime_ns {
                metrics::incr_skipped("mtime_unchanged");
                continue;
            }
        }

        let body = match std::fs::read_to_string(&f.absolute_path) {
            Ok(b) => b,
            Err(e) => {
                tracing::debug!(path = %abs, %e, "skip non-utf8");
                metrics::incr_skipped("read_error");
                continue;
            }
        };
        let chunks = chunk(&body, Lang::from_path(&abs));
        let file_hash = sha256_hex(&body);

        if let Some(prev) = &prev {
            if prev.content_hash == file_hash {
                cursor.upsert(&abs, &file_hash, f.mtime_ns, now_seconds_i64())?;
                metrics::incr_skipped("hash_unchanged");
                continue;
            }
        }

        // Delete-before-reindex (sprint 021, #315): a previously indexed
        // file whose content changed must have its OLD points removed
        // before the new chunks land — otherwise the stale versions stay
        // live and searchable and the corpus re-pollutes itself on every
        // edit. `scan_root` only pruned *vanished* files before this, so
        // edit-churn leaked chunks continuously. Skip on delete failure
        // (leave the cursor unadvanced to retry) rather than publish new
        // chunks on top of stale ones.
        if prev.is_some() {
            match publish_delete(base_url, bearer, &abs).await {
                Ok(n) => {
                    if n > 0 {
                        tracing::info!(path = %abs, deleted = n, "cleared stale chunks before reindex");
                    }
                }
                Err(e) => {
                    tracing::warn!(path = %abs, %e, "delete-before-reindex failed; leaving cursor unadvanced for retry");
                    metrics::incr_skipped("delete_failed");
                    continue;
                }
            }
        }

        let mut publish_failed = false;
        for c in &chunks {
            if let Err(e) = publish_chunk(client, &repo, &abs, c).await {
                tracing::warn!(path = %abs, idx = c.index, %e, "publish_chunk failed");
                publish_failed = true;
            }
        }
        if publish_failed {
            // Leave the cursor unadvanced so the next scan retries this
            // file — otherwise the mtime/hash short-circuit would skip it
            // forever and the dropped chunks would never be ingested.
            tracing::warn!(path = %abs, "publish incomplete; leaving cursor unadvanced for retry");
            metrics::incr_skipped("publish_failed");
            continue;
        }
        metrics::add_chunks(chunks.len() as u64);
        cursor.upsert(&abs, &file_hash, f.mtime_ns, now_seconds_i64())?;
        metrics::incr_processed();
    }

    // Prune vanished files — but ONLY within the current root's subtree.
    // `cursor.list_all()` spans every configured root while `seen` holds
    // just this root's paths, so without the prefix guard a multi-root
    // scan would treat every *other* root's files as "vanished" and
    // delete them from knowledge (each root would wipe the previous one).
    for prev in cursor.list_all()? {
        if seen.contains(&prev.absolute_path) {
            continue;
        }
        if !Path::new(&prev.absolute_path).starts_with(root) {
            // Belongs to a different root; not this scan's responsibility.
            continue;
        }
        match publish_delete(base_url, bearer, &prev.absolute_path).await {
            Ok(n) => {
                tracing::info!(path = %prev.absolute_path, deleted = n, "pruned");
                cursor.delete(&prev.absolute_path)?;
            }
            Err(e) => tracing::warn!(path = %prev.absolute_path, %e, "delete failed"),
        }
    }
    Ok(())
}

fn now_seconds_i64() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| i64::try_from(d.as_secs()).unwrap_or(0))
        .unwrap_or_default()
}
