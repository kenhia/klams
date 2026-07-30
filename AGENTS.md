# klams — working agreement

Guidance for anyone (human or agent — Claude Code, GitHub Copilot,
local models) making changes in this repo. This file replaces the
retired spec-kit constitution (`.specify/memory/constitution.md`,
removed in sprint 013); the principles below are carried over from it.

klams is purpose-built for Ken's homelab (`kubs0`, `kai`, specific
Postgres/Qdrant layout), but it is installable by strangers (sprint
035, the Operator track). The portability line is drawn like this:

- **Operator-facing surfaces** — defaults, example configs, the
  provision script, the justfile, docs — must be **generic or fail
  loudly**. No Ken-shaped hostname, path, or username may ship as a
  default; Ken-specific values live in rendered config and env, never
  in the repo's defaults. (Redrawn in sprint 035; the old blanket "do
  not generalize" rule predated the generalize area and contradicted
  it.)
- **The architecture stays purpose-built.** Do not grow abstraction
  for hypothetical deployments — no tenancy model, no pluggable
  storage backends, no "for portability" refactors of code paths that
  already carry no environment assumptions.

## Sprint workflow

Work is organized into **sprints**, where a sprint is simply *the work
that fits in one PR* — some are an afternoon, some are substantial.

1. **Pick the next sprint number** (sequential, zero-padded:
   `013`, `014`, …). Create a branch and a sprint directory with the
   same name: branch `###-<short-stub>`, directory
   `sprints/###-<short-stub>/`.
2. **Write intent before code**: open `sprints/###-<short-stub>/sprint.md`
   stating the goal, scope, and acceptance criteria. A few paragraphs
   is fine; heavyweight spec/plan/tasks ceremony is not required.
   At the same time, set the **PATCH segment of the workspace version
   to the sprint number** (`[workspace.package] version` in the root
   `Cargo.toml`; sprint 018 → `0.1.18`). The version surfaces on
   `/healthz` and MCP `server_info` — Ken's dashboard reads it, so
   it's the at-a-glance check that the latest sprint is deployed.
   MAJOR/MINOR stay hand-managed. (Convention started at 018.)
3. **Chronicle as you go**: decisions, surprises, contract changes,
   and outcomes get recorded in markdown inside the sprint directory
   (in `sprint.md` or sibling files — contracts, findings, migration
   notes — as the work warrants). The sprint dir is the durable record
   of *why*, not a scratchpad.
4. **Ship**: gate passes, docs updated, PR merges to `main`, sprint
   doc reflects what actually happened (not just what was planned).

Cross-sprint planning documents live in `sprints/planning/`. Ad-hoc
changes too small for a sprint dir still get a line in the PR
description explaining intent.

## Principles

### Test-Driven Development

TDD is mandatory for new code: write the failing test, make it pass,
refactor green. Tests exist before or alongside the code they
validate; coverage must not decrease; integration tests are required
at cross-component boundaries.

### Code standards gate

Every commit must pass the gate — `just gate`:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

This applies to existing code touched in passing, not just new code —
no broken windows.

**`just gate` is no longer everything CI runs** (sprint 031, #646).
The docker-compose integration stack used to come up on `main` only, so
every integration failure was discovered *after* merge; it now runs on
every branch. Before pushing anything that touches the store, the MCP
tools, or the write paths, also run:

```bash
docker compose -f tests/docker-compose.test.yml up -d   # once
just test-integration
docker compose -f tests/docker-compose.test.yml down     # when you are done
```

**Tear the test stack down when you finish.** Sprint 032 (#647) found
it had been up on kubs0 for two weeks alongside the production
containers, quietly holding a second postgres, qdrant and TEI. It costs
memory and disk and its qdrant accumulates test seeds until ranking
assertions starve — `just test-integration` sweeps that state, but only
for the stack it is about to use.

That recipe sweeps accumulated test state first (a long-lived stack
otherwise drifts until ranking assertions starve) and runs at default
parallelism — the `--test-threads=1` this suite used to need is gone
with the shared-table race behind it (#679). If you find yourself
reaching for it again, something regressed; fix that instead.

### Documentation is part of done

If a change alters how the system is built, configured, or used, the
docs reflect it **within the same sprint**: `README.md` (overview),
`docs/architecture.md` (design), `docs/setup.md` (provisioning),
`docs/usage.md` (operator recipes).

### Quality & observability

- CLI/service output: errors to stderr, results to stdout, `--json`
  where programmatic use is plausible, `NO_COLOR` respected.
- Errors are actionable — what went wrong and what to do about it; no
  raw stack traces outside debug mode.
- Logging via `tracing`: structured, leveled, quiet at default
  verbosity. Exit codes: 0 success, non-zero failure, documented in
  `docs/usage.md`.

### Simplicity (YAGNI)

Every addition must justify its complexity. No features, abstractions,
or config options for hypothetical futures. Prefer explicit over
implicit. Defensive coding at system boundaries only (user input,
external APIs, file I/O) — trust internal code.

## Directory structure

| Directory | Purpose | Tracked |
|-----------|---------|---------|
| `sprints/` | Sprint records (`###-<stub>/`) + `planning/` | Yes |
| `crates/` | Rust workspace crates | Yes |
| `viewport/` | Tauri + SvelteKit desktop UI (own workspace) | Yes |
| `tools/` | Non-shipping ops tooling (bench, soak, repair) | Yes |
| `docs/` | Architecture, setup, usage | Yes |
| `deploy/` | Compose, systemd units, Grafana/Prometheus | Yes |
| `migrations/` | Postgres migrations (sqlx) | Yes |
| `.scratch/` | User scratch space | No |
| `.scratch-agent/` | Agent scratch space | No |

Historical note: `sprints/001`–`012` were authored under GitHub
spec-kit and keep its spec/plan/tasks layout; don't retrofit them.
From 013 onward the lighter workflow above applies.
