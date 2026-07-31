#!/usr/bin/env bash
# If machine-related files changed, run codegen + verify
set -euo pipefail

ROOT="${ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
CARGO="${CARGO:-$ROOT/scripts/repo-cargo}"
MAKE_BIN="${MAKE_BIN:-make}"
MACHINE_AUTHORITY_CHANGED="${MACHINE_AUTHORITY_CHANGED:-$ROOT/scripts/machine-authority-changed}"
GIT_BIN="${GIT_BIN:-git}"
cd "$ROOT"

from_ref="${PRE_COMMIT_FROM_REF:-}"
to_ref="${PRE_COMMIT_TO_REF:-}"
if [[ -z "$from_ref" || -z "$to_ref" ]]; then
    echo "machine pre-push validation requires exact refs from pre-push-dispatch.sh" >&2
    echo "Reinstall repository hooks with: make install-hooks" >&2
    exit 1
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

echo "Machine files changed, running codegen + verify..."
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
# Route verification through the canonical TLC lane: it owns the documented
# over-budget composition skips (meerkat_mob_seam / adaptive_mob_bundle full
# sweeps) and the bounded adaptive witness proof. A bare `machine-verify --all`
# runs the full mob-seam ci.cfg sweep, which does not fit a pre-push budget.
"$MAKE_BIN" -C "$ROOT" machine-verify
