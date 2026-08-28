#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-pre-push-harness-only.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

git -C "$TEST_ROOT" init -q
git -C "$TEST_ROOT" config user.name Meerkat
git -C "$TEST_ROOT" config user.email meerkat@example.invalid
mkdir -p "$TEST_ROOT/scripts"
printf '#!/usr/bin/env bash\n' > "$TEST_ROOT/scripts/pre-push-unit.sh"
printf '#!/usr/bin/env bash\n' > "$TEST_ROOT/scripts/release-doctor"
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm base
base="$(git -C "$TEST_ROOT" rev-parse HEAD)"

printf '# contract update\n' >> "$TEST_ROOT/scripts/pre-push-unit.sh"
printf '# release SLO contract update\n' >> "$TEST_ROOT/scripts/release-doctor"
printf '#!/usr/bin/env bash\n' > "$TEST_ROOT/scripts/test-release-projection-new.sh"
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm harness-only
harness_head="$(git -C "$TEST_ROOT" rev-parse HEAD)"
(
  cd "$TEST_ROOT"
  "$REPO_ROOT/scripts/pre-push-harness-only.sh" \
    --base "$base" --head "$harness_head"
)

assert_rejected() {
  local label="$1"
  local candidate="$2"
  set +e
  (
    cd "$TEST_ROOT"
    "$REPO_ROOT/scripts/pre-push-harness-only.sh" \
      --base "$base" --head "$candidate" >/dev/null 2>&1
  )
  local command_status=$?
  set -e
  if [[ "$command_status" -ne 1 ]]; then
    echo "${label} returned ${command_status}; expected rejection status 1" >&2
    exit 1
  fi
}

git -C "$TEST_ROOT" checkout -q -B rust-change "$base"
printf 'pub fn changed() {}\n' > "$TEST_ROOT/source.rs"
git -C "$TEST_ROOT" add source.rs
git -C "$TEST_ROOT" commit -qm rust-change
assert_rejected rust-change "$(git -C "$TEST_ROOT" rev-parse HEAD)"

git -C "$TEST_ROOT" checkout -q -B config-change "$base"
printf 'fail_fast: false\n' > "$TEST_ROOT/.pre-commit-config.yaml"
git -C "$TEST_ROOT" add .pre-commit-config.yaml
git -C "$TEST_ROOT" commit -qm config-change
assert_rejected config-change "$(git -C "$TEST_ROOT" rev-parse HEAD)"

git -C "$TEST_ROOT" checkout -q -B deletion "$base"
git -C "$TEST_ROOT" rm -q scripts/pre-push-unit.sh
git -C "$TEST_ROOT" commit -qm deletion
assert_rejected deletion "$(git -C "$TEST_ROOT" rev-parse HEAD)"

if grep -Eq 'XTASK_TARGET_DIR|export CARGO_TARGET_DIR' \
    "$REPO_ROOT/scripts/pre-push-audit-generated-headers.sh"; then
  echo "generated-header hook still forces a second xtask target" >&2
  exit 1
fi
if ! grep -Fq '"$CARGO" xtask audit-generated-headers' \
    "$REPO_ROOT/scripts/pre-push-audit-generated-headers.sh"; then
  echo "generated-header hook does not use the shared repo-cargo xtask lane" >&2
  exit 1
fi

echo "pre-push harness-only classifier contracts hold"
