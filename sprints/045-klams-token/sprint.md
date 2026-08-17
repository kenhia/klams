# Sprint 045 — klams-token, the auth-grant CLI

**korg:** proposal 1369 (program 1374, "krot rollout — idea to working
rotation infrastructure") · work item #265 · task / M
**Branch:** `045-klams-token` · **Version:** 0.1.45

## Goal

Build `klams-token`: a small purpose-built CLI that edits the
`[[auth.tokens]]` grants in `/etc/klams/klams.toml` **structurally**,
so a hand-edit can never again clobber a sibling grant (korg #264, the
incident this exists to prevent), and so a dead grant can be noticed
before something depends on it (k-homelab S4's finding).

The tool lives in this repo — not k-homelab — because it must parse and
write the exact TOML shape `klams-service` defines, and reusing
`klams_types::TokenGrantConfig` is what keeps its schema understanding
from drifting out of sync with the service. A tool that drifts from the
schema it exists to protect is the bug it is supposed to prevent.
(Ownership decided in the #265 thread, moved k-homelab → klams
2026-08-02.)

## Scope

A new workspace member `tools/klams-token` producing a `klams-token`
binary, with:

**Subcommands**

| Command | Behaviour |
|---|---|
| `list` | grants as label / agent_name / scopes / token **fingerprint**. `--reveal` to print token values, never by default. `--verify` to probe each grant against the live service. `--json` for programmatic use. |
| `add` | append a new `[[auth.tokens]]` block. Token generated as `<short-name>-<32 hex bytes>`, matching the convention already in the file. Never edits an existing block. |
| `remove` | delete one grant by label or agent_name, confirmation prompt (`--yes` to skip). |
| `scopes` | change an existing grant's `scopes` array (`--set` / `--add` / `--remove`) without touching its token or any other field. |
| `rotate` | replace **only** the `token` value of one grant, preserving `agent_name`, `label` and `scopes`. Added beyond the WI's original list for krot #943, so its `klams-grant-rotate` procedure calls one command instead of a guarded manual recipe. |

**Every write** goes through one pipeline, in this order:

1. parse with `toml_edit` (format-preserving — the live file is heavily
   commented and a serde round-trip would flatten it and materialize
   every `#[serde(default)]`);
2. **fingerprint before**: `{agent_name → sha256(token)[:12]}` over the
   whole grant set;
3. apply the structural edit;
4. **fingerprint after and refuse to write** unless the delta is
   exactly the intended change (k-homelab S4's pattern — this, not the
   structural editing, is what makes a clobber impossible rather than
   merely unlikely);
5. validate the *resulting document* by deserializing each grant into
   `klams_types::TokenGrantConfig` and running its `validate()`, plus
   the config-level rules `--validate-config` enforces (no retired
   `bearer_token`, at least one grant);
6. timestamped backup, then write **through the existing inode**
   (truncate in place, not rename) so `root:klams 0640` survives;
7. re-read and re-validate; restore from the backup and report on any
   failure — never leave a broken config live.

`--dry-run` stops after step 5 and prints the diff.

**`--verify`** (the strongest requirement, per S4): one authenticated
request per grant against the live service. `401` = dead, `403` =
live but scope-limited, `2xx` = live. The distinction matters — a
write-only grant is healthy and must not be reported dead.

**Backup naming**: settle the one convention this file has been missing
— `klams.toml.bak-<YYYYMMDD>T<HHMMSS>Z`. The tool retains its own most
recent N and never touches backups it did not write (the file already
has five in three conventions; we add no sixth convention and delete no
one else's).

**Never print a token value without `--reveal`.** Fingerprints
everywhere else, including `--json`.

## Acceptance criteria

- `just gate` green; TDD throughout.
- A test proving **rotation does not orphan an agent's memories**:
  klams identity keys on `agent_name`, not token value (P0.1's
  finding), so `rotate` must leave `agent_name` byte-identical.
- A test proving a write that would alter a sibling grant is
  **refused**, not merely unlikely.
- A test proving comments and formatting survive a write.
- A test proving a validation failure restores the original file.
- `--verify` distinguishes 401 from 403 (wiremock).
- `docs/usage.md` gains the operator recipes; `docs/auth.md` points at
  the tool as the way to edit grants.

## Not in scope

- The retired top-level `bearer_token` / `[auth]` migration (#703
  already refuses it at startup; this tool only reports it).
- Restarting the service. Print the reminder — and prefer
  `systemctl reload`, since SIGHUP hot-reloads `[[auth.tokens]]` as of
  sprint 018.
- Wiring into krot's mint step — that is krot #943, a separate repo.

## Log

### `AuthConfig` moved down to `klams-types`

The proposal's anti-drift requirement — "reuse `klams_types`/config
types so the schema can't drift" — was only half satisfiable as
written. `TokenGrantConfig` (the grant shape, and where the real drift
risk lives) was already in `klams-types`, but the `[auth]` block that
contains it, `AuthConfig`, was in `klams-service::config`, and a small
CLI has no business depending on the whole service crate to reach two
fields.

So `AuthConfig` moved down to `klams-types::auth`, with
`klams_service::config::AuthConfig` left as a re-export. Three call
sites, no behaviour change.

The more useful half of that move: the *rules* moved with it.
`--validate-config`'s `[auth]` checks were an inline list in
`klams-service/src/main.rs`; they are now `AuthConfig::errors()` /
`::warnings()`, and both the service and `klams-token` call them. There
is one definition of "would this `[auth]` block start the service", not
a copy in the tool that can rot.

### `toml_edit`, not a serde round-trip

New workspace dependency. The obvious reason is that the live
`klams.toml` is heavily commented and those comments are the operator
documentation for the auth model. The less obvious one, and the one
that would have bitten quietly: a serde round-trip through `Config`
also *materializes* every `#[serde(default)]` in the service's config
tree, so the file would silently grow a frozen copy of today's defaults
and stop tracking them.

One wrinkle worth recording: `toml_edit` renders tables in `position`
order and puts unpositioned ones last, so a plain `ArrayOfTables::push`
drops a new `[[auth.tokens]]` block *below* `[postgres]` at the end of
the file. Valid TOML that reads exactly like the tool mangled the file.
`GrantsDoc::add` therefore computes an insert position after the last
existing grant and shifts everything at or after it down one.

### Fingerprints cover identity and token, deliberately not scopes

That is what makes the `scopes` subcommand's guarantee expressible:
its declared change is `Change::None`, meaning *the entire grant set
must come out identical*. A scopes edit that damaged a token or dropped
a sibling is caught by the same machinery that guards `add`/`remove`,
without scopes needing a special case.

The identity key is `agent_name` first — not cosmetic. klams attributes
memories by `agent_name`, so a "rotation" that moved the identity would
orphan everything that agent ever wrote (P0.1's finding). The delta
check refuses it, and there is a test named for it.

### Two defects the smoke test found, that the unit tests did not

Both came from running the binary against a realistic scratch config
rather than from a fixture:

1. **Same-second backup collision.** `add` then `scopes` inside one
   second produced the same backup name, and the second edit failed
   outright — with the correct-but-useless message "refusing to
   overwrite an existing backup". Back-to-back edits are the normal
   case, and krot's rotation procedure will do several in a row.
   Collisions now take `…Z-1`, `…Z-2`; the suffix sorts after the bare
   name so chronological order survives.

2. **Dry runs lied in two ways.** `--dry-run remove` printed "removed
   grant `krot`" on stdout while stderr said "would be rewritten", and
   `--dry-run add --reveal` printed a freshly generated token that was
   then discarded — a credential existing in the operator's scrollback
   and nowhere else. Dry runs now report in the conditional and
   suppress token values entirely.

### `--verify` classification

401 is dead; **403 is healthy**. A grant scoped `["write"]` cannot call
the read probe route, and reporting it dead would train the operator to
ignore the column — which is precisely the failure mode S4 was
describing. An unreachable service reports `unreachable` and exits 0:
that is an operator problem, not a grant problem. Only a confirmed 401
sets the exit code (2, distinct from 1 so a monitor can tell a broken
credential from a broken config).

The probe route is `GET /memory/policy`: authenticated, `read`-scoped,
touches no backend.

### Where it lives, and what shipped

`tools/klams-token`, alongside `bench`/`soak`/`reattribute-system` —
operator tooling that runs where the config is, not part of the service
deploy. `just install-klams-token` puts it on the host's PATH;
`just tokens-verify` runs the liveness probe from a checkout.

Docs: `docs/usage.md` (the sprint-045 section — pipeline, recipes,
exit codes), `docs/auth.md` (use the tool, not an editor),
`docs/setup.md` (why the in-place write matters for `root:klams 0640`,
and grant management after provisioning), `docs/architecture.md` (crate
table), `README.md`, and `deploy/config/klams.example.toml` itself.

Gate green; the docker integration suite green and torn down.

## Follow-ups

- **Publishing.** `klams-token` is built from a checkout today. If krot
  #943 wants it present on a host without one, it needs adding to the
  store publish + `deploy/install-from-store.sh` — a deliberate
  decision, not an oversight, since `tools/` is documented as
  non-shipping.
- **The five legacy backups** in `/etc/klams` (three conventions) are
  still there. The tool refuses to prune what it did not write, so
  retiring them is a one-time manual call.
- `--verify` probes with `read`; a grant with no `read` scope can only
  ever be distinguished as "authenticated" (403). That is the correct
  answer, but a dedicated unauthenticated-identity echo route would let
  it report scopes as the service sees them. Not worth a route today.
