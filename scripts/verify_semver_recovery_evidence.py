#!/usr/bin/env python3
"""Verify that a failed tag semver job needs declaration repair only.

This is not a semver bypass. It accepts a prior measurement only when GitHub
metadata binds the completed job to the exact immutable release tag and SHA,
all measurement setup steps succeeded, and the analyzer's complete failure
set contains only missing changelog declarations. The amended changelog must
then name every symbol from that exact failure set under the stamped release.
"""

from __future__ import annotations

import argparse
import json
import pathlib
import re
import sys
from dataclasses import dataclass

import check_semver_breaks as semver_gate


ANSI_RE = re.compile(r"\x1b\[[0-9;]*[A-Za-z]")
TIMESTAMP_RE = re.compile(r"^\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?Z(?:\s|$)")
FINDING_RE = re.compile(
    r"^\s*- \[(?P<crate>[^]]+)] (?P<lint>[a-z0-9_]+): "
    r"`(?P<item>.+)` is not named under `### Breaking` "
    r"\(missing: (?P<missing>.+)\)$"
)


class EvidenceError(ValueError):
    pass


@dataclass(frozen=True)
class MissingDeclaration:
    crate: str
    lint: str
    item: str
    symbols: tuple[str, ...]


def normalized_lines(text: str) -> list[str]:
    lines: list[str] = []
    for raw in ANSI_RE.sub("", text).splitlines():
        lines.append(TIMESTAMP_RE.sub("", raw, count=1))
    return lines


def exactly_one_index(lines: list[str], value: str) -> int:
    matches = [index for index, line in enumerate(lines) if line == value]
    if len(matches) != 1:
        raise EvidenceError(f"expected exactly one `{value}` line, found {len(matches)}")
    return matches[0]


def parse_missing_declarations(log_text: str) -> tuple[MissingDeclaration, ...]:
    lines = normalized_lines(log_text)
    selftest = exactly_one_index(lines, "semver-breaks: analyser self-test passed")
    failed = exactly_one_index(lines, "semver-breaks: FAILED")
    policy = exactly_one_index(lines, "Policy (M3): 0.x patch releases may break public API, but every break must be")
    exit_lines = [
        (index, line)
        for index, line in enumerate(lines)
        if line.startswith("cargo-semver-checks exited ")
        and line.endswith("; last 40 report lines:")
    ]
    if len(exit_lines) != 1:
        raise EvidenceError(
            f"expected exactly one cargo-semver-checks exit line, found {len(exit_lines)}"
        )
    exit_index, exit_line = exit_lines[0]
    if not selftest < failed < policy < exit_index:
        raise EvidenceError("semver evidence markers are incomplete or out of order")
    exit_match = re.fullmatch(
        r"cargo-semver-checks exited (?P<code>[1-9][0-9]*); last 40 report lines:",
        exit_line,
    )
    if exit_match is None:
        raise EvidenceError("semver evidence does not contain a nonzero tool exit")

    findings: list[MissingDeclaration] = []
    for line in lines[failed + 1 : policy]:
        if not line.strip():
            continue
        match = FINDING_RE.fullmatch(line)
        if match is None:
            raise EvidenceError(f"unexpected analyzer failure line: {line}")
        symbols = tuple(re.findall(r"`([^`]+)`", match.group("missing")))
        if not symbols:
            raise EvidenceError(f"missing declaration line has no symbols: {line}")
        findings.append(
            MissingDeclaration(
                crate=match.group("crate"),
                lint=match.group("lint"),
                item=match.group("item"),
                symbols=symbols,
            )
        )
    if not findings:
        raise EvidenceError("semver analyzer failure contains no missing declarations")
    return tuple(findings)


def require_equal(actual: object, expected: object, label: str) -> None:
    if actual != expected:
        raise EvidenceError(f"{label} is {actual!r}, expected {expected!r}")


def verify_metadata(
    run: dict[str, object],
    job: dict[str, object],
    *,
    evidence_run_id: int,
    evidence_job_id: int,
    release_tag: str,
    release_sha: str,
) -> None:
    require_equal(run.get("id"), evidence_run_id, "run id")
    require_equal(run.get("event"), "push", "run event")
    require_equal(run.get("head_branch"), release_tag, "run tag")
    require_equal(run.get("head_sha"), release_sha, "run SHA")
    require_equal(run.get("path"), ".github/workflows/release.yml", "run workflow path")
    require_equal(job.get("id"), evidence_job_id, "job id")
    require_equal(job.get("run_id"), evidence_run_id, "job run id")
    require_equal(job.get("workflow_name"), "Release", "job workflow")
    require_equal(job.get("name"), "Breaks declared and notes stamped", "job name")
    require_equal(job.get("head_branch"), release_tag, "job tag")
    require_equal(job.get("head_sha"), release_sha, "job SHA")
    require_equal(job.get("status"), "completed", "job status")
    require_equal(job.get("conclusion"), "failure", "job conclusion")

    steps = {
        step.get("name"): step.get("conclusion")
        for step in job.get("steps", [])
        if isinstance(step, dict)
    }
    for name in ("Set up job", "Checkout", "Install Rust", "Cache cargo", "Install cargo-semver-checks"):
        require_equal(steps.get(name), "success", f"step {name}")
    require_equal(
        steps.get("Verify every reported break is named and the notes are stamped"),
        "failure",
        "semver measurement step",
    )


def verify_changelog(
    changelog_path: pathlib.Path,
    release_tag: str,
    findings: tuple[MissingDeclaration, ...],
) -> None:
    if not release_tag.startswith("v") or len(release_tag) == 1:
        raise EvidenceError(f"release tag must have v prefix: {release_tag!r}")
    version = release_tag[1:]
    sections = semver_gate.parse_changelog(changelog_path.read_text(encoding="utf-8"))
    stamped_errors = semver_gate.check_stamped(sections, version)
    if stamped_errors:
        raise EvidenceError("; ".join(stamped_errors))
    section = semver_gate.pending_section(sections)
    if section is None:
        raise EvidenceError("changelog has no pending release section")
    body = semver_gate.breaking_body(section)
    if body is None:
        raise EvidenceError(f"{section.heading.strip()} has no `### Breaking` section")
    undeclared: list[str] = []
    for finding in findings:
        for symbol in finding.symbols:
            if not semver_gate.names_symbol(body, symbol):
                undeclared.append(f"[{finding.crate}] {finding.lint}: {symbol}")
    if undeclared:
        raise EvidenceError("amended changelog is still missing: " + ", ".join(undeclared))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--run-json", required=True, type=pathlib.Path)
    parser.add_argument("--job-json", required=True, type=pathlib.Path)
    parser.add_argument("--job-log", required=True, type=pathlib.Path)
    parser.add_argument("--evidence-run-id", required=True, type=int)
    parser.add_argument("--evidence-job-id", required=True, type=int)
    parser.add_argument("--release-tag", required=True)
    parser.add_argument("--release-sha", required=True)
    parser.add_argument("--changelog", required=True, type=pathlib.Path)
    args = parser.parse_args()
    try:
        run = json.loads(args.run_json.read_text(encoding="utf-8"))
        job = json.loads(args.job_json.read_text(encoding="utf-8"))
        verify_metadata(
            run,
            job,
            evidence_run_id=args.evidence_run_id,
            evidence_job_id=args.evidence_job_id,
            release_tag=args.release_tag,
            release_sha=args.release_sha,
        )
        findings = parse_missing_declarations(
            args.job_log.read_text(encoding="utf-8", errors="replace")
        )
        verify_changelog(args.changelog, args.release_tag, findings)
    except (EvidenceError, OSError, json.JSONDecodeError) as error:
        print(f"semver recovery evidence rejected: {error}", file=sys.stderr)
        return 1

    print(
        "semver recovery evidence accepted: "
        f"run {args.evidence_run_id}, job {args.evidence_job_id}, "
        f"tag {args.release_tag}, SHA {args.release_sha}, "
        f"{len(findings)} missing-declaration finding(s) now declared"
    )
    for finding in findings:
        print(f"  [{finding.crate}] {finding.lint}: {', '.join(finding.symbols)}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
