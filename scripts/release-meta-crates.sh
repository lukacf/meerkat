#!/usr/bin/env bash
# The hand-ordered enumeration of Meta's publishable crates, in dependency
# order: Meta's counterpart of scripts/release-rust-crates.sh, which stays
# Core's list alone. Meta's release publishes only these crates, from the one
# workspace, against the Core versions they declare (cargo strips the path
# dependencies when it packages them).
#
#   release-meta-crates.sh                 one crate per line, in this order
#   release-meta-crates.sh --levels [ARGS]  the same crates grouped into the
#       dependency levels a parallel publish walks, derived from this order
#       and `cargo metadata` by scripts/release_crate_levels.py (ARGS pass
#       through, for example `--format summary`)
#
# scripts/release-publish-rust-crates.sh and scripts/generate-patch-config.sh
# publish this list instead of Core's when MEERKAT_RELEASE_CRATE_LIST names
# this script.

set -euo pipefail

if [[ "${1:-}" == "--levels" ]]; then
  shift
  root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
  python="$("${root}/scripts/require-python" 3.10 "$(basename "${BASH_SOURCE[0]}") --levels")" || exit 1
  "${BASH_SOURCE[0]}" | "${python}" "${root}/scripts/release_crate_levels.py" --root "${root}" "$@"
  exit
fi
if [[ "$#" -ne 0 ]]; then
  echo "usage: $(basename "$0") [--levels [--format tsv|summary]]" >&2
  exit 2
fi

# The order itself is ci/products.json's meta publish_order: the one
# hand-ordered list of Meta's crates (the products registry the CI planner,
# the floor check and the release gates read).
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
python="$("${root}/scripts/require-python" 3.10 "$(basename "${BASH_SOURCE[0]}")")" || exit 1
"${python}" -I -c '
import json, sys
products = json.load(open(sys.argv[1]))["products"]
meta = next(product for product in products if product["name"] == "meta")
order = meta["publish_order"]
if not isinstance(order, list) or not order:
    raise SystemExit("ci/products.json: meta has no publish_order list")
print("\n".join(order))
' "${root}/ci/products.json"
