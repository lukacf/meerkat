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

  2. **SDK wrappers** (TypeScript + Python): for every catalog method we
     locate the wrapper send-site(s) — actual ``request("method", ...)`` call
     expressions or ``method: "..."`` JSON-RPC envelope literals, not prose —
     and, where the catalog's param/result type is available in that SDK's
     *generated* type module (``sdks/*/generated/``), require the enclosing
     wrapper function to reference the generated type identifier. A wrapper
     that hand-rolls its own ad-hoc shape for a method whose generated type
     exists fails.

Catalog truth is read from the committed ``artifacts/schemas/rpc-methods.json``
artifact (emitted from ``meerkat_contracts::rpc_method_catalog`` by
``make regen-schemas``); the sibling gate already pins that artifact against
the Rust catalog source, so this script never compiles anything.

Wrappers that hand-rolled shapes *before this gate existed* are grandfathered
in an explicit ratchet baseline below. The baseline only shrinks: adding a new
hand-rolled wrapper fails, and a baseline entry whose wrapper becomes
generated-typed fails as stale until the entry is deleted.
"""

from __future__ import annotations

import json
import pathlib
import re
import sys
from datetime import date
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

# ---------------------------------------------------------------------------
# RATCHET BASELINE — wrappers that hand-roll a shape although the generated
# SDK type for the catalog ref exists. These predate this gate. Do NOT add
# entries; delete an entry once its wrapper references the generated type.
# Key: (sdk, method, side) where side is "params" or "result".
# ---------------------------------------------------------------------------
BASELINE_HAND_ROLLED: set[tuple[str, str, str]] = {
    ("py", "auth/login/complete", "result"),
    ("py", "auth/login/device_start", "result"),
    ("py", "auth/login/start", "result"),
    ("py", "auth/logout", "result"),
    ("py", "auth/profile/create", "result"),
    ("py", "auth/profile/delete", "result"),
    ("py", "auth/profile/get", "result"),
    ("py", "auth/profile/list", "result"),
    ("py", "auth/status/get", "result"),
    ("py", "comms/peers", "params"),
    ("py", "comms/peers", "result"),
    ("py", "comms/send", "params"),
    ("py", "comms/send", "result"),
    ("py", "live/close", "params"),
    ("py", "live/commit_input", "params"),
    ("py", "live/commit_input", "result"),
    ("py", "live/interrupt", "params"),
    ("py", "live/interrupt", "result"),
    ("py", "live/open", "params"),
    ("py", "live/open", "result"),
    ("py", "live/refresh", "params"),
    ("py", "live/send_input", "params"),
    ("py", "live/send_input", "result"),
    ("py", "live/status", "params"),
    ("py", "live/status", "result"),
    ("py", "live/truncate", "params"),
    ("py", "live/truncate", "result"),
    ("py", "live/webrtc/answer", "params"),
    ("py", "live/webrtc/answer", "result"),
    ("py", "mcp/add", "params"),
    ("py", "mcp/reload", "params"),
    ("py", "mcp/remove", "params"),
    ("py", "mob/append_system_context", "params"),
    ("py", "mob/append_system_context", "result"),
    ("py", "mob/cancel_all_work", "params"),
    ("py", "mob/cancel_all_work", "result"),
    ("py", "mob/cancel_work", "params"),
    ("py", "mob/cancel_work", "result"),
    ("py", "mob/create", "params"),
    ("py", "mob/create", "result"),
    ("py", "mob/destroy", "params"),
    ("py", "mob/destroy", "result"),
    ("py", "mob/ensure_member", "params"),
    ("py", "mob/ensure_member", "result"),
    ("py", "mob/events", "params"),
    ("py", "mob/events", "result"),
    ("py", "mob/flow_cancel", "params"),
    ("py", "mob/flow_cancel", "result"),
    ("py", "mob/flow_run", "params"),
    ("py", "mob/flow_run", "result"),
    ("py", "mob/flow_status", "params"),
    ("py", "mob/flow_status", "result"),
    ("py", "mob/flows", "params"),
    ("py", "mob/flows", "result"),
    ("py", "mob/force_cancel", "params"),
    ("py", "mob/force_cancel", "result"),
    ("py", "mob/fork_helper", "params"),
    ("py", "mob/fork_helper", "result"),
    ("py", "mob/ingress_interaction", "params"),
    ("py", "mob/ingress_interaction", "result"),
    ("py", "mob/lifecycle", "params"),
    ("py", "mob/lifecycle", "result"),
    ("py", "mob/list", "result"),
    ("py", "mob/list_members_matching", "params"),
    ("py", "mob/list_members_matching", "result"),
    ("py", "mob/member_send", "params"),
    ("py", "mob/member_send", "result"),
    ("py", "mob/member_status", "params"),
    ("py", "mob/member_status", "result"),
    ("py", "mob/members", "params"),
    ("py", "mob/members", "result"),
    ("py", "mob/profile/create", "params"),
    ("py", "mob/profile/create", "result"),
    ("py", "mob/profile/delete", "params"),
    ("py", "mob/profile/delete", "result"),
    ("py", "mob/profile/get", "params"),
    ("py", "mob/profile/get", "result"),
    ("py", "mob/profile/list", "result"),
    ("py", "mob/profile/update", "params"),
    ("py", "mob/profile/update", "result"),
    ("py", "mob/reconcile", "params"),
    ("py", "mob/reconcile", "result"),
    ("py", "mob/respawn", "params"),
    ("py", "mob/respawn", "result"),
    ("py", "mob/retire", "params"),
    ("py", "mob/retire", "result"),
    ("py", "mob/rotate_supervisor", "params"),
    ("py", "mob/snapshot", "params"),
    ("py", "mob/snapshot", "result"),
    ("py", "mob/spawn", "params"),
    ("py", "mob/spawn", "result"),
    ("py", "mob/spawn_helper", "params"),
    ("py", "mob/spawn_helper", "result"),
    ("py", "mob/status", "params"),
    ("py", "mob/status", "result"),
    ("py", "mob/stream_close", "params"),
    ("py", "mob/stream_close", "result"),
    ("py", "mob/stream_open", "params"),
    ("py", "mob/stream_open", "result"),
    ("py", "mob/submit_work", "params"),
    ("py", "mob/submit_work", "result"),
    ("py", "mob/turn_start", "result"),
    ("py", "mob/unwire", "params"),
    ("py", "mob/unwire", "result"),
    ("py", "mob/wait_kickoff", "params"),
    ("py", "mob/wait_kickoff", "result"),
    ("py", "mob/wait_ready", "params"),
    ("py", "mob/wait_ready", "result"),
    ("py", "mob/wire", "params"),
    ("py", "mob/wire", "result"),
    ("py", "mob/wire_members_batch", "params"),
    ("py", "models/catalog", "result"),
    ("py", "realm/get", "result"),
    ("py", "realm/list", "result"),
    ("py", "schedule/delete", "params"),
    ("py", "schedule/get", "params"),
    ("py", "schedule/list", "params"),
    ("py", "schedule/list", "result"),
    ("py", "schedule/occurrences", "params"),
    ("py", "schedule/occurrences", "result"),
    ("py", "schedule/pause", "params"),
    ("py", "schedule/resume", "params"),
    ("py", "schedule/update", "params"),
    ("py", "session/external_event", "result"),
    ("py", "session/history", "result"),
    ("py", "session/peer_response_terminal", "result"),
    ("py", "session/read", "result"),
    ("py", "session/stream_close", "params"),
    ("py", "session/stream_close", "result"),
    ("py", "session/stream_open", "params"),
    ("py", "session/stream_open", "result"),
    ("py", "turn/start", "result"),
    ("ts", "auth/login/complete", "result"),
    ("ts", "auth/login/device_start", "result"),
    ("ts", "auth/login/start", "result"),
    ("ts", "auth/logout", "result"),
    ("ts", "auth/profile/create", "result"),
    ("ts", "auth/profile/delete", "result"),
    ("ts", "auth/profile/get", "result"),
    ("ts", "auth/profile/list", "result"),
    ("ts", "auth/status/get", "result"),
    ("ts", "comms/peers", "params"),
    ("ts", "comms/peers", "result"),
    ("ts", "comms/send", "params"),
    ("ts", "comms/send", "result"),
    ("ts", "mob/append_system_context", "params"),
    ("ts", "mob/append_system_context", "result"),
    ("ts", "mob/cancel_all_work", "params"),
    ("ts", "mob/cancel_all_work", "result"),
    ("ts", "mob/cancel_work", "params"),
    ("ts", "mob/cancel_work", "result"),
    ("ts", "mob/create", "params"),
    ("ts", "mob/create", "result"),
    ("ts", "mob/destroy", "params"),
    ("ts", "mob/destroy", "result"),
    ("ts", "mob/ensure_member", "params"),
    ("ts", "mob/ensure_member", "result"),
    ("ts", "mob/events", "params"),
    ("ts", "mob/events", "result"),
    ("ts", "mob/flow_cancel", "params"),
    ("ts", "mob/flow_cancel", "result"),
    ("ts", "mob/flow_run", "params"),
    ("ts", "mob/flow_run", "result"),
    ("ts", "mob/flow_status", "params"),
    ("ts", "mob/flow_status", "result"),
    ("ts", "mob/flows", "params"),
    ("ts", "mob/flows", "result"),
    ("ts", "mob/force_cancel", "params"),
    ("ts", "mob/force_cancel", "result"),
    ("ts", "mob/fork_helper", "params"),
    ("ts", "mob/fork_helper", "result"),
    ("ts", "mob/ingress_interaction", "params"),
    ("ts", "mob/ingress_interaction", "result"),
    ("ts", "mob/lifecycle", "params"),
    ("ts", "mob/lifecycle", "result"),
    ("ts", "mob/list", "result"),
    ("ts", "mob/list_members_matching", "params"),
    ("ts", "mob/list_members_matching", "result"),
    ("ts", "mob/member_send", "params"),
    ("ts", "mob/member_send", "result"),
    ("ts", "mob/member_status", "params"),
    ("ts", "mob/member_status", "result"),
    ("ts", "mob/members", "params"),
    ("ts", "mob/members", "result"),
    ("ts", "mob/profile/create", "params"),
    ("ts", "mob/profile/create", "result"),
    ("ts", "mob/profile/delete", "params"),
    ("ts", "mob/profile/delete", "result"),
    ("ts", "mob/profile/get", "params"),
    ("ts", "mob/profile/get", "result"),
    ("ts", "mob/profile/list", "result"),
    ("ts", "mob/profile/update", "params"),
    ("ts", "mob/profile/update", "result"),
    ("ts", "mob/reconcile", "params"),
    ("ts", "mob/reconcile", "result"),
    ("ts", "mob/respawn", "params"),
    ("ts", "mob/respawn", "result"),
    ("ts", "mob/retire", "params"),
    ("ts", "mob/retire", "result"),
    ("ts", "mob/rotate_supervisor", "params"),
    ("ts", "mob/snapshot", "params"),
    ("ts", "mob/snapshot", "result"),
    ("ts", "mob/spawn", "params"),
    ("ts", "mob/spawn", "result"),
    ("ts", "mob/spawn_helper", "params"),
    ("ts", "mob/spawn_helper", "result"),
    ("ts", "mob/status", "params"),
    ("ts", "mob/status", "result"),
    ("ts", "mob/stream_close", "params"),
    ("ts", "mob/stream_close", "result"),
    ("ts", "mob/stream_open", "params"),
    ("ts", "mob/stream_open", "result"),
    ("ts", "mob/submit_work", "params"),
    ("ts", "mob/submit_work", "result"),
    ("ts", "mob/turn_start", "params"),
    ("ts", "mob/turn_start", "result"),
    ("ts", "mob/unwire", "params"),
    ("ts", "mob/unwire", "result"),
    ("ts", "mob/wait_kickoff", "params"),
    ("ts", "mob/wait_kickoff", "result"),
    ("ts", "mob/wait_ready", "params"),
    ("ts", "mob/wait_ready", "result"),
    ("ts", "mob/wire", "params"),
    ("ts", "mob/wire", "result"),
    ("ts", "mob/wire_members_batch", "params"),
    ("ts", "mob/wire_members_batch", "result"),
    ("ts", "models/catalog", "result"),
    ("ts", "realm/get", "result"),
    ("ts", "realm/list", "result"),
    ("ts", "schedule/delete", "params"),
    ("ts", "schedule/get", "params"),
    ("ts", "schedule/list", "params"),
    ("ts", "schedule/list", "result"),
    ("ts", "schedule/occurrences", "params"),
    ("ts", "schedule/occurrences", "result"),
    ("ts", "schedule/pause", "params"),
    ("ts", "schedule/resume", "params"),
    ("ts", "schedule/update", "params"),
    ("ts", "session/external_event", "result"),
    ("ts", "session/history", "result"),
    ("ts", "session/peer_response_terminal", "result"),
    ("ts", "session/read", "result"),
    ("ts", "session/stream_close", "params"),
    ("ts", "session/stream_close", "result"),
    ("ts", "session/stream_open", "params"),
    ("ts", "session/stream_open", "result"),
    ("ts", "turn/start", "result"),
}

# The remaining grandfathered wrappers are owned debt, not a permanent escape
# hatch. New generated wrappers must remove entries; the whole baseline expires
# unless renewed with an explicit owner decision.
BASELINE_HAND_ROLLED_OWNER = "sdk-contracts"
BASELINE_HAND_ROLLED_EXPIRES = "2026-07-31"
BASELINE_HAND_ROLLED_MAX_ENTRIES = 247


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
    overview = re.search(
        r"## Method overview(.*?)\n## ", docs_text, flags=re.DOTALL
    )
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
    template_depth: list[int] = []  # brace depth inside `${ ... }` per template

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


def innermost_ts_function(
    spans: list[TsFunction], offset: int
) -> TsFunction | None:
    best: TsFunction | None = None
    for span in spans:
        if span.start <= offset < span.body_end:
            if best is None or (span.body_end - span.start) < (
                best.body_end - best.start
            ):
                best = span
    return best


def ts_import_map(text: str, generated_names: set[str], reexports: set[str]) -> set[str]:
    """Local identifiers in this file that resolve to generated types."""
    resolved: set[str] = set()
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
                    resolved.add(local)
        elif source.rstrip(".js").endswith("/types") or source in (
            "./types.js",
            "./types",
        ):
            for orig, local in pairs:
                if orig in reexports:
                    resolved.add(local)
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
                referenced = tokens & resolved_imports
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
            referenced = tokens & resolved_imports
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
    text: str, generated_names: set[str], reexport_modules: dict[str, set[str]]
) -> set[str]:
    import ast

    resolved: set[str] = set()
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
                    resolved.add(alias.asname or alias.name)
        elif module.lstrip(".") in reexport_modules:
            exported = reexport_modules[module.lstrip(".")]
            for alias in node.names:
                if alias.name in exported:
                    resolved.add(alias.asname or alias.name)
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
    reexport_modules: dict[str, set[str]] = {}
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
            return names & resolved_imports

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


# ---------------------------------------------------------------------------
# Enforcement.
# ---------------------------------------------------------------------------


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
                    f"`{method}` (expected a request(\"{method}\", ...) call "
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
                all(ref in site.referenced for ref in refs)
                for site in method_sites
            )
            key = (sdk, method, side)
            if compliant and key in BASELINE_HAND_ROLLED:
                failures.append(
                    f"{sdk}: `{method}` {side} — baseline entry is stale: the "
                    f"wrapper now references the generated type(s) "
                    f"{refs}; delete {key!r} from BASELINE_HAND_ROLLED"
                )
            elif not compliant and key not in BASELINE_HAND_ROLLED:
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

    ts_failures, ts_enforced, ts_untracked = check_sdk("ts", catalog, ts_sites, ts_gen)
    py_failures, py_enforced, py_untracked = check_sdk(
        "py", catalog, py_sites_map, py_gen
    )
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

    # Baseline keys must stay real: every entry must refer to a live method.
    for sdk, method, side in sorted(BASELINE_HAND_ROLLED):
        if method not in catalog:
            failures.append(
                f"{sdk}: baseline entry ({sdk!r}, {method!r}, {side!r}) refers "
                "to a method that is no longer in the catalog — delete it"
            )

    if not BASELINE_HAND_ROLLED_OWNER:
        failures.append("baseline policy: BASELINE_HAND_ROLLED_OWNER must be set")
    try:
        expiry = date.fromisoformat(BASELINE_HAND_ROLLED_EXPIRES)
    except ValueError:
        failures.append(
            "baseline policy: BASELINE_HAND_ROLLED_EXPIRES must be an ISO date"
        )
    else:
        if date.today() > expiry:
            failures.append(
                "baseline policy: BASELINE_HAND_ROLLED expired on "
                f"{BASELINE_HAND_ROLLED_EXPIRES}; remove entries or renew with "
                "an explicit owner decision"
            )
    if len(BASELINE_HAND_ROLLED) > BASELINE_HAND_ROLLED_MAX_ENTRIES:
        failures.append(
            "baseline policy: BASELINE_HAND_ROLLED grew to "
            f"{len(BASELINE_HAND_ROLLED)} entries, above the ratchet cap "
            f"{BASELINE_HAND_ROLLED_MAX_ENTRIES}"
        )

    if failures:
        print("RPC signature parity violations:")
        for failure in failures:
            print(f"  - {failure}")
        return 1

    baseline_count = len(BASELINE_HAND_ROLLED)
    print(
        "RPC signature parity OK: "
        f"{len(catalog)} methods; docs typed columns match the catalog; "
        f"ts={ts_enforced} py={py_enforced} web-auth={web_enforced} "
        "generated-typed sides enforced "
        f"({baseline_count} grandfathered hand-rolled, "
        f"owner={BASELINE_HAND_ROLLED_OWNER} expires={BASELINE_HAND_ROLLED_EXPIRES}, "
        f"ts={ts_untracked} py={py_untracked} web-auth={web_untracked} "
        "sides untracked because the "
        "generated SDK type does not exist yet)."
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
