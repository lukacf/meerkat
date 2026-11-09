#!/usr/bin/env bash
set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-release-projection-push.XXXXXX")"
HARNESS_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-release-projection-push-harness.XXXXXX")"
trap 'rm -rf "$TEST_ROOT" "$HARNESS_ROOT"' EXIT

git -C "$TEST_ROOT" init -q
git -C "$TEST_ROOT" config user.name Meerkat
git -C "$TEST_ROOT" config user.email meerkat@example.invalid

cat > "$TEST_ROOT/Cargo.toml" <<'EOF'
[workspace]

[workspace.package]
version = "1.2.3"
EOF
cat > "$TEST_ROOT/Cargo.lock" <<'EOF'
[[package]]
name = "fixture"
version = "1.2.3"
EOF
printf 'module before\n' > "$TEST_ROOT/MODULE.bazel.lock"
# The projection is the real stampers' output, as the classifier requires.
mkdir -p "$TEST_ROOT/scripts" "$TEST_ROOT/.claude/skills/meerkat-platform"
cp "$REPO_ROOT/scripts/stamp-docs-contract-version.sh" "$REPO_ROOT/scripts/stamp-changelog-release.py" "$TEST_ROOT/scripts/"
cat > "$TEST_ROOT/CHANGELOG.md" <<'EOF'
## [Unreleased]

- Projection fixture.

## [1.2.3] - 2026-08-01

- Previous fixture release.

[Unreleased]: https://github.com/example/meerkat/compare/v1.2.3...HEAD
[1.2.3]: https://github.com/example/meerkat/compare/v1.2.2...v1.2.3
EOF
printf 'meerkat = "1.2.3"\n' > "$TEST_ROOT/README.md"
printf 'cargo install rkat --version "=1.2.3"\n' > "$TEST_ROOT/.claude/skills/meerkat-platform/SKILL.md"
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm base
base="$(git -C "$TEST_ROOT" rev-parse HEAD)"

source_fingerprint() {
  local revision="$1"
  git -C "$TEST_ROOT" ls-tree -rz --full-tree "$revision" |
    while IFS= read -r -d '' record; do
      path="${record#*$'\t'}"
      case "$path" in
        Cargo.lock | MODULE.bazel.lock) continue ;;
      esac
      printf '%s\0' "$record"
    done |
    git -C "$TEST_ROOT" hash-object --stdin
}

base_fingerprint="$(source_fingerprint "$base")"
cache_root="$TEST_ROOT/.git/meerkat-hook-cache/deterministic"
mkdir -p "$cache_root"
printf 'source_fingerprint=%s\n' "$base_fingerprint" \
  > "$cache_root/v10-cargo-source-${base_fingerprint}.ok"

sed -i.bak 's/1\.2\.3/1.2.4/g' \
  "$TEST_ROOT/Cargo.toml" "$TEST_ROOT/Cargo.lock"
rm -f "$TEST_ROOT"/*.bak
printf 'module after\n' > "$TEST_ROOT/MODULE.bazel.lock"
bash "$TEST_ROOT/scripts/stamp-docs-contract-version.sh" 1.2.4 >/dev/null
MEERKAT_RELEASE_DATE=2026-08-28 python3 "$TEST_ROOT/scripts/stamp-changelog-release.py" "$TEST_ROOT/CHANGELOG.md" 1.2.4 >/dev/null
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm projection
head="$(git -C "$TEST_ROOT" rev-parse HEAD)"
head_fingerprint="$(source_fingerprint "$head")"

CALL_LOG="$HARNESS_ROOT/calls"
FAKE_CARGO="$HARNESS_ROOT/cargo"
FAKE_MAKE="$HARNESS_ROOT/make"
FAKE_MACHINE_CLASSIFIER="$HARNESS_ROOT/machine-classifier"

cat > "$FAKE_CARGO" <<'EOF'
#!/usr/bin/env bash
printf 'cargo %s\n' "$*" >> "$MEERKAT_RELEASE_PROJECTION_CALL_LOG"
exit 99
EOF
cat > "$FAKE_MAKE" <<'EOF'
#!/usr/bin/env bash
printf 'make %s\n' "$*" >> "$MEERKAT_RELEASE_PROJECTION_CALL_LOG"
EOF
cat > "$FAKE_MACHINE_CLASSIFIER" <<'EOF'
#!/usr/bin/env bash
printf 'machine-classifier %s\n' "$*" >> "$MEERKAT_RELEASE_PROJECTION_CALL_LOG"
exit 0
EOF
chmod +x "$FAKE_CARGO" "$FAKE_MAKE" "$FAKE_MACHINE_CLASSIFIER"

: > "$CALL_LOG"
(
  cd "$TEST_ROOT"
  ROOT="$TEST_ROOT" \
    CARGO="$FAKE_CARGO" \
    RELEASE_PROJECTION_ONLY="$REPO_ROOT/scripts/release-projection-only.mjs" \
    MEERKAT_RELEASE_PROJECTION_CALL_LOG="$CALL_LOG" \
    PRE_COMMIT_FROM_REF="$base" \
    PRE_COMMIT_TO_REF="$head" \
    "$REPO_ROOT/scripts/pre-push-unit.sh"
)
if [[ -s "$CALL_LOG" ]]; then
  echo "release projection source-evidence reuse invoked Cargo" >&2
  cat "$CALL_LOG" >&2
  exit 1
fi
derived_stamp="$cache_root/v10-cargo-source-${head_fingerprint}.ok"
if [[ ! -f "$derived_stamp" ]] || \
   ! grep -Fxq "reuse_parent_fingerprint=${base_fingerprint}" "$derived_stamp"; then
  echo "release projection did not record derived parent evidence" >&2
  exit 1
fi

: > "$CALL_LOG"
(
  cd "$TEST_ROOT"
  ROOT="$TEST_ROOT" \
    CARGO="$FAKE_CARGO" \
    MAKE_BIN="$FAKE_MAKE" \
    MACHINE_AUTHORITY_CHANGED="$FAKE_MACHINE_CLASSIFIER" \
    RELEASE_PROJECTION_ONLY="$REPO_ROOT/scripts/release-projection-only.mjs" \
    MEERKAT_RELEASE_PROJECTION_CALL_LOG="$CALL_LOG" \
    PRE_COMMIT_FROM_REF="$base" \
    PRE_COMMIT_TO_REF="$head" \
    "$REPO_ROOT/scripts/pre-push-machines.sh"
)
if [[ -s "$CALL_LOG" ]]; then
  echo "release projection unexpectedly entered the machine authority lane" >&2
  cat "$CALL_LOG" >&2
  exit 1
fi

# CI planning reads the actual commits: the planner's own path collection
# (run in the fixture repository) must name every path a change touches, both
# sides of a rename and type-only changes included, and the real workspace
# plan for those paths must give the touched crates Clippy lanes. No hook runs
# Clippy, so a path the planner drops is unlinted code. The plan
# must also hold the touched crates in its test scope and cover the hook's
# cargo-test binaries for that scope: the independent checker
# (scripts/ci-hook-coverage.mjs) re-reads the emitted plan and must report no
# gap. The planner resolves the repository from its working directory, so it
# runs from this checkout against the fixture's commits.
mkdir -p "$TEST_ROOT/crates/meerkat-runtime/src"
printf '[package]\nname = "meerkat-runtime"\nversion = "1.2.3"\n' > "$TEST_ROOT/crates/meerkat-runtime/Cargo.toml"
printf 'pub fn retained() {}\n' > "$TEST_ROOT/crates/meerkat-runtime/src/lib.rs"
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm ci-fixture
range_head="$(git -C "$TEST_ROOT" rev-parse HEAD)"
ci_plan_failures=0
assert_ci_plan() {
  local label="$1" from="$2" to="$3" expected_paths="$4" expected_packages="$5"
  local collected
  collected="$(cd "$TEST_ROOT" && node "$REPO_ROOT/scripts/ci-cargo-lanes.mjs" --changed-paths-only --base "$from" --head "$to")" || {
    printf 'FAIL ci plan: %s: path collection failed\n' "$label" >&2
    ci_plan_failures=$((ci_plan_failures + 1))
    return
  }
  if ! python3 - "$collected" "$expected_paths" "$expected_packages" "$REPO_ROOT" <<'PY'
import json
import subprocess
import sys

collected, expected_paths, expected_packages, repo_root = sys.argv[1:]
paths = json.loads(collected)["changed_paths"]
missing = [path for path in expected_paths.split(",") if path not in paths]
if missing:
    sys.exit(f"collection {paths} misses {missing}")
planned = subprocess.run(
    ["node", "scripts/ci-cargo-lanes.mjs", "--format", "json", "--", *paths],
    cwd=repo_root, check=True, capture_output=True, text=True,
).stdout
plan = json.loads(planned)
linted = {name for shard in plan["shards"] for name in shard["packages"]}
unlinted = [name for name in expected_packages.split(",") if name not in linted and plan["mode"] != "workspace"]
if not plan["rust_changed"] or unlinted:
    sys.exit(f"plan for {paths}: rust_changed={plan['rust_changed']}, Clippy lanes {sorted(linted)} miss {unlinted}")
untested = [name for name in expected_packages.split(",") if name not in plan.get("test_scope", [])]
if untested:
    sys.exit(f"plan for {paths}: test scope misses {untested}")
checked = subprocess.run(
    ["node", "scripts/ci-hook-coverage.mjs", "--plan", "-"],
    cwd=repo_root, input=planned, capture_output=True, text=True,
)
if checked.returncode != 0:
    sys.exit(f"plan for {paths}: hook coverage check failed: {checked.stdout}{checked.stderr}")
report = json.loads(checked.stdout)
if report["hook_binaries"] == 0 or report["missing"] or report["errors"]:
    sys.exit(f"plan for {paths}: hook coverage {report}")
PY
  then
    printf 'FAIL ci plan: %s\n' "$label" >&2
    ci_plan_failures=$((ci_plan_failures + 1))
  else
    printf 'PASS ci plan: %s\n' "$label"
  fi
}
git -C "$TEST_ROOT" checkout -q --detach "$range_head"
git -C "$TEST_ROOT" rm -q crates/meerkat-runtime/src/lib.rs
git -C "$TEST_ROOT" commit -qm ci-deleted-runtime-source
assert_ci_plan deleted-runtime-source "$range_head" "$(git -C "$TEST_ROOT" rev-parse HEAD)" \
  crates/meerkat-runtime/src/lib.rs meerkat-runtime
git -C "$TEST_ROOT" checkout -q --detach "$range_head"
git -C "$TEST_ROOT" mv crates/meerkat-runtime/src/lib.rs crates/meerkat-runtime/src/notes.txt
git -C "$TEST_ROOT" commit -qm ci-rust-renamed-to-text
assert_ci_plan rust-renamed-to-text "$range_head" "$(git -C "$TEST_ROOT" rev-parse HEAD)" \
  crates/meerkat-runtime/src/lib.rs,crates/meerkat-runtime/src/notes.txt meerkat-runtime
git -C "$TEST_ROOT" checkout -q --detach "$range_head"
mkdir -p "$TEST_ROOT/docs"
git -C "$TEST_ROOT" mv crates/meerkat-runtime/src/lib.rs docs/retired-test.md
git -C "$TEST_ROOT" commit -qm ci-rust-renamed-into-docs
assert_ci_plan rust-renamed-into-docs "$range_head" "$(git -C "$TEST_ROOT" rev-parse HEAD)" \
  crates/meerkat-runtime/src/lib.rs,docs/retired-test.md meerkat-runtime
git -C "$TEST_ROOT" checkout -q --detach "$range_head"
mkdir -p "$TEST_ROOT/crates/meerkat-core/src"
git -C "$TEST_ROOT" mv crates/meerkat-runtime/src/lib.rs crates/meerkat-core/src/moved_from_runtime.rs
git -C "$TEST_ROOT" commit -qm ci-test-moved-between-crates
assert_ci_plan test-moved-between-crates "$range_head" "$(git -C "$TEST_ROOT" rev-parse HEAD)" \
  crates/meerkat-runtime/src/lib.rs,crates/meerkat-core/src/moved_from_runtime.rs meerkat-runtime,meerkat-core
git -C "$TEST_ROOT" checkout -q --detach "$range_head"
rm "$TEST_ROOT/crates/meerkat-runtime/src/lib.rs"
ln -s ../../../README.md "$TEST_ROOT/crates/meerkat-runtime/src/lib.rs"
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm ci-rust-file-became-symlink
assert_ci_plan rust-file-type-change "$range_head" "$(git -C "$TEST_ROOT" rev-parse HEAD)" \
  crates/meerkat-runtime/src/lib.rs meerkat-runtime
git -C "$TEST_ROOT" checkout -q --detach "$range_head"
if [[ "$ci_plan_failures" -ne 0 ]]; then
  echo "$ci_plan_failures CI planning contract failures" >&2
  exit 1
fi
echo "release projection pre-push seams hold"
