#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-release-projection.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

git -C "$TEST_ROOT" init -q
git -C "$TEST_ROOT" config user.name Meerkat
git -C "$TEST_ROOT" config user.email meerkat@example.invalid

mkdir -p \
  "$TEST_ROOT/crate" \
  "$TEST_ROOT/docs/api" \
  "$TEST_ROOT/meerkat-contracts/src" \
  "$TEST_ROOT/sdks/web/src"

cat > "$TEST_ROOT/Cargo.toml" <<'EOF'
[workspace]
members = ["crate"]

[workspace.package]
version = "1.2.3"

[workspace.dependencies]
fixture = { version = "1.2.3", path = "crate" }
EOF
cat > "$TEST_ROOT/Cargo.lock" <<'EOF'
[[package]]
name = "fixture"
version = "1.2.3"
EOF
printf 'root digest before\n' > "$TEST_ROOT/MODULE.bazel.lock"
printf '## [Unreleased]\n\n' > "$TEST_ROOT/CHANGELOG.md"
printf 'fixture = "1.2.3"\n' > "$TEST_ROOT/README.md"
printf 'version = "1.2.3"\n' > "$TEST_ROOT/crate/BUILD.bazel"
printf '{"contract_version":{"major":1,"minor":2,"patch":3}}\n' \
  > "$TEST_ROOT/docs/api/contract.mdx"
cat > "$TEST_ROOT/meerkat-contracts/src/version.rs" <<'EOF'
impl ContractVersion {
    pub const CURRENT: Self = Self {
        major: 1,
        minor: 2,
        patch: 3,
    };
    pub const PRERELEASE: Option<&'static str> = None;
}
EOF
printf 'export const VERSION = "1.2.3";\n' > "$TEST_ROOT/sdks/web/src/runtime.ts"
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm base
base="$(git -C "$TEST_ROOT" rev-parse HEAD)"

sed -i.bak 's/1\.2\.3/1.2.4/g' \
  "$TEST_ROOT/Cargo.toml" \
  "$TEST_ROOT/Cargo.lock" \
  "$TEST_ROOT/README.md" \
  "$TEST_ROOT/crate/BUILD.bazel" \
  "$TEST_ROOT/sdks/web/src/runtime.ts"
rm -f "$TEST_ROOT"/*.bak "$TEST_ROOT/crate"/*.bak "$TEST_ROOT/sdks/web/src"/*.bak
sed -i.bak \
  's/"major":1,"minor":2,"patch":3/"major":1,"minor":2,"patch":4/' \
  "$TEST_ROOT/docs/api/contract.mdx"
rm -f "$TEST_ROOT/docs/api/contract.mdx.bak"
sed -i.bak 's/patch: 3/patch: 4/' "$TEST_ROOT/meerkat-contracts/src/version.rs"
rm -f "$TEST_ROOT/meerkat-contracts/src/version.rs.bak"
printf 'root digest after\n' > "$TEST_ROOT/MODULE.bazel.lock"
cat > "$TEST_ROOT/CHANGELOG.md" <<'EOF'
## [1.2.4] - 2026-08-28

- Pipeline-only fixture.
EOF
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm projection
projection="$(git -C "$TEST_ROOT" rev-parse HEAD)"

(
  cd "$TEST_ROOT"
  "$REPO_ROOT/scripts/release-projection-only.mjs" \
    --base "$base" --head "$projection" --verbose
)

assert_rejected() {
  local label="$1"
  local candidate="$2"
  set +e
  (
    cd "$TEST_ROOT"
    "$REPO_ROOT/scripts/release-projection-only.mjs" \
      --base "$base" --head "$candidate" >/dev/null 2>&1
  )
  local status=$?
  set -e
  if [[ "$status" -ne 1 ]]; then
    echo "${label} returned ${status}; expected classifier rejection status 1" >&2
    exit 1
  fi
}

git -C "$TEST_ROOT" checkout -q -b semantic-source "$base"
printf 'pub fn changed_behavior() {}\n' > "$TEST_ROOT/crate/src.rs"
sed -i.bak 's/1\.2\.3/1.2.4/g' "$TEST_ROOT/Cargo.toml"
rm -f "$TEST_ROOT/Cargo.toml.bak"
printf '## [1.2.4]\n' > "$TEST_ROOT/CHANGELOG.md"
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm semantic-source
assert_rejected semantic-source "$(git -C "$TEST_ROOT" rev-parse HEAD)"

git -C "$TEST_ROOT" checkout -q -B semantic-manifest "$base"
sed -i.bak 's/1\.2\.3/1.2.4/g' "$TEST_ROOT/Cargo.toml"
rm -f "$TEST_ROOT/Cargo.toml.bak"
printf '\nserde = "1"\n' >> "$TEST_ROOT/Cargo.toml"
printf '## [1.2.4]\n' > "$TEST_ROOT/CHANGELOG.md"
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm semantic-manifest
assert_rejected semantic-manifest "$(git -C "$TEST_ROOT" rev-parse HEAD)"

git -C "$TEST_ROOT" checkout -q -B equal-version-external "$base"
printf '\nexternal = "1.2.3"\n' >> "$TEST_ROOT/Cargo.toml"
git -C "$TEST_ROOT" add Cargo.toml
git -C "$TEST_ROOT" commit -qm external-base
external_base="$(git -C "$TEST_ROOT" rev-parse HEAD)"
sed -i.bak 's/1\.2\.3/1.2.4/g' "$TEST_ROOT/Cargo.toml"
rm -f "$TEST_ROOT/Cargo.toml.bak"
printf '## [1.2.4]\n' > "$TEST_ROOT/CHANGELOG.md"
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm equal-version-external
set +e
(
  cd "$TEST_ROOT"
  "$REPO_ROOT/scripts/release-projection-only.mjs" \
    --base "$external_base" --head "$(git rev-parse HEAD)" >/dev/null 2>&1
)
external_status=$?
set -e
if [[ "$external_status" -ne 1 ]]; then
  echo "same-version external dependency drift escaped the classifier" >&2
  exit 1
fi

git -C "$TEST_ROOT" checkout -q -B stale-generated "$projection"
printf '\n# semantic rule drift\n' >> "$TEST_ROOT/crate/BUILD.bazel"
git -C "$TEST_ROOT" add crate/BUILD.bazel
git -C "$TEST_ROOT" commit -qm stale-generated
assert_rejected stale-generated "$(git -C "$TEST_ROOT" rev-parse HEAD)"

git -C "$TEST_ROOT" checkout -q -B dependency-lock-drift "$base"
sed -i.bak 's/1\.2\.3/1.2.4/g' "$TEST_ROOT/Cargo.toml"
rm -f "$TEST_ROOT/Cargo.toml.bak"
printf '## [1.2.4]\n' > "$TEST_ROOT/CHANGELOG.md"
cat >> "$TEST_ROOT/Cargo.lock" <<'EOF'

[[package]]
name = "unrelated"
version = "9.9.9"
EOF
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm dependency-lock-drift
assert_rejected dependency-lock-drift "$(git -C "$TEST_ROOT" rev-parse HEAD)"

set +e
(
  cd "$TEST_ROOT"
  "$REPO_ROOT/scripts/release-projection-only.mjs" \
    --base missing-revision --head "$projection" >/dev/null 2>&1
)
invalid_ref_status=$?
set -e
if [[ "$invalid_ref_status" -ne 1 && "$invalid_ref_status" -ne 2 ]]; then
  echo "invalid revision returned ${invalid_ref_status}; expected fail-closed status" >&2
  exit 1
fi

echo "release projection classifier contracts hold"
