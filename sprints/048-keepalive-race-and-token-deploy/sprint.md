# Sprint 048 — the keep-alive race behind the "Unreachable" flicker, and klams-token in the deploy path

Proposal: korg:2220 (program korg:2233, backlog drain 3 of 3, slice rank 7).
Covers WI 1806 (S, bug) and WI 1697 (XS, bug). Version: `0.1.48`.

This sprint runs as an **overseen karc leg** (`klams-ce5444`, kubs0). The
overseer's ruling of 2026-09-10 (korg:2220 comment 1711) sets two of the
decisions below, and is quoted where it does.

## Goal

Two klams bugs with fleet-visible symptoms:

1. **1806** — kpidash's klams card intermittently flickers
   `Unreachable: error sending request for url (http://127.0.0.1:7777/healthz)`.
   Diagnosed 2026-09-02 by packet capture: klams-monitor's kpidash reporter
   polls `/healthz` every 30s over a **pooled keep-alive connection**, and
   klams-service's hyper `header_read_timeout` is also 30s. The request and
   the server's FIN cross; the server RSTs; reqwest reports a bare
   "error sending request" which lands verbatim on the dashboard card.
2. **1697** — `klams-token` is not in the deploy path. `just publish` and
   `just deploy-from-store` each carry three binaries; `klams-token` is not
   one of them, so a textbook deploy silently leaves it a version behind.

## Premise check (start-sprint Step 5)

Both premises **hold**, verified against the live kubs0 deployment rather
than the claim text.

**1806 — holds, and the two colliding numbers are both defaults.**

- `crates/klams-monitor/src/kpidash.rs:127` builds the healthz client as
  `reqwest::Client::builder().timeout(HTTP_TIMEOUT).build()` — no pool
  configuration, so reqwest's default pooling and 90s idle timeout apply and
  the connection *is* reused between polls.
- `crates/klams-service/src/limits.rs:244` passes
  `header_read_timeout(header_read)` to hyper's `http1::Builder`.
- Deployed `/etc/klams/klams.toml` has **no `[service.limits]` section**, so
  `header_read_timeout_secs` = 30 (`config.rs:141`).
- Deployed `/etc/klams/monitor.toml` `[kpidash]` has **no `interval_secs`**,
  so the poll cadence = 30 (`kpidash.rs:72`).

Two independent defaults that happen to be the same number. Nothing in the
config was tuned into this; a stock klams polled by a stock klams-monitor
races itself.

**1697 — holds.** `justfile:266` (`publish`) builds and publishes exactly
`klams-service klams-scanner klams-monitor`; `deploy-from-store` defaults to
the same three (`justfile:309`); `deploy/install-from-store.sh` knows only
those three names (its usage line and its post-install activation `case`).
`klams-token` appears only in `install-klams-token`, a build-from-source
recipe. The *symptom* is currently clear — `/usr/local/bin/klams-token`
reports 0.1.46, matching the other three — but only because sprint 046 fixed
it by hand. The defect is untouched: the next deploy re-opens it.

**1697's live consumer, per the overseer's ruling.** `sudo klams-token list`
shows grant idx 14, `kmon` — "kmon (homelab monitoring controller)",
scopes read,write, minted by kmon sprint 18. It is live, so the verification
for this item is a real store round-trip, not a synthetic one.

## Decisions

### D1 — `/healthz` responds `Connection: close`; the timeout is not retuned

The overseer offered two options and set the test for choosing between them:

> Pick the one that *removes* the race rather than moving it. A timeout
> bumped to 35s still collides with some future 35s poll.

That test rules out retuning `header_read_timeout`. **Any** server-side idle
close races a client whose inter-request gap sits in the close window — the
hazard is not the number 30, it is that a pooled connection can be closed by
one end while the other is writing to it. Choosing 47s moves the collision to
a 47s poller.

So the fix is the structural one: `/healthz` sets `Connection: close`, the
client does not pool it, and each poll is a fresh connection with no
possibility of a crossed FIN. `/healthz` is a liveness probe on a fixed
cadence — pooling buys it nothing, and one loopback handshake per 30s is
noise. This holds for every present and future watcher at every cadence,
which is what "removes" means.

### D2 — the sprint-009 contract is corrected in docs, not by retuning

WI 1806's own follow-up section notes the deeper mismatch: sprint 009
documented `header_read_timeout` as reaping clients that *never send headers*
and `keep_alive_timeout` (75s) as governing idle keep-alive. Under hyper 1.x
`Conn::poll_read_head` re-arms the header-read timer for **every** request
head, including after a response on a keep-alive connection — so the
effective idle window is `min(header_read_timeout_secs,
keep_alive_timeout_secs)` = 30s, and the 75s watchdog in `limits.rs` can
never fire first for an HTTP/1.1 keep-alive client.

The WI offers two acceptable resolutions — document it, or raise
`header_read_timeout` to >= `keep_alive_timeout`. Raising it is the same
move D1 just rejected (it relocates the collision to 75s) and it weakens the
slowloris defence the timer is actually there for. So the contract is
corrected in the docs to say what the code does, and the code keeps the
defence. Endpoints polled on a fixed cadence get D1's treatment instead.

`/metrics` is **not** at risk today: `deploy/prometheus/prometheus.yml` sets
`scrape_interval: 15s`, and 15s < 30s means the connection is never idle long
enough to be reaped. It would be at risk at a 30s scrape interval, so that
constraint is written down rather than left to be rediscovered.

### D3 — `klams-token` is published to the store (WI 1697 option 2)

The WI offers a smaller option (name it in the `deploy-kubs0` skill) and a
doctrinally consistent one (publish it like the other three). Taking the
second: k-homelab's rule is that every deploy publishes a versioned asset and
every install pulls from the store, and only that option lets an audit ask
the store which `klams-token` a host should be on. The skill gets updated too
— that is not an either/or.

## Cross-repo posture

Per the overseer's ruling, kpidash **658** is the same defect as 1806 and is
closed here with this sprint's evidence, as a korg write in the kpidash
project. **No kpidash repo edit.** The kpidash-side change the WI originally
proposed (`pool_max_idle_per_host(0)` in the reporter) is not needed: the
reporter lives in *this* repo (`klams-monitor`), and D1 makes the server tell
every client not to pool, which is strictly stronger.

## What shipped

### 1806 — the keep-alive race

**`/healthz` answers with `Connection: close`**
(`crates/klams-api/src/handlers/health.rs`). No watcher pools the
connection, so there is no idle connection left to race, at any cadence.

Three tests, deliberately at three levels, because two of them individually
would let the fix rot silently:

- `klams-api/tests/contract_health.rs::healthz_sets_connection_close` — the
  router sets the header.
- `klams-service/tests/connection_limits.rs::t4_connection_close_response_header_ends_the_connection`
  — hyper *honours* a handler-set `Connection: close` and actually ends the
  connection. `keep_alive_timeout_secs` is 300 in that test so nothing but the
  header can be what closed the socket. This is the characterization test: if
  a hyper upgrade ever stopped honouring the header, the card would flicker
  again and the other two tests would still pass.
- `klams-service/tests/us5_health.rs::healthz_declines_keep_alive_on_the_wire`
  (docker-gated) — the real router against the real store, speaking HTTP/1.1
  down a socket, with the request explicitly asking for `keep-alive`. Asserts
  the header comes back on the wire and the server closes anyway.

**The journal now records a bad poll** (`crates/klams-monitor/src/kpidash.rs`).
`check_health` returns a third element, `detail`, carrying the error's full
`source()` chain; `report_once` logs `warn!` whenever the state is not `ok`.
The card text and the published payload are **unchanged** — the chain goes to
the journal only, which is where the WI asked for it.

This half matters more than it looks. Publishing a `down` card is a
*successful* publish, so the reporter logged nothing and the only trace of a
failed poll was a Pi screen. And reqwest's `Display` for a transport failure
is just `error sending request for url (…)` — the sentence that identifies the
fault (`connection closed before message completed`) lives exclusively in
`source()`. Between them, that is why diagnosing #1806 needed a packet
capture. `error_chain` is unit-tested both ways (nested and lone).

**Docs**: `docs/architecture.md` §2.9 records the `/healthz` decision, and
§4.1 now states the real semantics — the effective idle keep-alive window is
`min(header_read_timeout_secs, keep_alive_timeout_secs)`, not the 75s the
sprint-009 contract claimed — plus the operational rule that follows from it
and the note that `/metrics` is safe only because Prometheus scrapes at 15s.

### 1697 — klams-token in the deploy path

`klams-token` is now a published, store-installed artifact like the other
three:

- `justfile` — `publish` builds it, includes it in the all-versions-agree
  assertion, and uploads it under its own artifact name; `deploy-from-store`
  has it in the default set.
- `deploy/install-from-store.sh` — named in the usage, and given its own
  activation case. Without that case it fell to the `*)` fallback, which
  prints "unknown unit — restart it by hand" and would send the reader looking
  for a service that does not exist. It prints
  `sudo klams-token list --verify` instead.
- `.claude/skills/deploy-kubs0/SKILL.md` — four binaries throughout, and step
  4 now requires `klams-token --version` beside the `/healthz` check, with the
  reason: `/healthz` is served by klams-service alone and is green whatever
  klams-token is on disk, which is exactly how 0.1.45 survived the 0.1.46
  deploy.
- `docs/architecture.md` §4.3, `docs/setup.md`, `docs/usage.md` — three/four
  corrected, and setup.md now points at the deploy rather than at
  `install-klams-token` as the way to keep the binary current.

## Verification

- `just gate` — green.
- `just test-integration` against `tests/docker-compose.test.yml` — green,
  122 test binaries, 0 failures. Stack torn down afterwards (AGENTS.md).
- The wire-level `/healthz` assertion above ran against the live test stack
  and passes; this is the end-to-end proof that the race is gone, not an
  inference from two unit tests.

**Not verifiable in this leg, and deliberately left for the deploy:** the
1697 fix is a *deploy-path* change, so the thing it fixes can only be observed
by a real publish + install round-trip. `klams-token` currently reads 0.1.46
on kubs0 — matching the other three — but only because sprint 046 repaired it
by hand. The evidence this sprint is owed is the 0.1.48 deploy showing all
four binaries move together, and `klams-token --version` in the deploy record
beside the `/healthz` version. The live consumer to verify against is grant
idx 14, `kmon` (read,write), minted by kmon sprint 18 — confirmed present via
`sudo klams-token list`.

## For the PR description (overseer's direction, post-review)

Two findings the overseer ruled belong in the PR description rather than
only in the wrap-up handoff:

1. **Both colliding numbers were defaults.** Deployed `klams.toml` has no
   `[service.limits]` section and deployed `monitor.toml` no `interval_secs`,
   so a stock klams polled by a stock klams-monitor races itself. This
   shipped to anyone who installs klams — it was never a kubs0 tuning
   accident.
2. **`deploy-from-store` now defaults to four binaries, so kai gains
   `klams-token`.** `just deploy-remote kai klams-scanner` names its binary
   and is unaffected; a bare `deploy-from-store` on kai would now also
   install the CLI. Harmless and arguably the point of 1697, but it is a
   behaviour change on a host this slice never otherwise touched. Also
   recorded on k-homelab 2280 so the fold-in captures the machine-state
   change.

## Post-review additions

- `deploy/prometheus/prometheus.yml` — the 30s constraint is now a comment
  beside `scrape_interval` itself. §4.1 was the wrong and only place for it:
  whoever one day raises that number will be editing prometheus.yml, not
  reading the architecture doc. The comment says what breaks, why 15s is
  what makes it safe, that the safety is accidental rather than designed,
  and that the fix for a slower scrape is `Connection: close` on `/metrics`
  rather than retuning the server.
- **klams 2283** filed — the documented `up -d` then `just test-integration`
  sequence fails, because `up -d` returns before healthchecks pass and
  `reset-test-stack.sh` probes once with no wait. Its hint then tells the
  operator to run the command they just ran. Filed with an explicit
  correction: the suite **cannot** pass without running — `reset-test-stack.sh`
  exits 1 and `just` propagates it. The exit-0 seen during this sprint was a
  caller-side `| tail` pipeline swallowing the status, not a repo defect.

## korg housekeeping

- **kpidash 658** is the same defect as 1806, per the overseer's ruling.
  Resolved in the kpidash project with this sprint's evidence. **No kpidash
  repo edit was made or is needed** — the server declines the pooled
  connection, which is strictly stronger than the reporter-side
  `pool_max_idle_per_host(0)` the WI originally proposed, and covers every
  other watcher too. The korg:2229 leg has nothing to do for it.
- **krot**: kmon's klams grant is *already* registered as a work item —
  krot **2215**, covered by proposal korg:2230, with the fingerprint,
  rotation procedure and verification already written into it (and the
  duplicate #2201 resolved). Nothing new filed; the overseer's
  "file it rather than registering it here" instruction is already satisfied.
- **k-homelab 2280** filed for the audit assertion WI 1697 suggests
  (`bin/audit kubs0` comparing `klams-token --version` against `/healthz`).
  That is a new assertion in a script another repo owns — a decision, not a
  documented procedure — so it was handed up rather than executed inline
  (`overseen-sprint` Branch B). The deploy *path* is fixed here; 2280 is the
  independent alarm that would notice if it ever silently stopped being
  fixed.

## Deployed 2026-09-10

- Version `0.1.48` live on kubs0 (`/healthz` confirms; was `0.1.46`).
- **All four binaries moved together — this is WI 1697's actual evidence**, not
  a formality, and it is the first deploy in which `klams-token` travelled on
  its own rather than being repaired by hand afterwards:

  | binary | before | after |
  |---|---|---|
  | `klams-service` | 0.1.46 | **0.1.48** |
  | `klams-scanner` | 0.1.46 | **0.1.48** |
  | `klams-monitor` | 0.1.46 | **0.1.48** |
  | `klams-token` | 0.1.46 | **0.1.48** |

  `/healthz` reports `0.1.48`; `klams-token --version` reports
  `klams-token 0.1.48`. Both recorded because `/healthz` is served by
  klams-service alone and cannot speak for the fourth binary — which is the
  whole of 1697.

- **The store confirmed the defect from the other side before the fix landed.**
  At preflight, `artifacts/klams-service/latest` read `0.1.46` and
  `artifacts/klams-token/latest` returned **404** — `klams-token` had never
  been published at all, in any version. It now exists at
  `artifacts/klams-token/0.1.48/`.
- Published to the store as `artifacts/klams-{service,scanner,monitor,token}/0.1.48/`.
- `install-from-store.sh` printed `klams-token`'s new activation line
  ("nothing to restart (operator CLI)") rather than the `*)` fallback's
  "unknown unit — restart it by hand". That fallback was the reason the case
  was added.
- Unit files: unchanged (no `deploy/*.service` or `deploy/*.timer` in the
  diff), so `install-systemd` was not run.
- kai's `klams-scanner`: **left at its current version, deliberately.** This
  sprint changed nothing the scanner executes — the `/healthz` header, the
  monitor's logging and the publish set are all kubs0-side. 0.1.48 is in the
  store whenever kai wants it (`just deploy-remote kai klams-scanner`). Noted
  because a scanner-affecting sprint that leaves kai behind is how the drift
  in #836 accumulated, and this one is not that.
- Rollback target: `0.1.46` via `just rollback` (`.prev` binaries in place for
  all four); any published version via `just deploy-from-store --version`.
- Migrations applied: none (no new files in `migrations/`).
- Config changes required: none. `/etc/klams/klams.toml` untouched.

### Verified live, beyond `/healthz`

- **The 1806 fix, on the production socket.** `curl -i --http1.1 -H 'Connection:
  keep-alive' http://127.0.0.1:7777/healthz` returns `connection: close`, and a
  raw `/dev/tcp` request that explicitly asks for keep-alive gets EOF from the
  server — the connection is genuinely closed, not merely labelled. There is no
  pooled connection left for a poll to race.
- **The new monitor logging, proving itself on its first run.** The restart
  produced exactly one failed poll, and the journal named its cause:

  ```
  WARN klams_monitor::kpidash: klams health poll did not come back ok
    state="down" text="Unreachable: error sending request for url (…/healthz)"
    detail="… : client error (Connect): tcp connect error: Connection refused (os error 111)"
  ```

  That is the known startup shape — the monitor comes up a few hundred ms
  before klams-service binds `:7777` — and it is *expected*, not a regression.
  What is new is the `detail` field: the `text` half is the same sourceless
  string that sat on the kpidash card for months saying nothing, and the
  `source()` chain beside it names `Connection refused` outright. Before this
  sprint that line did not exist at all, at any level.
- **15 of 15 grants live** against the new binary (`klams-token list
  --verify`), including idx 14 `kmon` — the consumer the overseer named for
  1697. That is 15 authenticated round-trips through the 0.1.48 service, and it
  also confirms the 0.1.48 `klams-token` works against the 0.1.48 service.
- **Ten poll cycles watched** after the restart (300s from 23:20:46 local):
  **zero** failed polls, no kpidash warn/error lines, no klams-service request
  errors. Because of this sprint's own logging, a failure in that window could
  not have been silent — which is what makes the zero mean something.
- `just health` / `just verify` were **not** run: both require `KLAMS_TOKEN`,
  which is not in `.env`, and sourcing a live grant into a session transcript
  is worse than the coverage is worth. `klams-token list --verify` is the
  stronger authenticated check and leaks nothing.
