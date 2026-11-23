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

# Archives already present when packaging starts were left by an earlier run.
for existing in "$CARGO_TARGET_DIR/package/"*.crate; do
  if [[ -e "$existing" ]]; then
    printf '%s\n' "$existing" >> "$STATE_DIR/preexisting-archives"
  fi
done

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
# license-file and internal-requirement inspection reads it like a published
# archive. beta requires alpha, and gamma requires beta through a renamed
# target dev-dependency, as a normalized manifest writes them;
# CARET_INTERNAL_CRATE makes that crate's internal requirement caret.
archive_root="$call_tmp.archive/$crate-0.0.0"
mkdir -p "$archive_root"
internal_requirement="=0.0.0"
[[ "${CARET_INTERNAL_CRATE:-}" == "$crate" ]] && internal_requirement="0.0.0"
{
  printf '[package]\nname = "%s"\nversion = "0.0.0"\n' "$crate"
  case "$crate" in
    beta)
      printf '\n[dependencies.alpha]\nversion = "%s"\n' "$internal_requirement"
      ;;
    gamma)
      printf '\n[target."cfg(unix)".dev-dependencies.renamed-beta]\npackage = "beta"\nversion = "%s"\n' \
        "$internal_requirement"
      ;;
  esac
} > "$archive_root/Cargo.toml"
if [[ "${OMIT_MIT_CRATE:-}" != "$crate" ]]; then
  printf 'MIT\n' > "$archive_root/LICENSE-MIT"
fi
if [[ "${OMIT_LICENSE_CRATE:-}" != "$crate" ]]; then
  printf 'Apache-2.0\n' > "$archive_root/LICENSE-APACHE"
fi
# SKIP_ARCHIVE_CRATE packages that crate without an archive at the workspace
# version, as cargo does for a crate that sets its own version.
if [[ "${SKIP_ARCHIVE_CRATE:-}" != "$crate" ]]; then
  tar -czf "$CARGO_TARGET_DIR/package/$crate-0.0.0.crate" -C "$call_tmp.archive" "$crate-0.0.0"
fi
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

# A packaged caret requirement on a sibling release crate fails, named by
# archive, table and dependency key.
caret_state="$TEST_ROOT/caret-internal"
caret_log="$TEST_ROOT/caret-internal.log"
status="$(run_checker "$CHECKER" "$caret_state" "$caret_log" CARET_INTERNAL_CRATE=gamma)"
if [[ "$status" -eq 0 ]]; then
  fail "a packaged caret requirement on a release crate was accepted"
fi
grep -Fq 'gamma-0.0.0.crate: [target.cfg(unix).dev-dependencies] `renamed-beta` requires `0.0.0`, not `=0.0.0`' \
  "$caret_log" ||
  fail "the inexact packaged requirement was not named" "$(cat "$caret_log")"
if grep -Fq 'beta-0.0.0.crate:' "$caret_log"; then
  fail "an exact packaged requirement was reported" "$(cat "$caret_log")"
fi
[[ ! -f "$caret_state/facade-target" ]] ||
  fail "the facade smoke ran after an inexact packaged requirement"

# A persistent CARGO_TARGET_DIR keeps the archives of earlier runs, here one
# at this version and one at a pre-release of it. A rerun checks only what it
# packaged itself: a crate whose rerun wrote no archive fails as missing,
# instead of passing on the earlier run's archive.
stale_target="$TEST_ROOT/stale-archive-target"
earlier_state="$TEST_ROOT/stale-archive-earlier"
status="$(run_checker "$CHECKER" "$earlier_state" "$TEST_ROOT/stale-archive-earlier.log" \
  CARGO_TARGET_DIR="$stale_target")"
if [[ "$status" -ne 0 ]]; then
  fail "the earlier packaging run failed" "$(cat "$TEST_ROOT/stale-archive-earlier.log")"
fi
earlier_archive="$stale_target/gamma/package/gamma-0.0.0.crate"
[[ -f "$earlier_archive" ]] ||
  fail "the earlier packaging run left no archive in the persistent target" "$earlier_archive"
cp "$earlier_archive" "$stale_target/gamma/package/gamma-0.0.0-rc.1.crate"

stale_state="$TEST_ROOT/stale-archive"
stale_log="$TEST_ROOT/stale-archive.log"
status="$(run_checker "$CHECKER" "$stale_state" "$stale_log" \
  CARGO_TARGET_DIR="$stale_target" SKIP_ARCHIVE_CRATE=gamma)"
if [[ "$status" -eq 0 ]]; then
  fail "a package archive left by an earlier run was checked as this run's" "$(cat "$stale_log")"
fi
grep -Fq "missing verified package archive: $earlier_archive" "$stale_log" ||
  fail "the crate without an archive from this run was not named" "$(cat "$stale_log")"
if [[ -f "$stale_state/preexisting-archives" ]]; then
  fail "package jobs started beside archives from an earlier run" \
    "$(cat "$stale_state/preexisting-archives")"
fi
[[ ! -f "$stale_state/facade-target" ]] ||
  fail "the facade smoke ran without this run's archives"

# The space readings are observation only. With a `df` and a `du` that both
# fail, each reading says it is unavailable and nothing else changes: the run
# still passes and the facade smoke still runs.
broken_tools="$TEST_ROOT/broken-space-tools"
mkdir -p "$broken_tools"
printf '%s\n' '#!/usr/bin/env bash' 'echo "broken space tool"' 'exit 1' > "$broken_tools/df"
cp "$broken_tools/df" "$broken_tools/du"
chmod +x "$broken_tools/df" "$broken_tools/du"
reading_state="$TEST_ROOT/unavailable-readings"
reading_log="$TEST_ROOT/unavailable-readings.log"
status="$(run_checker "$CHECKER" "$reading_state" "$reading_log" PATH="$broken_tools:$PATH")"
if [[ "$status" -ne 0 ]]; then
  fail "a space reading that could not be taken changed the exit status" "$(cat "$reading_log")"
fi
for unavailable in \
  "space reading [packaging start]: packaging target filesystem: unavailable" \
  "space reading [packaging end]: packaging target: unavailable" \
  "space reading [before the link smoke]: staged archive copies: unavailable"; do
  grep -Fq "$unavailable" "$reading_log" ||
    fail "a space reading that could not be taken was not reported as unavailable" \
      "$unavailable" "$(cat "$reading_log")"
done
[[ -f "$reading_state/facade-target" ]] ||
  fail "the facade smoke did not run after space readings that could not be taken"

# The facade link script's exit readings, on the real script with a fake cargo
# in place of the smoke's build. For a smoke that passes, one that fails and
# an exit before the smoke (no archive staged), each with a working and with a
# failing `df` and `du`: the script exits with the status it has without the
# readings, removes its temporary directory, and prints its readings under
# that status.
FACADE="$REPO_ROOT/scripts/check-published-facade-link.sh"
mkdir -p "$FIXTURE/crates/meerkat/tests/fixtures/agent_builder_policy"
printf '%s\n' 'fn main() {}' \
  > "$FIXTURE/crates/meerkat/tests/fixtures/agent_builder_policy/downstream_public_facade_agentbuilder.rs"
cat > "$FIXTURE/scripts/fake-smoke-cargo" <<'EOF'
#!/usr/bin/env bash
set -euo pipefail
# Stands in for `cargo run` of the downstream smoke: leaves a build output in
# its target directory, then prints the smoke's sentence, or fails without it.
: "${CARGO_TARGET_DIR:?}"
: "${FAKE_SMOKE:?}"
mkdir -p "$CARGO_TARGET_DIR/debug"
printf 'object\n' > "$CARGO_TARGET_DIR/debug/smoke"
if [[ "$FAKE_SMOKE" == pass ]]; then
  echo "public facade AgentBuilder constructed an agent"
  exit 0
fi
echo "error: intentional smoke failure" >&2
exit 101
EOF
chmod +x "$FIXTURE/scripts/fake-smoke-cargo"
facade_packages="$TEST_ROOT/facade-packages"
mkdir -p "$facade_packages/package"
cp "$success_state"/target/*/package/*-0.0.0.crate "$facade_packages/package/"

# Runs the facade link script as case $1: smoke $2 (pass or fail), staged
# archives $3, PATH $4. Its temporary directory is created under a directory
# of the case's own. With a fifth argument its standard output is file
# descriptor 8 and the log holds its error output only. Prints its status.
run_facade() {
  local name="$1" status
  local -a facade_env=(
    ROOT="$FIXTURE"
    CARGO="$FIXTURE/scripts/fake-smoke-cargo"
    FAKE_SMOKE="$2"
    MEERKAT_PUBLISHED_FACADE_PACKAGE_TARGET="$3"
    PATH="$4"
    TMPDIR="$TEST_ROOT/facade-$name-tmp"
  )
  mkdir -p "$TEST_ROOT/facade-$name-tmp"
  set +e
  if [[ "$#" -ge 5 ]]; then
    env "${facade_env[@]}" "$FACADE" >&8 2> "$TEST_ROOT/facade-$name.log"
  else
    env "${facade_env[@]}" "$FACADE" > "$TEST_ROOT/facade-$name.log" 2>&1
  fi
  status=$?
  set -e
  printf '%s' "$status"
}

# How many entries case $1 left in the directory its temporary directory was
# created in.
facade_left_behind() {
  find "$TEST_ROOT/facade-$1-tmp" -mindepth 1 | wc -l | tr -d ' '
}

for facade_case in "pass pass 0" "fail fail 1" "early pass 1"; do
  read -r facade_scenario facade_smoke facade_expected <<< "$facade_case"
  facade_staged="$facade_packages"
  if [[ "$facade_scenario" == early ]]; then
    facade_staged="$TEST_ROOT/facade-no-packages"
  fi
  for facade_tools in working failing; do
    facade_path="$PATH"
    if [[ "$facade_tools" == failing ]]; then
      facade_path="$broken_tools:$PATH"
    fi
    facade_name="$facade_scenario-$facade_tools"
    facade_log="$TEST_ROOT/facade-$facade_name.log"
    facade_what="($facade_scenario, $facade_tools df and du)"
    # An extended regular expression: the label's brackets are literal.
    facade_label="space reading \\[link smoke exit, status $facade_expected\\]"
    status="$(run_facade "$facade_name" "$facade_smoke" "$facade_staged" "$facade_path")"
    if [[ "$status" -ne "$facade_expected" ]]; then
      fail "the facade link script's exit status is not what it is without its readings $facade_what" \
        "expected $facade_expected, got $status" "$(cat "$facade_log")"
    fi
    if [[ "$(facade_left_behind "$facade_name")" -ne 0 ]]; then
      fail "the facade link script did not remove its temporary directory $facade_what" "$(cat "$facade_log")"
    fi
    if [[ "$facade_tools" == failing ]]; then
      facade_free="temporary directory filesystem: unavailable"
      facade_size="smoke target: unavailable"
    else
      facade_free="temporary directory filesystem: [0-9]+ MiB free of [0-9]+ MiB"
      facade_size="smoke target: [0-9]+ MiB"
    fi
    if [[ "$facade_scenario" == early ]]; then
      facade_size="smoke target: no such directory"
    fi
    for facade_line in "$facade_free" "$facade_size" "took [0-9]+ s"; do
      grep -Eq "^$facade_label: $facade_line\$" "$facade_log" ||
        fail "the facade link script's exit readings lack a line under its exit status $facade_what" \
          "$facade_line" "$(cat "$facade_log")"
    done
  done
done
grep -Fq "Published-style facade AgentBuilder link smoke passed" "$TEST_ROOT/facade-pass-working.log" ||
  fail "the passing facade smoke did not print its verdict" "$(cat "$TEST_ROOT/facade-pass-working.log")"
grep -Fq "error: intentional smoke failure" "$TEST_ROOT/facade-fail-working.log" ||
  fail "the failing facade smoke's error was not surfaced" "$(cat "$TEST_ROOT/facade-fail-working.log")"
grep -Fq "missing packaged crate archive" "$TEST_ROOT/facade-early-working.log" ||
  fail "the facade link script did not name the missing archive" "$(cat "$TEST_ROOT/facade-early-working.log")"

# With its standard output a pipe nobody reads, the script still exits with
# its own status and still removes its temporary directory: the shell that
# runs the exit trap writes nothing. The pipe's reader has exited before the
# script starts, so every write to it fails.
mkfifo "$TEST_ROOT/facade-dead-pipe"
(: < "$TEST_ROOT/facade-dead-pipe") &
facade_reader=$!
exec 8> "$TEST_ROOT/facade-dead-pipe"
wait "$facade_reader"
for facade_case in "fail fail 1" "early pass 1"; do
  read -r facade_scenario facade_smoke facade_expected <<< "$facade_case"
  facade_staged="$facade_packages"
  if [[ "$facade_scenario" == early ]]; then
    facade_staged="$TEST_ROOT/facade-no-packages"
  fi
  facade_name="$facade_scenario-dead-pipe"
  status="$(run_facade "$facade_name" "$facade_smoke" "$facade_staged" "$PATH" dead-pipe)"
  if [[ "$status" -ne "$facade_expected" ]]; then
    fail "a standard output nobody reads changed the facade link script's exit status ($facade_scenario)" \
      "expected $facade_expected, got $status" "$(cat "$TEST_ROOT/facade-$facade_name.log")"
  fi
  if [[ "$(facade_left_behind "$facade_name")" -ne 0 ]]; then
    fail "a standard output nobody reads kept the facade link script from removing its temporary directory ($facade_scenario)" \
      "$(cat "$TEST_ROOT/facade-$facade_name.log")"
  fi
done
exec 8>&-

echo "release packaging isolation contract holds"
