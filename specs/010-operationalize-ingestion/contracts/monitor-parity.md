# Contract: Monitor parity window (US3)

**Feature**: `010-operationalize-ingestion`
**Gates**: retiring `~/src/tools/ksvc-looper/klams_monitor.py`

The Rust `klams-monitor` MUST be shown at event parity with the legacy
python looper **before** the looper is stopped. This contract defines
"parity" operationally so the cutover is deliberate and gap-free
(FR-013, FR-014, SC-006, SC-007).

## Preconditions

- `klams-monitor.service` installed + active (US1).
- `/etc/klams/monitor.toml` `units` MUST equal the looper's watched set
  (so the comparison is like-for-like — [../data-model.md](../data-model.md) §2).
- Both monitors running concurrently for the duration of the window.

## Procedure

1. Record the start timestamp `T0`.
2. Drive a **representative transition set** over the watched units:
   for each unit, at least one `active→inactive` (Down) and one
   `inactive→active` (Up); include one version change if feasible.
3. For each transition, confirm the Rust monitor recorded a typed
   `Service` event with the correct `name` and `kind`
   (Up / Down / VersionChanged) via `event_search` (or the Activity
   surface) since `T0`.

## Parity criterion (MUST hold before cutover)

- **P1** Every transition driven in step 2 has a corresponding Rust
  `Service` event with matching unit name and kind.
- **P2** No transition produced **only** a looper event and no Rust event.
- **P3** Duplicate events for the same transition (one looper, one Rust)
  during the window are EXPECTED and MUST be recognizable as
  window-artifacts, not steady state.

## Cutover (only after P1–P3)

4. Stop and decommission the python looper.
5. Drive one further transition; confirm it is recorded by the Rust
   monitor **alone**, with no gap and no duplicate-source event (SC-007).

## MUST NOT

- MUST NOT stop the looper before P1–P3 are demonstrated (FR-013) —
  service observability must never be interrupted.
- MUST NOT leave both monitors running past the window (double-reporting
  muddies attribution).
