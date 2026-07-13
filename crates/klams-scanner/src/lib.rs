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

/// The host this scanner runs on, for chunk attribution (sprint 023
/// #407). Reads the kernel's live hostname from procfs — identical to
/// `gethostname(2)`, with no crate and no systemd-unit dependency
/// (systemd doesn't export `$HOSTNAME`; that's the monitor's #56
/// lesson). Falls back to `$HOSTNAME`, then `"unknown"`. A config
/// `host` key overrides this — the single host-source seam the future
/// central mount-scan mode (#406) extends.
#[must_use]
pub fn default_host() -> String {
    if let Ok(h) = std::fs::read_to_string("/proc/sys/kernel/hostname") {
        let h = h.trim();
        if !h.is_empty() {
            return h.to_owned();
        }
    }
    std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".into())
}

/// Walk a single root, diff against cursor, publish changes, prune
/// vanished files. Exposed at the library level so the integration
/// suite can drive it directly without spawning the binary. `host` is
/// stamped on every chunk and scopes deletes (sprint 023 #407/#408).
#[allow(clippy::too_many_arguments)]
pub async fn scan_root(
    client: &Client,
    base_url: &str,
    bearer: &str,
    host: &str,
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
            match publish_delete(base_url, bearer, host, &abs).await {
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
            if let Err(e) = publish_chunk(client, host, &repo, &abs, c).await {
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
        match publish_delete(base_url, bearer, host, &prev.absolute_path).await {
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

#[cfg(test)]
mod tests {
    use super::default_host;

    #[test]
    fn default_host_is_nonempty_and_trimmed() {
        // On Linux (dev + CI) procfs yields the real hostname; the only
        // contract we assert is a non-empty, whitespace-free value so a
        // chunk is never attributed to "" — never the systemd "unknown"
        // trap (#56) since procfs doesn't depend on $HOSTNAME.
        let h = default_host();
        assert!(!h.is_empty());
        assert_eq!(h, h.trim());
    }
}
