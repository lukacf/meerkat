#!/usr/bin/env bash

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHECKER="$REPO_ROOT/scripts/check-rust-release-packaging.sh"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-release-packaging-isolation.XXXXXX")"
FIXTURE="$TEST_ROOT/workspace"
PACKAGE_COUNT=3

cleanup() {
  local pending_status=$?
  chmod -R u+rwx "$TEST_ROOT" 2>/dev/null || true
  rm -rf -- "$TEST_ROOT"
  exit "$pending_status"
}
trap cleanup EXIT

fail() {
  echo "release packaging isolation contract violated: $1" >&2
  shift
  for extra in "$@"; do
    echo "  $extra" >&2
  done
  exit 1
}

value_from_env() {
  local key="$1"
  local text="$2"
  printf '%s\n' "$text" | awk -F= -v key="$key" '$1 == key { print substr($0, length(key) + 2) }'
}

mkdir -p "$FIXTURE/scripts"
cat > "$FIXTURE/Cargo.toml" <<'EOF'
[workspace]
members = []

[workspace.package]
version = "0.0.0"
EOF

cat > "$FIXTURE/scripts/release-rust-crates.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' alpha beta gamma
EOF

cat > "$FIXTURE/scripts/check-rust-release-config.sh" <<'EOF'
#!/usr/bin/env bash
exit 0
EOF

cat > "$FIXTURE/scripts/generate-patch-config.sh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' '[patch.crates-io]'
EOF

cat > "$FIXTURE/scripts/check-published-facade-link.sh" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
: "${STATE_DIR:?}"
: "${MEERKAT_PUBLISHED_FACADE_PACKAGE_TARGET:?}"
for crate in alpha beta gamma; do
  archive="$MEERKAT_PUBLISHED_FACADE_PACKAGE_TARGET/package/$crate-0.0.0.crate"
  [[ -f "$archive" ]] || {
    echo "missing staged verified archive: $archive" >&2
    exit 1
  }
done
printf '%s\n' "$MEERKAT_PUBLISHED_FACADE_PACKAGE_TARGET" > "$STATE_DIR/facade-target"
EOF

cat > "$FIXTURE/scripts/fake-cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
# `cargo metadata` feeds scripts/crate-license-files.sh. Every fixture crate
# is dual-licensed unless LICENSE_EXPR_CRATE names one whose expression is
# LICENSE_EXPR.
if [[ "${1:-}" == metadata ]]; then
  packages=""
  for name in alpha beta gamma; do
    expression="MIT OR Apache-2.0"
    [[ "${LICENSE_EXPR_CRATE:-}" == "$name" ]] && expression="${LICENSE_EXPR:?}"
    packages+="${packages:+,}{\"name\":\"$name\",\"license\":\"$expression\"}"
  done
  printf '{"packages":[%s]}\n' "$packages"
  exit 0
fi
: "${STATE_DIR:?}"
: "${EXPECTED_PACKAGES:?}"
: "${CARGO_TARGET_DIR:?}"
: "${RUST_LANE_ID:?}"

crate=""
while [[ "$#" -gt 0 ]]; do
  if [[ "$1" == -p ]]; then
    crate="$2"
    break
  fi
  shift
done
[[ -n "$crate" ]] || {
  echo "error: fake cargo did not receive -p" >&2
  exit 2
}

mkdir -p "$STATE_DIR/calls" "$STATE_DIR/call-tmp" "$CARGO_TARGET_DIR/package"
call_tmp="$(mktemp "$STATE_DIR/call-tmp/$crate.XXXXXX")"
printf '%s|%s\n' "$CARGO_TARGET_DIR" "$RUST_LANE_ID" > "$call_tmp"
mv "$call_tmp" "$STATE_DIR/calls/$crate"
printf '%s\n' "$crate" >> "$STATE_DIR/invocations"

for _ in $(seq 1 250); do
  started="$(find "$STATE_DIR/calls" -type f | wc -l | tr -d ' ')"
  [[ "$started" -ge "$EXPECTED_PACKAGES" ]] && break
  sleep 0.02
done

started="$(find "$STATE_DIR/calls" -type f | wc -l | tr -d ' ')"
if [[ "$started" -lt "$EXPECTED_PACKAGES" ]]; then
  echo "error: package jobs did not execute concurrently" >&2
  exit 90
fi

unique_targets="$(cut -d '|' -f 1 "$STATE_DIR"/calls/* | sort -u | wc -l | tr -d ' ')"
unique_lanes="$(cut -d '|' -f 2 "$STATE_DIR"/calls/* | sort -u | wc -l | tr -d ' ')"
if [[ "$unique_targets" -ne "$EXPECTED_PACKAGES" ]]; then
  echo "error: concurrent package jobs share CARGO_TARGET_DIR" >&2
  exit 91
fi
if [[ "$unique_lanes" -ne "$EXPECTED_PACKAGES" ]]; then
  echo "error: concurrent package jobs share RUST_LANE_ID" >&2
  exit 92
fi

# A real gzip tarball with the layout cargo produces, so the packaging check's
# license-file inspection reads it like a published archive.
archive_root="$call_tmp.archive/$crate-0.0.0"
mkdir -p "$archive_root"
printf 'fixture archive\n' > "$archive_root/Cargo.toml"
if [[ "${OMIT_MIT_CRATE:-}" != "$crate" ]]; then
  printf 'MIT\n' > "$archive_root/LICENSE-MIT"
fi
if [[ "${OMIT_LICENSE_CRATE:-}" != "$crate" ]]; then
  printf 'Apache-2.0\n' > "$archive_root/LICENSE-APACHE"
fi
tar -czf "$CARGO_TARGET_DIR/package/$crate-0.0.0.crate" -C "$call_tmp.archive" "$crate-0.0.0"
rm -rf "$call_tmp.archive"
if [[ "${FAIL_CRATE:-}" == "$crate" ]]; then
  echo "error: intentional package failure for $crate" >&2
  exit 42
fi
EOF

chmod +x "$FIXTURE/scripts/"*.sh "$FIXTURE/scripts/fake-cargo"

run_checker() {
  local checker="$1"
  local state_dir="$2"
  local output="$3"
  shift 3
  mkdir -p "$state_dir"
  set +e
  env \
    ROOT="$FIXTURE" \
    CARGO="$FIXTURE/scripts/fake-cargo" \
    CARGO_TARGET_DIR="$state_dir/target" \
    STATE_DIR="$state_dir" \
    EXPECTED_PACKAGES="$PACKAGE_COUNT" \
    MEERKAT_RELEASE_PACKAGING_JOBS="$PACKAGE_COUNT" \
    "$@" \
    "$checker" > "$output" 2>&1
  local status=$?
  set -e
  printf '%s' "$status"
}

run_checker_with_default_target() {
  local state_dir="$1"
  local output="$2"
  mkdir -p "$state_dir"
  env \
    -u CARGO_TARGET_DIR \
    ROOT="$FIXTURE" \
    CARGO="$FIXTURE/scripts/fake-cargo" \
    STATE_DIR="$state_dir" \
    EXPECTED_PACKAGES="$PACKAGE_COUNT" \
    MEERKAT_RELEASE_PACKAGING_JOBS="$PACKAGE_COUNT" \
    "$CHECKER" > "$output" 2>&1
}

wrapper_home="$TEST_ROOT/wrapper-home"
wrapper_a="$(
  CARGO_HOME="$wrapper_home" RUST_LANE_ID=release-package-alpha \
    "$REPO_ROOT/scripts/repo-cargo" --print-env
)"
wrapper_b="$(
  CARGO_HOME="$wrapper_home" RUST_LANE_ID=release-package-beta \
    "$REPO_ROOT/scripts/repo-cargo" --print-env
)"
toolchain_bin_a="$(value_from_env MEERKAT_RUST_TOOLCHAIN_BIN "$wrapper_a")"
toolchain_bin_b="$(value_from_env MEERKAT_RUST_TOOLCHAIN_BIN "$wrapper_b")"
if [[ -n "$toolchain_bin_a" || -n "$toolchain_bin_b" ]]; then
  if [[ -z "$toolchain_bin_a" || -z "$toolchain_bin_b" || "$toolchain_bin_a" == "$toolchain_bin_b" ]]; then
    fail "distinct package lanes share repo-cargo wrapper executables"
  fi
fi

success_state="$TEST_ROOT/success"
success_log="$TEST_ROOT/success.log"
status="$(run_checker "$CHECKER" "$success_state" "$success_log")"
if [[ "$status" -ne 0 ]]; then
  fail "isolated concurrent package verification failed" "$(cat "$success_log")"
fi
[[ -f "$success_state/facade-target" ]] ||
  fail "the facade smoke did not consume the verified package archives"

default_a_state="$TEST_ROOT/default-a"
default_b_state="$TEST_ROOT/default-b"
run_checker_with_default_target "$default_a_state" "$TEST_ROOT/default-a.log" &
default_a_pid=$!
run_checker_with_default_target "$default_b_state" "$TEST_ROOT/default-b.log" &
default_b_pid=$!
if ! wait "$default_a_pid"; then
  fail "the first default-target packaging invocation failed" \
    "$(cat "$TEST_ROOT/default-a.log")"
fi
if ! wait "$default_b_pid"; then
  fail "the second default-target packaging invocation failed" \
    "$(cat "$TEST_ROOT/default-b.log")"
fi
shared_default_targets="$(
  comm -12 \
    <(cut -d '|' -f 1 "$default_a_state"/calls/* | sort -u) \
    <(cut -d '|' -f 1 "$default_b_state"/calls/* | sort -u)
)"
if [[ -n "$shared_default_targets" ]]; then
  fail "concurrent packaging invocations share default Cargo targets" \
    "$shared_default_targets"
fi

mutated_checker="$TEST_ROOT/check-rust-release-packaging-shared-target.sh"
# The literal shell variables identify the mutation site.
# shellcheck disable=SC2016
sed 's|target_dir="$TARGET_ROOT/$crate"|target_dir="$TARGET_ROOT"|' \
  "$CHECKER" > "$mutated_checker"
chmod +x "$mutated_checker"
# The runner resolves its interpreter helper next to itself.
cp "$REPO_ROOT/scripts/require-python" "$TEST_ROOT/require-python"
if cmp -s "$CHECKER" "$mutated_checker"; then
  fail "the shared-target mutation did not modify the packaging runner"
fi

mutation_state="$TEST_ROOT/mutation"
mutation_log="$TEST_ROOT/mutation.log"
status="$(run_checker "$mutated_checker" "$mutation_state" "$mutation_log")"
if [[ "$status" -eq 0 ]]; then
  fail "a shared Cargo target mutation was accepted"
fi
grep -Fq "concurrent package jobs share CARGO_TARGET_DIR" "$mutation_log" ||
  fail "the shared-target mutation failed without naming the collision" "$(cat "$mutation_log")"

failure_state="$TEST_ROOT/failure"
failure_log="$TEST_ROOT/failure.log"
status="$(run_checker "$CHECKER" "$failure_state" "$failure_log" FAIL_CRATE=beta)"
if [[ "$status" -eq 0 ]]; then
  fail "an intentional package failure was hidden"
fi
grep -Fq "beta                              FAIL" "$failure_log" ||
  fail "the failing package was not named" "$(cat "$failure_log")"
grep -Fq "error: intentional package failure for beta" "$failure_log" ||
  fail "the package error was not surfaced" "$(cat "$failure_log")"
if [[ "$(grep -c '^beta$' "$failure_state/invocations")" -ne 1 ]]; then
  fail "the failing package was retried instead of reported"
fi

license_state="$TEST_ROOT/license"
license_log="$TEST_ROOT/license.log"
status="$(run_checker "$CHECKER" "$license_state" "$license_log" OMIT_LICENSE_CRATE=gamma)"
if [[ "$status" -eq 0 ]]; then
  fail "a package archive without LICENSE-APACHE was accepted"
fi
grep -Fq "package archive lacks LICENSE-APACHE" "$license_log" ||
  fail "the archive missing its license file was not named" "$(cat "$license_log")"
grep -Fq "gamma-0.0.0.crate" "$license_log" ||
  fail "the license failure did not name the archive" "$(cat "$license_log")"

# An Apache-2.0-only crate ships LICENSE-APACHE alone.
apache_state="$TEST_ROOT/apache-only"
apache_log="$TEST_ROOT/apache-only.log"
status="$(run_checker "$CHECKER" "$apache_state" "$apache_log" \
  LICENSE_EXPR_CRATE=gamma LICENSE_EXPR=Apache-2.0 OMIT_MIT_CRATE=gamma)"
if [[ "$status" -ne 0 ]]; then
  fail "an Apache-2.0-only archive with only LICENSE-APACHE was rejected" "$(cat "$apache_log")"
fi

# The same crate shipping LICENSE-MIT would misstate its licensing.
apache_mit_state="$TEST_ROOT/apache-only-mit"
apache_mit_log="$TEST_ROOT/apache-only-mit.log"
status="$(run_checker "$CHECKER" "$apache_mit_state" "$apache_mit_log" \
  LICENSE_EXPR_CRATE=gamma LICENSE_EXPR=Apache-2.0)"
if [[ "$status" -eq 0 ]]; then
  fail "an Apache-2.0-only archive shipping LICENSE-MIT was accepted"
fi
grep -Fq "package archive ships LICENSE-MIT, which its license (Apache-2.0) does not name" "$apache_mit_log" ||
  fail "the unexpected license text was not named" "$(cat "$apache_mit_log")"

# An expression the gate does not know fails closed.
unknown_state="$TEST_ROOT/unknown-expression"
unknown_log="$TEST_ROOT/unknown-expression.log"
status="$(run_checker "$CHECKER" "$unknown_state" "$unknown_log" \
  LICENSE_EXPR_CRATE=gamma LICENSE_EXPR=GPL-3.0-only)"
if [[ "$status" -eq 0 ]]; then
  fail "an unsupported license expression was accepted"
fi
grep -Fq "unsupported license expression for gamma: GPL-3.0-only" "$unknown_log" ||
  fail "the unsupported expression was not named" "$(cat "$unknown_log")"

echo "release packaging isolation contract holds"
