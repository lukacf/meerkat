#!/usr/bin/env bash
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
out_dir="${1:-${root}/dist/web-sdk-package}"
version="${VERSION:-${RELEASE_TAG:-}}"

if [[ -n "${version}" ]]; then
  case "${version}" in
    v*) version="${version#v}" ;;
  esac
fi

cd "${root}/sdks/web"

package_name="$(node -p "const p = require('./package.json'); p.name")"
package_version="$(node -p "const p = require('./package.json'); p.version")"
package_spec="${package_name}@${package_version}"

if [[ -n "${version}" && "${package_version}" != "${version}" ]]; then
  echo "Web SDK package version ${package_version} does not match release version ${version}" >&2
  exit 1
fi

npm install --ignore-scripts

npm run build &
build_pid=$!
elapsed=0
while kill -0 "${build_pid}" 2>/dev/null; do
  sleep 5
  elapsed=$((elapsed + 5))
  if ((elapsed % 60 == 0)) && kill -0 "${build_pid}" 2>/dev/null; then
    echo "Web SDK package build still running..."
  fi
done
wait "${build_pid}"

rm -rf "${out_dir}"
mkdir -p "${out_dir}"

pack_output="$(npm pack --ignore-scripts)"
printf '%s\n' "${pack_output}"
packfile="$(printf '%s\n' "${pack_output}" | awk 'NF { line = $0 } END { print line }')"
if [[ -z "${packfile}" ]]; then
  echo "npm pack did not report an output tarball" >&2
  exit 1
fi
tarball="${out_dir}/${packfile}"
mv "${packfile}" "${tarball}"

tar -tzf "${tarball}" | grep -Fxq "package/wasm/meerkat_web_runtime_bg.wasm"
tar -tzf "${tarball}" | grep -Fxq "package/dist/index.js"
tar -tzf "${tarball}" | grep -Fxq "package/proxy/cli.mjs"

smoke_dir="$(mktemp -d)"
trap 'rm -rf "${smoke_dir}"' EXIT
(
  cd "${smoke_dir}"
  npm init -y >/dev/null 2>&1
  npm install "${tarball}" >/dev/null 2>&1
  node --input-type=module -e "const web = await import('@rkat/web'); if (!web.MeerkatRuntime || !web.Session) throw new Error('missing @rkat/web exports');"
)

echo "Built ${package_spec} package at ${tarball}"
