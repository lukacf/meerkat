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

# The space readings are observation only, and every measurement in them is
# bounded. The rows below put their own `df` and `du` first on PATH, so no
# figure they assert comes from the host: tools that print fixed figures,
# tools that fail, and tools that never return.
#
# A bash older than 4.4 cannot bound a measurement and starts none: there
# each boundary prints one line that says so, and that line is what a row
# expects in place of its figures (expect_readings).
readings_bounded=false
if [[ "${BASH_VERSINFO[0]}" -gt 4 || ("${BASH_VERSINFO[0]}" -eq 4 && "${BASH_VERSINFO[1]}" -ge 4) ]]; then
  readings_bounded=true
fi

# A directory $1 with a `df` and a `du` whose bodies are $2 and $3.
space_tools() {
  mkdir -p "$1"
  printf '%s\n' '#!/usr/bin/env bash' "$2" > "$1/df"
  printf '%s\n' '#!/usr/bin/env bash' "$3" > "$1/du"
  chmod +x "$1/df" "$1/du"
}
# 1024 MiB free of 2048 MiB, on one filesystem whatever the path.
fixed_df='printf "%s\n" "Filesystem 1024-blocks Used Available Capacity Mounted on" "/dev/fixed 2097152 1048576 1048576 50% /"'
# 3 MiB for the first directory named, 2 MiB for the second, 1 MiB for the
# third. The tool expands its variables when it runs.
# shellcheck disable=SC2016
fixed_du='size=4096; for arg in "$@"; do case "$arg" in -*) ;; *) size=$((size - 1024)); printf "%s\t%s\n" "$size" "$arg" ;; esac; done'
fixed_tools="$TEST_ROOT/fixed-space-tools"
broken_tools="$TEST_ROOT/broken-space-tools"
hanging_df="$TEST_ROOT/hanging-df"
hanging_du="$TEST_ROOT/hanging-du"
stubborn_df="$TEST_ROOT/stubborn-df"
space_tools "$fixed_tools" "$fixed_df" "$fixed_du"
space_tools "$broken_tools" 'echo "broken space tool"; exit 1' 'echo "broken space tool"; exit 1'
# These never return within a row: the sleep is far past the bound and past
# the seconds a row is given. The third also ignores SIGTERM.
space_tools "$hanging_df" 'exec sleep 300' "$fixed_du"
space_tools "$hanging_du" "$fixed_df" 'exec sleep 300'
space_tools "$stubborn_df" 'trap "" TERM; exec sleep 300' "$fixed_du"

# In log $1, under boundary $2, each further argument is a reading line that
# must be there; where the host cannot bound a measurement, the boundary's
# one line that says so must be there instead.
expect_readings() {
  local log="$1" boundary="$2" line
  shift 2
  if [[ "$readings_bounded" != true ]]; then
    set -- "skipped: no bounded measurement available"
  fi
  for line in "$@"; do
    grep -Fqx "space reading [$boundary]: $line" "$log" ||
      fail "a space reading line is missing" "space reading [$boundary]: $line" "$(cat "$log")"
  done
}

# The process ids of $1 and of every process below it.
process_tree() {
  local child
  printf '%s\n' "$1"
  for child in $(pgrep -P "$1" 2>/dev/null); do
    process_tree "$child"
  done
}

# Runs "$@" (a run_checker or run_facade call, which prints the status) for
# at most $1 seconds. A call still running then is ended, with everything it
# started, and "overdue" is printed in place of a status: a measurement that
# never returns must not hold a script, and must not hold this test either.
within_seconds() {
  local limit="$1" started="$SECONDS" pid
  shift
  "$@" &
  pid=$!
  while kill -0 "$pid" 2>/dev/null; do
    if [[ $((SECONDS - started)) -ge "$limit" ]]; then
      # shellcheck disable=SC2046
      kill -KILL $(process_tree "$pid") 2>/dev/null || true
      printf 'overdue'
      return 0
    fi
    sleep 0.1
  done
}

# A directory $1 that holds the repository's scripts by link, so a script
# run from it finds its helpers, with the packaging script and the facade
# link script copied and their one line $2 changed to $3.
changed_scripts() {
  local dir="$1" script
  mkdir -p "$dir"
  ln -s "$REPO_ROOT"/scripts/* "$dir/"
  for script in check-rust-release-packaging.sh check-published-facade-link.sh; do
    rm -- "$dir/$script"
    awk -v old="$2" -v new="$3" '$0 == old { print new; next } { print }' \
      "$REPO_ROOT/scripts/$script" > "$dir/$script"
    chmod +x "$dir/$script"
    if cmp -s "$REPO_ROOT/scripts/$script" "$dir/$script"; then
      fail "the line to change is not in $script" "$2"
    fi
  done
}
# The same scripts with a bound of 2 seconds, and with no way to bound.
short_bound_scripts="$TEST_ROOT/scripts-short-bound"
unbounded_host_scripts="$TEST_ROOT/scripts-unbounded-host"
changed_scripts "$short_bound_scripts" 'SPACE_READING_BOUND=60' 'SPACE_READING_BOUND=2'
changed_scripts "$unbounded_host_scripts" 'space_bounded_available() {' 'space_bounded_available() { return 1'
short_bound_checker="$short_bound_scripts/check-rust-release-packaging.sh"
unbounded_host_checker="$unbounded_host_scripts/check-rust-release-packaging.sh"

# Fixed figures: every line of the three boundaries is what the tools said.
figures_state="$TEST_ROOT/fixed-readings"
figures_log="$TEST_ROOT/fixed-readings.log"
status="$(run_checker "$CHECKER" "$figures_state" "$figures_log" PATH="$fixed_tools:$PATH")"
if [[ "$status" -ne 0 ]]; then
  fail "a packaging run with fixed space readings failed" "$(cat "$figures_log")"
fi
expect_readings "$figures_log" "packaging start" \
  "packaging target filesystem: 1024 MiB free of 2048 MiB" \
  "temporary directory filesystem: 1024 MiB free of 2048 MiB (same filesystem as packaging target)" \
  "root filesystem: 1024 MiB free of 2048 MiB (same filesystem as packaging target)"
expect_readings "$figures_log" "packaging end" \
  "packaging target filesystem: 1024 MiB free of 2048 MiB" \
  "packaging target, 3 per-crate directories of the release crates, whole contents: 6 MiB" \
  "crate 1 of 3 by size: alpha 3 MiB" \
  "crate 2 of 3 by size: beta 2 MiB" \
  "crate 3 of 3 by size: gamma 1 MiB"
expect_readings "$figures_log" "before the link smoke" \
  "packaging target filesystem: 1024 MiB free of 2048 MiB" \
  "staged archive copies: 3 MiB"

# A `df` and a `du` that both fail: each reading says it is unavailable and
# nothing else changes: the run still passes and the facade smoke still runs.
reading_state="$TEST_ROOT/unavailable-readings"
reading_log="$TEST_ROOT/unavailable-readings.log"
status="$(run_checker "$CHECKER" "$reading_state" "$reading_log" PATH="$broken_tools:$PATH")"
if [[ "$status" -ne 0 ]]; then
  fail "a space reading that could not be taken changed the exit status" "$(cat "$reading_log")"
fi
expect_readings "$reading_log" "packaging start" "packaging target filesystem: unavailable"
expect_readings "$reading_log" "packaging end" "packaging target: unavailable"
expect_readings "$reading_log" "before the link smoke" "staged archive copies: unavailable"
[[ -f "$reading_state/facade-target" ]] ||
  fail "the facade smoke did not run after space readings that could not be taken"

# A `df` that never returns: each boundary gives it the bound, ends it, says
# so, skips what was left of the boundary, and the run goes on to pass. The
# whole run stays far inside the seconds the row gives it.
hanging_state="$TEST_ROOT/hanging-readings"
hanging_log="$TEST_ROOT/hanging-readings.log"
status="$(within_seconds 60 run_checker "$short_bound_checker" "$hanging_state" "$hanging_log" \
  PATH="$hanging_df:$PATH")"
if [[ "$status" != 0 ]]; then
  fail "a space reading that never returns held the packaging run or changed its status" \
    "status: $status" "$(cat "$hanging_log")"
fi
for hanging_boundary in "packaging start" "packaging end" "before the link smoke"; do
  expect_readings "$hanging_log" "$hanging_boundary" \
    "packaging target filesystem: timed out after 2 s" \
    "root filesystem: skipped: the time bound is used up"
done
expect_readings "$hanging_log" "packaging end" "packaging target: skipped: the time bound is used up"
[[ -f "$hanging_state/facade-target" ]] ||
  fail "the facade smoke did not run after space readings that never returned"

# No way to bound a measurement: none is started, each boundary says so, and
# nothing else changes. The tools here would never return if they were run.
unbounded_state="$TEST_ROOT/unbounded-host-readings"
unbounded_log="$TEST_ROOT/unbounded-host-readings.log"
status="$(within_seconds 60 run_checker "$unbounded_host_checker" "$unbounded_state" "$unbounded_log" \
  PATH="$hanging_df:$PATH")"
if [[ "$status" != 0 ]]; then
  fail "a host that cannot bound a measurement did not skip its space readings" \
    "status: $status" "$(cat "$unbounded_log")"
fi
for unbounded_boundary in "packaging start" "packaging end" "before the link smoke"; do
  grep -Fqx "space reading [$unbounded_boundary]: skipped: no bounded measurement available" "$unbounded_log" ||
    fail "a boundary that could not be bounded did not say it was skipped" "$unbounded_boundary" \
      "$(cat "$unbounded_log")"
done
[[ -f "$unbounded_state/facade-target" ]] ||
  fail "the facade smoke did not run after skipped space readings"

# The facade link script's exit readings, on the real script with a fake cargo
# in place of the smoke's build. For a smoke that passes, one that fails and
# an exit before the smoke (no archive staged), each with fixed and with
# failing tools: the script exits with the status it has without the
# readings, removes its temporary directory, and prints its readings under
# that status.
FACADE="$REPO_ROOT/scripts/check-published-facade-link.sh"
short_bound_facade="$short_bound_scripts/check-published-facade-link.sh"
unbounded_host_facade="$unbounded_host_scripts/check-published-facade-link.sh"
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
facade_no_packages="$TEST_ROOT/facade-no-packages"
mkdir -p "$facade_packages/package"
cp "$success_state"/target/*/package/*-0.0.0.crate "$facade_packages/package/"

# Runs facade link script $2 as case $1: smoke $3 (pass or fail), staged
# archives $4, PATH $5. Its temporary directory is created under a directory
# of the case's own. With a sixth argument its standard output is file
# descriptor 8 and the log holds its error output only. Prints its status.
run_facade() {
  local name="$1" script="$2" status
  local -a facade_env=(
    ROOT="$FIXTURE"
    CARGO="$FIXTURE/scripts/fake-smoke-cargo"
    FAKE_SMOKE="$3"
    MEERKAT_PUBLISHED_FACADE_PACKAGE_TARGET="$4"
    PATH="$5"
    TMPDIR="$TEST_ROOT/facade-$name-tmp"
  )
  mkdir -p "$TEST_ROOT/facade-$name-tmp"
  set +e
  if [[ "$#" -ge 6 ]]; then
    env "${facade_env[@]}" "$script" >&8 2> "$TEST_ROOT/facade-$name.log"
  else
    env "${facade_env[@]}" "$script" > "$TEST_ROOT/facade-$name.log" 2>&1
  fi
  status=$?
  set -e
  printf '%s' "$status"
}

# Case $1 ended with status $2 where $3 was expected, and left nothing in
# the directory its temporary directory was created in.
expect_facade_exit() {
  local name="$1" log="$TEST_ROOT/facade-$1.log"
  if [[ "$2" != "$3" ]]; then
    fail "the facade link script's exit status is not what it is without its readings ($name)" \
      "expected $3, got $2" "$(cat "$log")"
  fi
  if [[ "$(find "$TEST_ROOT/facade-$name-tmp" -mindepth 1 | wc -l | tr -d ' ')" -ne 0 ]]; then
    fail "the facade link script did not remove its temporary directory ($name)" "$(cat "$log")"
  fi
}

for facade_case in "pass pass 0" "fail fail 1" "early pass 1"; do
  read -r facade_scenario facade_smoke facade_expected <<< "$facade_case"
  facade_boundary="link smoke exit, status $facade_expected"
  facade_staged="$facade_packages"
  facade_size="smoke target: 3 MiB"
  if [[ "$facade_scenario" == early ]]; then
    facade_staged="$facade_no_packages"
    facade_size="smoke target: no such directory"
  fi

  status="$(run_facade "$facade_scenario-fixed" "$FACADE" "$facade_smoke" "$facade_staged" "$fixed_tools:$PATH")"
  expect_facade_exit "$facade_scenario-fixed" "$status" "$facade_expected"
  expect_readings "$TEST_ROOT/facade-$facade_scenario-fixed.log" "$facade_boundary" \
    "temporary directory filesystem: 1024 MiB free of 2048 MiB" \
    "root filesystem: 1024 MiB free of 2048 MiB (same filesystem as temporary directory)" \
    "$facade_size" \
    "unpacked registry directory: 3 MiB"

  if [[ "$facade_scenario" != early ]]; then
    facade_size="smoke target: unavailable"
  fi
  status="$(run_facade "$facade_scenario-failing" "$FACADE" "$facade_smoke" "$facade_staged" "$broken_tools:$PATH")"
  expect_facade_exit "$facade_scenario-failing" "$status" "$facade_expected"
  expect_readings "$TEST_ROOT/facade-$facade_scenario-failing.log" "$facade_boundary" \
    "temporary directory filesystem: unavailable" \
    "$facade_size" \
    "unpacked registry directory: unavailable"
done
grep -Fq "Published-style facade AgentBuilder link smoke passed" "$TEST_ROOT/facade-pass-fixed.log" ||
  fail "the passing facade smoke did not print its verdict" "$(cat "$TEST_ROOT/facade-pass-fixed.log")"
grep -Fq "error: intentional smoke failure" "$TEST_ROOT/facade-fail-fixed.log" ||
  fail "the failing facade smoke's error was not surfaced" "$(cat "$TEST_ROOT/facade-fail-fixed.log")"
grep -Fq "missing packaged crate archive" "$TEST_ROOT/facade-early-fixed.log" ||
  fail "the facade link script did not name the missing archive" "$(cat "$TEST_ROOT/facade-early-fixed.log")"

# A measurement that never returns in the exit trap: a failing smoke with a
# `df` that also ignores SIGTERM, and a passing smoke with a `du`. The trap
# ends the measurement at the bound, says so, and still reaches the cleanup:
# the status is the script's own and the temporary directory is removed, far
# inside the seconds the case is given.
status="$(within_seconds 60 run_facade fail-hanging-df "$short_bound_facade" fail "$facade_packages" \
  "$stubborn_df:$PATH")"
expect_facade_exit fail-hanging-df "$status" 1
expect_readings "$TEST_ROOT/facade-fail-hanging-df.log" "link smoke exit, status 1" \
  "temporary directory filesystem: timed out after 2 s" \
  "root filesystem: skipped: the time bound is used up" \
  "smoke target: skipped: the time bound is used up"
status="$(within_seconds 60 run_facade pass-hanging-du "$short_bound_facade" pass "$facade_packages" \
  "$hanging_du:$PATH")"
expect_facade_exit pass-hanging-du "$status" 0
expect_readings "$TEST_ROOT/facade-pass-hanging-du.log" "link smoke exit, status 0" \
  "temporary directory filesystem: 1024 MiB free of 2048 MiB" \
  "smoke target: timed out after 2 s" \
  "unpacked registry directory: skipped: the time bound is used up"

# No way to bound a measurement in the exit trap: none is started, the trap
# says so and reaches the cleanup with the script's own status.
status="$(within_seconds 60 run_facade fail-unbounded-host "$unbounded_host_facade" fail "$facade_packages" \
  "$hanging_df:$PATH")"
expect_facade_exit fail-unbounded-host "$status" 1
grep -Fqx "space reading [link smoke exit, status 1]: skipped: no bounded measurement available" \
  "$TEST_ROOT/facade-fail-unbounded-host.log" ||
  fail "an exit trap that could not bound its readings did not say they were skipped" \
    "$(cat "$TEST_ROOT/facade-fail-unbounded-host.log")"

# With its standard output a pipe nobody reads, the script still exits with
# its own status and still removes its temporary directory: the shell that
# runs the exit trap writes nothing. The pipe's reader has exited before the
# script starts, so every write to it fails.
mkfifo "$TEST_ROOT/facade-dead-pipe"
(: < "$TEST_ROOT/facade-dead-pipe") &
facade_reader=$!
exec 8> "$TEST_ROOT/facade-dead-pipe"
wait "$facade_reader"
status="$(run_facade fail-dead-pipe "$FACADE" fail "$facade_packages" "$fixed_tools:$PATH" dead-pipe)"
expect_facade_exit fail-dead-pipe "$status" 1
status="$(run_facade early-dead-pipe "$FACADE" pass "$facade_no_packages" "$fixed_tools:$PATH" dead-pipe)"
expect_facade_exit early-dead-pipe "$status" 1
exec 8>&-

echo "release packaging isolation contract holds"
