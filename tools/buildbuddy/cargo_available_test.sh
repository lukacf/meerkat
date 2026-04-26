#!/usr/bin/env bash
set -euo pipefail

cargo_bin="${CARGO:-}"
rustc_bin="${RUSTC:-}"

if [[ -z "${cargo_bin}" ]]; then
  cargo_bin="$(find "${TEST_SRCDIR:?}" -path '*rust_macos_aarch64__aarch64-apple-darwin__stable_tools*/bin/cargo' \( -type f -o -type l \) | head -1)"
fi
if [[ -z "${rustc_bin}" ]]; then
  rustc_bin="$(find "${TEST_SRCDIR:?}" -path '*rust_macos_aarch64__aarch64-apple-darwin__stable_tools*/bin/rustc' \( -type f -o -type l \) | head -1)"
fi

"${cargo_bin}" --version
"${rustc_bin}" --version
python3 --version
