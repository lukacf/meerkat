#!/usr/bin/env bash
# Sync canonical Meerkat dogma docs into the repo-local skill package.

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CANONICAL_DIR="$ROOT/docs/architecture"
SKILL_REF_DIR="$ROOT/.claude/skills/meerkat-dogma-inquisition/references"

docs=(
  "meerkat-dogma.md"
  "meerkat-dogma-commentary.md"
)

mkdir -p "$SKILL_REF_DIR"

changed=0

for doc in "${docs[@]}"; do
  src="$CANONICAL_DIR/$doc"
  dst="$SKILL_REF_DIR/$doc"

  if [[ ! -f "$src" ]]; then
    printf 'missing canonical dogma doc: %s\n' "$src" >&2
    exit 1
  fi

  if [[ ! -f "$dst" ]] || ! cmp -s "$src" "$dst"; then
    cp "$src" "$dst"
    printf 'synced %s -> %s\n' "${src#$ROOT/}" "${dst#$ROOT/}" >&2
    changed=1
  fi
done

if [[ "$changed" -ne 0 ]]; then
  cat >&2 <<'MSG'

Meerkat dogma skill docs were refreshed from docs/architecture.
Stage the updated .claude/skills/meerkat-dogma-inquisition/references files and retry.
MSG
  exit 1
fi

printf 'Meerkat dogma skill docs are in sync.\n'
