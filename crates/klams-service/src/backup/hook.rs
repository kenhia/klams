//! `status_hook` lifecycle executor (sprint 006 T045/T046).
//!
//! Builds JSON payloads matching
//! `sprints/006-maintenance-and-backups/contracts/backup-status-hook.schema.json`
//! and pipes them to an arbitrary executable. Hook failures are
//! observability (counters + tracing), never control flow.

use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

use chrono::{DateTime, Utc};
use klams_store::backup::BackupArtifact;
use serde::Serialize;
use tokio::io::AsyncWriteExt;
use tokio::process::Command;

use super::lifecycle::BackupRun;
use super::metrics;

/// Currently pinned to `1`; bumped only on a breaking payload change.
pub const SCHEMA_VERSION: u32 = 1;

/// Lifecycle point the orchestrator is signalling.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HookEventKind {
    Started,
    Finished,
    Failed,
}

impl HookEventKind {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Started => "started",
            Self::Finished => "finished",
            Self::Failed => "failed",
        }
    }
}

/// Wire-format payload piped on stdin to the configured executable.
/// Field shape matches `backup-status-hook.schema.json` (sprint 006).
#[derive(Debug, Clone, Serialize)]
pub struct BackupHookEvent {
    pub schema_version: u32,
    pub run_id: String,
    pub event: HookEventKind,
    pub started_at: DateTime<Utc>,
    pub ended_at: Option<DateTime<Utc>>,
    pub duration_ms: Option<u64>,
    pub artifacts: Vec<HookArtifact>,
    pub ok: bool,
    pub error: Option<String>,
}

/// Per-artifact entry inside `BackupHookEvent::artifacts`.
#[derive(Debug, Clone, Serialize)]
pub struct HookArtifact {
    pub kind: String,
    pub path: PathBuf,
    pub bytes: u64,
    pub duration_ms: u64,
    pub ok: bool,
    pub error: Option<String>,
}

impl From<&BackupArtifact> for HookArtifact {
    fn from(a: &BackupArtifact) -> Self {
        Self {
            kind: a.kind.prefix().to_string(),
            path: a.path.clone(),
            bytes: a.bytes,
            duration_ms: a.duration_ms,
            ok: a.ok,
            error: a.error.clone(),
        }
    }
}

impl BackupHookEvent {
    /// Build a `started` event from an in-flight `BackupRun`. The
    /// schema mandates empty artifacts, null ended/duration, ok=false.
    #[must_use]
    pub fn started(run: &BackupRun) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            run_id: run.run_id.to_string(),
            event: HookEventKind::Started,
            started_at: run.started_at,
            ended_at: None,
            duration_ms: None,
            artifacts: Vec::new(),
            ok: false,
            error: None,
        }
    }

    /// Build a `finished` event. Caller is responsible for only
    /// invoking this when every artifact in `run` has `ok == true`.
    #[must_use]
    pub fn finished(run: &BackupRun) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            run_id: run.run_id.to_string(),
            event: HookEventKind::Finished,
            started_at: run.started_at,
            ended_at: run.ended_at,
            duration_ms: run.duration_ms(),
            artifacts: run.artifacts.iter().map(HookArtifact::from).collect(),
            ok: true,
            error: None,
        }
    }

    /// Build a `failed` event. `error` is required by the schema; if
    /// `run.error` is empty we synthesise a stable fallback.
    #[must_use]
    pub fn failed(run: &BackupRun) -> Self {
        let error = run
            .error
            .clone()
            .unwrap_or_else(|| "unknown failure".to_string());
        Self {
            schema_version: SCHEMA_VERSION,
            run_id: run.run_id.to_string(),
            event: HookEventKind::Failed,
            started_at: run.started_at,
            ended_at: run.ended_at,
            duration_ms: run.duration_ms(),
            artifacts: run.artifacts.iter().map(HookArtifact::from).collect(),
            ok: false,
            error: Some(error),
        }
    }
}

/// Outcome of one hook invocation. Always returned to the caller as
/// observability; never propagated as an error.
#[derive(Debug, Clone)]
pub struct InvokeResult {
    pub ok: bool,
    pub exit_code: Option<i32>,
    pub timed_out: bool,
    pub stdout_tail: String,
    pub stderr_tail: String,
    pub error: Option<String>,
}

/// Invoke the configured `status_hook` with `event` piped on stdin.
///
/// * `hook_path = None` → no-op (returns `ok=true`, no metric bump).
/// * Times out at `timeout`; SIGTERM, then SIGKILL after a 2s grace
///   per spec.md edge case "`status_hook` stalls forever".
/// * Captures up to `TAIL_BYTES` from stdout/stderr for the trace.
/// * Bumps `klams_backup_hook_invocations_total{event, ok}`.
pub async fn invoke(
    hook_path: Option<&Path>,
    timeout: Duration,
    event: &BackupHookEvent,
) -> InvokeResult {
    let Some(path) = hook_path else {
        return InvokeResult {
            ok: true,
            exit_code: None,
            timed_out: false,
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            error: None,
        };
    };

    let payload = match serde_json::to_vec(event) {
        Ok(b) => b,
        Err(e) => {
            let msg = format!("serialize hook payload: {e}");
            tracing::error!(error = %msg, "status_hook serialize failed");
            metrics::incr_hook_invocations(event.event.as_str(), false);
            return InvokeResult {
                ok: false,
                exit_code: None,
                timed_out: false,
                stdout_tail: String::new(),
                stderr_tail: String::new(),
                error: Some(msg),
            };
        }
    };

    let result = spawn_and_wait(path, timeout, &payload, event).await;

    let event_label = event.event.as_str();
    metrics::incr_hook_invocations(event_label, result.ok);
    tracing::info!(
        target: "klams::backup::hook",
        path = %path.display(),
        run_id = %event.run_id,
        event = event_label,
        ok = result.ok,
        timed_out = result.timed_out,
        exit_code = ?result.exit_code,
        stdout_tail = %result.stdout_tail,
        stderr_tail = %result.stderr_tail,
        error = result.error.as_deref().unwrap_or(""),
        "status_hook invoked",
    );
    result
}

const TAIL_BYTES: usize = 4 * 1024;
const SIGTERM_GRACE: Duration = Duration::from_secs(2);

async fn spawn_and_wait(
    path: &Path,
    timeout: Duration,
    payload: &[u8],
    event: &BackupHookEvent,
) -> InvokeResult {
    let mut cmd = Command::new(path);
    cmd.env("KLAMS_BACKUP_RUN_ID", &event.run_id)
        .env("KLAMS_BACKUP_EVENT", event.event.as_str())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true);

    let mut child = match cmd.spawn() {
        Ok(c) => c,
        Err(e) => {
            return InvokeResult {
                ok: false,
                exit_code: None,
                timed_out: false,
                stdout_tail: String::new(),
                stderr_tail: String::new(),
                error: Some(format!("spawn: {e}")),
            };
        }
    };

    if let Some(mut stdin) = child.stdin.take() {
        if let Err(e) = stdin.write_all(payload).await {
            tracing::warn!(error = %e, "status_hook stdin write failed");
        }
        // Drop closes stdin so the hook sees EOF.
    }

    let pid = child.id();
    let mut stdout = child.stdout.take();
    let mut stderr = child.stderr.take();

    match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            let stdout_bytes = drain(stdout.as_mut()).await;
            let stderr_bytes = drain(stderr.as_mut()).await;
            let ok = status.success();
            InvokeResult {
                ok,
                exit_code: status.code(),
                timed_out: false,
                stdout_tail: tail_lossy(&stdout_bytes),
                stderr_tail: tail_lossy(&stderr_bytes),
                error: (!ok).then(|| format!("exit status {status}")),
            }
        }
        Ok(Err(e)) => InvokeResult {
            ok: false,
            exit_code: None,
            timed_out: false,
            stdout_tail: String::new(),
            stderr_tail: String::new(),
            error: Some(format!("wait: {e}")),
        },
        Err(_) => {
            // Timed out: SIGTERM, grace, SIGKILL.
            #[cfg(unix)]
            if let Some(pid) = pid {
                use nix::sys::signal::{kill, Signal};
                use nix::unistd::Pid;
                #[allow(clippy::cast_possible_wrap)]
                let nix_pid = Pid::from_raw(pid as i32);
                let _ = kill(nix_pid, Signal::SIGTERM);
                if tokio::time::timeout(SIGTERM_GRACE, child.wait())
                    .await
                    .is_err()
                {
                    let _ = child.start_kill();
                    let _ = child.wait().await;
                }
            }
            #[cfg(not(unix))]
            {
                let _ = child.start_kill();
                let _ = child.wait().await;
            }
            InvokeResult {
                ok: false,
                exit_code: None,
                timed_out: true,
                stdout_tail: String::new(),
                stderr_tail: String::new(),
                error: Some(format!("timed out after {timeout:?}")),
            }
        }
    }
}

async fn drain<R: tokio::io::AsyncRead + Unpin>(reader: Option<&mut R>) -> Vec<u8> {
    use tokio::io::AsyncReadExt;
    let mut buf = Vec::new();
    if let Some(r) = reader {
        let _ = r.read_to_end(&mut buf).await;
    }
    buf
}

fn tail_lossy(bytes: &[u8]) -> String {
    let start = bytes.len().saturating_sub(TAIL_BYTES);
    String::from_utf8_lossy(&bytes[start..]).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use klams_store::backup::ArtifactKind;
    use std::path::PathBuf;

    fn make_run() -> BackupRun {
        let mut run = BackupRun::start();
        run.artifact_done(BackupArtifact {
            kind: ArtifactKind::Postgres,
            path: PathBuf::from("/tmp/postgres-2026-05-23.dump"),
            bytes: 42,
            duration_ms: 10,
            ok: true,
            error: None,
        });
        run
    }

    #[test]
    fn started_payload_obeys_schema_constraints() {
        let run = make_run();
        let ev = BackupHookEvent::started(&run);
        assert_eq!(ev.schema_version, 1);
        assert_eq!(ev.event, HookEventKind::Started);
        assert!(ev.ended_at.is_none());
        assert!(ev.duration_ms.is_none());
        assert!(ev.artifacts.is_empty());
        assert!(!ev.ok);
        assert!(ev.error.is_none());
    }

    #[test]
    fn finished_payload_marks_ok_and_carries_artifacts() {
        let mut run = make_run();
        run.finish_ok();
        let ev = BackupHookEvent::finished(&run);
        assert_eq!(ev.event, HookEventKind::Finished);
        assert!(ev.ok);
        assert!(ev.error.is_none());
        assert!(ev.ended_at.is_some());
        assert!(ev.duration_ms.is_some());
        assert_eq!(ev.artifacts.len(), 1);
    }

    #[test]
    fn failed_payload_requires_error_string() {
        let mut run = make_run();
        run.finish_err("qdrant boom");
        let ev = BackupHookEvent::failed(&run);
        assert_eq!(ev.event, HookEventKind::Failed);
        assert!(!ev.ok);
        assert_eq!(ev.error.as_deref(), Some("qdrant boom"));
    }

    #[test]
    fn failed_payload_synthesises_error_when_missing() {
        let run = make_run();
        let ev = BackupHookEvent::failed(&run);
        assert!(ev.error.is_some());
    }

    #[tokio::test]
    async fn invoke_with_no_hook_is_noop() {
        let run = make_run();
        let ev = BackupHookEvent::started(&run);
        let r = invoke(None, Duration::from_secs(1), &ev).await;
        assert!(r.ok);
        assert!(!r.timed_out);
    }
}
