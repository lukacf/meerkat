#!/usr/bin/env bash
set -euo pipefail

lane="${1:?lane is required}"
runfiles_root="${TEST_SRCDIR:?}/${TEST_WORKSPACE:?}"
work_root="${TEST_TMPDIR:?}/workspace"

find_runfile() {
  local pattern="$1"
  find "${TEST_SRCDIR}" -path "${pattern}" \( -type f -o -type l \) | head -1
}

find_rust_tool() {
  local toolchain="$1"
  local name="$2"
  find_runfile "*${toolchain}*/bin/${name}"
}

case "$(uname -s)-$(uname -m)" in
  Darwin-arm64)
    host_triple="aarch64-apple-darwin"
    host_rust_toolchain="rust_macos_aarch64__aarch64-apple-darwin__stable_tools"
    wasm_rust_toolchain="rust_macos_aarch64__wasm32-unknown-unknown__stable_tools"
    node_repo="node_darwin_arm64"
    python_repo="python_darwin_arm64"
    cargo_deny_repo="cargo_deny_darwin_arm64"
    cargo_nextest_repo="cargo_nextest_darwin_arm64"
    wasm_pack_repo="wasm_pack_darwin_arm64"
    ;;
  Linux-x86_64)
    host_triple="x86_64-unknown-linux-gnu"
    host_rust_toolchain="rust_linux_x86_64__x86_64-unknown-linux-gnu__stable_tools"
    wasm_rust_toolchain="rust_linux_x86_64__wasm32-unknown-unknown__stable_tools"
    node_repo="node_linux_x86_64"
    python_repo="python_linux_x86_64"
    cargo_deny_repo="cargo_deny_linux_x86_64"
    cargo_nextest_repo="cargo_nextest_linux_x86_64"
    wasm_pack_repo="wasm_pack_linux_x86_64"
    ;;
  *)
    echo "unsupported BuildBuddy full lane host: $(uname -s)-$(uname -m)" >&2
    exit 127
    ;;
esac

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

prepend_path() {
  local dir="$1"
  if [[ -n "${dir}" ]]; then
    PATH="${dir}:${PATH:-/usr/bin:/bin:/usr/sbin:/sbin}"
  fi
}

append_rust_cfg() {
  local cfg="$1"
  if [[ -n "${CARGO_ENCODED_RUSTFLAGS:-}" ]]; then
    local unit_separator
    unit_separator=$'\x1f'
    case "${CARGO_ENCODED_RUSTFLAGS}" in
      *"${cfg}"*) ;;
      *)
        export CARGO_ENCODED_RUSTFLAGS="${CARGO_ENCODED_RUSTFLAGS}${unit_separator}--cfg${unit_separator}${cfg}"
        ;;
    esac
  else
    case " ${RUSTFLAGS:-} " in
      *" --cfg ${cfg} "* | *" --cfg=${cfg} "*) ;;
      *) export RUSTFLAGS="${RUSTFLAGS:-} --cfg ${cfg}" ;;
    esac
  fi
}

configure_rust() {
  local toolchain="$1"
  local cargo_bin rustc_bin rustdoc_bin rustfmt_bin rust_objcopy_bin rust_lld_bin
  cargo_bin="$(find_rust_tool "${toolchain}" cargo)"
  rustc_bin="$(find_rust_tool "${toolchain}" rustc)"
  rustdoc_bin="$(find_rust_tool "${toolchain}" rustdoc)"
  rustfmt_bin="$(find_rust_tool "${toolchain}" rustfmt)"
  rust_objcopy_bin="$(find_rust_tool "${toolchain}" rust-objcopy)"
  rust_lld_bin="$(find_rust_tool "${toolchain}" rust-lld)"
  if [[ -z "${cargo_bin}" || -z "${rustc_bin}" ]]; then
    echo "rules_rust cargo/rustc runfiles were not found for ${toolchain}" >&2
    exit 127
  fi

  export CARGO="${cargo_bin}"
  export RUSTC="${rustc_bin}"
  if [[ -n "${rustdoc_bin}" ]]; then
    export RUSTDOC="${rustdoc_bin}"
  fi
  if [[ -n "${rustfmt_bin}" ]]; then
    export RUSTFMT="${rustfmt_bin}"
  fi
  prepend_path "$(dirname "${cargo_bin}")"
  if [[ -n "${rust_objcopy_bin}" ]]; then
    prepend_path "$(dirname "${rust_objcopy_bin}")"
  fi
  if [[ -n "${rust_lld_bin}" ]]; then
    prepend_path "$(dirname "${rust_lld_bin}")"
  fi

  export CARGO_HOME="${MEERKAT_HOST_CARGO_HOME:-${TEST_TMPDIR}/cargo-home}"
  export CARGO_TARGET_DIR="${TEST_TMPDIR}/cargo-target"
  export CARGO_INCREMENTAL=0
  export CARGO_TERM_COLOR=always
}

configure_rust_with_wasm_target() {
  local host_cargo_bin wasm_std_dir toolchain_root sandbox_toolchain
  host_cargo_bin="$(find_rust_tool "${host_rust_toolchain}" cargo)"
  wasm_std_dir="$(find "${TEST_SRCDIR}" -path "*wasm32-unknown-unknown__stable_tools*/lib/rustlib/wasm32-unknown-unknown" -type d | head -1)"
  if [[ -z "${host_cargo_bin}" || -z "${wasm_std_dir}" ]]; then
    echo "host Rust toolchain or wasm32 std runfiles were not found" >&2
    exit 127
  fi

  toolchain_root="$(cd "$(dirname "${host_cargo_bin}")/.." && pwd -P)"
  sandbox_toolchain="${TEST_TMPDIR}/rust-toolchain-host-plus-wasm"
  rm -rf "${sandbox_toolchain}"
  mkdir -p "${sandbox_toolchain}"
  cp -a "${toolchain_root}/." "${sandbox_toolchain}/"
  rm -rf "${sandbox_toolchain}/lib/rustlib/wasm32-unknown-unknown"
  mkdir -p "${sandbox_toolchain}/lib/rustlib"
  cp -a "${wasm_std_dir}" "${sandbox_toolchain}/lib/rustlib/"

  export CARGO="${sandbox_toolchain}/bin/cargo"
  export RUSTC="${sandbox_toolchain}/bin/rustc"
  export RUSTDOC="${sandbox_toolchain}/bin/rustdoc"
  export RUSTFMT="${sandbox_toolchain}/bin/rustfmt"
  # `cargo clippy` resolves the external `cargo-clippy` subcommand from
  # $CARGO_HOME/bin before PATH, and CARGO_HOME is the executor image's
  # /usr/local/cargo, whose rustup proxy runs the image toolchain. That mixed a
  # 1.94.1 clippy-driver into a 1.94.0 build (E0514 "compiled by an
  # incompatible version of rustc") on the 2026-09-22 wasm-check lane. Name the
  # sandbox binary and fail closed if the toolchain does not ship it.
  export CARGO_CLIPPY="${sandbox_toolchain}/bin/cargo-clippy"
  if [[ ! -x "${CARGO_CLIPPY}" || ! -x "${sandbox_toolchain}/bin/clippy-driver" ]]; then
    echo "rules_rust host toolchain runfiles lack cargo-clippy/clippy-driver at ${sandbox_toolchain}/bin" >&2
    exit 127
  fi
  prepend_path "${sandbox_toolchain}/bin"
  prepend_path "${sandbox_toolchain}/lib/rustlib/${host_triple}/bin"
  export CARGO_HOME="${MEERKAT_HOST_CARGO_HOME:-${TEST_TMPDIR}/cargo-home}"
  export CARGO_TARGET_DIR="${TEST_TMPDIR}/cargo-target"
  export CARGO_INCREMENTAL=0
  export CARGO_TERM_COLOR=always
}

configure_node() {
  local node_bin npm_cli wrapper_dir
  node_bin="$(find_runfile "*${node_repo}/bin/node")"
  npm_cli="$(find_runfile "*${node_repo}/lib/node_modules/npm/bin/npm-cli.js")"
  if [[ -z "${node_bin}" || -z "${npm_cli}" ]]; then
    echo "pinned Node/npm runfiles were not found" >&2
    exit 127
  fi
  wrapper_dir="${TEST_TMPDIR}/node-wrapper"
  mkdir -p "${wrapper_dir}"
  printf '#!/usr/bin/env bash\nexec %q %q "$@"\n' "${node_bin}" "${npm_cli}" >"${wrapper_dir}/npm"
  chmod +x "${wrapper_dir}/npm"
  prepend_path "$(dirname "${node_bin}")"
  prepend_path "${wrapper_dir}"
  export NODE="${node_bin}"
  export NPM_CONFIG_CACHE="${TEST_TMPDIR}/npm-cache"
}

configure_python() {
  local python_bin
  python_bin="$(find_runfile "*${python_repo}/python/bin/python3.14")"
  if [[ -z "${python_bin}" ]]; then
    echo "pinned Python 3.14 runfiles were not found" >&2
    exit 127
  fi
  prepend_path "$(dirname "${python_bin}")"
  export PYTHON="${python_bin}"
  export PIP_CACHE_DIR="${TEST_TMPDIR}/pip-cache"
}

configure_cargo_deny() {
  local cargo_deny_bin
  cargo_deny_bin="$(find_runfile "*${cargo_deny_repo}/cargo-deny")"
  if [[ -z "${cargo_deny_bin}" ]]; then
    echo "pinned cargo-deny runfile was not found" >&2
    exit 127
  fi
  export CARGO_DENY="${cargo_deny_bin}"
}

configure_cargo_nextest() {
  local cargo_nextest_bin
  cargo_nextest_bin="$(find_runfile "*${cargo_nextest_repo}/cargo-nextest")"
  if [[ -z "${cargo_nextest_bin}" ]]; then
    echo "pinned cargo-nextest runfile was not found" >&2
    exit 127
  fi
  # Invoked directly (`cargo-nextest nextest run ...`), never through `cargo
  # nextest`: Cargo resolves external subcommands from $CARGO_HOME/bin first,
  # and CARGO_HOME here is the executor image's rustup home.
  export CARGO_NEXTEST="${cargo_nextest_bin}"
}

configure_wasm_pack() {
  local wasm_pack_bin
  wasm_pack_bin="$(find_runfile "*${wasm_pack_repo}/wasm-pack")"
  if [[ -z "${wasm_pack_bin}" ]]; then
    echo "pinned wasm-pack runfile was not found" >&2
    exit 127
  fi
  export WASM_PACK="${wasm_pack_bin}"
}

run_parallel_job() {
  local name="$1"
  shift
  local log_file="${TEST_TMPDIR}/${name}.log"
  (
    set -euo pipefail
    "$@"
  ) >"${log_file}" 2>&1 &
  echo "$! ${name} ${log_file}" >>"${parallel_jobs_file}"
}

wait_parallel_jobs() {
  local failed=0
  local pid name log_file
  while read -r pid name log_file; do
    if wait "${pid}"; then
      echo "PASS ${name}"
      continue
    fi
    failed=1
    echo "FAIL ${name}; log follows:" >&2
    # Keep the head (tool versions, install output) and always the tail: the
    # failing compiler error or test summary of a long build sits at the end,
    # and the 09-16 web-sdk failure was undiagnosable from the head alone.
    local total_lines
    total_lines="$(wc -l <"${log_file}" || echo 0)"
    if ((total_lines <= 340)); then
      cat "${log_file}" >&2 || true
    else
      sed -n '1,220p' "${log_file}" >&2 || true
      echo "... (${total_lines} lines total; last 120 follow)" >&2
      tail -n 120 "${log_file}" >&2 || true
    fi
  done <"${parallel_jobs_file}"
  return "${failed}"
}

copy_workspace
cd "${work_root}"
# Parity with scripts/repo-cargo: source-level tests resolve the workspace
# from this variable instead of the crate directory Cargo runs them from.
export MEERKAT_WORKSPACE_ROOT="${work_root}"

run_wasm_contract_test() {
  local test_name="$1"
  local runner="$2"
  "${CARGO}" test -p meerkat-web-runtime --target wasm32-unknown-unknown --test "${test_name}" --no-run
  case "${runner}" in
    chrome)
      "${WASM_PACK}" test --headless --chrome meerkat-web-runtime --test "${test_name}"
      ;;
    node)
      "${WASM_PACK}" test --node meerkat-web-runtime --test "${test_name}"
      ;;
    *)
      echo "unknown wasm contract runner: ${runner}" >&2
      exit 2
      ;;
  esac
}

case "${lane}" in
  security-audit)
    configure_rust "${host_rust_toolchain}"
    configure_cargo_deny
    "${CARGO_DENY}" check
    ;;
  # The Native unit and integration-fast lanes: the same nextest invocations
  # scripts/pre-push-unit.sh runs (default features, kind(lib) for units,
  # profile fast + kind(test) for integration), one process per test, with
  # the hook's RUST_MIN_STACK. The workspace root and the `fast` profile in
  # .config/nextest.toml come with the copied workspace.
  test-unit)
    configure_rust "${host_rust_toolchain}"
    configure_cargo_nextest
    export RUST_MIN_STACK="${RUST_MIN_STACK:-33554432}"
    "${CARGO_NEXTEST}" nextest run --workspace \
      -E 'kind(lib)' --no-tests=fail --no-fail-fast \
      --show-progress none --status-level none --final-status-level fail
    ;;
  integration-fast)
    configure_rust "${host_rust_toolchain}"
    configure_cargo_nextest
    export RUST_MIN_STACK="${RUST_MIN_STACK:-33554432}"
    "${CARGO_NEXTEST}" nextest run --workspace \
      --profile fast -E 'kind(test)' --no-tests=fail --no-fail-fast \
      --show-progress none --status-level none --final-status-level fail
    ;;
  release-validate)
    configure_rust "${host_rust_toolchain}"
    configure_node
    configure_python
    "${CARGO}" build -p meerkat-rpc
    # The release workflow runs packaged-crate validation after this BB lane so
    # the registry-layout smoke stays outside the remote Bazel test sandbox.
    parallel_jobs_file="${TEST_TMPDIR}/release-validate-jobs.tsv"
    : >"${parallel_jobs_file}"
    run_parallel_job release-contracts bash -lc '
      export CARGO_TARGET_DIR="${TEST_TMPDIR}/cargo-target-release-contracts"
      make verify-version-parity CARGO="${CARGO}"
      make verify-rpc-surface-alignment CARGO="${CARGO}"
      make verify-sdk-wrapper-freshness CARGO="${CARGO}" PYTHON="${PYTHON}"
      make verify-schema-freshness CARGO="${CARGO}"
    '
    run_parallel_job python-sdk bash -lc '
      cd sdks/python
      "${PYTHON}" -m pip install --upgrade pip
      "${PYTHON}" -m pip install -e ".[dev]"
      "${PYTHON}" -m pytest -q tests
    '
    run_parallel_job typescript-sdk bash -lc '
      cd sdks/typescript
      npm install --ignore-scripts
      npm run build
      npm test
    '
    wait_parallel_jobs
    ;;
  sdk-suites)
    configure_rust_with_wasm_target
    configure_node
    configure_python
    configure_wasm_pack
    "${CARGO}" build -p meerkat-rpc
    parallel_jobs_file="${TEST_TMPDIR}/sdk-suites-jobs.tsv"
    : >"${parallel_jobs_file}"
    run_parallel_job python-sdk bash -lc '
      export PIP_CACHE_DIR="${TEST_TMPDIR}/pip-cache-python-sdk"
      cd sdks/python &&
      "${PYTHON}" -m pip install --upgrade pip &&
      "${PYTHON}" -m pip install -e ".[dev]" &&
      "${PYTHON}" -m pytest -q tests
    '
    run_parallel_job typescript-sdk bash -lc '
      export NPM_CONFIG_CACHE="${TEST_TMPDIR}/npm-cache-typescript-sdk"
      cd sdks/typescript &&
      npm install --ignore-scripts &&
      npm run build &&
      npm test
    '
    run_parallel_job web-sdk bash -lc '
      export NPM_CONFIG_CACHE="${TEST_TMPDIR}/npm-cache-web-sdk"
      cd sdks/web &&
      npm install --ignore-scripts &&
      npm run build &&
      npm test
    '
    wait_parallel_jobs
    make verify-sdk-wrapper-freshness CARGO="${CARGO}" PYTHON="${PYTHON}"
    ;;
  # The three suites below are the `sdk-suites` lane split one suite per
  # remote action (see SDK_SUITE_CARGO_EQUIVALENT_TESTS in BUILD.bazel). Each
  # keeps exactly the steps the combined lane ran for that suite: the Python
  # and TypeScript suites resolve the rkat-rpc binary, so they build it; the
  # Web suite builds the wasm bundle and never spawns rkat-rpc; the wrapper
  # freshness check needs cargo and python and rides with the Python suite.
  sdk-python)
    configure_rust "${host_rust_toolchain}"
    configure_python
    "${CARGO}" build -p meerkat-rpc
    (
      cd sdks/python &&
      "${PYTHON}" -m pip install --upgrade pip &&
      "${PYTHON}" -m pip install -e ".[dev]" &&
      "${PYTHON}" -m pytest -q tests
    )
    make verify-sdk-wrapper-freshness CARGO="${CARGO}" PYTHON="${PYTHON}"
    ;;
  sdk-typescript)
    configure_rust "${host_rust_toolchain}"
    configure_node
    "${CARGO}" build -p meerkat-rpc
    (
      cd sdks/typescript &&
      npm install --ignore-scripts &&
      npm run build &&
      npm test
    )
    ;;
  sdk-web)
    configure_rust_with_wasm_target
    configure_node
    configure_wasm_pack
    (
      cd sdks/web &&
      npm install --ignore-scripts &&
      npm run build &&
      npm test
    )
    ;;
  wasm-check)
    configure_rust_with_wasm_target
    append_rust_cfg 'getrandom_backend="wasm_js"'
    "${CARGO}" check -p meerkat-web-runtime --target wasm32-unknown-unknown --all-targets
    "${CARGO_CLIPPY}" clippy -p meerkat-web-runtime --target wasm32-unknown-unknown --all-targets -- -D warnings
    ;;
  wasm-contract-tests)
    configure_rust_with_wasm_target
    configure_node
    configure_wasm_pack
    append_rust_cfg 'getrandom_backend="wasm_js"'
    run_wasm_contract_test browser_contract chrome
    run_wasm_contract_test release_targets chrome
    run_wasm_contract_test wasm_external_resolver node
    ;;
  wasm-contract-browser-contract)
    configure_rust_with_wasm_target
    configure_node
    configure_wasm_pack
    append_rust_cfg 'getrandom_backend="wasm_js"'
    run_wasm_contract_test browser_contract chrome
    ;;
  wasm-contract-release-targets)
    configure_rust_with_wasm_target
    configure_node
    configure_wasm_pack
    append_rust_cfg 'getrandom_backend="wasm_js"'
    run_wasm_contract_test release_targets chrome
    ;;
  wasm-contract-external-resolver)
    configure_rust_with_wasm_target
    configure_node
    configure_wasm_pack
    append_rust_cfg 'getrandom_backend="wasm_js"'
    run_wasm_contract_test wasm_external_resolver node
    ;;
  *)
    echo "unknown full BuildBuddy lane: ${lane}" >&2
    exit 2
    ;;
esac
