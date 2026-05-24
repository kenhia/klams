//! `pg_dump` / `pg_restore` shell-out (sprint 006 T021/T031).
//!
//! Writes to `<backup_dir>/postgres-<UTC-date>[-N].dump.partial`
//! and atomic-renames to `.dump` on a clean exit. On any error the
//! `.partial` file is left in place for triage; the retention pruner
//! intentionally ignores `.partial` files (they're not commits).

use std::path::{Path, PathBuf};
use std::time::Instant;

use tokio::process::Command;

use super::{ArtifactKind, BackupArtifact, BackupError};

/// Resolve `pg_dump` / `pg_restore` to an absolute path when the
/// operator pins `KLAMS_PG_BIN_DIR` (e.g. `/usr/lib/postgresql/16/bin`),
/// otherwise fall back to `$PATH`. Required because a dump produced
/// by a newer `pg_dump` may emit `SET` statements that an older
/// server cannot restore (FR-002 / FR-013). `KLAMS_PG_BIN_DIR`
/// overrides any caller-supplied directory; this lets operators
/// override without redeploying config.
fn pg_binary(name: &str, override_dir: Option<&Path>) -> PathBuf {
    if let Some(dir) = std::env::var_os("KLAMS_PG_BIN_DIR") {
        return PathBuf::from(dir).join(name);
    }
    override_dir.map_or_else(|| PathBuf::from(name), |d| d.join(name))
}

/// Run `pg_dump -Fc` against `pg_url`, writing the artifact under
/// `backup_dir`. The filename is `postgres-<date>[-N].dump` where
/// `N` is a numeric suffix used when a file for `date` already
/// exists and `same_day_strategy = Suffix`.
///
/// # Errors
///
/// Returns [`BackupError::Io`] for filesystem errors and
/// [`BackupError::PgDumpFailed`] if `pg_dump` exits non-zero.
pub async fn dump(
    backup_dir: &Path,
    pg_url: &str,
    date_str: &str,
    suffix: Option<u32>,
    pg_bin_dir: Option<&Path>,
) -> Result<BackupArtifact, BackupError> {
    let started = Instant::now();
    let (final_path, partial_path) = artifact_paths(backup_dir, date_str, suffix);

    if let Some(parent) = final_path.parent() {
        tokio::fs::create_dir_all(parent).await?;
    }

    let output = Command::new(pg_binary("pg_dump", pg_bin_dir))
        .arg("-Fc")
        .arg("--no-owner")
        .arg("--no-privileges")
        .arg("-f")
        .arg(&partial_path)
        .arg(pg_url)
        .output()
        .await?;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(BackupError::PgDumpFailed {
            status: output.status.code().unwrap_or(-1),
            stderr,
        });
    }

    tokio::fs::rename(&partial_path, &final_path).await?;
    let bytes = tokio::fs::metadata(&final_path).await?.len();

    Ok(BackupArtifact {
        kind: ArtifactKind::Postgres,
        path: final_path,
        bytes,
        duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        ok: true,
        error: None,
    })
}

/// Restore from a previously-captured `pg_dump -Fc` artifact into
/// `pg_url`. Uses `pg_restore --single-transaction --clean --if-exists --no-owner`.
///
/// The `--single-transaction` wrapper makes the whole restore
/// atomic: any error rolls back both the `--clean` `DROP`s and the
/// partial data load, leaving the target in its pre-call state.
///
/// # Errors
///
/// Returns [`BackupError::PgRestoreFailed`] on a non-zero exit.
pub async fn restore(
    pg_url: &str,
    dump_path: &Path,
    pg_bin_dir: Option<&Path>,
) -> Result<(), BackupError> {
    let output = Command::new(pg_binary("pg_restore", pg_bin_dir))
        .arg("--single-transaction")
        .arg("--clean")
        .arg("--if-exists")
        .arg("--no-owner")
        .arg("--no-privileges")
        .arg("--dbname")
        .arg(pg_url)
        .arg(dump_path)
        .output()
        .await?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        return Err(BackupError::PgRestoreFailed {
            status: output.status.code().unwrap_or(-1),
            stderr,
        });
    }
    Ok(())
}

fn artifact_paths(backup_dir: &Path, date_str: &str, suffix: Option<u32>) -> (PathBuf, PathBuf) {
    let stem = match suffix {
        Some(n) => format!("postgres-{date_str}-{n}"),
        None => format!("postgres-{date_str}"),
    };
    let final_path = backup_dir.join(format!("{stem}.dump"));
    let partial_path = backup_dir.join(format!("{stem}.dump.partial"));
    (final_path, partial_path)
}

#[cfg(test)]
mod tests {
    use super::artifact_paths;
    use std::path::Path;

    #[test]
    fn artifact_paths_no_suffix() {
        let dir = Path::new("/tmp/backups");
        let (f, p) = artifact_paths(dir, "2026-05-23", None);
        assert_eq!(f, Path::new("/tmp/backups/postgres-2026-05-23.dump"));
        assert_eq!(
            p,
            Path::new("/tmp/backups/postgres-2026-05-23.dump.partial")
        );
    }

    #[test]
    fn artifact_paths_with_suffix() {
        let dir = Path::new("/tmp/backups");
        let (f, p) = artifact_paths(dir, "2026-05-23", Some(2));
        assert_eq!(f, Path::new("/tmp/backups/postgres-2026-05-23-2.dump"));
        assert_eq!(
            p,
            Path::new("/tmp/backups/postgres-2026-05-23-2.dump.partial")
        );
    }
}
