# Decision — the legacy `[auth] bearer_token` (WI #670)

**Sprint 032. Decided 2026-07-26; Ken signed off on the recommendation
before implementation.**

#670 was filed as a *think-about*, deliberately deferred until the deep-
review backlog cleared. It asks four questions about one credential's
default. This records the answers and what was done.

## The finding, restated

`docs/reviews/2026-07-25-deep-review.md` F-3.4 noted that the legacy
`[auth] bearer_token` materializes a grant with every scope, and that
provisioning renders one by default — so a fresh install begins holding
a full-admin credential nobody chose. Sprint 025 made it slightly
stronger: scopes became flat-and-four (`read`/`write`/`manage`/`admin`,
`Scope::satisfies` is exact equality), and `Manage` was added to the
legacy grant on the reasoning that omitting it would *remove* capability
from the only token some deployments have.

Both readings are defensible, which is why this got a decision rather
than a default.

## Q1 — Is it used?

**No.** Established empirically on kubs0 before changing anything:

- The live token value appears in `/etc/klams/klams.toml` and its four
  `.bak-*` siblings, and **nowhere else** — not in any consumer config,
  agent config, `.env`, or script under `/etc/klams` or `~`.
- The config carries **14 scoped `[[auth.tokens]]` grants** covering
  every known consumer: `claude`, `ghcp`, `viewport`, `klams-mind`,
  `klams-scanner` (kubs0), `kai-scanner`, `klams-monitor`,
  `klams-bench`, `ansible_k`, `kyac`, `multea-viae`, `token-master`,
  `ken-admin`, and one test identity (`alice`).
- One of those, `ken-admin` (`agent_name = "ken_admin"`), already
  carries all four scopes — an **attributable** everything-token. The
  legacy credential was strictly redundant with it.
- `just health` / `just verify` default `KLAMS_TOKEN` to the literal
  `dev-token`, so the smoke scripts were never users of it either.

So this was a delete, not a redesign.

## Q2 — Should `bearer_token` keep granting all four scopes?

**Yes — unchanged.** It is the "everything" token by construction, and
narrowing it is the more dangerous edit: a deployment whose *only*
credential is `bearer_token` would silently lose capability on upgrade,
which is a worse failure than the one being avoided. The scope set is
now pinned by `crates/klams-api/tests/auth_scoped_tokens.rs`, which
previously asserted only three of the four — `Manage` was unpinned, i.e.
the set could have drifted exactly the way this WI worried about. All
four plus the grant's length are asserted now.

## Q3 — Should provisioning stop rendering one by default?

**Yes. This is the change.** `deploy/config/klams.example.toml` led with

```toml
[auth]
bearer_token = "changeme-rendered-by-provision-script"
```

as the first line of the block, so the full-admin credential was the
path of least resistance and the scoped grants were the commented-out
alternative. That is now inverted: `bearer_token` ships commented out
and labelled break-glass, and `[[auth.tokens]]` leads. A config with
neither refuses to start, so an operator makes a deliberate choice
instead of inheriting one.

Also fixed while here: `AuthConfigError::NoTokens` existed but was
**never constructed** — the real "at least one token form" guard was a
hand-written string duplicated across the startup validator and the
SIGHUP reloader. Both now render the error type, so the message has one
source. (The guard itself was already correct, which is what made the
kubs0 migration below safe: `[[auth.tokens]]`-only is a valid config.)

## Q4 — Should a `manage`/`admin` grant be required to declare an `agent_name`?

**Recommended, not implemented — needs its own change.** Every
privileged action being attributable is the property sprint 025 was
built around, and the legacy token is the one hole in it. But the legacy
grant *cannot* declare an `agent_name`; making it mandatory for
`manage`/`admin` is therefore equivalent to retiring the legacy path,
which is a larger behavioral change than the posture fix approved here
(and one needing a migration note, since a `bearer_token`-only
deployment would lose all access).

Recorded as the natural follow-on. The cost of deferring is low: kubs0
no longer has an unattributable privileged credential regardless, and
grants hot-reload on SIGHUP, so this stays cheap whenever it is picked
up.

## What changed

| Where | Change |
|---|---|
| `deploy/config/klams.example.toml` | `[auth]` block inverted: scoped grants lead, `bearer_token` commented out as break-glass, with the two properties it cannot have spelled out |
| `docs/auth.md` | "The legacy `auth.bearer_token`" section rewritten to record the decision, not just describe the mechanism |
| `crates/klams-api/tests/auth_scoped_tokens.rs` | Scope set pinned at exactly four (was three, `Manage` unpinned); asserts the grant binds to `system` |
| `crates/klams-service/src/main.rs` | The two ad-hoc "no tokens" bails now render `AuthConfigError::NoTokens` |
| `/etc/klams/klams.toml` (live, not in repo) | `bearer_token` removed; kubs0 runs on scoped grants only |

## Deliberately not in scope

Not an auth redesign. Sprint 025 settled the model (flat scopes,
ownership on delete, `manage` for cross-author curation) and it works in
production. F-4.5's accepted-posture items — no rate limiting, public
`/metrics`, tokens in a root-readable file — stay accepted and are not
reopened here.
