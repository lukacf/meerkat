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

# The packed tarball is what npm publishes: check its contents, refuse a wasm
# stack below 8 MiB (typed parse), and run one turn from it in Node.
node scripts/smoke-packed-package.mjs "${tarball}"

echo "Built ${package_spec} package at ${tarball}"
