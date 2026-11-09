#!/usr/bin/env python3
"""Every PRODUCTION AgentLlmClient decorator must forward request_attempt_authority.

`AgentLlmClient::request_attempt_authority` carries a DEFAULT returning
`LegacySplit`. A decorator that omits it compiles cleanly, emits no warning, and
silently reports its own authority instead of the wrapped client's. meerkat
0.8.31 rejects `materialize resume` for a client reporting LegacySplit over a
Unified adapter: that cost 72 stranded identities at boot, measured downstream,
with nothing in Meta's own suite going red.

The per-wrapper unit tests cover the wrappers that EXIST. This covers the ones
that do not exist yet - a new decorator added later inherits the same default and
is silently wrong again, and no per-wrapper test can fail for a wrapper nobody
has written.

THREE TRAPS THIS AVOIDS, all found the hard way:

  FILE-LEVEL MATCHING IS NOT IMPL-LEVEL. Asking whether the FILE containing an
  impl mentions the symbol passes a tree where a production decorator omits it
  and a test double in the same file supplies it. That is the literal shape of
  mob_handle_runtime.rs, which holds both.

  `#[cfg(test)]` IS NOT ALWAYS FOLLOWED BY `mod`. Real code puts an
  `#[allow(...)]` between them. A pattern requiring `cfg(test)` then whitespace
  then `mod` matches nothing and silently classifies every test double as
  production - or, inverted, every production impl as a test.

  A TEST MODULE MAY CARRY A VISIBILITY. Shared test support is declared
  `pub(crate) mod ... {` (or `pub`, `pub(super)`, `pub(in path)`). A module
  pattern that requires the line to start with `mod` misses it, so the test
  doubles inside count as production: dispatch_taint.rs's
  `controller_test_support::PinnedInner` failed this gate that way.
"""

from __future__ import annotations

import re
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "crates" / "meerkat-meta" / "src"

IMPL = re.compile(r"^(\s*)impl\s+(?:[\w:]+::)?AgentLlmClient\s+for\s+(\w+)")
CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\]")
MOD_LINE = re.compile(r"^(\s*)(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{")

# Production decorators that legitimately do NOT forward. Empty by design: add a
# name here only with a reason, never to silence a failure.
EXEMPT: dict[str, str] = {}


def test_regions(lines: list[str]) -> list[tuple[int, int]]:
    """Line ranges (0-indexed, half-open) inside `#[cfg(test)]` modules.

    Scans forward from each `#[cfg(test)]` past any intervening attributes to
    the `mod ... {` line, then brace-matches to its close.
    """
    regions: list[tuple[int, int]] = []
    for i, line in enumerate(lines):
        if not CFG_TEST.match(line):
            continue
        j = i + 1
        while j < len(lines) and (lines[j].strip().startswith("#[") or not lines[j].strip()):
            j += 1
        if j >= len(lines) or not MOD_LINE.match(lines[j]):
            continue  # cfg(test) on something other than a module
        depth = 0
        for k in range(j, len(lines)):
            depth += lines[k].count("{") - lines[k].count("}")
            # MOD_LINE requires the `{`, so a balanced declaration line is a
            # module that closes where it opens (`pub(crate) mod empty {}`).
            if depth <= 0:
                regions.append((i, k + 1))
                break
        else:
            regions.append((i, len(lines)))
    return regions


def impl_block_end(lines: list[str], start: int) -> int:
    depth = 0
    for k in range(start, len(lines)):
        depth += lines[k].count("{") - lines[k].count("}")
        if depth <= 0 and k > start:
            return k + 1
    return len(lines)


def main() -> int:
    failures: list[str] = []
    production = 0
    for path in sorted(SRC.rglob("*.rs")):
        lines = path.read_text().splitlines()
        regions = test_regions(lines)
        for i, line in enumerate(lines):
            m = IMPL.match(line)
            if not m:
                continue
            in_test = any(lo <= i < hi for lo, hi in regions)
            name = m.group(2)
            if in_test:
                continue
            production += 1
            body = "\n".join(lines[i:impl_block_end(lines, i)])
            if "fn request_attempt_authority" in body:
                continue
            if name in EXEMPT:
                print(f"  exempt   {name} ({EXEMPT[name]})")
                continue
            rel = path.relative_to(ROOT)
            failures.append(
                f"{name} ({rel}:{i + 1}) does not forward request_attempt_authority. "
                "The trait default returns LegacySplit, so this decorator silently "
                "downgrades every client it wraps. Add:\n"
                "        fn request_attempt_authority(&self) "
                "-> meerkat_core::RequestAttemptAuthority {\n"
                "            self.inner.request_attempt_authority()\n"
                "        }"
            )

    if not production:
        print(
            "error: found NO production AgentLlmClient impls. Either the trait "
            "moved or this pattern stopped matching - both need a human, and "
            "neither is a pass.",
            file=sys.stderr,
        )
        return 1

    if failures:
        print("production AgentLlmClient decorators must forward authority:", file=sys.stderr)
        for f in failures:
            print(f"  - {f}", file=sys.stderr)
        return 1

    print(f"decorator authority OK: {production} production impl(s), all forwarding")
    return 0


if __name__ == "__main__":
    sys.exit(main())
