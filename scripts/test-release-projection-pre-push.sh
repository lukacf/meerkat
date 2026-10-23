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
FAKE_VERIFY="$HARNESS_ROOT/verify"
FAKE_AGENT_GATE="$HARNESS_ROOT/agent-gate"
FAKE_MAKE="$HARNESS_ROOT/make"
FAKE_MACHINE_CLASSIFIER="$HARNESS_ROOT/machine-classifier"

cat > "$FAKE_CARGO" <<'EOF'
#!/usr/bin/env bash
printf 'cargo %s\n' "$*" >> "$MEERKAT_RELEASE_PROJECTION_CALL_LOG"
exit 99
EOF
cat > "$FAKE_VERIFY" <<'EOF'
#!/usr/bin/env bash
printf 'verify-version-parity\n' >> "$MEERKAT_RELEASE_PROJECTION_CALL_LOG"
EOF
cat > "$FAKE_AGENT_GATE" <<'EOF'
#!/usr/bin/env bash
printf 'agent-gate %s\n' "$*" >> "$MEERKAT_RELEASE_PROJECTION_CALL_LOG"
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
chmod +x "$FAKE_CARGO" "$FAKE_VERIFY" "$FAKE_AGENT_GATE" \
  "$FAKE_MAKE" "$FAKE_MACHINE_CLASSIFIER"

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
    RELEASE_PROJECTION_ONLY="$REPO_ROOT/scripts/release-projection-only.mjs" \
    VERIFY_VERSION_PARITY="$FAKE_VERIFY" \
    AGENT_GATE="$FAKE_AGENT_GATE" \
    MEERKAT_RELEASE_PROJECTION_CALL_LOG="$CALL_LOG" \
    PRE_COMMIT_FROM_REF="$base" \
    PRE_COMMIT_TO_REF="$head" \
    "$REPO_ROOT/scripts/pre-push-ratchets.sh"
)
if [[ "$(cat "$CALL_LOG")" != "verify-version-parity" ]]; then
  echo "release projection ratchets seam did not run only version parity" >&2
  cat "$CALL_LOG" >&2
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

git -C "$TEST_ROOT" checkout -q -B semantic "$base"
sed -i.bak 's/1\.2\.3/1.2.4/g' "$TEST_ROOT/Cargo.toml"
rm -f "$TEST_ROOT/Cargo.toml.bak"
printf '## [1.2.4]\n' > "$TEST_ROOT/CHANGELOG.md"
printf 'pub fn semantic_change() {}\n' > "$TEST_ROOT/source.rs"
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm semantic
semantic_head="$(git -C "$TEST_ROOT" rev-parse HEAD)"
: > "$CALL_LOG"
(
  cd "$TEST_ROOT"
  ROOT="$TEST_ROOT" \
    RELEASE_PROJECTION_ONLY="$REPO_ROOT/scripts/release-projection-only.mjs" \
    VERIFY_VERSION_PARITY="$FAKE_VERIFY" \
    AGENT_GATE="$FAKE_AGENT_GATE" \
    MEERKAT_RELEASE_PROJECTION_CALL_LOG="$CALL_LOG" \
    PRE_COMMIT_FROM_REF="$base" \
    PRE_COMMIT_TO_REF="$semantic_head" \
    "$REPO_ROOT/scripts/pre-push-ratchets.sh"
)
if [[ "$(cat "$CALL_LOG")" != "agent-gate --committed --ratchets-only --machine-drift-by-hook" ]]; then
  echo "semantic change bypassed the ordinary ratchets gate" >&2
  cat "$CALL_LOG" >&2
  exit 1
fi


# The real committed Cargo selector must use the dispatcher's exact push range,
# rather than replaying previously accepted Web SDK edits since origin/main.
mkdir -p "$TEST_ROOT/scripts" "$TEST_ROOT/crates/meerkat-runtime/src" "$TEST_ROOT/sdks/web/src"
cp "$REPO_ROOT/scripts/cargo-agent-gate" "$TEST_ROOT/scripts/cargo-agent-gate"
printf '#!/usr/bin/env bash\nexit 1\n' > "$TEST_ROOT/scripts/machine-authority-changed"
printf '#!/usr/bin/env bash\nexit 1\n' > "$TEST_ROOT/scripts/generated-contract-ratchet-changed"
printf '#!/usr/bin/env bash\nexit 0\n' > "$TEST_ROOT/scripts/rust-embedded-inputs.mjs"
printf '#!/usr/bin/env bash\nexit 0\n' > "$TEST_ROOT/scripts/cargo-exact-tests.mjs"
printf '[package]\nname = "meerkat-runtime"\nversion = "1.2.3"\n' > "$TEST_ROOT/crates/meerkat-runtime/Cargo.toml"
printf 'pub fn retained() {}\n' > "$TEST_ROOT/crates/meerkat-runtime/src/lib.rs"
printf 'export const old = 1;\n' > "$TEST_ROOT/sdks/web/src/runtime.ts"
chmod +x "$TEST_ROOT/scripts/"*
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm gate-fixture
range_base="$(git -C "$TEST_ROOT" rev-parse HEAD)"
git -C "$TEST_ROOT" update-ref refs/remotes/origin/main "$range_base"
printf 'export const already_accepted = 2;\n' > "$TEST_ROOT/sdks/web/src/runtime.ts"
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm previously-accepted-web
range_parent="$(git -C "$TEST_ROOT" rev-parse HEAD)"
printf 'pub fn successor_runtime_change() {}\n' > "$TEST_ROOT/crates/meerkat-runtime/src/lib.rs"
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm runtime-only-successor
range_head="$(git -C "$TEST_ROOT" rev-parse HEAD)"
empty_tree="$(git -C "$TEST_ROOT" hash-object -t tree /dev/null)"
FAKE_METADATA="$HARNESS_ROOT/metadata-only-cargo"
cat > "$FAKE_METADATA" <<'EOF_METADATA'
#!/usr/bin/env bash
if [[ "$1" == metadata ]]; then
  printf '{"packages":[{"name":"meerkat-runtime","manifest_path":"%s/crates/meerkat-runtime/Cargo.toml"}]}\n' "$MEERKAT_GATE_FIXTURE_ROOT"
  exit 0
fi
echo "unexpected Cargo execution: $*" >&2
exit 99
EOF_METADATA
chmod +x "$FAKE_METADATA"
range_failures=0
assert_range_gate() {
  local label="$1" expected="$2" from="$3" to="$4" mode="$5"
  shift 5
  local output status=0
  output="$(
    cd "$TEST_ROOT"
    unset CARGO_AGENT_BASE PRE_COMMIT_FROM_REF PRE_COMMIT_TO_REF
    export CARGO="$FAKE_METADATA"
    export MEERKAT_GATE_FIXTURE_ROOT="$(git rev-parse --show-toplevel)"
    export PRE_COMMIT_FROM_REF="$from" PRE_COMMIT_TO_REF="$to"
    if [[ "$mode" == hook ]]; then
      ROOT="$TEST_ROOT" \
        RELEASE_PROJECTION_ONLY="$REPO_ROOT/scripts/release-projection-only.mjs" \
        AGENT_GATE="$TEST_ROOT/scripts/cargo-agent-gate" \
        "$REPO_ROOT/scripts/pre-push-ratchets.sh" --dry-run "$@"
    elif [[ "$mode" == env-override ]]; then
      CARGO_AGENT_BASE="$range_base" ./scripts/cargo-agent-gate --committed --clippy-only --dry-run "$@"
    elif [[ "$mode" == diff-error ]]; then
      PATH="$HARNESS_ROOT:$PATH" MEERKAT_GATE_REAL_GIT="$REAL_GIT" ./scripts/cargo-agent-gate --committed --clippy-only --dry-run "$@"
    else
      ./scripts/cargo-agent-gate --committed --clippy-only --dry-run "$@"
    fi
  )" || status=$?
  local passed=0
  # Match captured output directly: grep -q can close its pipe before printf
  # finishes and make a successful match fail with SIGPIPE under pipefail.
  case "$expected" in
    ratchets-quiet)
      # The hook lints nothing: a runtime-only range runs no ratchet, no
      # Clippy and no tests.
      if [[ "$status" -eq 0 && "$output" == *'ratchets only'* \
        && "$output" != *clippy* && "$output" != *test-sdk-web* \
        && "$output" != *wasm-check* && "$output" != *'fast --no-run'* \
        && "$output" != *'nextest run'* ]]; then
        passed=1
      fi
      ;;
    ratchets-web)
      if [[ "$status" -eq 0 && "$output" == *'DRY-RUN make test-sdk-web'* \
        && "$output" != *clippy* && "$output" != *'fast --no-run'* \
        && "$output" != *'nextest run'* ]]; then
        passed=1
      fi
      ;;
    runtime-only)
      if [[ "$status" -eq 0 && "$output" == *'clippy -p meerkat-runtime'* \
        && "$output" != *test-sdk-web* && "$output" != *wasm-check* \
        && "$output" != *'fast --no-run'* && "$output" != *'nextest run'* ]]; then
        passed=1
      fi
      ;;
    full-history)
      if [[ "$status" -eq 0 && "$output" == *'DRY-RUN make test-sdk-web'* \
        && "$output" == *'clippy -p meerkat-runtime'* ]]; then
        passed=1
      fi
      ;;
    whole-tree)
      if [[ "$status" -eq 0 && "$output" == *'DRY-RUN make test-sdk-web'* \
        && "$output" == *'clippy --workspace'* ]]; then
        passed=1
      fi
      ;;
    workspace-only)
      if [[ "$status" -eq 0 && "$output" == *'clippy --workspace'* \
        && "$output" != *test-sdk-web* ]]; then
        passed=1
      fi
      ;;
    reject)
      if [[ "$status" -ne 0 && "$output" != *DRY-RUN* ]]; then
        passed=1
      fi
      ;;
  esac
  if [[ "$passed" -eq 1 ]]; then
    printf 'PASS committed range: %s\n' "$label"
  else
    printf 'FAIL committed range: %s (exit=%s; expected=%s)\n%s\n' "$label" "$status" "$expected" "$output" >&2
    range_failures=$((range_failures + 1))
  fi
}
assert_range_gate exact-push-parent ratchets-quiet "$range_parent" "$range_head" hook
assert_range_gate exact-push-range-with-web ratchets-web "$range_base" "$range_head" hook
assert_range_gate exact-push-parent-gate runtime-only "$range_parent" "$range_head" direct
assert_range_gate zero-oid-new-branch whole-tree 0000000000000000000000000000000000000000 "$range_head" direct
assert_range_gate empty-tree-new-branch whole-tree "$empty_tree" "$range_head" direct
assert_range_gate explicit-base-override full-history "$range_parent" "$range_head" direct --base "$range_base"
assert_range_gate environment-base-override full-history "$range_parent" "$range_head" env-override
assert_range_gate mismatched-push-head reject "$range_parent" "$range_parent" direct
assert_range_gate missing-push-base reject '' "$range_head" direct
assert_range_gate missing-push-head reject "$range_parent" '' direct
assert_range_gate unusable-push-base reject deadbeef "$range_head" direct
assert_range_gate ordinary-committed-fallback full-history '' '' direct
assert_range_gate short-zero-invalid-base reject 0 "$range_head" direct
# Removal and cross-classification rename must keep the removed owner's gate.
git -C "$TEST_ROOT" rm -q crates/meerkat-runtime/src/lib.rs
git -C "$TEST_ROOT" commit -qm deleted-runtime-source
deleted_head="$(git -C "$TEST_ROOT" rev-parse HEAD)"
assert_range_gate deleted-runtime-source runtime-only "$range_head" "$deleted_head" direct
git -C "$TEST_ROOT" checkout -q --detach "$range_head"
git -C "$TEST_ROOT" rm -q Cargo.lock
git -C "$TEST_ROOT" commit -qm deleted-global-input
deleted_head="$(git -C "$TEST_ROOT" rev-parse HEAD)"
assert_range_gate deleted-global-input workspace-only "$range_head" "$deleted_head" direct
git -C "$TEST_ROOT" checkout -q --detach "$range_head"
git -C "$TEST_ROOT" rm -qr crates/meerkat-runtime
git -C "$TEST_ROOT" commit -qm deleted-crate-manifest
deleted_head="$(git -C "$TEST_ROOT" rev-parse HEAD)"
assert_range_gate deleted-crate-manifest workspace-only "$range_head" "$deleted_head" direct
git -C "$TEST_ROOT" checkout -q --detach "$range_head"
git -C "$TEST_ROOT" mv crates/meerkat-runtime/src/lib.rs crates/meerkat-runtime/src/notes.txt
git -C "$TEST_ROOT" commit -qm rust-renamed-to-text
renamed_head="$(git -C "$TEST_ROOT" rev-parse HEAD)"
assert_range_gate rust-renamed-to-text runtime-only "$range_head" "$renamed_head" direct
git -C "$TEST_ROOT" checkout -q --detach "$range_head"
rm "$TEST_ROOT/crates/meerkat-runtime/src/lib.rs"
ln -s ../../../README.md "$TEST_ROOT/crates/meerkat-runtime/src/lib.rs"
git -C "$TEST_ROOT" add .
git -C "$TEST_ROOT" commit -qm rust-file-became-symlink
type_head="$(git -C "$TEST_ROOT" rev-parse HEAD)"
assert_range_gate rust-file-type-change runtime-only "$range_head" "$type_head" direct
git -C "$TEST_ROOT" checkout -q --detach "$range_head"

# The same actual commits feed CI planning: the planner's own path collection
# (run in the fixture repository) must name every path a change touches, both
# sides of a rename and type-only changes included, and the real workspace
# plan for those paths must give the touched crates Clippy lanes. The hook no
# longer runs Clippy, so a path the planner drops is unlinted code. The plan
# must also hold the touched crates in its test scope and cover the hook's
# cargo-test binaries for that scope: the independent checker
# (scripts/ci-hook-coverage.mjs) re-reads the emitted plan and must report no
# gap. The planner resolves the repository from its working directory, so it
# runs from this checkout against the fixture's commits.
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
REAL_GIT="$(command -v git)"
cat > "$HARNESS_ROOT/git" <<'EOF_GIT'
#!/usr/bin/env bash
if [[ "$1" == diff ]]; then
  echo "forced changed-path diff failure" >&2
  exit 7
fi
exec "$MEERKAT_GATE_REAL_GIT" "$@"
EOF_GIT
chmod +x "$HARNESS_ROOT/git"
assert_range_gate changed-path-diff-error reject "$range_parent" "$range_head" diff-error
if [[ "$range_failures" -ne 0 ]]; then
  echo "$range_failures committed-range contract failures" >&2
  exit 1
fi

echo "release projection pre-push seams hold"
