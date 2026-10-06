#!/usr/bin/env bash
# If machine-related files changed, run codegen + verify.
#
#   --codegen-only  machine-codegen and protocol-codegen under the clean-tree
#                   contract: the local machine/protocol drift check for the
#                   exact pushed refs (pre-push hook machine-codegen-drift,
#                   which the dispatcher refuses to SKIP)
#   --verify-only   the canonical TLC lane (pre-push hook
#                   machine-codegen-verify)
#   (no flag)       both, in that order
set -euo pipefail

run_codegen=1
run_verify=1
case "${1:-}" in
    --codegen-only) run_verify=0 ;;
    --verify-only) run_codegen=0 ;;
    "") ;;
    *)
        echo "usage: pre-push-machines.sh [--codegen-only|--verify-only]" >&2
        exit 2
        ;;
esac

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="${ROOT:-$(cd "$SCRIPT_DIR/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"
MAKE_BIN="${MAKE_BIN:-make}"
MACHINE_AUTHORITY_CHANGED="${MACHINE_AUTHORITY_CHANGED:-$ROOT/scripts/machine-authority-changed}"
RELEASE_PROJECTION_ONLY="${RELEASE_PROJECTION_ONLY:-$SCRIPT_DIR/release-projection-only.mjs}"
GIT_BIN="${GIT_BIN:-git}"
cd "$ROOT"

from_ref="${PRE_COMMIT_FROM_REF:-}"
to_ref="${PRE_COMMIT_TO_REF:-}"
if [[ -z "$from_ref" || -z "$to_ref" ]]; then
    echo "machine pre-push validation requires exact refs from pre-push-dispatch.sh" >&2
    echo "Reinstall repository hooks with: make install-hooks" >&2
    exit 1
fi

if "$RELEASE_PROJECTION_ONLY" \
    --base "$from_ref" --head "$to_ref"; then
    echo "Release projection only; machine authority is unchanged."
    exit 0
else
    release_projection_status=$?
    if [[ "$release_projection_status" -ne 1 ]]; then
        echo "release projection classification failed with status ${release_projection_status}" >&2
        exit "$release_projection_status"
    fi
fi

if "$MACHINE_AUTHORITY_CHANGED" --base "$from_ref" --head "$to_ref" >/dev/null; then
    classifier_status=0
else
    classifier_status=$?
fi
case "$classifier_status" in
    1)
        exit 0
        ;;
    0)
        ;;
    *)
        echo "machine-authority change classification failed with status ${classifier_status}" >&2
        exit "$classifier_status"
        ;;
esac

if [[ "$run_codegen" -eq 1 ]]; then
echo "Machine files changed, running codegen (drift check)..."
"$CARGO" xtask machine-codegen --all
if ! worktree_status="$("$GIT_BIN" status --porcelain=v1 --untracked-files=all)"; then
    echo "Failed to determine exact-tree cleanliness after machine codegen." >&2
    exit 1
fi
if [[ -n "$worktree_status" ]]; then
    echo "Machine codegen changed the exact pushed tree; commit generated artifacts and retry." >&2
    "$GIT_BIN" status --short --untracked-files=all >&2
    exit 1
fi
# protocol-codegen is a second renderer over the same authority catalog, with
# its own artifact set (handoff protocol helpers, terminal surface mapping,
# generated authority contracts). Regenerate it under the same clean-tree
# contract so a DSL edit cannot be pushed without its emitted counterpart.
"$CARGO" xtask protocol-codegen
if ! worktree_status="$("$GIT_BIN" status --porcelain=v1 --untracked-files=all)"; then
    echo "Failed to determine exact-tree cleanliness after protocol codegen." >&2
    exit 1
fi
if [[ -n "$worktree_status" ]]; then
    echo "Protocol codegen changed the exact pushed tree; commit generated artifacts and retry." >&2
    "$GIT_BIN" status --short --untracked-files=all >&2
    exit 1
fi
fi
if [[ "$run_verify" -eq 1 ]]; then
echo "Machine files changed, running the canonical TLC lane..."
# Route verification through the canonical TLC lane: it owns the documented
# over-budget composition skips (meerkat_mob_seam / adaptive_mob_bundle full
# sweeps) and the bounded adaptive witness proof. A bare `machine-verify --all`
# runs the full mob-seam ci.cfg sweep, which does not fit a pre-push budget.
"$MAKE_BIN" -C "$ROOT" machine-verify
fi
