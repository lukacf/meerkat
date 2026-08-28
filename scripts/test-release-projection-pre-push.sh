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

echo "release projection pre-push seams hold"
