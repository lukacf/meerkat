#!/usr/bin/env bash
# Sync canonical Meerkat dogma docs into the repo-local skill package.
#
# Modes:
#   (default)  rewrite stale skill copies and exit 1 so they can be staged
#              (pre-commit auto-fix convenience).
#   --check    verify-only rerun-and-diff gate: never writes; exits 1 when the
#              skill mirror has drifted from the canonical docs (pre-push gate,
#              so doctrine drift cannot be pushed).

set -euo pipefail

CHECK_ONLY=0
if [[ "${1:-}" == "--check" ]]; then
  CHECK_ONLY=1
elif [[ -n "${1:-}" ]]; then
  printf 'unknown argument: %s (expected --check or nothing)\n' "$1" >&2
  exit 2
fi

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CANONICAL_DIR="$ROOT/docs/architecture"
SKILL_REF_DIR="$ROOT/.claude/skills/meerkat-dogma-inquisition/references"

docs=(
  "meerkat-dogma.md"
  "meerkat-dogma-commentary.md"
)

if [[ "$CHECK_ONLY" -eq 0 ]]; then
  mkdir -p "$SKILL_REF_DIR"
fi

changed=0

for doc in "${docs[@]}"; do
  src="$CANONICAL_DIR/$doc"
  dst="$SKILL_REF_DIR/$doc"

  if [[ ! -f "$src" ]]; then
    printf 'missing canonical dogma doc: %s\n' "$src" >&2
    exit 1
  fi

  if [[ ! -f "$dst" ]] || ! cmp -s "$src" "$dst"; then
    if [[ "$CHECK_ONLY" -eq 1 ]]; then
      printf 'dogma skill mirror is stale: %s differs from %s\n' \
        "${dst#$ROOT/}" "${src#$ROOT/}" >&2
      changed=1
    else
      cp "$src" "$dst"
      printf 'synced %s -> %s\n' "${src#$ROOT/}" "${dst#$ROOT/}" >&2
      changed=1
    fi
  fi
done

if [[ "$changed" -ne 0 ]]; then
  if [[ "$CHECK_ONLY" -eq 1 ]]; then
    cat >&2 <<'MSG'

Meerkat dogma skill docs have drifted from docs/architecture.
Run scripts/sync-meerkat-dogma-skill-docs.sh, commit the refreshed
.claude/skills/meerkat-dogma-inquisition/references files, and push again.
MSG
  else
    cat >&2 <<'MSG'

Meerkat dogma skill docs were refreshed from docs/architecture.
Stage the updated .claude/skills/meerkat-dogma-inquisition/references files and retry.
MSG
  fi
  exit 1
fi

printf 'Meerkat dogma skill docs are in sync.\n'
