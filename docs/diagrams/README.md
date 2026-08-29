# docs/diagrams

Two kinds of thing live here, and they are maintained in opposite ways.
Read the table before editing either.

| File | Kind | Change it by |
|---|---|---|
| `klams-topology.svg` | hand-authored | editing the SVG |
| `klams-read-path.svg` | hand-authored | editing the SVG |
| `klams-write-path.svg` | hand-authored | editing the SVG |
| `klams.architecture.json` | **source** | editing this spec |
| `klams-architecture.html` | **generated** | re-rendering from the spec |

The three SVGs are hand-drawn (sprint 033, #694) and inlined into
[architecture.md](../architecture.md). Do not try to regenerate them —
there is nothing to regenerate them from.

## The interactive component map

**[▶ Rendered](https://htmlpreview.github.io/?https://github.com/kenhia/klams/blob/main/docs/diagrams/klams-architecture.html)**
(via htmlpreview.github.io; source:
[klams-architecture.html](klams-architecture.html) — self-contained, no
external requests, light/dark aware. It opens straight from a checkout
too.)

`klams-architecture.html` is **generated**. Never hand-edit it: the next
re-render discards the change silently. Edit
[`klams.architecture.json`](klams.architecture.json) instead — that spec
is the source of truth, and it is small enough to review in a diff.

### Provenance

| | |
|---|---|
| Tool | [archify](https://github.com/tt-a1i/archify) 2.16.0-dev.0 @ `0853a805` |
| Authored by | Claude Opus 5, via Claude Code — sprint 047, korg #1702 |
| Subject | klams @ `ceffb6c6` |
| Receipt | 9/9 checks, `showcase` profile, 0 errors, 0 warnings |
| Evidence | verified — 9 repository references |
| spec sha256 | `f963a405d43b04e85fc494acacc2d7f9344dcbd653ac6d9dee17f8d8c071435f` |
| artifact sha256 | `53886dabe3901f6fb31a02af78903e0cb7097375ee0dbb074ee8bde0b364a7a9` |

The spec carries `meta.repository.revision`, so the rendered page states
which klams commit it describes. That is the staleness signal: when the
architecture moves on, the diagram says which past it is showing instead
of quietly misleading someone.

### Re-rendering

archify is not installed in this repo or on `kubs0`. It is a skill
package, fetched when needed, and it vendors its own runtime
dependencies — there is no `npm install` step. Node >= 18.

```sh
git clone https://github.com/tt-a1i/archify /tmp/archify
git -C /tmp/archify checkout 0853a805
cd /tmp/archify/archify

node bin/archify.mjs deliver architecture \
  "$KLAMS"/docs/diagrams/klams.architecture.json \
  "$KLAMS"/docs/diagrams/klams-architecture.html \
  --quality showcase --repo-root "$KLAMS" --json
```

Update `meta.repository.revision` in the spec to the commit you are
describing *before* re-rendering. `deliver` writes nothing on a failed
check, so a non-zero exit means the committed artifact is untouched —
read the `diagnostics` array, fix the one field it names, and run it
again.

### Known caveats

- **Size.** 739 KB, roughly 37x `docs/sharing/pitch.html`. htmlpreview
  proxies through raw.githubusercontent and is noticeably slower at this
  size. Every re-commit adds another ~739 KB to history permanently, so
  iterate on the spec and re-commit the HTML only when the diagram
  meaningfully changes.
- **Small type.** Node sublabels project to 6.31px at a 1440x900 desktop
  viewport, against archify's 6px floor — it passes, but only just. It
  reads comfortably from 1600px up; the viewer has pan and zoom for
  everything below that.
- **Structure only.** The diagram deliberately does not carry the
  thirteen retrieval stages, the scope/auth model, or the failure
  contracts. Those are [architecture.md](../architecture.md), and a
  diagram that tried to hold them would be worse at both jobs.
