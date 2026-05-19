//! klams-scanner library surface.

pub mod chunk;
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
    use chunk::{chunk, sha256_hex};
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

        if let Some(prev) = cursor.get(&abs)? {
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
        let chunks = chunk(&body);
        let file_hash = sha256_hex(&body);

        if let Some(prev) = cursor.get(&abs)? {
            if prev.content_hash == file_hash {
                cursor.upsert(&abs, &file_hash, f.mtime_ns, now_seconds_i64())?;
                metrics::incr_skipped("hash_unchanged");
                continue;
            }
        }

        for c in &chunks {
            if let Err(e) = publish_chunk(client, &repo, &abs, &c.text).await {
                tracing::warn!(path = %abs, idx = c.index, %e, "publish_chunk failed");
            }
        }
        metrics::add_chunks(chunks.len() as u64);
        cursor.upsert(&abs, &file_hash, f.mtime_ns, now_seconds_i64())?;
        metrics::incr_processed();
    }

    // Prune vanished files.
    for prev in cursor.list_all()? {
        if seen.contains(&prev.absolute_path) {
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
