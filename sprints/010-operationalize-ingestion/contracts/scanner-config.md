# Contract: Scanner config + deployment (US1, US2)

**Feature**: `010-operationalize-ingestion`
**Files**: `/etc/klams/scanner.toml` (deployed), `deploy/config/scanner.example.toml` (committed example)

This contract fixes the deployed scanner configuration and the host
preconditions that make ingestion actually walk Ken's trees. It exists
because the shipped defaults (`~/src`, `~/obsidian`) are wrong for a unit
running as `User=klams` (see [../research.md](../research.md) §R2).

## Config shape (existing `Config`, unchanged code)

```toml
# /etc/klams/scanner.toml
url           = "http://127.0.0.1:7777"
token         = "<scoped scanner bearer>"
roots         = ["/home/ken/src", "/home/ken/obsidian"]   # absolute, NOT ~
interval_secs = 3600
state_dir     = "/var/lib/klams"                          # aligns with StateDirectory=klams
```

## MUST

- **C1** `roots` MUST be absolute paths. `~` is forbidden — it expands to
  the `klams` user's home, not `/home/ken`.
- **C2** The `klams` system user MUST be able to read every configured
  root under the unit's `ProtectHome=read-only` sandbox. If `/home/ken`
  permissions (`0700`) block traversal, the deployment MUST grant read
  access by the least-broad mechanism (supplementary group / ACL /
  `ReadWritePaths`-free `ReadOnlyPaths=`) rather than disabling
  `ProtectHome`.
- **C3** `token` MUST resolve to a **registered author** so ingestion
  writes are attributable on the per-author surface (feeds US4 counts).
- **C4** `state_dir` MUST resolve to a path the `klams` user owns across
  runs — the systemd `StateDirectory=klams` (`/var/lib/klams`) — so the
  mtime cursor persists and re-scans stay idempotent (FR-009).

## Acceptance probes

| Probe | Expected | Maps to |
|-------|----------|---------|
| `sudo -u klams test -r /home/ken/src && echo ok` | `ok` | C2 |
| `systemctl start klams-scanner.service; journalctl -u klams-scanner -n 50` | walks roots, posts chunks, exits 0 | FR-006, US2 |
| Drop sentinel note in `/home/ken/obsidian`, run one cycle, `memory_search` the token | sentinel returned with `source_file` attribution | SC-004, FR-010 |
| Run two consecutive cycles on an unchanged corpus | second cycle reports `mtime_unchanged` skips; no net knowledge growth | SC-005, FR-009 |
| Confirm a `.gitignore`/`.klamsignore`-excluded path | not present as a knowledge item | FR-008 |

## MUST NOT

- MUST NOT weaken `klams-scanner.service` hardening beyond the minimum
  needed for C2 (no blanket `ProtectHome=` removal).
- MUST NOT commit the real bearer token; `deploy/config/scanner.example.toml`
  carries a placeholder only.
