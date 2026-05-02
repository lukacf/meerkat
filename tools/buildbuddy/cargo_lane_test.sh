#!/usr/bin/env bash
set -euo pipefail

lane="${1:?lane is required}"
runfiles_root="${TEST_SRCDIR:?}/${TEST_WORKSPACE:?}"
work_root="${TEST_TMPDIR:?}/workspace"

find_tool() {
  local name="$1"
  find "${TEST_SRCDIR}" -path "*rust_macos_aarch64__aarch64-apple-darwin__stable_tools*/bin/${name}" \( -type f -o -type l \) | head -1
}

cargo_bin="$(find_tool cargo)"
rustc_bin="$(find_tool rustc)"
rustdoc_bin="$(find_tool rustdoc)"
rustfmt_bin="$(find_tool rustfmt)"
rust_objcopy_bin="$(find_tool rust-objcopy)"
rust_lld_bin="$(find_tool rust-lld)"
if [[ -z "${cargo_bin}" || -z "${rustc_bin}" ]]; then
  echo "rules_rust cargo/rustc runfiles were not found" >&2
  exit 127
fi

rm -rf "${work_root}"
mkdir -p "${work_root}"
rsync -aL \
  --exclude '.git' \
  --exclude 'bazel-*' \
  --exclude 'target' \
  --exclude 'target-*' \
  "${runfiles_root}/" "${work_root}/"

export CARGO="${cargo_bin}"
export RUSTC="${rustc_bin}"
if [[ -n "${rustdoc_bin}" ]]; then
  export RUSTDOC="${rustdoc_bin}"
fi
if [[ -n "${rustfmt_bin}" ]]; then
  export RUSTFMT="${rustfmt_bin}"
fi
tool_dirs=("$(dirname "${cargo_bin}")")
if [[ -n "${rust_objcopy_bin}" ]]; then
  tool_dirs+=("$(dirname "${rust_objcopy_bin}")")
fi
if [[ -n "${rust_lld_bin}" ]]; then
  tool_dirs+=("$(dirname "${rust_lld_bin}")")
fi
tool_path="$(IFS=:; printf '%s' "${tool_dirs[*]}")"
export PATH="${tool_path}:${PATH:-/usr/bin:/bin:/usr/sbin:/sbin}"
export CARGO_HOME="${TEST_TMPDIR}/cargo-home"
export CARGO_TARGET_DIR="${TEST_TMPDIR}/cargo-target"
export CARGO_INCREMENTAL=0
export CARGO_TERM_COLOR=always

cd "${work_root}"

case "${lane}" in
  test-minimal)
    "${CARGO}" check -p meerkat-core
    "${CARGO}" check -p meerkat-client --no-default-features
    "${CARGO}" check -p meerkat-store --no-default-features
    "${CARGO}" check -p meerkat-tools --no-default-features
    "${CARGO}" check -p meerkat --no-default-features
    "${CARGO}" test -p meerkat-core --lib --tests
    ;;
  test-feature-matrix-lib)
    "${CARGO}" check -p meerkat-tools --no-default-features --features comms
    "${CARGO}" check -p meerkat-tools --no-default-features --features mcp
    "${CARGO}" check -p meerkat-tools --no-default-features --features comms,mcp
    "${CARGO}" check -p meerkat --no-default-features --features openai,memory-store
    "${CARGO}" check -p meerkat --no-default-features --features gemini,jsonl-store
    "${CARGO}" check -p meerkat --features all-providers,comms,mcp
    "${CARGO}" check -p meerkat-mob --no-default-features
    "${CARGO}" check -p meerkat-mob --no-default-features --features runtime-adapter
    "${CARGO}" test -p meerkat --features all-providers,comms,mcp --lib --tests
    ;;
  *)
    echo "unknown cargo-equivalent BuildBuddy lane: ${lane}" >&2
    exit 2
    ;;
esac
