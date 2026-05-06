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

copy_workspace() {
  rm -rf "${work_root}"
  mkdir -p "${work_root}"
  rsync -aL \
    --exclude '.git' \
    --exclude 'bazel-*' \
    --exclude 'target' \
    --exclude 'target-*' \
    "${runfiles_root}/" "${work_root}/"
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

  export CARGO_HOME="${TEST_TMPDIR}/cargo-home"
  export CARGO_TARGET_DIR="${TEST_TMPDIR}/cargo-target"
  export CARGO_INCREMENTAL=0
  export CARGO_TERM_COLOR=always
}

configure_rust_with_wasm_target() {
  local host_cargo_bin wasm_std_dir toolchain_root sandbox_toolchain
  host_cargo_bin="$(find_rust_tool "aarch64-apple-darwin__stable_tools" cargo)"
  wasm_std_dir="$(find "${TEST_SRCDIR}" -path "*wasm32-unknown-unknown__stable_tools*/lib/rustlib/wasm32-unknown-unknown" -type d | head -1)"
  if [[ -z "${host_cargo_bin}" || -z "${wasm_std_dir}" ]]; then
    echo "host Rust toolchain or wasm32 std runfiles were not found" >&2
    exit 127
  fi

  toolchain_root="$(cd "$(dirname "${host_cargo_bin}")/.." && pwd -P)"
  sandbox_toolchain="${TEST_TMPDIR}/rust-toolchain-host-plus-wasm"
  rm -rf "${sandbox_toolchain}"
  mkdir -p "${sandbox_toolchain}"
  rsync -aL "${toolchain_root}/" "${sandbox_toolchain}/"
  rm -rf "${sandbox_toolchain}/lib/rustlib/wasm32-unknown-unknown"
  mkdir -p "${sandbox_toolchain}/lib/rustlib"
  rsync -aL "${wasm_std_dir}" "${sandbox_toolchain}/lib/rustlib/"

  export CARGO="${sandbox_toolchain}/bin/cargo"
  export RUSTC="${sandbox_toolchain}/bin/rustc"
  export RUSTDOC="${sandbox_toolchain}/bin/rustdoc"
  export RUSTFMT="${sandbox_toolchain}/bin/rustfmt"
  prepend_path "${sandbox_toolchain}/bin"
  prepend_path "${sandbox_toolchain}/lib/rustlib/aarch64-apple-darwin/bin"
  export CARGO_HOME="${TEST_TMPDIR}/cargo-home"
  export CARGO_TARGET_DIR="${TEST_TMPDIR}/cargo-target"
  export CARGO_INCREMENTAL=0
  export CARGO_TERM_COLOR=always
}

configure_node() {
  local node_bin npm_cli wrapper_dir
  node_bin="$(find_runfile "*node_darwin_arm64/bin/node")"
  npm_cli="$(find_runfile "*node_darwin_arm64/lib/node_modules/npm/bin/npm-cli.js")"
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
  python_bin="$(find_runfile "*python_darwin_arm64/python/bin/python3.14")"
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
  cargo_deny_bin="$(find_runfile "*cargo_deny_darwin_arm64/cargo-deny")"
  if [[ -z "${cargo_deny_bin}" ]]; then
    echo "pinned cargo-deny runfile was not found" >&2
    exit 127
  fi
  export CARGO_DENY="${cargo_deny_bin}"
}

configure_wasm_pack() {
  local wasm_pack_bin
  wasm_pack_bin="$(find_runfile "*wasm_pack_darwin_arm64/wasm-pack")"
  if [[ -z "${wasm_pack_bin}" ]]; then
    echo "pinned wasm-pack runfile was not found" >&2
    exit 127
  fi
  export WASM_PACK="${wasm_pack_bin}"
}

copy_workspace
cd "${work_root}"

case "${lane}" in
  security-audit)
    configure_rust "aarch64-apple-darwin__stable_tools"
    configure_cargo_deny
    "${CARGO_DENY}" check
    ;;
  release-validate)
    configure_rust "aarch64-apple-darwin__stable_tools"
    configure_node
    configure_python
    "${CARGO}" build -p meerkat-rpc
    make verify-version-parity CARGO="${CARGO}"
    make verify-rpc-surface-alignment CARGO="${CARGO}"
    make verify-sdk-wrapper-freshness CARGO="${CARGO}" PYTHON="${PYTHON}"
    (cd sdks/python &&
      "${PYTHON}" -m pip install --upgrade pip &&
      "${PYTHON}" -m pip install -e ".[dev]" &&
      "${PYTHON}" -m pytest -q tests)
    (cd sdks/typescript &&
      npm install --ignore-scripts &&
      npm run build &&
      npm test)
    make verify-schema-freshness CARGO="${CARGO}"
    make check-rust-release-packaging CARGO="${CARGO}"
    ;;
  sdk-suites)
    configure_rust "aarch64-apple-darwin__stable_tools"
    configure_node
    configure_python
    "${CARGO}" build -p meerkat-rpc
    (cd sdks/python &&
      "${PYTHON}" -m pip install --upgrade pip &&
      "${PYTHON}" -m pip install -e ".[dev]" &&
      "${PYTHON}" -m pytest -q tests)
    (cd sdks/typescript &&
      npm install --ignore-scripts &&
      npm run build &&
      npm test)
    make verify-sdk-wrapper-freshness CARGO="${CARGO}" PYTHON="${PYTHON}"
    ;;
  wasm-check)
    configure_rust_with_wasm_target
    append_rust_cfg 'getrandom_backend="wasm_js"'
    "${CARGO}" check -p meerkat-web-runtime --target wasm32-unknown-unknown
    "${CARGO}" clippy -p meerkat-web-runtime --target wasm32-unknown-unknown -- -D warnings
    ;;
  wasm-contract-tests)
    configure_rust_with_wasm_target
    configure_node
    configure_wasm_pack
    append_rust_cfg 'getrandom_backend="wasm_js"'
    "${CARGO}" test -p meerkat-web-runtime --target wasm32-unknown-unknown --test browser_contract --no-run
    "${CARGO}" test -p meerkat-web-runtime --target wasm32-unknown-unknown --test release_targets --no-run
    "${CARGO}" test -p meerkat-web-runtime --target wasm32-unknown-unknown --test wasm_external_resolver --no-run
    "${WASM_PACK}" test --headless --chrome meerkat-web-runtime --test browser_contract
    "${WASM_PACK}" test --headless --chrome meerkat-web-runtime --test release_targets
    "${WASM_PACK}" test --node meerkat-web-runtime --test wasm_external_resolver
    ;;
  *)
    echo "unknown full BuildBuddy lane: ${lane}" >&2
    exit 2
    ;;
esac
