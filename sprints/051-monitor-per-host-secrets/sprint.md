# Sprint 051 — klams-monitor onto the per-host secrets file

Proposal korg:2435, work item **2391**. Slice 16 of program korg:2440
(simplify homelab secrets). karc leg `klams-8bbfd1` on kubs0, overseen.

## Goal

klams-monitor stops keeping a private copy of the central Redis password.
It reads `REDISCLI_AUTH` from `/etc/khomelab/secrets.env` — the one file
k-homelab renders per host — and `/etc/klams/monitor.env` is deleted once
the monitor is verified publishing on the new file.

## Premise check

| claim (WI 2391) | verdict |
|---|---|
| `/etc/klams/monitor.env` is `root:klams 0640` and holds the central Redis password | **holds** — verified on kubs0; exactly one key, `REDISCLI_AUTH`, nothing else |
| it is a *copy* of the value now in the per-host file | **holds** — both fingerprint `76cde5f57955`; the empty-string control is `e3b0c44298fc`, so the measurement is real |
| the unit is authored in this repo and installed from here | **drifted, and this is the interesting one** — see below |
| the `klams` account joins group `khomelab` | correct as a manifest declaration only; the unit does **not** need it (convention 1) |

### The drift: the live `EnvironmentFile=` has no source of truth in any repo

`deploy/klams-monitor.service` carries the line **commented out**. The
`EnvironmentFile=/etc/klams/monitor.env` that is actually in force on kubs0
comes from `/etc/systemd/system/klams-monitor.service.d/10-kpidash.conf`, a
drop-in hand-written during sprint 010's live cutover and never added to this
repo. `install-systemd.sh` installs units only, so a reinstall has never
touched it.

So the change WI 2391 asks for could not be made by editing the shipped unit
alone — there would still be a hand-made drop-in overriding it. The fix has
to put the drop-in itself under repo ownership (kaed PD-9: author it in the
repo, install it from there).

## Decisions

### D-1 — The password goes in a repo-owned drop-in, not in the shipped unit

`deploy/klams-monitor.service` is an operator-facing surface, and AGENTS.md
draws a hard line: no Ken-shaped path may ship as a default. A bare
`EnvironmentFile=/etc/khomelab/secrets.env` in the shipped unit would make
klams-monitor **fail to start** for anyone who installs klams without
k-homelab — strictly worse than the status quo.

So: `deploy/klams-monitor.service.d/10-khomelab-secrets.conf` is new and
repo-owned, and `install-systemd.sh` installs it **only on a host that
already has `/etc/khomelab/secrets.env`**. The file's presence is exactly the
fact "this is a k-homelab host", so it is the honest trigger, and there is no
flag to forget on a later deploy — a forgotten flag would silently drop the
monitor back to an unauthenticated Redis, which is the failure this program
keeps finding. Both branches print what they did, and both were
negative-tested (below).

### D-2 — No `-` on the `EnvironmentFile`, per convention 7, and the reason is in the file

Without `REDISCLI_AUTH` the reporter does not stop: `kpidash.rs` warns and
"will attempt an unauthenticated connection", which fails per command while
the monitor keeps running and publishes nothing. That is precisely the
"running process publishing nothing" convention 7 rules against, so the
drop-in hard-fails instead.

Note this cuts against the shape of the unit — the kpidash card is a
*secondary* feature, and the monitor's primary job (systemd-state events into
klams) does not need the secret at all. The overseer pre-ruled "a monitor is
the second kind" and the ruling is followed; flagging it because a strict
reading of "the value is the only thing the unit needs" would have gone the
other way, and because it is now the answer to korg:2436's question:

> **On a host carrying this drop-in, `klams-monitor` hard-fails if
> `/etc/khomelab/secrets.env` is missing.**

The hard-fail is scoped to hosts that have the file, so it can never strand a
stranger's install.

### D-3 — No `SupplementaryGroups=khomelab`

Convention 1, and the unit is confirmed a **system** unit
(`/etc/systemd/system/klams-monitor.service`, `User=klams`,
`SupplementaryGroups=` empty), so the kpidash user-manager caveat does not
apply. systemd reads `EnvironmentFile=` as PID 1 before dropping to
`User=klams`. The group is declared in kubs0's k-homelab manifest as
documentation; a working monitor is no evidence the membership took, and it
is asserted separately.

### D-4 — Convention 6 (the kdeskdash trap) checked, and it clears

The per-host file injects every key it holds into the process environment.
On kubs0 it holds exactly two: `REDISCLI_AUTH` and `HF_TOKEN`.

- The repo's only use of `REDISCLI_AUTH` is `crates/klams-monitor/src/kpidash.rs`,
  and it is for **the same Redis** the fleet key names (`rediscli-auth-rpi53`
  on rpi53). Same name, same secret, same purpose — nothing to rename.
- `HF_TOKEN` appears nowhere in this repo.
- `POSTGRES_PASSWORD` and `GRAFANA_ADMIN_PASSWORD` do appear, but only in
  `deploy/docker-compose.yml` / `compose.env.example` / the provision script —
  klams's own containers, not klams-monitor's process tree — and neither key
  is rendered into kubs0's file (they are kubsdb keys).

`/etc/klams/monitor.toml`'s `[kpidash]` section sets no inline `password`, so
the environment variable is the live path, not a dormant fallback.

## Live check (convention 3) and cutover

Recorded in `cutover.md` alongside this file.

## Repaired in passing

- `/etc/klams/monitor.env.bak-20260813-202310` — a plaintext backup of the
  same password, in `/etc/klams/` since June, flagged as out of scope by
  sprint 046 and never removed. Deleting `monitor.env` while leaving its
  backup would have left the copy this slice exists to remove. Deleted with it.

## Checked and deliberately left alone

- `/etc/klams/klams.toml.bak-20260913T005012Z`, left by sprint 050's ship
  turn. Verified token-free with `klams-token --config <bak> list` (zero grant
  rows, names only, no value read), so it is not secret-bearing and is not
  this sprint's file. Recorded so nobody re-audits it.
