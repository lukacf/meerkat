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
    fails closed if a stale artifact still carries one), and
  * the SDK wrapper surfaces (Python ``sdks/python/meerkat`` and TypeScript
    ``sdks/typescript/src``) must agree with the catalog in BOTH directions:
    every catalog method appears in hand-written SDK source (generated
    mirrors do not count as coverage), and every RPC-shaped method literal
    in SDK source resolves to a catalog method, a catalog notification, or
    a named ``SDK_STRING_ALLOWLIST`` entry — no phantom SDK methods, and
  * the web SDK (``sdks/web/src``) is the WASM-embedded runtime surface, not
    an RPC client; it must stay free of RPC-shaped method literals or be
    promoted into the checked RPC SDK list above.
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

# JSON pass-through markers that are intentionally untyped on the wire.
UNTYPED_REFS = {"Value"}

# Shape of a JSON-RPC method name in this codebase: lowercase namespaced
# segments joined by `/` (e.g. `session/create`, `auth/login/start`). The
# catalog also carries a handful of slash-less names (`initialize`,
# `initialized`, `cancel`); those are covered by the catalog->SDK direction
# (exact-literal search) and intentionally excluded from the phantom scan,
# where bare words would be hopelessly noisy.
RPC_METHOD_SHAPE = re.compile(r"^[a-z][a-z0-9_.-]*(?:/[a-z0-9_.-]+)+$")

# Single- or double-quoted string literal (single line), for Python and
# TypeScript SDK sources alike.
STRING_LITERAL = re.compile(r'"([^"\n]*)"|\'([^\'\n]*)\'')

# RPC-shaped strings that legitimately appear in SDK source without being
# catalog methods. Every entry must carry a named justification — silent
# filtering is exactly how catalog<->SDK drift goes unnoticed. This extends
# the `internal_exclusions` convention from verify_sdk_wrapper_freshness.py
# with explicit per-entry reasons.
SDK_STRING_ALLOWLIST: dict[str, str] = {
    "tool/execute": (
        "server->client reverse request the SDKs *receive* to run a "
        "registered callback tool (meerkat-rpc/src/callback_dispatcher.rs); "
        "never a client-callable catalog method"
    ),
    "lukacf/meerkat": (
        "GitHub repository slug used by the TypeScript client's release "
        "metadata lookup (sdks/typescript/src/client.ts MEERKAT_REPO); "
        "matches the method shape but is not an RPC method"
    ),
}

# SDKs whose hand-written sources are full JSON-RPC clients and must cover
# the entire catalog. (sdk label, repo-relative root, file suffix)
RPC_SDK_SURFACES = [
    ("Python SDK", pathlib.Path("sdks") / "python" / "meerkat", ".py"),
    ("TypeScript SDK", pathlib.Path("sdks") / "typescript" / "src", ".ts"),
]

# The web SDK is the WASM-embedded runtime surface (`@rkat/web` calls
# wasm_bindgen exports, not JSON-RPC). It must stay RPC-string-free; if it
# ever grows RPC method literals it has silently become an RPC client and
# must be moved into RPC_SDK_SURFACES instead.
NON_RPC_SDK_SURFACES = [
    ("Web SDK", pathlib.Path("sdks") / "web" / "src", ".ts"),
]

WEB_AUTH_WRAPPER = pathlib.Path("sdks") / "web" / "src" / "auth.ts"
WEB_AUTH_METHODS = pathlib.Path("sdks") / "web" / "src" / "generated" / "auth.ts"


def collect_sdk_source(
    sdk_root: pathlib.Path,
    suffix: str,
    excluded_paths: set[pathlib.Path] | None = None,
) -> tuple[set[str], str]:
    """RPC-shaped string literals + concatenated hand-written SDK source.

    Generated mirrors (``generated/`` trees) are excluded on both sides: a
    method name surviving only in a generated docstring is not wrapper
    coverage, and generated content is projected from the catalog so it can
    never be a phantom.
    """
    literals: set[str] = set()
    blob_parts: list[str] = []
    excluded_paths = excluded_paths or set()
    for path in sorted(sdk_root.rglob(f"*{suffix}")):
        rel_path = path.relative_to(sdk_root.parents[2])
        if rel_path in excluded_paths:
            continue
        if "generated" in path.parts or "__pycache__" in path.parts:
            continue
        try:
            text = path.read_text(encoding="utf-8", errors="ignore")
        except OSError:
            continue
        blob_parts.append(text)
        for double_quoted, single_quoted in STRING_LITERAL.findall(text):
            value = double_quoted or single_quoted
            if RPC_METHOD_SHAPE.match(value):
                literals.add(value)
    return literals, "\n".join(blob_parts)


def web_auth_rpc_methods(root: pathlib.Path) -> set[str]:
    """RPC methods used by the Web SDK Auth wrapper via generated constants."""
    methods_path = root / WEB_AUTH_METHODS
    wrapper_path = root / WEB_AUTH_WRAPPER
    try:
        methods_text = methods_path.read_text(encoding="utf-8")
        wrapper_text = wrapper_path.read_text(encoding="utf-8")
    except OSError:
        return set()

    constant_to_method = {
        key: method
        for key, method in re.findall(r"(\w+):\s*[\"']([^\"']+)[\"']", methods_text)
        if RPC_METHOD_SHAPE.match(method)
    }
    used_constants = set(re.findall(r"\bAUTH_RPC_METHODS\.(\w+)\b", wrapper_text))
    return {
        method
        for key, method in constant_to_method.items()
        if key in used_constants
    }


def check_web_auth_surface(root: pathlib.Path, catalog_methods: set[str]) -> list[str]:
    """The Web SDK Auth wrapper is intentionally RPC-shaped and must be gated."""
    failures: list[str] = []
    expected = {method for method in catalog_methods if method.startswith("auth/")}
    actual = web_auth_rpc_methods(root)

    missing = sorted(expected - actual)
    if missing:
        failures.append(
            "Web SDK Auth wrapper (sdks/web/src/auth.ts) is missing generated "
            "AUTH_RPC_METHODS coverage for auth catalog methods:"
        )
        failures.extend(f"  - {name}" for name in missing)

    extra = sorted(actual - expected)
    if extra:
        failures.append(
            "Web SDK Auth wrapper references generated AUTH_RPC_METHODS that "
            "are not auth catalog methods:"
        )
        failures.extend(f"  - {name}" for name in extra)

    return failures


def check_sdk_surfaces(
    root: pathlib.Path,
    catalog_methods: set[str],
    notification_names: set[str],
) -> list[str]:
    """Catalog<->SDK divergence in both directions. Returns failure lines."""
    failures: list[str] = []
    known = catalog_methods | notification_names | set(SDK_STRING_ALLOWLIST)

    for label, rel_root, suffix in RPC_SDK_SURFACES:
        literals, blob = collect_sdk_source(root / rel_root, suffix)

        phantoms = sorted(literals - known)
        if phantoms:
            failures.append(
                f"{label} ({rel_root}) references RPC-shaped method strings "
                "that are neither catalog methods, catalog notifications, nor "
                "named SDK_STRING_ALLOWLIST entries:"
            )
            failures.extend(f"  - {name}" for name in phantoms)

        missing = sorted(
            method
            for method in catalog_methods
            if f'"{method}"' not in blob and f"'{method}'" not in blob
        )
        if missing:
            failures.append(
                f"{label} ({rel_root}) hand-written source has no wrapper/"
                "usage for catalog methods (generated mirrors do not count):"
            )
            failures.extend(f"  - {name}" for name in missing)

    failures.extend(check_web_auth_surface(root, catalog_methods))

    for label, rel_root, suffix in NON_RPC_SDK_SURFACES:
        literals, _ = collect_sdk_source(
            root / rel_root,
            suffix,
            excluded_paths={WEB_AUTH_WRAPPER},
        )
        stray = sorted(literals - set(SDK_STRING_ALLOWLIST))
        if stray:
            failures.append(
                f"{label} ({rel_root}) is a WASM-backed surface and must not "
                "hand-roll RPC method strings; either remove these or promote "
                "the SDK into RPC_SDK_SURFACES so it is fully gated:"
            )
            failures.extend(f"  - {name}" for name in stray)

    return failures


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

    # -------------------------------------------------------------------
    # SDK wrapper surfaces: catalog <-> SDK method-name parity in both
    # directions (no phantom SDK methods, no uncovered catalog methods),
    # plus the web-SDK is-not-an-RPC-client invariant.
    # -------------------------------------------------------------------
    notification_names = {
        entry["name"]
        for entry in generated.get("notifications", [])
        if isinstance(entry, dict) and "name" in entry
    }
    sdk_failures = check_sdk_surfaces(root, catalog_methods, notification_names)
    if sdk_failures:
        for line in sdk_failures:
            print(line)
        return 1

    print(
        "RPC surface alignment OK: generated catalog, docs method table, "
        "typed param/result refs, and SDK wrapper surfaces are in sync."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
