#!/usr/bin/env python3
"""Verify the REST router, path catalog, and generated OpenAPI stay aligned.

Generated-artifact-theater guard (Dogma Rule 9): the published REST contract
(`artifacts/schemas/rest-openapi.json`, emitted from
`meerkat_contracts::rest_path_catalog`) must describe exactly the
(path, method) endpoints the production axum `router()` actually serves. Without
this gate the catalog can advertise phantom 404 endpoints (a real shipped drift
this guard was added to catch), omit live routes, or advertise/miss a specific
HTTP method on a shared path.

Mirrors `verify_rpc_surface_alignment.py`. Lineage enforced:
`rest_path_catalog -> rest-openapi.json` (kept fresh by `verify-schema-freshness`)
`rest-openapi.json <-> router()` (kept aligned here, per (path, method) pair).
"""

from __future__ import annotations

import json
import pathlib
import re
import sys


# WorkGraph REST paths are folded into BOTH the catalog and the router from the
# same typed source (`meerkat_workgraph::workgraph_rest_path_catalog` via
# `workgraph_observability_router`), so they are structurally drift-proof. On the
# router side they carry no string-literal path (the fold uses
# `router.route(descriptor.path, ..)`), so they are skipped here, and they are
# filtered out of the OpenAPI side by prefix.
WORKGRAPH_PREFIX = "/workgraph/"

# (path, method) endpoints that exist only in `#[cfg(test)]` modules of
# meerkat-rest and are never part of the production `router()` surface.
TEST_ONLY_ENDPOINTS = {
    ("/probe", "post"),
}

# axum method-router builder names that map to HTTP methods.
HTTP_METHODS = ("get", "post", "put", "patch", "delete", "head", "options", "trace")
_METHOD_BUILDER_RE = re.compile(r"\b(" + "|".join(HTTP_METHODS) + r")\s*\(")
_DOT_ROUTE_RE = re.compile(r"\.route\s*\(")
_FIRST_STRING_RE = re.compile(r'"([^"]+)"')
# Boundaries that end one route's method-router argument when scanning the
# builder source (only used as a sanity fallback; primary parsing is paren-matched).


def _matched_paren_slice(text: str, open_paren_idx: int) -> str:
    """Return the substring inside the parentheses starting at `open_paren_idx`
    (which must point at a '('), respecting nesting."""
    depth = 0
    for i in range(open_paren_idx, len(text)):
        ch = text[i]
        if ch == "(":
            depth += 1
        elif ch == ")":
            depth -= 1
            if depth == 0:
                return text[open_paren_idx + 1 : i]
    return text[open_paren_idx + 1 :]


def router_endpoints(router_text: str) -> set[tuple[str, str]]:
    """Extract (path, method) pairs from every `.route("PATH", <method-router>)`
    call in the crate source. Paren-matched so multi-method chains
    (`get(h).post(h)`) and multi-line route arms are handled. Routes whose path
    is not a string literal (the workgraph fold's `descriptor.path`) are skipped.
    """
    endpoints: set[tuple[str, str]] = set()
    for m in _DOT_ROUTE_RE.finditer(router_text):
        open_paren = router_text.index("(", m.start())
        args = _matched_paren_slice(router_text, open_paren)
        path_match = _FIRST_STRING_RE.search(args)
        if path_match is None:
            continue  # non-literal path (workgraph fold) — skip
        path = path_match.group(1)
        # Method builders appear after the path literal in the method-router arg.
        method_region = args[path_match.end() :]
        for method in _METHOD_BUILDER_RE.findall(method_region):
            endpoints.add((path, method.lower()))
    return endpoints


def openapi_endpoints(openapi: dict) -> set[tuple[str, str]]:
    endpoints: set[tuple[str, str]] = set()
    for path, ops in openapi.get("paths", {}).items():
        if not isinstance(ops, dict):
            continue
        for method in ops:
            if method.lower() in HTTP_METHODS:
                endpoints.add((path, method.lower()))
    return endpoints


def main() -> int:
    if len(sys.argv) != 2:
        print("Usage: verify_rest_surface_alignment.py REPO_ROOT", file=sys.stderr)
        return 2

    root = pathlib.Path(sys.argv[1])
    router_path = root / "meerkat-rest" / "src" / "lib.rs"
    openapi_path = root / "artifacts" / "schemas" / "rest-openapi.json"

    router_text = router_path.read_text(encoding="utf-8")
    openapi = json.loads(openapi_path.read_text(encoding="utf-8"))

    def keep(endpoint: tuple[str, str]) -> bool:
        path, _ = endpoint
        return not path.startswith(WORKGRAPH_PREFIX) and endpoint not in TEST_ONLY_ENDPOINTS

    served = {e for e in router_endpoints(router_text) if keep(e)}
    advertised = {e for e in openapi_endpoints(openapi) if keep(e)}

    def fmt(pairs: set[tuple[str, str]]) -> list[str]:
        return sorted(f"{method.upper()} {path}" for path, method in pairs)

    failures = {
        "OpenAPI advertises (path, method) the axum router() does not serve (phantom 404s):": fmt(
            advertised - served
        ),
        "Router serves (path, method) absent from the OpenAPI catalog (undocumented surface):": fmt(
            served - advertised
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
        f"REST surface alignment OK: {len(served)} (path, method) endpoints in "
        "rest-openapi.json and the axum router() are in sync."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
