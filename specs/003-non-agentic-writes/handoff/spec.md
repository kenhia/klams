# Feature Specification: klams integration in ansible-k

**Input**: ansible-k must persist a queryable record of "what was
true on each host after this play" by streaming facts and events to
the klams memory service over its sprint-003 HTTP surface.

## User Scenarios & Testing

### User Story 1 — Callback plugin posts per-host facts (Priority: P1)

As an operator running an ansible-k play against multiple hosts, I
want every host's gathered facts (kernel, distro, hostname, package
manager state) and every task's `Service`-category result to land in
klams so that, after the play, `klams search "kubs0 kernel"` returns
the current truth without me having to grep through `journalctl`.

**Independent test**: run a play with a single `setup` module against
`kubs0`. After the play succeeds, `curl
$KLAMS_URL/memory/search?...` (or the `klams` CLI when it ships)
returns a row whose `payload.host == "kubs0"` and `payload.kernel`
matches the play output. Repeat the same play within 60 seconds —
no new versions appear, only `last_used_at` advances.

## Requirements

- **FR-1**: The callback plugin MUST post one `UserFact` row per
  host per play, containing at minimum `{name, host}` (the operator
  name and target hostname).
- **FR-2**: The callback plugin MUST post one `EnvFact` row per
  host per play, containing at minimum `{host, kernel, distro}` from
  Ansible's gathered facts.
- **FR-3**: The callback plugin MUST post one `Event(category=Service)`
  row per task whose module is `systemd`, `service`, or `ansible.builtin.service`,
  containing `{service, host, state, version?}`.
- **FR-4**: A klams 5xx, network failure, or auth rejection MUST NOT
  fail the play. The plugin SHOULD log at WARNING level and continue.
- **FR-5**: A klams 422 (validation error) MUST be logged at ERROR
  level with the offending payload so the operator can fix the
  plugin code. Retrying is not allowed.
- **FR-6**: A klams 200 / 202 response MUST be debug-logged with the
  returned `path` field so dedupe behavior is observable.
- **FR-7**: The plugin MUST probe `GET /healthz?contract=v1` on play
  start. If the probe does not return `{"contract":"v1"}`, the
  plugin MUST disable itself for the rest of the play (no posts) and
  log a single WARNING. The probe MUST be repeated on every play
  start.
- **FR-8**: Bearer token MUST be read from `/etc/ansible/klams.token`
  (mode 0600, owned by the deploy user). The token MUST NOT be
  printed in plugin output.
- **FR-9**: Posts MUST be issued asynchronously where the plugin
  hook permits it (e.g. `runner_on_ok`), bounded by a 2-second
  per-call timeout. Synchronous fallback is acceptable for hooks that
  don't support async.

## Success Criteria

- **SC-1**: After a play against N hosts, `klams search "host:<H>
  kernel"` returns exactly one row per host with the play's gathered
  kernel value. Measured by an integration test in the ansible-k
  repo.
- **SC-2**: Re-running the same play within 60 seconds adds zero
  new fact versions. Measured by querying `version` before and after.
- **SC-3**: A deliberate klams outage (stop `klams-service`) during
  a play does not change the play's exit code. Measured by a chaos
  test in the ansible-k repo.
- **SC-4**: The plugin adds <2 % wall-clock overhead to a 10-host
  play. Measured by `time ansible-playbook` with and without the
  plugin enabled.

## Assumptions

- klams sprint-003 (this repo, this branch) is deployed on the
  target operations host and reachable over HTTPS (or local HTTP
  when ansible-k is colocated, e.g. on `kubs0`).
- The bearer token has already been provisioned out-of-band — see
  [api-contract.md § Auth model](api-contract.md#auth-model).
- ansible-k's Python runtime has `requests` (or equivalent) available
  on the controller. No new system dependencies are introduced.
- Drift detection via `GET /healthz?contract=v1` is the **only**
  klams-side contract guarantee. Anything not in
  [api-contract.md](api-contract.md) is subject to change without
  notice.
