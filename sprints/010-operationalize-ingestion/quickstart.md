# Quickstart: Operationalize Ingestion (operator runbook)

**Feature**: `010-operationalize-ingestion` | **Host**: `kubs0`
**Plan**: [plan.md](plan.md) | **Research**: [research.md](research.md)

This is the operator walkthrough for the sprint. It is ordered by the
story dependency chain (US1 → US2 → US3, then US4/US5 in parallel, then
US6). Stories 1–3 are **live-host operations**; their "tests" are the
acceptance probes below.

> Most of this sprint is deployment + verification of code that shipped
> CI-green in sprint 003. Two bug-fix stories (kwi #32, #33) are largely
> already shipped — see [research.md](research.md) §R1.

## Prerequisites

- `klams-service.service` active on `kubs0` (already true).
- Release binaries built: `cargo build --release` (produces
  `target/release/klams-{service,scanner,monitor}`).
- Postgres + Qdrant + TEI up (the service already depends on them).

## US1 — Install scanner + monitor under systemd

1. **Author configs** (do NOT commit real tokens):
   - `/etc/klams/scanner.toml` per
     [contracts/scanner-config.md](contracts/scanner-config.md) —
     **absolute** roots `/home/ken/src`, `/home/ken/obsidian`.
   - `/etc/klams/monitor.toml` with `units` = the python looper's watched
     set ([contracts/monitor-parity.md](contracts/monitor-parity.md)).
2. **Verify `klams` can read the trees** (research.md §R2):
   ```sh
   sudo -u klams test -r /home/ken/src && sudo -u klams test -r /home/ken/obsidian && echo ok
   ```
   If this fails, grant least-broad read access before proceeding.
3. **Dry-run the installer**, then run it for real:
   ```sh
   cd deploy
   ./install-systemd.sh --dry-run     # review actions (FR-002)
   sudo ./install-systemd.sh          # install + enable all three units
   ```
4. **Confirm unit state** (SC-001):
   ```sh
   systemctl is-active klams-monitor.service        # active
   systemctl is-enabled klams-scanner.timer klams-monitor.service
   systemctl list-timers | grep klams-scanner       # shows next-elapse
   ```
5. **Reboot durability** (SC-002): reboot `kubs0`, re-run step 4 —
   monitor active, timer re-armed, no manual action.

## US2 — Verify ingestion end-to-end

1. **Baseline** the knowledge count and a negative search:
   - record current `knowledge_items` count;
   - `memory_search` a unique token that is not yet on disk → expect 0.
2. **Drop a sentinel note** containing that unique token into
   `/home/ken/obsidian` (and a sentinel file under a scanned `/home/ken/src`
   path).
3. **Run one cycle**: `sudo systemctl start klams-scanner.service`;
   watch `journalctl -u klams-scanner -f`.
4. **Assert** (SC-003, SC-004, FR-006…FR-011):
   - knowledge count increased;
   - `memory_search "<token>"` returns the sentinel with `source_file`
     attribution;
   - an ignored path did not get indexed (FR-008);
   - run a second cycle unchanged → no net duplicate growth (SC-005).

## US3 — Retire the python looper (parity-gated)

Follow [contracts/monitor-parity.md](contracts/monitor-parity.md):

1. With both monitors running, drive Up/Down/version transitions on the
   watched units.
2. Confirm each produced a Rust `Service` event (P1–P3).
3. **Only then** stop + decommission
   `~/src/tools/ksvc-looper/klams_monitor.py`.
4. Drive one more transition → recorded by the Rust monitor alone, no gap
   (SC-007).

## US4 — Surface the knowledge count in the viewport (kwi #32)

Render-only ([contracts/author-counts-ui.md](contracts/author-counts-ui.md)):

1. Write the vitest assertions first (list + detail render
   `counts.knowledge`; facts vs knowledge distinct).
2. Add the `Knowledge` cell to `authors/+page.svelte` and the detail
   summary in `authors/[id]/+page.svelte`.
3. `just gate` + `svelte-check` green; close kwi #32 (SC-008).

## US5 — Verify bench-clean drains (kwi #33)

Already shipped (`?wait=true` in the recipe); verify and close:

```sh
# seed bench corpus, then:
just bench-clean
# confirm zero residual points for the bench author:
curl -sS "$QDRANT_URL/collections/knowledge_items/points/count" \
  -H 'Content-Type: application/json' \
  -d '{"filter":{"must":[{"key":"author_id","match":{"value":"<bench-author-id>"}}]},"exact":true}'
# expect count == 0  (SC-009) → close kwi #33
```

## US6 — TokenMaster spike (after US1+US2)

1. Run TokenMaster (`/token-master`) against a **Python** repo (graphify
   is proven there; sparse on Rust) — confirm it builds a usable graph.
2. Point the TMX routing agent at the klams MCP endpoint; ask a
   recall-shaped question → confirm it uses `memory_search` and returns
   real indexed content from US2.
3. Write a durable finding via `memory_add`; confirm later recall.
4. Commit findings + a go/no-go on "Lightweight graph memory" under
   [../planning/tokenmaster-integration/](../planning/tokenmaster-integration/README.md)
   (SC-010). Ships no production code.

## Definition of done (this sprint)

- All three units installed/enabled/active and reboot-durable (SC-001/2).
- Knowledge memory demonstrably populated; sentinel searchable within one
  cycle (SC-003/4); idempotent re-scan (SC-005).
- Python looper retired after parity (SC-006/7).
- Viewport shows knowledge counts; kwi #32 closed (SC-008).
- bench-clean verified zero-residue; kwi #33 closed (SC-009).
- TokenMaster findings + go/no-go committed (SC-010).
- `docs/{setup,architecture,usage}.md` updated; `just gate` green.
