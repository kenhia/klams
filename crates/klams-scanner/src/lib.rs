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

/// Derive the repo name for a file under a scan root (sprint 028 #640).
///
/// The deepest ancestor directory (from the file's parent down to and
/// including the root) containing a `.git` entry wins — `.git` may be a
/// directory or, for worktrees/submodules, a file. With no `.git`
/// anywhere, the first path segment under the root stands in; a file
/// directly under the root (or outside it) falls back to the root's
/// basename — the pre-028 behavior, now the last resort instead of the
/// only answer.
#[must_use]
pub fn derive_repo(root: &Path, file: &Path) -> String {
    let root_name = || {
        root.file_name().map_or_else(
            || root.display().to_string(),
            |s| s.to_string_lossy().into_owned(),
        )
    };
    let Ok(rel) = file.strip_prefix(root) else {
        return root_name();
    };
    let mut dir = file.parent();
    while let Some(d) = dir {
        if d.join(".git").exists() {
            if d == root {
                return root_name();
            }
            if let Some(name) = d.file_name() {
                return name.to_string_lossy().into_owned();
            }
        }
        if d == root {
            break;
        }
        dir = d.parent();
    }
    match rel.components().next() {
        Some(first) if rel.components().count() > 1 => {
            first.as_os_str().to_string_lossy().into_owned()
        }
        _ => root_name(),
    }
}

/// Per-parent-directory memo over [`derive_repo`], which stats ancestor
/// directories — files share parents, so one lookup covers a directory.
#[derive(Default)]
struct RepoCache(std::collections::HashMap<std::path::PathBuf, String>);

impl RepoCache {
    fn get(&mut self, root: &Path, file: &Path) -> String {
        match file.parent() {
            Some(dir) => self
                .0
                .entry(dir.to_path_buf())
                .or_insert_with(|| derive_repo(root, file))
                .clone(),
            None => derive_repo(root, file),
        }
    }
}

/// Walk a single root, diff against cursor, publish changes, prune
/// vanished files. Exposed at the library level so the integration
/// suite can drive it directly without spawning the binary. `host` is
/// stamped on every chunk and scopes deletes (sprint 023 #407/#408).
///
/// `embed_limit` is the embedder ceiling every published chunk must fit
/// (sprint 027 #420) — an oversized chunk is split here rather than
/// accepted by the API with a 202 and then dropped at the worker, which
/// is how ~30k chunks were lost silently on kai.
#[allow(clippy::too_many_arguments)]
pub async fn scan_root(
    client: &Client,
    base_url: &str,
    bearer: &str,
    host: &str,
    cursor_path: &Path,
    root: &Path,
    embed_limit: klams_types::EmbedLimit,
) -> Result<()> {
    use chunk::{chunk, sha256_hex, Lang};
    use cursor::Cursor;
    use publish::{publish_chunk, publish_delete};
    use walk::walk;

    let cursor = Cursor::open(cursor_path)?;
    let files = walk(root);
    let mut seen: HashSet<String> = HashSet::new();
    // Sprint 028 (#640): repo is derived per file, not from the root's
    // basename (which stamped repo="src" on 218k of 222k live points).
    let mut repos = RepoCache::default();

    for f in files {
        let abs = f.absolute_path.display().to_string();
        seen.insert(abs.clone());
        let repo = repos.get(root, &f.absolute_path);

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
        let chunks = chunk(&body, Lang::from_path(&abs), embed_limit);
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
        let mut skipped_too_large = 0usize;
        for c in &chunks {
            match publish_chunk(client, host, &repo, &abs, c).await {
                Ok(publish::Published::Ok) => {}
                // Sprint 027 (#420): permanent for this chunk. Do NOT set
                // publish_failed — that would hold the cursor back and
                // re-offer the same doomed chunk on every scan forever.
                // The loss is counted and logged inside publish_chunk.
                Ok(publish::Published::TooLarge) => skipped_too_large += 1,
                Err(e) => {
                    tracing::warn!(path = %abs, idx = c.index, %e, "publish_chunk failed");
                    publish_failed = true;
                }
            }
        }
        if skipped_too_large > 0 {
            tracing::warn!(
                path = %abs,
                skipped = skipped_too_large,
                total = chunks.len(),
                "file indexed with chunks missing — they exceed the embedder's ceiling"
            );
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
    use super::{default_host, derive_repo};
    use std::path::Path;

    // -----------------------------------------------------------------
    // Sprint 028 (#640) — the recorded repo must be the actual repo, not
    // the scan root's basename (which made 218k of 222k live points say
    // repo="src").

    #[test]
    fn repo_is_nearest_git_ancestor() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("kpidash/.git")).unwrap();
        std::fs::create_dir_all(root.join("kpidash/src")).unwrap();
        let file = root.join("kpidash/src/main.rs");
        assert_eq!(derive_repo(root, &file), "kpidash");
    }

    #[test]
    fn nested_repo_beats_first_segment() {
        // /root/tools/kpidash/.git — the repo is kpidash, not tools.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("tools/kpidash/.git")).unwrap();
        std::fs::create_dir_all(root.join("tools/kpidash/docs")).unwrap();
        let file = root.join("tools/kpidash/docs/README.md");
        assert_eq!(derive_repo(root, &file), "kpidash");
    }

    #[test]
    fn gitfile_worktree_counts_as_repo_marker() {
        // Worktrees and submodules have a `.git` *file*, not a directory.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("wt/sub")).unwrap();
        std::fs::write(root.join("wt/.git"), "gitdir: elsewhere").unwrap();
        let file = root.join("wt/sub/lib.rs");
        assert_eq!(derive_repo(root, &file), "wt");
    }

    #[test]
    fn no_git_falls_back_to_first_segment() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        std::fs::create_dir_all(root.join("notes/topic")).unwrap();
        let file = root.join("notes/topic/idea.md");
        assert_eq!(derive_repo(root, &file), "notes");
    }

    #[test]
    fn file_directly_under_root_uses_root_basename() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("src");
        std::fs::create_dir_all(&root).unwrap();
        let file = root.join("README.md");
        assert_eq!(
            derive_repo(&root, &file),
            root.file_name().unwrap().to_string_lossy()
        );
    }

    #[test]
    fn root_that_is_itself_a_repo_uses_its_basename() {
        // Scanning a repo directly: /root/.git exists — every file
        // belongs to the root repo, not to its top-level directories.
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("klams");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::create_dir_all(root.join("crates/klams-core")).unwrap();
        let file = root.join("crates/klams-core/src/lib.rs");
        assert_eq!(derive_repo(&root, &file), "klams");
    }

    #[test]
    fn file_outside_root_falls_back_to_root_basename() {
        // Defensive: never panic on a path that doesn't strip.
        assert_eq!(
            derive_repo(Path::new("/a/src"), Path::new("/elsewhere/x.md")),
            "src"
        );
    }

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
