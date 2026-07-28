#!/usr/bin/env bash
# Semver-break detection gate (M3 policy: exact-pin + break detection).
#
# Policy: meerkat 0.x PATCH releases MAY contain breaking public-API changes
# (pre-1.0 clean-break discipline). Downstreams must exact-pin (`=0.7.24`).
# In exchange, every release that breaks public API must SAY SO: this gate
# runs cargo-semver-checks against the last published crates.io baseline and
# fails the release preflight when breaks exist without a `### Breaking`
# section in the changelog's pending (topmost) release notes.
set -euo pipefail

cd "$(dirname "$0")/.."

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
trap 'rm -f "$report_file"' EXIT
breaks_found=0
if ! cargo semver-checks check-release --workspace --release-type patch \
    >"$report_file" 2>&1; then
    breaks_found=1
fi

if [[ "$breaks_found" -eq 0 ]]; then
    echo "semver-breaks: no public-API breaks vs the published baselines"
    exit 0
fi

# Breaks exist: they are allowed by policy, but only when loudly declared.
# The pending release notes are the topmost section of CHANGELOG.md —
# `## [Unreleased]` when populated, otherwise the topmost stamped release.
#
# Stamping a release leaves an EMPTY `## [Unreleased]` stub above it, so
# "topmost section" and "the section this release is declared in" stop being
# the same thing at exactly the moment this gate runs. Reading the stub would
# fail every stamped release no matter how loudly it declared its breaks,
# which is a gate that cannot be satisfied rather than one that is hard to
# satisfy. Skip a content-free pending section and read the one below it.
pending_section="$(awk '
    /^## \[/ { count++ }
    count >= 1 && count <= 2 { section[count] = section[count] $0 "\n" }
    count == 3 { exit }
    END {
        body = section[1]
        sub(/^## \[[^\n]*\n/, "", body)
        gsub(/[[:space:]]/, "", body)
        printf "%s", (length(body) > 0 ? section[1] : section[2])
    }
' CHANGELOG.md)"
if ! grep -q '^### Breaking' <<<"$pending_section"; then
    echo "semver-breaks: public-API breaks detected vs published baselines:" >&2
    echo >&2
    grep -E "^(--- failure|Summary|.*semver requires)" "$report_file" >&2 || tail -40 "$report_file" >&2
    echo >&2
    echo "error: the pending CHANGELOG.md section has no '### Breaking' heading." >&2
    echo "Policy (M3): 0.x patch releases may break public API, but every break" >&2
    echo "must be declared under '### Breaking' with the changed signatures so" >&2
    echo "exact-pinned downstreams can plan the bump. Add the section and rerun." >&2
    exit 1
fi

echo "semver-breaks: public-API breaks detected and declared under '### Breaking':"
grep -E "^(--- failure|Summary)" "$report_file" || true
exit 0
