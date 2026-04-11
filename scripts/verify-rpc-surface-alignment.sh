#!/usr/bin/env bash
# Verify router, RPC catalog, and docs method inventory remain aligned.
#
# Fails if:
# - router methods are missing from catalog
# - catalog methods are missing from router
# - catalog methods are missing from docs/api/rpc.mdx method overview
# - docs/api/rpc.mdx lists methods not present in catalog

set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

red()   { printf '\033[0;31m%s\033[0m\n' "$*"; }
green() { printf '\033[0;32m%s\033[0m\n' "$*"; }

python3 - "$ROOT" <<'PY'
from __future__ import annotations

import pathlib
import re
import sys

root = pathlib.Path(sys.argv[1])

router_path = root / "meerkat-rpc" / "src" / "router.rs"
catalog_path = root / "meerkat-contracts" / "src" / "rpc_catalog.rs"
docs_path = root / "docs" / "api" / "rpc.mdx"

router_text = router_path.read_text(encoding="utf-8")
catalog_text = catalog_path.read_text(encoding="utf-8")
docs_text = docs_path.read_text(encoding="utf-8")

router_methods = {
    m
    for m in re.findall(r'"([a-z][a-z_]*(?:/[a-z_]+)*)"\s*=>', router_text)
}

catalog_methods = {
    m
    for m in re.findall(
        r'RpcMethodDescriptor::(?:basic|typed|result_only)\(\s*"([^"]+)"',
        catalog_text,
    )
}

router_methods.discard("initialized")

method_overview_match = re.search(
    r"## Method overview(.*?)## Protocol",
    docs_text,
    flags=re.DOTALL,
)
if method_overview_match is None:
    print("Could not locate '## Method overview' section in docs/api/rpc.mdx")
    sys.exit(1)

method_overview_text = method_overview_match.group(1)
docs_methods = {
    m
    for m in re.findall(r"\|\s*`([^`]+)`\s*\|\s*[^|]+\s*\|", method_overview_text)
}

missing_in_catalog = sorted(router_methods - catalog_methods)
extra_in_catalog = sorted(catalog_methods - router_methods)
missing_in_docs = sorted(catalog_methods - docs_methods)
extra_in_docs = sorted(docs_methods - catalog_methods)

fail = False

def report(label: str, items: list[str]) -> None:
    global fail
    if not items:
        return
    fail = True
    print(label)
    for item in items:
        print(f"  - {item}")

report("Router methods missing from rpc_catalog.rs:", missing_in_catalog)
report("Catalog methods missing from router.rs:", extra_in_catalog)
report("Catalog methods missing from docs/api/rpc.mdx overview table:", missing_in_docs)
report("docs/api/rpc.mdx overview methods not present in catalog:", extra_in_docs)

if fail:
    sys.exit(1)

print("RPC surface alignment OK: router, catalog, and docs method table are in sync.")
PY

green "RPC surface alignment check passed"
