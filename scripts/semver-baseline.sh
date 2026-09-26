# shellcheck shell=bash
# The inputs below are set by the caller and the outputs read by it.
# shellcheck disable=SC2154,SC2034
# Baseline selection for the semver-breaks gate. Sourced by
# scripts/check-semver-breaks.sh and by its self-test,
# scripts/test-semver-baseline.sh; not executable on its own.
#
# A release tree carries its notes stamped `## [<version>] - DATE` and declares
# the breaks since the newest published release, so that is its baseline.
#
# After a release is cut the workspace version stays at it until the next bump,
# and new notes gather under a non-empty `## [Unreleased]` above the stamped
# section. Those notes declare the breaks since the workspace version, so once
# HEAD has moved past the workspace version's tag, that tag is the baseline.
# Measured against the release below it instead, the stamped release's own
# breaks are reported again and demanded again under `## [Unreleased]`, which
# fails every such tree until crates.io publishes the workspace version.
#
# Inputs (variables set by the caller):
#   PYTHON                           Python 3.11+ interpreter
#   semver_analyser                  path to scripts/check_semver_breaks.py
#   semver_changelog                 path to the CHANGELOG.md under test
#   workspace_version                workspace.package.version of HEAD
#   MEERKAT_SEMVER_BASELINE_VERSION  optional explicit baseline override
#   MEERKAT_SEMVER_REQUIRE_RELEASE_TREE  "1" refuses a post-release tree: the
#                                    release workflow's own measurement sets it,
#                                    because a tree whose notes follow the
#                                    stamped version is not that version's
#                                    release and must never publish as it
# Git commands run in the current directory, which must be the repository.
#
# Outputs (variables set on success):
#   baseline_version                 version whose tag is the baseline
#   post_release                     true when the baseline is the workspace
#                                    version's own tag, else false

# The newest published release. crates.io is the source of truth because the
# gate exists to protect exact-pinned downstreams of what is actually
# published, not of what a local tag claims. The self-test redefines this.
semver_published_version() {
    curl -fsSL --retry 6 --retry-delay 10 \
        -H 'User-Agent: meerkat-semver-breaks (https://github.com/lukacf/meerkat)' \
        'https://crates.io/api/v1/crates/meerkat-core' \
        | "$PYTHON" -c 'import json,sys; print(json.load(sys.stdin)["crate"]["max_version"])'
}

# Make origin's tag $1 local. Returns 0 when fetched, 2 when origin has no such
# tag, and 1 (after saying why) when origin could not be asked: an offline
# machine or a fork without the tag is not evidence the tag does not exist.
semver_fetch_release_tag() {
    local tag="$1" output status
    if output="$(git ls-remote --exit-code --tags origin "refs/tags/${tag}" 2>&1)"; then
        status=0
    else
        status=$?
    fi
    case "$status" in
        0) ;;
        2) return 2 ;;
        *)
            echo "error: could not ask origin whether ${tag} exists (git ls-remote exited ${status}): ${output}" >&2
            return 1
            ;;
    esac
    if ! output="$(git fetch --no-tags origin "+refs/tags/${tag}:refs/tags/${tag}" 2>&1)"; then
        echo "error: origin has ${tag} but fetching it failed: ${output}" >&2
        return 1
    fi
    return 0
}

# Returns non-zero, after explaining why on stderr, when no baseline applies.
resolve_semver_baseline() {
    baseline_version="${MEERKAT_SEMVER_BASELINE_VERSION:-}"
    post_release=false

    local notes_baseline
    if ! notes_baseline="$(
        "$PYTHON" "$semver_analyser" --notes-baseline \
            --changelog "$semver_changelog" --version "$workspace_version"
    )"; then
        echo "error: could not classify the pending CHANGELOG.md notes" >&2
        return 1
    fi
    case "$notes_baseline" in
        published | workspace-version) ;;
        *)
            echo "error: unexpected notes baseline '${notes_baseline}' from ${semver_analyser}" >&2
            return 1
            ;;
    esac

    local release_tag="v${workspace_version}"
    if [[ "$notes_baseline" == "workspace-version" && "${MEERKAT_SEMVER_REQUIRE_RELEASE_TREE:-}" == "1" ]]; then
        echo "error: this is a post-release tree: its pending \`## [Unreleased]\` notes sit above the stamped" >&2
        echo "       ${workspace_version} section, so it is not the ${workspace_version} release and must not publish as it." >&2
        echo "       Release the tree whose notes are stamped for the version it publishes (the release commit" >&2
        echo "       cargo-release makes), or ${release_tag} itself." >&2
        return 1
    fi

    if [[ -z "$baseline_version" && "$notes_baseline" == "workspace-version" ]]; then
        local release_commit head_commit fetch_status shallow newest_published
        if ! git rev-parse -q --verify "refs/tags/${release_tag}^{commit}" >/dev/null 2>&1; then
            if semver_fetch_release_tag "$release_tag"; then
                fetch_status=0
            else
                fetch_status=$?
            fi
            case "$fetch_status" in
                0) ;;
                2)
                    echo "error: the pending \`## [Unreleased]\` notes sit above the stamped ${workspace_version} section," >&2
                    echo "       but ${release_tag} is not tagged, locally or on origin. If this is the ${workspace_version}" >&2
                    echo "       release commit, those notes belong to ${workspace_version}: move them into its stamped" >&2
                    echo "       section. Otherwise they follow ${release_tag}: measure them once it is tagged." >&2
                    return 1
                    ;;
                *) return 1 ;;
            esac
        fi
        if ! release_commit="$(git rev-parse -q --verify "refs/tags/${release_tag}^{commit}")"; then
            echo "error: ${release_tag} does not name a commit" >&2
            return 1
        fi
        if ! head_commit="$(git rev-parse -q --verify 'HEAD^{commit}')"; then
            echo "error: HEAD does not name a commit" >&2
            return 1
        fi
        # At the tagged commit itself nothing has moved past the release, so
        # the published baseline below applies exactly as it did before.
        if [[ "$release_commit" != "$head_commit" ]]; then
            if ! git merge-base --is-ancestor "$release_commit" "$head_commit"; then
                shallow="$(git rev-parse --is-shallow-repository 2>/dev/null || echo unknown)"
                if [[ "$shallow" != "false" ]]; then
                    echo "error: this clone is shallow (or its depth is unknown), so it cannot tell whether" >&2
                    echo "       ${release_tag} is an ancestor of HEAD. Fetch full history (git fetch --unshallow," >&2
                    echo "       or actions/checkout with fetch-depth: 0) and measure again." >&2
                else
                    echo "error: the pending \`## [Unreleased]\` notes sit above the stamped ${workspace_version} section," >&2
                    echo "       but ${release_tag} is not an ancestor of HEAD, so it is not the release they follow." >&2
                fi
                return 1
            fi
            baseline_version="$workspace_version"
            post_release=true
            # The next release commit is measured against crates.io's newest
            # release, not this tag; say so when the two differ.
            if newest_published="$(semver_published_version 2>/dev/null)" && [[ -n "$newest_published" ]]; then
                if [[ "$newest_published" != "$workspace_version" ]]; then
                    echo "warning: crates.io's newest meerkat-core is ${newest_published}, not ${workspace_version} (unpublished," >&2
                    echo "         yanked, or a published prerelease). These notes are measured against ${release_tag}, but" >&2
                    echo "         the next release commit is measured against ${newest_published}, so this run may not predict it." >&2
                fi
            else
                echo "warning: could not read crates.io's newest meerkat-core to compare with ${release_tag}; the next" >&2
                echo "         release commit is measured against it, so this run may not predict that measurement." >&2
            fi
            return 0
        fi
    fi

    if [[ -z "$baseline_version" ]]; then
        if ! baseline_version="$(semver_published_version)" || [[ -z "$baseline_version" ]]; then
            echo "error: could not resolve the published baseline version from crates.io" >&2
            echo "set MEERKAT_SEMVER_BASELINE_VERSION explicitly to retry offline" >&2
            return 1
        fi
    fi
    if [[ "$baseline_version" == "$workspace_version" ]]; then
        echo "error: workspace version ${workspace_version} is already the published baseline" >&2
        return 1
    fi
    return 0
}
