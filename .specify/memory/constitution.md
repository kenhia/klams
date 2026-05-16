<!-- Sync Impact Report
Version change: N/A → 1.0.0 (initial ratification)
Added principles:
  - I. Spec-Driven Development (SDD)
  - II. Test-Driven Development (TDD)
  - III. Code Standards Gate
  - IV. Documentation
  - V. Quality & Observability
  - VI. Simplicity & Intentional Design
Added sections:
  - Core Principles
  - Directory Structure
  - Pre-Commit Checks
  - Development Workflow
  - Governance
Removed sections: none
Templates requiring updates:
  - .specify/templates/plan-template.md ✅ no changes needed
  - .specify/templates/spec-template.md ✅ no changes needed
  - .specify/templates/tasks-template.md ✅ no changes needed
Follow-up TODOs: none
-->

# klams Constitution

## Core Principles

### I. Spec-Driven Development (SDD)

All changes MUST be documented in `/specs/` before implementation begins. Iteration-scoped changes live in their spec directory (e.g., `/specs/001-feature-name/spec.md`). Ad-hoc changes that fall outside an active spec MUST be added to the current spec or to `/specs/supplemental-spec.md`.

- No code change without a corresponding spec entry.
- Specs define acceptance criteria before implementation starts.
- Spec updates are part of the definition of done for every iteration.

### II. Test-Driven Development (TDD)

TDD is mandatory for all new code changes. The Red-Green-Refactor cycle MUST be followed:

1. Write a failing test that captures the requirement.
2. Implement the minimum code to make the test pass.
3. Refactor while keeping tests green.

- Tests MUST exist before or alongside the code they validate.
- Test coverage MUST NOT decrease with new changes.
- Integration tests are required for cross-component boundaries.

### III. Code Standards Gate

All code MUST pass the following checks before commit:

1. **Formatted** — `cargo fmt` produces no diff.
2. **Linted** — `cargo clippy` reports no errors or warnings.
3. **Type-checked** — `cargo check` passes clean.
4. **Unit tests** — `cargo test` passes with no failures.

The CI variant of each check (strict/non-interactive) MUST pass clean. This applies to both new and existing code — no broken windows.

See [Pre-Commit Checks](#pre-commit-checks) for the full command set.

### IV. Documentation

Each iteration (spec/sprint) MUST update user-facing documentation:

- **README.md** — project overview and getting started
- **docs/architecture.md** — technical design and component relationships
- **docs/setup.md** — build prerequisites and installation steps
- **docs/usage.md** — usage guide updated as features become user-facing

Documentation updates are part of the definition of done for every iteration, not a follow-up task. If a feature changes how the system is built, configured, or used, the docs MUST reflect that before the iteration is complete.

### V. Quality & Observability

User experience MUST be consistent across all interfaces. Specific standards:

- **CLI output**: Consistent formatting; errors to stderr, results to stdout; `--json` output available for programmatic use; `NO_COLOR` respected.
- **Error messages**: Actionable — tell the user what went wrong and what they can do about it. Never expose raw stack traces in non-debug mode.
- **Logging**: Structured, leveled (`tracing` crate), and sufficient for debugging without being noisy at default verbosity.
- **Exit codes**: 0 for success, non-zero for failure; documented in `docs/usage.md`.

### VI. Simplicity & Intentional Design

Every addition MUST justify its complexity. YAGNI applies:

- Do not add features, abstractions, or configuration options for hypothetical future requirements.
- Prefer explicit over implicit behavior.
- Start with the simplest approach that meets the spec; refactor only when measured need arises.
- Defensive coding at system boundaries only (user input, external APIs, file I/O). Trust internal code and framework guarantees.

## Directory Structure

| Directory | Purpose | Git tracked |
|-----------|---------|-------------|
| `.scratch-agent/` | Temporary workspace for agent use | No (`.gitignore`) |
| `.scratch/` | Temporary workspace for user use | No (`.gitignore`) |
| `docs/` | Project documentation (architecture, setup, usage) | Yes |
| `specs/` | Iteration and supplemental specifications (SDD) | Yes |
| `src/` | Rust source code | Yes |
| `tests/` | Integration tests | Yes |

## Pre-Commit Checks

### Rust

```bash
# Standard
cargo fmt
cargo clippy --all-targets --all-features
cargo check
cargo test

# CI variant (must pass clean before commit)
cargo fmt --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test
```

## Development Workflow

1. **Spec** — Define or update the spec (`/specs/`).
2. **Plan** — Create implementation plan from spec.
3. **Implement** — Follow TDD; write tests first, then code.
4. **Check** — Run pre-commit checks (format, lint, type, test).
5. **Document** — Update `docs/` as needed.
6. **Review** — Verify constitution compliance before commit.

Ad-hoc changes follow the same workflow but reference `/specs/supplemental-spec.md` instead of a feature spec.

## Governance

This constitution supersedes all other development practices for the klams project. All code changes, reviews, and architectural decisions MUST verify compliance with these principles.

**Amendment procedure**:
1. Propose the change with rationale.
2. Document the amendment in this file.
3. Update the version number per semantic versioning:
   - **MAJOR**: Principle removal or backward-incompatible redefinition.
   - **MINOR**: New principle or materially expanded guidance.
   - **PATCH**: Clarifications, wording, or typo fixes.
4. Update `LAST_AMENDED_DATE`.
5. Propagate changes to dependent templates and documentation.

**Compliance review**: Every commit MUST pass the Code Standards Gate. Architecture and spec alignment are verified during iteration polish.

**Version**: 1.0.0 | **Ratified**: 2026-05-16 | **Last Amended**: 2026-05-16
