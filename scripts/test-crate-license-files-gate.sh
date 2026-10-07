#!/usr/bin/env bash
# Contract test for scripts/check-crate-license-files.sh: the gate must fail
# on the defect it was written for (a release crate whose package file list
# lacks a license file its license expression names), name the crate and the
# missing file, reject a license text the expression does not name, and fail
# closed when cargo cannot list the package or the expression is unknown.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
CHECKER="${REPO_ROOT}/scripts/check-crate-license-files.sh"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-crate-license.XXXXXX")"
trap 'rm -rf "$TEST_ROOT"' EXIT

# A fake cargo driven by the crate name. `cargo metadata` gives `apache-*`
# the expression `Apache-2.0`, `weird-*` `GPL-3.0-only` and every other crate
# `MIT OR Apache-2.0`. `package --list -p <crate>`: `good-*` and
# `apache-mit-*` list both files, `no-apache-*` lacks LICENSE-APACHE,
# `apache-only-*` lists only LICENSE-APACHE, `nested-*` has them only below
# the crate root, `broken-*` fails.
FAKE_CARGO="${TEST_ROOT}/cargo"
cat > "$FAKE_CARGO" <<'EOF'
#!/usr/bin/env bash
if [[ "${1:-}" == metadata ]]; then
  packages=""
  for name in good-a good-b no-apache-a nested-a broken-a apache-only-a apache-mit-a weird-a; do
    case "$name" in
      apache-*) expression="Apache-2.0" ;;
      weird-*) expression="GPL-3.0-only" ;;
      *) expression="MIT OR Apache-2.0" ;;
    esac
    packages+="${packages:+,}{\"name\":\"$name\",\"license\":\"$expression\"}"
  done
  printf '{"packages":[%s]}\n' "$packages"
  exit 0
fi
crate=""
while [[ $# -gt 0 ]]; do
  if [[ "$1" == "-p" ]]; then crate="$2"; shift; fi
  shift
done
printf 'Cargo.toml\nCargo.toml.orig\nsrc/lib.rs\n'
case "$crate" in
  good-* | apache-mit-*) printf 'LICENSE-APACHE\nLICENSE-MIT\n' ;;
  apache-only-*) printf 'LICENSE-APACHE\n' ;;
  no-apache-*) printf 'LICENSE-MIT\n' ;;
  nested-*) printf 'docs/LICENSE-APACHE\ndocs/LICENSE-MIT\n' ;;
  broken-*) echo "error: package \`$crate\` not found" >&2; exit 101 ;;
esac
EOF
chmod +x "$FAKE_CARGO"

run_checker() {
  CARGO="$FAKE_CARGO" ROOT="$REPO_ROOT" "$CHECKER" "$@" > "${TEST_ROOT}/out" 2>&1
}

expect_pass() {
  if ! run_checker "$@"; then
    echo "FAIL: expected the gate to pass for: $*" >&2
    cat "${TEST_ROOT}/out" >&2
    exit 1
  fi
}

expect_fail_naming() {
  local needle="$1"
  shift
  if run_checker "$@"; then
    echo "FAIL: expected the gate to fail for: $*" >&2
    cat "${TEST_ROOT}/out" >&2
    exit 1
  fi
  if ! grep -qF -- "$needle" "${TEST_ROOT}/out"; then
    echo "FAIL: gate output for '$*' does not name: $needle" >&2
    cat "${TEST_ROOT}/out" >&2
    exit 1
  fi
}

expect_pass good-a good-b
expect_fail_naming "no-apache-a" good-a no-apache-a
expect_fail_naming "MISSING LICENSE-APACHE" no-apache-a
expect_fail_naming "MISSING LICENSE-MIT LICENSE-APACHE" nested-a
expect_fail_naming "cargo package --list failed" broken-a
# An Apache-2.0-only crate ships LICENSE-APACHE alone; LICENSE-MIT beside it
# would misstate its licensing.
expect_pass good-a apache-only-a
expect_fail_naming "UNEXPECTED LICENSE-MIT (license is Apache-2.0)" apache-mit-a
# An expression the gate does not know fails closed.
expect_fail_naming "unsupported license expression: GPL-3.0-only" weird-a

echo "crate license files gate contract: OK"
