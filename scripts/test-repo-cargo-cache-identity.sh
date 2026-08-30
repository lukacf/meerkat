#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TEST_ROOT="$(mktemp -d "${TMPDIR:-/tmp}/meerkat-repo-cargo-identity.XXXXXX")"
TEST_REPO="${TEST_ROOT}/source-repo"
WORKTREE_A="${TEST_ROOT}/detached-a"
WORKTREE_B="${TEST_ROOT}/detached-b"

cleanup() {
  local pending_status=$?
  git -C "${TEST_REPO}" worktree remove --force "${WORKTREE_A}" >/dev/null 2>&1 || true
  git -C "${TEST_REPO}" worktree remove --force "${WORKTREE_B}" >/dev/null 2>&1 || true
  chmod -R u+rwx "${TEST_ROOT}" 2>/dev/null || true
  rm -rf -- "${TEST_ROOT}"
  exit "${pending_status}"
}
trap cleanup EXIT

value_from_env() {
  local key="$1"
  local text="$2"
  printf '%s\n' "${text}" | awk -F= -v key="${key}" '$1 == key { print substr($0, length(key) + 2) }'
}

mkdir -p "${TEST_REPO}/scripts"
cp "${ROOT}/scripts/repo-cargo" "${TEST_REPO}/scripts/repo-cargo"
cp "${ROOT}/rust-toolchain.toml" "${TEST_REPO}/rust-toolchain.toml"
git -C "${TEST_REPO}" init -q
git -C "${TEST_REPO}" add scripts/repo-cargo rust-toolchain.toml
git -C "${TEST_REPO}" -c user.name=Meerkat -c user.email=meerkat@example.invalid \
  commit -qm "cache identity fixture"
git -C "${TEST_REPO}" worktree add --detach --quiet "${WORKTREE_A}" HEAD
git -C "${TEST_REPO}" worktree add --detach --quiet "${WORKTREE_B}" HEAD

shared_a="$(cd "${WORKTREE_A}" && RUST_LANE_ID=shared-cache ./scripts/repo-cargo --print-env)"
shared_b="$(cd "${WORKTREE_B}" && RUST_LANE_ID=shared-cache ./scripts/repo-cargo --print-env)"
shared_source="$(cd "${TEST_REPO}" && RUST_LANE_ID=shared-cache ./scripts/repo-cargo --print-env)"
repo_key_a="$(value_from_env repo_key "${shared_a}")"
repo_key_b="$(value_from_env repo_key "${shared_b}")"
repo_key_source="$(value_from_env repo_key "${shared_source}")"
cargo_home_a="$(value_from_env CARGO_HOME "${shared_a}")"
cargo_home_b="$(value_from_env CARGO_HOME "${shared_b}")"
cargo_home_source="$(value_from_env CARGO_HOME "${shared_source}")"
target_a="$(value_from_env CARGO_TARGET_DIR "${shared_a}")"
target_b="$(value_from_env CARGO_TARGET_DIR "${shared_b}")"
target_source="$(value_from_env CARGO_TARGET_DIR "${shared_source}")"
toolchain_bin_a="$(value_from_env MEERKAT_RUST_TOOLCHAIN_BIN "${shared_a}")"
toolchain_bin_b="$(value_from_env MEERKAT_RUST_TOOLCHAIN_BIN "${shared_b}")"
toolchain_bin_source="$(value_from_env MEERKAT_RUST_TOOLCHAIN_BIN "${shared_source}")"

if ! [[ "${repo_key_a}" == "${repo_key_b}" && "${repo_key_a}" == "${repo_key_source}" ]]; then
  echo "repo-cargo split one repository by detached worktree basename" >&2
  printf 'source repo key: %s\nworktree A repo key: %s\nworktree B repo key: %s\n' \
    "${repo_key_source}" "${repo_key_a}" "${repo_key_b}" >&2
  exit 1
fi
if ! [[ "${cargo_home_a}" == "${cargo_home_b}" &&
  "${cargo_home_a}" == "${cargo_home_source}" &&
  "${target_a}" == "${target_b}" && "${target_a}" == "${target_source}" &&
  "${toolchain_bin_a}" == "${toolchain_bin_b}" &&
  "${toolchain_bin_a}" == "${toolchain_bin_source}" ]]; then
  echo "repo-cargo did not reuse one explicitly named lane across linked worktrees" >&2
  exit 1
fi

default_a="$(cd "${WORKTREE_A}" && env -u RUST_LANE_ID ./scripts/repo-cargo --print-env)"
default_b="$(cd "${WORKTREE_B}" && env -u RUST_LANE_ID ./scripts/repo-cargo --print-env)"
default_target_a="$(value_from_env CARGO_TARGET_DIR "${default_a}")"
default_target_b="$(value_from_env CARGO_TARGET_DIR "${default_b}")"
default_toolchain_bin_a="$(value_from_env MEERKAT_RUST_TOOLCHAIN_BIN "${default_a}")"
default_toolchain_bin_b="$(value_from_env MEERKAT_RUST_TOOLCHAIN_BIN "${default_b}")"
if [[ "${default_target_a}" == "${default_target_b}" ]]; then
  echo "repo-cargo collapsed distinct default worktree target lanes" >&2
  exit 1
fi
if [[ -n "${default_toolchain_bin_a}" || -n "${default_toolchain_bin_b}" ]]; then
  if [[ -z "${default_toolchain_bin_a}" || -z "${default_toolchain_bin_b}" ||
    "${default_toolchain_bin_a}" == "${default_toolchain_bin_b}" ]]; then
    echo "repo-cargo collapsed distinct default worktree executable lanes" >&2
    exit 1
  fi
fi

echo "repo-cargo linked-worktree cache identity selftest passed"
