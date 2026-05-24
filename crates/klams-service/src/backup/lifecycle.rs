//! `BackupRun` state machine + lockfile + stale-lockfile recovery
//! (sprint 006 T024).

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use klams_store::backup::BackupArtifact;
use serde::{Deserialize, Serialize};
use ulid::Ulid;

/// One end-to-end backup run.
#[derive(Debug, Clone)]
pub struct BackupRun {
    pub run_id: Ulid,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    /// `None` while the run is in flight; `Some(true/false)` after finish.
    pub ok: Option<bool>,
    pub artifacts: Vec<BackupArtifact>,
    /// First failure encountered, if any.
    pub error: Option<String>,
}

impl BackupRun {
    #[must_use]
    pub fn start() -> Self {
        Self {
            run_id: Ulid::new(),
            started_at: Utc::now(),
            ended_at: None,
            ok: None,
            artifacts: Vec::new(),
            error: None,
        }
    }

    pub fn artifact_done(&mut self, artifact: BackupArtifact) {
        if !artifact.ok && self.error.is_none() {
            self.error.clone_from(&artifact.error);
        }
        self.artifacts.push(artifact);
    }

    pub fn finish_ok(&mut self) {
        self.ended_at = Some(Utc::now());
        self.ok = Some(self.error.is_none());
    }

    pub fn finish_err(&mut self, msg: impl Into<String>) {
        if self.error.is_none() {
            self.error = Some(msg.into());
        }
        self.ended_at = Some(Utc::now());
        self.ok = Some(false);
    }

    #[must_use]
    pub fn duration_ms(&self) -> Option<u64> {
        self.ended_at.map(|e| {
            let d = e - self.started_at;
            u64::try_from(d.num_milliseconds().max(0)).unwrap_or(0)
        })
    }
}

/// On-disk lockfile contents: `{pid, run_id, started_at}`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockfileContents {
    pub pid: u32,
    pub run_id: String,
    pub started_at: DateTime<Utc>,
}

/// Filename of the lockfile inside `backup_dir`.
pub const LOCKFILE_NAME: &str = "lockfile";

/// Write a fresh lockfile recording this process's PID and the
/// active run. Fails if the lockfile already exists.
///
/// # Errors
///
/// Returns [`LockfileError::AlreadyLocked`] if a lockfile is
/// already present and [`LockfileError::Io`] for filesystem failures.
pub async fn acquire_lock(
    backup_dir: &Path,
    run_id: Ulid,
    started_at: DateTime<Utc>,
) -> Result<PathBuf, LockfileError> {
    tokio::fs::create_dir_all(backup_dir).await?;
    let path = backup_dir.join(LOCKFILE_NAME);
    if tokio::fs::metadata(&path).await.is_ok() {
        return Err(LockfileError::AlreadyLocked(path));
    }
    let contents = LockfileContents {
        pid: std::process::id(),
        run_id: run_id.to_string(),
        started_at,
    };
    let bytes = serde_json::to_vec_pretty(&contents)
        .map_err(|e| LockfileError::Serialize(e.to_string()))?;
    tokio::fs::write(&path, bytes).await?;
    Ok(path)
}

/// Remove the lockfile if it exists. Quietly succeeds if missing.
///
/// # Errors
///
/// Returns [`LockfileError::Io`] for filesystem failures other than
/// `NotFound`.
pub async fn release_lock(backup_dir: &Path) -> Result<(), LockfileError> {
    let path = backup_dir.join(LOCKFILE_NAME);
    match tokio::fs::remove_file(&path).await {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.into()),
    }
}

/// On startup, detect a stale lockfile (pid not alive), delete it
/// plus any `.partial` artifact files, and return the recovered
/// [`LockfileContents`] so the caller can fire a `failed` status hook.
///
/// Returns `Ok(None)` if no lockfile is present or the pid is still
/// alive (live in-flight run from another process — refuse to recover).
///
/// # Errors
///
/// Returns [`LockfileError::Io`] for filesystem errors and
/// [`LockfileError::Parse`] for malformed lockfile contents.
pub async fn recover_stale_lock(
    backup_dir: &Path,
) -> Result<Option<LockfileContents>, LockfileError> {
    let path = backup_dir.join(LOCKFILE_NAME);
    let raw = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    let contents: LockfileContents =
        serde_json::from_slice(&raw).map_err(|e| LockfileError::Parse(e.to_string()))?;

    if pid_is_alive(contents.pid) {
        return Ok(None);
    }

    // Delete every .partial file under backup_dir (they were never
    // committed). Don't touch committed snapshots.
    if let Ok(mut rd) = tokio::fs::read_dir(backup_dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            let p = entry.path();
            if let Some(name) = p.file_name().and_then(|s| s.to_str()) {
                if name.ends_with(".partial") {
                    let _ = tokio::fs::remove_file(&p).await;
                }
            }
        }
    }

    tokio::fs::remove_file(&path).await?;
    Ok(Some(contents))
}

#[cfg(target_os = "linux")]
fn pid_is_alive(pid: u32) -> bool {
    std::path::Path::new(&format!("/proc/{pid}")).exists()
}

#[cfg(not(target_os = "linux"))]
fn pid_is_alive(_pid: u32) -> bool {
    // On non-Linux targets, conservatively treat any lockfile pid as
    // dead so recovery proceeds (homelab target is Linux; non-Linux
    // is dev-only).
    false
}

#[derive(Debug, thiserror::Error)]
pub enum LockfileError {
    #[error("backup already in progress (lockfile present at {0})")]
    AlreadyLocked(PathBuf),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("serialize lockfile: {0}")]
    Serialize(String),
    #[error("parse lockfile: {0}")]
    Parse(String),
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    #[tokio::test]
    async fn acquire_then_release() {
        let dir = tempdir().unwrap();
        let id = Ulid::new();
        let p = acquire_lock(dir.path(), id, Utc::now()).await.unwrap();
        assert!(p.exists());
        release_lock(dir.path()).await.unwrap();
        assert!(!p.exists());
    }

    #[tokio::test]
    async fn acquire_twice_fails() {
        let dir = tempdir().unwrap();
        let id = Ulid::new();
        acquire_lock(dir.path(), id, Utc::now()).await.unwrap();
        let err = acquire_lock(dir.path(), id, Utc::now()).await.unwrap_err();
        assert!(matches!(err, LockfileError::AlreadyLocked(_)));
    }

    #[tokio::test]
    async fn release_missing_is_ok() {
        let dir = tempdir().unwrap();
        release_lock(dir.path()).await.unwrap();
    }

    #[tokio::test]
    async fn recover_stale_with_dead_pid_clears_lock_and_partials() {
        let dir = tempdir().unwrap();
        // Pick a pid that's almost certainly dead.
        let dead_pid: u32 = 999_999;
        let contents = LockfileContents {
            pid: dead_pid,
            run_id: Ulid::new().to_string(),
            started_at: Utc::now(),
        };
        tokio::fs::write(
            dir.path().join(LOCKFILE_NAME),
            serde_json::to_vec(&contents).unwrap(),
        )
        .await
        .unwrap();
        tokio::fs::write(dir.path().join("postgres-2026-05-23.dump.partial"), b"x")
            .await
            .unwrap();
        tokio::fs::write(dir.path().join("qdrant-2026-05-23.snapshot.partial"), b"x")
            .await
            .unwrap();
        // A committed file must NOT be touched.
        tokio::fs::write(dir.path().join("postgres-2026-05-22.dump"), b"x")
            .await
            .unwrap();

        let recovered = recover_stale_lock(dir.path()).await.unwrap();
        assert!(recovered.is_some());
        assert!(!dir.path().join(LOCKFILE_NAME).exists());
        assert!(!dir.path().join("postgres-2026-05-23.dump.partial").exists());
        assert!(!dir
            .path()
            .join("qdrant-2026-05-23.snapshot.partial")
            .exists());
        assert!(dir.path().join("postgres-2026-05-22.dump").exists());
    }

    #[tokio::test]
    async fn recover_with_live_pid_refuses() {
        let dir = tempdir().unwrap();
        let contents = LockfileContents {
            pid: std::process::id(), // ourselves: definitely alive
            run_id: Ulid::new().to_string(),
            started_at: Utc::now(),
        };
        tokio::fs::write(
            dir.path().join(LOCKFILE_NAME),
            serde_json::to_vec(&contents).unwrap(),
        )
        .await
        .unwrap();
        let recovered = recover_stale_lock(dir.path()).await.unwrap();
        assert!(recovered.is_none());
        assert!(dir.path().join(LOCKFILE_NAME).exists());
    }

    #[tokio::test]
    async fn recover_no_lockfile_is_none() {
        let dir = tempdir().unwrap();
        assert!(recover_stale_lock(dir.path()).await.unwrap().is_none());
    }

    #[test]
    fn run_lifecycle_tracks_artifacts_and_error() {
        use klams_store::backup::ArtifactKind;
        let mut r = BackupRun::start();
        assert!(r.ok.is_none());
        r.artifact_done(BackupArtifact {
            kind: ArtifactKind::Postgres,
            path: PathBuf::from("/tmp/x"),
            bytes: 10,
            duration_ms: 5,
            ok: true,
            error: None,
        });
        r.artifact_done(BackupArtifact {
            kind: ArtifactKind::Qdrant,
            path: PathBuf::from("/tmp/y"),
            bytes: 0,
            duration_ms: 1,
            ok: false,
            error: Some("nope".into()),
        });
        r.finish_ok();
        assert_eq!(r.ok, Some(false));
        assert_eq!(r.error.as_deref(), Some("nope"));
        assert!(r.duration_ms().is_some());
    }
}
