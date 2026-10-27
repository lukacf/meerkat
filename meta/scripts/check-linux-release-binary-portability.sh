#!/usr/bin/env bash
# check-linux-release-binary-portability.sh - fail closed when a packaged
# Linux GNU gateway binary would not load on the oldest supported distro.
#
# The floor comes from the release build environment: build_binaries in
# .github/workflows/release.yml builds the *-unknown-linux-gnu legs inside
# docker.io/library/buildpack-deps:bullseye (glibc 2.31), the same image
# meerkat's release lane uses, so the two products ship one Linux floor.
# Before that container existed the legs built directly on ubuntu-latest
# (Ubuntu 24.04, glibc 2.39) and nothing read the result: the v0.8.30
# rpc_gateway and meta_gateway archives carried a hard GLIBC_2.39 version
# requirement (pidfd_spawnp/pidfd_getpid) and the loader refused them on
# Debian bookworm (2.36), Ubuntu 22.04 (2.35) and bullseye (2.31). This gate
# exists so a container or runner change that raises the floor fails the
# release before anything is packaged. Keep META_GLIBC_FLOOR in sync with
# that image; scripts/test_release_workflow.py asserts the pair agrees.
#
# Checks per binary:
#   1. No versioned glibc symbol reference newer than the declared floor.
#   2. No dynamic dependency on OpenSSL (libssl.so*/libcrypto.so*): the
#      gateway's TLS is rustls (meerkat-meta pins reqwest with rustls-tls
#      and no default features), so an OpenSSL NEEDED entry means a
#      dependency regressed into a native-tls stack. meerkat v0.8.21 shipped
#      exactly that through oai-rt-rs, and Meta links the same stack.
#
# Usage: check-linux-release-binary-portability.sh <binary> [<binary>...]

set -euo pipefail

META_GLIBC_FLOOR="${META_GLIBC_FLOOR:-2.31}"

if [[ "$#" -lt 1 ]]; then
  echo "usage: $0 <binary> [<binary>...]" >&2
  exit 2
fi

meta_readelf_bin=""
for meta_readelf_candidate in readelf llvm-readelf; do
  if command -v "${meta_readelf_candidate}" >/dev/null 2>&1; then
    meta_readelf_bin="${meta_readelf_candidate}"
    break
  fi
done
if [[ -z "${meta_readelf_bin}" ]]; then
  echo "check-linux-release-binary-portability: neither readelf nor llvm-readelf is available" >&2
  exit 2
fi

meta_portability_failed=0

for meta_release_binary in "$@"; do
  if [[ ! -f "${meta_release_binary}" ]]; then
    echo "FAIL ${meta_release_binary}: file not found" >&2
    meta_portability_failed=1
    continue
  fi

  meta_binary_failed=0

  # Gate 1: glibc symbol-version floor. Version sort, not lexical: GLIBC_2.4
  # (__stack_chk_fail, present in nearly every binary) is below 2.31.
  meta_max_glibc="$("${meta_readelf_bin}" --dyn-syms --wide "${meta_release_binary}" \
    | grep -o 'GLIBC_[0-9][0-9.]*' | sed 's/^GLIBC_//' | sort -uV | tail -1 || true)"
  if [[ -n "${meta_max_glibc}" ]]; then
    meta_newest_of_pair="$(printf '%s\n%s\n' "${meta_max_glibc}" "${META_GLIBC_FLOOR}" | sort -V | tail -1)"
    if [[ "${meta_newest_of_pair}" != "${META_GLIBC_FLOOR}" ]]; then
      echo "FAIL ${meta_release_binary}: references GLIBC_${meta_max_glibc}, above the declared floor GLIBC_${META_GLIBC_FLOOR}" >&2
      meta_binary_failed=1
    fi
  fi

  # Gate 2: no dynamic OpenSSL.
  meta_openssl_needed="$("${meta_readelf_bin}" -d "${meta_release_binary}" \
    | grep -E 'NEEDED.*\[(libssl|libcrypto)\.so' || true)"
  if [[ -n "${meta_openssl_needed}" ]]; then
    echo "FAIL ${meta_release_binary}: dynamically links OpenSSL:" >&2
    echo "${meta_openssl_needed}" >&2
    meta_binary_failed=1
  fi

  if [[ "${meta_binary_failed}" -eq 0 ]]; then
    echo "PASS ${meta_release_binary}: max glibc ref GLIBC_${meta_max_glibc:-none}, floor GLIBC_${META_GLIBC_FLOOR}, no OpenSSL dynamic deps"
  else
    meta_portability_failed=1
  fi
done

exit "${meta_portability_failed}"
