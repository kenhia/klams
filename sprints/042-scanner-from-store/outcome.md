# Sprint 042 — outcome

Measured 2026-08-05/06 on kubs0 and kai. All five acceptance criteria met.

## 1. `just publish` puts 0.1.42 in the store

```
==> publishing klams 0.1.42 (x86_64-linux) to kubsdb
published artifacts/klams-service/0.1.42/klams-service-x86_64-linux
published artifacts/klams-service/0.1.42/install-from-store.sh
latest -> 0.1.42
published artifacts/klams-scanner/0.1.42/klams-scanner-x86_64-linux
published artifacts/klams-scanner/0.1.42/install-from-store.sh
latest -> 0.1.42
published artifacts/klams-monitor/0.1.42/klams-monitor-x86_64-linux
published artifacts/klams-monitor/0.1.42/install-from-store.sh
latest -> 0.1.42
```

klams is the store's second `artifacts/` consumer (after kaed) and its
first via a repo's own `just publish` — kaed's was a hand-run fetch.

## 2 & 3. kai, with no checkout, went 0.1.28 → 0.1.42

`just deploy-remote kai klams-scanner`, run from kubs0:

```
installing klams 0.1.42 (x86_64-linux) from https://kubsdb…:4880
==> klams-scanner-x86_64-linux
    checksum OK
    reports 0.1.42
+ rotating /usr/local/bin/klams-scanner (0.1.28) -> /usr/local/bin/klams-scanner.prev
```

kai afterwards:

| | Before | After |
|---|---|---|
| `klams-scanner --version` | 0.1.28 | **0.1.42** |
| `klams-scanner.prev --version` | — | 0.1.28 |
| version floor (`min_version`) | 0.1.41 — **13 releases behind** | above the floor |

Thirteen releases of drift, closed in one command, on a host that has
never had a klams checkout and still doesn't.

The binary was then exercised, not just installed —
`sudo systemctl start klams-scanner.service`:

```
INFO klams_scanner: klams-scanner starting roots=1 interval_secs=3600 once=true host=kai …
INFO klams_scanner: cleared stale chunks before reindex path=… deleted=15
klams-scanner.service: Deactivated successfully.
```

It scanned and wrote to the service on kubs0. `is-failed` → `inactive`.

## 4. The advisory is gone

`~/k-homelab/bin/audit kai`, the check that has been reporting this
since sprint 013:

```
- klams-scanner: ok
```

Previously:

> `advisory: klams-scanner 0.1.28 is older than the declared floor
> 0.1.41 (fix: deploy from the klams repo — there is no automated path
> to kai, klams #836 …)`

k-homelab's recipe was not touched. The other advisories in that audit
run (`tailscale-serve :8100`, `github-repos`) are unrelated and
pre-existing; the `:8100` one is slice 4's business per korg:1026.

## 5. Nothing else on kai changed

md5, before and after:

| Path | Before | After |
|---|---|---|
| `/etc/systemd/system/klams-scanner.service` | `eb9961ff…` | `eb9961ff…` |
| `/etc/systemd/system/klams-scanner.timer` | `dc42b502…` | `dc42b502…` |
| `/etc/klams/scanner.toml` | `ecc21d09…` | `ecc21d09…` |

Timer `enabled` + `active` both before and after. kai's deliberate unit
divergence (`User=ken`, no `After=klams-service.service`) survived the
deploy untouched — which is the whole reason the installer refuses to
ship unit files.

## kubs0's own path, verified without deploying

The production deploy belongs to `/sprint-ship` after merge, so kubs0's
store path was proved into a staging directory instead
(`BIN_DST_DIR=…`):

```
resolved latest klams-service = 0.1.42
==> klams-service-x86_64-linux   checksum OK   reports 0.1.42
==> klams-scanner-x86_64-linux   checksum OK   reports 0.1.42
==> klams-monitor-x86_64-linux   checksum OK   reports 0.1.42
```

kubs0's `/usr/local/bin` still held 0.1.41 afterwards, as intended.

That check also caught the deployed 0.1.41 `klams-service` still
exhibiting the defect this sprint fixed — it answers `--version` with

```
Error: no config found: tried /ai/klams/config/klams.toml and …
```

because it resolves config before parsing flags. The 0.1.42 binary
fetched from the store answers `klams-service 0.1.42`. The fix reaches
production when this branch ships.

## Gate

- `just gate` — exit 0, 113 test groups.
- `just test-integration` — exit 0, full docker-gated suite; stack torn
  down afterwards (#647).
- 12 new tests, all in `just gate` (no network: the fake store is a
  temp directory served over `curl file://`).

## What the next slice inherits

klams-view (#1013) was queued to "copy klams-scanner's shape once slice
2 lands". The shape is:

1. `just publish` — clean-tree gate, build, one artifact name per
   deployable, installer published alongside so a repo-less host can
   bootstrap.
2. Verify the *label*, not just the checksum. A mislabelled publish is
   invisible to SHA256 and defeats the version floor, which is the only
   drift alarm the fleet has.
3. Install binaries, never units or config. Hosts diverge on purpose.
4. Store URL and publish host have no repo default.

One thing worth carrying: `curl file://` makes the whole store a temp
directory, so the fetch/verify/install path is testable in an ordinary
`cargo test` with no network and no server. That is what turned this
from "shell we hope works" into 9 assertions about how it fails.
