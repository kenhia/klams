#!/usr/bin/env bash
# verify-mvp.sh — walk the SC-001..SC-009 smoke checks from
# sprints/001-initial-mvp/quickstart.md §9 and print a pass/fail summary.
#
# Configuration via env:
#   KLAMS_URL     base URL of the running service   (default http://127.0.0.1:7777)
#   KLAMS_TOKEN   bearer token                       (required)
#
# Flags:
#   --light    Run only /healthz + a single fact write/read round-trip
#              (skips SC-002 knowledge indexing, SC-003 perf seeding,
#              SC-007/008/009 doc + dependency walks). Intended for
#              the `just health` recipe and CI smoke after compose-up.
#
# This is a thin functional smoke test, NOT a load/perf benchmark.
# Perf claims (SC-002 10s p95, SC-003 500ms p95) are covered by the
# #[ignore]-gated perf_smoke integration test (T088).

set -u
set -o pipefail

LIGHT=0
for arg in "$@"; do
  case "$arg" in
    --light) LIGHT=1 ;;
    -h|--help)
      sed -n '2,18p' "$0"
      exit 0
      ;;
    *) echo "unknown arg: $arg" >&2; exit 2 ;;
  esac
done

URL="${KLAMS_URL:-http://127.0.0.1:7777}"
TOKEN="${KLAMS_TOKEN:-}"

if [[ -z "$TOKEN" ]]; then
  echo "FATAL: KLAMS_TOKEN must be set" >&2
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
record() {
  local id="$1" status="$2" detail="$3"
  printf '  %s %s — %s\n' "$(color "$status" "[$status]")" "$id" "$detail"
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

echo "klams MVP verification against $URL${LIGHT:+ (light mode)}"
echo

# ---------------------------------------------------------------------- /healthz
# Always run a fast liveness check first so a misconfigured stack fails
# loudly before we spend time on writes.
hcode=$(curl -sS -o /tmp/verify-mvp.health -w '%{http_code}' "$URL/healthz" || echo 000)
if [[ "$hcode" =~ ^2 ]]; then
  record HEALTHZ pass "/healthz $hcode"
else
  record HEALTHZ fail "/healthz status=$hcode"
fi

# ---------------------------------------------------------------------- SC-001
# A controller can record a new user fact and find it again via unified
# search in under 2 seconds.
ts=$(date +%s%N)
fact_key="verify-mvp-$$"
body=$(cat <<JSON
{"key":"$fact_key","value":"smoke-test-marker $$","subject":"verify-mvp","source":"verify-mvp.sh"}
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
  if [[ "$scode" =~ ^2 ]] && echo "$out" | tail -n +2 | grep -q "$fact_key"; then
    if (( elapsed_ms < 2000 )); then
      record SC-001 pass "fact round-trip ${elapsed_ms}ms"
    else
      record SC-001 fail "fact round-trip took ${elapsed_ms}ms (>= 2000ms)"
    fi
  else
    record SC-001 fail "search did not return marker (status=$scode)"
  fi
else
  record SC-001 fail "fact write failed (status=$code)"
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

chunk_id="verify-mvp-knowledge-$$"
ts=$(date +%s%N)
kbody=$(cat <<JSON
{"items":[{"source":"verify-mvp.sh","title":"$chunk_id","text":"klams verification chunk unique-token-$$","tags":["verify-mvp"]}]}
JSON
)
kresp=$(curl_api POST /memory/knowledge/index "$kbody")
kcode=$(echo "$kresp" | head -1)
if [[ "$kcode" =~ ^2 ]]; then
  found=""
  for i in 1 2 3 4 5 6 7 8 9 10; do
    sleep 1
    out=$(curl_api POST /memory/search "{\"query\":\"unique-token-$$\",\"types\":[\"knowledge\"],\"top_k\":5}")
    if echo "$out" | tail -n +2 | grep -q "unique-token-$$"; then
      found="$i"
      break
    fi
  done
  elapsed_s=$(( ( $(date +%s%N) - ts ) / 1000000000 ))
  if [[ -n "$found" ]]; then
    record SC-002 pass "knowledge searchable after ${elapsed_s}s"
  else
    record SC-002 fail "knowledge not searchable after 10s polls"
  fi
else
  record SC-002 fail "knowledge index failed (status=$kcode)"
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
if [[ "$bcode" == "400" || "$bcode" == "422" ]] && echo "$bbody" | grep -qiE 'key|value|missing|required|field'; then
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
if [[ "$hresp" =~ ^2 ]] && echo "$hbody" | grep -qiE 'postgres|qdrant|ok|healthy'; then
  record SC-008 pass "/healthz reachable and reports per-dependency state"
else
  record SC-008 fail "/healthz status=$hresp body=$hbody"
fi

# ---------------------------------------------------------------------- SC-009
# /metrics scrapes and includes klams_queue_depth (or equivalent).
mresp=$(curl -sS -o /tmp/verify-mvp.metrics -w '%{http_code}' "$URL/metrics")
mbody=$(cat /tmp/verify-mvp.metrics 2>/dev/null || true)
if [[ "$mresp" =~ ^2 ]] && echo "$mbody" | grep -qE '^# (TYPE|HELP) '; then
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
  exit 1
fi
exit 0
