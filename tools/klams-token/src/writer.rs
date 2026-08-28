//! The write pipeline: backup → write through the inode → re-validate
//! → restore on any failure.
//!
//! Two details here are load-bearing and easy to get wrong.
//!
//! **The write goes through the existing inode**, not through the
//! usual write-temp-and-rename. `/etc/klams/klams.toml` is `root:klams
//! 0640` — the service reads it as the `klams` group — and a rename
//! would replace it with a fresh file owned by whoever ran `sudo`,
//! silently locking the service out of its own config at the next
//! restart. Truncate-in-place keeps owner, group and mode by
//! construction. The cost is a window where the file is short; the
//! backup taken immediately before is what covers it.
//!
//! **The backup is a secret too.** It is a verbatim copy of a file full
//! of live bearer tokens, so it inherits the original's ownership and
//! mode rather than defaulting to whatever the operator's umask says.

use anyhow::{bail, Context, Result};
use std::fs;
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};

/// How many of *our own* backups to keep. Backups written under any
/// other naming convention are never counted and never deleted — the
/// live file already carries five in three conventions and this tool's
/// job is to stop adding to that, not to clean up after other people's
/// (possibly deliberate) copies.
pub const DEFAULT_RETAIN: usize = 10;

/// The one backup naming convention, settled here (sprint 045, #265):
/// `<config-name>.bak-YYYYMMDDTHHMMSSZ`. UTC, second resolution, sorts
/// lexicographically in chronological order, and is unambiguous next to
/// the ad-hoc `bak-016-pre704` style names already in the directory.
#[must_use]
pub fn backup_suffix(now: time::OffsetDateTime) -> String {
    let now = now.to_offset(time::UtcOffset::UTC);
    format!(
        "bak-{:04}{:02}{:02}T{:02}{:02}{:02}Z",
        now.year(),
        u8::from(now.month()),
        now.day(),
        now.hour(),
        now.minute(),
        now.second(),
    )
}

/// True if `name` is a backup *this tool* wrote for `config_name`:
/// `<config>.bak-YYYYMMDDTHHMMSSZ`, optionally with a `-N`
/// disambiguator for a second edit inside the same second.
#[must_use]
pub fn is_our_backup(config_name: &str, name: &str) -> bool {
    let Some(rest) = name
        .strip_prefix(config_name)
        .and_then(|r| r.strip_prefix(".bak-"))
    else {
        return false;
    };
    // Sprint 046 (#1384): durable backups are age-encrypted and carry a
    // `.age` suffix, with the plaintext fingerprint manifest beside
    // them. All three are ours, and prune must see them as one
    // generation or it would keep manifests for backups it deleted.
    let rest = rest
        .strip_suffix(crate::backup::MANIFEST_EXT)
        .unwrap_or(rest);
    let rest = rest.strip_suffix(crate::backup::AGE_EXT).unwrap_or(rest);
    // YYYYMMDDTHHMMSSZ, then an optional -N.
    let (stamp, seq) = match rest.split_once("Z-") {
        Some((s, n)) => (s, Some(n)),
        None => (rest.strip_suffix('Z').unwrap_or(""), None),
    };
    if let Some(n) = seq {
        if n.is_empty() || !n.chars().all(|c| c.is_ascii_digit()) {
            return false;
        }
    }
    stamp.len() == 15
        && stamp.as_bytes()[8] == b'T'
        && stamp[..8].chars().all(|c| c.is_ascii_digit())
        && stamp[9..].chars().all(|c| c.is_ascii_digit())
}

/// Copy `path` alongside itself, preserving ownership and mode.
///
/// An existing backup is **never** overwritten — a second edit inside
/// the same second must not destroy the first one's safety net. Since
/// back-to-back edits are the normal case (an `add` followed by a
/// `scopes`, or krot rotating several grants in a row), a collision
/// takes the next free `-N` rather than failing the edit. The suffix
/// sorts after the bare name, so chronological order survives.
///
/// # Errors
/// If the copy cannot be created, or 100 names in the same second are
/// all taken (which is not a collision any more, it is a loop).
pub fn back_up(path: &Path, now: time::OffsetDateTime) -> Result<PathBuf> {
    let dest = free_backup_path(path, now, "")?;
    fs::copy(path, &dest)
        .with_context(|| format!("writing backup {} (need sudo?)", dest.display()))?;

    let meta = fs::metadata(path)?;
    fs::set_permissions(&dest, fs::Permissions::from_mode(meta.mode() & 0o7777))?;
    // Best-effort: only root can hand a file to another owner, and a
    // non-root operator who could read the config in the first place
    // already owns a readable copy. Failing the whole edit over the
    // backup's group would be worse than the group being wrong.
    let _ = std::os::unix::fs::chown(&dest, Some(meta.uid()), Some(meta.gid()));
    Ok(dest)
}

/// The next free `<config>.bak-<stamp>[-N]<ext>` beside `path`.
///
/// An existing backup is **never** overwritten — a second edit inside
/// the same second must not destroy the first one's safety net. Since
/// back-to-back edits are the normal case (an `add` followed by a
/// `scopes`, or krot rotating several grants in a row), a collision
/// takes the next free `-N` rather than failing the edit. The suffix
/// sorts after the bare name, so chronological order survives.
///
/// # Errors
/// If 100 names in the same second are all taken, which is not a
/// collision any more, it is a loop.
pub fn free_backup_path(path: &Path, now: time::OffsetDateTime, ext: &str) -> Result<PathBuf> {
    let name = file_name(path)?;
    let base = format!("{name}.{}", backup_suffix(now));
    let mut dest = path.with_file_name(format!("{base}{ext}"));
    let mut seq = 0;
    while dest.exists() {
        seq += 1;
        if seq > 99 {
            bail!(
                "cannot name a backup: {} and its -1..-99 variants all exist",
                path.with_file_name(format!("{base}{ext}")).display()
            );
        }
        dest = path.with_file_name(format!("{base}-{seq}{ext}"));
    }
    Ok(dest)
}

/// Overwrite `path`'s contents **in place**, keeping the inode and
/// therefore its owner, group and mode.
///
/// # Errors
/// If the file cannot be opened for writing (the usual cause is the
/// missing `sudo`) or the write fails.
pub fn write_in_place(path: &Path, text: &str) -> Result<()> {
    let mut f = fs::OpenOptions::new()
        .write(true)
        .truncate(true)
        .open(path)
        .with_context(|| format!("opening {} for writing (need sudo?)", path.display()))?;
    f.write_all(text.as_bytes())
        .with_context(|| format!("writing {}", path.display()))?;
    f.sync_all()
        .with_context(|| format!("flushing {}", path.display()))?;
    Ok(())
}

/// Put `backup` back over `path`, in place. Used when the freshly
/// written config fails to validate.
///
/// # Errors
/// If the restore itself fails — which the caller must report loudly,
/// since the live config is then broken and only the operator can fix
/// it.
pub fn restore(path: &Path, backup: &Path) -> Result<()> {
    let original = fs::read_to_string(backup)
        .with_context(|| format!("reading backup {}", backup.display()))?;
    write_in_place(path, &original)
}

/// What a completed write did.
#[derive(Debug)]
pub struct Written {
    pub backup: PathBuf,
    /// Whether the durable backup is age-encrypted (#1384). False means
    /// a live-token plaintext copy just landed on disk, which the CLI
    /// says out loud rather than leaving the operator to assume.
    pub encrypted: bool,
    pub manifest: Option<PathBuf>,
    pub pruned: Vec<PathBuf>,
}

/// How the durable backup should be written.
///
/// `recipient: None` keeps the sprint-045 behaviour — a plaintext copy
/// — because refusing to edit the config when encryption is not
/// configured would turn a hardening feature into an outage. The caller
/// reports which one happened.
#[derive(Debug, Default)]
pub struct DurableBackup<'a> {
    pub recipient: Option<&'a str>,
    /// `{agent_name: sha256(token)[:12]}` for the config being replaced,
    /// written in the clear beside an encrypted backup so what it holds
    /// stays knowable to krot and to an audit without decrypting it.
    pub fingerprints: Vec<(String, String)>,
}

/// What one durable backup produced.
#[derive(Debug)]
pub struct DurableWritten {
    pub path: PathBuf,
    pub encrypted: bool,
    pub manifest: Option<PathBuf>,
}

impl DurableBackup<'_> {
    /// Write the durable backup of `original` beside `path`.
    ///
    /// # Errors
    /// If the backup cannot be named or written, or `age` fails.
    pub fn write(
        &self,
        path: &Path,
        original: &str,
        now: time::OffsetDateTime,
    ) -> Result<DurableWritten> {
        let Some(recipient) = self.recipient else {
            return Ok(DurableWritten {
                path: back_up(path, now)?,
                encrypted: false,
                manifest: None,
            });
        };
        let dest = free_backup_path(path, now, crate::backup::AGE_EXT)?;
        crate::backup::encrypt(recipient, original, &dest, path)?;
        let manifest =
            crate::backup::write_manifest(&dest, &crate::backup::manifest(&self.fingerprints)).ok();
        Ok(DurableWritten {
            path: dest,
            encrypted: true,
            manifest,
        })
    }
}

/// Back up, write in place, then re-read and hand the bytes that
/// actually landed to `validate`. If that fails, put the backup back.
///
/// The re-read is not paranoia theatre: an in-memory document that
/// serializes cleanly and a file on disk that parses are different
/// claims, and the second one is the one the service will make.
///
/// # Errors
/// If the backup or write fails; if `validate` rejects what landed (the
/// error then says the file was restored); or — the loud case — if the
/// restore itself also fails, leaving a live broken config that only
/// the operator can recover.
pub fn write_validated(
    path: &Path,
    new_text: &str,
    now: time::OffsetDateTime,
    retain: usize,
    durable: &DurableBackup<'_>,
    validate: impl Fn(&str) -> Result<()>,
) -> Result<Written> {
    // Sprint 046 (#1384): the rollback copy is held in MEMORY, not read
    // back off the durable backup. That split is what makes encrypting
    // the durable copy affordable — the same-run transactional undo no
    // longer depends on being able to read it, so a failed validate at
    // 2am still self-heals without Ken's passphrase.
    let rollback = fs::read_to_string(path)
        .with_context(|| format!("reading {} before editing it", path.display()))?;

    let backup = durable.write(path, &rollback, now)?;
    write_in_place(path, new_text)?;

    let landed = fs::read_to_string(path)
        .with_context(|| format!("re-reading {} after writing it", path.display()))?;
    if let Err(e) = validate(&landed) {
        match write_in_place(path, &rollback) {
            Ok(()) => bail!(
                "the written config failed validation and was rolled back \
                 (durable backup: {})\n  cause: {e:#}",
                backup.path.display()
            ),
            Err(restore_err) => bail!(
                "the written config failed validation AND the rollback failed — {} is live and \
                 broken, recover it from {}\n  validation: {e:#}\n  rollback: {restore_err:#}",
                path.display(),
                backup.path.display()
            ),
        }
    }

    // Pruning is housekeeping; a completed edit must not be reported as
    // failed because an old backup was stubborn.
    let pruned = prune_backups(path, retain).unwrap_or_default();
    Ok(Written {
        backup: backup.path,
        encrypted: backup.encrypted,
        manifest: backup.manifest,
        pruned,
    })
}

/// Delete all but the newest `retain` backups this tool wrote for
/// `path`. Returns the ones removed.
///
/// # Errors
/// If the directory cannot be listed. Individual deletions that fail
/// are skipped rather than fatal — pruning is housekeeping, and a
/// completed edit must not be reported as failed because an old backup
/// was stubborn.
pub fn prune_backups(path: &Path, retain: usize) -> Result<Vec<PathBuf>> {
    let name = file_name(path)?;
    let dir = path.parent().unwrap_or_else(|| Path::new("."));
    let mut ours: Vec<PathBuf> = fs::read_dir(dir)
        .with_context(|| format!("listing {}", dir.display()))?
        .filter_map(std::result::Result::ok)
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| is_our_backup(name, n))
        })
        .collect();
    // The convention sorts chronologically, which is the point of it.
    ours.sort();
    if ours.len() <= retain {
        return Ok(Vec::new());
    }
    let doomed: Vec<PathBuf> = ours[..ours.len() - retain].to_vec();
    Ok(doomed
        .into_iter()
        .filter(|p| fs::remove_file(p).is_ok())
        .collect())
}

fn file_name(path: &Path) -> Result<&str> {
    path.file_name()
        .and_then(|n| n.to_str())
        .with_context(|| format!("{} has no file name", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn backup_suffix_is_sortable_utc() {
        assert_eq!(
            backup_suffix(datetime!(2026-08-16 23:45:01 UTC)),
            "bak-20260816T234501Z"
        );
        // A non-UTC input is normalized, not written through.
        assert_eq!(
            backup_suffix(datetime!(2026-08-16 23:45:01 -07:00)),
            "bak-20260817T064501Z"
        );
    }

    #[test]
    fn recognizes_only_its_own_convention() {
        assert!(is_our_backup(
            "klams.toml",
            "klams.toml.bak-20260816T234501Z"
        ));
        // Same-second disambiguator.
        assert!(is_our_backup(
            "klams.toml",
            "klams.toml.bak-20260816T234501Z-1"
        ));
        assert!(!is_our_backup(
            "klams.toml",
            "klams.toml.bak-20260816T234501Z-"
        ));
        assert!(!is_our_backup(
            "klams.toml",
            "klams.toml.bak-20260816T234501Z-x"
        ));
        // The three conventions already in /etc/klams stay untouched.
        assert!(!is_our_backup("klams.toml", "klams.toml.bak-016-pre704"));
        assert!(!is_our_backup("klams.toml", "klams.toml.bak"));
        assert!(!is_our_backup("klams.toml", "klams.toml.bak-2026-08-16"));
        assert!(!is_our_backup(
            "klams.toml",
            "other.toml.bak-20260816T234501Z"
        ));
    }

    #[test]
    fn write_in_place_keeps_the_inode_and_mode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("klams.toml");
        fs::write(&path, "original\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();
        let before = fs::metadata(&path).unwrap();

        write_in_place(&path, "rewritten\n").unwrap();

        let after = fs::metadata(&path).unwrap();
        assert_eq!(
            before.ino(),
            after.ino(),
            "inode changed — a rename crept in"
        );
        assert_eq!(after.mode() & 0o777, 0o640);
        assert_eq!(fs::read_to_string(&path).unwrap(), "rewritten\n");
    }

    #[test]
    fn backup_copies_content_and_mode_and_never_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("klams.toml");
        fs::write(&path, "secrets\n").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o640)).unwrap();

        let now = datetime!(2026-08-16 23:45:01 UTC);
        let backup = back_up(&path, now).unwrap();
        assert_eq!(fs::read_to_string(&backup).unwrap(), "secrets\n");
        assert_eq!(fs::metadata(&backup).unwrap().mode() & 0o777, 0o640);
    }

    /// Back-to-back edits are the normal case — an `add` then a
    /// `scopes`, or krot rotating several grants in one pass. A second
    /// backup inside the same second must neither clobber the first nor
    /// fail the edit.
    #[test]
    fn a_second_backup_in_the_same_second_gets_its_own_name() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("klams.toml");
        fs::write(&path, "first\n").unwrap();

        let now = datetime!(2026-08-16 23:45:01 UTC);
        let one = back_up(&path, now).unwrap();
        fs::write(&path, "second\n").unwrap();
        let two = back_up(&path, now).unwrap();

        assert_ne!(one, two);
        assert_eq!(fs::read_to_string(&one).unwrap(), "first\n");
        assert_eq!(fs::read_to_string(&two).unwrap(), "second\n");
        // And the suffix sorts after the bare name, so the directory
        // still lists chronologically.
        assert!(one < two);
        assert!(is_our_backup(
            "klams.toml",
            two.file_name().unwrap().to_str().unwrap()
        ));
    }

    #[test]
    fn restore_puts_the_original_back_through_the_same_inode() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("klams.toml");
        fs::write(&path, "good\n").unwrap();
        let backup = back_up(&path, datetime!(2026-08-16 23:45:01 UTC)).unwrap();
        let ino = fs::metadata(&path).unwrap().ino();

        write_in_place(&path, "broken\n").unwrap();
        restore(&path, &backup).unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "good\n");
        assert_eq!(fs::metadata(&path).unwrap().ino(), ino);
    }

    /// The guarantee the whole pipeline exists for: a config that
    /// fails validation after landing never stays live.
    #[test]
    fn a_write_that_fails_validation_is_rolled_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("klams.toml");
        fs::write(&path, "the good config\n").unwrap();
        let ino = fs::metadata(&path).unwrap().ino();

        let err = write_validated(
            &path,
            "the bad config\n",
            datetime!(2026-08-16 23:45:01 UTC),
            DEFAULT_RETAIN,
            &DurableBackup::default(),
            |_| bail!("nope"),
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("rolled back"), "{err}");
        assert_eq!(fs::read_to_string(&path).unwrap(), "the good config\n");
        assert_eq!(
            fs::metadata(&path).unwrap().ino(),
            ino,
            "the rollback replaced the file instead of rewriting it"
        );
        // The backup survives the rollback — it is the operator's
        // second chance if the restore itself was wrong.
        assert!(dir.path().join("klams.toml.bak-20260816T234501Z").exists());
    }

    #[test]
    fn a_write_that_validates_keeps_the_new_content() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("klams.toml");
        fs::write(&path, "old\n").unwrap();

        let written = write_validated(
            &path,
            "new\n",
            datetime!(2026-08-16 23:45:01 UTC),
            DEFAULT_RETAIN,
            &DurableBackup::default(),
            |landed| {
                assert_eq!(landed, "new\n", "validator sees what actually landed");
                Ok(())
            },
        )
        .unwrap();

        assert_eq!(fs::read_to_string(&path).unwrap(), "new\n");
        assert_eq!(fs::read_to_string(&written.backup).unwrap(), "old\n");
    }

    /// Sprint 046 (#1384): the rollback must not depend on being able
    /// to READ the durable backup, because once it is age-encrypted
    /// nothing on this machine can. The in-memory copy is what makes a
    /// failed validate at 2am self-heal without Ken's passphrase.
    #[test]
    fn rollback_does_not_read_the_durable_backup() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("klams.toml");
        fs::write(&path, "the good config\n").unwrap();

        let durable = DurableBackup {
            recipient: None,
            fingerprints: Vec::new(),
        };
        let backup = dir.path().join("klams.toml.bak-20260816T234501Z");

        let err = write_validated(
            &path,
            "the bad config\n",
            datetime!(2026-08-16 23:45:01 UTC),
            DEFAULT_RETAIN,
            &durable,
            |_| {
                // Make the durable copy unreadable mid-flight. A
                // rollback that reaches for it now would fail.
                let _ = fs::remove_file(&backup);
                bail!("nope")
            },
        )
        .unwrap_err()
        .to_string();

        assert!(err.contains("rolled back"), "{err}");
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "the good config\n",
            "the rollback must come from memory, not from the durable backup"
        );
    }

    /// An encrypted durable backup must not carry the plaintext, and
    /// its manifest must stay readable — that is the whole trade: krot
    /// can still tell what a backup holds without decrypting it.
    #[test]
    fn an_encrypted_backup_hides_the_config_but_not_its_manifest() {
        if std::process::Command::new("age")
            .arg("--version")
            .output()
            .is_err()
        {
            eprintln!("age not installed; skipping");
            return;
        }
        let out = std::process::Command::new("age-keygen").output().unwrap();
        let keytext = String::from_utf8(out.stdout).unwrap();
        let recipient = keytext
            .lines()
            .find_map(|l| l.strip_prefix("# public key: "))
            .unwrap()
            .to_string();

        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("klams.toml");
        fs::write(&path, "token = \"live-secret\"\n").unwrap();

        let durable = DurableBackup {
            recipient: Some(&recipient),
            fingerprints: vec![("claude-kubs0".into(), "0123456789ab".into())],
        };
        let written = write_validated(
            &path,
            "token = \"new-secret\"\n",
            datetime!(2026-08-16 23:45:01 UTC),
            DEFAULT_RETAIN,
            &durable,
            |_| Ok(()),
        )
        .unwrap();

        assert!(written.encrypted);
        assert!(written.backup.to_string_lossy().ends_with(".age"));
        let raw = fs::read(&written.backup).unwrap();
        assert!(
            !String::from_utf8_lossy(&raw).contains("live-secret"),
            "the durable backup still holds the old token in the clear"
        );

        let manifest = written.manifest.expect("manifest written");
        let text = fs::read_to_string(&manifest).unwrap();
        assert!(text.contains("claude-kubs0"), "{text}");
        assert!(text.contains("0123456789ab"), "{text}");
        assert!(
            !text.contains("live-secret"),
            "the manifest must carry fingerprints, never values: {text}"
        );
    }

    /// Prune must treat an encrypted backup and its manifest as ONE
    /// generation, or it would delete backups and leave their manifests
    /// behind describing files that no longer exist.
    #[test]
    fn prune_counts_an_encrypted_backup_and_its_manifest_together() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("klams.toml");
        fs::write(&path, "live\n").unwrap();
        for stamp in ["20260101T000000Z", "20260202T000000Z", "20260303T000000Z"] {
            fs::write(dir.path().join(format!("klams.toml.bak-{stamp}.age")), "x").unwrap();
            fs::write(
                dir.path()
                    .join(format!("klams.toml.bak-{stamp}.age.manifest.json")),
                "{}",
            )
            .unwrap();
        }
        assert!(is_our_backup(
            "klams.toml",
            "klams.toml.bak-20260101T000000Z.age"
        ));
        assert!(is_our_backup(
            "klams.toml",
            "klams.toml.bak-20260101T000000Z.age.manifest.json"
        ));

        prune_backups(&path, 2).unwrap();
        // 3 generations x 2 files, retaining 2 files => the oldest four
        // go. What must NOT happen is a manifest outliving its backup.
        for entry in fs::read_dir(dir.path()).unwrap().flatten() {
            let name = entry.file_name().to_string_lossy().to_string();
            if let Some(base) = name.strip_suffix(".manifest.json") {
                assert!(
                    dir.path().join(base).exists(),
                    "{name} outlived the backup it describes"
                );
            }
        }
    }

    #[test]
    fn prune_keeps_the_newest_and_spares_foreign_conventions() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("klams.toml");
        fs::write(&path, "live\n").unwrap();
        for stamp in [
            "20260101T000000Z",
            "20260202T000000Z",
            "20260303T000000Z",
            "20260404T000000Z",
        ] {
            fs::write(dir.path().join(format!("klams.toml.bak-{stamp}")), "x").unwrap();
        }
        fs::write(dir.path().join("klams.toml.bak-016-pre704"), "x").unwrap();

        let removed = prune_backups(&path, 2).unwrap();
        assert_eq!(removed.len(), 2);
        assert!(!dir.path().join("klams.toml.bak-20260101T000000Z").exists());
        assert!(!dir.path().join("klams.toml.bak-20260202T000000Z").exists());
        assert!(dir.path().join("klams.toml.bak-20260303T000000Z").exists());
        assert!(dir.path().join("klams.toml.bak-20260404T000000Z").exists());
        assert!(
            dir.path().join("klams.toml.bak-016-pre704").exists(),
            "pruned a backup it did not write"
        );
    }
}
