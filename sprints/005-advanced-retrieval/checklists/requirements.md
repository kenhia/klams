# Specification Quality Checklist: Advanced Retrieval and Summarization

**Purpose**: Validate specification completeness and quality before proceeding to planning  
**Created**: 2026-05-20  
**Feature**: [spec.md](../spec.md)

## Content Quality

- [x] No implementation details (languages, frameworks, APIs)  
  *Note: API endpoint paths (`POST /memory/context`, `/memory/search`) and store names (Postgres, Qdrant) are referenced because they are pre-existing system surface from sprints 001–003 — they describe **what already exists** that this feature plugs into, not implementation choices made by this spec. Library/algorithm choices (RRF vs cross-encoder, tiktoken vs char-count, LLM vs extractive) are deferred to clarification, not embedded in requirements.*
- [x] Focused on user value and business needs
- [x] Written for non-technical stakeholders (Ken in his ops/architect hat; the master plan is the audience)
- [x] All mandatory sections completed

## Requirement Completeness

- [ ] No [NEEDS CLARIFICATION] markers remain  
  *Three markers present, all flagged as Phase 4 planning concerns by `sprints/planning/plan.md` §9 or by the deliverable scope. Resolve before `/speckit.plan`.*
- [x] Requirements are testable and unambiguous
- [x] Success criteria are measurable
- [x] Success criteria are technology-agnostic (no implementation details)
- [x] All acceptance scenarios are defined
- [x] Edge cases are identified
- [x] Scope is clearly bounded (the dedupe/decay-weight backlog item is explicitly out of scope; no new data stores)
- [x] Dependencies and assumptions identified

## Feature Readiness

- [x] All functional requirements have clear acceptance criteria
- [x] User scenarios cover primary flows
- [x] Feature meets measurable outcomes defined in Success Criteria
- [x] No implementation details leak into specification

## Open Clarifications (must resolve before `/speckit.plan`)

1. **FR-002 — Token-cost heuristic.** Cheap `chars / N` (no model dependency) vs `tiktoken cl100k_base` (matches OpenAI/Anthropic chat tokenizers used by MCP clients) vs configurable. Affects whether klams takes a new dependency and whether budgets are portable across model families. Plan §9 lists this as a Phase 4 planning concern.
2. **FR-005 — Hybrid retrieval fusion strategy.** Reciprocal rank fusion (cheapest, no score normalization) vs weighted score blending (configurable, needs normalization) vs two-stage retrieve-then-rerank with a cross-encoder (highest quality, adds a model dependency). Drives latency budget and ops surface area.
3. **FR-010 — Summarization mechanism.** Extractive/rule-based (no model dependency, ships fast, low prose quality) vs local LLM via Ollama on `kubs0`'s GPU (better quality, ops dependency) vs hybrid (extractive for events, LLM for knowledge clusters). Drives the biggest single architectural decision in this sprint.

## Notes

- Items marked incomplete require spec updates before `/speckit.clarify` or `/speckit.plan`.
- The three open clarifications above are intentional: they are decisions the master plan flagged for Phase 4 planning, not omissions. Spec-quality items not gated on them are all green.
- Dedupe/decay-weight backlog item (Phase-7) is explicitly out of scope (Assumptions section).
