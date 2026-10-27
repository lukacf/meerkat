#!/usr/bin/env python3
"""Every production decorator of a checked meerkat trait must name every method.

Checked traits: meerkat-mob's `MobSessionService` and meerkat-runtime's
`RuntimeStore` (see `TRAIT_SPECS`).

meerkat-mob's `MobSessionService` carries dozens of DEFAULTED methods, and most
defaults are typed refusals (`Unsupported`, `Unavailable`) or downgrades (read
cost `Unsupported`, fork source `None`). A decorator that omits one compiles
cleanly and silently answers the default instead of forwarding. On 2026-09-03
that shape failed a destructive reset in production
("session service cannot run a pre-retire archive hook") and an audit found
nine more such methods in both decorators - see PR #390 / #391.

This gate reads the trait from the pinned meerkat-mob source (via `cargo
metadata`, so it is the exact version Cargo.lock resolves), reads the features
Cargo enables on meerkat-mob (via `cargo tree`), and fails when any PRODUCTION
`impl MobSessionService for X` block in crates/meerkat-meta/src lacks a method that
exists under those features. Test doubles inside `#[cfg(test)]` modules are
skipped with the same trap-aware scan as verify-decorator-authority.py.

Required methods are checked too: a missing one is a compile error anyway, but
listing it here keeps the report complete.

`RuntimeStore` joined on 2026-09-04 for the same shape one seam over: the
gateway's epoch-tracking runtime-store facade did not forward
`list_runtime_delivery_authorities` (trait default: typed `Unsupported`), so the
1 Hz durable-callback health projection failed on every tick and logged
`durable callback health projection failed ... list_runtime_delivery_authorities`
forever. A trait method may be exempted only with a reason, never to silence a
failure: `commit_prepared_session_boundary_with_fence` is exempt because meerkat
documents it as decorator opt-in with no forwarding default, precisely so a
decorator cannot bypass its own write-epoch and projection behaviour.

Meerkat 0.9.6 adds `settle_cold_predecessor_with_mob_lifecycle_authority_before`
(meerkat-mob, feature `runtime-adapter`, on by default), whose default refuses
typed and touches nothing. Since the method set is read from the pinned
source, this gate requires both decorators to forward it from the pin that
carries it; tests/capability_invariant_choke.rs and the decorators' forwarding
row check the same seam.
"""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path
from typing import NamedTuple

ROOT = Path(__file__).resolve().parents[1]
SRC = ROOT / "crates" / "meerkat-meta" / "src"
# Honour the repository wrapper when a caller sets it (CARGO=scripts/repo-cargo);
# CI uses plain `cargo` with its own cache.
CARGO = os.environ.get("CARGO", "cargo")
TRAIT = "MobSessionService"


class TraitSpec(NamedTuple):
    name: str
    package: str
    source_relpath: str
    # A parser that finds fewer methods than this is stale, not lucky.
    min_expected_methods: int
    # Method name -> reason it legitimately is not forwarded. Never add an
    # entry to silence a failure.
    exempt: dict[str, str] = {}


TRAIT_SPECS: tuple[TraitSpec, ...] = (
    TraitSpec(
        name=TRAIT,
        package="meerkat-mob",
        source_relpath="src/runtime/session_service.rs",
        min_expected_methods=40,  # the trait has ~57 today
    ),
    # The base session-service traits the decorators also implement. Their
    # defaulted methods (`update_system_prompt`, `read_instruction_activation_records`,
    # the transcript rewrite/restore verbs, ...) default to typed `Unsupported`,
    # so an unforwarded one silently refuses through Meta's erased service:
    # `meta/member_instruction_activations` read nothing until 2026-10-05.
    TraitSpec(
        name="SessionService",
        package="meerkat-core",
        source_relpath="src/service/mod.rs",
        min_expected_methods=15,
    ),
    TraitSpec(
        name="SessionServiceHistoryExt",
        package="meerkat-core",
        source_relpath="src/service/mod.rs",
        min_expected_methods=4,
    ),
    # The other two base traits MobSessionService requires. Unchecked until
    # 2026-10-23, when a scan found both decorators answering the defaults of
    # `append_authenticated_system_context` and
    # `append_authorized_system_context` (typed `Unavailable`, so a governed
    # append through the mob's service failed) and of `send_comms` (a send
    # on the raw comms runtime instead of the inner service's own send).
    TraitSpec(
        name="SessionServiceCommsExt",
        package="meerkat-core",
        source_relpath="src/service/mod.rs",
        min_expected_methods=4,
    ),
    TraitSpec(
        name="SessionServiceControlExt",
        package="meerkat-core",
        source_relpath="src/service/mod.rs",
        min_expected_methods=4,
    ),
    TraitSpec(
        name="RuntimeStore",
        package="meerkat-runtime",
        source_relpath="src/store/mod.rs",
        min_expected_methods=60,  # the trait has 79 today
        exempt={
            "commit_prepared_session_boundary_with_fence": (
                "meerkat documents this verb as decorator opt-in with no forwarding "
                "default, so a decorator cannot bypass its own write-epoch, projection, "
                "or recovery behaviour; the facade has not opted in"
            ),
            "input_persist_ack_loss_fault_for_test": (
                "meerkat-runtime store/mod.rs documents this test-support-only fault "
                "seam as answered by SqliteRuntimeStore alone: every other store, "
                "including a wrapper of a SqliteRuntimeStore, returns None, so a "
                "control that needs the seam fails closed instead of arming nothing"
            ),
        },
    ),
)


def impl_pattern(trait: str) -> re.Pattern[str]:
    return re.compile(r"^(\s*)impl\s+(?:[\w:]+::)?" + trait + r"\s+for\s+(\$?\w+)")


IMPL = impl_pattern(TRAIT)
CFG_TEST = re.compile(r"^\s*#\[cfg\(test\)\]")
# A test module may carry a visibility (`pub(crate) mod support {`).
MOD_LINE = re.compile(r"^(\s*)(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{")
FN = re.compile(r"^\s*(?:pub(?:\([^)]*\))?\s+)?(?:async\s+)?fn\s+([a-z_0-9]+)\s*[<(]")
CFG_FEATURE = re.compile(r'#\[cfg\(feature\s*=\s*"([^"]+)"\)\]')


def trait_methods(trait_source: str, trait: str = TRAIT) -> dict[str, str | None]:
    """Method name -> feature it is gated on (None when unconditional)."""
    m = re.search(r"^pub trait " + trait + r"\b.*?\n\}\n", trait_source, re.S | re.M)
    if not m:
        raise SystemExit(f"error: `pub trait {trait}` not found in the pinned meerkat source")
    methods: dict[str, str | None] = {}
    pending_feature: str | None = None
    depth = 0
    # The header may span several lines (`pub trait X:\n    A + B\n{`); the
    # body starts after the first brace, whatever line it is on.
    body = m.group(0)[m.group(0).index("{") + 1 :]
    for line in body.splitlines():
        stripped = line.strip()
        if depth == 0:
            cfg = CFG_FEATURE.search(stripped)
            if cfg:
                pending_feature = cfg.group(1)
            fn = FN.match(line)
            if fn:
                methods[fn.group(1)] = pending_feature
                pending_feature = None
            elif stripped and not stripped.startswith(("#[", "//", "///")):
                # any other top-level statement resets a dangling cfg
                if not stripped.startswith(("async", "fn", "pub")):
                    pending_feature = None
        depth += line.count("{") - line.count("}")
    return methods


def test_regions(lines: list[str]) -> list[tuple[int, int]]:
    regions: list[tuple[int, int]] = []
    for i, line in enumerate(lines):
        if not CFG_TEST.match(line):
            continue
        j = i + 1
        while j < len(lines) and (lines[j].strip().startswith("#[") or not lines[j].strip()):
            j += 1
        if j >= len(lines) or not MOD_LINE.match(lines[j]):
            continue
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


def production_impls(source: str, trait: str = TRAIT) -> list[tuple[str, set[str]]]:
    """(impl target, method names) for every impl outside `#[cfg(test)]` modules."""
    impl = impl_pattern(trait)
    lines = source.splitlines()
    regions = test_regions(lines)
    found: list[tuple[str, set[str]]] = []
    for i, line in enumerate(lines):
        m = impl.match(line)
        if not m:
            continue
        if any(a <= i < b for a, b in regions):
            continue
        end = impl_block_end(lines, i)
        names = {fm.group(1) for l in lines[i:end] if (fm := FN.match(l))}
        found.append((m.group(2), names))
    return found


def missing_methods(
    methods: dict[str, str | None],
    enabled_features: set[str],
    impl_names: set[str],
    exempt: dict[str, str] | None = None,
) -> list[str]:
    exempt = exempt or {}
    return sorted(
        name
        for name, feature in methods.items()
        if (feature is None or feature in enabled_features)
        and name not in impl_names
        and name not in exempt
    )


def stale_exemptions(methods: dict[str, str | None], exempt: dict[str, str]) -> list[str]:
    """Exempt names the trait no longer has: a reason that outlived its method."""
    return sorted(name for name in exempt if name not in methods)


_METADATA: dict | None = None


def cargo_metadata() -> dict:
    global _METADATA
    if _METADATA is None:
        _METADATA = json.loads(
            subprocess.run(
                [CARGO, "metadata", "--format-version", "1", "--locked"],
                check=True,
                capture_output=True,
                cwd=ROOT,
            ).stdout
        )
    return _METADATA


def pinned_trait_source(spec: TraitSpec) -> str:
    pkgs = [p for p in cargo_metadata()["packages"] if p["name"] == spec.package]
    if len(pkgs) != 1:
        raise SystemExit(f"error: expected exactly one {spec.package} package, found {len(pkgs)}")
    manifest = Path(pkgs[0]["manifest_path"])
    path = manifest.parent / spec.source_relpath
    if not path.is_file():
        raise SystemExit(f"error: {spec.package} source not found at {path}")
    return path.read_text(encoding="utf-8")


def meerkat_mob_trait_source() -> str:
    return pinned_trait_source(TRAIT_SPECS[0])


def enabled_features(package: str) -> set[str]:
    out = subprocess.run(
        [CARGO, "tree", "-p", "meerkat-meta", "-e", "features", "-i", package, "--depth", "1"],
        check=True,
        capture_output=True,
        text=True,
        cwd=ROOT,
    ).stdout
    return set(re.findall(re.escape(package) + r' feature "([^"]+)"', out))


def enabled_meerkat_mob_features() -> set[str]:
    return enabled_features("meerkat-mob")


MIN_EXPECTED_METHODS = TRAIT_SPECS[0].min_expected_methods


def check_trait(spec: TraitSpec) -> bool:
    """Report every production decorator of `spec` missing a reachable method."""
    methods = trait_methods(pinned_trait_source(spec), spec.name)
    if len(methods) < spec.min_expected_methods:
        print(
            f"error: parsed only {len(methods)} {spec.name} methods (expected >= "
            f"{spec.min_expected_methods}); the trait parser is stale and this gate would pass vacuously",
            file=sys.stderr,
        )
        return False
    stale = stale_exemptions(methods, spec.exempt)
    if stale:
        print(
            f"error: {spec.name} exemption(s) name methods the pinned trait no longer has: {stale}",
            file=sys.stderr,
        )
        return False
    features = enabled_features(spec.package)
    ok = True
    checked = 0
    for path in sorted(SRC.rglob("*.rs")):
        for target, names in production_impls(path.read_text(encoding="utf-8"), spec.name):
            checked += 1
            missing = missing_methods(methods, features, names, spec.exempt)
            if missing:
                ok = False
                print(f"{path.relative_to(ROOT)}: `impl {spec.name} for {target}` lacks {len(missing)} method(s):")
                for name in missing:
                    print(f"    - {name}")
    if checked == 0:
        print(f"error: found NO production `impl {spec.name}` blocks; the pattern is stale", file=sys.stderr)
        return False
    if ok:
        reachable = len([m for m, f in methods.items() if f is None or f in features])
        exempt_note = f" ({len(spec.exempt)} exempt with reason)" if spec.exempt else ""
        print(f"ok: {checked} production {spec.name} impl(s) name all {reachable} reachable trait methods{exempt_note}")
    return ok


def main() -> int:
    if not all([check_trait(spec) for spec in TRAIT_SPECS]):
        print(
            "a decorator that omits a defaulted method answers the trait default (a refusal or a downgrade) "
            "instead of forwarding; add the forward",
            file=sys.stderr,
        )
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
