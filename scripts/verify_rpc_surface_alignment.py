#!/usr/bin/env python3
"""Verify router, RPC catalog, and docs method inventory remain aligned."""

from __future__ import annotations

import pathlib
import re
import sys


def main() -> int:
    if len(sys.argv) != 2:
        print("Usage: verify_rpc_surface_alignment.py REPO_ROOT", file=sys.stderr)
        return 2

    root = pathlib.Path(sys.argv[1])

    router_path = root / "meerkat-rpc" / "src" / "router.rs"
    runtime_handlers_path = root / "meerkat-rpc" / "src" / "handlers" / "runtime.rs"
    catalog_path = root / "meerkat-contracts" / "src" / "rpc_catalog.rs"
    docs_path = root / "docs" / "api" / "rpc.mdx"

    router_text = router_path.read_text(encoding="utf-8")
    runtime_handlers_text = runtime_handlers_path.read_text(encoding="utf-8")
    catalog_text = catalog_path.read_text(encoding="utf-8")
    docs_text = docs_path.read_text(encoding="utf-8")

    # Method arms may have an optional `if guard` between the literal and `=>`,
    # e.g. `"live/open" if self.live_ws_state.is_some() => { ... }` for arms
    # that are gated on optional infrastructure being attached at construction.
    router_methods = {
        m
        for m in re.findall(
            r'"([a-z][a-z_]*(?:/[a-z_]+)*)"\s*(?:if[^=]+)?=>', router_text
        )
    }

    catalog_methods = {
        m
        for m in re.findall(
            r'RpcMethodDescriptor::(?:basic|typed|params_only|result_only)\(\s*"([^"]+)"',
            catalog_text,
        )
    }

    router_methods.discard("initialized")

    # Router-only compatibility shims intentionally remain absent from the
    # public catalog and docs. Retired runtime/session_* control paths must not
    # be added here: the public catalog owns the callable RPC surface.
    router_only_compat = {
        "skills/inspect",
    }
    router_methods -= router_only_compat

    method_overview_match = re.search(
        r"## Method overview(.*?)## Protocol",
        docs_text,
        flags=re.DOTALL,
    )
    if method_overview_match is None:
        print("Could not locate '## Method overview' section in docs/api/rpc.mdx")
        return 1

    method_overview_text = method_overview_match.group(1)
    docs_methods = {
        m
        for m in re.findall(
            r"\|\s*`([^`]+)`\s*\|\s*[^|]+\s*\|",
            method_overview_text,
        )
    }

    failures = {
        "Router methods missing from rpc_catalog.rs:": sorted(
            router_methods - catalog_methods
        ),
        "Catalog methods missing from router.rs:": sorted(
            catalog_methods - router_methods
        ),
        "Catalog methods missing from docs/api/rpc.mdx overview table:": sorted(
            catalog_methods - docs_methods
        ),
        "docs/api/rpc.mdx overview methods not present in catalog:": sorted(
            docs_methods - catalog_methods
        ),
    }

    failed = False
    for label, items in failures.items():
        if not items:
            continue
        failed = True
        print(label)
        for item in items:
            print(f"  - {item}")

    if failed:
        return 1

    retired_runtime_handlers = [
        "handle_runtime_status",
        "handle_runtime_submit",
        "handle_runtime_submission",
        "handle_runtime_submissions",
        "handle_runtime_retire",
        "handle_runtime_reset",
    ]
    public_retired_runtime_handlers = [
        handler
        for handler in retired_runtime_handlers
        if re.search(rf"\bpub\s+async\s+fn\s+{handler}\b", runtime_handlers_text)
    ]
    if public_retired_runtime_handlers:
        print(
            "Retired runtime/session handler functions remain public in "
            f"{runtime_handlers_path}:"
        )
        for handler in public_retired_runtime_handlers:
            print(f"  - {handler}")
        return 1

    print(
        "RPC surface alignment OK: router, catalog, docs method table, and "
        "retired handler exports are in sync."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
