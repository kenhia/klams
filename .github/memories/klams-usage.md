# klams memory usage

Guard: only applies in workspaces where `mcp_klams_*` tools are
available. If the tools are not present, ignore this file entirely —
do not attempt to install, configure, or invoke them.

## When to search

- Debugging an unfamiliar error or behavior — search before guessing.
- Researching a topic that smells like prior work (auth flows, build
  pipelines, config schemas, integration quirks).
- Before designing something non-trivial — check whether a decision
  has already been made and recorded.

## When to write

- Sprint ship (the sprint-ship skill records the milestone event).
- A surprising root cause that took more than a few minutes to find
  (knowledge memory — tag with the affected component).
- A non-obvious config invariant, footgun, or version-specific quirk.
- A security-relevant decision (token scopes, allowlists, middleware
  ordering).

## When NOT to write

- Per-commit or per-task event spam.
- Restating what a comment, doc, or spec already captures.
- Routine progress updates ("started X", "finished Y").

## When NOT to search

- Every new user prompt (don't pre-load context speculatively).
- Topics already fully covered by files in the current workspace.

## Author identity

Always call `mcp_klams_register_author` once per session before any
write — it returns the author UUID used for attribution.
