#!/usr/bin/env bash
set -euo pipefail

ROOT="$(git rev-parse --show-toplevel)"
PYTHON="$("$(dirname "${BASH_SOURCE[0]}")/require-python" 3.11 "$(basename "${BASH_SOURCE[0]}")")" || exit 1
CHECKER="${ROOT}/scripts/check-bazel-path-patch-runfiles.py"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-path-patch-runfiles.XXXXXX")"
trap 'rm -rf "${TEST_ROOT}"' EXIT

mkdir -p "${TEST_ROOT}/third-party/example/src"
cat >"${TEST_ROOT}/Cargo.toml" <<'EOF'
[patch.crates-io]
example = { path = "third-party/example" }
EOF
cat >"${TEST_ROOT}/BUILD.bazel" <<'EOF'
filegroup(
    name = "workspace_runfiles",
    srcs = ["//third-party/example:package_runfiles"],
)
EOF
cat >"${TEST_ROOT}/third-party/example/BUILD.bazel" <<'EOF'
filegroup(
    name = "package_runfiles",
    srcs = glob(["**/*"]),
)
EOF

"${PYTHON}" "${CHECKER}" "${TEST_ROOT}" >/dev/null

sed -i.bak '/package_runfiles/d' "${TEST_ROOT}/BUILD.bazel"
if "${PYTHON}" "${CHECKER}" "${TEST_ROOT}" >/dev/null 2>&1; then
  echo "error: checker accepted a root runfiles set missing the path patch" >&2
  exit 1
fi
mv "${TEST_ROOT}/BUILD.bazel.bak" "${TEST_ROOT}/BUILD.bazel"

sed -i.bak '/name = "package_runfiles"/d' "${TEST_ROOT}/third-party/example/BUILD.bazel"
if "${PYTHON}" "${CHECKER}" "${TEST_ROOT}" >/dev/null 2>&1; then
  echo "error: checker accepted a path patch without a package_runfiles export" >&2
  exit 1
fi

echo "Bazel path-patch runfiles gate tests passed"
