# Tasks: Operationalize Ingestion

**Input**: Design documents from `/specs/010-operationalize-ingestion/`
**Prerequisites**: [plan.md](plan.md), [spec.md](spec.md), [research.md](research.md), [data-model.md](data-model.md), [contracts/](contracts/), [quickstart.md](quickstart.md)

**Tests**: TDD applies to the one production-code change (US4 viewport
render — vitest first, per [contracts/author-counts-ui.md](contracts/author-counts-ui.md)).
The deployment stories (US1–US3) are verified by **executable acceptance
probes** (systemd status, sentinel-note search, parity transitions) — the
SDD analogue of tests for ops work. US5 is verification-only; US6 ships no
code.

**Organization**: Tasks are grouped by user story. The bulk of this sprint
is live-host operations on `kubs0`, not source changes — see
[research.md](research.md) §R1 for why US4 is render-only and US5 is
verify-and-close.

## Format: `[ID] [P?] [Story] Description`

- **[P]**: Can run in parallel (different files/host steps, no dependency)
- **[Story]**: US1–US6 maps to the spec's user stories
- File paths are exact; host commands name the unit/target

---

## Phase 1: Setup (Shared Infrastructure)

**Purpose**: Build the artifacts and committed examples the deployment needs.

- [X] T001 [P] Build release binaries with `cargo build --release` and confirm `target/release/klams-service`, `target/release/klams-scanner`, and `target/release/klams-monitor` exist
- [X] T044 [Defect] **Deployment-surfaced defect (discovered during Phase 1)**: the unit files export `KLAMS_CONFIG`, but `klams-scanner`/`klams-monitor` read `KLAMS_SCANNER_CONFIG`/`KLAMS_MONITOR_CONFIG` (latent since sprint 003) — so the oneshot scanner would fail "`--url or --config required`" and the monitor would crash-loop. Fixed by unifying both binaries on `KLAMS_CONFIG` (`crates/klams-scanner/src/main.rs`, `crates/klams-monitor/src/main.rs`), matching `klams-service` and all three unit files. fmt/clippy/test green; release rebuilt; behavior verified (`KLAMS_CONFIG=… ./klams-{scanner,monitor}` now reads the config path).
- [X] T045 [Defect] **Deployment-surfaced defects (discovered during Phase 4/US2 end-to-end ingestion)** — four scanner + one attribution defect, all fixed:
  1. *Installer host-dep check*: `deploy/install-systemd.sh` pre-flight required host `postgresql.service`, but the DB runs in Docker — changed to check `docker.service` (matches the unit's `After=/Wants=`).
  2. *Backpressure data loss*: `publish_chunk` had no 503 (`queue_full`) retry and `scan_root` advanced the cursor even on publish failure → permanently dropped chunks. Added aggressive 503-aware backoff (`crates/klams-scanner/src/publish.rs`: 2s→60s ×12) + metric `klams_scanner_chunk_retries_total`; `scan_root` now holds the cursor unadvanced on any publish failure so the file retries next scan.
  3. *Multi-root prune cross-deletion*: `scan_root`'s prune loop iterated `cursor.list_all()` (all roots) while `seen` held only the current root → scanning one root deleted the other's knowledge. Guarded with `Path::starts_with(root)`; regression test `prune_is_scoped_to_current_root`.
  4. *Walk traversed dependency/cache trees*: `/home/ken/src` was ~950K files (Python `.venv` site-packages). `walk.rs` now PRUNES via `WalkBuilder::filter_entry` *before* descent and expands `ALWAYS_SKIP` (incl. `.venv`, `venv`, `__pycache__`, `.mypy_cache`, `.pnpm-store`, `.obsidian`, etc.). Added a root `/home/ken/src/.klamsignore` excluding `opc/`, `ai/llama.cpp/`, `ai/ComfyUI/`. Result: cursor 950K→**7,787** first-party files; a no-cursor-reset rescan pruned **14,260** stale chunks from Qdrant (94,152 knowledge chunks remain). Tests: `walk_prunes_python_venv_and_cache_dirs`, `klamsignore_prunes_anchored_nested_subdirs` (8 walk tests green).
  5. *`last_seen_at` not bumped on HTTP writes*: `touch_author_last_seen_at` was called only from MCP tools, so daemons (scanner/monitor) writing over HTTP showed a stale "last seen" in the viewport Authors page. Added `touch_author_last_seen_at` to the `Store` trait (default no-op), implemented on `CompositeStore`, and called fire-and-forget from the knowledge/facts/events HTTP handlers. Verified on `kubs0`: a probe write advanced `last_seen_at` to now. fmt/clippy `-D warnings`/tests green; `klams-service` + `klams-scanner` rebuilt and reinstalled.
  6. *Activity feed not globally newest-first* (also surfaced): root-caused to sectioned pagination + Qdrant scroll ascending order. Out of US4's render-only scope → **deferred** to a future sprint as **kwi #54** (klams/service, bug, M).
- [X] T002 [P] Create `deploy/config/scanner.example.toml` with the shape from [contracts/scanner-config.md](contracts/scanner-config.md) — absolute `roots = ["/home/ken/src", "/home/ken/obsidian"]`, `url`, placeholder `token`, `interval_secs`, `state_dir` (placeholder token only; no real secret)
- [X] T003 [P] Create `deploy/config/monitor.example.toml` with the shape from [data-model.md](data-model.md) §2 — `url`, placeholder `token`, `units` (the looper's watched set), `interval_secs`

**Checkpoint**: Binaries built; committed config examples exist for operators.

---

## Phase 2: Foundational (Blocking Prerequisites)

**Purpose**: Host preconditions that MUST hold before the deployment stories
(US1–US3) can run on `kubs0`. These do **not** block US4/US5.

- [X] T004 Verify `kubs0` host dependencies are present: the database unit the service depends on is active, release binaries are on disk, and the `klams` system user + state/config directories exist (per spec Assumptions). **Verified 2026-06-10**: `klams-service.service` active; healthz reports postgres/qdrant/embeddings all `Ok` (DB runs in Docker — host `postgresql.service` inactive is expected, not a problem); `klams` user exists (uid 998); `/etc/klams/klams.toml` present. Note: scanner/monitor binaries not yet in `/usr/local/bin` (the installer copies them in T010).
- [X] T005 Author `/etc/klams/scanner.toml` on `kubs0` from `deploy/config/scanner.example.toml` with the real scanner bearer and absolute roots per [contracts/scanner-config.md](contracts/scanner-config.md) C1/C3. **Resolved 2026-06-10**: generated a dedicated write-scoped token, added a `[[auth.tokens]]` grant with `agent_name = "klams-scanner"` to `/etc/klams/klams.toml` (backed up first), wrote `/etc/klams/scanner.toml` (absolute roots, `state_dir=/var/lib/klams`, `interval_secs=3600`), perms `root:klams 0640`. Validated config (8 scoped grants), restarted service; journal confirms author bound `klams-scanner` → `019eb25b-ff68-7cc3-b1ac-02ef62e50047`. Secrets never echoed.
- [X] T006 Author `/etc/klams/monitor.toml` on `kubs0` from `deploy/config/monitor.example.toml` with the real monitor bearer and `units` matching the python looper's watched set ([contracts/monitor-parity.md](contracts/monitor-parity.md)). **Resolved 2026-06-10**: generated a dedicated write-scoped token, added a `[[auth.tokens]]` grant with `agent_name = "klams-monitor"` to `/etc/klams/klams.toml`, wrote `/etc/klams/monitor.toml` (`units = ["klams-service.service"]`, `interval_secs=30`), perms `root:klams 0640`. Journal confirms author bound `klams-monitor` → `019eb25b-ff6c-7d53-9070-410904caaab3`. Secrets never echoed.
- [X] T007 Verify the `klams` user can read the scan roots under `ProtectHome=read-only` — if it fails, grant least-broad read access (supplementary group / ACL), NOT a `ProtectHome` relaxation ([research.md](research.md) §R2, [contracts/scanner-config.md](contracts/scanner-config.md) C2). **Resolved 2026-06-10**: R2 materialized (`/home/ken` was `0750 ken:ken`, klams couldn't traverse). Installed `acl`, applied surgical ACLs: `u:klams:--x` on `/home/ken` (traverse only), `u:klams:r-X` recursively on `/home/ken/src` + `/home/ken/obsidian` with default ACLs for inheritance. Verified: klams reads/lists both trees and reads files; cannot list `/home/ken`; default ACL present.
- [X] T008 Verify the scanner `state_dir` resolves to the unit's `StateDirectory=klams` (`/var/lib/klams`) and is owned by the `klams` user so the mtime cursor persists across runs ([contracts/scanner-config.md](contracts/scanner-config.md) C4, [research.md](research.md) §R3). **Verified 2026-06-10**: `/var/lib/klams` exists, `drwxr-xr-x klams:klams`. The example config sets `state_dir = "/var/lib/klams"` explicitly (the binary reads the config value, not `$STATE_DIRECTORY`).

**Checkpoint**: Host is ready — configs in place, `klams` can read the corpus, cursor path durable. US1 can now run.

---

## Phase 3: User Story 1 - Scanner and monitor run under systemd on kubs0 (Priority: P1) 🎯 MVP

**Goal**: Install, enable, and activate `klams-scanner.timer` and
`klams-monitor.service` alongside the running service via the existing
installer, surviving reboot.

**Independent Test**: `systemctl` shows all three klams units; scanner timer
enabled with a next-elapse; monitor active; all survive a host reboot.

- [X] T009 [US1] Dry-run the installer on `kubs0`: `cd deploy && ./install-systemd.sh --dry-run`; confirm it reports the exact install + enable actions for `klams-scanner.timer` and `klams-monitor.service` without changing the host (FR-002). **Verified 2026-06-11**: dry-run prints the full staged action list (user/dirs, stage+rotate binaries, install 4 unit files, `daemon-reload`, `enable --now` for service/scanner.timer/monitor) with no host mutation.
- [X] T010 [US1] Run the installer for real: `sudo ./install-systemd.sh`; confirm all three units install and the scanner timer + monitor service are enabled (FR-001). **Verified 2026-06-11**: installer exits rc=0; binaries in `/usr/local/bin` (prev rotated to `*.prev`), all 4 unit files in `/etc/systemd/system`, `enable --now` applied.
- [X] T011 [US1] Verify unit state: `systemctl is-active klams-monitor.service` (active), `systemctl is-enabled klams-scanner.timer klams-monitor.service`, and `systemctl list-timers | grep klams-scanner` shows a next-elapse (FR-004, FR-005, SC-001). **Verified 2026-06-11**: `klams-service`+`klams-monitor` = active+enabled; `klams-scanner.service` = `static` (timer-driven, correct); `klams-scanner.timer` = enabled with next-elapse 01:02:32 PDT and **last-fired 00:02:32** (proves the hourly timer actually triggers scans).
- [X] T012 [US1] Verify idempotency: re-run `sudo ./install-systemd.sh` and confirm it re-installs/enables without error and without disrupting the running `klams-service.service` (FR-003). **Verified 2026-06-11**: second run exits rc=0; running `klams-service` `MainPID` unchanged (1308180, same `ActiveEnterTimestamp`, `SubState=running`) — `enable --now` is a no-op on an already-active unit, so the live service was not disrupted; monitor stayed active; timer stayed armed.
- [X] T013 [US1] Verify reboot durability: reboot `kubs0`, then confirm the monitor returns to active and the scanner timer re-arms with no manual intervention (FR-004, SC-002). **Verified 2026-06-13** (post-reboot, `uptime` ~1 min, no manual intervention): `docker.service`, `klams-service.service`, `klams-monitor.service`, and `klams-scanner.timer` all came back `enabled/active`; the three datastore containers (`klams-postgres`/`klams-qdrant`/`klams-tei`, `restart=unless-stopped`) returned `running=true`; `klams-scanner.timer` re-armed with a fresh next-elapse (17:52 PDT); and `GET /healthz` returned `200` once the datastores were up. Pre-reboot readiness audit confirmed all units `enabled`, `klams-service` ordered `After=/Wants=docker.service`, and all `/etc/klams/*.toml` at `root:klams 0640`.

**Checkpoint**: All three units managed by systemd and reboot-durable. This is the MVP — everything else is downstream.

---

## Phase 4: User Story 2 - End-to-end ingestion verified (Priority: P1)

**Goal**: Prove a scan cycle populates searchable knowledge memory from
`/home/ken/src` and `/home/ken/obsidian`, idempotently and with source
attribution.

**Independent Test**: A sentinel note dropped before a scan is returned by
klams search within one cycle, with source attribution; a second unchanged
cycle adds no duplicates.

**Depends on**: US1 (scanner installed and runnable).

- [X] T014 [US2] Baseline: record the current `knowledge_items` count and run `memory_search` for a unique sentinel token not yet on disk → expect zero results. **Verified 2026-06-11**: token `klamsSENT1781164223`; baseline Qdrant `points_count` = 94178; `memory_search` returned the usual K results but **0 contained the token** (vector search always returns K, so the meaningful assertion is "no result text contains the sentinel").
- [X] T015 [US2] Drop a sentinel note containing the unique token into `/home/ken/obsidian` and a sentinel file under a scanned `/home/ken/src` path. **Verified 2026-06-11**: created `/home/ken/src/klams-st-<UNIQ>/kept-<UNIQ>.md` + `/home/ken/obsidian/klams-st-<UNIQ>.md` (plus an `…/node_modules/ignored-<IGN>.md` for T018). Confirmed the `klams` user can read both indexable sentinels via the inherited default ACL.
- [X] T016 [US2] Run one scan cycle: `sudo systemctl start klams-scanner.service`; tail `journalctl -u klams-scanner -f` and confirm it walks the roots, chunks/embeds, and exits 0 (FR-006). **Verified 2026-06-11**: scan ran 00:50:57→00:50:59, `roots=2`, `Result=success` rc=0 (fast because the pruned corpus is mostly mtime-cached; only the 2 new sentinels were embedded).
- [X] T017 [US2] Assert ingestion: knowledge-item count increased materially over baseline, and `memory_search "<token>"` returns the sentinel with `source_file` attribution within one cycle (FR-007, FR-010, SC-003, SC-004). **Verified 2026-06-11**: count 94178→94184; search for the token returned **both** sentinels with correct `file=` attribution (`/home/ken/src/klams-st-…/kept-…md` and `/home/ken/obsidian/klams-st-…md`).
- [X] T018 [US2] Verify ignore handling: confirm a `.gitignore`/`.klamsignore`-excluded path under the roots was NOT indexed (FR-008). **Verified 2026-06-11**: the `node_modules/ignored-<IGN>.md` sentinel produced **0** search hits for its token and is **absent from the scanner cursor** (pruned before descent by `ALWAYS_SKIP`).
- [X] T019 [US2] Verify idempotency: run a second scan cycle on the unchanged corpus and confirm no net increase in knowledge-item count attributable to duplication; the scanner reports `mtime_unchanged` skips (FR-009, SC-005, [research.md](research.md) §R3). **Verified 2026-06-11**: second scan (00:51:56→00:51:59, success) left the count unchanged at 94184 — mtime cursor short-circuits unchanged files, no duplicate ingestion.
- [X] T020 [US2] Verify persistence: restart `klams-service.service` and confirm the sentinel knowledge item remains findable via `memory_search` (FR-011). **Verified 2026-06-11**: after `systemctl restart klams-service` (returned active), both sentinels were still findable with attribution. *(Cleanup: sentinel files removed and a prune scan deleted them from Qdrant + cursor — post-cleanup search returns 0 sentinel hits.)*

**Checkpoint**: Knowledge memory is demonstrably live, attributed, idempotent, and durable.

---

## Phase 5: User Story 3 - Retire the legacy python looper (Priority: P1)

**Goal**: Decommission `~/src/tools/ksvc-looper/klams_monitor.py` only after
the Rust monitor demonstrates event parity, with no observability gap.

**Independent Test**: Every transition in a representative parity set produces
a Rust `Service` event; after the looper is stopped, a further transition is
recorded by the Rust monitor alone with no gap or duplicate source.

**Depends on**: US1 (Rust monitor installed). Follows
[contracts/monitor-parity.md](contracts/monitor-parity.md).

- [X] T021 [US3] Open the parity window. **Done 2026-06-11 (T0=2026-06-11T18:06:32Z)**: both monitors confirmed running concurrently (2× `klams_monitor.py` PIDs 764411/1322880 + `klams-monitor.service` active). **FINDING — contract premise invalid**: the python looper does NOT watch systemd units and does NOT emit klams events. It polls `http://kubs0:7777/healthz` every 30s and pushes app-health states (ok/unhealthy/maintenance/down) to **kpidash** ([../../../tools/ksvc-looper/klams_monitor.py](../../../tools/ksvc-looper/klams_monitor.py)). The Rust monitor watches systemd unit state and emits typed `Service` klams events. They observe different signals into different sinks → the "`units` equals the looper's watched set" precondition (data-model §2) and the P1–P3 like-for-like event comparison are **inapplicable**. The two are complementary, not replacements.
- [X] T022 [US3] Drive the representative transition set. **Done 2026-06-11**: because the production watched set is only `klams-service.service` — which is ALSO the monitor's event sink (`POST /memory/events`) — a real Down on it is structurally unrecordable (publish fails during the outage, no retry/buffer; see [../../../crates/klams-monitor/src/main.rs](../../../crates/klams-monitor/src/main.rs#L92)). To demonstrate the pipeline without a service outage, added a transient dummy unit `klams-parity-test.service` to `monitor.toml` (alongside the production unit), restarted the monitor, then drove inactive→active→inactive. Both DRIVEN transitions recorded as typed `Service` events with matching name + kind: `up` @18:07:41 (START @18:07:39) and `down` @18:08:41 (STOP @18:08:19). Window fully torn down: dummy unit removed, `monitor.toml` restored to `["klams-service.service"]`, monitor restarted. **P1 satisfied** for the driven set.
- [X] T023 [US3] Confirm parity criteria. **Done 2026-06-11**: P1 ✅ (every driven transition has a matching Rust `Service` event). P2 ✅ trivially (looper emits no klams events, so no "looper-only" klams event is possible). P3 N/A (no duplicate klams events — the looper's sink is kpidash, not klams). **Two production findings filed:** (a) **kwi #55** monitor cannot record `klams-service`'s own Down (sink self-dependency, M); (b) **kwi #56** all `Service` events carry `host=unknown` (monitor `default_host` reads `$HOSTNAME` which systemd does not pass, S).
- [X] T024 [US3] Retire the looper: stop and decommission `~/src/tools/ksvc-looper/klams_monitor.py`. **DONE 2026-06-13 (live cutover).** The kpidash health-reporting path the looper owned is built into the Rust monitor behind the default-on `kpidash` cargo feature: an optional `[kpidash]` config section makes klams-monitor poll `<url>/healthz` and publish the same `kpidash:services:<name>:<host>` Redis card (identical JSON: `{ts,state,text,host,icon}`). See [../../../crates/klams-monitor/src/kpidash.rs](../../../crates/klams-monitor/src/kpidash.rs). Inert when the section is absent, so a fresh clone without Redis never connects. Putting it in the monitor (not the service) keeps it an external observer that can still report `down`. **Cutover executed:** python looper stopped (operator); `REDISCLI_AUTH` wired via systemd drop-in `/etc/systemd/system/klams-monitor.service.d/10-kpidash.conf` → `EnvironmentFile=/etc/klams/monitor.env` (root:klams 0640); `[kpidash] redis_host="rpi53" name="klams" icon=8` appended to `/etc/klams/monitor.toml`; release binary redeployed (old → `.prev`); `daemon-reload` + restart. Startup log shows `kpidash reporter enabled interval_secs=30`, `NRestarts=0`.
- [X] T025 [US3] Verify clean cutover. **DONE 2026-06-13.** With the looper stopped and the redeployed monitor the sole writer, `kpidash:services:klams:_` refreshes every interval: two reads 30s apart showed `ts` 1781402561 → 1781402591 and text `v0.1.0 up 1h14m` → `up 1h15m`, both `state=ok`, no gaps/duplicates. Monitor unit `active (running)`, `NRestarts=0`; `pgrep klams_monitor.py` returns nothing.

**Checkpoint**: Rust monitor pipeline demonstrated live (driven Up/Down recorded). Python looper **retired 2026-06-13** — its kpidash health card is now produced by the Rust monitor (`[kpidash]` section → `kpidash:services:klams:_` refreshing every 30s). Cutover (T024/T025) complete.

---

## Phase 6: User Story 4 - Author counts render knowledge writes (Priority: P2)

**Goal**: Render the per-author `knowledge` count the API already returns, so
facts and knowledge are both visible and distinct. **Viewport-render-only** —
no backend/store/API/type change ([research.md](research.md) §R1,
[contracts/author-counts-ui.md](contracts/author-counts-ui.md)).

**Independent Test**: An author with indexed knowledge shows a non-zero
knowledge count distinct from writes on both the list and detail surfaces.

**Independent of** US1–US3, US5. (Effect is most visible after US2 produces volume.)

### Tests for US4 (write first — TDD)

- [X] T026 [P] [US4] **Done 2026-06-11**: `viewport/src/routes/authors/page.test.ts` asserts the list renders `counts.knowledge` in a distinct cell (value + label) for a row with `knowledge=42` (U1, FR-015). Pure-data test against the shared `authorCountCells` helper the list now renders from (no DOM testing-library, matching `row.test.ts` convention).
- [X] T027 [P] [US4] **Done 2026-06-11**: `viewport/src/routes/authors/[id]/page.test.ts` asserts the detail summary surfaces `knowledge` alongside `writes`, and that an author with `writes=0, knowledge=9` shows `9` not `0` (U2, FR-015, SC-008).
- [X] T028 [P] [US4] **Done 2026-06-11**: assertion in `page.test.ts` confirms `writes` and `knowledge` are separate labelled cells and no cell carries the summed value (`writes+knowledge`) (U3, FR-016).

### Implementation for US4

- [X] T029 [US4] **Done 2026-06-11**: extracted `viewport/src/routes/authors/counts.ts::authorCountCells` (ordered, distinctly-labelled measures); the Authors list `+page.svelte` now renders its count `<th>`/`<td>` cells from it, including the `Knowledge` cell bound to `a.counts.knowledge` (FR-015, FR-016, U1).
- [X] T030 [US4] **Done 2026-06-11**: the detail `[id]/+page.svelte` Counts summary now renders from the same `authorCountCells` helper, surfacing `knowledge` alongside `writes` (FR-015, FR-016, U2).
- [X] T031 [US4] **Done 2026-06-11**: `pnpm check` (svelte-check) 0 errors/0 warnings; `pnpm test` (vitest) 39 passed incl. the 4 new US4 tests (Constitution III).

**Checkpoint**: Authors surfaces show knowledge counts; kwi #32 ready to close (SC-008).

---

## Phase 7: User Story 5 - bench-clean drains synchronously (Priority: P2)

**Goal**: Verify the already-shipped synchronous bench-clean delete leaves
zero residue under live load, then close the item. **No code change** —
`?wait=true` already in the recipe ([research.md](research.md) §R1).

**Independent Test**: A bench seed + clean leaves zero points for the bench
author with no manual follow-up.

**Independent of** all other stories.

- [X] T032 [US5] **Done 2026-06-11**: seeded a small bench corpus (`./target/release/seed --facts 50 --knowledge 200`, 0 errors) → baseline 50 facts + 200 Qdrant points for author `klams-bench` (`019e76c8-3bb7-7a73-985c-0697269256cf`). Ran `just bench-clean`; confirmed the recipe POSTs `points/delete?wait=true` so the delete blocks until committed (justfile L352). Output: `DELETE 50`, `qdrant: ok` (FR-017).
- [X] T033 [US5] **Done 2026-06-11**: immediately after `bench-clean`, with no manual follow-up drain, residue == 0 everywhere — Postgres `facts=0, events=0` and Qdrant exact `points/count` (author_id filter) = `0` (FR-018, SC-009).

**Checkpoint**: bench-clean drainage confirmed; kwi #33 ready to close (SC-009).

---

## Phase 8: User Story 6 - TokenMaster integration spike (Priority: P3)

**Goal**: Timeboxed exploratory spike validating the TMX↔klams seam against
real indexed data. Ships **no production code** (FR-021).

**Independent Test**: A findings document with an explicit go/no-go exists
under the tokenmaster-integration planning folder, demonstrating the routing
agent recalled real indexed content via klams `memory_search`.

**Depends on**: US1 + US2 (real indexed data to recall).

- [X] T034 [US6] Run TokenMaster against a Python repository (where its graph supplier is proven, not the klams Rust repo) and confirm it builds a usable code graph (FR-019)
- [X] T035 [US6] Wire the TMX routing agent to the klams MCP endpoint; ask a recall-shaped question and confirm it reaches klams `memory_search` and returns real indexed content from US2 (not empty) (FR-020, SC-010)
- [X] T036 [US6] Write a durable finding via klams `memory_add` and confirm it is recallable in a later turn/session (FR-020 acceptance #2)
- [X] T037 [US6] Commit a findings document with an explicit go/no-go recommendation on the "Lightweight graph memory" backlog item under `specs/planning/tokenmaster-integration/` — including the negative-outcome reasoning if the integration looked unviable (FR-021, SC-010)

**Checkpoint**: Spike findings committed with a go/no-go decision.

---

## Phase 9: Polish & Cross-Cutting Concerns

**Purpose**: Close tracked items and update operator-facing docs.

- [X] T038 [P] **Done 2026-06-11**: closed **kwi #32** (status=closed) referencing the US4 render change — both list + detail now show `counts.knowledge` distinctly via the shared `authorCountCells` helper (SC-008).
- [X] T039 [P] **Done 2026-06-11**: closed **kwi #33** (status=closed) referencing the live zero-residue check — seed 50f/200k → `bench-clean` (`?wait=true`) → 0 residue in PG + Qdrant with no manual drain (SC-009).
- [X] T040 [P] **Done 2026-06-11**: updated `docs/setup.md` — corrected the install precondition (`docker.service`, not host `postgresql.service`), fixed the monitor config example to the real keys (`units` / `interval_secs`, was `services` / `poll_interval_secs`), and added an absolute-`roots` note, a `klams` read-access (ACL) note, and the `root:klams 0640` config-perms note (Constitution IV).
- [X] T041 [P] **Done 2026-06-11**: updated `docs/architecture.md` with a "Sprint 010 — ingestion operationalized" subsection (scanner timer + walk pruning, monitor `Service` events + the kwi #55/#56 limitations, `docker.service` dependency). Accurately records that the python looper is a kpidash health reporter **not** replaced by the Rust monitor (retirement deferred), rather than "retired" (Constitution IV).
- [X] T042 [P] **Done 2026-06-11**: updated `docs/usage.md` `/authors` workflow to document the new distinct **Knowledge** count on both surfaces (writes vs knowledge never summed; `writes=0` author shows real knowledge), shared via `counts.ts` (Constitution IV).
- [X] T043 **Done 2026-06-11**: `just gate` green — `cargo fmt --all --check`, `cargo clippy --workspace --all-targets --all-features -D warnings`, and `cargo test --workspace` all pass (EXIT=0, 0 test failures) on stable 1.96.0 (matches CI). Not pushed — landing left to the operator.

---

## Dependencies & Execution Order

### Story completion order

```text
Setup (T001–T003)
   └─▶ Foundational (T004–T008)            ← blocks deployment stories only
          └─▶ US1 (T009–T013)  P1  🎯 MVP
                 ├─▶ US2 (T014–T020)  P1
                 │      └─▶ US6 (T034–T037)  P3   (needs real data)
                 └─▶ US3 (T021–T025)  P1

US4 (T026–T031)  P2   ── independent of US1–US3, US5
US5 (T032–T033)  P2   ── independent of all
Polish (T038–T043)    ── after the stories they reference
```

### Key dependencies

- **US1 → US2 → US6**: ingestion must be live and populated before the spike has data.
- **US1 → US3**: the Rust monitor must be installed before the parity window.
- **US3 cutover gated**: T024 (retire looper) MUST follow T023 (parity P1–P3 shown).
- **US4 TDD**: T026–T028 (tests) precede T029–T030 (implementation).
- **US4/US5 are independent** of the switchover and of each other — parallelizable with US1–US3.

---

## Parallel Execution Examples

**Setup** — all three are independent:

```text
T001 (build binaries) ‖ T002 (scanner.example.toml) ‖ T003 (monitor.example.toml)
```

**US4 tests** — independent test files:

```text
T026 (list page test) ‖ T027 (detail page test) ‖ T028 (facts-vs-knowledge assertion)
```

**Cross-story parallelism** — once Foundational is done, the P2 stories can run alongside the P1 deployment chain:

```text
[ US1 → US2 → US3 ]   ‖   [ US4 render + tests ]   ‖   [ US5 verify ]
```

**Polish** — doc + close-out tasks are independent:

```text
T038 ‖ T039 ‖ T040 ‖ T041 ‖ T042
```

---

## Implementation Strategy

### MVP first

**US1 (T009–T013)** is the MVP: the units that drive everything else are
installed, enabled, active, and reboot-durable. Stop here and the sprint has
already moved klams from "service-only" to "scheduled ingestion capable."

### Incremental delivery

1. **Setup + Foundational** (T001–T008) — build + host readiness (R2 is the load-bearing risk).
2. **US1** (T009–T013) — switchover → **MVP**, independently demonstrable.
3. **US2** (T014–T020) — prove ingestion populates searchable memory.
4. **US3** (T021–T025) — retire the looper after parity.
5. **US4 + US5** in parallel (T026–T033) — close the two tracked items (render + verify).
6. **US6** (T034–T037) — spike on the now-real data.
7. **Polish** (T038–T043) — close kwi items, update docs, gate.

### Scope guardrail

Per [research.md](research.md) §R1, do **not** add backend/store/API tasks for
kwi #32 or #33 — that work shipped in sprint 009. US4 is a viewport render +
tests; US5 is a live verification. Adding backend tasks here would re-implement
shipped code and violate Constitution Principle VI.

---

## Task Summary

- **Total tasks**: 43
- **Setup**: 3 (T001–T003)
- **Foundational**: 5 (T004–T008)
- **US1** (P1, MVP): 5 (T009–T013)
- **US2** (P1): 7 (T014–T020)
- **US3** (P1): 5 (T021–T025)
- **US4** (P2, render-only): 6 (T026–T031, incl. 3 TDD tests)
- **US5** (P2, verify-only): 2 (T032–T033)
- **US6** (P3, spike): 4 (T034–T037)
- **Polish**: 6 (T038–T043)
- **Parallel opportunities**: Setup (3), US4 tests (3), the P1-chain ‖ US4 ‖ US5 fan-out, Polish (5)
- **Suggested MVP scope**: US1 (T009–T013) — switchover demonstrable on its own

### Independent test criteria per story

- **US1**: three units present in `systemctl`; timer next-elapse; monitor active; survive reboot.
- **US2**: sentinel note searchable within one cycle with source attribution; second cycle adds no duplicates.
- **US3**: every parity transition yields a Rust `Service` event; post-cutover transition recorded by the Rust monitor alone.
- **US4**: an author with indexed knowledge shows a non-zero knowledge count distinct from writes, list + detail.
- **US5**: bench seed + clean leaves zero points for the bench author, no manual drain.
- **US6**: committed findings doc with go/no-go, demonstrating real recall via `memory_search`.
