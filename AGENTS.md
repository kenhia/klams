# klams — working agreement

Guidance for anyone (human or agent — Claude Code, GitHub Copilot,
local models) making changes in this repo. This file replaces the
retired spec-kit constitution (`.specify/memory/constitution.md`,
removed in sprint 013); the principles below are carried over from it.

klams is purpose-built for Ken's homelab (`kubs0`, `kai`, specific
Postgres/Qdrant layout). It is not a general-purpose system; do not
generalize paths, hostnames, or assumptions "for portability."

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

Every commit must pass the gate — `just gate` runs exactly what CI
runs:

```bash
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

This applies to existing code touched in passing, not just new code —
no broken windows.

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
