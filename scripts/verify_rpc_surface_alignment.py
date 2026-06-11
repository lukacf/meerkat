#!/usr/bin/env python3
"""Verify the generated RPC catalog, docs method inventory, and typed refs.

The single source of truth for the callable RPC surface is
``meerkat_contracts::rpc_method_catalog``. Its generated projection
(``artifacts/schemas/rpc-methods.json``) is kept byte-fresh against the Rust
source by ``make verify-schema-freshness``, and router<->catalog dispatch
parity is enforced in-process by the catalog-driven dispatch test in
``meerkat-rpc`` (every catalog method dispatches; unknown methods reject).

This gate owns the remaining projections of that catalog:

  * the docs method table (``docs/api/rpc.mdx``) must list exactly the
    generated catalog methods, and
  * every typed ``params_type``/``result_type`` ref must resolve to a real
    schema or ``meerkat-rpc`` surface type (no phantom contracts), and
  * every method must carry a typed result contract (no schema-light
    descriptors — the Rust catalog has no schema-less constructor; this
    fails closed if a stale artifact still carries one).
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

# JSON pass-through markers that are intentionally untyped on the wire.
UNTYPED_REFS = {"Value"}


def split_type_refs(type_ref: str | None) -> list[str]:
    """Split a descriptor type ref like ``"A | B"`` into individual names."""
    if not type_ref:
        return []
    return [part.strip() for part in type_ref.split("|") if part.strip()]


def collect_schema_type_names(schemas_dir: pathlib.Path) -> set[str]:
    """All top-level schema type names emitted across the schema artifacts.

    The schema JSON artifacts are flat ``{TypeName: schema}`` maps; the RPC
    descriptor file is ``{methods: [...], notifications: [...]}`` and is not a
    type source, so it is skipped.
    """
    names: set[str] = set()
    if not schemas_dir.is_dir():
        return names
    for path in sorted(schemas_dir.glob("*.json")):
        if path.name == "rpc-methods.json":
            continue
        try:
            data = json.loads(path.read_text(encoding="utf-8"))
        except (OSError, json.JSONDecodeError):
            continue
        if isinstance(data, dict):
            for key, value in data.items():
                if isinstance(value, dict):
                    names.add(key)
    return names


def collect_surface_type_names(rpc_src: pathlib.Path) -> set[str]:
    """Public param/result types declared in the ``meerkat-rpc`` surface.

    A handful of RPC param/result types (e.g. ``CreateSessionParams``,
    ``StartTurnParams``) live in the surface crate rather than in
    ``meerkat-contracts``, so they never reach the schema artifacts. They are
    still real, resolvable types — discover them so the typed-ref check does
    not false-positive on them.
    """
    names: set[str] = set()
    if not rpc_src.is_dir():
        return names
    decl = re.compile(r"\bpub\s+(?:struct|enum|type)\s+([A-Za-z0-9_]+)")
    for path in rpc_src.rglob("*.rs"):
        try:
            text = path.read_text(encoding="utf-8")
        except OSError:
            continue
        names.update(decl.findall(text))
    return names


def main() -> int:
    if len(sys.argv) != 2:
        print("Usage: verify_rpc_surface_alignment.py REPO_ROOT", file=sys.stderr)
        return 2

    root = pathlib.Path(sys.argv[1])

    docs_path = root / "docs" / "api" / "rpc.mdx"
    generated_catalog_path = root / "artifacts" / "schemas" / "rpc-methods.json"

    docs_text = docs_path.read_text(encoding="utf-8")

    try:
        generated = json.loads(generated_catalog_path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as exc:
        print(
            f"Could not read generated RPC descriptor {generated_catalog_path}: {exc}"
        )
        return 1

    generated_methods = {
        entry["name"]: entry
        for entry in generated.get("methods", [])
        if isinstance(entry, dict) and "name" in entry
    }
    catalog_methods = set(generated_methods)

    method_overview_match = re.search(
        r"## Method overview(.*?)## Protocol",
        docs_text,
        flags=re.DOTALL,
    )
    if method_overview_match is None:
        print("Could not locate '## Method overview' section in docs/api/rpc.mdx")
        return 1

    method_overview_text = method_overview_match.group(1)
    # Parse the overview table row-wise: split each row on unescaped pipes and
    # take the first cell as the method name. (A naive "backticked token
    # followed by a pipe" regex would also match the typed Params/Result
    # columns.)
    docs_methods: set[str] = set()
    for line in method_overview_text.splitlines():
        stripped = line.strip()
        if not stripped.startswith("|"):
            continue
        cells = [c.strip() for c in re.split(r"(?<!\\)\|", stripped)[1:-1]]
        if not cells or cells[0] == "Method" or set(cells[0]) <= set("-: "):
            continue
        cell_match = re.match(r"^`([^`]+)`$", cells[0])
        if cell_match:
            docs_methods.add(cell_match.group(1))

    failures = {
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

    # -------------------------------------------------------------------
    # Catalog totality: no schema-light methods.
    # -------------------------------------------------------------------
    schema_light = sorted(
        name
        for name, entry in generated_methods.items()
        if not entry.get("result_type")
    )
    if schema_light:
        print(
            "RPC catalog methods without a typed result contract (the catalog "
            "has no schema-less constructor; regenerate with `make regen-schemas`):"
        )
        for name in schema_light:
            print(f"  - {name}")
        return 1

    # -------------------------------------------------------------------
    # Typed param/result resolution: every typed ref must resolve against
    # the universe of real schema/surface types (a method advertising a
    # type that exists nowhere fails closed).
    # -------------------------------------------------------------------
    schema_types = collect_schema_type_names(root / "artifacts" / "schemas")
    surface_types = collect_surface_type_names(root / "meerkat-rpc" / "src")
    resolvable = schema_types | surface_types | UNTYPED_REFS

    unresolved: list[str] = []
    for name in sorted(generated_methods):
        entry = generated_methods[name]
        for kind in ("params_type", "result_type"):
            for ref in split_type_refs(entry.get(kind)):
                if ref not in resolvable:
                    unresolved.append(f"  - {name}.{kind} -> `{ref}`")

    if unresolved:
        print(
            "RPC catalog advertises typed param/result refs that resolve to no "
            "schema or meerkat-rpc surface type:"
        )
        for line in unresolved:
            print(line)
        return 1

    print(
        "RPC surface alignment OK: generated catalog, docs method table, and "
        "typed param/result refs are in sync."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
