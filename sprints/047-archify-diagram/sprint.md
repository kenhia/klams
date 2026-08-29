# Sprint 047 — archify architecture diagram in docs/diagrams

**Proposal** korg:1703 · **Work item** korg #1702 · **Branch**
`047-archify-diagram`

Docs only. No crate changed, nothing was deployed.

## Goal

Make the archify-generated runtime-architecture diagram shareable outside
the homelab: give it a home in the repo, record where it came from and
how to reproduce it, and repair the one sentence elsewhere in the docs
that adding it made untrue.

## Why `docs/diagrams/`, and not the two obvious alternatives

**`kai:~/docs`** — a new personal docs directory on the homelab. Rejected
on the requirement rather than on taste: it is tailnet-only, so the
people the diagram is meant for cannot reach it at all. It would also be
untracked, unreviewed, and detached from the code it describes, which is
the usual way documentation goes stale without anyone noticing.

**`docs/archify/`** — a directory named for the tool that made the
contents. Rejected because directories are named for what they hold. The
name becomes a lie the first time a different renderer is used, and it
splits diagrams across two homes for no gain.

`docs/diagrams/` is already the diagram home, and this repo already ships
self-contained HTML rendered through `htmlpreview.github.io` in two
places — `docs/pitch/klams-pitch.html` and `docs/sharing/pitch.html`. No
GitHub Pages site is configured, and none is needed. The pattern was
already here; this sprint only used it.

## What landed

| Path | |
|---|---|
| `docs/diagrams/klams.architecture.json` | new — 8.7 KB spec, the source of truth |
| `docs/diagrams/klams-architecture.html` | new — 739 KB generated artifact |
| `docs/diagrams/README.md` | new — which files are hand-drawn, which are generated, provenance, re-render command, caveats |
| `docs/architecture.md` | the §1 parenthetical no longer claims every diagram here is hand-authored; links the interactive map |
| `README.md` | links the interactive map beside the existing pitch link |

Delivery receipt, reproduced from the committed spec: 9/9 checks,
`showcase` profile, 0 errors, 0 warnings, evidence verified across 9
repository references. Both files were transferred to `kubs0` by
base64-over-ssh and their sha256 verified against the receipt after
landing — `f963a405…` for the spec, `53886dab…` for the artifact.

## Decisions

### No workspace version bump — deliberate

AGENTS.md says to set the workspace version PATCH to the sprint number at
sprint start, because `/healthz` and MCP `server_info` surface it and
that is the at-a-glance check that the newest sprint is deployed.

This sprint ships no code and deploys nothing. Bumping to `0.1.47` would
make the dashboard advertise a version that is not running, which breaks
exactly the signal the convention exists to provide. The version stays at
`0.1.46`. `0.1.47` will simply never exist; sprint 048 bumps to
`0.1.48`.

The convention is about deployed code. A docs sprint is outside its
subject, not an exception to it.

### The spec is committed, not just the artifact

The 8.7 KB JSON is reviewable in a diff and is what a future change
edits; the 739 KB HTML is a build output that happens to also be the
deliverable. Committing both means the artifact is shareable *and*
reproducible. Committing only the HTML would have made the next edit a
hand-edit of generated markup — the failure this repo avoids elsewhere by
keeping generated validators next to their generator.

### Abbreviated ship

No `cargo` gates, and the `.sprint-deploy` `deploy-kubs0` phase was
skipped: no binary changed, so there is nothing to build or restart.
Requested explicitly.

## Acceptance

- [x] Committed spec and artifact match the delivery receipt hashes.
- [x] `docs/architecture.md` no longer asserts that every diagram in
      `docs/diagrams/` is hand-authored.
- [x] Provenance is recorded where someone will find it — beside the
      files, not only in this sprint doc.
- [x] The htmlpreview URL renders the committed HTML. Verified after
      merge: the page loads and is fully interactive at 739 KB — guided
      views, legend, cards and viewer controls all present, theme
      following the browser. `raw.githubusercontent` serves the artifact
      at the receipt's sha256 (`53886dab…`, 739232 bytes). The size
      concern did not materialise.

## Post-merge notes

- **The squash subject missed the house style.** It landed as `docs: add
  the archify runtime-architecture diagram (sprint 047) (#51)` rather
  than `docs(047): …`. The PR title *was* corrected to the house form
  before merging, but this repo's squash setting takes the message from
  the single commit rather than the PR title, so the correction had no
  effect. Left alone — rewriting `main` to fix a subject line is not
  worth a force-push. For 048: put the `type(NNN):` prefix on the
  **commit**, not only on the PR title, whenever the branch has one
  commit.
- No deploy phase ran, so there is no `docs(NNN): record the deploy`
  commit for this sprint. This note is its equivalent.

## Follow-ups, not done here

- If htmlpreview struggles at 739 KB, the fallback is a GitHub Pages site
  for `docs/` — which would also improve the two existing pitch links.
  Not worth doing speculatively.
- The diagram is pinned to `ceffb6c6`. It has no mechanism that notices
  when the architecture moves past it; the revision in the page is a
  passive signal a reader has to look at. A CI check comparing that
  revision against `main` would make it active. Deferred — the diagram
  would need to be regenerated more than once for that to pay off.
- archify itself is worth a second look for the other tool repos (korg,
  kfdc, kfo). Its `--repo-root` evidence receipt pins one repo, so a
  cross-repo picture and a pinned per-repo picture are different
  artifacts. Not klams' problem; noted so the evaluation is not redone
  from scratch.
