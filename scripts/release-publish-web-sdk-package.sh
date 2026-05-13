#!/usr/bin/env bash
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
tarball="${1:-}"

if [[ -z "${tarball}" ]]; then
  tarball="$(find "${root}/web-sdk-package" "${root}/dist/web-sdk-package" -maxdepth 1 -name 'rkat-web-*.tgz' -print -quit 2>/dev/null || true)"
fi

if [[ -z "${tarball}" || ! -f "${tarball}" ]]; then
  echo "Usage: $0 /path/to/rkat-web-<version>.tgz" >&2
  exit 2
fi

cd "${root}"

dry_run="${MEERKAT_REGISTRY_DRY_RUN:-${REGISTRY_DRY_RUN:-false}}"
package_spec="$(node -p "const p = require('./sdks/web/package.json'); p.name + '@' + p.version")"

if [[ "${dry_run}" != "true" ]] && npm view "${package_spec}" version >/dev/null 2>&1; then
  echo "${package_spec} already published, skipping"
  exit 0
fi

if [[ -n "${NODE_AUTH_TOKEN:-}" ]]; then
  npm config set //registry.npmjs.org/:_authToken "${NODE_AUTH_TOKEN}"
fi

if [[ "${dry_run}" == "true" ]]; then
  npm publish "${tarball}" --access public --dry-run --ignore-scripts
else
  npm publish "${tarball}" --access public --ignore-scripts
fi
