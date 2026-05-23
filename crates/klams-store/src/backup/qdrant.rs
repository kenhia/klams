//! Qdrant snapshot REST + restore (sprint 006 T022/T032).
//!
//! Uses Qdrant's HTTP REST API (default port 6333) to create a
//! point-in-time snapshot of `collection`, then streams the resulting
//! file to `<backup_dir>/qdrant-<date>[-N].snapshot.partial` and
//! atomic-renames on success. The in-qdrant copy is dropped after
//! a successful download when `drop_remote = true`.

use std::path::{Path, PathBuf};
use std::time::Instant;

use serde::Deserialize;
use tokio::io::AsyncWriteExt;

use super::{ArtifactKind, BackupArtifact, BackupError};

#[derive(Debug, Deserialize)]
struct SnapshotResult {
    name: String,
}

#[derive(Debug, Deserialize)]
struct SnapshotEnvelope {
    result: SnapshotResult,
}

/// Take a snapshot of `collection` via Qdrant REST and stream it to
/// `<backup_dir>/qdrant-<date>[-N].snapshot`. If `drop_remote` is true,
/// the in-qdrant snapshot is deleted after a successful download.
///
/// `rest_url` is the base URL of Qdrant's HTTP API (e.g.
/// `http://127.0.0.1:6333`), without a trailing slash.
///
/// # Errors
///
/// Returns [`BackupError::QdrantHttp`] or [`BackupError::QdrantSnapshot`]
/// for snapshot-API failures, and [`BackupError::Io`] for filesystem errors.
pub async fn snapshot(
    backup_dir: &Path,
    rest_url: &str,
    collection: &str,
    date_str: &str,
    suffix: Option<u32>,
    drop_remote: bool,
) -> Result<BackupArtifact, BackupError> {
    let started = Instant::now();
    let (final_path, partial_path) = artifact_paths(backup_dir, date_str, suffix);

    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(60 * 60))
        .build()
        .map_err(|e| BackupError::QdrantHttp(format!("client build: {e}")))?;

    let create_url = format!("{rest_url}/collections/{collection}/snapshots");
    let resp = client
        .post(&create_url)
        .send()
        .await
        .map_err(|e| BackupError::QdrantHttp(format!("create POST: {e}")))?;
    if !resp.status().is_success() {
        let status = resp.status();
        let body = resp.text().await.unwrap_or_default();
        return Err(BackupError::QdrantSnapshot(format!(
            "create returned {status}: {body}"
        )));
    }
    let envelope: SnapshotEnvelope = resp
        .json()
        .await
        .map_err(|e| BackupError::QdrantHttp(format!("create body parse: {e}")))?;
    let snapshot_name = envelope.result.name;

    let download_url = format!("{rest_url}/collections/{collection}/snapshots/{snapshot_name}");
    let mut dl = client
        .get(&download_url)
        .send()
        .await
        .map_err(|e| BackupError::QdrantHttp(format!("download GET: {e}")))?;
    if !dl.status().is_success() {
        let status = dl.status();
        let body = dl.text().await.unwrap_or_default();
        return Err(BackupError::QdrantSnapshot(format!(
            "download returned {status}: {body}"
        )));
    }

    let mut out = tokio::fs::File::create(&partial_path).await?;
    while let Some(chunk) = dl
        .chunk()
        .await
        .map_err(|e| BackupError::QdrantHttp(format!("download chunk: {e}")))?
    {
        out.write_all(&chunk).await?;
    }
    out.flush().await?;
    drop(out);

    tokio::fs::rename(&partial_path, &final_path).await?;
    let bytes = tokio::fs::metadata(&final_path).await?.len();

    if drop_remote {
        let del = client.delete(&download_url).send().await;
        if let Err(e) = del {
            tracing::warn!(error = %e, %snapshot_name, "qdrant snapshot delete failed (download succeeded; leaving remote copy)");
        }
    }

    Ok(BackupArtifact {
        kind: ArtifactKind::Qdrant,
        path: final_path,
        bytes,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        ok: true,
        error: None,
    })
}

/// Upload a previously-captured Qdrant snapshot file to a collection.
///
/// Sprint 006 Phase 4 (T032) lands the real implementation. The MVP
/// (Phase 3) returns an error so the call-site type-checks while
/// `klams-service::backup::restore` is still a TODO.
///
/// # Errors
///
/// Always returns [`BackupError::QdrantSnapshot`] in the MVP.
#[allow(clippy::unused_async)]
pub async fn restore(
    _rest_url: &str,
    _collection: &str,
    _snapshot_path: &Path,
) -> Result<(), BackupError> {
    Err(BackupError::QdrantSnapshot(
        "qdrant restore not implemented yet (sprint 006 Phase 4 / T032)".into(),
    ))
}

fn artifact_paths(backup_dir: &Path, date_str: &str, suffix: Option<u32>) -> (PathBuf, PathBuf) {
    let stem = match suffix {
        Some(n) => format!("qdrant-{date_str}-{n}"),
        None => format!("qdrant-{date_str}"),
    };
    let final_path = backup_dir.join(format!("{stem}.snapshot"));
    let partial_path = backup_dir.join(format!("{stem}.snapshot.partial"));
    (final_path, partial_path)
}

#[cfg(test)]
mod tests {
    use super::artifact_paths;
    use std::path::Path;

    #[test]
    fn artifact_paths_no_suffix() {
        let (f, p) = artifact_paths(Path::new("/b"), "2026-05-23", None);
        assert_eq!(f, Path::new("/b/qdrant-2026-05-23.snapshot"));
        assert_eq!(p, Path::new("/b/qdrant-2026-05-23.snapshot.partial"));
    }
}
