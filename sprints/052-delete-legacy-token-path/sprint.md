# Sprint 052 — delete the legacy `[[auth.tokens]]` code path

Proposal **korg:2503**, slice 7.6 of program korg:2440 (simplify
homelab secrets). Covers **WI 2489**. Karc leg `klams-fcf92b` on kubs0,
overseen.

## Goal

Sprint 050 deleted every `[[auth.tokens]]` *row* — that is what closed
the transition window (049 D-1: the rows are the flag). This sprint
deletes the *code* that parsed and matched them, so klams authenticates
by declared identity and nothing else.

## Why now, and not in 050

The window had to be **observed** closed from a restarted session on
every host, not merely believed closed: a session that was already
running when the rows went keeps the config it loaded at start. WI
2489's observation list completed at 2026-09-12 19:05 PDT — Claude Code
and Copilot on kai, kubs0 and cleo each authenticate on the header their
own file stores (200) with an unknown-name control (401), and a karc leg
on each of kubs0 and kai came up on the cut-over toolbox. No client file
on any host carries a credential-shaped header for klams.

## Scope — what goes

- `resolve_bearer` and its constant-time loop (`klams-api/src/auth.rs`)
- `TokenGrant`, `AuthTables.tokens`, `legacy_window_open`,
  `AuthState::new`/`with_grants`/`replace_grants`/`grants_for_test`
- `TokenGrantConfig`, `AuthConfig.tokens`, `AuthConfig.bearer_token`
  and the sprint-034 `LegacyBearerTokenRetired` refusal
- `klams-token`'s legacy token subcommands (`list`, `add`, `remove`,
  `scopes`, `rotate`) and `--reveal`
- `[[auth.tokens]]` remnants in `deploy/config/klams.example.toml` and
  `docs/auth.md`

## Scope — what stays, deliberately

Borrowed from kaed 024 (korg:2533), whose slice is the same shape one
repo over:

- **D-1 — a retired field is a named refusal, not a serde error.**
  `AuthConfig` is not `deny_unknown_fields`, so deleting the fields
  would make a surviving `[[auth.tokens]]` row *silently ignored* —
  the operator believes a credential is live and it is not. klams
  scans the raw config text before parsing and refuses, naming the
  field, the sprint and the fix. It **strips comments** first (this
  repo's own example config carries prose about the cutover) and
  matches longest-first (`bearer_token` is a substring of nothing here,
  but `auth.tokens` vs `tokens` is the same trap kaed hit with
  `prev_token_file`).
- **D-2 — `Authorization` is still read, for the 401 diagnostic only.**
  It authenticates nothing. A request carrying a bearer and no
  `X-Homelab-Agent` gets a 401 whose body names the header to send and
  the one to drop. WI 2490's day of silent 401s across three clients is
  the justification: from outside, "sent a retired credential" and
  "sent nothing" were indistinguishable.

## Acceptance

1. No token can authenticate: `Authorization: Bearer <anything>` is 401
   with a diagnostic body naming both headers.
2. A config carrying `[[auth.tokens]]` or `bearer_token` refuses to
   start, naming the field and the fix; a config mentioning either only
   in a comment starts fine.
3. `just gate` green; the docker-compose integration suite green.
4. `--validate-config` against the deployed `/etc/klams/klams.toml`
   passes on the new binary **before** publish.
5. Deployed as 0.1.52 in the ship turn; `just verify` after.

## Rules that travel

- Never read `/etc/klams/klams.toml` directly (krot WI 2466);
  `sudo klams-token identity list` only.
- This leg's own ship turn is a klams client and must come up on the
  header — it does; the toolbox was cut over in 050.

## Chronicle

(written as the work happens)

### Premise check (start-sprint Step 5)

Every claim on WI 2489 held, checked against the code and the live
service on kubs0:

| claim | verdict |
|---|---|
| `resolve_bearer` + constant-time loop in `klams-api/src/auth.rs` | **holds** — present, ~40 lines |
| `TokenGrant`, `AuthTables.tokens` | **holds** |
| `klams-token` legacy subcommands + `--reveal` | **holds** — `list`/`add`/`remove`/`scopes`/`rotate` |
| sprint-034 `bearer_token` migration refusal | **holds** |
| `[[auth.tokens]]` remnants in example config and `docs/auth.md` | **holds** |
| live config holds no token row | **holds** — `sudo klams-token list` returns zero rows; 12 identities |

The observation gate (WI 2489's thread) was complete before the leg
started; nothing re-derived here.

## Decisions

**D-1 — a retired field is a named refusal, not a serde error.**
`AuthConfig` is **not** `deny_unknown_fields`, so deleting the fields
alone would make a surviving `[[auth.tokens]]` row *silently ignored*:
the operator keeps believing a credential is live while it authenticates
nothing. `Config::from_path` now scans the raw text before serde sees it
(`klams_types::retired_fields`) and refuses, naming the field, the
sprint and the fix. Two properties are load-bearing, both from kaed
024 D-1:

- **Comments are stripped first** — this repo's own example config
  documents both retired forms in prose, and several live configs do
  too. A guard that refused to start over a comment would fail exactly
  the operators it protects. `strip_toml_comments` is quote-aware, so a
  `#` inside the Postgres URL does not truncate the line and hide a
  retired key after it.
- **Longest match first, and each match is consumed**, so the report
  describes the file rather than the pattern list. Pinned by a test on
  `RETIRED`'s ordering so it cannot rot.

**D-2 — `Authorization` is still read, for the 401 diagnostic only.**
It authenticates nothing; `had_bearer` is a `bool` that never reaches a
lookup. A request with a bearer and no `X-Homelab-Agent` gets
`bearer_retired`, whose body names the header to send and the one to
drop. Sending nothing at all still gets the plain `unauthorized`, so the
two stay distinguishable — which is the entire point. WI 2490 is the
justification: three clients sat on a bare 401 for a day because, from
outside, "sent a retired credential" and "sent nothing" were identical.

**D-3 — `klams-token identity` stays a subcommand group.** It is now the
only one, so flattening it to `klams-token list` was available and
rejected: `sudo klams-token identity list` is the documented way to read
the roster without opening the file (krot WI 2466), and it is in
operator muscle memory. A test pins that the five retired token
subcommands fail as *unrecognised* rather than resolving to something
else, and that a refused command writes nothing.

## Verification

- `just gate` — green. 121 suites.
- Integration stack (docker compose) — see below.
- **Deploy precondition, checked from kubs0 (the host that will do the
  deploy), on the release binary:** `--validate-config` against the
  live `/etc/klams/klams.toml` → `OK: [auth] identities=12`, rc 0. The
  file was never read directly (krot WI 2466).
- **Both controls, on the shipped binary**, because the guard is only
  meaningful if it distinguishes them:
  - the shipped example config + one identity row → **rc 0**, starts.
    That file documents `[[auth.tokens]]` and `bearer_token` in
    comments, so this is the live proof of comment-stripping.
  - the same file plus one real `[[auth.tokens]]` row → **rc 2**,
    refusing with the field, the sprint and the fix.

## What was deleted

`resolve_bearer` and its constant-time loop; `TokenGrant` and its two
constructors; `AuthTables.tokens` and `legacy_window_open`;
`AuthState::{new, with_grants, replace_grants, grants_for_test}`;
`AuthMethod` (the `auth=identity` log field is now a literal, so the
journal line is unchanged); `AuthenticatedPeer.method`;
`TokenGrantConfig`; `AuthConfig.{bearer_token, tokens}`;
`AuthConfigError::{TokenTooShort, PrivilegedGrantNeedsAgentName,
LegacyBearerTokenRetired}`; `klams-token`'s `list`/`add`/`remove`/
`scopes`/`rotate` and `--reveal`, its whole `verify` module, its token
generator, and with them the `rand`, `reqwest`, `tokio` and `wiremock`
dependencies — the CLI is now synchronous. Net **−1669 lines**.

## Repaired in passing

- **A live regression this deletion caused, caught by its own test.**
  `next_position_in` anchored the first `[[auth.identities]]` block on
  the *sibling* `[[auth.tokens]]` array. Deleting that array took the
  anchor with it, so `identity add` against a config with no identities
  rendered the block below `[postgres]` — valid TOML that reads exactly
  like a mangled file. Re-anchored on the `[auth]` table itself. This is
  the second time the same failure has been measured (sprint 049 hit it
  first); the test now names both.
- **`docs/install.md` claimed the provision script renders three
  `[[auth.tokens]]` grants.** Sprint 050 converted it to identities and
  left the doc behind — which would have told a new operator to expect a
  config this sprint's binary refuses to start on. Corrected.
- **`docs/usage.md` documented `klams-token restore` and age-encrypted
  backups.** Sprint 050 retired both; the subcommand does not exist.
  Corrected to the plain-timestamped-copy behaviour the writer actually
  has.
- **`just tokens-verify` pointed at a deleted subcommand.** Renamed to
  `just identities`, reading the roster instead of probing it, with the
  reason for the change recorded in the recipe.
- **Two more callers of the deleted `klams-token list --verify`**, found
  by sweeping for dangling references rather than by a compiler error,
  since neither is Rust: `deploy/install-from-store.sh` printed it as
  the post-install confirmation step, and `install-klams-token`'s
  success message suggested it. Both now say
  `sudo klams-token identity list`.

## Filed

**WI 2541** — should `identity list` grow a `--verify`? Sprint 049 ruled
there is "nothing to verify" for an identity, and deleting the token
`list` took the only `--verify` (and `just tokens-verify`, and the
distinct exit code 2) with it. The question 049 answered is not quite
the question that is left: a *token* could go stale, which is what 049
denied, but a config on disk can still differ from the roster the
running service loaded (an edit with no SIGHUP). Reversing a written
decision from a prior sprint is a decision, not a repair, so it is
Ken's/the overseer's call rather than this leg's.

## No soak

Nothing here has a time-gated acceptance criterion. Every clause was
verified in-session, with controls.
