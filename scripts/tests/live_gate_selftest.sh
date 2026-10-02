#!/usr/bin/env bash
# Contract test for the live gate's two scripts: scripts/live-gate-changed
# (which pull requests run the GPT Live scenarios) and scripts/live-gate-verdict
# (GREEN / RED / VOID from one single-attempt Bazel run).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
classifier="${ROOT}/scripts/live-gate-changed"
verdict="${ROOT}/scripts/live-gate-verdict"
work="$(mktemp -d "${TMPDIR:-/tmp}/live-gate-selftest.XXXXXX")"
trap 'rm -rf "$work"' EXIT
repo="${work}/repo"
mkdir -p "$repo"

g() { git -C "$repo" -c user.name=Meerkat -c user.email=meerkat@example.invalid "$@"; }
g init -q -b main
mkdir -p "$repo/crates/meerkat-live/src" "$repo/crates/meerkat-core/src" "$repo/docs"
echo "pub fn a() {}" > "$repo/crates/meerkat-core/src/lib.rs"
echo "pub fn b() {}" > "$repo/crates/meerkat-live/src/lib.rs"
echo "doc" > "$repo/docs/guide.md"
g add -A && g commit -qm base
base="$(g rev-parse HEAD)"

classify() {
  local head="$1"
  set +e
  (cd "$repo" && "$classifier" "$base" "$head" >/dev/null 2>&1)
  local status=$?
  set -e
  echo "$status"
}

fail() { echo "live_gate_selftest: $*" >&2; exit 1; }

# A docs-only change is not live.
g checkout -qb docs
echo "more" >> "$repo/docs/guide.md"; g commit -qam docs
[[ "$(classify "$(g rev-parse HEAD)")" == 1 ]] || fail "a docs-only change classified live"

# A Rust change that names no live state is not live.
g checkout -q "$base" && g checkout -qb plain
echo "pub fn c() {}" >> "$repo/crates/meerkat-core/src/lib.rs"; g commit -qam plain
[[ "$(classify "$(g rev-parse HEAD)")" == 1 ]] || fail "a non-live Rust change classified live"

# A file of the live stack is live by path.
g checkout -q "$base" && g checkout -qb livepath
echo "pub fn d() {}" >> "$repo/crates/meerkat-live/src/lib.rs"; g commit -qam livepath
[[ "$(classify "$(g rev-parse HEAD)")" == 0 ]] || fail "a live-stack path classified not live"

# A shared Rust file whose changed lines name live state is live by content.
g checkout -q "$base" && g checkout -qb livecontent
echo "pub fn e(live_channel_id: u32) -> u32 { live_channel_id }" >> "$repo/crates/meerkat-core/src/lib.rs"
g commit -qam livecontent
[[ "$(classify "$(g rev-parse HEAD)")" == 0 ]] || fail "a Rust change naming live state classified not live"

# An unknown revision is an error, never "not live".
[[ "$(classify deadbeefdeadbeef)" == 2 ]] || fail "an unknown revision did not fail closed"

run_verdict() {
  set +e
  python3 "$verdict" "$1" "$2" >/dev/null 2>&1
  local status=$?
  set -e
  echo "$status"
}
section() { printf '==================== Test output for //:e2e_smoke_turbo_s_s%s (run 1 of 1):\n%s\n%s\n' "$1" "$2" "$(printf '=%.0s' $(seq 1 80))"; }

: > "$work/green.log"
[[ "$(run_verdict "$work/green.log" 0)" == 0 ]] || fail "a clean run was not GREEN"
[[ "$(run_verdict "$work/green.log" 1)" == 1 ]] || fail "a Bazel failure without test output was not RED"
section 104 "GPT_LIVE_VERDICT scenario=S104 verdict=provider_degraded exchange=e2" > "$work/void.log"
[[ "$(run_verdict "$work/void.log" 3)" == 3 ]] || fail "a provider-degraded-only run was not VOID"
{ section 104 "GPT_LIVE_VERDICT scenario=S104 verdict=provider_degraded exchange=e2"; section 101 "S101 deterministic checks failed"; } > "$work/red.log"
[[ "$(run_verdict "$work/red.log" 3)" == 1 ]] || fail "a real failure next to a void was not RED"

echo "live gate selftest: ok"
