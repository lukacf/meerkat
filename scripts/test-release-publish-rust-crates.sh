#!/usr/bin/env bash
# Self-test for scripts/release-publish-rust-crates.sh, the level-parallel
# crates.io publisher, with a stubbed `cargo publish` and a file:// sparse
# index (no network, no upload). The real canonical order and `cargo metadata`
# supply the levels.
#
# It proves: every release crate is published exactly once; no crate starts
# before every crate of the levels below it finished and is in the index;
# uploads overlap within a level but never exceed MEERKAT_CRATE_PUBLISH_JOBS,
# while a dry run stays sequential and does not wait for the index; an
# already published version is skipped; the first failing crate stops the
# publication with its exit status before any later level starts; a crate
# that never reaches the index fails the level once
# MEERKAT_CRATE_INDEX_TIMEOUT has passed; crates.io server errors, cargo's
# 30-second timeout report included, and network errors are retried on their
# own bounded budget, separate from the rate limit's; and
# MEERKAT_CRATE_PUBLISH_PATCH=false drops the patch config from real uploads
# only, while any value but true or false refuses to start.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
PUBLISHER="${REPO_ROOT}/scripts/release-publish-rust-crates.sh"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-publish-levels.XXXXXX")"
trap '[[ -n "${KEEP_TEST_ROOT:-}" ]] || rm -rf "$TEST_ROOT"' EXIT

REAL_CARGO="$(command -v cargo)"
LEVELS="$("${REPO_ROOT}/scripts/release-rust-crates.sh" --levels)"
mkdir -p "${TEST_ROOT}/bin"

# cargo publish -p CRATE ...: records start/end times and the concurrency,
# then follows ${STATE}/behaviour/CRATE (default "ok"): ok uploads and adds
# the version to the index, exists answers like crates.io for a published
# version, fail exits 3, hidden uploads without ever reaching the index.
cat >"${TEST_ROOT}/bin/cargo" <<'STUB'
#!/usr/bin/env bash
set -euo pipefail
if [[ "${1:-}" != "publish" ]]; then
  exec "${REAL_CARGO}" "$@"
fi
crate=""
dry_run=false
config=no
while [[ "$#" -gt 0 ]]; do
  case "$1" in
    -p) crate="$2"; shift 2 ;;
    --dry-run) dry_run=true; shift ;;
    --config) config=yes; shift 2 ;;
    *) shift ;;
  esac
done
echo "${crate} ${config}" >>"${STATE}/configs"
now_ms() { date +%s%3N; }
{
  flock 9
  running=$(( $(cat "${STATE}/running" 2>/dev/null || echo 0) + 1 ))
  echo "${running}" >"${STATE}/running"
  peak="$(cat "${STATE}/peak" 2>/dev/null || echo 0)"
  (( running > peak )) && echo "${running}" >"${STATE}/peak"
  echo "start ${crate} $(now_ms)" >>"${STATE}/events"
} 9>"${STATE}/lock"
sleep 0.1
behaviour="$(cat "${STATE}/behaviour/${crate}" 2>/dev/null || echo ok)"
attempt="$({
  flock 9
  count=$(( $(cat "${STATE}/attempts.${crate}" 2>/dev/null || echo 0) + 1 ))
  echo "${count}" >"${STATE}/attempts.${crate}"
  echo "${count}"
} 9>"${STATE}/lock")"
# Server-error behaviours answer their first attempts as crates.io did for
# v0.9.3, then publish: server-error-once (one HTML 503), server-error-json
# (one JSON 500), server-error-landed (a 503 for an upload that did land, so
# the retry finds it published), timeout-landed (the same, reported as cargo
# reports a 503 that arrives 29 s or more into the upload), network-landed
# (curl's timeout for an upload that did land), rate-limit-then-server-error
# (a 429 that names the next second, a 503, then the upload), server-error
# (always 503).
server_error() {
  cat >&2 <<EOF
error: failed to publish ${crate} v${VERSION} to registry at https://crates.io

Caused by:
  failed to get a 200 OK response, got 503
  headers:
  	HTTP/2 503
  	server: Heroku
  body:
  <title>Application Error</title>
EOF
  status=101
}
upload() {
  echo "    Uploaded ${crate} v${VERSION} to registry \`crates-io\`"
  if [[ "${dry_run}" == "false" ]]; then
    name="${crate,,}"
    path="${name:0:2}/${name:2:2}/${name}"
    mkdir -p "${INDEX_DIR}/$(dirname "${path}")"
    printf '{"name":"%s","vers":"%s"}\n' "${crate}" "${VERSION}" >>"${INDEX_DIR}/${path}"
  fi
}
status=0
case "${behaviour}:${attempt}" in
  server-error-once:1 | rate-limit-then-server-error:2 | server-error:*)
    server_error
    ;;
  server-error-json:1)
    echo "error: failed to publish ${crate} v${VERSION} to registry at https://crates.io" >&2
    echo "Caused by:" >&2
    echo "  the remote server responded with an error (status 500 Internal Server Error): internal error" >&2
    status=101
    ;;
  server-error-landed:1)
    upload
    server_error
    ;;
  timeout-landed:1)
    upload
    cat >&2 <<EOF
error: failed to publish ${crate} v${VERSION} to registry at https://crates.io

Caused by:
  Request timed out after 30 seconds. If you're trying to upload a crate it may be too large. If the crate is under 10MB in size, you can email help@crates.io for assistance.
  Total size was 123456.
EOF
    status=101
    ;;
  network-landed:1)
    upload
    printf 'error: failed to publish to registry at https://crates.io\n\nCaused by:\n  [28] Timeout was reached (Operation timed out after 30000 milliseconds with 0 bytes received)\n' >&2
    status=101
    ;;
  server-error-landed:* | timeout-landed:* | network-landed:*)
    echo "error: crate version \`${VERSION}\` is already uploaded" >&2
    status=101
    ;;
  rate-limit-then-server-error:1)
    echo "error: failed to publish ${crate} v${VERSION} to registry at https://crates.io" >&2
    echo "Caused by:" >&2
    echo "  the remote server responded with an error (status 429 Too Many Requests): You have published too many updates to existing crates in a short period of time. Please try again after $(date -u -d '+1 second' '+%a, %d %b %Y %H:%M:%S GMT') and see https://crates.io/docs/rate-limits for more details." >&2
    status=101
    ;;
  server-error-once:* | server-error-json:* | rate-limit-then-server-error:*)
    upload
    ;;
esac
case "${behaviour}" in
  ok|hidden)
    echo "    Uploaded ${crate} v${VERSION} to registry \`crates-io\`"
    if [[ "${behaviour}" == "ok" && "${dry_run}" == "false" ]]; then
      name="${crate,,}"
      path="${name:0:2}/${name:2:2}/${name}"
      mkdir -p "${INDEX_DIR}/$(dirname "${path}")"
      printf '{"name":"%s","vers":"%s"}\n' "${crate}" "${VERSION}" >>"${INDEX_DIR}/${path}"
    fi
    ;;
  exists)
    echo "error: crate version \`${VERSION}\` is already uploaded" >&2
    status=101
    ;;
  fail)
    echo "error: simulated upload failure" >&2
    status=3
    ;;
esac
{
  flock 9
  echo $(( $(cat "${STATE}/running") - 1 )) >"${STATE}/running"
  echo "end ${crate} $(now_ms)" >>"${STATE}/events"
} 9>"${STATE}/lock"
exit "${status}"
STUB
chmod +x "${TEST_ROOT}/bin/cargo"

fail() {
  echo "level-parallel publisher contract violated: $1" >&2
  shift
  for extra in "$@"; do
    echo "  ${extra}" >&2
  done
  exit 1
}

VERSION="$(awk -F '\t' 'NR == 1 { print $3 }' <<<"${LEVELS}")"
export REAL_CARGO VERSION

# publish <label> <expected status> [VAR=value ...]: runs the publisher.
publish() {
  local label="$1" expected="$2"
  shift 2
  STATE="${TEST_ROOT}/${label}"
  INDEX_DIR="${STATE}/index"
  mkdir -p "${STATE}/behaviour" "${INDEX_DIR}" "${STATE}/runner-temp"
  : >"${STATE}/events"
  export STATE INDEX_DIR
  local status
  set +e
  env PATH="${TEST_ROOT}/bin:${PATH}" ROOT="${REPO_ROOT}" RUNNER_TEMP="${STATE}/runner-temp" \
    GITHUB_ACTIONS=false MEERKAT_CRATE_INDEX_URL="file://${INDEX_DIR}" \
    MEERKAT_CRATE_INDEX_POLL_SECONDS=0 "$@" \
    "${PUBLISHER}" >"${STATE}/out.log" 2>&1
  status=$?
  set -e
  if [[ "${status}" -ne "${expected}" ]]; then
    fail "${label}: expected exit ${expected}, got ${status}" "$(tail -n 20 "${STATE}/out.log")"
  fi
}

# prepare <label> <crate> <behaviour>: set one crate's behaviour before a run.
prepare() {
  mkdir -p "${TEST_ROOT}/$1/behaviour"
  echo "$3" >"${TEST_ROOT}/$1/behaviour/$2"
}

# Every start of a level-N crate is after every end of every lower level.
check_level_order() {
  local label="$1"
  python3 - "${TEST_ROOT}/${label}/events" "${LEVELS}" <<'PY' || fail "${label}: a level started before a lower level finished"
import sys
events_path, levels_tsv = sys.argv[1:3]
level = {}
for line in levels_tsv.splitlines():
    number, crate, _ = line.split("\t")
    level[crate] = int(number)
start, end = {}, {}
for line in open(events_path, encoding="utf-8"):
    kind, crate, stamp = line.split()
    (start if kind == "start" else end)[crate] = int(stamp)
for crate, began in start.items():
    for other, finished in end.items():
        if level[other] < level[crate] and finished > began:
            sys.exit(f"{crate} (level {level[crate]}) started before {other} (level {level[other]}) finished")
PY
}

started() {
  awk '$1 == "start" { print $2 }' "${TEST_ROOT}/$1/events" | sort
}

all_crates="$(cut -f2 <<<"${LEVELS}" | sort)"

# 1. Uploads: every crate once, levels in order, overlap bounded by the jobs,
#    and an already published version (the first crate) skipped.
first_crate="$(awk -F '\t' 'NR == 1 { print $2 }' <<<"${LEVELS}")"
prepare uploads "${first_crate}" exists
name="${first_crate,,}"
mkdir -p "${TEST_ROOT}/uploads/index/${name:0:2}/${name:2:2}"
printf '{"name":"%s","vers":"%s"}\n' "${first_crate}" "${VERSION}" >"${TEST_ROOT}/uploads/index/${name:0:2}/${name:2:2}/${name}"
publish uploads 0 MEERKAT_CRATE_PUBLISH_JOBS=4
[[ "$(started uploads)" == "${all_crates}" ]] || fail "uploads: not every crate was published exactly once"
check_level_order uploads
peak="$(cat "${TEST_ROOT}/uploads/peak")"
if [[ "${peak}" -lt 2 || "${peak}" -gt 4 ]]; then
  fail "uploads: peak concurrency ${peak}, expected between 2 and MEERKAT_CRATE_PUBLISH_JOBS=4"
fi
grep -Fq "${first_crate}: already published, skipping" "${TEST_ROOT}/uploads/out.log" ||
  fail "uploads: the published version was not reported as skipped"
grep -Fq "Every release crate is published" "${TEST_ROOT}/uploads/out.log" || fail "uploads: no completion line"
echo "ok: uploads publish every crate once, level by level, at most 4 at a time (peak ${peak}), skipping a published version"

# 2. A dry run checks one crate at a time and never waits for the index (the
#    stub adds no entry for a dry run); a failure stops it as it stops uploads.
dry_level=2
dry_crate="$(awk -F '\t' -v level="${dry_level}" '$1 == level { print $2; exit }' <<<"${LEVELS}")"
prepare dry-run "${dry_crate}" fail
publish dry-run 3 MEERKAT_REGISTRY_DRY_RUN=true MEERKAT_CRATE_INDEX_TIMEOUT=1
[[ "$(cat "${TEST_ROOT}/dry-run/peak")" -eq 1 ]] || fail "dry-run: crates overlapped"
check_level_order dry-run
below="$(awk -F '\t' -v level="${dry_level}" '$1 < level { print $2 }' <<<"${LEVELS}" | sort)"
[[ "$(comm -12 <(started dry-run) <(printf '%s\n' "${below}"))" == "${below}" ]] ||
  fail "dry-run: the levels below ${dry_crate} did not all run without index entries"
if awk -F '\t' -v level="${dry_level}" '$1 > level { print $2 }' <<<"${LEVELS}" | grep -Fxq -f - <(started dry-run); then
  fail "dry-run: a later level started after ${dry_crate} failed"
fi
echo "ok: a dry run checks crates one at a time without waiting for the index and stops at ${dry_crate}"

# 4. A failing crate stops the publication with its status: once it failed,
#    no further crate of its level starts (one at a time here, so none of its
#    siblings), and no later level starts.
failing_level=3
failing_crate="$(awk -F '\t' -v level="${failing_level}" '$1 == level { print $2; exit }' <<<"${LEVELS}")"
prepare failure "${failing_crate}" fail
publish failure 3 MEERKAT_CRATE_PUBLISH_JOBS=1
check_level_order failure
later="$(awk -F '\t' -v level="${failing_level}" '$1 > level { print $2 }' <<<"${LEVELS}" | sort)"
if comm -12 <(started failure) <(printf '%s\n' "${later}") | grep -q .; then
  fail "failure: a crate of a later level started after ${failing_crate} failed"
fi
siblings="$(awk -F '\t' -v level="${failing_level}" -v crate="${failing_crate}" '$1 == level && $2 != crate { print $2 }' <<<"${LEVELS}" | sort)"
if comm -12 <(started failure) <(printf '%s\n' "${siblings}") | grep -q .; then
  fail "failure: a sibling of ${failing_crate} started after it failed" "$(comm -12 <(started failure) <(printf '%s\n' "${siblings}"))"
fi
grep -Fq "Level ${failing_level}: ${failing_crate} failed (exit 3)" "${TEST_ROOT}/failure/out.log" ||
  fail "failure: the failing crate is not named"
grep -Fq "and the rest of the level are not started" "${TEST_ROOT}/failure/out.log" ||
  fail "failure: the unstarted rest of the level is not reported"
echo "ok: ${failing_crate} failing stops the publication with exit 3; no further crate starts"

# 5. A crate that never reaches the index fails its level after the timeout.
hidden_crate="$(awk -F '\t' 'NR == 1 { print $2 }' <<<"${LEVELS}")"
prepare invisible "${hidden_crate}" hidden
publish invisible 1 MEERKAT_CRATE_INDEX_TIMEOUT=1
grep -Fq "Level 0: not visible in" "${TEST_ROOT}/invisible/out.log" ||
  fail "invisible: the missing index entry is not named" "$(tail -n 5 "${TEST_ROOT}/invisible/out.log")"
if awk -F '\t' '$1 > 0 { print $2 }' <<<"${LEVELS}" | grep -Fxq -f - <(started invisible); then
  fail "invisible: level 1 started although level 0 is not in the index"
fi
echo "ok: a crate missing from the index fails its level after MEERKAT_CRATE_INDEX_TIMEOUT"

level_crate() {
  awk -F '\t' -v level="$1" '$1 == level { print $2; exit }' <<<"${LEVELS}"
}
attempts() {
  cat "${TEST_ROOT}/$1/attempts.$2"
}

# 6. crates.io server errors are retried after the configured delays, on a
#    budget separate from the rate limit's: an HTML 503, a JSON 500, a 503 for
#    an upload that landed (the retry finds it published and skips it), cargo's
#    timeout report for one, curl's timeout for one, and a 429 followed by a
#    503 all end published, with every level in order.
once_crate="$(level_crate 0)"
json_crate="$(level_crate 1)"
landed_crate="$(level_crate 2)"
mixed_crate="$(level_crate 3)"
timeout_crate="$(level_crate 4)"
network_crate="$(level_crate 5)"
prepare server-errors "${once_crate}" server-error-once
prepare server-errors "${json_crate}" server-error-json
prepare server-errors "${landed_crate}" server-error-landed
prepare server-errors "${mixed_crate}" rate-limit-then-server-error
prepare server-errors "${timeout_crate}" timeout-landed
prepare server-errors "${network_crate}" network-landed
publish server-errors 0 MEERKAT_CRATE_PUBLISH_JOBS=4 MEERKAT_CRATE_PUBLISH_SERVER_ERROR_BACKOFF="0 0"
[[ "$(started server-errors | sort -u)" == "${all_crates}" ]] || fail "server-errors: not every crate was published"
check_level_order server-errors
for crate in "${once_crate}" "${json_crate}" "${landed_crate}" "${timeout_crate}" "${network_crate}"; do
  [[ "$(attempts server-errors "${crate}")" == 2 ]] ||
    fail "server-errors: ${crate} took $(attempts server-errors "${crate}") attempts, expected 2"
done
[[ "$(attempts server-errors "${mixed_crate}")" == 3 ]] ||
  fail "server-errors: ${mixed_crate} took $(attempts server-errors "${mixed_crate}") attempts, expected 3 (429, 503, upload)"
for crate in "${once_crate}" "${json_crate}" "${landed_crate}" "${mixed_crate}" "${timeout_crate}"; do
  grep -Fq "${crate}: crates.io server error, publish retry 1/2 in 0s" "${TEST_ROOT}/server-errors/out.log" ||
    fail "server-errors: ${crate} reported no server-error retry"
done
grep -Fq "${network_crate}: network error, publish retry 1/2 in 0s" "${TEST_ROOT}/server-errors/out.log" ||
  fail "server-errors: ${network_crate} reported no network-error retry"
for crate in "${landed_crate}" "${timeout_crate}" "${network_crate}"; do
  grep -Fq "${crate}: already published, skipping" "${TEST_ROOT}/server-errors/out.log" ||
    fail "server-errors: ${crate}'s upload that landed before its error was not skipped"
done
grep -Fq "${mixed_crate}: crates.io rate limited publish attempt 1/12" "${TEST_ROOT}/server-errors/out.log" ||
  fail "server-errors: ${mixed_crate}'s 429 did not take the rate-limit path"
grep -Fq "Every release crate is published" "${TEST_ROOT}/server-errors/out.log" || fail "server-errors: no completion line"
echo "ok: an HTML 503, a JSON 500, a 503 for a landed upload, cargo's and curl's timeouts for one and a 429 then 503 are retried and published"

# 7. Server errors that outlast the budget fail the crate as any failure does:
#    1 + 2 attempts, its status, and no later level.
budget_level=4
budget_crate="$(level_crate "${budget_level}")"
prepare server-error-budget "${budget_crate}" server-error
publish server-error-budget 101 MEERKAT_CRATE_PUBLISH_JOBS=1 MEERKAT_CRATE_PUBLISH_SERVER_ERROR_BACKOFF="0 0"
[[ "$(attempts server-error-budget "${budget_crate}")" == 3 ]] ||
  fail "server-error-budget: ${budget_crate} took $(attempts server-error-budget "${budget_crate}") attempts, expected 3"
later="$(awk -F '\t' -v level="${budget_level}" '$1 > level { print $2 }' <<<"${LEVELS}" | sort)"
if comm -12 <(started server-error-budget | sort -u) <(printf '%s\n' "${later}") | grep -q .; then
  fail "server-error-budget: a later level started after ${budget_crate} failed"
fi
grep -Fq "Level ${budget_level}: ${budget_crate} failed (exit 101)" "${TEST_ROOT}/server-error-budget/out.log" ||
  fail "server-error-budget: the failing crate is not named"
echo "ok: ${budget_crate}'s server errors outlast 2 retries and stop the publication with exit 101"

# 8. An empty delay list retries nothing; a malformed one refuses to start.
prepare server-error-off "${once_crate}" server-error-once
publish server-error-off 101 MEERKAT_CRATE_PUBLISH_SERVER_ERROR_BACKOFF=""
[[ "$(attempts server-error-off "${once_crate}")" == 1 ]] || fail "server-error-off: an empty delay list retried"
publish server-error-malformed 2 MEERKAT_CRATE_PUBLISH_SERVER_ERROR_BACKOFF="30 soon"
[[ ! -s "${TEST_ROOT}/server-error-malformed/events" ]] || fail "server-error-malformed: a crate started"
grep -Fq "MEERKAT_CRATE_PUBLISH_SERVER_ERROR_BACKOFF must list whole seconds, got soon" "${TEST_ROOT}/server-error-malformed/out.log" ||
  fail "server-error-malformed: the bad delay is not named"
echo "ok: an empty server-error delay list disables the retry and a malformed one refuses to start"

# 9. The patch config: by default every upload carries it (run 1 above);
#    MEERKAT_CRATE_PUBLISH_PATCH=false drops it from every real upload and
#    keeps it on a dry run; any other value refuses to start.
configs_with() {
  awk -v want="$2" '$2 == want' "${TEST_ROOT}/$1/configs" | wc -l | tr -d ' '
}
crate_count="$(wc -l <<<"${all_crates}" | tr -d ' ')"
[[ "$(configs_with uploads no)" == 0 && "$(configs_with uploads yes)" -gt 0 ]] ||
  fail "patch: a default upload ran without the patch config"
publish patch-off 0 MEERKAT_CRATE_PUBLISH_JOBS=4 MEERKAT_CRATE_PUBLISH_PATCH=false
[[ "$(started patch-off)" == "${all_crates}" ]] || fail "patch-off: not every crate was published exactly once"
check_level_order patch-off
[[ "$(configs_with patch-off yes)" == 0 && "$(configs_with patch-off no)" == "${crate_count}" ]] ||
  fail "patch-off: an upload carried the patch config" "$(cat "${TEST_ROOT}/patch-off/configs")"
grep -Fq "upload without the patch config" "${TEST_ROOT}/patch-off/out.log" ||
  fail "patch-off: the patch-free upload is not reported"
publish patch-off-dry 0 MEERKAT_CRATE_PUBLISH_PATCH=false MEERKAT_REGISTRY_DRY_RUN=true
[[ "$(configs_with patch-off-dry no)" == 0 && "$(configs_with patch-off-dry yes)" == "${crate_count}" ]] ||
  fail "patch-off-dry: a dry run dropped the patch config"
publish patch-malformed 2 MEERKAT_CRATE_PUBLISH_PATCH=no
[[ ! -s "${TEST_ROOT}/patch-malformed/events" ]] || fail "patch-malformed: a crate started"
grep -Fq "MEERKAT_CRATE_PUBLISH_PATCH must be true or false, got no" "${TEST_ROOT}/patch-malformed/out.log" ||
  fail "patch-malformed: the bad value is not named"
echo "ok: MEERKAT_CRATE_PUBLISH_PATCH=false drops the patch config from every upload, keeps it on a dry run, and only true or false start"

echo "level-parallel publisher contract holds"
