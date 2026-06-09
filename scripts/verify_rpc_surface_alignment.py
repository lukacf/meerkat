#!/usr/bin/env python3
"""Verify router, RPC catalog, and docs method inventory remain aligned.

Beyond name-set parity, this also diffs the *typed* param/result refs the
catalog descriptors carry against:

  * the generated ``artifacts/schemas/rpc-methods.json`` descriptor (so a
    catalog edit that is not re-emitted via ``make regen-schemas`` fails), and
  * the universe of real schema/surface types (so a method whose
    ``params_type``/``result_type`` names a type that does not exist anywhere
    fails closed instead of advertising a phantom contract).
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

    router_path = root / "meerkat-rpc" / "src" / "router.rs"
    runtime_handlers_path = root / "meerkat-rpc" / "src" / "handlers" / "runtime.rs"
    catalog_path = root / "meerkat-contracts" / "src" / "rpc_catalog.rs"
    docs_path = root / "docs" / "api" / "rpc.mdx"
    generated_catalog_path = (
        root / "artifacts" / "schemas" / "rpc-methods.json"
    )

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

    # -------------------------------------------------------------------
    # Typed param/result alignment.
    #
    # The catalog carries typed `params_type`/`result_type` refs per method.
    # `make regen-schemas` emits them into `artifacts/schemas/rpc-methods.json`
    # via `meerkat_contracts::rpc_method_catalog`. Diff:
    #   (a) the generated descriptor's typed refs against the catalog source
    #       (a catalog type edit not re-emitted fails), and
    #   (b) every typed ref against the universe of real schema/surface types
    #       (a method advertising a type that exists nowhere fails closed).
    # -------------------------------------------------------------------
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

    # Typed refs declared in the catalog source, keyed by method name. This is
    # the source-of-truth the generated artifact must mirror; a drift here
    # means `make regen-schemas` was not run after editing a descriptor type.
    catalog_typed = {
        name: (params or None, result or None)
        for name, params, result in re.findall(
            r"RpcMethodDescriptor::typed\(\s*"
            r'"([^"]+)"\s*,\s*'  # method name
            r'(?:"(?:[^"\\]|\\.)*")\s*,\s*'  # description (discarded)
            r'"([^"]+)"\s*,\s*'  # params_type
            r'"([^"]+)"',  # result_type
            catalog_text,
        )
    }

    generated_drift: list[str] = []
    for name, (src_params, src_result) in sorted(catalog_typed.items()):
        gen = generated_methods.get(name)
        if gen is None:
            generated_drift.append(
                f"  - {name}: typed in catalog source but absent from generated rpc-methods.json"
            )
            continue
        if gen.get("params_type") != src_params:
            generated_drift.append(
                f"  - {name}: params_type catalog=`{src_params}` "
                f"generated=`{gen.get('params_type')}`"
            )
        if gen.get("result_type") != src_result:
            generated_drift.append(
                f"  - {name}: result_type catalog=`{src_result}` "
                f"generated=`{gen.get('result_type')}`"
            )

    if generated_drift:
        print(
            "Generated artifacts/schemas/rpc-methods.json is stale vs the "
            "catalog typed refs (run `make regen-schemas`):"
        )
        for line in generated_drift:
            print(line)
        return 1

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
        "RPC surface alignment OK: router, catalog, docs method table, "
        "retired handler exports, and typed param/result refs are in sync."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
