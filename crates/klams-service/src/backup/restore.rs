//! `restore::run_from(date, force)` driver (sprint 006 T033).
//!
//! Resolves `<backup_dir>/{postgres,qdrant}-<date>.{dump,snapshot}`
//! (picking the highest-`-N` suffix when multiple exist for the
//! date), refuses to clobber a non-empty target unless `force=true`,
//! then delegates to [`klams_store::backup::postgres::restore`] and
//! [`klams_store::backup::qdrant::restore`]. Designed to mirror the
//! operator runbook so the integration test IS the once-per-sprint
//! restore exercise (FR-016).

use std::path::{Path, PathBuf};

use klams_store::backup as store_backup;
use klams_store::backup::{ArtifactKind, BackupError};

use super::OrchestratorDeps;

/// Errors raised by [`run_from`].
#[derive(Debug, thiserror::Error)]
pub enum RestoreError {
    #[error("no {kind:?} artifact found for date {date} in {dir}")]
    ArtifactMissing {
        kind: ArtifactKind,
        date: String,
        dir: String,
    },
    #[error("target {target} is non-empty (count={count}); pass --force to overwrite")]
    NonEmptyTarget { target: &'static str, count: u64 },
    #[error("postgres probe: {0}")]
    PgProbe(String),
    #[error("qdrant probe: {0}")]
    QdrantProbe(String),
    #[error("backup: {0}")]
    Backup(#[from] BackupError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
}

/// Per-step progress event emitted by [`run_from`] so the CLI recipe
/// can print operator-friendly lines.
#[derive(Debug, Clone)]
pub enum RestoreProgress {
    Resolved { pg_path: PathBuf, q_path: PathBuf },
    PgRestoreStarted,
    PgRestoreDone,
    QdrantRestoreStarted,
    QdrantRestoreDone,
}

/// Restore from the snapshot pair stamped `date` into the targets
/// referenced by `deps`. `force=false` refuses to overwrite a target
/// that already contains data.
///
/// # Errors
///
/// See [`RestoreError`].
pub async fn run_from<F>(
    deps: &OrchestratorDeps,
    date: &str,
    force: bool,
    mut on_progress: F,
) -> Result<(), RestoreError>
where
    F: FnMut(RestoreProgress),
{
    let pg_path = newest_artifact(&deps.backup_dir, ArtifactKind::Postgres, date)?;
    let q_path = newest_artifact(&deps.backup_dir, ArtifactKind::Qdrant, date)?;
    on_progress(RestoreProgress::Resolved {
        pg_path: pg_path.clone(),
        q_path: q_path.clone(),
    });

    if !force {
        let pg_rows = probe_postgres_non_empty(&deps.pg_url).await?;
        if pg_rows > 0 {
            return Err(RestoreError::NonEmptyTarget {
                target: "postgres",
                count: pg_rows,
            });
        }
        let q_points =
            probe_qdrant_non_empty(&deps.qdrant_rest_url, &deps.qdrant_collection).await?;
        if q_points > 0 {
            return Err(RestoreError::NonEmptyTarget {
                target: "qdrant",
                count: q_points,
            });
        }
    }

    on_progress(RestoreProgress::PgRestoreStarted);
    store_backup::postgres::restore(&deps.pg_url, &pg_path, deps.pg_bin_dir.as_deref()).await?;
    on_progress(RestoreProgress::PgRestoreDone);

    on_progress(RestoreProgress::QdrantRestoreStarted);
    store_backup::qdrant::restore(&deps.qdrant_rest_url, &deps.qdrant_collection, &q_path).await?;
    on_progress(RestoreProgress::QdrantRestoreDone);

    Ok(())
}

/// Resolve the artifact path for `(kind, date)`, preferring the
/// highest numeric suffix when multiple files share the date.
fn newest_artifact(
    backup_dir: &Path,
    kind: ArtifactKind,
    date: &str,
) -> Result<PathBuf, RestoreError> {
    let prefix = format!("{}-{date}", kind.prefix());
    let ext = kind.extension();
    let mut candidates: Vec<(Option<u32>, PathBuf)> = Vec::new();
    let entries = std::fs::read_dir(backup_dir).map_err(RestoreError::Io)?;
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(&prefix) else {
            continue;
        };
        let Some(rest) = rest.strip_suffix(&format!(".{ext}")) else {
            continue;
        };
        if rest.is_empty() {
            candidates.push((None, entry.path()));
        } else if let Some(num) = rest.strip_prefix('-').and_then(|s| s.parse::<u32>().ok()) {
            candidates.push((Some(num), entry.path()));
        }
    }
    candidates.sort_by_key(|(n, _)| *n);
    candidates
        .pop()
        .map(|(_, p)| p)
        .ok_or_else(|| RestoreError::ArtifactMissing {
            kind,
            date: date.to_string(),
            dir: backup_dir.display().to_string(),
        })
}

async fn probe_postgres_non_empty(pg_url: &str) -> Result<u64, RestoreError> {
    let pool = sqlx::PgPool::connect(pg_url)
        .await
        .map_err(|e| RestoreError::PgProbe(e.to_string()))?;
    let row: (i64,) =
        sqlx::query_as("SELECT (SELECT COUNT(*) FROM facts) + (SELECT COUNT(*) FROM events)")
            .fetch_one(&pool)
            .await
            .map_err(|e| RestoreError::PgProbe(e.to_string()))?;
    pool.close().await;
    Ok(u64::try_from(row.0).unwrap_or(0))
}

async fn probe_qdrant_non_empty(rest_url: &str, collection: &str) -> Result<u64, RestoreError> {
    #[derive(serde::Deserialize)]
    struct CountResult {
        points_count: Option<u64>,
    }
    #[derive(serde::Deserialize)]
    struct CountEnvelope {
        result: CountResult,
    }
    let client = reqwest::Client::new();
    let url = format!("{rest_url}/collections/{collection}");
    let resp = client
        .get(&url)
        .send()
        .await
        .map_err(|e| RestoreError::QdrantProbe(e.to_string()))?;
    if !resp.status().is_success() {
        // Treat missing collection as empty.
        return Ok(0);
    }
    let env: CountEnvelope = resp
        .json()
        .await
        .map_err(|e| RestoreError::QdrantProbe(e.to_string()))?;
    Ok(env.result.points_count.unwrap_or(0))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use tempfile::tempdir;

    #[test]
    fn newest_artifact_prefers_highest_suffix() {
        let d = tempdir().unwrap();
        for name in [
            "postgres-2026-05-23.dump",
            "postgres-2026-05-23-1.dump",
            "postgres-2026-05-23-2.dump",
            "postgres-2026-05-22.dump",
        ] {
            fs::write(d.path().join(name), b"x").unwrap();
        }
        let p = newest_artifact(d.path(), ArtifactKind::Postgres, "2026-05-23").unwrap();
        assert_eq!(p.file_name().unwrap(), "postgres-2026-05-23-2.dump");
    }

    #[test]
    fn newest_artifact_falls_back_to_unsuffixed() {
        let d = tempdir().unwrap();
        fs::write(d.path().join("postgres-2026-05-23.dump"), b"x").unwrap();
        let p = newest_artifact(d.path(), ArtifactKind::Postgres, "2026-05-23").unwrap();
        assert_eq!(p.file_name().unwrap(), "postgres-2026-05-23.dump");
    }

    #[test]
    fn newest_artifact_missing_returns_err() {
        let d = tempdir().unwrap();
        let err = newest_artifact(d.path(), ArtifactKind::Qdrant, "2026-05-23").unwrap_err();
        assert!(matches!(err, RestoreError::ArtifactMissing { .. }));
    }
}
