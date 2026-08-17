//! End-to-end tests: drive the real binary against a scratch config.
//!
//! These are the tests that would have caught korg #264 — the incident
//! where a hand-edit of `/etc/klams/klams.toml` clobbered a sibling
//! grant — and the k-homelab S4 finding that a grant can sit dead at
//! 401 with nothing able to notice.

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
[[auth.tokens]]
token      = "klams-view-000000000000000000000000"
scopes     = ["read"]
label      = "klams-view"
agent_name = "klams-view"

# The scanner writes its own chunks and nothing else.
[[auth.tokens]]
token      = "scanner-111111111111111111111111"
scopes     = ["write"]
label      = "scanner"
agent_name = "klams-scanner"

[[auth.tokens]]
token      = "ansible-222222222222222222222222"
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

#[test]
fn list_never_prints_a_token_value() {
    let f = Fixture::new();
    let out = f.run(&["list"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("klams-view"));
    assert!(text.contains("klams-scanner"));
    assert!(
        !text.contains("000000000000000000000000"),
        "a token value leaked into `list`:\n{text}"
    );
}

#[test]
fn list_reveal_prints_token_values_when_asked() {
    let f = Fixture::new();
    let out = f.run(&["list", "--reveal"]);
    assert!(stdout(&out).contains("klams-view-000000000000000000000000"));
}

#[test]
fn list_json_carries_fingerprints_not_tokens() {
    let f = Fixture::new();
    let rows = json(&f.run(&["list", "--json"]));
    let rows = rows.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0]["identity"], "klams-view");
    assert_eq!(rows[0]["scopes"], serde_json::json!(["read"]));
    assert_eq!(rows[0]["token_fingerprint"].as_str().unwrap().len(), 12);
    assert!(rows[0].get("token").is_none());
}

// ------------------------------------------------------------- writing

#[test]
fn add_appends_a_grant_and_leaves_the_rest_byte_identical() {
    let f = Fixture::new();
    let before = f.text();
    let out = f.run(&["add", "krot", "--scopes", "read,write", "--reveal"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let after = f.text();
    // Every original line survives, in order, with its comments.
    for line in before.lines() {
        assert!(
            after.contains(line),
            "`add` disturbed an existing line: {line:?}"
        );
    }
    assert!(after.contains(r#"agent_name = "krot""#));
    assert!(stdout(&out).contains("token: krot-"));
    assert_eq!(f.backups().len(), 1);
    assert!(stderr(&out).contains("systemctl reload klams-service"));
}

#[test]
fn add_refuses_a_duplicate_identity() {
    let f = Fixture::new();
    let before = f.text();
    let out = f.run(&["add", "klams-view", "--scopes", "read"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("already exists"), "{}", stderr(&out));
    assert_eq!(f.text(), before, "a refused add still wrote");
    assert!(f.backups().is_empty(), "a refused add still took a backup");
}

#[test]
fn add_refuses_a_grant_klams_service_would_not_accept() {
    let f = Fixture::new();
    // #703: manage/admin require an agent_name — and every grant this
    // tool writes has one, so the way to trip the rule is a scope set
    // the service rejects for another reason. An empty one is rejected
    // by clap; an unknown scope name never reaches the file.
    let out = f.run(&["add", "krot", "--scopes", "superuser"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("unknown scope"), "{}", stderr(&out));
    assert_eq!(f.text(), FIXTURE);
}

#[test]
fn remove_deletes_exactly_one_grant() {
    let f = Fixture::new();
    let out = f.run(&["remove", "ansible-k", "--yes"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let after = f.text();
    assert!(!after.contains("ansible-222222222222222222222222"));
    assert!(after.contains("klams-view-000000000000000000000000"));
    assert!(after.contains("scanner-111111111111111111111111"));
    // The comments belonging to the survivors are still there.
    assert!(after.contains("# The dashboard only reads."));
    assert!(after.contains("# The scanner writes its own chunks and nothing else."));
    assert!(after.contains("[postgres]"));
}

#[test]
fn remove_without_yes_refuses_rather_than_prompting_a_pipe() {
    let f = Fixture::new();
    let out = f.run(&["remove", "ansible-k"]);
    assert!(!out.status.success());
    assert!(stderr(&out).contains("--yes"), "{}", stderr(&out));
    assert_eq!(f.text(), FIXTURE);
}

#[test]
fn scopes_changes_one_grant_and_no_token() {
    let f = Fixture::new();
    let out = f.run(&["scopes", "klams-scanner", "--add", "read"]);
    assert!(out.status.success(), "{}", stderr(&out));

    let after = f.text();
    assert!(after.contains(r#"scopes     = ["read", "write"]"#));
    // Every token value in the file is untouched.
    for token in [
        "klams-view-000000000000000000000000",
        "scanner-111111111111111111111111",
        "ansible-222222222222222222222222",
    ] {
        assert!(after.contains(token), "token {token} was disturbed");
    }
    // And the dashboard grant still reads only.
    let rows = json(&f.run(&["list", "--json"]));
    assert_eq!(rows[0]["scopes"], serde_json::json!(["read"]));
    assert_eq!(rows[1]["scopes"], serde_json::json!(["read", "write"]));
}

#[test]
fn scopes_that_would_empty_the_set_is_refused_before_any_write() {
    let f = Fixture::new();
    let out = f.run(&["scopes", "klams-scanner", "--remove", "write"]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("would not start klams-service"), "{err}");
    assert!(err.contains("at least one scope"), "{err}");
    assert_eq!(f.text(), FIXTURE, "a refused edit still wrote");
    assert!(f.backups().is_empty());
}

#[test]
fn scopes_is_a_noop_when_nothing_would_change() {
    let f = Fixture::new();
    let out = f.run(&["scopes", "klams-view", "--add", "read"]);
    assert!(out.status.success(), "{}", stderr(&out));
    assert!(stdout(&out).contains("already has scopes read"));
    assert_eq!(f.text(), FIXTURE);
    assert!(f.backups().is_empty(), "a no-op took a backup");
}

// ------------------------------------------------------------ rotation

/// The property P0.1 flagged: klams keys a memory's author on
/// `agent_name`, **not** on the token value. If rotation moved the
/// identity, every memory that agent ever wrote would be orphaned from
/// the credential that wrote it.
#[test]
fn rotate_changes_the_token_and_nothing_that_identifies_the_agent() {
    let f = Fixture::new();
    let before = json(&f.run(&["list", "--json"]));
    let before = before.as_array().unwrap().clone();

    let out = f.run(&["rotate", "klams-scanner", "--json", "--reveal"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let rotated = json(&out);
    assert_ne!(rotated["old_fingerprint"], rotated["new_fingerprint"]);

    let after = json(&f.run(&["list", "--json"]));
    let after = after.as_array().unwrap();

    // The rotated grant keeps everything that identifies it.
    assert_eq!(after[1]["agent_name"], before[1]["agent_name"]);
    assert_eq!(after[1]["identity"], "klams-scanner");
    assert_eq!(after[1]["label"], before[1]["label"]);
    assert_eq!(after[1]["scopes"], before[1]["scopes"]);
    assert_ne!(
        after[1]["token_fingerprint"],
        before[1]["token_fingerprint"]
    );

    // And its neighbours did not move at all — the whole point of
    // fingerprinting the set rather than just the target.
    for i in [0, 2] {
        assert_eq!(
            after[i], before[i],
            "rotating one grant disturbed grant {i}"
        );
    }
    // Belt and braces: the file itself still holds the other tokens
    // verbatim, so "unchanged fingerprint" is not the tool agreeing
    // with itself about a value it also rewrote.
    let text = f.text();
    assert!(text.contains("klams-view-000000000000000000000000"));
    assert!(text.contains("ansible-222222222222222222222222"));
    assert!(!text.contains("scanner-111111111111111111111111"));
}

#[test]
fn rotate_keeps_the_token_prefix_so_a_leaked_value_is_still_traceable() {
    let f = Fixture::new();
    let out = f.run(&["rotate", "klams-scanner", "--json", "--reveal"]);
    let token = json(&out)["token"].as_str().unwrap().to_string();
    assert!(token.starts_with("scanner-"), "{token}");
    assert_eq!(token.len(), "scanner-".len() + 64);
}

// ------------------------------------------------------------- dry run

#[test]
fn dry_run_validates_everything_and_writes_nothing() {
    let f = Fixture::new();
    let out = f.run(&["--dry-run", "remove", "ansible-k"]);
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
    let out = stdout(&f.run(&["--dry-run", "remove", "ansible-k"]));
    assert!(out.contains("would remove"), "{out}");
    assert!(!out.contains("removed grant"), "{out}");
}

/// `--dry-run add --reveal` generates a token and throws it away.
/// Printing it would hand the operator a credential that exists
/// nowhere — in the file, in a backup, or in the service.
#[test]
fn dry_run_add_never_prints_a_token_that_was_not_written() {
    let f = Fixture::new();
    let out = f.run(&["--dry-run", "add", "krot", "--scopes", "read", "--reveal"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("would add grant `krot`"), "{text}");
    assert!(!text.contains("token: krot-"), "{text}");
    assert_eq!(f.text(), FIXTURE);

    // Same in JSON: no token, no fingerprint of a value that vanished.
    let row = json(&f.run(&[
        "--dry-run",
        "--json",
        "add",
        "krot",
        "--scopes",
        "read",
        "--reveal",
    ]));
    assert_eq!(row["dry_run"], true);
    assert!(row.get("token").is_none());
    assert!(row.get("token_fingerprint").is_none());
}

#[test]
fn dry_run_rotate_never_prints_a_token_that_was_not_written() {
    let f = Fixture::new();
    let out = f.run(&["--dry-run", "rotate", "klams-scanner", "--reveal"]);
    assert!(out.status.success(), "{}", stderr(&out));
    let text = stdout(&out);
    assert!(text.contains("would rotate"), "{text}");
    assert!(!text.contains("token: scanner-"), "{text}");
    assert_eq!(f.text(), FIXTURE);
}

/// Back-to-back edits are the normal case (an `add` then a `scopes`,
/// or krot rotating several grants in one pass). A same-second backup
/// collision must not fail the second edit.
#[test]
fn consecutive_edits_each_get_their_own_backup() {
    let f = Fixture::new();
    for args in [
        vec!["add", "krot", "--scopes", "read,write"],
        vec!["scopes", "klams-scanner", "--add", "read"],
        vec!["rotate", "klams-view"],
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
    let out = f.run(&["rotate", "typo"]);
    assert!(!out.status.success());
    let err = stderr(&out);
    assert!(err.contains("no grant matches `typo`"), "{err}");
    assert!(err.contains("klams-scanner"), "{err}");
}

#[test]
fn a_missing_config_names_every_path_it_tried() {
    let out = Command::new(BIN)
        .arg("--config")
        .arg("/nonexistent/klams.toml")
        .arg("list")
        .output()
        .unwrap();
    assert!(!out.status.success());
    assert!(stderr(&out).contains("/nonexistent/klams.toml"));
}
