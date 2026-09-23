#!/usr/bin/env bash
set -euo pipefail

lane="${1:?lane is required}"
runfiles_root="${TEST_SRCDIR:?}/${TEST_WORKSPACE:?}"
work_root="${TEST_TMPDIR:?}/workspace"

find_tool() {
  local name="$1"
  find "${TEST_SRCDIR}" -path "*${host_triple}__stable_tools*/bin/${name}" \( -type f -o -type l \) | head -1
}

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) host_triple="aarch64-apple-darwin" ;;
  Linux-x86_64) host_triple="x86_64-unknown-linux-gnu" ;;
  *)
    echo "unsupported BuildBuddy cargo lane host: $(uname -s)-$(uname -m)" >&2
    exit 127
    ;;
esac

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

copy_workspace() {
  local attempt log status statuses
  for attempt in 1 2 3; do
    rm -rf "${work_root}"
    mkdir -p "${work_root}"
    log="${TEST_TMPDIR}/workspace-copy-${attempt}.log"
    set +e
    (
      cd "${runfiles_root}"
      tar -chf - --exclude='.git' --exclude='bazel-*' --exclude='target' --exclude='target-*' .
    ) 2>"${log}" | tar -xf - -C "${work_root}" 2>>"${log}"
    statuses=("${PIPESTATUS[@]}")
    set -e
    if [[ "${statuses[0]}" == "0" && "${statuses[1]}" == "0" ]]; then
      return
    fi
    if grep -Fq 'file changed as we read it' "${log}" && [[ "${attempt}" -lt 3 ]]; then
      cat "${log}" >&2
      echo "workspace copy observed changing runfile; retrying (${attempt}/3)" >&2
      sleep "${attempt}"
      continue
    fi
    cat "${log}" >&2
    status="${statuses[0]}"
    if [[ "${status}" == "0" ]]; then
      status="${statuses[1]}"
    fi
    exit "${status}"
  done
}

copy_workspace

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
export CARGO_HOME="${MEERKAT_HOST_CARGO_HOME:-${TEST_TMPDIR}/cargo-home}"
export CARGO_TARGET_DIR="${TEST_TMPDIR}/cargo-target"
export CARGO_INCREMENTAL=0
export CARGO_TERM_COLOR=always

cd "${work_root}"
# Parity with scripts/repo-cargo: source-level tests (meerkat's
# brain_swap_surface_parity reads other crates' files) resolve the workspace
# from this variable, falling back to the cwd only when it is the workspace.
# Cargo runs test binaries from the crate directory, whose Cargo.toml made the
# fallback pick `<workspace>/meerkat` and fail every read on the 09-16 lane.
export MEERKAT_WORKSPACE_ROOT="${work_root}"

run_feature_matrix_lane() {
  local feature_lane="$1"
  case "${feature_lane}" in
    test-feature-matrix-tools-comms)
      "${CARGO}" check -p meerkat-tools --no-default-features --features comms
      ;;
    test-feature-matrix-tools-mcp)
      "${CARGO}" check -p meerkat-tools --no-default-features --features mcp
      ;;
    test-feature-matrix-tools-comms-mcp)
      "${CARGO}" check -p meerkat-tools --no-default-features --features comms,mcp
      ;;
    test-feature-matrix-meerkat-openai-memory)
      "${CARGO}" check -p meerkat --no-default-features --features openai,memory-store
      ;;
    test-feature-matrix-meerkat-gemini-jsonl)
      "${CARGO}" check -p meerkat --no-default-features --features gemini,jsonl-store
      ;;
    test-feature-matrix-meerkat-all-providers-check)
      "${CARGO}" check -p meerkat --features all-providers,comms,mcp
      ;;
    test-feature-matrix-mob-minimal)
      "${CARGO}" check -p meerkat-mob --no-default-features
      ;;
    test-feature-matrix-mob-runtime-adapter)
      "${CARGO}" check -p meerkat-mob --no-default-features --features runtime-adapter
      ;;
    test-feature-matrix-meerkat-all-providers-tests)
      "${CARGO}" test -p meerkat --features all-providers,comms,mcp --lib --tests
      ;;
    *)
      echo "unknown cargo-equivalent feature-matrix lane: ${feature_lane}" >&2
      exit 2
      ;;
  esac
}

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
    for feature_lane in \
      test-feature-matrix-tools-comms \
      test-feature-matrix-tools-mcp \
      test-feature-matrix-tools-comms-mcp \
      test-feature-matrix-meerkat-openai-memory \
      test-feature-matrix-meerkat-gemini-jsonl \
      test-feature-matrix-meerkat-all-providers-check \
      test-feature-matrix-mob-minimal \
      test-feature-matrix-mob-runtime-adapter \
      test-feature-matrix-meerkat-all-providers-tests; do
      run_feature_matrix_lane "${feature_lane}"
    done
    ;;
  test-feature-matrix-*)
    run_feature_matrix_lane "${lane}"
    ;;
  *)
    echo "unknown cargo-equivalent BuildBuddy lane: ${lane}" >&2
    exit 2
    ;;
esac
