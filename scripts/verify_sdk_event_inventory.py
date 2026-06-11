#!/usr/bin/env python3
"""Verify the generated SDK event-type inventories match the schema authority.

The canonical event-type inventory lives in
``artifacts/schemas/events.json`` under ``WireEvent.known_event_types``. Each
SDK ships a *generated* copy of that inventory that drives its fail-closed
event parser:

  * web        -> sdks/web/src/generated/events.ts          (KNOWN_AGENT_EVENT_TYPES)
  * typescript -> sdks/typescript/src/generated/events.ts   (KNOWN_AGENT_EVENT_TYPES)
  * python     -> sdks/python/meerkat/generated/event_inventory.py (KNOWN_AGENT_EVENT_TYPES)

This gate asserts that every schema-known event type has an entry in every
SDK's generated inventory (and that no SDK advertises an event type the schema
does not declare). It fails closed so that adding an event type to the wire
contract without re-running ``make regen-schemas`` (which regenerates the SDK
inventories) breaks CI instead of silently shipping an SDK that would reject a
contract-valid event as "unknown".

This complements the broader rerun-and-diff gate
(``scripts/verify-sdk-codegen-freshness``): this script is a focused,
semantic assertion that does not require re-running the full code generator.
"""

from __future__ import annotations

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
EVENTS_SCHEMA = ROOT / "artifacts" / "schemas" / "events.json"

WEB_EVENTS = ROOT / "sdks" / "web" / "src" / "generated" / "events.ts"
TS_EVENTS = ROOT / "sdks" / "typescript" / "src" / "generated" / "events.ts"
PY_EVENTS = ROOT / "sdks" / "python" / "meerkat" / "generated" / "event_inventory.py"


def schema_known_event_types() -> set[str]:
    data = json.loads(EVENTS_SCHEMA.read_text(encoding="utf-8"))
    wire_event = data.get("WireEvent", {})
    values = wire_event.get("known_event_types")
    if not isinstance(values, list) or not all(isinstance(v, str) for v in values):
        raise SystemExit(
            f"{EVENTS_SCHEMA}: WireEvent.known_event_types must be a string array"
        )
    return set(values)


def _literal_body(text: str, const_name: str) -> str:
    """Extract the array/set literal body assigned to ``const_name``.

    Works for both the TS ``export const KNOWN_AGENT_EVENT_TYPES = [...]`` and
    the Python ``KNOWN_AGENT_EVENT_TYPES: frozenset[str] = frozenset({...})``
    forms. We locate the assignment ``=`` after the name (so a ``frozenset[str]``
    type annotation is skipped), then capture from the first ``[`` or ``{`` of
    the right-hand-side value to its matching closer.
    """
    idx = text.find(const_name)
    if idx == -1:
        raise SystemExit(f"{const_name} not found")
    eq = text.find("=", idx)
    if eq == -1:
        raise SystemExit(f"{const_name}: no assignment found")
    rest = text[eq:]
    open_match = re.search(r"[\[{]", rest)
    if not open_match:
        raise SystemExit(f"{const_name}: no array/set literal found")
    opener = rest[open_match.start()]
    closer = "]" if opener == "[" else "}"
    body = rest[open_match.start() + 1:]
    close_idx = body.find(closer)
    if close_idx == -1:
        raise SystemExit(f"{const_name}: unterminated literal")
    return body[:close_idx]


def generated_event_types(path: pathlib.Path) -> set[str]:
    if not path.is_file():
        raise SystemExit(f"missing generated inventory: {path}")
    text = path.read_text(encoding="utf-8")
    body = _literal_body(text, "KNOWN_AGENT_EVENT_TYPES")
    return set(re.findall(r'"([^"]+)"', body))


def main() -> int:
    known = schema_known_event_types()
    failures: list[str] = []

    for label, path in (
        ("web", WEB_EVENTS),
        ("typescript", TS_EVENTS),
        ("python", PY_EVENTS),
    ):
        generated = generated_event_types(path)
        missing = known - generated
        extra = generated - known
        if missing:
            failures.append(
                f"  {label} ({path.relative_to(ROOT)}): missing {sorted(missing)}"
            )
        if extra:
            failures.append(
                f"  {label} ({path.relative_to(ROOT)}): unknown-to-schema {sorted(extra)}"
            )

    if failures:
        print("SDK event-inventory parity check FAILED:")
        print("\n".join(failures))
        print()
        print("The generated SDK event inventories are out of sync with")
        print("artifacts/schemas/events.json (WireEvent.known_event_types).")
        print("Run: make regen-schemas")
        print("Then commit the updated generated SDK inventories.")
        return 1

    print(f"SDK event-inventory parity OK ({len(known)} event types across 3 SDKs)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
