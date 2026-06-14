# Contracts — Operationalize Ingestion

**Feature**: `010-operationalize-ingestion`

klams exposes no new HTTP/MCP surface this sprint — the scanner and
monitor are existing clients of the existing API, and the viewport change
is render-only. The "contracts" here are therefore the **deployment and
verification interfaces** that the stories must satisfy on the live host,
plus the one UI render contract for kwi #32.

| Contract | Covers | Stories |
|----------|--------|---------|
| [scanner-config.md](scanner-config.md) | `/etc/klams/scanner.toml` shape + root/path/permission rules | US1, US2 |
| [monitor-parity.md](monitor-parity.md) | the parity-window procedure that gates retiring the python looper | US3 |
| [author-counts-ui.md](author-counts-ui.md) | viewport render of `AuthorCounts.knowledge` (backend already supplies it) | US4 |

kwi #33 (US5) has no contract beyond the existing `bench-clean` recipe —
it is verified, not changed (see [../research.md](../research.md) §R1).
The TokenMaster spike (US6) produces a findings document, not a code
contract.
