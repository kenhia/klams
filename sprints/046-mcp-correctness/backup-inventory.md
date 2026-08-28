# /etc/klams backup inventory — the #1377 reconciliation

Measured on kubs0, 2026-08-27, with `klams-token list --config <backup>`.
**No token value was read out**; the counts below come from
`sha256(token)[:12]` fingerprints compared against the live config's.

The #1377 comment asked to "reconcile the five-vs-seven count against
the live directory first". The answer is **seven**, and the exposure is
worse than the WI's "several":

| backup | convention | grants | still live NOW | note |
|---|---|---:|---:|---|
| `klams.toml.bak-016-pre704` | ad-hoc | 14 | **12** | |
| `klams.toml.bak-030` | ad-hoc | 14 | **10** | |
| `klams.toml.bak-032-pre670` | ad-hoc | 14 | **10** | |
| `klams.toml.bak-1783644937` | ad-hoc | 0 | — | carries the **retired `bearer_token`** — pre-`[[auth.tokens]]` shape, still a live secret |
| `klams.toml.bak-20260610-092654` | ad-hoc | 6 | **2** | |
| `klams.toml.bak-20260707-143753` | ad-hoc | 10 | **6** | |
| `klams.toml.bak-20260817T021051Z` | **sprint-045** | 14 | **13** | the tool's own; newest |

Live config: 14 grants.

Six of the seven hold at least one token that is **currently accepted by
the running service**; the newest holds 13 of 14. `bak-1783644937`
returns zero grants not because it is harmless but because it predates
the multi-token schema — it carries the single retired `bearer_token`
field, which `klams-token list` does not enumerate. It is secret-bearing
too, and it is the one a fingerprint sweep would most easily miss.

Four naming conventions are represented, not three: `bak-NNN`,
`bak-NNN-preNNN`, `bak-<unix-ts>`, `bak-YYYYMMDD-HHMMSS`, plus the
settled `bak-YYYYMMDDTHHMMSSZ`.

`klams-token`'s pruner matches only its own convention and never deletes
a backup it did not write. That is deliberate (sprint 045): two of these
are named for the sprint that made them (`bak-016-pre704`,
`bak-032-pre670`), which reads like somebody wanted them kept, and a
tool that guesses which are disposable is a tool that deletes the one
that mattered. So this pass stays manual.

## Also noticed, out of #1377's scope

`monitor.env.bak-20260813-202310` is a backup of `monitor.env` in a
sixth ad-hoc convention. `monitor.env` is 74 bytes and the backup is 30,
so they differ. Worth a look when someone next touches monitor config —
the generalizing principle in `docs/auth.md` applies to it too.
