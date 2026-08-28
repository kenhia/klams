//! Encrypted durable backups (sprint 046, WI #1384).
//!
//! A backup of a secret-bearing file is itself a secret-bearing
//! surface. krot's grant inventory (#1377) found seven
//! `klams.toml.bak-*` files in `/etc/klams`, several still holding the
//! **current** live token for most of the 14 grants — same
//! `0640 root:klams` exposure as the config, none of the attention. And
//! every rotation minted another one, so the pile grew precisely as the
//! tool got used.
//!
//! Ken's design (reviewing #1377) splits the backup's two roles, which
//! is the move that makes encryption affordable:
//!
//! * The **same-run transactional rollback** — restore-on-failed-validate
//!   — uses the in-memory copy the writer already holds. No plaintext
//!   outlives the operation, and a failed validate at 2am still
//!   self-heals with nobody awake.
//! * Only the **durable** `.bak` on disk is encrypted, to a recipient
//!   whose private half is passphrase-protected and kept OFF the
//!   homelab filesystem. Restoring from one requires Ken, deliberately.
//!
//! `age` rather than new tooling: the homelab already runs it for the
//! k-homelab secret store, the recipient string is public and can live
//! in config, and Ken generates the keypair off-homelab.
//!
//! Beside each encrypted backup sits a **plaintext manifest** of
//! `{agent_name: sha256(token)[:12]}` fingerprints, so what a backup
//! contains stays knowable — to krot, to an audit, to a human deciding
//! whether a file is still needed — without decrypting anything or
//! learning any token value.
//!
//! Losing the passphrase loses only undo history. The live config and
//! the k-homelab store are the primaries.

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::os::unix::fs::{MetadataExt, PermissionsExt};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

/// Filename holding the age recipient, looked for beside the config.
pub const RECIPIENT_FILE: &str = "backup.age-recipient";
/// Environment override.
pub const RECIPIENT_ENV: &str = "KLAMS_TOKEN_AGE_RECIPIENT";
/// Extension appended to an encrypted backup.
pub const AGE_EXT: &str = ".age";
/// Extension for the plaintext fingerprint manifest.
pub const MANIFEST_EXT: &str = ".manifest.json";

/// Where the recipient came from, so the tool can say so rather than
/// leaving the operator to guess whether encryption is on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RecipientSource {
    Flag,
    Env,
    File(PathBuf),
}

/// A resolved age recipient.
#[derive(Debug, Clone)]
pub struct Recipient {
    pub value: String,
    pub source: RecipientSource,
}

impl Recipient {
    /// Resolve the recipient: explicit flag, then `$KLAMS_TOKEN_AGE_RECIPIENT`,
    /// then `backup.age-recipient` beside the config.
    ///
    /// The file is the primary route on a systemd host: these commands
    /// run under `sudo`, which drops the environment by default, so an
    /// env var is the thing most likely to be silently absent exactly
    /// when it matters.
    ///
    /// `None` means encryption is not configured — the caller decides
    /// whether that is fatal, and this crate's answer is that it is not:
    /// refusing to edit a config because backups cannot be encrypted
    /// would make a hardening feature into an outage.
    ///
    /// # Errors
    /// If the recipient file exists but cannot be read.
    pub fn resolve(explicit: Option<&str>, config: &Path) -> Result<Option<Self>> {
        if let Some(v) = explicit {
            return Ok(Some(Self {
                value: v.trim().to_string(),
                source: RecipientSource::Flag,
            }));
        }
        if let Ok(v) = std::env::var(RECIPIENT_ENV) {
            if !v.trim().is_empty() {
                return Ok(Some(Self {
                    value: v.trim().to_string(),
                    source: RecipientSource::Env,
                }));
            }
        }
        let path = config
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(RECIPIENT_FILE);
        if !path.exists() {
            return Ok(None);
        }
        let text = std::fs::read_to_string(&path)
            .with_context(|| format!("reading {}", path.display()))?;
        // The file may carry comment lines; the recipient is the first
        // line that looks like one.
        let value = text
            .lines()
            .map(str::trim)
            .find(|l| l.starts_with("age1"))
            .map(str::to_string);
        match value {
            Some(value) => Ok(Some(Self {
                value,
                source: RecipientSource::File(path),
            })),
            None => bail!(
                "{} exists but holds no `age1…` recipient — remove it or put the public \
                 recipient in it (the private identity must stay off this machine)",
                path.display()
            ),
        }
    }
}

/// Encrypt `plaintext` to `dest` for `recipient`, then give `dest` the
/// same ownership and mode as `like`.
///
/// # Errors
/// If `age` is missing, refuses the recipient, or writes nothing.
pub fn encrypt(recipient: &str, plaintext: &str, dest: &Path, like: &Path) -> Result<()> {
    let mut child = Command::new("age")
        .arg("-r")
        .arg(recipient)
        .arg("-o")
        .arg(dest)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("running `age` (is it installed? the homelab ships it for k-homelab)")?;
    child
        .stdin
        .take()
        .context("age stdin")?
        .write_all(plaintext.as_bytes())
        .context("writing plaintext to age")?;
    let out = child.wait_with_output().context("waiting for age")?;
    if !out.status.success() {
        bail!(
            "age failed encrypting to {}: {}",
            dest.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    // An encrypted file is still not for the world; match the original.
    if let Ok(meta) = std::fs::metadata(like) {
        let _ =
            std::fs::set_permissions(dest, std::fs::Permissions::from_mode(meta.mode() & 0o7777));
        let _ = std::os::unix::fs::chown(dest, Some(meta.uid()), Some(meta.gid()));
    }
    Ok(())
}

/// Decrypt `backup` using the age identity at `identity`.
///
/// # Errors
/// If `age` is missing, the identity is wrong, or the file is not an
/// age file.
pub fn decrypt(identity: &Path, backup: &Path) -> Result<String> {
    let out = Command::new("age")
        .arg("-d")
        .arg("-i")
        .arg(identity)
        .arg(backup)
        .output()
        .context("running `age`")?;
    if !out.status.success() {
        bail!(
            "age could not decrypt {}: {}",
            backup.display(),
            String::from_utf8_lossy(&out.stderr).trim()
        );
    }
    String::from_utf8(out.stdout).context("decrypted config is not UTF-8")
}

/// The plaintext manifest for a config's grants: agent name to token
/// fingerprint, nothing else.
///
/// Deliberately NOT the labels or scopes — the question a manifest
/// answers is "does this backup still hold a live token?", and the
/// fingerprint answers it exactly. Anything more is a second copy of
/// the config's structure sitting in the clear.
#[must_use]
pub fn manifest(grants: &[(String, String)]) -> serde_json::Value {
    let map: serde_json::Map<String, serde_json::Value> = grants
        .iter()
        .map(|(name, digest)| (name.clone(), serde_json::Value::String(digest.clone())))
        .collect();
    serde_json::json!({
        "fingerprints": map,
        "digest": "sha256[:12] of each grant's token value",
    })
}

/// Write `value` beside a backup as its manifest.
///
/// # Errors
/// If the manifest cannot be written.
pub fn write_manifest(backup: &Path, value: &serde_json::Value) -> Result<PathBuf> {
    let mut name = backup.as_os_str().to_os_string();
    name.push(MANIFEST_EXT);
    let path = PathBuf::from(name);
    let text = serde_json::to_string_pretty(value).context("serializing manifest")?;
    std::fs::write(&path, format!("{text}\n"))
        .with_context(|| format!("writing {}", path.display()))?;
    // The manifest is not secret — it is the thing that stays readable
    // when the backup does not — but it need not be world-readable
    // either.
    let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o640));
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recipient_prefers_flag_then_env_then_file() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("klams.toml");
        std::fs::write(&config, "").unwrap();
        std::fs::write(
            dir.path().join(RECIPIENT_FILE),
            "# Ken's off-homelab key\nage1filefilefilefile\n",
        )
        .unwrap();

        let from_file = Recipient::resolve(None, &config).unwrap().unwrap();
        assert_eq!(from_file.value, "age1filefilefilefile");
        assert!(matches!(from_file.source, RecipientSource::File(_)));

        let explicit = Recipient::resolve(Some("age1flag"), &config)
            .unwrap()
            .unwrap();
        assert_eq!(explicit.value, "age1flag");
        assert_eq!(explicit.source, RecipientSource::Flag);
    }

    /// Encryption not being configured must not be an error: refusing
    /// to edit the config because backups cannot be encrypted would
    /// turn a hardening feature into an outage.
    #[test]
    fn absent_recipient_is_none_not_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("klams.toml");
        std::fs::write(&config, "").unwrap();
        assert!(Recipient::resolve(None, &config).unwrap().is_none());
    }

    #[test]
    fn recipient_file_without_a_recipient_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = dir.path().join("klams.toml");
        std::fs::write(&config, "").unwrap();
        std::fs::write(dir.path().join(RECIPIENT_FILE), "# just a comment\n").unwrap();
        assert!(Recipient::resolve(None, &config).is_err());
    }

    #[test]
    fn manifest_carries_fingerprints_and_no_token_values() {
        let m = manifest(&[("claude-kubs0".into(), "0123456789ab".into())]);
        let text = serde_json::to_string(&m).unwrap();
        assert!(text.contains("claude-kubs0"));
        assert!(text.contains("0123456789ab"));
    }

    /// The round trip, against the real `age` binary the design names.
    #[test]
    fn encrypt_then_decrypt_round_trips() {
        if Command::new("age").arg("--version").output().is_err() {
            eprintln!("age not installed; skipping round-trip");
            return;
        }
        let dir = tempfile::tempdir().unwrap();
        let identity = dir.path().join("id.txt");
        let out = Command::new("age-keygen").output().expect("age-keygen");
        let keytext = String::from_utf8(out.stdout).unwrap();
        std::fs::write(&identity, keytext.as_bytes()).unwrap();
        let recipient = keytext
            .lines()
            .find_map(|l| l.strip_prefix("# public key: "))
            .expect("recipient")
            .to_string();

        let like = dir.path().join("klams.toml");
        std::fs::write(&like, "x").unwrap();
        let dest = dir.path().join("klams.toml.bak-20260827T000000Z.age");
        let secret = "[[auth.tokens]]\ntoken = \"live-secret-value\"\n";
        encrypt(&recipient, secret, &dest, &like).unwrap();

        // The ciphertext must not carry the plaintext.
        let raw = std::fs::read(&dest).unwrap();
        assert!(
            !String::from_utf8_lossy(&raw).contains("live-secret-value"),
            "the encrypted backup still holds the token in the clear"
        );
        assert_eq!(decrypt(&identity, &dest).unwrap(), secret);
    }
}
