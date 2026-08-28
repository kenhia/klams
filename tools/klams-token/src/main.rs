//! `klams-token` — manage the `[[auth.tokens]]` grants in `klams.toml`.
//!
//! See `docs/usage.md` ("Managing auth grants") for the operator
//! recipes and `tools/klams-token/src/lib.rs` for why the write path
//! looks the way it does.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use klams_token::doc::{GrantView, GrantsDoc};
use klams_token::fingerprint::{token_digest, verify_delta, Change, GrantFingerprint};
use klams_token::{paths, verify, writer};
use klams_types::{validate_agent_name, Scope, TokenGrantConfig};
use rand::RngCore;
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

/// Exit code when `--verify` found at least one dead grant. Distinct
/// from 1 (the command failed) so a monitoring wrapper can tell "your
/// config is broken" from "a credential is broken".
const EXIT_DEAD_GRANT: i32 = 2;

#[derive(Debug, Parser)]
#[command(
    name = "klams-token",
    version,
    about = "Structural editor for the [[auth.tokens]] grants in klams.toml",
    long_about = "Edits klams.toml's auth grants structurally, so a write cannot clobber a \
                  sibling grant (korg #264). Every mutation takes a timestamped backup, \
                  fingerprints the grant set before and after, refuses anything but the change \
                  you asked for, validates the result against the schema klams-service boots \
                  from, and restores the backup if that validation fails.\n\n\
                  Token values are never printed without --reveal."
)]
struct Cli {
    /// Config to edit. Defaults to `$KLAMS_CONFIG`, then the shipped
    /// locations (`/ai/klams/config/klams.toml`,
    /// `/etc/klams/klams.toml`), then
    /// `$XDG_CONFIG_HOME/klams/klams.toml`.
    #[arg(long, global = true, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Machine-readable output on stdout.
    #[arg(long, global = true)]
    json: bool,

    /// Show what would change and stop before writing anything.
    #[arg(long, global = true)]
    dry_run: bool,

    /// age recipient the durable backup is encrypted to (sprint 046,
    /// #1384). Defaults to `$KLAMS_TOKEN_AGE_RECIPIENT`, then
    /// `backup.age-recipient` beside the config — the file is the
    /// primary route, because these commands run under `sudo` and sudo
    /// drops the environment.
    ///
    /// The recipient is a PUBLIC key. Its private half is
    /// passphrase-protected and lives off this machine; that is the
    /// point, and it is why `restore` needs Ken.
    #[arg(long, global = true, value_name = "age1…")]
    age_recipient: Option<String>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Decrypt an encrypted durable backup and print it, or put it
    /// back over the live config.
    ///
    /// Needs the age identity whose public half backups were encrypted
    /// to — deliberately Ken's, deliberately not on this machine.
    /// Losing the passphrase loses only undo history: the live config
    /// and the k-homelab store are the primaries.
    Restore {
        /// The `.bak-…Z.age` file to read.
        backup: PathBuf,
        /// age identity file. Use `-` to read it from stdin so it never
        /// touches this filesystem.
        #[arg(long, value_name = "PATH|-")]
        identity: String,
        /// Write it over the live config instead of printing it. Takes
        /// its own durable backup first, like any other write.
        #[arg(long)]
        apply: bool,
    },

    /// List the grants.
    List {
        /// Print token values. Off by default: these are live secrets
        /// and a terminal is a log.
        #[arg(long)]
        reveal: bool,
        /// Probe every grant against the running service — one
        /// authenticated request each — and report live/dead.
        #[arg(long)]
        verify: bool,
        /// Service base URL for `--verify`. Defaults to `$KLAMS_URL`,
        /// then the `[server]` block of the config being inspected.
        #[arg(long, value_name = "URL")]
        url: Option<String>,
    },

    /// Append a new grant. Never edits an existing one.
    Add {
        /// Short name. Prefixes the generated token and, unless
        /// --agent-name says otherwise, becomes the grant's identity.
        name: String,
        /// Comma-separated: read, write, manage, admin. Scopes are
        /// flat — "write" does not imply "read".
        #[arg(long, required = true, value_delimiter = ',', value_parser = parse_scope)]
        scopes: Vec<Scope>,
        /// Defaults to <name>.
        #[arg(long)]
        label: Option<String>,
        /// Identity memories written through this grant are attributed
        /// to. Defaults to <name>.
        #[arg(long)]
        agent_name: Option<String>,
        /// Print the generated token. You need it once, to hand to
        /// whatever will present it.
        #[arg(long)]
        reveal: bool,
    },

    /// Delete a grant by `agent_name` or label.
    Remove {
        selector: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },

    /// Change a grant's scopes, touching nothing else.
    Scopes {
        selector: String,
        /// Replace the scope set outright.
        #[arg(long, value_delimiter = ',', value_parser = parse_scope)]
        set: Vec<Scope>,
        /// Add these scopes to the existing set.
        #[arg(long, value_delimiter = ',', value_parser = parse_scope)]
        add: Vec<Scope>,
        /// Remove these scopes from the existing set.
        #[arg(long = "remove", value_delimiter = ',', value_parser = parse_scope)]
        remove: Vec<Scope>,
    },

    /// Replace a grant's token, preserving its identity and scopes.
    ///
    /// klams attributes memories by `agent_name`, not by token value,
    /// so rotating here does not orphan anything that agent wrote.
    Rotate {
        selector: String,
        /// Print the new token.
        #[arg(long)]
        reveal: bool,
    },
}

fn parse_scope(s: &str) -> Result<Scope, String> {
    match s.trim() {
        "read" => Ok(Scope::Read),
        "write" => Ok(Scope::Write),
        "manage" => Ok(Scope::Manage),
        "admin" => Ok(Scope::Admin),
        other => Err(format!(
            "unknown scope `{other}` (expected read, write, manage or admin)"
        )),
    }
}

/// The config under edit, plus the flags every command consults.
///
/// Bundled rather than threaded through each command's signature: these
/// travel together everywhere, and `before_text` in particular is only
/// meaningful next to the `doc` it was parsed from.
struct Session {
    path: PathBuf,
    /// The file exactly as it was read, so a write that would produce
    /// identical bytes can be skipped rather than churn a backup.
    before_text: String,
    doc: GrantsDoc,
    json: bool,
    dry_run: bool,
    /// Resolved age recipient for durable backups (#1384). `None` means
    /// encryption is not configured, and the write path says so out
    /// loud rather than letting a plaintext copy land quietly.
    recipient: Option<klams_token::backup::Recipient>,
    /// Operator notes a completed write leaves behind (where the
    /// backup went, what to reload). Flushed to stderr *after* the
    /// command reports its own result, so the terminal reads in the
    /// order things happened rather than interleaving by stream.
    notes: Vec<String>,
}

#[tokio::main]
async fn main() {
    if let Err(e) = run().await {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

async fn run() -> Result<()> {
    let cli = Cli::parse();
    let path = paths::resolve(cli.config.clone())?;
    let path2 = path.clone();
    let before_text = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "reading {} — it is root:klams 0640 on a deployed host, so this usually means sudo",
            path.display()
        )
    })?;
    let doc =
        GrantsDoc::parse(&before_text).with_context(|| format!("parsing {}", path.display()))?;
    let mut s = Session {
        path,
        before_text,
        doc,
        json: cli.json,
        dry_run: cli.dry_run,
        recipient: klams_token::backup::Recipient::resolve(cli.age_recipient.as_deref(), &path2)?,
        notes: Vec::new(),
    };

    let result = match &cli.command {
        Command::List {
            reveal,
            verify: do_verify,
            url,
        } => s.list(*reveal, *do_verify, url.as_deref()).await,
        Command::Add {
            name,
            scopes,
            label,
            agent_name,
            reveal,
        } => s.add(
            name,
            scopes,
            label.as_deref(),
            agent_name.as_deref(),
            *reveal,
        ),
        Command::Remove { selector, yes } => s.remove(selector, *yes),
        Command::Scopes {
            selector,
            set,
            add: to_add,
            remove: to_remove,
        } => s.scopes(selector, set, to_add, to_remove),
        Command::Rotate { selector, reveal } => s.rotate(selector, *reveal),
        Command::Restore {
            backup,
            identity,
            apply,
        } => s.restore(backup, identity, *apply),
    };
    for note in &s.notes {
        eprintln!("{note}");
    }
    result
}

impl Session {
    // ------------------------------------------------------------ list

    async fn list(&self, reveal: bool, do_verify: bool, url: Option<&str>) -> Result<()> {
        let grants = self.doc.grants()?;
        let mut liveness: Vec<Option<verify::Liveness>> = vec![None; grants.len()];

        if do_verify {
            let base = self.probe_url(url)?;
            eprintln!("probing {} grants against {base}", grants.len());
            let client = verify::client()?;
            for (i, g) in grants.iter().enumerate() {
                liveness[i] = Some(verify::probe(&client, &base, &g.token).await);
            }
        }

        if self.json {
            let rows: Vec<serde_json::Value> = grants
                .iter()
                .zip(&liveness)
                .map(|(g, l)| {
                    let mut row = serde_json::json!({
                        "index": g.index,
                        "identity": g.identity(),
                        "label": g.label,
                        "agent_name": g.agent_name,
                        "scopes": g.scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                        "token_fingerprint": token_digest(&g.token),
                    });
                    if reveal {
                        row["token"] = serde_json::Value::String(g.token.clone());
                    }
                    if let Some(l) = l {
                        row["liveness"] = serde_json::Value::String(l.label());
                    }
                    row
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&rows)?);
        } else {
            print_table(&grants, &liveness, reveal);
        }

        let dead: Vec<&GrantView> = grants
            .iter()
            .zip(&liveness)
            .filter(|(_, l)| l.as_ref().is_some_and(verify::Liveness::is_dead))
            .map(|(g, _)| g)
            .collect();
        if !dead.is_empty() {
            eprintln!(
                "\n{} grant(s) returned 401 — the service holds no such token: {}",
                dead.len(),
                dead.iter()
                    .map(|g| g.identity())
                    .collect::<Vec<_>>()
                    .join(", ")
            );
            eprintln!(
                "whatever presents these has a value the service does not; rotate with \
                 `klams-token rotate <identity>` and redeploy the consumer, or remove the grant."
            );
            std::process::exit(EXIT_DEAD_GRANT);
        }
        Ok(())
    }

    /// Where `--verify` should probe: `--url`, then `$KLAMS_URL`, then
    /// the `[server]` block of the very config being inspected.
    fn probe_url(&self, explicit: Option<&str>) -> Result<String> {
        if let Some(u) = explicit {
            return Ok(u.to_string());
        }
        if let Ok(u) = std::env::var("KLAMS_URL") {
            if !u.is_empty() {
                return Ok(u);
            }
        }
        let parsed: ServerSlice = toml::from_str(&self.doc.to_string()).context(
            "no --url and no $KLAMS_URL, and the config has no readable [server] block to \
             derive one from",
        )?;
        Ok(verify::base_url_from_config(
            &parsed.server.listen_addr,
            parsed.server.port,
        ))
    }

    // ------------------------------------------------------------- add

    fn add(
        &mut self,
        name: &str,
        scopes: &[Scope],
        label: Option<&str>,
        agent_name: Option<&str>,
        reveal: bool,
    ) -> Result<()> {
        // The short name prefixes a live credential, so it gets the
        // same charset rules as an identity rather than none at all.
        validate_agent_name(name)
            .map_err(|r| anyhow::anyhow!("`{name}` is not a usable short name ({r})"))?;
        let agent = agent_name.unwrap_or(name).to_string();
        validate_agent_name(&agent)
            .map_err(|r| anyhow::anyhow!("`{agent}` is not a valid agent_name ({r})"))?;

        let before = self.doc.fingerprints()?;
        if let Ok(existing) = self.doc.find(&agent) {
            bail!(
                "a grant with identity `{}` already exists (index {}) — `add` never edits an \
                 existing grant; use `rotate` to replace its token or `scopes` to change its \
                 permissions",
                existing.identity(),
                existing.index
            );
        }

        let token = generate_token(name);
        let grant = TokenGrantConfig {
            token: token.clone(),
            scopes: scopes.to_vec(),
            label: Some(label.unwrap_or(name).to_string()),
            agent_name: Some(agent.clone()),
        };
        grant
            .validate()
            .context("the grant you asked for is not one klams-service would accept")?;

        self.doc.add(&grant)?;
        let change = Change::Added(GrantFingerprint::new(agent.clone(), &token));
        self.commit(&before, &change)?;

        // A dry run generated a token and threw it away — reporting
        // it would hand the operator a credential that exists nowhere.
        if self.dry_run {
            return self.report(
                &serde_json::json!({
                    "action": "add",
                    "identity": agent,
                    "scopes": scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                    "dry_run": true,
                }),
                &format!(
                    "would add grant `{agent}` [{}]; re-run without --dry-run to \
                     generate and write its token",
                    render_scopes(scopes)
                ),
            );
        }

        if self.json {
            let mut out = serde_json::json!({
                "action": "add",
                "identity": agent,
                "scopes": scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                "token_fingerprint": token_digest(&token),
                "dry_run": false,
            });
            if reveal {
                out["token"] = serde_json::Value::String(token.clone());
            }
            println!("{}", serde_json::to_string_pretty(&out)?);
        } else {
            println!(
                "added grant `{agent}` [{}] (token {})",
                render_scopes(scopes),
                token_digest(&token)
            );
            if reveal {
                println!("token: {token}");
            } else {
                println!("re-run with --reveal to print the token value (you need it once).");
            }
        }
        Ok(())
    }

    // ---------------------------------------------------------- remove

    fn remove(&mut self, selector: &str, yes: bool) -> Result<()> {
        let target = self.doc.find(selector)?;
        let before = self.doc.fingerprints()?;

        if !yes && !self.dry_run && !self.confirm_removal(&target)? {
            println!("aborted; nothing was written.");
            return Ok(());
        }

        self.doc.remove(target.index)?;
        self.commit(&before, &Change::Removed(target.fingerprint()))?;

        self.report(
            &serde_json::json!({
                "action": "remove",
                "identity": target.identity(),
                "dry_run": self.dry_run,
            }),
            &format!(
                "{} grant `{}`",
                if self.dry_run {
                    "would remove"
                } else {
                    "removed"
                },
                target.identity()
            ),
        )
    }

    fn confirm_removal(&self, target: &GrantView) -> Result<bool> {
        if self.json {
            bail!("--json requires --yes (there is nobody to answer the confirmation prompt)");
        }
        if !std::io::stdin().is_terminal() {
            bail!(
                "removing `{}` needs confirmation and stdin is not a terminal — pass --yes",
                target.identity()
            );
        }
        print!(
            "remove grant `{}` (label {}, scopes {}, token {})? [y/N] ",
            target.identity(),
            target.label.as_deref().unwrap_or("-"),
            target.scope_list(),
            token_digest(&target.token)
        );
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        Ok(matches!(
            answer.trim().to_ascii_lowercase().as_str(),
            "y" | "yes"
        ))
    }

    // ---------------------------------------------------------- scopes

    fn scopes(
        &mut self,
        selector: &str,
        set: &[Scope],
        to_add: &[Scope],
        to_remove: &[Scope],
    ) -> Result<()> {
        if set.is_empty() && to_add.is_empty() && to_remove.is_empty() {
            bail!("nothing to do: pass --set, --add or --remove");
        }
        if !set.is_empty() && (!to_add.is_empty() || !to_remove.is_empty()) {
            bail!("--set replaces the whole scope set; it cannot be combined with --add/--remove");
        }

        let target = self.doc.find(selector)?;
        let before = self.doc.fingerprints()?;
        let next = next_scopes(&target.scopes, set, to_add, to_remove);

        if next == target.scopes {
            return self.report(
                &serde_json::json!({
                    "action": "scopes",
                    "identity": target.identity(),
                    "changed": false,
                    "scopes": next.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                }),
                &format!(
                    "`{}` already has scopes {} — nothing to write",
                    target.identity(),
                    render_scopes(&next)
                ),
            );
        }

        self.doc.set_scopes(target.index, &next)?;
        // Scopes are not part of a fingerprint, so the declared change
        // is "the grant set must come out identical" — which is exactly
        // the guarantee wanted here: no sibling touched, no token
        // disturbed.
        self.commit(&before, &Change::None)?;

        self.report(
            &serde_json::json!({
                "action": "scopes",
                "identity": target.identity(),
                "changed": true,
                "from": target.scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                "to": next.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                "dry_run": self.dry_run,
            }),
            &format!(
                "`{}` scopes {} -> {}",
                target.identity(),
                render_scopes(&target.scopes),
                render_scopes(&next)
            ),
        )
    }

    // ---------------------------------------------------------- rotate

    fn rotate(&mut self, selector: &str, reveal: bool) -> Result<()> {
        let target = self.doc.find(selector)?;
        let before = self.doc.fingerprints()?;

        // Keep the existing prefix if the token has one, so a rotated
        // value still announces which consumer it belongs to.
        let prefix = target
            .token
            .split_once('-')
            .map_or_else(|| target.identity(), |(p, _)| p.to_string());
        let new_token = generate_token(&prefix);

        self.doc.set_token(target.index, &new_token)?;
        self.commit(
            &before,
            &Change::Rotated {
                key: target.identity(),
            },
        )?;

        if self.dry_run {
            return self.report(
                &serde_json::json!({
                    "action": "rotate",
                    "identity": target.identity(),
                    "old_fingerprint": token_digest(&target.token),
                    "dry_run": true,
                }),
                &format!(
                    "would rotate `{}` (token {}), keeping its agent_name; re-run without \
                     --dry-run to generate and write the new value",
                    target.identity(),
                    token_digest(&target.token)
                ),
            );
        }

        if self.json {
            let mut out = serde_json::json!({
                "action": "rotate",
                "identity": target.identity(),
                "old_fingerprint": token_digest(&target.token),
                "new_fingerprint": token_digest(&new_token),
                "dry_run": false,
            });
            if reveal {
                out["token"] = serde_json::Value::String(new_token.clone());
            }
            println!("{}", serde_json::to_string_pretty(&out)?);
        } else {
            println!(
                "rotated `{}`: token {} -> {} (agent_name unchanged, so its memories stay \
                 attributed)",
                target.identity(),
                token_digest(&target.token),
                token_digest(&new_token)
            );
            if reveal {
                println!("token: {new_token}");
            } else {
                println!("re-run with --reveal to print the new token value.");
            }
        }
        Ok(())
    }

    // --------------------------------------------------------- restore

    /// Decrypt an encrypted durable backup (sprint 046, #1384).
    ///
    /// Restore is a command rather than an improvisation because the
    /// alternative — an operator reaching for `age -d` under pressure
    /// with a broken config live — is where the mistakes are. `--apply`
    /// goes through the same validated write pipeline as every other
    /// mutation, so putting an old config back cannot itself break the
    /// service.
    fn restore(&mut self, backup: &std::path::Path, identity: &str, apply: bool) -> Result<()> {
        // `-` reads the identity from stdin so it never touches this
        // filesystem, which is the whole premise of keeping it off the
        // homelab.
        let tmp;
        let identity_path = if identity == "-" {
            use std::io::Read as _;
            let mut buf = String::new();
            std::io::stdin()
                .read_to_string(&mut buf)
                .context("reading the age identity from stdin")?;
            tmp = tempfile::NamedTempFile::new().context("staging the identity")?;
            std::fs::write(tmp.path(), buf.as_bytes()).context("staging the identity")?;
            tmp.path().to_path_buf()
        } else {
            std::path::PathBuf::from(identity)
        };

        let text = klams_token::backup::decrypt(&identity_path, backup)?;
        // Prove it is a config before offering to make it the live one.
        let doc = GrantsDoc::parse(&text).with_context(|| {
            format!("{} decrypted, but does not parse as TOML", backup.display())
        })?;
        let grants = doc.fingerprints()?;

        if !apply {
            if self.json {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "backup": backup.display().to_string(),
                        "grants": grants.len(),
                        "fingerprints": grants
                            .iter()
                            .map(|f| (f.key.clone(), serde_json::Value::String(f.token.clone())))
                            .collect::<serde_json::Map<_, _>>(),
                    }))?
                );
            } else {
                print!("{text}");
                eprintln!(
                    "\n({} grants; re-run with --apply to make this the live config)",
                    grants.len()
                );
            }
            return Ok(());
        }

        if self.dry_run {
            eprintln!(
                "dry run: {} would be restored over {} ({} grants)",
                backup.display(),
                self.path.display(),
                grants.len()
            );
            return Ok(());
        }

        let before = self.doc.fingerprints()?;
        let durable = writer::DurableBackup {
            recipient: self.recipient.as_ref().map(|r| r.value.as_str()),
            fingerprints: before
                .iter()
                .map(|f| (f.key.clone(), f.token.clone()))
                .collect(),
        };
        let written = writer::write_validated(
            &self.path,
            &text,
            time::OffsetDateTime::now_utc(),
            writer::DEFAULT_RETAIN,
            &durable,
            |landed| {
                let errors = GrantsDoc::parse(landed)?.auth()?.errors();
                if errors.is_empty() {
                    Ok(())
                } else {
                    bail!("{}", errors.join("; "))
                }
            },
        )?;
        self.notes.push(format!(
            "restored {} over {} ({} grants); the config it replaced is backed up at {}",
            backup.display(),
            self.path.display(),
            grants.len(),
            written.backup.display()
        ));
        self.notes.push(
            "reload the service to pick this up: sudo systemctl reload klams-service".to_string(),
        );
        Ok(())
    }

    // ---------------------------------------------------------- commit

    /// The write pipeline every mutation goes through.
    fn commit(&mut self, before: &[GrantFingerprint], change: &Change) -> Result<()> {
        // 1. Fingerprint-and-refuse: nothing but the declared change.
        let after = self.doc.fingerprints()?;
        verify_delta(before, &after, change)?;

        // 2. Would klams-service boot on the result? Same rules, same
        //    definition — `AuthConfig::errors` is what
        //    `--validate-config` reports too.
        let auth = self.doc.auth()?;
        let errors = auth.errors();
        if !errors.is_empty() {
            bail!(
                "refusing to write: the resulting config would not start klams-service\n  {}",
                errors.join("\n  ")
            );
        }
        for w in auth.warnings() {
            eprintln!("warning: {w}");
        }

        let new_text = self.doc.to_string();
        if new_text == self.before_text {
            eprintln!("note: the file is already in the requested state; nothing written.");
            return Ok(());
        }

        if self.dry_run {
            eprintln!(
                "dry run: {} would be rewritten ({} grants, delta verified, result validates)",
                self.path.display(),
                after.len()
            );
            return Ok(());
        }

        // 3. Backup, write through the existing inode, re-read what
        //    actually landed, and roll back if it does not validate.
        // The manifest describes the config being REPLACED — `before` is
        // its grant set — so an encrypted backup stays legible to krot
        // and to an audit without anyone decrypting it (#1384).
        let durable = writer::DurableBackup {
            recipient: self.recipient.as_ref().map(|r| r.value.as_str()),
            fingerprints: before
                .iter()
                .map(|f| (f.key.clone(), f.token.clone()))
                .collect(),
        };
        let written = writer::write_validated(
            &self.path,
            &new_text,
            time::OffsetDateTime::now_utc(),
            writer::DEFAULT_RETAIN,
            &durable,
            |landed| {
                let errors = GrantsDoc::parse(landed)?.auth()?.errors();
                if errors.is_empty() {
                    Ok(())
                } else {
                    bail!("{}", errors.join("; "))
                }
            },
        )?;

        self.notes.push(format!(
            "backup: {}{}",
            written.backup.display(),
            if written.encrypted {
                " (age-encrypted)"
            } else {
                ""
            }
        ));
        if let Some(m) = &written.manifest {
            self.notes.push(format!("manifest: {}", m.display()));
        }
        if !written.encrypted {
            // Never silent: a plaintext copy of every live token just
            // landed on disk, which is the exact hazard #1377 found
            // seven instances of.
            self.notes.push(format!(
                "WARNING: this backup is PLAINTEXT and holds every live token. Configure an age \
                 recipient to encrypt it — put a public `age1…` key in {}",
                self.path
                    .parent()
                    .unwrap_or_else(|| std::path::Path::new("."))
                    .join(klams_token::backup::RECIPIENT_FILE)
                    .display()
            ));
        }
        if !written.pruned.is_empty() {
            self.notes
                .push(format!("pruned {} old backup(s)", written.pruned.len()));
        }
        self.notes.push(
            "reload the service to pick this up: sudo systemctl reload klams-service \
             (SIGHUP hot-reloads [[auth.tokens]] since sprint 018 — a restart is not needed)"
                .to_string(),
        );
        Ok(())
    }

    /// Print a command's own result. Under `--dry-run` the human line
    /// is marked, so "removed grant `x`" can never be read as a thing
    /// that happened when it did not.
    fn report(&self, json: &serde_json::Value, human: &str) -> Result<()> {
        if self.json {
            println!("{}", serde_json::to_string_pretty(json)?);
        } else if self.dry_run {
            println!("dry run — nothing written: {human}");
        } else {
            println!("{human}");
        }
        Ok(())
    }
}

/// Just the `[server]` block, for deriving a probe URL.
#[derive(serde::Deserialize)]
struct ServerSlice {
    server: ServerBlock,
}

#[derive(serde::Deserialize)]
struct ServerBlock {
    listen_addr: String,
    port: u16,
}

/// Apply `--set` / `--add` / `--remove` and canonicalize, so a re-run
/// that changes nothing produces byte-identical output.
fn next_scopes(
    current: &[Scope],
    set: &[Scope],
    to_add: &[Scope],
    to_remove: &[Scope],
) -> Vec<Scope> {
    let mut next: Vec<Scope> = if set.is_empty() {
        current.to_vec()
    } else {
        set.to_vec()
    };
    for s in to_add {
        if !next.contains(s) {
            next.push(*s);
        }
    }
    next.retain(|s| !to_remove.contains(s));
    next.sort_by_key(|s| match s {
        Scope::Read => 0,
        Scope::Write => 1,
        Scope::Manage => 2,
        Scope::Admin => 3,
    });
    next.dedup();
    next
}

/// `<short-name>-<32 random bytes, hex>` — the convention already
/// visible in the live file (`alice_…`, `mind-…`, `bench-…`), which is
/// `openssl rand -hex 32` with a readable prefix.
fn generate_token(name: &str) -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill_bytes(&mut bytes);
    let hex = bytes.iter().fold(String::with_capacity(64), |mut acc, b| {
        use std::fmt::Write as _;
        let _ = write!(acc, "{b:02x}");
        acc
    });
    format!("{name}-{hex}")
}

fn render_scopes(scopes: &[Scope]) -> String {
    if scopes.is_empty() {
        return "(none)".into();
    }
    scopes
        .iter()
        .map(|s| s.as_str())
        .collect::<Vec<_>>()
        .join(",")
}

fn print_table(grants: &[GrantView], liveness: &[Option<verify::Liveness>], reveal: bool) {
    let token_header = if reveal { "TOKEN" } else { "FINGERPRINT" };
    let token_col: Vec<String> = grants
        .iter()
        .map(|g| {
            if reveal {
                g.token.clone()
            } else {
                token_digest(&g.token)
            }
        })
        .collect();

    let w_id = width("IDENTITY", grants.iter().map(GrantView::identity));
    let w_label = width(
        "LABEL",
        grants
            .iter()
            .map(|g| g.label.clone().unwrap_or_else(|| "-".into())),
    );
    let w_scopes = width("SCOPES", grants.iter().map(GrantView::scope_list));
    let w_token = width(token_header, token_col.iter().cloned());

    let verifying = liveness.iter().any(Option::is_some);
    let mut header = format!(
        "{:<4}{:<w_id$}  {:<w_label$}  {:<w_scopes$}  {:<w_token$}",
        "IDX", "IDENTITY", "LABEL", "SCOPES", token_header
    );
    if verifying {
        header.push_str("  STATUS");
    }
    println!("{header}");

    for ((g, token), l) in grants.iter().zip(&token_col).zip(liveness) {
        let mut row = format!(
            "{:<4}{:<w_id$}  {:<w_label$}  {:<w_scopes$}  {:<w_token$}",
            g.index,
            g.identity(),
            g.label.clone().unwrap_or_else(|| "-".into()),
            g.scope_list(),
            token
        );
        if let Some(l) = l {
            row.push_str("  ");
            row.push_str(&l.label());
        }
        println!("{}", row.trim_end());
    }
}

fn width(header: &str, values: impl Iterator<Item = String>) -> usize {
    values
        .map(|v| v.chars().count())
        .max()
        .unwrap_or(0)
        .max(header.len())
}
