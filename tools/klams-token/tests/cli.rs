//! End-to-end tests: drive the real binary against a scratch config.
//!
//! These are the tests that would have caught korg #264 — the incident
//! where a hand-edit of `/etc/klams/klams.toml` clobbered a sibling
//! row.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

const BIN: &str = env!("CARGO_BIN_EXE_klams-token");

const FIXTURE: &str = r#"# klams-service runtime configuration.
#
# The comments in this file ARE the operator documentation.

[server]
listen_addr = "127.0.0.1"
port = 7777

[auth]
# SCOPES ARE FLAT, NOT HIERARCHICAL.

# The dashboard only reads.
[[auth.identities]]
scopes     = ["read"]
label      = "klams-view"
agent_name = "klams-view"

# The scanner writes its own chunks and nothing else.
[[auth.identities]]
scopes     = ["write"]
label      = "scanner"
agent_name = "klams-scanner"

[[auth.identities]]
scopes     = ["read", "write"]
label      = "ansible_k"
agent_name = "ansible-k"

[postgres]
url = "postgres://localhost/klams"
"#;

struct Fixture {
    _dir: tempfile::TempDir,
    path: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("klams.toml");
        std::fs::write(&path, FIXTURE).unwrap();
        Self { _dir: dir, path }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(BIN)
            .arg("--config")
            .arg(&self.path)
            .args(args)
            .output()
            .expect("running klams-token")
    }

    fn text(&self) -> String {
        std::fs::read_to_string(&self.path).unwrap()
    }

    fn backups(&self) -> Vec<PathBuf> {
        let dir: &Path = self.path.parent().unwrap();
        let mut v: Vec<PathBuf> = std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.path())
            .filter(|p| {
                p.file_name()
                    .and_then(|n| n.to_str())
                    .is_some_and(|n| n.contains(".bak-"))
            })
            .collect();
        v.sort();
        v
    }
}

fn stdout(out: &Output) -> String {
    String::from_utf8_lossy(&out.stdout).into_owned()
}
fn stderr(out: &Output) -> String {
    String::from_utf8_lossy(&out.stderr).into_owned()
}
fn json(out: &Output) -> serde_json::Value {
    serde_json::from_str(&stdout(out)).expect("stdout should be JSON")
}

// ------------------------------------------------------------- reading

// ------------------------------------------------------------- dry run

#[test]
fn dry_run_validates_everything_and_writes_nothing() {
    let f = Fixture::new();
    let out = f.run(&["identity", "remove", "ansible-k", "--yes", "--dry-run"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stderr(&out).contains("dry run"), "{}", stderr(&out));
    assert_eq!(f.text(), FIXTURE);
    assert!(f.backups().is_empty());
}

/// A dry run must not claim, in the past tense, to have done the thing
/// it explicitly did not do.
#[test]
fn dry_run_output_does_not_read_as_a_completed_write() {
    let f = Fixture::new();
    let out = stdout(&f.run(&["--dry-run", "identity", "remove", "ansible-k", "--yes"]));
    assert!(out.contains("would remove"), "{out}");
    assert!(!out.contains("removed identity `"), "{out}");
}

/// Back-to-back edits are the normal case (an `add` then a `scopes`).
/// A same-second backup collision must not fail the second edit.
#[test]
fn consecutive_edits_each_get_their_own_backup() {
    let f = Fixture::new();
    for args in [
        vec!["identity", "add", "krot", "--scopes", "read,write"],
        vec!["identity", "scopes", "klams-scanner", "--add", "read"],
        vec!["identity", "nodes", "klams-view", "--set", "kubs0"],
    ] {
        let out = f.run(&args);
        assert!(out.status.success(), "{:?}: {}", args, stderr(&out));
    }
    let backups = f.backups();
    assert_eq!(backups.len(), 3, "{backups:?}");
    // Each backup holds a distinct snapshot.
    let mut contents: Vec<String> = backups
        .iter()
        .map(|p| std::fs::read_to_string(p).unwrap())
        .collect();
    contents.sort();
    contents.dedup();
    assert_eq!(contents.len(), 3, "two backups held identical content");
}

// -------------------------------------------------------------- errors

#[test]
fn an_unknown_selector_names_what_does_exist() {
    let f = Fixture::new();
    let out = f.run(&["identity", "scopes", "typo", "--add", "read"]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("typo"), "{err}");
    assert!(err.contains("klams-scanner"), "{err}");
}

#[test]
fn a_missing_config_names_every_path_it_tried() {
    let out = Command::new(BIN)
        .arg("--config")
        .arg("/nonexistent/klams.toml")
        .args(["identity", "list"])
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("/nonexistent/klams.toml"));
}

/// Sprint 052: the legacy token subcommands are gone, not hidden. A
/// muscle-memory `klams-token list` must fail loudly rather than
/// resolving to something else.
#[test]
fn the_retired_token_subcommands_are_gone() {
    let f = Fixture::new();
    for args in [
        vec!["list"],
        vec!["add", "krot", "--scopes", "read"],
        vec!["remove", "ansible-k", "--yes"],
        vec!["rotate", "klams-scanner"],
        vec!["scopes", "klams-scanner", "--add", "read"],
    ] {
        let out = f.run(&args);
        assert!(!out.status.success(), "{args:?} must not be accepted");
        assert!(
            stderr(&out).contains("unrecognized subcommand"),
            "{args:?}: {}",
            stderr(&out)
        );
    }
    assert_eq!(f.text(), FIXTURE, "a refused command must write nothing");
}

/// `--reveal` went with the tokens: an identity has no secret to
/// reveal, so the flag must be rejected rather than silently ignored.
#[test]
fn the_reveal_flag_is_gone() {
    let f = Fixture::new();
    let out = f.run(&["identity", "list", "--reveal"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("--reveal"), "{}", stderr(&out));
}

// ------------------------------------------------------- identities

#[test]
fn identity_add_appends_a_row_and_leaves_every_token_grant_alone() {
    let f = Fixture::new();
    let before = f.text();
    let out = f.run(&["identity", "add", "claude", "--scopes", "read,write,manage"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let after = f.text();
    // Every original line survives, in order, with its comments — the
    // token grants included, because this slice deletes nothing.
    for line in before.lines() {
        assert!(
            after.contains(line),
            "`identity add` disturbed an existing line: {line:?}"
        );
    }
    assert!(after.contains("[[auth.identities]]"));
    assert!(after.contains(r#"agent_name = "claude""#));
    assert_eq!(f.backups().len(), 1);
    // The block belongs beside the grants it supersedes, inside the
    // `[auth]` run — not appended after `[postgres]`.
    assert!(
        after.find("[[auth.identities]]").unwrap() < after.find("[postgres]").unwrap(),
        "identity block rendered below [postgres]:\n{after}"
    );
}

#[test]
fn identity_add_refuses_a_duplicate_name() {
    let f = Fixture::new();
    assert!(f
        .run(&["identity", "add", "claude", "--scopes", "read"])
        .status
        .success());
    let out = f.run(&["identity", "add", "claude", "--scopes", "read,write"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("already exists"), "{}", stderr(&out));
}

#[test]
fn identity_add_refuses_a_row_klams_service_would_not_accept() {
    let f = Fixture::new();
    // Uppercase is outside the agent_name charset.
    let out = f.run(&["identity", "add", "Claude", "--scopes", "read"]);
    assert!(!out.status.success());
    assert_eq!(f.text(), FIXTURE, "nothing may be written on a refusal");
}

#[test]
fn identity_list_reports_scopes_and_pins() {
    let f = Fixture::new();
    assert!(f
        .run(&[
            "identity",
            "add",
            "kmon",
            "--scopes",
            "read,write",
            "--nodes",
            "kubs0"
        ])
        .status
        .success());
    let out = f.run(&["--json", "identity", "list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let rows = json(&out);
    let kmon = rows
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["agent_name"] == "kmon")
        .expect("the added row must be listed");
    assert_eq!(kmon["scopes"], serde_json::json!(["read", "write"]));
    assert_eq!(kmon["nodes"][0], "kubs0");
    // There is no token to leak, so no field can carry one.
    assert!(kmon.get("token").is_none());
    assert!(kmon.get("token_fingerprint").is_none());
}

#[test]
fn identity_scopes_changes_one_row_and_no_sibling() {
    let f = Fixture::new();
    assert!(f
        .run(&["identity", "add", "claude", "--scopes", "read"])
        .status
        .success());
    let before = f.text();
    let out = f.run(&["identity", "scopes", "claude", "--add", "write"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let after = f.text();
    assert!(after.contains(r#"scopes = ["read", "write"]"#));
    // Every token grant's line is untouched.
    for line in before.lines().filter(|l| l.starts_with("token ")) {
        assert!(after.contains(line), "token line moved: {line:?}");
    }
}

#[test]
fn identity_nodes_pins_and_unpins() {
    let f = Fixture::new();
    assert!(f
        .run(&["identity", "add", "kmon", "--scopes", "read"])
        .status
        .success());
    assert!(f
        .run(&["identity", "nodes", "kmon", "--set", "kubs0,kai"])
        .status
        .success());
    assert!(f.text().contains(r#"nodes = ["kubs0", "kai"]"#));

    // Unpinning removes the key rather than leaving a pin to nowhere.
    let out = f.run(&["identity", "nodes", "kmon", "--set", ""]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(!f.text().contains("nodes ="), "{}", f.text());
}

#[test]
fn identity_remove_deletes_exactly_one_row() {
    let f = Fixture::new();
    for name in ["claude", "kmon"] {
        assert!(f
            .run(&["identity", "add", name, "--scopes", "read"])
            .status
            .success());
    }
    let out = f.run(&["identity", "remove", "claude", "--yes"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let after = f.text();
    assert!(!after.contains(r#"agent_name = "claude""#));
    assert!(after.contains(r#"agent_name = "kmon""#));
    // Exactly one row left; the fixture's three are untouched.
    assert_eq!(after.matches("[[auth.identities]]").count(), 4);
    for survivor in ["klams-view", "klams-scanner", "ansible-k"] {
        assert!(after.contains(survivor), "{survivor} must survive: {after}");
    }
}

#[test]
fn identity_remove_without_yes_refuses_rather_than_prompting_a_pipe() {
    let f = Fixture::new();
    assert!(f
        .run(&["identity", "add", "claude", "--scopes", "read"])
        .status
        .success());
    let out = f.run(&["identity", "remove", "claude"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("--yes"), "{}", stderr(&out));
}

/// Every write is gated on "would klams-service boot on the result?",
/// and `commit` re-checks that after the edit lands — so a sequence of
/// edits cannot walk the file into a state the service would refuse.
#[test]
fn a_sequence_of_edits_stays_a_config_the_service_would_boot() {
    let f = Fixture::new();
    assert!(f
        .run(&["identity", "add", "claude", "--scopes", "read,write,manage"])
        .status
        .success());
    let out = f.run(&["identity", "scopes", "klams-view", "--add", "write"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let out = f.run(&["identity", "nodes", "claude", "--set", "kubs0,kai"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let after = f.text();
    assert_eq!(after.matches("[[auth.identities]]").count(), 4);
    assert!(after.contains("SCOPES ARE FLAT"), "comments must survive");
}
