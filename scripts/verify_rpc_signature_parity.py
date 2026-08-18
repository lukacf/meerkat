#!/usr/bin/env python3
"""Verify RPC *signature* parity between the generated method catalog, the
documented surface, and the SDK wrappers.

The sibling ``verify_rpc_surface_alignment.py`` gate verifies method NAME-set
parity. This gate goes one level deeper and verifies, per method, the
declared param/result SHAPE (the typed refs the catalog descriptors carry):

  1. **Docs**: the ``## Method overview`` table in ``docs/api/rpc.mdx`` has
     ``Params``/``Result`` columns; for every catalog method the documented
     type must equal the catalog's ``params_type``/``result_type`` exactly.
     A doc that drifts on shape (not just on name) fails.

  2. **SDK transports** (TypeScript + Python): generated method maps bind every
     catalog method to its generated params/result refs at the actual request
     boundary. Wrapper-local marker aliases do not count and are rejected.

Catalog truth is read from the committed ``artifacts/schemas/rpc-methods.json``
artifact (emitted from ``meerkat_contracts::rpc_method_catalog`` by
``make regen-schemas``); the sibling gate already pins that artifact against
the Rust catalog source, so this script never compiles anything.

Historical wrappers follow the same generated transport rule as new wrappers.
There is no grandfathered baseline or expiry waiver.
"""

from __future__ import annotations

import ast
import json
import pathlib
import re
import sys
from dataclasses import dataclass, field

# JSON pass-through markers that are intentionally untyped on the wire.
UNTYPED_REFS = {"Value"}

# Methods that intentionally have no public wrapper in the SDKs. Keep in sync
# with verify_sdk_wrapper_freshness.py rationale: stream open/close are
# transport internals managed by EventSubscription plumbing.
SDK_SEND_SITE_EXCLUSIONS = {
    "ts": set(),
    "py": set(),
    "web-auth": set(),
}

# There is intentionally no grandfathered baseline. Every SDK wrapper whose
# catalog type exists in the generated module must reference that generated
# type, so newly introduced and historical wrappers follow the same rule.


def split_type_refs(type_ref: str | None) -> list[str]:
    if not type_ref:
        return []
    return [part.strip() for part in type_ref.split("|") if part.strip()]


def load_catalog(root: pathlib.Path) -> dict[str, dict[str, str | None]]:
    path = root / "artifacts" / "schemas" / "rpc-methods.json"
    data = json.loads(path.read_text(encoding="utf-8"))
    catalog: dict[str, dict[str, str | None]] = {}
    for entry in data.get("methods", []):
        if isinstance(entry, dict) and "name" in entry:
            catalog[entry["name"]] = {
                "params": entry.get("params_type"),
                "result": entry.get("result_type"),
            }
    return catalog


# ---------------------------------------------------------------------------
# Docs: typed Params/Result columns in the method overview table.
# ---------------------------------------------------------------------------


def split_table_cells(line: str) -> list[str]:
    cells = re.split(r"(?<!\\)\|", line.strip())
    return [c.strip() for c in cells[1:-1]]


def parse_type_cell(cell: str) -> str | None:
    text = cell.replace("\\|", "|").strip()
    if text in ("", "—", "-", "–"):
        return None
    match = re.match(r"^`([^`]+)`$", text)
    if match is None:
        return text  # compared verbatim; will fail with a clear message
    return match.group(1).strip()


def check_docs(
    root: pathlib.Path, catalog: dict[str, dict[str, str | None]]
) -> list[str]:
    docs_path = root / "docs" / "api" / "rpc.mdx"
    docs_text = docs_path.read_text(encoding="utf-8")
    overview = re.search(r"## Method overview(.*?)\n## ", docs_text, flags=re.DOTALL)
    if overview is None:
        return ["docs: could not locate '## Method overview' section in rpc.mdx"]

    failures: list[str] = []
    header_cols: list[str] | None = None
    documented: dict[str, tuple[str | None, str | None]] = {}
    for line in overview.group(1).splitlines():
        stripped = line.strip()
        if not stripped.startswith("|"):
            continue
        cells = split_table_cells(stripped)
        if not cells:
            continue
        if cells[0] == "Method":
            header_cols = cells
            continue
        if set(cells[0]) <= set("-: "):
            continue
        if header_cols is None:
            continue
        name_match = re.match(r"^`([^`]+)`$", cells[0])
        if name_match is None:
            continue
        row = dict(zip(header_cols, cells))
        documented[name_match.group(1)] = (
            parse_type_cell(row.get("Params", "")),
            parse_type_cell(row.get("Result", "")),
        )

    if header_cols is None:
        return ["docs: method overview table has no header row"]
    for required in ("Params", "Result"):
        if required not in header_cols:
            failures.append(
                f"docs: method overview table is missing the `{required}` "
                f"column (found columns: {header_cols}); the documented "
                "surface must carry the catalog's typed refs"
            )
    if failures:
        return failures

    for name in sorted(catalog):
        if name not in documented:
            failures.append(
                f"docs: method `{name}` missing from the typed overview table"
            )
            continue
        doc_params, doc_result = documented[name]
        for side, doc_val in (("params", doc_params), ("result", doc_result)):
            cat_val = catalog[name][side]
            if doc_val != cat_val:
                failures.append(
                    f"docs: `{name}` {side} type drift — catalog declares "
                    f"`{cat_val}` but docs/api/rpc.mdx documents `{doc_val}`"
                )
    for name in sorted(set(documented) - set(catalog)):
        failures.append(
            f"docs: overview table documents `{name}` which is not in the "
            "generated catalog"
        )
    return failures


# ---------------------------------------------------------------------------
# Generated SDK type inventories.
# ---------------------------------------------------------------------------


def ts_generated_names(root: pathlib.Path) -> set[str]:
    names: set[str] = set()
    gen_dir = root / "sdks" / "typescript" / "src" / "generated"
    for path in sorted(gen_dir.glob("*.ts")):
        text = path.read_text(encoding="utf-8")
        names.update(
            re.findall(
                r"export\s+(?:declare\s+)?(?:interface|type|enum|const enum|class)\s+"
                r"([A-Za-z_$][\w$]*)",
                text,
            )
        )
    return names


def web_generated_names(root: pathlib.Path) -> set[str]:
    names: set[str] = set()
    gen_dir = root / "sdks" / "web" / "src" / "generated"
    for path in sorted(gen_dir.glob("*.ts")):
        text = path.read_text(encoding="utf-8")
        names.update(
            re.findall(
                r"export\s+(?:declare\s+)?(?:interface|type|enum|const enum|class)\s+"
                r"([A-Za-z_$][\w$]*)",
                text,
            )
        )
    return names


def py_generated_names(root: pathlib.Path) -> set[str]:
    names: set[str] = set()
    gen_dir = root / "sdks" / "python" / "meerkat" / "generated"
    for path in sorted(gen_dir.glob("*.py")):
        text = path.read_text(encoding="utf-8")
        names.update(re.findall(r"^class\s+([A-Za-z_]\w*)", text, flags=re.M))
        names.update(
            re.findall(r"^([A-Za-z_]\w*)\s*(?::\s*[^=\n]+)?=", text, flags=re.M)
        )
    return names


# ---------------------------------------------------------------------------
# TypeScript analysis: masking, import resolution, function spans, send sites.
# ---------------------------------------------------------------------------


def mask_ts(text: str) -> str:
    """Blank out comments and string literal *contents* (length-preserving)."""
    out = list(text)
    i = 0
    n = len(text)

    def blank(idx: int) -> None:
        if out[idx] != "\n":
            out[idx] = " "

    while i < n:
        c = text[i]
        nxt = text[i + 1] if i + 1 < n else ""
        if c == "/" and nxt == "/":
            while i < n and text[i] != "\n":
                blank(i)
                i += 1
            continue
        if c == "/" and nxt == "*":
            blank(i)
            blank(i + 1)
            i += 2
            while i < n and not (text[i] == "*" and i + 1 < n and text[i + 1] == "/"):
                blank(i)
                i += 1
            if i < n:
                blank(i)
                blank(i + 1)
                i += 2
            continue
        if c in ("'", '"'):
            quote = c
            i += 1
            while i < n and text[i] != quote:
                if text[i] == "\\":
                    blank(i)
                    i += 1
                if i < n:
                    blank(i)
                    i += 1
            i += 1
            continue
        if c == "`":
            i += 1
            while i < n:
                if text[i] == "\\":
                    blank(i)
                    i += 1
                    if i < n:
                        blank(i)
                        i += 1
                    continue
                if text[i] == "`":
                    i += 1
                    break
                if text[i] == "$" and i + 1 < n and text[i + 1] == "{":
                    # Skip interpolation contents unmasked (close enough; the
                    # interpolation is code, not prose).
                    depth = 0
                    while i < n:
                        if text[i] == "{":
                            depth += 1
                        elif text[i] == "}":
                            depth -= 1
                            if depth == 0:
                                i += 1
                                break
                        i += 1
                    continue
                blank(i)
                i += 1
            continue
        i += 1
    return "".join(out)


TS_KEYWORDS = {
    "if",
    "for",
    "while",
    "switch",
    "catch",
    "return",
    "typeof",
    "new",
    "await",
    "delete",
    "void",
    "do",
    "else",
    "in",
    "of",
    "case",
    "throw",
    "yield",
    "super",
    "constructor",
}


@dataclass
class TsFunction:
    name: str
    start: int  # header start offset
    body_end: int  # offset one past closing brace
    line: int


def ts_function_spans(masked: str) -> list[TsFunction]:
    spans: list[TsFunction] = []
    header_re = re.compile(
        r"(?m)^[ \t]*(?:export\s+)?(?:public\s+|private\s+|protected\s+|static\s+|"
        r"abstract\s+|readonly\s+)*(?:async\s+)?(?:function\s+)?(?:get\s+|set\s+)?"
        r"(?:\*\s*)?([A-Za-z_$][\w$]*)\s*(?:<[^<>\n]*>)?\("
    )
    arrow_re = re.compile(
        r"(?m)^[ \t]*(?:export\s+)?(?:const|let|var)\s+([A-Za-z_$][\w$]*)\s*"
        r"(?::[^=\n]+)?=\s*(?:async\s*)?\("
    )
    for regex in (header_re, arrow_re):
        for match in regex.finditer(masked):
            name = match.group(1)
            if name in TS_KEYWORDS:
                continue
            paren_open = masked.index("(", match.end() - 1)
            depth = 0
            i = paren_open
            n = len(masked)
            while i < n:
                if masked[i] == "(":
                    depth += 1
                elif masked[i] == ")":
                    depth -= 1
                    if depth == 0:
                        break
                i += 1
            if i >= n:
                continue
            # Walk to the body's opening brace. A return-type annotation may
            # sit between `)` and `{` and may itself contain braces
            # (`Promise<{...}>`), parens, or brackets — the body brace is the
            # first `{` at zero nesting depth. Abort at a depth-0 `;`
            # (declaration without a body).
            j = i + 1
            nest = 0
            body_open = -1
            while j < n:
                ch = masked[j]
                if ch in "([{":
                    if ch == "{" and nest == 0:
                        body_open = j
                        break
                    nest += 1
                elif ch in ")]}":
                    nest -= 1
                elif ch == "<":
                    nest += 1
                elif ch == ">":
                    # `=>` in function-type annotations is an arrow, not a
                    # generic close.
                    if masked[j - 1] != "=":
                        nest -= 1
                elif ch == ";" and nest == 0:
                    break
                j += 1
            if body_open < 0:
                continue
            j = body_open
            depth = 0
            k = j
            while k < n:
                if masked[k] == "{":
                    depth += 1
                elif masked[k] == "}":
                    depth -= 1
                    if depth == 0:
                        break
                k += 1
            if k >= n:
                continue
            line = masked.count("\n", 0, match.start()) + 1
            spans.append(TsFunction(name, match.start(), k + 1, line))
    return spans


def innermost_ts_function(spans: list[TsFunction], offset: int) -> TsFunction | None:
    best: TsFunction | None = None
    for span in spans:
        if span.start <= offset < span.body_end:
            if best is None or (span.body_end - span.start) < (
                best.body_end - best.start
            ):
                best = span
    return best


def ts_import_map(
    text: str, generated_names: set[str], reexports: set[str]
) -> dict[str, str]:
    """Map local identifiers in this file to their generated type names."""
    resolved: dict[str, str] = {}
    for match in re.finditer(
        r"import\s+(?:type\s+)?\{([^}]*)\}\s*from\s*[\"']([^\"']+)[\"']",
        text,
        flags=re.DOTALL,
    ):
        body, source = match.groups()
        entries = [e.strip() for e in body.split(",") if e.strip()]
        pairs = []
        for entry in entries:
            entry = re.sub(r"^type\s+", "", entry)
            alias_match = re.match(r"^([\w$]+)\s+as\s+([\w$]+)$", entry)
            if alias_match:
                pairs.append((alias_match.group(1), alias_match.group(2)))
            elif re.match(r"^[\w$]+$", entry):
                pairs.append((entry, entry))
        if "generated/" in source:
            for orig, local in pairs:
                if orig in generated_names:
                    resolved[local] = orig
        elif source.rstrip(".js").endswith("/types") or source in (
            "./types.js",
            "./types",
        ):
            for orig, local in pairs:
                if orig in reexports:
                    resolved[local] = orig
    return resolved


def ts_types_reexports(root: pathlib.Path, generated_names: set[str]) -> set[str]:
    """Names sdks/typescript/src/types.ts re-exports verbatim from generated."""
    path = root / "sdks" / "typescript" / "src" / "types.ts"
    text = path.read_text(encoding="utf-8")
    names: set[str] = set()
    for match in re.finditer(
        r"export\s+(?:type\s+)?\{([^}]*)\}\s*from\s*[\"']([^\"']+)[\"']",
        text,
        flags=re.DOTALL,
    ):
        body, source = match.groups()
        if "generated/" not in source:
            continue
        for entry in body.split(","):
            entry = re.sub(r"^\s*type\s+", "", entry.strip())
            if re.match(r"^[\w$]+$", entry) and entry in generated_names:
                names.add(entry)
    return names


@dataclass
class SendSite:
    file: pathlib.Path
    line: int
    wrapper: str  # enclosing function name (or "<module>")
    referenced: set[str] = field(default_factory=set)  # generated-resolved ids


def ts_send_sites(
    root: pathlib.Path,
    catalog: dict[str, dict[str, str | None]],
    generated_names: set[str],
) -> dict[str, list[SendSite]]:
    src = root / "sdks" / "typescript" / "src"
    reexports = ts_types_reexports(root, generated_names)
    sites: dict[str, list[SendSite]] = {}
    for path in sorted(src.rglob("*.ts")):
        if "generated" in path.parts:
            continue
        text = path.read_text(encoding="utf-8")
        masked = mask_ts(text)
        spans = ts_function_spans(masked)
        resolved_imports = ts_import_map(text, generated_names, reexports)

        def record(method: str, offset: int) -> None:
            func = innermost_ts_function(spans, offset)
            if func is None:
                wrapper, referenced = "<module>", set()
            else:
                body = masked[func.start : func.body_end]
                tokens = set(re.findall(r"[A-Za-z_$][\w$]*", body))
                referenced = {
                    resolved_imports[token]
                    for token in tokens
                    if token in resolved_imports
                }
                wrapper = func.name
            line = text.count("\n", 0, offset) + 1
            sites.setdefault(method, []).append(
                SendSite(path.relative_to(root), line, wrapper, referenced)
            )

        # Dispatch-helper calls: request("m", ...), request<T>("m", ...), and
        # subscription helpers that take open/close method names as string
        # arguments. Catalog-method string literals inside the argument list
        # count as send-sites.
        helper_re = re.compile(
            r"\.(?:request|openEventSubscription)\s*(?:<[^(]*?>)?\s*\("
        )
        for match in helper_re.finditer(masked):
            open_paren = match.end() - 1
            depth = 0
            end = open_paren
            while end < len(masked):
                if masked[end] == "(":
                    depth += 1
                elif masked[end] == ")":
                    depth -= 1
                    if depth == 0:
                        break
                end += 1
            for literal in re.finditer(r"\"([^\"]+)\"", text[open_paren:end]):
                method = literal.group(1)
                if method in catalog:
                    record(method, open_paren + literal.start())

        # JSON-RPC envelope literals (streaming paths build the frame inline).
        for match in re.finditer(r"\bmethod:\s*\"([^\"]+)\"", text):
            if match.group(1) in catalog:
                record(match.group(1), match.start())
    return sites


def web_auth_send_sites(
    root: pathlib.Path,
    catalog: dict[str, dict[str, str | None]],
    generated_names: set[str],
) -> dict[str, list[SendSite]]:
    """Send-sites for the intentional Web SDK auth RPC wrapper."""
    auth_path = root / "sdks" / "web" / "src" / "auth.ts"
    methods_path = root / "sdks" / "web" / "src" / "generated" / "auth.ts"
    text = auth_path.read_text(encoding="utf-8")
    methods_text = methods_path.read_text(encoding="utf-8")
    constant_to_method = {
        key: method
        for key, method in re.findall(r"(\w+):\s*[\"']([^\"']+)[\"']", methods_text)
        if method in catalog
    }

    masked = mask_ts(text)
    spans = ts_function_spans(masked)
    resolved_imports = ts_import_map(text, generated_names, set())
    sites: dict[str, list[SendSite]] = {}

    for match in re.finditer(r"\bAUTH_RPC_METHODS\.(\w+)\b", text):
        method = constant_to_method.get(match.group(1))
        if method is None:
            continue
        func = innermost_ts_function(spans, match.start())
        if func is None:
            wrapper, referenced = "<module>", set()
        else:
            body = masked[func.start : func.body_end]
            tokens = set(re.findall(r"[A-Za-z_$][\w$]*", body))
            referenced = {
                resolved_imports[token] for token in tokens if token in resolved_imports
            }
            wrapper = func.name
        line = text.count("\n", 0, match.start()) + 1
        sites.setdefault(method, []).append(
            SendSite(auth_path.relative_to(root), line, wrapper, referenced)
        )
    return sites


# ---------------------------------------------------------------------------
# Python analysis (ast-based).
# ---------------------------------------------------------------------------


def py_module_generated_imports(
    text: str,
    generated_names: set[str],
    reexport_modules: dict[str, dict[str, str]],
) -> dict[str, str]:
    import ast

    resolved: dict[str, str] = {}
    tree = ast.parse(text)
    for node in ast.walk(tree):
        if not isinstance(node, ast.ImportFrom) or node.module is None:
            continue
        module = node.module
        if node.level and not module.startswith("."):
            module = "." * node.level + module
        if "generated" in module:
            for alias in node.names:
                if alias.name in generated_names:
                    resolved[alias.asname or alias.name] = alias.name
        elif module.lstrip(".") in reexport_modules:
            exported = reexport_modules[module.lstrip(".")]
            for alias in node.names:
                if alias.name in exported:
                    resolved[alias.asname or alias.name] = exported[alias.name]
    return resolved


def py_send_sites(
    root: pathlib.Path,
    catalog: dict[str, dict[str, str | None]],
    generated_names: set[str],
) -> dict[str, list[SendSite]]:
    import ast

    pkg = root / "sdks" / "python" / "meerkat"

    # First pass: which generated names do meerkat-local modules (types.py,
    # mob.py, ...) import directly? Importing through such a re-exporting
    # module still counts as referencing the generated type.
    reexport_modules: dict[str, dict[str, str]] = {}
    for path in sorted(pkg.glob("*.py")):
        text = path.read_text(encoding="utf-8")
        direct = py_module_generated_imports(text, generated_names, {})
        reexport_modules[path.stem] = direct

    sites: dict[str, list[SendSite]] = {}
    for path in sorted(pkg.glob("*.py")):
        text = path.read_text(encoding="utf-8")
        tree = ast.parse(text)
        resolved_imports = py_module_generated_imports(
            text, generated_names, reexport_modules
        )

        functions: list[ast.AST] = [
            node
            for node in ast.walk(tree)
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
        ]

        def innermost(lineno: int):
            best = None
            for fn in functions:
                if fn.lineno <= lineno <= (fn.end_lineno or fn.lineno):
                    if best is None or (
                        (fn.end_lineno or 0) - fn.lineno
                        < (best.end_lineno or 0) - best.lineno
                    ):
                        best = fn
            return best

        def fn_referenced(fn) -> set[str]:
            names: set[str] = set()
            for node in ast.walk(fn):
                if isinstance(node, ast.Name):
                    names.add(node.id)
                elif isinstance(node, ast.Attribute):
                    names.add(node.attr)
                elif isinstance(node, ast.Constant) and isinstance(node.value, str):
                    # String annotations under `from __future__ import annotations`.
                    if re.fullmatch(r"[A-Za-z_][\w\[\], .|]*", node.value or " "):
                        names.update(re.findall(r"[A-Za-z_]\w*", node.value))
            return {
                resolved_imports[name] for name in names if name in resolved_imports
            }

        def record(method: str, lineno: int) -> None:
            fn = innermost(lineno)
            if fn is None:
                wrapper, referenced = "<module>", set()
            else:
                wrapper, referenced = fn.name, fn_referenced(fn)
            sites.setdefault(method, []).append(
                SendSite(path.relative_to(root), lineno, wrapper, referenced)
            )

        for node in ast.walk(tree):
            if isinstance(node, ast.Call):
                func = node.func
                attr = (
                    func.attr
                    if isinstance(func, ast.Attribute)
                    else func.id
                    if isinstance(func, ast.Name)
                    else None
                )
                if attr in (
                    "_request",
                    "request",
                    "_open_event_subscription",
                ):
                    # Catalog-method string literals passed positionally to a
                    # dispatch helper count as send-sites (subscription
                    # helpers take open/close method names as arguments).
                    for arg in node.args:
                        if (
                            isinstance(arg, ast.Constant)
                            and isinstance(arg.value, str)
                            and arg.value in catalog
                        ):
                            record(arg.value, arg.lineno)
            elif isinstance(node, ast.Dict):
                for key, value in zip(node.keys, node.values):
                    if (
                        isinstance(key, ast.Constant)
                        and key.value == "method"
                        and isinstance(value, ast.Constant)
                        and isinstance(value.value, str)
                        and value.value in catalog
                    ):
                        record(value.value, node.lineno)
    return sites


def _rpc_schema_documents(root: pathlib.Path) -> list[dict]:
    documents: list[dict] = []
    for name in ("params.json", "wire-types.json", "runtime-host.json"):
        path = root / "artifacts" / "schemas" / name
        if path.exists():
            value = json.loads(path.read_text(encoding="utf-8"))
            if isinstance(value, dict):
                documents.append(value)
    return documents


def _find_named_schema(documents: list[dict], name: str) -> dict | None:
    for document in documents:
        direct = document.get(name)
        if isinstance(direct, dict):
            return direct
        defs = document.get("$defs")
        if isinstance(defs, dict) and isinstance(defs.get(name), dict):
            return defs[name]
        for value in document.values():
            if not isinstance(value, dict):
                continue
            local_defs = value.get("$defs")
            if isinstance(local_defs, dict) and isinstance(local_defs.get(name), dict):
                return local_defs[name]
    return None


def _required_key_variants(
    schema: dict | None,
    documents: list[dict],
    seen: frozenset[str] = frozenset(),
) -> list[frozenset[str]]:
    if not isinstance(schema, dict):
        return [frozenset()]
    ref = schema.get("$ref")
    if isinstance(ref, str):
        name = ref.rsplit("/", 1)[-1]
        if name in seen:
            return [frozenset()]
        return _required_key_variants(
            _find_named_schema(documents, name), documents, seen | {name}
        )

    outer = frozenset(key for key in schema.get("required", []) if isinstance(key, str))
    for branch_key in ("oneOf", "anyOf"):
        branches = schema.get(branch_key)
        if isinstance(branches, list) and branches:
            variants = [
                outer | required
                for branch in branches
                for required in _required_key_variants(branch, documents, seen)
            ]
            return list(dict.fromkeys(variants))

    branches = schema.get("allOf")
    if isinstance(branches, list) and branches:
        variants = [outer]
        for branch in branches:
            variants = [
                current | required
                for current in variants
                for required in _required_key_variants(branch, documents, seen)
            ]
        return list(dict.fromkeys(variants))
    return [outer]


def _annotation_names(annotation: ast.expr | None) -> set[str]:
    if annotation is None:
        return set()
    return {node.id for node in ast.walk(annotation) if isinstance(node, ast.Name)} | {
        node.attr for node in ast.walk(annotation) if isinstance(node, ast.Attribute)
    }


def _python_payload_shape(
    expression: ast.expr,
    function: ast.FunctionDef | ast.AsyncFunctionDef,
    before_line: int,
    seen: frozenset[str] = frozenset(),
) -> tuple[set[str], set[str]]:
    if isinstance(expression, ast.Dict):
        keys: set[str] = set()
        annotations: set[str] = set()
        for key, value in zip(expression.keys, expression.values):
            if isinstance(key, ast.Constant) and isinstance(key.value, str):
                keys.add(key.value)
            elif key is None:
                nested_keys, nested_annotations = _python_payload_shape(
                    value, function, before_line, seen
                )
                keys.update(nested_keys)
                annotations.update(nested_annotations)
        return keys, annotations

    if isinstance(expression, ast.DictComp):
        keys: set[str] = set()
        annotations: set[str] = set()
        for generator in expression.generators:
            iterator = generator.iter
            if (
                isinstance(iterator, ast.Call)
                and isinstance(iterator.func, ast.Attribute)
                and iterator.func.attr == "items"
            ):
                nested_keys, nested_annotations = _python_payload_shape(
                    iterator.func.value, function, before_line, seen
                )
                keys.update(nested_keys)
                annotations.update(nested_annotations)
        return keys, annotations

    if isinstance(expression, ast.Call):
        keys = {keyword.arg for keyword in expression.keywords if keyword.arg}
        annotations: set[str] = set()
        for keyword in expression.keywords:
            if keyword.arg is None:
                nested_keys, nested_annotations = _python_payload_shape(
                    keyword.value, function, before_line, seen
                )
                keys.update(nested_keys)
                annotations.update(nested_annotations)
        if expression.args:
            nested_keys, nested_annotations = _python_payload_shape(
                expression.args[0], function, before_line, seen
            )
            keys.update(nested_keys)
            annotations.update(nested_annotations)
        return keys, annotations

    if isinstance(expression, ast.BoolOp):
        keys: set[str] = set()
        annotations: set[str] = set()
        for value in expression.values:
            nested_keys, nested_annotations = _python_payload_shape(
                value, function, before_line, seen
            )
            keys.update(nested_keys)
            annotations.update(nested_annotations)
        return keys, annotations

    if isinstance(expression, ast.Attribute) and expression.attr == "__dict__":
        return _python_payload_shape(expression.value, function, before_line, seen)

    if not isinstance(expression, ast.Name) or expression.id in seen:
        return set(), set()

    name = expression.id
    keys: set[str] = set()
    annotations: set[str] = set()
    for argument in (*function.args.args, *function.args.kwonlyargs):
        if argument.arg == name:
            annotations.update(_annotation_names(argument.annotation))

    for node in ast.walk(function):
        if getattr(node, "lineno", before_line) >= before_line:
            continue
        value: ast.expr | None = None
        if isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            if node.target.id == name:
                value = node.value
        elif isinstance(node, ast.Assign):
            if any(
                isinstance(target, ast.Name) and target.id == name
                for target in node.targets
            ):
                value = node.value
            for target in node.targets:
                if (
                    isinstance(target, ast.Subscript)
                    and isinstance(target.value, ast.Name)
                    and target.value.id == name
                    and isinstance(target.slice, ast.Constant)
                    and isinstance(target.slice.value, str)
                ):
                    keys.add(target.slice.value)
        elif (
            isinstance(node, ast.Call)
            and isinstance(node.func, ast.Attribute)
            and isinstance(node.func.value, ast.Name)
            and node.func.value.id == name
            and node.func.attr == "update"
            and node.args
        ):
            value = node.args[0]

        if value is not None:
            nested_keys, nested_annotations = _python_payload_shape(
                value, function, before_line, seen | {name}
            )
            keys.update(nested_keys)
            annotations.update(nested_annotations)
    return keys, annotations


def check_python_required_param_shapes(
    root: pathlib.Path, catalog: dict[str, dict[str, str | None]]
) -> list[str]:
    """Require every literal Python send to satisfy a generated params shape.

    This is intentionally AST-based rather than an annotation-presence check:
    dictionary literals, named payloads, constructor calls, and conditional
    key additions are inspected at the actual request call. Direct pass-through
    is accepted only when the public argument itself names the catalog params
    type; the transport cast alone can never satisfy this gate.
    """
    documents = _rpc_schema_documents(root)
    required_by_method: dict[str, list[frozenset[str]]] = {}
    refs_by_method: dict[str, set[str]] = {}
    for method, descriptor in catalog.items():
        refs = set(split_type_refs(descriptor["params"]))
        refs_by_method[method] = refs
        variants: list[frozenset[str]] = []
        for ref in refs:
            if ref in UNTYPED_REFS:
                variants.append(frozenset())
                continue
            variants.extend(
                _required_key_variants(_find_named_schema(documents, ref), documents)
            )
        required_by_method[method] = variants or [frozenset()]

    failures: list[str] = []
    package = root / "sdks" / "python" / "meerkat"
    for path in sorted(package.glob("*.py")):
        tree = ast.parse(path.read_text(encoding="utf-8"))
        functions = [
            node
            for node in ast.walk(tree)
            if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef))
        ]

        def innermost(lineno: int):
            candidates = [
                function
                for function in functions
                if function.lineno <= lineno <= (function.end_lineno or function.lineno)
            ]
            return min(
                candidates,
                key=lambda function: (function.end_lineno or 0) - function.lineno,
                default=None,
            )

        for node in ast.walk(tree):
            if not (
                isinstance(node, ast.Call)
                and isinstance(node.func, ast.Attribute)
                and node.func.attr in ("_request", "request")
                and len(node.args) >= 2
                and isinstance(node.args[0], ast.Constant)
                and isinstance(node.args[0].value, str)
                and node.args[0].value in catalog
            ):
                continue
            function = innermost(node.lineno)
            if function is None:
                continue
            method = node.args[0].value
            keys, annotations = _python_payload_shape(
                node.args[1], function, node.lineno
            )
            if any(required <= keys for required in required_by_method[method]):
                continue
            if annotations & refs_by_method[method]:
                continue
            expected = " or ".join(
                "{" + ", ".join(sorted(required)) + "}"
                for required in required_by_method[method]
            )
            failures.append(
                f"py: `{method}` params shape drift at "
                f"{path.relative_to(root)}:{node.lineno} - payload keys "
                f"{sorted(keys)} do not satisfy generated required fields "
                f"{expected}; a transport cast or unrelated type reference "
                "does not count"
            )
    return failures


# ---------------------------------------------------------------------------
# Enforcement.
# ---------------------------------------------------------------------------


def _compact_type(value: str) -> str:
    return re.sub(r"\s+", " ", value).strip()


def _expected_contract_type(
    type_ref: str | None,
    generated: set[str],
    *,
    sdk: str,
    params: bool,
) -> str:
    if not type_ref:
        return (
            "Record<string, never>"
            if sdk == "ts" and params
            else ("Record<string, unknown>" if sdk == "ts" else "dict[str, Any]")
        )
    rendered: list[str] = []
    for part in split_type_refs(type_ref):
        if part == "Value" or part not in generated:
            rendered.append(
                "Record<string, unknown>"
                if sdk == "ts" and params
                else "unknown"
                if sdk == "ts"
                else "dict[str, Any]"
                if params
                else "Any"
            )
        else:
            rendered.append(part)
    joined = " | ".join(rendered)
    if sdk == "ts" and not params and joined != "unknown":
        return f"({joined}) & Record<string, unknown>"
    return joined


def check_generated_transport_contracts(
    root: pathlib.Path,
    catalog: dict[str, dict[str, str | None]],
    *,
    sdk: str,
    generated: set[str],
) -> list[str]:
    failures: list[str] = []
    if sdk == "ts":
        contracts_path = root / "sdks/typescript/src/generated/rpc_contracts.ts"
        client_path = root / "sdks/typescript/src/client.ts"
        contracts = contracts_path.read_text(encoding="utf-8")
        client = client_path.read_text(encoding="utf-8")
        entries = {
            name: (_compact_type(params), _compact_type(result))
            for name, params, result in re.findall(
                r'^\s*"([^"]+)":\s*\{\s*params:\s*(.*?);\s*'
                r"result:\s*(.*?);\s*\};",
                contracts,
                flags=re.MULTILINE | re.DOTALL,
            )
        }
        transport_patterns = (
            r"request<M extends RpcMethodName>",
            r"params:\s*RpcMethodContracts\[M\]\[\"params\"\]",
            r"Promise<RpcMethodContracts\[M\]\[\"result\"\]>",
        )
        for pattern in transport_patterns:
            if re.search(pattern, client) is None:
                failures.append(
                    "ts: client request transport is not generic over the "
                    f"generated RpcMethodContracts map (missing `{pattern}`)"
                )
    else:
        contracts_path = root / "sdks/python/meerkat/generated/rpc_contracts.py"
        client_path = root / "sdks/python/meerkat/client.py"
        contracts = contracts_path.read_text(encoding="utf-8")
        client = client_path.read_text(encoding="utf-8")
        entries = {
            name: (_compact_type(params), _compact_type(result))
            for name, params, result in re.findall(
                r"method:\s*Literal\[\"([^\"]+)\"\],\s*"
                r"params:\s*(.*?),\s*/,.+?Awaitable\[(.*?)\]",
                contracts,
                flags=re.DOTALL,
            )
        }
        transport_patterns = (
            r"_request:\s*RpcRequest",
            r"self\._request\s*=\s*cast\(RpcRequest,\s*self\._request_impl\)",
            r"async def _request_impl\(",
        )
        for pattern in transport_patterns:
            if re.search(pattern, client) is None:
                failures.append(
                    "py: client request transport is not bound to the generated "
                    f"RpcRequest overloads (missing `{pattern}`)"
                )

    expected_names = set(catalog)
    actual_names = set(entries)
    for name in sorted(expected_names - actual_names):
        failures.append(f"{sdk}: generated RPC contract missing `{name}`")
    for name in sorted(actual_names - expected_names):
        failures.append(
            f"{sdk}: generated RPC contract contains non-catalog method `{name}`"
        )
    for name in sorted(expected_names & actual_names):
        actual_params, actual_result = entries[name]
        expected_params = _expected_contract_type(
            catalog[name]["params"], generated, sdk=sdk, params=True
        )
        expected_result = _expected_contract_type(
            catalog[name]["result"], generated, sdk=sdk, params=False
        )
        if actual_params != expected_params:
            failures.append(
                f"{sdk}: `{name}` params contract drift - expected "
                f"`{expected_params}`, generated `{actual_params}`"
            )
        if actual_result != expected_result:
            failures.append(
                f"{sdk}: `{name}` result contract drift - expected "
                f"`{expected_result}`, generated `{actual_result}`"
            )

    marker_patterns = (
        "_RpcSignature",
        "_RpcGeneratedSignature",
        "_rpc_signature",
        "_rpc_generated_signature",
    )
    for marker in marker_patterns:
        if marker in client:
            failures.append(
                f"{sdk}: obsolete wrapper-local marker `{marker}` is forbidden; "
                "bind the actual request transport instead"
            )
    return failures


def check_send_site_coverage(
    sdk: str,
    catalog: dict[str, dict[str, str | None]],
    sites: dict[str, list[SendSite]],
) -> list[str]:
    failures: list[str] = []
    exclusions = SDK_SEND_SITE_EXCLUSIONS[sdk]
    for method in sorted(catalog):
        if not sites.get(method) and method not in exclusions:
            failures.append(
                f"{sdk}: no structural send-site found for catalog method `{method}`"
            )
        if sites.get(method) and method in exclusions:
            failures.append(f"{sdk}: `{method}` has a send-site but remains excluded")
    return failures


def check_sdk(
    sdk: str,
    catalog: dict[str, dict[str, str | None]],
    sites: dict[str, list[SendSite]],
    generated: set[str],
) -> tuple[list[str], int, int]:
    failures: list[str] = []
    enforced = 0
    untracked = 0
    exclusions = SDK_SEND_SITE_EXCLUSIONS[sdk]

    for method in sorted(catalog):
        method_sites = sites.get(method, [])
        if not method_sites:
            if method not in exclusions:
                failures.append(
                    f"{sdk}: no structural send-site found for catalog method "
                    f'`{method}` (expected a request("{method}", ...) call '
                    "or a JSON-RPC envelope literal in the SDK source)"
                )
            continue
        if method in exclusions:
            failures.append(
                f"{sdk}: `{method}` is listed in SDK_SEND_SITE_EXCLUSIONS but "
                "a send-site exists — remove the stale exclusion"
            )
        for side in ("params", "result"):
            refs = [
                r
                for r in split_type_refs(catalog[method][side])
                if r not in UNTYPED_REFS
            ]
            if not refs:
                continue
            available = [r for r in refs if r in generated]
            if len(available) != len(refs):
                untracked += 1
                continue
            enforced += 1
            compliant = any(
                all(ref in site.referenced for ref in refs) for site in method_sites
            )
            if not compliant:
                locations = ", ".join(
                    f"{s.wrapper} ({s.file}:{s.line})" for s in method_sites
                )
                failures.append(
                    f"{sdk}: `{method}` {side} type drift — catalog declares "
                    f"`{catalog[method][side]}` and the generated SDK type "
                    f"exists, but no wrapper send-site references it "
                    f"(ad-hoc shape). Send-sites: {locations}. Expected a "
                    f"reference to generated type(s) {refs} inside the wrapper."
                )
    return failures, enforced, untracked


def main() -> int:
    if len(sys.argv) != 2:
        print("Usage: verify_rpc_signature_parity.py REPO_ROOT", file=sys.stderr)
        return 2

    root = pathlib.Path(sys.argv[1]).resolve()
    catalog = load_catalog(root)
    if not catalog:
        print("Could not load methods from artifacts/schemas/rpc-methods.json")
        return 1

    failures = check_docs(root, catalog)

    ts_gen = ts_generated_names(root)
    py_gen = py_generated_names(root)
    web_gen = web_generated_names(root)
    ts_sites = ts_send_sites(root, catalog, ts_gen)
    py_sites_map = py_send_sites(root, catalog, py_gen)
    web_auth_sites = web_auth_send_sites(root, catalog, web_gen)

    ts_failures = check_generated_transport_contracts(
        root, catalog, sdk="ts", generated=ts_gen
    )
    ts_failures.extend(check_send_site_coverage("ts", catalog, ts_sites))
    py_failures = check_generated_transport_contracts(
        root, catalog, sdk="py", generated=py_gen
    )
    py_failures.extend(check_send_site_coverage("py", catalog, py_sites_map))
    py_failures.extend(check_python_required_param_shapes(root, catalog))
    auth_catalog = {
        method: descriptor
        for method, descriptor in catalog.items()
        if method.startswith("auth/")
    }
    web_failures, web_enforced, web_untracked = check_sdk(
        "web-auth", auth_catalog, web_auth_sites, web_gen
    )
    failures.extend(ts_failures)
    failures.extend(py_failures)
    failures.extend(web_failures)

    if failures:
        print("RPC signature parity violations:")
        for failure in failures:
            print(f"  - {failure}")
        return 1

    print(
        "RPC signature parity OK: "
        f"{len(catalog)} methods; docs typed columns match the catalog; "
        "TypeScript and Python request transports are bound to generated "
        "method contracts; wrapper-local marker aliases are absent; "
        f"web-auth={web_enforced} generated-typed sides enforced "
        f"({web_untracked} sides untracked because the generated Web SDK "
        "type does not exist yet)."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
