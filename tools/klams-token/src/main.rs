//! `klams-token` — manage the `[[auth.tokens]]` grants in `klams.toml`.
//!
//! See `docs/usage.md` ("Managing auth grants") for the operator
//! recipes and `tools/klams-token/src/lib.rs` for why the write path
//! looks the way it does.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use klams_token::doc::{GrantsDoc, IdentityView};
use klams_token::fingerprint::{verify_delta, Change, GrantFingerprint};
use klams_token::{paths, writer};
use klams_types::{IdentityConfig, Scope};
use std::io::{IsTerminal, Write};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "klams-token",
    version,
    about = "Structural editor for the [[auth.identities]] rows in klams.toml",
    long_about = "Edits klams.toml's auth identities structurally, so a write cannot clobber a \
                  sibling row (korg #264). Every mutation takes a timestamped backup, \
                  fingerprints the identity set before and after, refuses anything but the \
                  change you asked for, validates the result against the schema klams-service \
                  boots from, and restores the backup if that validation fails.\n\n\
                  The `[[auth.tokens]]` grants this tool was built for are retired \
                  (sprint 052); an identity carries no secret, so there is nothing to reveal \
                  and nothing to rotate."
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

    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Edit the `[[auth.identities]]` table.
    ///
    /// The successor to the retired token grants: a caller declares its
    /// name in `X-Homelab-Agent` and klams looks the row up by that
    /// name. There is no secret here, so there is nothing to reveal and
    /// nothing to rotate — which is why the subcommands are
    /// list/add/remove/scopes/nodes and stop there.
    ///
    /// Kept as a subcommand group rather than flattened in sprint 052,
    /// even though it is now the only one: `klams-token identity list`
    /// is the documented way to read the live roster without opening
    /// the file (krot WI 2466), and it is in operator muscle memory.
    #[command(subcommand)]
    Identity(IdentityCommand),
}

#[derive(Debug, Subcommand)]
enum IdentityCommand {
    /// List the identities.
    List,

    /// Append a new identity. Never edits an existing one.
    Add {
        /// The `agent_name` callers will declare. Also the identity
        /// memories written under it are attributed to.
        name: String,
        /// Comma-separated: read, write, manage, admin. Scopes are
        /// flat — "write" does not imply "read".
        #[arg(long, required = true, value_delimiter = ',', value_parser = parse_scope)]
        scopes: Vec<Scope>,
        /// Defaults to <name>.
        #[arg(long)]
        label: Option<String>,
        /// Tailnet nodes this identity may be used from. Only enforced
        /// when `[auth.whois] enforce = true`, which is off by default;
        /// until then it is documentation.
        #[arg(long, value_delimiter = ',')]
        nodes: Vec<String>,
    },

    /// Delete an identity by `agent_name` or label.
    Remove {
        selector: String,
        /// Skip the confirmation prompt.
        #[arg(long)]
        yes: bool,
    },

    /// Change an identity's scopes, touching nothing else.
    Scopes {
        selector: String,
        #[arg(long, value_delimiter = ',', value_parser = parse_scope)]
        set: Vec<Scope>,
        #[arg(long, value_delimiter = ',', value_parser = parse_scope)]
        add: Vec<Scope>,
        #[arg(long = "remove", value_delimiter = ',', value_parser = parse_scope)]
        remove: Vec<Scope>,
    },

    /// Replace an identity's `nodes` pin list. Pass no nodes to unpin.
    Nodes {
        selector: String,
        #[arg(long = "set", value_delimiter = ',')]
        set: Vec<String>,
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

/// The identity set's fingerprints, taken before a write.
///
/// Sprint 049 made this two sets, one per auth table, so "my identity
/// edit also removed a token grant" was a refusal rather than a
/// discovery. Sprint 052 deleted the token table, so there is one set
/// again — but the guard it feeds is unchanged: an edit must produce
/// exactly the declared change and nothing else.
struct Snapshot {
    identities: Vec<GrantFingerprint>,
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
    /// Operator notes a completed write leaves behind (where the
    /// backup went, what to reload). Flushed to stderr *after* the
    /// command reports its own result, so the terminal reads in the
    /// order things happened rather than interleaving by stream.
    notes: Vec<String>,
}

fn main() {
    if let Err(e) = run() {
        eprintln!("error: {e:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let cli = Cli::parse();
    let path = paths::resolve(cli.config.clone())?;
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
        notes: Vec::new(),
    };

    let result = match &cli.command {
        Command::Identity(cmd) => match cmd {
            IdentityCommand::List => s.identity_list(),
            IdentityCommand::Add {
                name,
                scopes,
                label,
                nodes,
            } => s.identity_add(name, scopes, label.as_deref(), nodes),
            IdentityCommand::Remove { selector, yes } => s.identity_remove(selector, *yes),
            IdentityCommand::Scopes {
                selector,
                set,
                add: to_add,
                remove: to_remove,
            } => s.identity_scopes(selector, set, to_add, to_remove),
            IdentityCommand::Nodes { selector, set } => s.identity_nodes(selector, set),
        },
    };
    for note in &s.notes {
        eprintln!("{note}");
    }
    result
}

impl Session {
    // ---------------------------------------------------- identities

    fn identity_list(&self) -> Result<()> {
        let identities = self.doc.identities()?;
        if self.json {
            let rows: Vec<serde_json::Value> = identities
                .iter()
                .map(|i| {
                    serde_json::json!({
                        "index": i.index,
                        "identity": i.identity(),
                        "label": i.label,
                        "agent_name": i.agent_name,
                        "scopes": i.scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                        "nodes": i.nodes,
                    })
                })
                .collect();
            println!("{}", serde_json::to_string_pretty(&rows)?);
            return Ok(());
        }
        if identities.is_empty() {
            println!("no `[[auth.identities]]` rows.");
            return Ok(());
        }
        let w_name = identities
            .iter()
            .map(|i| i.agent_name.len())
            .max()
            .unwrap_or(10)
            .max("agent_name".len());
        let w_scopes = identities
            .iter()
            .map(|i| i.scope_list().len())
            .max()
            .unwrap_or(6)
            .max("scopes".len());
        println!("{:<w_name$}  {:<w_scopes$}  nodes", "agent_name", "scopes");
        for i in &identities {
            println!(
                "{:<w_name$}  {:<w_scopes$}  {}",
                i.agent_name,
                i.scope_list(),
                if i.nodes.is_empty() {
                    "-".to_string()
                } else {
                    i.nodes.join(",")
                }
            );
        }
        // There is nothing to reveal: an identity carries no secret, so
        // `--reveal` would have nothing to print. Sprint 049 ruled
        // there is nothing to verify either, and sprint 052 did not
        // reopen that — the token path's `--verify` went with it.
        Ok(())
    }

    fn identity_add(
        &mut self,
        name: &str,
        scopes: &[Scope],
        label: Option<&str>,
        nodes: &[String],
    ) -> Result<()> {
        let before = self.snapshot()?;
        if self.doc.identities()?.iter().any(|i| i.agent_name == name) {
            bail!(
                "an identity named `{name}` already exists — edit it with \
                 `klams-token identity scopes {name} …` rather than adding a second row \
                 (duplicate agent_names are refused at startup)"
            );
        }

        let nodes: Vec<String> = nodes
            .iter()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        let id = IdentityConfig {
            agent_name: name.to_string(),
            scopes: scopes.to_vec(),
            label: Some(label.unwrap_or(name).to_string()),
            nodes: nodes.clone(),
        };
        id.validate()
            .context("the identity you asked for is not one klams-service would accept")?;

        self.doc.add_identity(&id)?;
        self.commit(&before, &Change::Added(GrantFingerprint::identity(name)))?;

        self.report(
            &serde_json::json!({
                "action": "identity-add",
                "identity": name,
                "scopes": scopes.iter().map(|s| s.as_str()).collect::<Vec<_>>(),
                "nodes": nodes,
                "dry_run": self.dry_run,
            }),
            &format!(
                "{} identity `{name}` [{}]{}",
                if self.dry_run { "would add" } else { "added" },
                render_scopes(scopes),
                if nodes.is_empty() {
                    String::new()
                } else {
                    format!(" pinned to {}", nodes.join(","))
                }
            ),
        )
    }

    fn identity_remove(&mut self, selector: &str, yes: bool) -> Result<()> {
        let target = self.doc.find_identity(selector)?;
        let before = self.snapshot()?;

        if !yes && !self.dry_run && !self.confirm_identity_removal(&target)? {
            println!("aborted; nothing was written.");
            return Ok(());
        }

        self.doc.remove_identity(target.index)?;
        self.commit(&before, &Change::Removed(target.fingerprint()))?;

        self.report(
            &serde_json::json!({
                "action": "identity-remove",
                "identity": target.identity(),
                "dry_run": self.dry_run,
            }),
            &format!(
                "{} identity `{}`",
                if self.dry_run {
                    "would remove"
                } else {
                    "removed"
                },
                target.identity()
            ),
        )
    }

    fn confirm_identity_removal(&self, target: &IdentityView) -> Result<bool> {
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
            "remove identity `{}` (label {}, scopes {})? [y/N] ",
            target.identity(),
            target.label.as_deref().unwrap_or("-"),
            target.scope_list(),
        );
        std::io::stdout().flush()?;
        let mut answer = String::new();
        std::io::stdin().read_line(&mut answer)?;
        Ok(matches!(answer.trim(), "y" | "Y" | "yes"))
    }

    fn identity_scopes(
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

        let target = self.doc.find_identity(selector)?;
        let before = self.snapshot()?;
        let next = next_scopes(&target.scopes, set, to_add, to_remove);

        if next == target.scopes {
            return self.report(
                &serde_json::json!({
                    "action": "identity-scopes",
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

        self.doc.set_identity_scopes(target.index, &next)?;
        // Scopes are not part of a fingerprint, so BOTH sets must come
        // out identical: no sibling identity touched, no token grant
        // disturbed.
        self.commit(&before, &Change::None)?;

        self.report(
            &serde_json::json!({
                "action": "identity-scopes",
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

    fn identity_nodes(&mut self, selector: &str, set: &[String]) -> Result<()> {
        // `--set ""` is the natural way to type "unpin", and clap hands
        // it over as one empty string. Taken literally that writes
        // `nodes = [""]` — a pin to a node that cannot exist, which
        // enforcement would then refuse every request against.
        let set: Vec<String> = set
            .iter()
            .map(|n| n.trim().to_string())
            .filter(|n| !n.is_empty())
            .collect();
        let set = set.as_slice();

        let target = self.doc.find_identity(selector)?;
        let before = self.snapshot()?;

        if set == target.nodes.as_slice() {
            return self.report(
                &serde_json::json!({
                    "action": "identity-nodes",
                    "identity": target.identity(),
                    "changed": false,
                    "nodes": set,
                }),
                &format!(
                    "`{}` is already pinned to {} — nothing to write",
                    target.identity(),
                    if set.is_empty() {
                        "no nodes".to_string()
                    } else {
                        set.join(",")
                    }
                ),
            );
        }

        self.doc.set_identity_nodes(target.index, set)?;
        self.commit(&before, &Change::None)?;

        self.report(
            &serde_json::json!({
                "action": "identity-nodes",
                "identity": target.identity(),
                "changed": true,
                "from": target.nodes,
                "to": set,
                "dry_run": self.dry_run,
            }),
            &format!(
                "`{}` nodes {} -> {}",
                target.identity(),
                if target.nodes.is_empty() {
                    "-".to_string()
                } else {
                    target.nodes.join(",")
                },
                if set.is_empty() {
                    "- (unpinned)".to_string()
                } else {
                    set.join(",")
                }
            ),
        )
    }

    // ---------------------------------------------------------- commit

    /// Snapshot the identity table's fingerprints.
    fn snapshot(&self) -> Result<Snapshot> {
        Ok(Snapshot {
            identities: self.doc.identity_fingerprints()?,
        })
    }

    /// The write pipeline every mutation goes through.
    fn commit(&mut self, before: &Snapshot, identities_change: &Change) -> Result<()> {
        // 1. Fingerprint-and-refuse: nothing but the declared change.
        let after = self.snapshot()?;
        verify_delta(&before.identities, &after.identities, identities_change)?;
        let after = after.identities;

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
                "dry run: {} would be rewritten ({} identities, delta verified, result validates)",
                self.path.display(),
                after.len()
            );
            return Ok(());
        }

        // 3. Backup, write through the existing inode, re-read what
        //    actually landed, and roll back if it does not validate.
        // Sprint 050: the backup is a plain timestamped copy. It used
        // to be age-encrypted with a fingerprint manifest beside it,
        // because the file held every live bearer token; an
        // identities-only config holds names and scopes, so there is
        // nothing left to encrypt.
        let written = writer::write_validated(
            &self.path,
            &new_text,
            time::OffsetDateTime::now_utc(),
            writer::DEFAULT_RETAIN,
            |landed| {
                let errors = GrantsDoc::parse(landed)?.auth()?.errors();
                if errors.is_empty() {
                    Ok(())
                } else {
                    bail!("{}", errors.join("; "))
                }
            },
        )?;

        self.notes
            .push(format!("backup: {}", written.backup.display()));
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
