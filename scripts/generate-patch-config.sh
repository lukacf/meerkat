#!/usr/bin/env bash
# Generates [patch.crates-io] config for dry-run and CI publishing.
#
# The crate set is DERIVED from scripts/release-rust-crates.sh (which
# check_rust_release_packaging.py already gates against the publishable
# workspace members) and the directory for each crate is read from the
# manifests themselves. A hand-maintained copy of the crate list used to live
# here, so adding one workspace member silently produced a patch config missing
# that crate and the omission only surfaced during package verification.
#
# Usage: generate-patch-config.sh [ROOT_DIR] [EXCLUDE_CRATE]
#
# EXCLUDE_CRATE: omit this crate from the patch list (avoids lockfile
# collision when cargo publish verifies the package being published).

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
ROOT="${1:-$(cd "${SCRIPT_DIR}/.." && pwd)}"
EXCLUDE="${2:-}"

# name<TAB>directory for every workspace member manifest (crates/*, tests/*). Only the [package]
# section counts: several crates declare [[bin]] targets whose names collide
# with other crates' package names.
crate_table="$(
  for manifest in "${ROOT}"/*/Cargo.toml "${ROOT}"/crates/*/Cargo.toml "${ROOT}"/tests/integration/Cargo.toml "${ROOT}"/tests/fixtures/*/Cargo.toml; do
    [[ -f "${manifest}" ]] || continue
    awk -v dir="$(dirname "${manifest}")" '
      /^[[:space:]]*\[/ { in_package = ($0 ~ /^[[:space:]]*\[package\][[:space:]]*$/) }
      in_package && /^[[:space:]]*name[[:space:]]*=/ {
        if (match($0, /"[^"]*"/)) {
          printf "%s\t%s\n", substr($0, RSTART + 1, RLENGTH - 2), dir
          exit
        }
      }
    ' "${manifest}"
  done
)"

echo "[patch.crates-io]"
while IFS= read -r crate; do
  [[ -n "${crate}" ]] || continue
  crate_dir="$(awk -F '\t' -v want="${crate}" '$1 == want { print $2; exit }' <<<"${crate_table}")"
  if [[ -z "${crate_dir}" ]]; then
    echo "generate-patch-config.sh: no workspace directory under ${ROOT} declares package '${crate}'" >&2
    echo "Release crate list and workspace members disagree; run: make check-rust-release-config" >&2
    exit 1
  fi
  if [[ "${crate}" != "${EXCLUDE}" ]]; then
    echo "${crate} = { path = \"${crate_dir}\" }"
  fi
done < <("${SCRIPT_DIR}/release-rust-crates.sh")
