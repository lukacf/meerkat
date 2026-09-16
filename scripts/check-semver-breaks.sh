#!/usr/bin/env bash
# Public-API break gate against the published baselines.
#
# Measures only the publishable crates whose source directory or declared
# dependency specs differ from the published baseline release tag
# (scripts/semver_changed_crates.py). cargo-semver-checks reports on a crate's
# own items, which cannot differ when nothing the crate is built from differs,
# so baseline-identical crates are recorded as reached by equivalence rather
# than rebuilt.
#
# Neither side of a measurement is built inside cargo-semver-checks:
#   - the baseline side is the rustdoc JSON the release workflow attached to
#     the baseline's GitHub release (`semver-rustdoc-<version>.tar.zst`,
#     produced by scripts/semver-rustdoc-json.sh on the same pinned rustc);
#   - the candidate side is produced by the same script on this tree, in one
#     cargo invocation for every publishable library crate.
# The tool then compares JSON to JSON (`--baseline-rustdoc`,
# `--current-rustdoc`) in seconds per crate. Only when the baseline asset is
# missing, or was built by a different rustc, does the gate check out the
# baseline tag and run the generator there as well.
#
# Environment:
#   MEERKAT_SEMVER_BASELINE_VERSION      override the baseline version (default:
#                                        the newest meerkat-core version on crates.io)
#   MEERKAT_SEMVER_BASELINE_RUSTDOC_DIR  use this directory of baseline rustdoc
#                                        JSON (manifest.json + <crate>.json)
#                                        instead of downloading the release asset
#   MEERKAT_SEMVER_REPORT_OUT            copy the tool report to this path
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

PYTHON="${PYTHON:-$(command -v python3.11 2>/dev/null || command -v python3)}"

selftest_log="$(mktemp)"
report_file="$(mktemp)"
classification_file="$(mktemp)"
baseline_dir=""
rustdoc_download_dir=""
current_rustdoc_dir=""
cleanup() {
    rm -f "$selftest_log" "$report_file" "$classification_file"
    if [[ -n "$baseline_dir" ]]; then
        git worktree remove --force "$baseline_dir" >/dev/null 2>&1 || rm -rf "$baseline_dir"
    fi
    if [[ -n "$rustdoc_download_dir" ]]; then
        rm -rf "$rustdoc_download_dir"
    fi
    if [[ -n "$current_rustdoc_dir" ]]; then
        rm -rf "$current_rustdoc_dir"
    fi
}
trap cleanup EXIT

if ! "$PYTHON" "$ROOT/scripts/test_check_semver_breaks.py" >"$selftest_log" 2>&1 \
    || ! "$PYTHON" "$ROOT/scripts/test_semver_changed_crates.py" >>"$selftest_log" 2>&1; then
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

workspace_version="$(
    "$PYTHON" -c 'import pathlib,tomllib; print(tomllib.loads(pathlib.Path("Cargo.toml").read_text())["workspace"]["package"]["version"])'
)"

# The baseline is the newest published release. crates.io is the source of
# truth because the gate exists to protect exact-pinned downstreams of what is
# actually published, not of what a local tag claims.
baseline_version="${MEERKAT_SEMVER_BASELINE_VERSION:-}"
if [[ -z "$baseline_version" ]]; then
    baseline_version="$(
        curl -fsSL --retry 6 --retry-delay 10 \
            -H 'User-Agent: meerkat-semver-breaks (https://github.com/lukacf/meerkat)' \
            'https://crates.io/api/v1/crates/meerkat-core' \
            | "$PYTHON" -c 'import json,sys; print(json.load(sys.stdin)["crate"]["max_version"])'
    )" || {
        echo "error: could not resolve the published baseline version from crates.io" >&2
        echo "set MEERKAT_SEMVER_BASELINE_VERSION explicitly to retry offline" >&2
        exit 1
    }
fi
if [[ "$baseline_version" == "$workspace_version" ]]; then
    echo "error: workspace version ${workspace_version} is already the published baseline" >&2
    exit 1
fi
baseline_tag="v${baseline_version}"
echo "semver-breaks: baseline ${baseline_tag}, candidate ${workspace_version}"

if ! git rev-parse -q --verify "refs/tags/${baseline_tag}^{commit}" >/dev/null 2>&1; then
    echo "semver-breaks: fetching ${baseline_tag}"
    git fetch --no-tags --depth=1 origin "+refs/tags/${baseline_tag}:refs/tags/${baseline_tag}"
fi

mapfile -t release_crates < <("$ROOT/scripts/release-rust-crates.sh")

classify_args=()
for crate in "${release_crates[@]}"; do
    [[ -n "$crate" ]] || continue
    classify_args+=(--release-crate "$crate")
done
"$PYTHON" "$ROOT/scripts/semver_changed_crates.py" \
    --repo-root "$ROOT" --baseline-tag "$baseline_tag" --head HEAD \
    "${classify_args[@]}" >"$classification_file"

classified() {
    "$PYTHON" -c 'import json,sys; [print(c) for c in json.load(open(sys.argv[1]))[sys.argv[2]]]' "$classification_file" "$1"
}
mapfile -t changed_crates < <(classified changed)
mapfile -t unchanged_crates < <(classified unchanged)
mapfile -t first_publish_crates < <(classified first_publish)

echo "semver-breaks: ${#changed_crates[@]} crate(s) differ from ${baseline_tag}, ${#unchanged_crates[@]} identical, ${#first_publish_crates[@]} first publication"
{
    echo "# semver-breaks classification against ${baseline_tag}"
    cat "$classification_file"
    echo
} >"$report_file"

# Published baseline rustdoc JSON. Left empty when unusable; the gate then
# generates the baseline itself from a checkout of the tag.
baseline_rustdoc_dir="${MEERKAT_SEMVER_BASELINE_RUSTDOC_DIR:-}"
baseline_rustdoc_note=""
local_rustc="$(rustc -Vv | awk '/^commit-hash:/ {print $2}')"
manifest_field() {
    "$PYTHON" -c 'import json,sys; print(json.load(open(sys.argv[1]))[sys.argv[2]])' "$1" "$2"
}
manifest_has_crate() {
    "$PYTHON" -c 'import json,sys; sys.exit(0 if sys.argv[2] in json.load(open(sys.argv[1]))["crates"] else 1)' "$1" "$2"
}
if [[ -z "$baseline_rustdoc_dir" && ${#changed_crates[@]} -gt 0 ]]; then
    asset="semver-rustdoc-${baseline_version}.tar.zst"
    asset_url="https://github.com/lukacf/meerkat/releases/download/${baseline_tag}/${asset}"
    rustdoc_download_dir="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-semver-rustdoc.XXXXXX")"
    if curl -fsSL --retry 3 -o "$rustdoc_download_dir/$asset" "$asset_url" \
        && tar --zstd -xf "$rustdoc_download_dir/$asset" -C "$rustdoc_download_dir"; then
        baseline_rustdoc_dir="$rustdoc_download_dir"
        echo "semver-breaks: baseline rustdoc asset ${asset} downloaded"
    else
        baseline_rustdoc_note="no baseline rustdoc asset at ${asset_url}"
    fi
fi
if [[ -n "$baseline_rustdoc_dir" ]]; then
    if [[ ! -f "$baseline_rustdoc_dir/manifest.json" ]]; then
        baseline_rustdoc_note="baseline rustdoc directory ${baseline_rustdoc_dir} has no manifest.json"
        baseline_rustdoc_dir=""
    else
        manifest_version="$(manifest_field "$baseline_rustdoc_dir/manifest.json" version)"
        manifest_rustc="$(manifest_field "$baseline_rustdoc_dir/manifest.json" rustc_commit_hash)"
        if [[ "$manifest_version" != "$baseline_version" ]]; then
            echo "error: baseline rustdoc manifest is for ${manifest_version}, expected ${baseline_version}" >&2
            exit 1
        fi
        if [[ "$manifest_rustc" != "$local_rustc" ]]; then
            baseline_rustdoc_note="baseline rustdoc was built by rustc ${manifest_rustc}, this run uses ${local_rustc}"
            baseline_rustdoc_dir=""
        fi
    fi
fi

tool_exit=0
tool_skipped=false
run_tool() {
    local status=0
    "$ROOT/scripts/repo-cargo" semver-checks check-release --release-type patch "$@" \
        >>"$report_file" 2>&1 || status=$?
    if [[ "$status" -ne 0 ]]; then
        tool_exit="$status"
    fi
}
if [[ ${#changed_crates[@]} -eq 0 ]]; then
    tool_skipped=true
    echo "semver-breaks: every publishable crate is identical to ${baseline_tag}; nothing to measure"
else
    # Candidate side: one build for every publishable library crate, so the
    # feature unification matches the published baseline's.
    current_rustdoc_dir="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-semver-current.XXXXXX")"
    echo "semver-breaks: generating candidate rustdoc JSON"
    "$ROOT/scripts/semver-rustdoc-json.sh" --source-root "$ROOT" --out "$current_rustdoc_dir" >/dev/null

    # Only library crates are measurable; the analyser already knows that
    # proc-macro and binary-only crates never appear in a report.
    measured_crates=()
    for crate in "${changed_crates[@]}"; do
        [[ -n "$crate" ]] || continue
        if manifest_has_crate "$current_rustdoc_dir/manifest.json" "$crate"; then
            measured_crates+=("$crate")
        else
            echo "# ${crate}: not a library crate, nothing for cargo-semver-checks to measure" >>"$report_file"
        fi
    done

    if [[ -n "$baseline_rustdoc_dir" ]]; then
        for crate in "${measured_crates[@]}"; do
            if [[ ! -f "$baseline_rustdoc_dir/${crate}.json" ]]; then
                baseline_rustdoc_note="published baseline rustdoc has no ${crate}.json"
                baseline_rustdoc_dir=""
                break
            fi
        done
    fi
    if [[ -z "$baseline_rustdoc_dir" ]]; then
        echo "semver-breaks: warning: ${baseline_rustdoc_note}; generating the baseline from a ${baseline_tag} checkout" >&2
        echo "# ${baseline_rustdoc_note}; baseline generated from a ${baseline_tag} checkout" >>"$report_file"
        baseline_dir="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-semver-baseline.XXXXXX")"
        git worktree add --detach --force "$baseline_dir" "${baseline_tag}" >/dev/null
        baseline_rustdoc_dir="$baseline_dir/.semver-rustdoc"
        "$ROOT/scripts/semver-rustdoc-json.sh" --source-root "$baseline_dir" --out "$baseline_rustdoc_dir" >/dev/null
    else
        echo "# baseline: published rustdoc JSON of ${baseline_tag}" >>"$report_file"
    fi

    for crate in "${measured_crates[@]}"; do
        echo "semver-breaks: measuring ${crate}"
        run_tool --package "$crate" \
            --baseline-rustdoc "$baseline_rustdoc_dir/${crate}.json" \
            --current-rustdoc "$current_rustdoc_dir/${crate}.json"
    done
fi

release_args=()
for crate in "${release_crates[@]}"; do
    [[ -n "$crate" ]] || continue
    release_args+=(--release-crate "$crate")
done
for crate in "${unchanged_crates[@]}"; do
    [[ -n "$crate" ]] || continue
    release_args+=(--unchanged-crate "$crate")
done
for crate in "${first_publish_crates[@]}"; do
    [[ -n "$crate" ]] || continue
    release_args+=(--first-publish-crate "$crate")
done
if [[ "$tool_skipped" == true ]]; then
    release_args+=(--tool-skipped)
fi

analyser_exit=0
"$PYTHON" "$ROOT/scripts/check_semver_breaks.py" \
    --report "$report_file" \
    --changelog "$ROOT/CHANGELOG.md" \
    --repo-root "$ROOT" \
    --baseline-tag "$baseline_tag" \
    --tool-exit-code "$tool_exit" \
    "${release_args[@]}" || analyser_exit=$?

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
