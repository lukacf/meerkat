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
printf '## [Unreleased]\n' > "$TEST_ROOT/CHANGELOG.md"
printf 'meerkat = "1.2.3"\n' > "$TEST_ROOT/README.md"
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
  "$TEST_ROOT/Cargo.toml" "$TEST_ROOT/Cargo.lock" "$TEST_ROOT/README.md"
rm -f "$TEST_ROOT"/*.bak
printf 'module after\n' > "$TEST_ROOT/MODULE.bazel.lock"
printf '## [1.2.4]\n\n- Projection fixture.\n' > "$TEST_ROOT/CHANGELOG.md"
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
    "$REPO_ROOT/scripts/pre-push-clippy.sh"
)
if [[ "$(cat "$CALL_LOG")" != "verify-version-parity" ]]; then
  echo "release projection clippy seam did not run only version parity" >&2
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
    "$REPO_ROOT/scripts/pre-push-clippy.sh"
)
if [[ "$(cat "$CALL_LOG")" != "agent-gate --committed --clippy-only" ]]; then
  echo "semantic change bypassed the ordinary clippy gate" >&2
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
        "$REPO_ROOT/scripts/pre-push-clippy.sh" --dry-run "$@"
    elif [[ "$mode" == env-override ]]; then
      CARGO_AGENT_BASE="$range_base" ./scripts/cargo-agent-gate --committed --clippy-only --dry-run "$@"
    elif [[ "$mode" == diff-error ]]; then
      PATH="$HARNESS_ROOT:$PATH" MEERKAT_GATE_REAL_GIT="$REAL_GIT" ./scripts/cargo-agent-gate --committed --clippy-only --dry-run "$@"
    else
      ./scripts/cargo-agent-gate --committed --clippy-only --dry-run "$@"
    fi
  )" || status=$?
  local passed=0
  case "$expected" in
    runtime-only)
      if [[ "$status" -eq 0 ]] && printf '%s' "$output" | grep -Fq -- 'clippy -p meerkat-runtime' \
        && ! printf '%s' "$output" | grep -Eq 'test-sdk-web|wasm-check|fast --no-run|nextest run'; then
        passed=1
      fi
      ;;
    full-history)
      if [[ "$status" -eq 0 ]] && printf '%s' "$output" | grep -Fq 'DRY-RUN make test-sdk-web' \
        && printf '%s' "$output" | grep -Fq -- 'clippy -p meerkat-runtime'; then
        passed=1
      fi
      ;;
    whole-tree)
      if [[ "$status" -eq 0 ]] && printf '%s' "$output" | grep -Fq 'DRY-RUN make test-sdk-web' \
        && printf '%s' "$output" | grep -Fq -- 'clippy --workspace'; then
        passed=1
      fi
      ;;
    workspace-only)
      if [[ "$status" -eq 0 ]] && printf '%s' "$output" | grep -Fq -- 'clippy --workspace' \
        && ! printf '%s' "$output" | grep -Fq 'test-sdk-web'; then
        passed=1
      fi
      ;;
    reject)
      if [[ "$status" -ne 0 ]] && ! printf '%s' "$output" | grep -Fq 'DRY-RUN'; then
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
assert_range_gate exact-push-parent runtime-only "$range_parent" "$range_head" hook
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
