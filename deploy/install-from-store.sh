#!/usr/bin/env bash
# sprint 042 — install klams binaries from the homelab package store.
#
#   install-from-store.sh [OPTIONS] BINARY [BINARY...]
#
# Fetches each named binary from the store, verifies it against the
# published SHA256SUMS, asserts it reports the version it was labelled
# with, and installs it into /usr/local/bin (rotating the outgoing copy
# to <name>.prev).
#
# SELF-CONTAINED BY DESIGN. This script is published into every klams
# artifact directory alongside the binaries, so a host with no klams
# checkout can bootstrap from the store alone (see docs/setup.md). It
# depends on nothing but bash, curl, sha256sum and install.
#
# WHAT IT DOES NOT DO, deliberately:
#   * unit files      — hosts diverge on purpose. kai's
#                       klams-scanner.service runs as User=ken and drops
#                       After=klams-service.service because kai has no
#                       local service; k-homelab's recipe README says not
#                       to "fix" that. Shipping units would overwrite it
#                       on every deploy. Units come from install-systemd.sh
#                       on hosts that have a checkout.
#   * config          — /etc/klams/*.toml carries bearer tokens.
#   * restart/reload  — installing and activating are separate steps, so
#                       the caller decides when a service takes the new
#                       binary. The follow-up commands are printed.

set -euo pipefail

STORE_URL=${KLAMS_STORE_URL:-}
VERSION=""
DRY_RUN=0
BIN_DST_DIR=${BIN_DST_DIR:-/usr/local/bin}
BINS=()

usage() {
    cat >&2 <<'EOF'
usage: install-from-store.sh [OPTIONS] BINARY [BINARY...]

  BINARY            klams-service | klams-scanner | klams-monitor

options:
  --store URL       package store base URL (default: $KLAMS_STORE_URL)
  --version VER     version to install (default: the store's `latest`
                    pointer for the first named binary, applied to all)
  --dry-run         print what would happen; touch nothing
  -h, --help        this message

example (any tailnet host, no checkout required):
  sudo KLAMS_STORE_URL=https://store.example:4880 \
      bash install-from-store.sh klams-scanner
EOF
}

fail() { printf 'ERROR: %s\n' "$1" >&2; exit 1; }

say() {
    if [ "$DRY_RUN" -eq 1 ]; then printf '[dry-run] %s\n' "$*"
    else printf '+ %s\n' "$*"; fi
}

run() {
    say "$*"
    [ "$DRY_RUN" -eq 0 ] && eval "$@"
    return 0
}

while [ $# -gt 0 ]; do
    case "$1" in
        --store)   STORE_URL="${2:-}"; shift 2 ;;
        --version) VERSION="${2:-}"; shift 2 ;;
        --dry-run) DRY_RUN=1; shift ;;
        -h|--help) usage; exit 0 ;;
        -*)        usage; fail "unknown option: $1" ;;
        *)         BINS+=("$1"); shift ;;
    esac
done

# --- 0. Pre-flight --------------------------------------------------------

[ ${#BINS[@]} -gt 0 ] || { usage; fail "name at least one binary to install"; }

# No default store URL, on purpose (the #682 / #776 pattern): a guessed
# hostname fails later as a confusing curl error instead of saying which
# variable you forgot.
[ -n "$STORE_URL" ] || fail \
    "no package store URL — pass --store URL or set KLAMS_STORE_URL (e.g. https://<host>:4880)"
STORE_URL=${STORE_URL%/}

for cmd in curl sha256sum install; do
    command -v "$cmd" >/dev/null 2>&1 || fail "$cmd not found on this host"
done

# The precondition is writability, not root: BIN_DST_DIR is overridable
# and a host may install into a user-owned prefix (~/.local/bin) instead.
if [ "$DRY_RUN" -eq 0 ]; then
    [ -d "$BIN_DST_DIR" ] || fail "$BIN_DST_DIR does not exist"
    # `sudo env VAR=…` rather than `sudo -E`: sudo's env_reset strips
    # KLAMS_STORE_URL on a default Debian/Ubuntu sudoers.
    [ -w "$BIN_DST_DIR" ] || fail \
        "$BIN_DST_DIR is not writable by $(id -un) — try: sudo env KLAMS_STORE_URL=\"\$KLAMS_STORE_URL\" bash $0 ${BINS[*]}"
fi

# The published filename carries the target arch, so a wrong-arch host
# gets a 404 naming what it asked for rather than an ELF that will not
# exec.
ARCH=$(uname -m)
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
SUFFIX="${ARCH}-${OS}"

# --- 1. Resolve the version ----------------------------------------------

# One version across every binary in a single invocation: klams-scanner
# and klams-service speak the same wire contract and docs/setup.md
# already requires them to match. Resolving each binary's own `latest`
# independently would let one publish drift them apart silently.
if [ -z "$VERSION" ]; then
    first="${BINS[0]}"
    VERSION=$(curl -fsS "$STORE_URL/artifacts/$first/latest" 2>/dev/null) \
        || fail "cannot read $STORE_URL/artifacts/$first/latest — is $first published, and is the store reachable?"
    VERSION=$(printf '%s' "$VERSION" | tr -d '[:space:]')
    [ -n "$VERSION" ] || fail "the latest pointer for $first is empty"
    printf 'resolved latest %s = %s\n' "$first" "$VERSION"
fi

printf 'installing klams %s (%s) from %s\n' "$VERSION" "$SUFFIX" "$STORE_URL"

WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

# --- 2. Fetch + verify EVERY binary before installing ANY ----------------

# Two passes on purpose: a checksum failure on the third binary must not
# leave the first two already swapped into place.
for bin in "${BINS[@]}"; do
    base="$STORE_URL/artifacts/$bin/$VERSION"
    file="$bin-$SUFFIX"

    printf '==> %s\n' "$file"
    curl -fsS -o "$WORK/$file" "$base/$file" \
        || fail "fetch failed: $base/$file (is $bin published at $VERSION?)"

    sums=$(curl -fsS "$base/SHA256SUMS") \
        || fail "fetch failed: $base/SHA256SUMS"
    line=$(printf '%s\n' "$sums" | grep -E "[[:space:]]\*?$file\$" | head -1)
    [ -n "$line" ] || fail "$file is not listed in $base/SHA256SUMS"

    ( cd "$WORK" && printf '%s\n' "$line" | sha256sum -c --status - ) \
        || fail "checksum MISMATCH for $file — refusing to install"
    printf '    checksum OK\n'

    chmod 0755 "$WORK/$file"

    # The checksum proves the transfer; this proves the label. A binary
    # published under the wrong version number would otherwise install
    # cleanly and then lie to `--version`, which is precisely the signal
    # k-homelab's version floor reads.
    reported=$("$WORK/$file" --version 2>/dev/null | awk '{print $NF}') || true
    [ -n "$reported" ] || fail "$file --version produced nothing — wrong arch, or not a klams binary"
    [ "$reported" = "$VERSION" ] || fail \
        "$file reports version $reported but was published as $VERSION — the store labelling is wrong, not this host"
    printf '    reports %s\n' "$reported"
done

# --- 3. Install (rotate prev, atomic move) -------------------------------

for bin in "${BINS[@]}"; do
    src="$WORK/$bin-$SUFFIX"
    dst="$BIN_DST_DIR/$bin"
    if [ -e "$dst" ]; then
        old=$("$dst" --version 2>/dev/null | awk '{print $NF}' || true)
        say "rotating $dst (${old:-unknown}) -> $dst.prev"
        [ "$DRY_RUN" -eq 0 ] && mv -f "$dst" "$dst.prev"
    fi
    run "install -m 0755 '$src' '$dst'"
done

# --- 4. Say what to do next; do not do it --------------------------------

printf '\ndone — klams %s installed into %s\n' "$VERSION" "$BIN_DST_DIR"
printf 'Nothing was restarted. Activate what applies to this host:\n'
for bin in "${BINS[@]}"; do
    case "$bin" in
        klams-scanner)
            printf '  klams-scanner  the next klams-scanner.timer tick picks it up;\n'
            printf '                 force one now with: systemctl start klams-scanner.service\n' ;;
        klams-service) printf '  klams-service  systemctl restart klams-service\n' ;;
        klams-monitor) printf '  klams-monitor  systemctl restart klams-monitor\n' ;;
        *)             printf '  %s  (unknown unit — restart it by hand)\n' "$bin" ;;
    esac
done
printf 'Rollback: install-from-store.sh --version <older> %s\n' "${BINS[*]}"
