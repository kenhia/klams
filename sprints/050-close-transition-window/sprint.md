# Sprint 050 — close the transition window

**Proposal:** korg:2450 (slice 4.5 of program korg:2440, simplify-secrets)
**Covers:** WI 2390 — client cutover to the identity header, then delete
every `[[auth.tokens]]` row
**Branch:** `050-close-transition-window` · **Version:** 0.1.50
**Leg:** karc `klams-26ae1c` on kubs0, overseen.

## Goal

Sprint 049 taught klams to authenticate a declared name
(`X-Homelab-Agent`) and left the legacy bearer path alive: the window is
open exactly while `[[auth.tokens]]` has rows (049 D-1). Every client
repo has since shipped its half. This sprint moves the clients klams
itself owns, repoints the host files, and then deletes the rows — which
is what closes the window.

Acceptance (WI 2390): no `[[auth.tokens]]` row remains; every consumer
file carries the header and no bearer; every listed client still writes
under its own author.

## Premise check (start-sprint Step 5)

Measured on kubs0 and kai at sprint start, 2026-09-12.

| claim | verdict |
|---|---|
| 15 `[[auth.tokens]]` rows, 16 `[[auth.identities]]` rows live | **holds** — `sudo klams-token list` / `identity list` agree exactly with handoff korg:2457 |
| kmon (2420) deployed and token-free | **holds** — `current` → `releases/20260912T223914-c590859`, no `KMON_KLAMS_TOKEN` in kai's `.env` |
| klams-view (2422) deployed, `KLAMS_TOKEN` gone | **holds** — `/etc/klams-view/klams-view.env` has `KLAMS_URL` and no token; unit active |
| klams-mind (2423) complete at merge | **holds** — CLI only, no unit |
| kyac (2421) may not be deployed | **DRIFTED, and the drift is the one the overseer predicted.** `eae5a8f` is checked out on kai but `kyac-server.service` entered active at 11:31:58 PDT and the commit is 14:07:22 PDT — the running server predates its own cutover. `just deploy` is required, and is pre-ruled Branch A on the proposal. |
| host files still on bearer | **holds** — klams entry is `Authorization` in all 7 files (`~/.claude.json` ×3, `~/.copilot/mcp-config.json` ×2, `~/.config/karc/karc-legs.mcp.json` ×2) |
| `multea-viae` is a live grant to delete | **holds** — token row 6 and an identity row both exist |
| four zero-consumer grants get no identity row | **DRIFTED, in the direction of more work.** 049 mirrored *all fifteen* grants, so `alice`, `klams-bench`, `ken_admin` and `token-master` each have an identity row that WI 2390 says must not exist. This slice deletes five identity rows (those four plus `multea-viae`), not just the token table. |

Cross-project plan (Step 6): klams is not in
`cross-project-planning/index.md` — no plan applies.

## Scope decisions

- **D-1 — `klams-client` sends a name, not a secret.** `Client::new`
  takes an `agent_name` and sends `X-Homelab-Agent`. There is no
  credential mode and no fallback: the program's client convention
  (ruling 1 on korg:2440) is that identity-and-token together is an
  error, and shape (a) — the setting ceases to exist — is preferred.
- **D-2 — the server's legacy bearer path stays, and gets a follow-up.**
  kaed filed WI 2471 and karc filed WI 2474 for exactly this: deleting
  the code path is gated on *observing* the window closed from a
  restarted session, which is time, not work. Deleting it here would
  also strand this leg's own MCP client mid-turn.
- **D-3 — the four zero-consumer grants lose their identity rows too.**
  WI 2390 names `alice`, `bench`, `ken-admin` and `token-master` as
  getting no identity row, and Ken's "delete now" (2026-09-11) predates
  049's decision to mirror all fifteen grants. `multea-viae` goes with
  them — kyac's retired predecessor, "delete, do not migrate". Five
  rows, leaving eleven. Flagged for the overseer: this makes the bench
  harness unrunnable until somebody adds the row back, which is one
  `klams-token identity add` and no secret. `tools/bench/README.md`
  now says so.
- **D-4 — `klams-client` keeps `Client::new(url, name)`'s shape.** The
  parameter went from a token to a name with no type change, so nothing
  would have failed to compile if a call site had been missed. All six
  are in this repo and all six were changed; a wiremock test
  (`no_authorization_header_is_ever_sent`) asserts the client emits no
  `Authorization` header at all, which is the regression the type
  system cannot catch.

## What changed in the repo

**Client side — the three klams owns.**

- `klams-client` sends `X-Homelab-Agent` and no credential. It exports
  `AGENT_HEADER` rather than depending on `klams-api`.
- `klams-scanner`: config `token` → `agent`; CLI `--token`/`$KLAMS_TOKEN`
  → `--agent`/`$KLAMS_AGENT`. `publish_delete`'s raw-reqwest path too.
- `klams-monitor`: config `token` → `agent`.
- `tools/bench`: `--klams-token`/`$KLAMS_TOKEN` → `--klams-agent`/
  `$KLAMS_AGENT`.

A stale `token = ` left in an old config file is **ignored, not
refused** — shape (a) in the program's refined client convention: the
setting ceased to exist, and serde drops unknown fields.

**Operator surfaces.**

- `provision-storage-root.sh` renders `[[auth.identities]]` rows and
  generates nothing. It no longer prints a credential at the end,
  because there isn't one; it prints the identity name and a `curl`.
- `verify-mvp.sh` / the justfile take `KLAMS_AGENT`. The `@` on those
  recipes is no longer load-bearing (nothing secret to echo) and the
  comment saying it was has been corrected.
- Example configs, `klams.example.toml`'s `[[auth.tokens]]` block
  (retired, with a migration table), `README`, `docs/auth.md`,
  `architecture.md`, `setup.md`, `usage.md`, `install.md`,
  `klams-mcp-for-agents.md`, `tools/bench/README.md`.

**The `age` backup machinery is gone** (proposal note 3): `backup.rs`,
`--age-recipient`, `$KLAMS_TOKEN_AGE_RECIPIENT`, `backup.age-recipient`,
`klams-token restore --identity`, and the fingerprint manifests.
Durable backups are plain timestamped copies. Two things were kept
deliberately:

- the in-memory rollback split from 046 — introduced to afford
  encryption, worth keeping on its own merits, since a rollback that
  needs the disk copy fails exactly when the disk is the problem;
- `prune`'s recognition of `.age` / `.manifest.json` suffixes, so it can
  still sweep what an older `klams-token` left behind.

## The cutover

Order throughout, per the program's rule: **repoint → live-check →
only then delete**. Every probe was run **from the host that owns the
file**, with two controls each — an unknown declared name and no header
at all, both of which must be `401`. A single `200` proves the header
works; it does not prove a bearer stopped being required, and the
controls are what separate those.

### The seven host MCP config files

| host | file | before | after |
|---|---|---|---|
| kubs0 | `~/.claude.json` | `Authorization` | `X-Homelab-Agent: claude` |
| kai | `~/.claude.json` | `Authorization` | `X-Homelab-Agent: claude` |
| cleo | `~/.claude.json` | `Authorization` | `X-Homelab-Agent: claude` |
| kubs0 | `~/.copilot/mcp-config.json` | `Authorization` | `X-Homelab-Agent: ghcp` |
| kai | `~/.copilot/mcp-config.json` | `Authorization` | `X-Homelab-Agent: ghcp` |
| kubs0 | `~/.config/karc/karc-legs.mcp.json` | `Authorization` | `X-Homelab-Agent: claude` |
| kai | `~/.config/karc/karc-legs.mcp.json` | `Authorization` | `X-Homelab-Agent: claude` |

The three `~/.claude.json` files went through `claude mcp remove -s user`
then `claude mcp add -s user -t http … -H`, never a hand edit — kaed 023
measured that route and it is the only one available on cleo, whose file
holds project keys differing only in case that PowerShell 5.1 rejects
outright. The other four are small and structurally simple, so a
targeted Python rewrite of the one header was used.

**Verification was by structural census, not by a backup file.** The
documented route takes a pre-edit backup and compares counts. A backup
of these files holds live bearers, which is a new copy of the thing the
program is deleting, so instead the census — bytes, top-level keys,
`mcpServers` count, project count, BOM, and the header *names* per
server — was captured before and after and compared. Nothing but the
byte count moved on any of the seven, and every project key survived
(kai 56/56, kubs0 12/12, cleo 7/7). Nothing that could hold a credential
was written to disk at any point.

All seven then probed `200` on their own stored header, `401` on an
unknown name, `401` with no header.

### Repaired in passing — kubs0's karc toolbox had a dead kaed entry

`~/.config/karc/karc-legs.mcp.json` on kubs0 still carried an
`Authorization` bearer for `kaed-kai`, which has been `401` since kaed's
slice korg:2424 deleted every kaed credential. Every karc leg on kubs0
has been running with a broken kaed client since then — including this
one.

Repaired here rather than filed: the file was already open for the klams
edit, the correct value is not a choice (`claude-kubs0` is an existing
kaed identity, and the *same host's* `~/.claude.json` already declares
it and answers `200`), and the fix is verifiable by re-probing. It went
`401` → `200`.

The same dead entry in `~/.copilot/mcp-config.json` on **both** hosts is
**not** repaired and is filed as korg:2490, because there it is a
decision: kaed's roster is exactly `claude`, `claude-kai`,
`claude-kubs0` — there is no `ghcp` identity, so there is no correct
value to write.

## The bug the type system could not catch, and how it surfaced

`Client::new(url, bearer)` became `Client::new(url, agent_name)` with no
type change — both are `impl Into<String>` — so a call site that kept
passing a token would compile and then send the token bytes as a
declared name, which is a `401`. D-4 named that risk; the integration
suite then found a live instance of it.

`us3d_scanner_e2e` failed with `nonce_b not found`. The chain:
`scan_root` took `bearer: &str` as its third argument, the test passed
`&server.bearer_token`, so `publish_delete` declared the token string as
an agent name, got `401`, and the delete-before-reindex step aborted —
so the *edit* never landed and the new nonce was never searchable. A
missed rename presenting as a retrieval failure two layers away.

**Fixed by deleting the parameter, not by correcting the call sites.**
`scan_root` already receives the `Client`, and the client knows the
identity it declares (`Client::agent()`), so the third argument was
redundant the moment the credential became a name. Removing it makes the
whole bug class unrepresentable rather than fixed once. Three call sites
got shorter.

**A second lesson, about the gate rather than the code.** The first
integration run was invoked as `just test-integration 2>&1 | tail -60`,
and the pipeline reported the exit status of `tail` — `0` — while the
suite underneath had exited `101`. The failure was visible in the text
and invisible in the status, which is the shape where a run gets
recorded as green. The re-run redirects to a file and reads `$?`
directly. A suppressed failure and a clean pass are indistinguishable
unless the status is asserted separately from the output.

### Four more surfaces a second sweep found

The first pass followed the obvious trail — the two daemons, the bench
harness, the docs. A `grep` for `KLAMS_TOKEN|bearer_auth|Authorization:
Bearer` across everything shipped then turned up four more, none of
which the first pass would ever have reached:

- **`tools/response-tokens`** — a second ops tool (not just bench)
  calling klams with `reqwest`'s `.bearer_auth()`. It reads
  `$KLAMS_AGENT` and declares it now.
- **`sprints/003-non-agentic-writes/handoff/examples/post-userfact.sh`**
  — a *live* example script, still executed by an integration test, that
  posts with `Authorization: Bearer`. Repaired despite living under a
  spec-kit-era sprint directory: AGENTS.md's "don't retrofit them" is
  about not imposing the newer layout on old records, not about leaving
  an executable that this change breaks.
- **`us3e_handoff_layout.rs`**, which drives that script.
- **`provision-storage-root.sh`'s printed next steps**, still telling a
  fresh operator to run `KLAMS_TOKEN=<token> just smoke` — the one line
  of the script the first pass did not rewrite, and the only line a new
  operator actually types.

What survives on purpose: every `Authorization: Bearer` in
`crates/klams-service/tests/` and `crates/klams-api/src/auth.rs`. Those
exercise the legacy server path against the in-process router, and that
path is deliberately still alive until WI 2489. No **client** in this
repo sends a credential.

### The daemons and kyac

`0.1.50` published to the store and installed on kubs0 (service,
scanner, monitor, `klams-token`) and kai (scanner). **Binaries before
configs, deliberately** — the new scanner requires `agent` and kai's
timer was 30 minutes out, so repointing its config first would have
broken the next tick.

- kubs0 `/etc/klams/scanner.toml`, `monitor.toml` → `agent = `. Edited
  with a script that prints line counts and key names only; no value
  from `/etc/klams` was ever read into the transcript (krot WI 2466).
- Both scanners forced a run: `Result=success`, and the journal shows
  `cleared stale chunks before reindex` — which is `publish_delete`
  authenticating on the header, the exact path that failed in the test.
- **kyac, Branch A.** Its precondition had drifted as the overseer
  predicted: `eae5a8f` was checked out on kai but `kyac-server.service`
  entered active at 11:31:58 PDT against a 14:07:22 PDT commit, so the
  *running* server still sent a bearer. Ran kyac's documented
  `just deploy` (pull → `uv sync` → `web-build` → `server-restart`) on
  kai, then `just check-live` from kai: 4 passed.

### Deleting the rows

Fifteen `[[auth.tokens]]` rows, then five `[[auth.identities]]` rows
(`alice`, `klams-bench`, `ken_admin`, `token-master`, `multea-viae`).
`klams-token remove` refuses a non-interactive removal without `--yes`,
which is the right shape and worth knowing.

```
OK: [auth] identities=11, legacy_grants=0 (transition window closed)
transition window closed: no legacy `[[auth.tokens]]` grants remain
SIGHUP: auth tables reloaded  grants=0  identities=11
```

**`/etc/klams/` now holds no secret but the Postgres DSN.** The five
sprint-046 `.age` backups and their manifests were swept by `prune`
itself — the suffix recognition kept for exactly this. The plaintext
backups the removals created were deleted immediately afterwards along
with `backup.age-recipient`.

One thing worth saying plainly rather than letting it pass: because 050
retired the encryption, each of the fifteen removals wrote a *plaintext*
backup holding the tokens not yet deleted. That is the hazard klams
#1377 found seven instances of. The window was a few minutes, in a
`root`-only directory on a single-user host, and every one of those
files was deleted before the sprint ended — but it is a real
consequence of doing the retirement and the deletion in one sprint, and
the honest order for anyone repeating this is: delete the rows first,
retire the encryption second.

## Verification

Every probe named the host it ran from.

| check | result |
|---|---|
| 11 surviving identities, `GET /memory/policy` from kubs0 | all `200` |
| 5 deleted identities | all `401` |
| a bearer, any value | `401` |
| no credential | `401` |
| `klams-view` (read scope) `POST /memory/knowledge/index` | `403` — scopes still enforced |
| `claude` from **kai**, `POST /memory/knowledge/index` | `200`; audit line `agent_name=claude tailnet_node=kai auth=identity` |
| header / unknown name / none, from **kai** | `200` / `401` / `401` |
| header / unknown name / none, from **cleo** | `200` / `401` / `401` |
| `just health`, `just verify` | 7 passed, 0 failed, 3 skipped — same as 049 |
| `klams-service` errors since reload | none |
| klams-view `GET /` and `/api/authors` | `200`, live data |

**Attribution provably did not move.** The identity→author bindings were
captured from the journal before the cutover and after the reload:
exactly the five deleted names disappeared, and **not one surviving
identity's `author_id` changed** — the set difference in the other
direction is empty. That is the claim the whole design rests on, and it
is measured rather than argued.

Fleet sweep, counts only: zero `Authorization` headers for klams in any
of the seven host files, and zero `token =` lines in either daemon
config.
