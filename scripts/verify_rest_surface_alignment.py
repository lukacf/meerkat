#!/usr/bin/env python3
"""Verify the REST router, path catalog, and generated OpenAPI stay aligned.

Generated-artifact-theater guard (Dogma Rule 9): the published REST contract
(`artifacts/schemas/rest-openapi.json`, emitted from
`meerkat_contracts::rest_path_catalog`) must describe exactly the endpoints the
production axum `router()` actually serves. Without this gate the catalog can
advertise phantom 404 endpoints (a real shipped drift this guard was added to
catch) or omit live routes.

Mirrors `verify_rpc_surface_alignment.py`. Lineage enforced:
`rest_path_catalog -> rest-openapi.json` (kept fresh by `verify-schema-freshness`)
`rest-openapi.json <-> router()` (kept aligned here).
"""

from __future__ import annotations

import json
import pathlib
import re
import sys


# WorkGraph REST paths are folded into BOTH the catalog and the router from the
# same typed source (`meerkat_workgraph::workgraph_rest_path_catalog` via
# `workgraph_observability_router`), so they are structurally drift-proof and
# carry no hand-authored `.route(...)` literal. They are excluded from the
# hand-authored comparison below.
WORKGRAPH_PREFIX = "/workgraph/"

# Routes that exist only in `#[cfg(test)]` modules of meerkat-rest and are never
# part of the production `router()` / advertised surface.
TEST_ONLY_ROUTES = {
    "/probe",
}


def main() -> int:
    if len(sys.argv) != 2:
        print("Usage: verify_rest_surface_alignment.py REPO_ROOT", file=sys.stderr)
        return 2

    root = pathlib.Path(sys.argv[1])
    router_path = root / "meerkat-rest" / "src" / "lib.rs"
    openapi_path = root / "artifacts" / "schemas" / "rest-openapi.json"

    router_text = router_path.read_text(encoding="utf-8")
    openapi = json.loads(openapi_path.read_text(encoding="utf-8"))

    # Hand-authored production routes: every `.route("PATH", ...)` literal in
    # the crate source. cfg-gated arms (mob/mcp) are included because we parse
    # source text, matching the unconditional catalog. Test-only routes removed.
    router_paths = {
        m
        for m in re.findall(r'\.route\(\s*"([^"]+)"', router_text)
    }
    router_paths -= TEST_ONLY_ROUTES
    router_paths = {p for p in router_paths if not p.startswith(WORKGRAPH_PREFIX)}

    # Advertised contract: every path key in the generated OpenAPI, minus the
    # fold-protected workgraph subtree.
    openapi_paths = {
        p
        for p in openapi.get("paths", {}).keys()
        if not p.startswith(WORKGRAPH_PREFIX)
    }

    failures = {
        "OpenAPI advertises paths the axum router() does not serve (phantom 404s):": sorted(
            openapi_paths - router_paths
        ),
        "Router serves paths absent from the OpenAPI catalog (undocumented surface):": sorted(
            router_paths - openapi_paths
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
        print(
            "\nFix: drive both from meerkat_contracts::rest_path_catalog "
            "(drop phantom catalog entries and/or add the missing route arm), "
            "then `make regen-schemas`.",
            file=sys.stderr,
        )
        return 1

    print(
        "REST surface alignment OK: rest-openapi.json paths and the axum "
        "router() routes are in sync."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
