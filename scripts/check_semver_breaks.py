#!/usr/bin/env python3
"""Analyse a cargo-semver-checks report against the pending CHANGELOG section.

`scripts/check-semver-breaks.sh` produces the report and calls this analyser.
The analyser is a pure function of its arguments (report path, changelog path,
repo root, the tool's exit code) so it can be unit-tested against committed
fixtures without cargo-semver-checks installed. There is deliberately no env
override that relaxes it: an env hole in a release gate is a gate.

Three obligations:

1. MEASURED. A report the tool could not produce is not evidence of no breaks.
   Exit code and parsed content must agree, and every crate the release
   publishes must appear as checked or explicitly skipped in the report.

2. NAMED. Every break cargo-semver-checks reports must be named in the pending
   release notes' `### Breaking` subsection, at the granularity of the finding
   rather than of the type. `MaintenanceBridgeReport` gaining a field and
   `MaintenanceBridgeReport` losing `Copy` are two breaks; naming the first
   does not declare the second.

3. STAMPED. The pending section must be declared against a version. Notes that
   sit under `## [Unreleased]` after the version bump has landed would publish
   as release notes titled "Unreleased".

Behaviour-only breaks (a public signature that keeps its shape and changes what
it does) are invisible to cargo-semver-checks and therefore invisible here.
They remain a hand-written obligation; this gate cannot enforce them.
"""

from __future__ import annotations

import argparse
import pathlib
import re
import sys
from dataclasses import dataclass, field

try:
    import tomllib
except ModuleNotFoundError:  # pragma: no cover - exercised only on Python < 3.11
    import tomli as tomllib


ANSI_RE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")

# Report grammar, transcribed from a real cargo-semver-checks 0.48.0 run
# (fixture: scripts/fixtures/semver-breaks/report-meerkat-sqlite-0.8.22.txt).
FAILURE_HEADER_RE = re.compile(r"^--- failure ([A-Za-z0-9_]+):")
WARNING_HEADER_RE = re.compile(r"^--- warning ([A-Za-z0-9_]+):")
CHECKING_RE = re.compile(r"^\s+Checking (\S+) v")
FINISHED_RE = re.compile(r"^\s+Finished \[[^\]]*\] (\S+)\s*$")
SUMMARY_RE = re.compile(r"^\s+Summary\b")
FAILED_IN_RE = re.compile(r"^Failed in:\s*$")
ITEM_RE = re.compile(r"^  (\S.*)$")
# Every "Failed in:" line ends with the source location the break was found at.
# cargo-semver-checks uses three observed spellings: `in <path>`,
# `in file <path>`, and `, previously in file <path>`.
LOCATION_SUFFIX_RE = re.compile(
    r"(?:,\s+previously\s+in|,?\s+in)(?:\s+file)?\s+\S+:\d+\s*$"
)

PASCAL_CASE_RE = re.compile(r"\b[A-Z][A-Za-z0-9_]*\b")

CHANGELOG_SECTION_RE = re.compile(r"^## \[([^\]]+)\](.*)$")
STAMPED_SUFFIX_RE = re.compile(r"^\s*-\s*\d{4}-\d{2}-\d{2}\s*$")
BREAKING_HEADING_RE = re.compile(r"^### Breaking\b")
SUBSECTION_RE = re.compile(r"^###\s")


@dataclass(frozen=True)
class Finding:
    """One reported break: a lint id plus the item it fired on."""

    lint_id: str
    crate: str
    item: str
    symbols: tuple[str, ...]
    structural: bool


@dataclass
class ReportParse:
    findings: list[Finding] = field(default_factory=list)
    checked_crates: list[str] = field(default_factory=list)
    finished_crates: list[str] = field(default_factory=list)
    summary_lines: int = 0


@dataclass
class CrateScope:
    """Which published crates cargo-semver-checks can actually look at.

    Observed, not assumed: a `--workspace` run says NOTHING about a proc-macro
    crate. No Building line, no Checking line, no Skipping line - the crate is
    simply absent from the report. Requiring every published crate to appear
    would therefore red-gate every release forever.
    """

    checkable: list[str] = field(default_factory=list)
    proc_macro: list[str] = field(default_factory=list)
    no_lib_target: list[str] = field(default_factory=list)
    missing_manifest: list[str] = field(default_factory=list)


# ---------------------------------------------------------------------------
# Report parsing
# ---------------------------------------------------------------------------


def strip_location(item: str) -> str:
    """Drop the trailing source location cargo-semver-checks appends."""
    return LOCATION_SUFFIX_RE.sub("", item).strip()


def _symbols_constructible_struct_adds_field(text: str) -> tuple[str, ...] | None:
    # field MaintenanceBridgeReport.refused
    match = re.fullmatch(r"field\s+([A-Za-z0-9_:]+)\.([A-Za-z0-9_]+)", text)
    if not match:
        return None
    return _path_symbols(match.group(1)) + (match.group(2),)


def _symbols_enum_struct_variant_field_added(text: str) -> tuple[str, ...] | None:
    # field bridgeable of variant SqliteStoreError::UnledgeredDomainObjects
    match = re.fullmatch(
        r"field\s+([A-Za-z0-9_]+)\s+of\s+variant\s+([A-Za-z0-9_:]+)",
        text,
    )
    if not match:
        return None
    return (match.group(1),) + _path_symbols(match.group(2))


def _symbols_enum_variant_added(text: str) -> tuple[str, ...] | None:
    # variant SqliteStoreError:WalConversionContended (single colon: real output)
    match = re.fullmatch(r"variant\s+([A-Za-z0-9_:]+)", text)
    if match:
        return _path_symbols(match.group(1))

    # MeerkatMachineInputVariant::ResolveInputPublicLifecycle moved from
    # position 99 to 101
    match = re.fullmatch(
        r"([A-Za-z0-9_:]+)\s+moved\s+from\s+position\s+\d+\s+to\s+\d+",
        text,
    )
    return _path_symbols(match.group(1)) if match else None


def _symbols_enum_no_repr_variant_discriminant_changed(
    text: str,
) -> tuple[str, ...] | None:
    # variant AgentEvent::ToolExecutionStarted 19 -> 21
    match = re.fullmatch(
        r"variant\s+([A-Za-z0-9_:]+)\s+\d+\s+->\s+\d+",
        text,
    )
    return _path_symbols(match.group(1)) if match else None


def _symbols_enum_variant_missing(text: str) -> tuple[str, ...] | None:
    # variant RegistrationOutcome::ReboundOwnName
    match = re.fullmatch(r"variant\s+([A-Za-z0-9_:]+)", text)
    return _path_symbols(match.group(1)) if match else None


def _symbols_path_member(path: str) -> tuple[str, ...] | None:
    """Return the public owner and final field/method segment of a path."""
    segments = [seg for seg in re.split(r":+", path) if seg]
    if len(segments) < 2:
        return None
    return _path_symbols("::".join(segments[:-1])) + (segments[-1],)


def _symbols_inherent_method_missing(text: str) -> tuple[str, ...] | None:
    # JobHealthSnapshot::is_degraded
    match = re.fullmatch(r"([A-Za-z0-9_:]+)", text)
    return _symbols_path_member(match.group(1)) if match else None


def _symbols_struct_pub_field_missing(text: str) -> tuple[str, ...] | None:
    # field delivery_backlog of struct JobHealthSummary
    match = re.fullmatch(
        r"field\s+([A-Za-z0-9_]+)\s+of\s+struct\s+([A-Za-z0-9_:]+)",
        text,
    )
    if not match:
        return None
    return _path_symbols(match.group(2)) + (match.group(1),)


def _symbols_trait_method_added(text: str) -> tuple[str, ...] | None:
    # trait method meerkat_jobs::DetachedJobStore::count_pending_outbox_jobs
    match = re.fullmatch(r"trait\s+method\s+([A-Za-z0-9_:]+)", text)
    return _symbols_path_member(match.group(1)) if match else None


def _symbols_derive_trait_impl_removed(text: str) -> tuple[str, ...] | None:
    # type MaintenanceBridgeReport no longer derives Copy
    match = re.fullmatch(
        r"type\s+([A-Za-z0-9_:]+)\s+no longer derives\s+([A-Za-z0-9_:]+)",
        text,
    )
    if not match:
        return None
    return _path_symbols(match.group(1)) + _path_symbols(match.group(2))


def _symbols_auto_trait_impl_removed(text: str) -> tuple[str, ...] | None:
    # type LiveWebrtcAnswerAccepted is no longer Sync
    match = re.fullmatch(
        r"type\s+([A-Za-z0-9_:]+)\s+is\s+no\s+longer\s+([A-Za-z0-9_:]+)",
        text,
    )
    if not match:
        return None
    return _path_symbols(match.group(1)) + _path_symbols(match.group(2))


def _symbols_struct_missing(text: str) -> tuple[str, ...] | None:
    # struct meerkat_live::host::LiveChannelId
    match = re.fullmatch(r"struct\s+([A-Za-z0-9_:]+)", text)
    return _path_symbols(match.group(1)) if match else None


def _symbols_callable_parameter_count_changed(text: str) -> tuple[str, ...] | None:
    # meerkat_rpc::handlers::live::handle_live_close now takes 6 parameters instead of 5
    # SessionRuntime::truncate_live_output takes 6 parameters in <baseline>, but now ...
    match = re.match(
        r"([A-Za-z0-9_:]+)\s+(?:now\s+)?takes\s+\d+\s+"
        r"(?:parameters\b|instead\s+of\s+\d+\s+parameters\b)",
        text,
    )
    return _symbols_path_member(match.group(1)) if match else None


def _symbols_function_parameter_count_changed(text: str) -> tuple[str, ...] | None:
    symbols = _symbols_callable_parameter_count_changed(text)
    return symbols[-1:] if symbols else None


def _symbols_method_generic_count_changed(text: str) -> tuple[str, ...] | None:
    # PendingPromotionCleanup::recover takes 1 generic types instead of 0
    match = re.match(
        r"([A-Za-z0-9_:]+)\s+takes\s+\d+\s+generic\s+types\s+instead\s+of\s+\d+",
        text,
    )
    return _symbols_path_member(match.group(1)) if match else None


def _symbols_partial_ord_struct_field_reordered(text: str) -> tuple[str, ...] | None:
    # SessionLlmCapabilitySurface.image_generation moved from position 9 to 10
    match = re.fullmatch(
        r"([A-Za-z0-9_:]+)\.([A-Za-z0-9_]+)\s+moved\s+from\s+position\s+\d+\s+to\s+\d+",
        text,
    )
    if not match:
        return None
    return _path_symbols(match.group(1)) + (match.group(2),)


STRUCTURAL_EXTRACTORS = {
    "auto_trait_impl_removed": _symbols_auto_trait_impl_removed,
    "constructible_struct_adds_field": _symbols_constructible_struct_adds_field,
    "constructible_struct_adds_private_field": _symbols_constructible_struct_adds_field,
    "enum_no_repr_variant_discriminant_changed": (
        _symbols_enum_no_repr_variant_discriminant_changed
    ),
    "enum_struct_variant_field_added": _symbols_enum_struct_variant_field_added,
    "enum_variant_added": _symbols_enum_variant_added,
    "enum_variant_missing": _symbols_enum_variant_missing,
    "derive_trait_impl_removed": _symbols_derive_trait_impl_removed,
    "inherent_method_missing": _symbols_inherent_method_missing,
    "function_parameter_count_changed": _symbols_function_parameter_count_changed,
    "method_parameter_count_changed": _symbols_callable_parameter_count_changed,
    "method_requires_different_generic_type_params": _symbols_method_generic_count_changed,
    "partial_ord_enum_variants_reordered": _symbols_enum_variant_added,
    "partial_ord_struct_fields_reordered": _symbols_partial_ord_struct_field_reordered,
    "struct_missing": _symbols_struct_missing,
    "struct_pub_field_missing": _symbols_struct_pub_field_missing,
    "trait_method_added": _symbols_trait_method_added,
    "trait_method_parameter_count_changed": _symbols_callable_parameter_count_changed,
}


def _path_symbols(path: str) -> tuple[str, ...]:
    """Split `a::B::C` into the segments a downstream reader would recognise.

    Leading module segments are dropped: the changelog may write
    `meerkat_sqlite::SqliteStoreError` or plain `SqliteStoreError` and both name
    the same thing. Only the type-ish tail segments are required.

    Split on any colon run, not on `::` alone: `enum_variant_added` really does
    print `variant SqliteStoreError:WalConversionContended` with one colon.
    """
    segments = [seg for seg in re.split(r":+", path) if seg]
    kept = [seg for seg in segments if seg[:1].isupper()]
    return tuple(kept) if kept else tuple(segments[-1:])


def extract_symbols(lint_id: str, item_text: str) -> tuple[tuple[str, ...], bool]:
    """Return (required symbols, structural?) for one de-located item line.

    Structural extraction is exact for the lint ids whose message shape has been
    read off a real report. Anything else falls back to PascalCase tokens, which
    cannot pick up prose (lint prose is lowercase) but also cannot see field or
    method names. The fallback is reported, never silent, and a fallback that
    extracts nothing is a hard failure rather than a free pass.
    """
    extractor = STRUCTURAL_EXTRACTORS.get(lint_id)
    if extractor is not None:
        symbols = extractor(item_text)
        if symbols is not None:
            return dedupe(symbols), True
    return dedupe(tuple(PASCAL_CASE_RE.findall(item_text))), False


def dedupe(values: tuple[str, ...]) -> tuple[str, ...]:
    seen: list[str] = []
    for value in values:
        if value not in seen:
            seen.append(value)
    return tuple(seen)


def parse_report(text: str) -> ReportParse:
    parsed = ReportParse()
    current_lint: str | None = None
    current_crate = "<unknown>"
    collecting = False
    seen: set[tuple[str, str, str]] = set()

    for raw_line in ANSI_RE.sub("", text).splitlines():
        line = raw_line.rstrip("\n")

        header = FAILURE_HEADER_RE.match(line) or WARNING_HEADER_RE.match(line)
        if header:
            current_lint = header.group(1)
            collecting = False
            continue

        checking = CHECKING_RE.match(line)
        if checking:
            current_crate = checking.group(1)
            parsed.checked_crates.append(current_crate)
            collecting = False
            continue

        finished = FINISHED_RE.match(line)
        if finished:
            parsed.finished_crates.append(finished.group(1))
            collecting = False
            continue

        if SUMMARY_RE.match(line):
            parsed.summary_lines += 1
            collecting = False
            continue

        if FAILED_IN_RE.match(line):
            collecting = current_lint is not None
            continue

        if not collecting:
            continue

        item_match = ITEM_RE.match(line)
        if not item_match:
            collecting = False
            continue

        item = strip_location(item_match.group(1))
        # The tool repeats each item once per feature configuration it checked.
        key = (current_crate, current_lint or "", item)
        if key in seen:
            continue
        seen.add(key)
        symbols, structural = extract_symbols(current_lint or "", item)
        parsed.findings.append(
            Finding(
                lint_id=current_lint or "",
                crate=current_crate,
                item=item,
                symbols=symbols,
                structural=structural,
            )
        )

    return parsed


# ---------------------------------------------------------------------------
# Changelog parsing
# ---------------------------------------------------------------------------


@dataclass(frozen=True)
class Section:
    heading: str
    label: str
    suffix: str
    body: str

    @property
    def version(self) -> str | None:
        return None if self.label.strip().lower() == "unreleased" else self.label.strip()

    @property
    def is_stamped(self) -> bool:
        return self.version is not None and bool(STAMPED_SUFFIX_RE.match(self.suffix))

    @property
    def is_empty(self) -> bool:
        return not self.body.strip()


def parse_changelog(text: str) -> list[Section]:
    sections: list[Section] = []
    heading: str | None = None
    label = ""
    suffix = ""
    body: list[str] = []

    for line in text.splitlines():
        match = CHANGELOG_SECTION_RE.match(line)
        if match:
            if heading is not None:
                sections.append(Section(heading, label, suffix, "\n".join(body)))
            heading = line
            label = match.group(1)
            suffix = match.group(2)
            body = []
            continue
        if heading is not None:
            body.append(line)

    if heading is not None:
        sections.append(Section(heading, label, suffix, "\n".join(body)))
    return sections


def pending_section(sections: list[Section]) -> Section | None:
    """The section this release is declared in.

    Stamping a release leaves an EMPTY `## [Unreleased]` stub above it, so
    "topmost section" and "the section this release is declared in" stop being
    the same thing at exactly the moment this gate runs. Reading the stub would
    fail every stamped release no matter how loudly it declared its breaks,
    which is a gate that cannot be satisfied rather than one that is hard to
    satisfy. Skip a content-free pending section and read the one below it.
    """
    if not sections:
        return None
    if sections[0].is_empty and len(sections) > 1:
        return sections[1]
    return sections[0]


def breaking_body(section: Section) -> str | None:
    """The `### Breaking` subsection body, or None when there is no such heading.

    Scoped to the subsection: a break named under `### Fixed` is not a
    declaration, and matching against the whole section would accept one.
    """
    lines = section.body.splitlines()
    collected: list[str] = []
    inside = False
    for line in lines:
        if BREAKING_HEADING_RE.match(line):
            inside = True
            continue
        if inside and SUBSECTION_RE.match(line):
            break
        if inside:
            collected.append(line)
    return "\n".join(collected) if inside else None


def names_symbol(body: str, symbol: str) -> bool:
    pattern = r"(?<![A-Za-z0-9_])" + re.escape(symbol) + r"(?![A-Za-z0-9_])"
    return re.search(pattern, body) is not None


ENUM_WILDCARD_LINTS = {
    "enum_no_repr_variant_discriminant_changed",
    "partial_ord_enum_variants_reordered",
}


def names_finding_symbol(body: str, finding: Finding, index: int, symbol: str) -> bool:
    if names_symbol(body, symbol):
        return True
    if (
        finding.lint_id in ENUM_WILDCARD_LINTS
        and index > 0
        and finding.symbols
        and names_symbol(body, f"{finding.symbols[0]}::*")
    ):
        return True
    return False


# ---------------------------------------------------------------------------
# Gate
# ---------------------------------------------------------------------------


def workspace_version(repo_root: pathlib.Path) -> str:
    data = tomllib.loads((repo_root / "Cargo.toml").read_text(encoding="utf-8"))
    return str(data["workspace"]["package"]["version"])


def workspace_manifests(repo_root: pathlib.Path) -> dict[str, dict]:
    """Map crate name to parsed manifest for every workspace member."""
    workspace = tomllib.loads((repo_root / "Cargo.toml").read_text(encoding="utf-8"))
    paths: list[pathlib.Path] = []
    for member in workspace["workspace"]["members"]:
        if "*" in member:
            paths.extend(sorted(repo_root.glob(member)))
        else:
            paths.append(repo_root / member)

    manifests: dict[str, dict] = {}
    for path in paths:
        manifest = path / "Cargo.toml"
        if not manifest.exists():
            continue
        data = tomllib.loads(manifest.read_text(encoding="utf-8"))
        name = data.get("package", {}).get("name")
        if name:
            data["__dir__"] = str(path)
            manifests[name] = data
    return manifests


def classify_crates(repo_root: pathlib.Path, release_crates: list[str]) -> CrateScope:
    """Split the published crates into "the report must cover it" and "it cannot"."""
    manifests = workspace_manifests(repo_root)
    scope = CrateScope()
    for name in release_crates:
        manifest = manifests.get(name)
        if manifest is None:
            scope.missing_manifest.append(name)
            continue
        lib = manifest.get("lib", {})
        if lib.get("proc-macro") is True:
            scope.proc_macro.append(name)
            continue
        lib_path = pathlib.Path(manifest["__dir__"]) / lib.get("path", "src/lib.rs")
        if not lib_path.exists():
            scope.no_lib_target.append(name)
            continue
        scope.checkable.append(name)
    return scope


def check_measured(parsed: ReportParse, tool_exit_code: int, scope: CrateScope | None) -> list[str]:
    """Fail closed when the report is not evidence about the whole release."""
    errors: list[str] = []

    if parsed.summary_lines == 0 and not parsed.finished_crates:
        errors.append(
            "the cargo-semver-checks report contains no Summary/Finished line: "
            "the run did not complete, so it is not evidence of anything"
        )

    if tool_exit_code == 0 and parsed.findings:
        errors.append(
            f"cargo-semver-checks exited 0 but the report contains "
            f"{len(parsed.findings)} failure item(s): report and exit code disagree"
        )

    if tool_exit_code != 0 and not parsed.findings:
        errors.append(
            f"cargo-semver-checks exited {tool_exit_code} with no parsed failure items: "
            "the run failed for a reason other than a detected break (build failure, "
            "missing baseline, network), so this gate could not measure the release"
        )

    if scope is not None:
        if scope.missing_manifest:
            errors.append(
                "these crates are published by the release but have no workspace "
                "manifest, so the gate cannot tell whether they were checked: "
                + ", ".join(scope.missing_manifest)
            )
        covered = set(parsed.finished_crates)
        missing = [crate for crate in scope.checkable if crate not in covered]
        if missing:
            errors.append(
                "the report never reached these publishable crates (no `Finished` "
                "line): " + ", ".join(missing)
            )

    return errors


def check_stamped(sections: list[Section], version: str) -> list[str]:
    """The pending notes must be declared against a version, not against nothing."""
    pending = pending_section(sections)
    if pending is None:
        return ["CHANGELOG.md has no `## [...]` section at all"]

    if pending.version is None:
        stamped = next((section for section in sections if section.version is not None), None)
        if stamped is None:
            return [
                "the pending CHANGELOG.md section is `## [Unreleased]` and there is no "
                "stamped release below it to declare against"
            ]
        if stamped.version != version:
            return [
                f"the pending CHANGELOG.md section is `## [Unreleased]` but the workspace "
                f"version is already {version} (topmost stamped section is "
                f"`{stamped.heading.strip()}`). The version bump landed and the notes did "
                f"not: this release would publish notes titled \"Unreleased\". Stamp them "
                f"as `## [{version}] - YYYY-MM-DD`."
            ]
        return []

    if pending.version != version:
        return [
            f"the pending CHANGELOG.md section is `{pending.heading.strip()}` but the "
            f"workspace version is {version}: the release notes are declared against a "
            f"different version than the one being released"
        ]

    if not pending.is_stamped:
        return [
            f"the pending CHANGELOG.md section `{pending.heading.strip()}` carries no "
            f"`- YYYY-MM-DD` release date"
        ]

    return []


def check_named(parsed: ReportParse, section: Section) -> list[str]:
    """Every reported break must be named, at finding granularity."""
    body = breaking_body(section)
    if body is None:
        return [
            f"the pending CHANGELOG.md section `{section.heading.strip()}` has no "
            f"`### Breaking` heading, but cargo-semver-checks reported "
            f"{len(parsed.findings)} break(s)"
        ]

    errors: list[str] = []
    for finding in parsed.findings:
        if not finding.symbols:
            errors.append(
                f"[{finding.crate}] {finding.lint_id}: `{finding.item}` yielded no "
                f"nameable symbol, so this gate cannot tell whether it was declared. "
                f"Teach scripts/check_semver_breaks.py this lint's message shape."
            )
            continue
        missing = [
            symbol
            for index, symbol in enumerate(finding.symbols)
            if not names_finding_symbol(body, finding, index, symbol)
        ]
        if missing:
            errors.append(
                f"[{finding.crate}] {finding.lint_id}: `{finding.item}` is not named "
                f"under `### Breaking` (missing: {', '.join('`' + m + '`' for m in missing)})"
            )
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--report", required=True, type=pathlib.Path)
    parser.add_argument("--changelog", required=True, type=pathlib.Path)
    parser.add_argument("--tool-exit-code", required=True, type=int)
    parser.add_argument("--repo-root", type=pathlib.Path)
    parser.add_argument("--version", help="workspace version (defaults to --repo-root Cargo.toml)")
    parser.add_argument(
        "--release-crate",
        action="append",
        default=[],
        help="crate this release publishes; repeatable. Those with a non-proc-macro "
        "lib target must appear in the report.",
    )
    args = parser.parse_args()

    if args.version:
        version = args.version
    elif args.repo_root:
        version = workspace_version(args.repo_root)
    else:
        print("error: one of --version or --repo-root is required", file=sys.stderr)
        return 2

    scope: CrateScope | None = None
    if args.release_crate:
        if not args.repo_root:
            print("error: --release-crate requires --repo-root", file=sys.stderr)
            return 2
        scope = classify_crates(args.repo_root, args.release_crate)

    report_text = args.report.read_text(encoding="utf-8", errors="replace")
    parsed = parse_report(report_text)
    sections = parse_changelog(args.changelog.read_text(encoding="utf-8"))

    errors = check_measured(parsed, args.tool_exit_code, scope)
    errors.extend(check_stamped(sections, version))

    section = pending_section(sections)
    if parsed.findings and section is not None:
        errors.extend(check_named(parsed, section))

    fallback_lints = sorted({f.lint_id for f in parsed.findings if not f.structural})
    if fallback_lints:
        print(
            "semver-breaks: NOTE: no structural extractor matched the message shape of "
            "lint(s) "
            + ", ".join(fallback_lints)
            + "; fell back to PascalCase symbols, which cannot see field or method names. "
            "Either the lint is new or its message shape changed: teach "
            "scripts/check_semver_breaks.py the shape to restore full granularity.",
            file=sys.stderr,
        )

    # Say out loud which published crates no tool is checking. A coverage gap
    # that only exists in the parser's head is the same shape of defect as an
    # undeclared break.
    if scope is not None and (scope.proc_macro or scope.no_lib_target):
        unchecked = [f"{name} (proc-macro)" for name in scope.proc_macro]
        unchecked += [f"{name} (no lib target)" for name in scope.no_lib_target]
        print(
            "semver-breaks: NOTE: cargo-semver-checks does not look at these published "
            "crates, and neither does this gate: " + ", ".join(unchecked),
            file=sys.stderr,
        )

    if errors:
        print("semver-breaks: FAILED", file=sys.stderr)
        for error in errors:
            print(f"  - {error}", file=sys.stderr)
        print(file=sys.stderr)
        print(
            "Policy (M3): 0.x patch releases may break public API, but every break must be\n"
            "declared under `### Breaking` in the pending release section, naming the changed\n"
            "signatures, so exact-pinned downstreams can plan the bump.",
            file=sys.stderr,
        )
        return 1

    if not parsed.findings:
        print(
            f"semver-breaks: no public-API breaks vs the published baselines "
            f"({len(parsed.finished_crates)} crate(s) checked, notes stamped for {version})"
        )
        return 0

    print(
        f"semver-breaks: {len(parsed.findings)} public-API break(s) detected, all named under "
        f"`### Breaking` in the {version} release notes:"
    )
    for finding in parsed.findings:
        print(f"  [{finding.crate}] {finding.lint_id}: {finding.item}")
    return 0


if __name__ == "__main__":
    sys.exit(main())
