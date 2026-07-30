#!/usr/bin/env bash
# verify-mvp.sh — walk the SC-001..SC-009 smoke checks from
# sprints/001-initial-mvp/quickstart.md §9 and print a pass/fail summary.
#
# Configuration via env:
#   KLAMS_URL     base URL of the running service   (default http://127.0.0.1:7777)
#   KLAMS_TOKEN   bearer token                       (required)
#
# Flags:
#   --light      Run only /healthz + a single fact write/read round-trip
#                (skips SC-002 knowledge indexing, SC-003 perf seeding,
#                SC-007/008/009 doc + dependency walks). Intended for
#                the `just health` recipe and CI smoke after compose-up.
#   --first-run  Full run with a plain-language verdict at the end
#                (sprint 035, #779). This is `just smoke` — the check
#                docs/install.md ends with. Valid on a completely
#                empty store: every check creates its own data.
#
# This is a thin functional smoke test, NOT a load/perf benchmark.
# Perf claims (SC-002 10s p95, SC-003 500ms p95) are covered by the
# #[ignore]-gated perf_smoke integration test (T088).

set -u
set -o pipefail

# Feed greps from a herestring, never `echo ... | grep -q`. With
# `pipefail` set, `grep -q` exiting on its first match can hand `echo`
# a SIGPIPE, and the pipeline then reports 141 even though the match
# succeeded. Measured at ~1 spurious failure in 30 against /metrics
# (sprint 031, #682) — a smoke gate that fails 3% of the time for no
# reason teaches you to re-run it rather than read it.

LIGHT=0
FIRST_RUN=0
for arg in "$@"; do
  case "$arg" in
    --light) LIGHT=1 ;;
    --first-run) FIRST_RUN=1 ;;
    -h|--help)
      sed -n '2,22p' "$0"
      exit 0
      ;;
    *) echo "unknown arg: $arg" >&2; exit 2 ;;
  esac
done

if (( LIGHT && FIRST_RUN )); then
  echo "--light and --first-run are mutually exclusive (the first-run smoke is a full run)" >&2
  exit 2
fi

URL="${KLAMS_URL:-http://127.0.0.1:7777}"
TOKEN="${KLAMS_TOKEN:-}"

if [[ -z "$TOKEN" ]]; then
  echo "FATAL: KLAMS_TOKEN must be set — every check past /healthz is authenticated." >&2
  echo "  e.g. KLAMS_TOKEN=<token> just health" >&2
  exit 2
fi

PASS=()
FAIL=()
SKIP=()

color() {
  case "$1" in
    pass) printf '\033[32m%s\033[0m' "$2" ;;
    fail) printf '\033[31m%s\033[0m' "$2" ;;
    skip) printf '\033[33m%s\033[0m' "$2" ;;
    *)    printf '%s' "$2" ;;
  esac
}

# record SC outcome: $1=id $2=status (pass|fail|skip) $3=detail
# $4 (optional) = first-run hint: what to check when this fails
# (sprint 035, #779 — a stranger's failure needs a next step, not
# just a status code).
record() {
  local id="$1" status="$2" detail="$3" hint="${4:-}"
  printf '  %s %s — %s\n' "$(color "$status" "[$status]")" "$id" "$detail"
  if [[ "$status" == fail && -n "$hint" ]]; then
    printf '        ↳ check: %s\n' "$hint"
  fi
  case "$status" in
    pass) PASS+=("$id") ;;
    fail) FAIL+=("$id $detail") ;;
    skip) SKIP+=("$id $detail") ;;
  esac
}

curl_api() {
  # $1=method $2=path $3=body (optional). Outputs HTTP status \n body.
  local method="$1" path="$2" body="${3:-}"
  if [[ -n "$body" ]]; then
    curl -sS -o /tmp/verify-mvp.body -w '%{http_code}' \
      -X "$method" "$URL$path" \
      -H "Authorization: Bearer $TOKEN" \
      -H 'Content-Type: application/json' \
      --data "$body"
  else
    curl -sS -o /tmp/verify-mvp.body -w '%{http_code}' \
      -X "$method" "$URL$path" \
      -H "Authorization: Bearer $TOKEN"
  fi
  echo
  cat /tmp/verify-mvp.body 2>/dev/null || true
}

# `${LIGHT:+...}` tested for non-EMPTY, and LIGHT is `0` in full runs —
# so every run announced itself as light mode. Test the value.
if ((FIRST_RUN)); then
  echo "klams first-run smoke against $URL"
else
  echo "klams MVP verification against $URL$( ((LIGHT)) && echo ' (light mode)' )"
fi
echo

# ---------------------------------------------------------------------- /healthz
# Always run a fast liveness check first so a misconfigured stack fails
# loudly before we spend time on writes.
hcode=$(curl -sS -o /tmp/verify-mvp.health -w '%{http_code}' "$URL/healthz" || echo 000)
if [[ "$hcode" =~ ^2 ]]; then
  record HEALTHZ pass "/healthz $hcode"
else
  record HEALTHZ fail "/healthz status=$hcode" \
    "is klams-service running? (\`just run\` in another shell, or \`systemctl status klams-service\`) — and is KLAMS_URL right? (currently $URL)"
fi

# ---------------------------------------------------------------------- SC-001
# A controller can record a new user fact and find it again via unified
# search in under 2 seconds.
#
# Sprint 031 (#682): this posted the MVP-era flat shape
# `{key, value, subject, source:"verify-mvp.sh"}` until 031, which the
# service has rejected since the typed-fact schema landed in sprint 003.
# `source` is a `Source` enum (User|Controller|Task|AgentProposal), and
# the request carries `type` + `payload`, not flat fields — so SC-001
# failed 422 on every build for ~28 sprints, taking `just health` and
# `just verify` down with it. A gate that always fails trains whoever is
# shipping to ignore red, which is exactly when a real regression slips
# through; 027's deploy had to smoke-test itself by hand.
#
# `EnvFact.payload.key` is validated against ^[A-Z][A-Z0-9_]*$, hence
# the shouty marker. `expected_version: 0` asserts "this is a new fact",
# which the PID suffix keeps true across repeat runs.
ts=$(date +%s%N)
fact_key="VERIFY_MVP_$$"
body=$(cat <<JSON
{"type":"EnvFact","payload":{"key":"$fact_key","value":"smoke-test-marker $$"},"source":"Controller","expected_version":0}
JSON
)
status_body=$(curl_api POST /memory/facts "$body")
code=$(echo "$status_body" | head -1)
if [[ "$code" =~ ^2 ]]; then
  # search for it
  search_body=$(cat <<JSON
{"query":"$fact_key","types":["fact"],"top_k":5}
JSON
)
  out=$(curl_api POST /memory/search "$search_body")
  scode=$(echo "$out" | head -1)
  elapsed_ms=$(( ( $(date +%s%N) - ts ) / 1000000 ))
  if [[ "$scode" =~ ^2 ]] && grep -q "$fact_key" <<<"$(tail -n +2 <<<"$out")"; then
    if (( elapsed_ms < 2000 )); then
      record SC-001 pass "fact round-trip ${elapsed_ms}ms"
    else
      record SC-001 fail "fact round-trip took ${elapsed_ms}ms (>= 2000ms)"
    fi
  else
    record SC-001 fail "search did not return marker (status=$scode)" \
      "read the service logs; the /healthz body names any sick dependency (postgres)"
  fi
else
  # Carry the response body, not just the status. #682 sat undiagnosed
  # partly because "status=422" says nothing about *which* field the
  # service rejected — and the service does return that detail.
  record SC-001 fail "fact write failed (status=$code): $(echo "$status_body" | tail -n +2 | head -c 300)" \
    "401/403 → wrong KLAMS_TOKEN (grants are [[auth.tokens]] in klams.toml); 5xx → service logs"
fi

# ---------------------------------------------------------------------- SC-002
# Knowledge chunk searchable within 10s p95. Functional check only.
if (( LIGHT )); then
  record SC-002 skip "skipped (--light)"
  record SC-003 skip "skipped (--light)"
  record SC-004 skip "skipped (--light)"
  record SC-005 skip "skipped (--light)"
  record SC-006 skip "skipped (--light)"
  record SC-007 skip "skipped (--light)"
  record SC-008 skip "skipped (--light)"
  record SC-009 skip "skipped (--light)"
  echo
  echo "Summary:"
  printf '  %s %d passed   %s %d failed   %s %d skipped\n' \
    "$(color pass '✓')" "${#PASS[@]}" \
    "$(color fail '✗')" "${#FAIL[@]}" \
    "$(color skip '·')" "${#SKIP[@]}"
  if (( ${#FAIL[@]} > 0 )); then
    printf '\nFailed:\n'
    for f in "${FAIL[@]}"; do printf '  - %s\n' "$f"; done
    exit 1
  fi
  exit 0
fi

# Sprint 031 (#682): stale in the same way SC-001 was. This posted a
# batch envelope `{"items":[{source, title, text, tags}]}`, but
# `/memory/knowledge/index` takes ONE `IndexKnowledgeRequest` — no
# `items` wrapper, no `title`, and `source` is the same `Source` enum
# the fact path uses. 422 on every build.
ts=$(date +%s%N)
kbody=$(cat <<JSON
{"text":"klams verification chunk unique-token-$$","source":"Controller","tags":["verify-mvp"]}
JSON
)
kresp=$(curl_api POST /memory/knowledge/index "$kbody")
kcode=$(echo "$kresp" | head -1)
if [[ "$kcode" =~ ^2 ]]; then
  found=""
  for i in 1 2 3 4 5 6 7 8 9 10; do
    sleep 1
    out=$(curl_api POST /memory/search "{\"query\":\"unique-token-$$\",\"types\":[\"knowledge\"],\"top_k\":5}")
    if grep -q "unique-token-$$" <<<"$(tail -n +2 <<<"$out")"; then
      found="$i"
      break
    fi
  done
  elapsed_s=$(( ( $(date +%s%N) - ts ) / 1000000000 ))
  if [[ -n "$found" ]]; then
    record SC-002 pass "knowledge searchable after ${elapsed_s}s"
  else
    record SC-002 fail "knowledge not searchable after 10s polls" \
      "embedding can be slow on CPU — re-run once; then check the tei + qdrant containers (docker compose ps) and the service logs"
  fi
else
  record SC-002 fail "knowledge index failed (status=$kcode): $(echo "$kresp" | tail -n +2 | head -c 300)" \
    "is the embedder container healthy? (docker compose ps — tei should be 'healthy'); 401 → wrong KLAMS_TOKEN"
fi

# ---------------------------------------------------------------------- SC-003
# Unified search p95 < 500ms at full corpus — perf test, not measured here.
record SC-003 skip "perf claim; see crates/klams-service/tests/perf_smoke.rs"

# ---------------------------------------------------------------------- SC-004
# Writes survive restart — covered by integration test, not by this script
# (would need to bounce systemd unit). Skip with pointer.
record SC-004 skip "restart-survival covered by integration tests"

# ---------------------------------------------------------------------- SC-005
# Malformed write returns an actionable error.
bad=$(curl_api POST /memory/facts '{"not_a_real_field":1}')
bcode=$(echo "$bad" | head -1)
bbody=$(echo "$bad" | tail -n +2)
if [[ "$bcode" == "400" || "$bcode" == "422" ]] && grep -qiE 'key|value|missing|required|field' <<<"$bbody"; then
  record SC-005 pass "validation error names offending field (status=$bcode)"
else
  record SC-005 fail "expected 400/422 with field detail, got status=$bcode body=$bbody"
fi

# ---------------------------------------------------------------------- SC-006
# Viewport cold launch < 3s — manual / GUI check.
record SC-006 skip "manual: launch klams-viewport.exe on Windows"

# ---------------------------------------------------------------------- SC-007
# Cold-checkout build — verify build commands documented in setup.md /
# viewport/README.md exist; full rebuild is too slow for smoke.
docs_ok=1
for f in docs/setup.md viewport/README.md; do
  [[ -f "$f" ]] || docs_ok=0
done
if (( docs_ok )); then
  record SC-007 pass "build docs present (docs/setup.md, viewport/README.md)"
else
  record SC-007 fail "missing build documentation"
fi

# ---------------------------------------------------------------------- SC-008
# /healthz reports non-200 within 5s of dep loss — destructive, manual.
hresp=$(curl -sS -o /tmp/verify-mvp.health -w '%{http_code}' "$URL/healthz")
hbody=$(cat /tmp/verify-mvp.health 2>/dev/null || true)
if [[ "$hresp" =~ ^2 ]] && grep -qiE 'postgres|qdrant|ok|healthy' <<<"$hbody"; then
  record SC-008 pass "/healthz reachable and reports per-dependency state"
else
  record SC-008 fail "/healthz status=$hresp body=$hbody" \
    "the /healthz body names the sick dependency — check that container (docker compose ps)"
fi

# ---------------------------------------------------------------------- SC-009
# /metrics scrapes and includes klams_queue_depth (or equivalent).
mresp=$(curl -sS -o /tmp/verify-mvp.metrics -w '%{http_code}' "$URL/metrics")
mbody=$(cat /tmp/verify-mvp.metrics 2>/dev/null || true)
if [[ "$mresp" =~ ^2 ]] && grep -qE '^# (TYPE|HELP) ' <<<"$mbody"; then
  record SC-009 pass "/metrics exposes Prometheus exposition format"
else
  record SC-009 fail "/metrics status=$mresp"
fi

echo
echo "Summary:"
printf '  %s %d passed   %s %d failed   %s %d skipped\n' \
  "$(color pass '✓')" "${#PASS[@]}" \
  "$(color fail '✗')" "${#FAIL[@]}" \
  "$(color skip '·')" "${#SKIP[@]}"

if (( ${#FAIL[@]} > 0 )); then
  printf '\nFailed:\n'
  for f in "${FAIL[@]}"; do printf '  - %s\n' "$f"; done
  if ((FIRST_RUN)); then
    echo
    echo "$(color fail '✗') Your install is not working yet — start with the ↳ hints above."
  fi
  exit 1
fi
if ((FIRST_RUN)); then
  echo
  echo "$(color pass '✓') Your install works: klams answered health, stored and"
  echo "  recalled both a fact and a knowledge chunk (write → embed → search),"
  echo "  rejected malformed input with a useful error, and exposed metrics."
  echo "  Next: point the scanner at your files and connect an agent"
  echo "  (docs/install.md §7–8)."
fi
exit 0
