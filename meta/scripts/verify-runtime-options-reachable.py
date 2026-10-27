#!/usr/bin/env python3
"""Every runtime_options key the gateway HANDLES must be in its allowlist.

The unknown-field rejection runs BEFORE the per-key handlers, so a handled key
that is missing from `let supported = [...]` is dead code in the shipped binary:
the field is documented, the handler compiles, every test that builds the options
struct directly still passes, and the caller gets
`unsupported runtime_options fields: <key>`.

That shipped in 0.8.24 for BOTH `mob_composition` and `declare_spec_update`. It
was found by a consumer sending the documented line to the published artifact,
not by any test. This gate is ten lines of comparison that would have caught both
before the tag; it exists because enumerating beats remembering.
"""
import re
import sys
import pathlib

GATEWAY = pathlib.Path(__file__).resolve().parent.parent / "crates/meerkat-meta/src/rpc_gateway.rs"


def main() -> int:
    src = GATEWAY.read_text(encoding="utf-8")
    function_start = src.find("fn parse_gateway_runtime_options(")
    if function_start == -1:
        print("FAIL: could not find parse_gateway_runtime_options", file=sys.stderr)
        return 1
    match = re.search(
        r"let supported = \[(.*?)\];", src[function_start:], re.S
    )
    if not match:
        print("FAIL: could not find the runtime_options allowlist", file=sys.stderr)
        return 1
    allowed = set(re.findall(r'"([^"]+)"', match.group(1)))
    handled = set(re.findall(r'runtime_options\.get\("([^"]+)"\)', src))

    dead = sorted(handled - allowed)
    if dead:
        print("FAIL: runtime_options handlers unreachable behind the allowlist:", file=sys.stderr)
        for key in dead:
            print(f"  {key}: handler exists, allowlist rejects the field first", file=sys.stderr)
        print(
            "\nAdd each key to the `supported` array in parse_gateway_runtime_options.",
            file=sys.stderr,
        )
        return 1

    print(f"runtime_options reachability: OK ({len(handled)} handled, {len(allowed)} allowlisted)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
