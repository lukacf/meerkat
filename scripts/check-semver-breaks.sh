#!/usr/bin/env bash
# Semver-break detection gate (M3 policy: exact-pin + break detection).
#
# Policy: meerkat 0.x PATCH releases MAY contain breaking public-API changes
# (pre-1.0 clean-break discipline). Downstreams must exact-pin (`=0.7.24`).
# In exchange, every release that breaks public API must SAY SO: this gate
# runs cargo-semver-checks against the last published crates.io baseline and
# fails the release when a reported break is not NAMED in the pending release
# notes' `### Breaking` section, or when those notes are not STAMPED against
# the version being released.
#
# This script is the mechanical half: install check, run the tool, hand the
# report to `scripts/check_semver_breaks.py`, which owns every judgement and is
# unit-tested against committed real reports. There is deliberately no env
# override that relaxes the analyser.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"
PYTHON="${PYTHON:-$(command -v python3.11 2>/dev/null || command -v python3)}"

# Self-test the analyser before trusting its verdict. A parser that has drifted
# from the report grammar reports "no breaks" for the same reason a clean
# release does, and that is the failure mode this gate exists to prevent.
selftest_log="$(mktemp)"
trap 'rm -f "$selftest_log"' EXIT
if ! "$PYTHON" "$ROOT/scripts/test_check_semver_breaks.py" >"$selftest_log" 2>&1; then
    cat "$selftest_log" >&2
    echo "error: the semver-breaks analyser failed its own unit tests" >&2
    exit 1
fi
echo "semver-breaks: analyser self-test passed"

if ! command -v cargo-semver-checks >/dev/null 2>&1; then
    echo "error: cargo-semver-checks is required for the semver-breaks gate" >&2
    echo "install: cargo install cargo-semver-checks --locked" >&2
    exit 1
fi

# Check the full publishable workspace against the latest published baselines.
# `--release-type patch` declares our intent (we ship patches); any detected
# break is then a REPORTED violation we convert into a changelog obligation
# rather than a hard stop.
report_file="$(mktemp)"
trap 'rm -f "$selftest_log" "$report_file"' EXIT
tool_exit=0
"$ROOT/scripts/repo-cargo" semver-checks check-release --workspace --release-type patch \
    >"$report_file" 2>&1 || tool_exit=$?

# Every crate the release publishes and that cargo-semver-checks can look at
# must appear in the report. A run that died after twelve crates exits non-zero
# with real findings, and judging only the findings it managed to print would
# call a partial measurement a pass. The analyser decides which crates the tool
# can look at (proc-macro crates are absent from its output entirely).
release_args=()
while IFS= read -r crate; do
    [[ -n "$crate" ]] || continue
    release_args+=(--release-crate "$crate")
done < <("$ROOT/scripts/release-rust-crates.sh")

analyser_exit=0
"$PYTHON" "$ROOT/scripts/check_semver_breaks.py" \
    --report "$report_file" \
    --changelog "$ROOT/CHANGELOG.md" \
    --repo-root "$ROOT" \
    --tool-exit-code "$tool_exit" \
    "${release_args[@]}" || analyser_exit=$?

# Release-readiness CI preserves the complete measurement as exact-tree
# evidence. This output path is observational only and cannot relax the
# analyser verdict.
if [[ -n "${MEERKAT_SEMVER_REPORT_OUT:-}" ]]; then
    mkdir -p "$(dirname "$MEERKAT_SEMVER_REPORT_OUT")"
    cp "$report_file" "$MEERKAT_SEMVER_REPORT_OUT"
fi

if [[ "$analyser_exit" -ne 0 ]]; then
    echo >&2
    echo "cargo-semver-checks exited ${tool_exit}; last 40 report lines:" >&2
    tail -40 "$report_file" >&2
fi

exit "$analyser_exit"
