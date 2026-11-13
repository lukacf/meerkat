#!/usr/bin/env bash
# Put ripgrep (`rg`) on PATH for a CI job without depending on the Ubuntu
# package mirror:
#
#   scripts/install-ripgrep.sh [--dir DIR]
#
# - `rg` is already on PATH: print its version and exit 0. Nothing is fetched.
# - Otherwise install the ripgrep release pinned below, the static musl build
#   for this machine (Linux x86_64 or aarch64), from the ripgrep GitHub
#   release. The archive comes through the GitHub API with the job's own
#   token first, when the step passes one as `GH_TOKEN: ${{ github.token }}`
#   (scripts/ci-release-asset-download), and from the anonymous release URL,
#   with bounded retries, when there is no token or that fails. The token is
#   unset before anything else runs. The archive is unpacked only once its
#   sha256 matches the pin, whichever way it came. `rg` lands in DIR/bin (DIR
#   defaults to $RUNNER_TEMP/ripgrep-install), DIR/bin is appended to
#   $GITHUB_PATH when that is set, and `rg --version` must run.
# - Only when that download fails or its sha256 does not match: the Ubuntu
#   package (`apt-get update`, `apt-get install -y ripgrep`), the one source
#   these jobs had before. An archive that does not match is deleted, never
#   unpacked or run, and the warning that announces the fallback names both
#   digests.
# - A machine with no pinned build (any other system or architecture) is an
#   error: nothing is downloaded and nothing is installed.
#
# The last line of output names the source that was used:
#   ripgrep source: path | release (api) | release (url) | apt
#
# On 2026-11-11 the hosted runners could not resolve the Ubuntu package
# mirror, and the apt-only install failed the required Format + governance
# job on main and on pull requests. With two independent sources the job
# stops only when both are unavailable.
#
# A new RIPGREP_VERSION changes both sha256 values with it: the digests of
# the two release archives, which the release also publishes beside each
# archive as <archive>.sha256.
set -euo pipefail

RIPGREP_VERSION="15.2.0"
RIPGREP_SHA256_X86_64="33e15bcf1624b25cdd2a55813a47a2f95dbe126268203e76aa6a585d1e7b149c"
RIPGREP_SHA256_AARCH64="800b1e7206afe799dfb5a6901f23147cfaabe0e52210538100f61e86e1740915"
RIPGREP_REPO="BurntSushi/ripgrep"

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
install_dir=""

usage() {
  echo "usage: install-ripgrep.sh [--dir DIR]"
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dir)
      if [[ $# -lt 2 || -z "$2" ]]; then
        usage >&2
        exit 2
      fi
      install_dir="$2"
      shift 2
      ;;
    --help | -h)
      usage
      exit 0
      ;;
    *)
      echo "install-ripgrep: unknown argument: $1" >&2
      usage >&2
      exit 2
      ;;
  esac
done

if command -v rg > /dev/null 2>&1; then
  echo "install-ripgrep: rg is already on PATH ($(command -v rg)); nothing to install"
  rg --version
  echo "ripgrep source: path"
  exit 0
fi

os="$(uname -s)"
arch="$(uname -m)"
case "${os}/${arch}" in
  Linux/x86_64 | Linux/amd64)
    triple="x86_64-unknown-linux-musl"
    sha256="${RIPGREP_SHA256_X86_64}"
    ;;
  Linux/aarch64 | Linux/arm64)
    triple="aarch64-unknown-linux-musl"
    sha256="${RIPGREP_SHA256_AARCH64}"
    ;;
  *)
    echo "::error::install-ripgrep: no pinned ripgrep ${RIPGREP_VERSION} build for ${os}/${arch} (pinned: Linux x86_64 and Linux aarch64); install ripgrep on this machine so that rg is on PATH" >&2
    exit 1
    ;;
esac

name="ripgrep-${RIPGREP_VERSION}-${triple}"
asset="${name}.tar.gz"
url="https://github.com/${RIPGREP_REPO}/releases/download/${RIPGREP_VERSION}/${asset}"

# The second source. It runs only after the release archive was not used, and
# says why first; a failure here fails the step.
install_from_apt() {
  local reason="$1"
  # A workflow command reads %XX as an escape.
  reason="${reason//%/%25}"
  echo "::warning::ripgrep ${RIPGREP_VERSION} was not installed from its GitHub release (${reason}); falling back to the Ubuntu package (apt-get install ripgrep)"
  local sudo=(sudo)
  if [[ "$(id -u)" == 0 ]]; then sudo=(); fi
  "${sudo[@]}" apt-get update
  "${sudo[@]}" apt-get install -y ripgrep
  rg --version
  echo "ripgrep source: apt"
}

if [[ -z "${install_dir}" ]]; then
  install_dir="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/ripgrep-install"
fi
mkdir -p "${install_dir}"
install_dir="$(cd "${install_dir}" && pwd)"
archive="${install_dir}/${asset}"
rm -f -- "${archive}"

route=""
if bash "${script_dir}/ci-release-asset-download" \
  "${RIPGREP_REPO}" "${RIPGREP_VERSION}" "${asset}" "${archive}"; then
  route="api"
fi
unset GH_TOKEN
if [[ -z "${route}" ]]; then
  curl_status=0
  curl --proto '=https' --tlsv1.2 --fail --silent --show-error --location \
    --connect-timeout 20 --max-time 60 \
    --retry 5 --retry-delay 2 --retry-max-time 120 --retry-all-errors \
    -o "${archive}" "${url}" || curl_status=$?
  if ((curl_status != 0)); then
    rm -f -- "${archive}"
    install_from_apt "download failed: curl exit ${curl_status} for ${url}"
    exit 0
  fi
  route="url"
fi

actual="$(sha256sum "${archive}" | cut -d' ' -f1)"
if [[ "${actual}" != "${sha256}" ]]; then
  rm -f -- "${archive}"
  install_from_apt "sha256 mismatch for ${asset} (${route}): pinned ${sha256}, downloaded ${actual}; the archive was deleted and not unpacked"
  exit 0
fi

mkdir -p "${install_dir}/bin"
tar -xzf "${archive}" -C "${install_dir}/bin" --strip-components=1 "${name}/rg"
"${install_dir}/bin/rg" --version
if [[ -n "${GITHUB_PATH:-}" ]]; then
  echo "${install_dir}/bin" >> "${GITHUB_PATH}"
  echo "install-ripgrep: ${install_dir}/bin is on PATH for the steps after this one"
else
  echo "install-ripgrep: no GITHUB_PATH here; add ${install_dir}/bin to PATH to use it"
fi
echo "install-ripgrep: ripgrep ${RIPGREP_VERSION} (${triple}, sha256 ${sha256}) installed in ${install_dir}/bin"
echo "ripgrep source: release (${route})"
