#!/usr/bin/env bash
# check-linux-release-binary-portability.sh - fail closed when a packaged
# Linux GNU release binary would not load on the oldest supported distro.
#
# The floor comes from the release build environment: //tools/bazel/platforms:linux_x86_64
# and //tools/bazel/platforms:linux_arm64 in tools/bazel/platforms/BUILD.bazel pin
# buildpack-deps:bullseye (glibc 2.31), and the GitHub-hosted fallback lane in
# .github/workflows/release.yml builds inside the same image. This gate exists
# so a container or runner bump that silently raises the floor fails the
# release before publish (v0.8.21 shipped GLIBC_2.34 binaries that Debian
# Bullseye could not load). Keep MEERKAT_GLIBC_FLOOR in sync with those images.
#
# Checks per binary:
#   1. No versioned glibc symbol reference newer than the declared floor.
#   2. No dynamic dependency on OpenSSL (libssl.so*/libcrypto.so*): all
#      first-party TLS is rustls, so an OpenSSL NEEDED entry means a
#      dependency regressed into reqwest's default-tls/native-tls stack
#      (v0.8.21 shipped libssl.so.3 references via oai-rt-rs).
#
# Usage: check-linux-release-binary-portability.sh <binary> [<binary>...]

set -euo pipefail

MEERKAT_GLIBC_FLOOR="${MEERKAT_GLIBC_FLOOR:-2.31}"

if [[ "$#" -lt 1 ]]; then
  echo "usage: $0 <binary> [<binary>...]" >&2
  exit 2
fi

meerkat_readelf_bin=""
for meerkat_readelf_candidate in readelf llvm-readelf; do
  if command -v "${meerkat_readelf_candidate}" >/dev/null 2>&1; then
    meerkat_readelf_bin="${meerkat_readelf_candidate}"
    break
  fi
done
if [[ -z "${meerkat_readelf_bin}" ]]; then
  echo "check-linux-release-binary-portability: neither readelf nor llvm-readelf is available" >&2
  exit 2
fi

meerkat_portability_failed=0

for meerkat_release_binary in "$@"; do
  if [[ ! -f "${meerkat_release_binary}" ]]; then
    echo "FAIL ${meerkat_release_binary}: file not found" >&2
    meerkat_portability_failed=1
    continue
  fi

  meerkat_binary_failed=0

  # Gate 1: glibc symbol-version floor.
  meerkat_max_glibc="$("${meerkat_readelf_bin}" --dyn-syms --wide "${meerkat_release_binary}" \
    | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -uV | tail -1 || true)"
  if [[ -n "${meerkat_max_glibc}" ]]; then
    meerkat_newest_of_pair="$(printf '%s\n%s\n' "${meerkat_max_glibc}" "${MEERKAT_GLIBC_FLOOR}" | sort -V | tail -1)"
    if [[ "${meerkat_newest_of_pair}" != "${MEERKAT_GLIBC_FLOOR}" ]]; then
      echo "FAIL ${meerkat_release_binary}: references GLIBC_${meerkat_max_glibc}, above the declared floor GLIBC_${MEERKAT_GLIBC_FLOOR}" >&2
      meerkat_binary_failed=1
    fi
  fi

  # Gate 2: no dynamic OpenSSL.
  meerkat_openssl_needed="$("${meerkat_readelf_bin}" -d "${meerkat_release_binary}" \
    | grep -E 'NEEDED.*\[(libssl|libcrypto)\.so' || true)"
  if [[ -n "${meerkat_openssl_needed}" ]]; then
    echo "FAIL ${meerkat_release_binary}: dynamically links OpenSSL:" >&2
    echo "${meerkat_openssl_needed}" >&2
    meerkat_binary_failed=1
  fi

  if [[ "${meerkat_binary_failed}" -eq 0 ]]; then
    echo "PASS ${meerkat_release_binary}: max glibc ref GLIBC_${meerkat_max_glibc:-none}, floor GLIBC_${MEERKAT_GLIBC_FLOOR}, no OpenSSL dynamic deps"
  else
    meerkat_portability_failed=1
  fi
done

exit "${meerkat_portability_failed}"
