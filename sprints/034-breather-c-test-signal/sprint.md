# Sprint 034 — Breather C: test-signal truth + the deferred upgrades

**korg:** proposal 769 (`korg:769`), covering #732 #733 #734 #702 #703 #773 #774 #775
**Branch:** `034-breather-c-test-signal`

## Goal

Third installment of the breather-A/B pattern: clear the mechanical debt the
033 retrospective filed, the two upgrades 032 deliberately deferred, and the
three Tier-0 recovery-story bugs from `sprints/planning/generalize-klams.md`
§5.1. Everything is low-risk and protects whatever runs next.

## Scope

Keystone first — #732/#733/#734 rewrite the same test files, so they land as
one coordinated pass (trait shrink → helper dedupe → dim parameterization):

1. **#734 (S)** — remove the unreachable `Store::upsert_fact` v1 trait rung;
   `upsert_fact_v2` becomes the required method; migrate the seed helper;
   delete the mock impls nothing calls.
2. **#733 (M)** — dedupe the arc's test helpers: 6× `mcp_state_from`, the
   78-line `McpSession` copy in `mcp_bearer_author.rs`, 3× `INIT_BODY`,
   duplicated `state()`/caller/`RegisterAuthorInput` builders in klams-mcp,
   4× `ensure_collection`, and the 10 `Ok(vec![0.0; 384])` stub embedders —
   so the next suite copies from common, not from a sibling.
3. **#732 (M)** — the test/CI stack still exercises the retired embedder
   shape (bge-small, 384-dim) while production has been
   Qwen3-Embedding-0.6B/1024-dim since 028. Parameterize the test dim, make
   CPU-runner bge-small a decision-with-a-comment, add a hermetic
   query-prefix test and at least one 1024-dim wiring test.
4. **#703 (S)** — `TokenGrantConfig::validate` rejects `manage`/`admin`
   grants without an `agent_name`; decide the legacy `bearer_token` path's
   fate in the same change; docs + example config + tests updated together.
5. **#773 (S, bug)** — `provision-storage-root.sh` discards the token it
   generates (sed pattern doesn't exist in `klams.example.toml`); emit a
   scoped `[[auth.tokens]]` grant (`agent_name = "operator"`) and print it.
   Paired with #703 so the script and the validation rule agree on day one.
6. **#774 (XS, bug)** — drop `ReadWritePaths=/gratch/klams-backup` from the
   shipped systemd unit; document the `[backup]` drop-in.
7. **#775 (XS)** — `KLAMS_CONFIG` falls back to
   `$XDG_CONFIG_HOME/klams/klams.toml` when `/ai/klams/config/klams.toml`
   doesn't exist; justfile recipes aligned; no-config error names both paths.
8. **#702 (M)** — container image refresh, each bump with its own risk
   class's verification: Qdrant v1.18 (backup + counts + eval; unblocked by
   032's v1-collection drop), TEI tag (eval-gated ×2 — embedder and
   reranker share the tag; check whether newer TEI serves Qwen3-Reranker),
   Prometheus/Grafana **not from here** — they run on kubsdb, an
   unverifiable tag bump from kubs0 is worse than none.

## Acceptance

- Test suite exercises (or at least pins) the deployed embedder shape; a
  dim mismatch can't ride to production silently.
- One shared source for each test helper that had drifted copies.
- `upsert_fact` v1 gone from the trait and all mocks.
- Privileged grants require `agent_name`; provision script emits a config
  the validator accepts and the printed token round-trips.
- Shipped unit starts on a host without `/gratch/klams-backup`.
- Fresh host with no `/ai/klams` finds config under XDG.
- Image bumps landed with their verification, `compose.env.example`
  matching what is actually deployed; TEI half may slip alone if the
  bake-off drags.

## Chronicle

- **2026-07-29** — Sprint opened from `korg:769` (marked active).
  Version bumped to 0.1.34. Branch `034-breather-c-test-signal`.

### #734 — upsert_fact v1 rung removed

As specified: `upsert_fact_v2` is the trait's required method; the v1
trait method, `CompositeStore::upsert_fact`, and the inherent
`PostgresStore::upsert_fact` (plain INSERT … ON CONFLICT with none of
v2's trust/dissent semantics) are gone. Call-site migrations:
`common/seed.rs` and `mcp_write_policy.rs` seed via v2 and unwrap
`Persisted`; `us3_decay.rs` got a local `seed_fact` helper doing the
same. All 11 mock stores (9 klams-api contract tests, klams-core
queue.rs, klams-mcp MemStore) now implement v2 — the method production
actually calls — instead of a method nothing called.

### #733 — helper dedupe

Everything on the WI's inventory landed in
`klams-service/tests/common/mod.rs` (or `klams-mcp/tests/support/mod.rs`
for that crate): `mcp_state_from` (6 copies), `make_author` (4 copies —
api_phase7's "variant" metadata was never asserted on, so all four
collapsed to the all-`None` shape), the 78-line `McpSession` copy in
`mcp_bearer_author.rs`, `INIT_BODY` (3) + `parse_sse_json` (2) now
`pub` in common, session seeders as `McpSession::seed_knowledge` /
`seed_fact` methods, `ensure_collection(collection, drop_first)` (4
copies, with the snapshot-poisoning rationale preserved on the common
doc), and the test-stack URL/pg-bin helpers. klams-mcp's `state()`,
generic `caller()` (+`writer`/`curator`), and `author_input()` moved
into `tests/support`. The cross-crate "one shared mock store" idea was
deliberately NOT taken: each contract test's hand-rolled `impl Store`
IS its fixture, and a configurable shared mock would be a bigger
abstraction than the duplication it removes (YAGNI).

### #732 — embedder test-signal truth

- CPU-runner bge-small is now a **decision with a comment**, twice: on
  `TEST_EMBED_DIM` in `tests/common/mod.rs` (the ONE place the suite's
  dim lives; everything derives from it) and on the TEI service in
  `tests/docker-compose.test.yml`.
- Hermetic query-prefix tests: `prefixed_query` extracted in
  `composite.rs` (it was inlined in `embed_query`, untestable without a
  live stack); tests pin empty-prefix-verbatim and the exact deployed
  Qwen3 instruct prefix.
- Hermetic 1024-dim wiring tests in `embeddings.rs` (wiremock): a
  TEI-dialect client configured for 1024 accepts 1024-wide vectors and
  refuses the retired 384 shape — the dim-mismatch regression class,
  pinned without a GPU.
- The six in-file fixture TOMLs in `config.rs` now carry the deployed
  shape (Qwen3-Embedding-0.6B / 1024), so new tests copy-paste truth.
  (The example-toml test pinning 1024 + the instruct prefix already
  existed from 032.)

### #703 — privileged grants must be attributable; legacy path RETIRED

`TokenGrantConfig::validate` rejects `manage`/`admin` grants without
`agent_name`. The legacy `[auth] bearer_token`'s fate: **retired**, not
exempted — it is exactly the shape the new rule forbids, kubs0 has run
scoped-only since 032, and #773's provision fix removes the last
producer of one. The key still *parses* so a config carrying one is
refused loudly (startup, `--validate-config`, SIGHUP — the reload keeps
the previous table) with the migration note, instead of silently
ignoring a credential the operator believes is live. Auth validation now
lives in ONE place (`build_auth_grants`), shared by startup and reload —
previously startup never ran per-grant validation at all.
`auth_scoped_tokens.rs` flipped from pinning the legacy grant's shape to
pinning that the shape is no longer expressible. Verified live: kubs0's
`/etc/klams/klams.toml` has `bearer_token` commented out and every
privileged grant carries `agent_name` — the new rules pass on the
production config as-is.

### #773 — provision script renders a bootable config

Root cause as filed: the sed pattern only ever existed in
`compose.env.example`, so the generated token went nowhere and the
rendered config had no active token form (post-#670 they're all
commented out). The script now appends a scoped
`[[auth.tokens]]` grant (`read+write+manage`, `agent_name = "operator"`
— satisfying #703's rule on day one) and prints the token plus a
round-trip check step. Verified end-to-end against a scratch
`KLAMS_ROOT`: rendered config passes `--validate-config` with
`scoped_grants=1`.

### #774 — unit no longer hardcodes the backup path

`ReadWritePaths=/gratch/klams-backup` dropped from the shipped unit;
the comment now documents the `[backup]` drop-in. kubs0 got the drop-in
(`klams-service.service.d/backup.conf`) **before** this ships, so the
next deploy doesn't EROFS the nightly backup; recorded as k-homelab
WI #784 for fold-in.

### #775 — config XDG fallback

`resolve_config_path()` in main.rs: `KLAMS_CONFIG` wins; else
`/ai/klams/config/klams.toml` if present; else
`$XDG_CONFIG_HOME/klams/klams.toml` (`~/.config` fallback; empty
XDG_CONFIG_HOME = unset per spec). No-config error names both tried
paths. justfile recipes share one `klams_config` variable with the same
resolution. (Fun fact surfaced by testing: `/ai/klams/config/klams.toml`
doesn't exist even on kubs0 — the live config is `/etc/klams/klams.toml`
via the unit's env var, so the storage-root "default" was already
fictional everywhere.)

### #702 — image refresh: mostly already true

Reality check against the live host made most of this WI moot:

- **Qdrant v1.18.0** has been the *deployed server* since 2026-05-17 —
  the WI's "server bump, storage-format change, wants backup+counts+eval
  around it" described a bump that had already happened (what 032
  actually fixed was the *client* lagging on 1.12). Verified live:
  `/collections/knowledge_items_v2` green, 180,412 points, 1024-dim,
  server reports 1.18.0. `compose.env.example` has pinned v1.18.0 since
  030. Nothing to do.
- **TEI `89-1.9`**: the containers were re-pulled 2026-07-26 and carry
  the current upstream build (latest TEI release is still 1.9.3,
  2026-03-23; no newer minor exists). **Qwen3-Reranker: still blocked**
  — upstream PR #886 (+ #730/#835) open unmerged as of 2026-07-29, so
  the `RERANKER_MODEL_ID` swap + bake-off stays parked (the 030 gotcha
  memory updated). No tag change → no eval gate triggered.
- **Prometheus/Grafana**: run on kubsdb, not here; bumping their tags
  from kubs0 stays an unverifiable production change (same call as
  032). Filed where it can be done and verified: **k-homelab WI #785**.

### Verification (2026-07-29)

- `just gate` green: `cargo fmt --check`, workspace clippy with
  `-D warnings`, full unit suite (300+ tests, includes the four new
  hermetic prefix/dim tests).
- Docker-gated integration suite: **123 passed, 0 failed** at default
  parallelism against a fresh `docker-compose.test.yml` stack; stack
  torn down afterwards (the 032 lesson).
- Live-host checks, ahead of any deploy: kubs0's
  `/etc/klams/klams.toml` passes the new `--validate-config`
  (`scoped_grants=14` — bearer_token already commented out, every
  privileged grant already attributed); the `backup.conf` drop-in is
  installed and `daemon-reload`ed (k-homelab WI #784); the provision
  script renders a config that validates with `scoped_grants=1` on a
  scratch root; the no-config error names both resolution paths.
- Docs updated in-sprint: `auth.md` (migration note), `setup.md`
  (provision flow, config resolution, backup drop-in), `usage.md`,
  `architecture.md`, `klams.example.toml`.
