# klams-viewport memory walkthrough

This is the user-facing guide for browsing klams memory from the
Windows desktop app. It maps to the acceptance scenarios in
[viewport.md §4](../../specs/planning/viewport.md) deliverable 4 and
the requirements for User Story 6 in
[spec.md](../../specs/001-initial-mvp/spec.md).

## First-run setup

1. Copy `klams-viewport.exe` (from
   `viewport/target/x86_64-pc-windows-msvc/release/`) to the Windows
   workstation. No installer; it runs in place.
2. Launch it. The header dot will be grey ("connecting") because no
   service URL is configured yet.
3. Click the **⚙** gear in the top-right.
4. Set:
   - **klams URL** — `http://kubs0:7777` (or wherever your service
     lives).
   - **Bearer token** — paste the token issued by the service. It is
     stored in the Windows Credential Manager under
     `klams-viewport / bearer`.
   - **Refresh interval (seconds)** — defaults to `10`.
5. **Save**. The next health poll should turn the dot green ("Ok")
   within `refresh interval` seconds.

The `klams URL` and refresh interval are stored at
`%APPDATA%\klams\viewport.toml`. The bearer token never touches disk.

## Dashboard

The home route shows:

- Service URL and viewport version.
- Last refresh timestamp (updates on every successful health poll).
- Aggregate health (`Ok` / `Degraded` / `Down`) plus per-subsystem
  rows for Postgres, Qdrant, and Embeddings.
- Queue depth, capacity, and worker count.

If the dot turns yellow or red, hover the relevant subsystem row to
see the most recent error message returned by `/healthz`.

> **Screenshot placeholder**: `docs/img/dashboard.png` — capture once
> the viewport is running against a live `kubs0` deployment.

## Facts

`/facts` opens the facts browser.

- **Filters**: type (`UserFact`/`TaskFact`/`EnvFact`), source
  (`User`/`Controller`/`Task`/`AgentProposal`), created-after,
  created-before, limit.
- **Columns**: payload preview, confidence, decay weight, last used,
  use count.
- Click a row to open the detail pane with the full JSON payload and
  a **Copy id** button — handy for pasting into a shell or another
  tool.

> **Screenshot placeholder**: `docs/img/facts.png`.

## Events

`/events` mirrors facts but for the event log.

- **Filters**: `task_id`, `category` (e.g. `agent.activity`),
  created-after, created-before, limit.
- **Columns**: created-at, category, task id, payload preview.
- Detail pane shows the full payload plus a **Copy id** action.

> **Screenshot placeholder**: `docs/img/events.png`.

## Knowledge

`/knowledge` is the semantic search view.

- Type a query, optionally adjust **top k** (default 10), and click
  **Search**.
- Results are ranked by score (descending). A degraded badge appears
  if the knowledge backend returned partial results.
- Click a row to open the full text, tags, and metadata.

> **Screenshot placeholder**: `docs/img/knowledge.png`.

## Connection states

| Header dot | Meaning |
|-----------|---------|
| grey | viewport hasn't received a health snapshot yet |
| green | last `/healthz` returned `Ok` |
| amber | one subsystem is degraded; service still responsive |
| red | `/healthz` returned `Down` or the request failed |

If the dot stays grey for more than a few seconds, check:

1. Settings → klams URL is reachable from the Windows host (firewall,
   VPN).
2. The bearer token matches what the service expects.
3. The service is actually running (`docker ps` on `kubs0`).

Health-poll failures back off exponentially (start = configured
interval, double on each consecutive failure, capped at 60 s) and
reset to the configured interval on the next success — see FR-028.

## Updating the token

Open Settings, paste a new token, click Save. The old token is
overwritten in the Credential Manager. Leaving the token field blank
keeps the existing token.

## Uninstall

- Delete `klams-viewport.exe`.
- Delete `%APPDATA%\klams\viewport.toml` to drop the saved URL.
- In Credential Manager → Windows Credentials, remove the entry whose
  internet/network address is `klams-viewport`.
