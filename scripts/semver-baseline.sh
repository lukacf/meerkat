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

    if [[ -z "$baseline_version" && "$notes_baseline" == "workspace-version" ]]; then
        local release_tag="v${workspace_version}"
        local release_commit head_commit
        if ! git rev-parse -q --verify "refs/tags/${release_tag}^{commit}" >/dev/null 2>&1; then
            git fetch --no-tags origin "+refs/tags/${release_tag}:refs/tags/${release_tag}" \
                >/dev/null 2>&1 || true
        fi
        if ! release_commit="$(git rev-parse -q --verify "refs/tags/${release_tag}^{commit}")"; then
            echo "error: the pending \`## [Unreleased]\` notes follow the stamped ${workspace_version} release," >&2
            echo "       so they declare the breaks since ${release_tag}, but ${release_tag} is not tagged yet." >&2
            echo "       Measure them once ${release_tag} exists, or set MEERKAT_SEMVER_BASELINE_VERSION." >&2
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
                echo "error: the pending \`## [Unreleased]\` notes follow the stamped ${workspace_version} release," >&2
                echo "       but ${release_tag} is not an ancestor of HEAD, so it is not the release they follow." >&2
                return 1
            fi
            baseline_version="$workspace_version"
            post_release=true
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
