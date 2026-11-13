#!/usr/bin/env bash
# check-bazel-one-tokio.sh - every Bazel-built release binary, and the
# meerkat-live unit test, links exactly one tokio.
#
# Bazel resolves external crates in two crate_universe repositories: `crates`
# (from Cargo's lockfile) and `meerkat_live_crates` (the optional WebRTC
# stack: MODULE.bazel). Each resolves its own tokio. A process enters one
# tokio runtime; code linked against a second copy finds no runtime, and the
# WebRTC transport panicked at the first peer connection ("there is no
# reactor running"). MODULE.bazel therefore points the second repository's
# tokio at the first one's. This check holds the result: for each target
# below, `bazel query` of its Rust library closure names exactly one tokio
# library (a `...__tokio-<version>//:tokio` label). Two or more fail; so does
# none, which means the query did not see the graph it is meant to judge.
#
# Targets: the release roots (what the `release-build-linux-x86` case of
# scripts/buildbuddy-bazel-poc builds, read from that file) and
# //crates/meerkat-live:meerkat_live_unit_test, whose rows open real peers.
#
# Usage: check-bazel-one-tokio.sh [--root DIR] [--closures DIR] [bazel query arguments...]
#   BAZEL names the Bazel binary (default: bazel). Arguments are passed to
#   `bazel query` (a repository cache, for one). --root DIR checks another
#   checkout of the repository (default: the one this script is in).
#   --closures DIR reads `<n>.labels` files instead of running Bazel, one per
#   target in the order printed with --print-targets: the contract test's
#   seam.
# Exit 0 = one tokio everywhere, 1 = a target links another count, 2 = the
# inputs cannot be read.
set -euo pipefail

root="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)"
if [[ "${1-}" == "--root" ]]; then
  [[ $# -ge 2 && -d "$2" ]] || { echo "check-bazel-one-tokio: --root needs a directory" >&2; exit 2; }
  root="$(cd -- "$2" && pwd)"
  shift 2
fi
launcher="${root}/scripts/buildbuddy-bazel-poc"
lock="${root}/MODULE.bazel.lock"
test_target="//crates/meerkat-live:meerkat_live_unit_test"

roots_line="$(awk '
  /^  release-build-linux-x86\)$/ { inside = 1; next }
  inside && /^    default_target="/ { sub(/^    default_target="/, ""); sub(/"$/, ""); print; exit }
  inside && /^    ;;$/ { exit }
' "${launcher}" 2>/dev/null || true)"
if [[ -z "${roots_line}" ]]; then
  echo "check-bazel-one-tokio: cannot read the release roots of release-build-linux-x86 in scripts/buildbuddy-bazel-poc" >&2
  exit 2
fi
read -r -a targets <<<"${roots_line}"
targets+=("${test_target}")

closures=""
if [[ "${1-}" == "--print-targets" ]]; then
  printf '%s\n' "${targets[@]}"
  exit 0
fi
if [[ "${1-}" == "--closures" ]]; then
  [[ $# -ge 2 && -d "$2" ]] || { echo "check-bazel-one-tokio: --closures needs a directory" >&2; exit 2; }
  closures="$2"
  shift 2
fi

status=0
index=0
for target in "${targets[@]}"; do
  index=$((index + 1))
  if [[ -n "${closures}" ]]; then
    file="${closures}/${index}.labels"
    [[ -f "${file}" ]] || { echo "check-bazel-one-tokio: no closure file ${file} for ${target}" >&2; exit 2; }
    labels="$(cat "${file}")"
  else
    errors="$(mktemp)"
    if ! labels="$(cd -- "${root}" && "${BAZEL:-bazel}" query "kind('rust_library', deps(${target}))" --output=label "$@" 2>"${errors}")"; then
      echo "check-bazel-one-tokio: bazel query failed for ${target}:" >&2
      tail -40 "${errors}" >&2
      rm -f "${errors}"
      exit 2
    fi
    rm -f "${errors}"
  fi
  tokio="$(printf '%s\n' "${labels}" | grep -E '__tokio-[0-9][^/]*//:tokio$' | LC_ALL=C sort -u || true)"
  count="$(printf '%s' "${tokio}" | grep -c . || true)"
  if [[ "${count}" -eq 1 ]]; then
    echo "${target}: one tokio (${tokio})"
  elif [[ "${count}" -eq 0 ]]; then
    echo "${target} links no tokio library: the query did not see its Rust closure" >&2
    status=1
  else
    echo "${target} links ${count} tokio libraries; a process enters one runtime, and code linked against another copy finds none:" >&2
    printf '  %s\n' ${tokio} >&2
    echo "  point the other repository's tokio at the first (crate.annotation override_target_lib in MODULE.bazel)" >&2
    status=1
  fi
done

# The second repository's crates are compiled against the first one's tokio,
# whatever version their own resolution picked. A tokio feature they need and
# the first lacks fails their build. A different major version would not be
# the tokio they were resolved for at all: refuse that pair here, from the
# versions MODULE.bazel.lock renders for the two repositories.
majors="$(grep -oE '"(crates|meerkat_live_crates)__tokio-[0-9]+\.[0-9]+\.[0-9]+"' "${lock}" 2>/dev/null \
  | sed -E 's/^"(.*)__tokio-([0-9]+)\..*$/\1 \2/' | LC_ALL=C sort -u || true)"
if ! grep -q '^crates ' <<<"${majors}" || ! grep -q '^meerkat_live_crates ' <<<"${majors}"; then
  echo "check-bazel-one-tokio: MODULE.bazel.lock does not render tokio for both crate repositories" >&2
  exit 2
fi
if [[ "$(cut -d' ' -f2 <<<"${majors}" | LC_ALL=C sort -u | grep -c .)" -ne 1 ]]; then
  echo "the two crate repositories resolve tokio of different major versions (MODULE.bazel.lock):" >&2
  printf '  %s\n' "$(tr '\n' ';' <<<"${majors}")" >&2
  echo "  the live stack would be compiled against a tokio it was not resolved for" >&2
  status=1
fi
exit "${status}"
