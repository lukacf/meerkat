#!/usr/bin/env python3
"""Semantic release-workflow contract checks for the release doctor.

The doctor used to assert release-workflow behaviour by grepping literal lines
out of `.github/workflows/release.yml`. Two of those greps went stale the
moment the workflow was reflowed (a folded `if: >-` condition and a
`--slo-seconds ${{ ... }}` expression), so `make release-doctor` failed on a
main branch whose behaviour had not changed (#1091).

This module asserts what the workflow DOES instead of how it is spelled. It
extracts a job, splits it into steps, and evaluates each step's `if:` and the
`${{ }}` expressions in its `run:` body under concrete event contexts (a tag
push, a package-recovery dispatch, an explicit historical-evidence dispatch).
Reflowing a condition across lines, collapsing it onto one line, or rewriting
an expression into an equivalent one all pass; gating a step off tag pushes,
re-enabling the long measurement on tags, or relaxing the 30 minute SLO all
fail and name the defect.

Only the Python standard library is used, so the doctor and its contract test
run wherever `python3` does.
"""

from __future__ import annotations

import argparse
import re
import sys
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field
from pathlib import Path

DEFAULT_WORKFLOW = Path(".github/workflows/release.yml")
TAG_SLO_SECONDS = 1800

SEMVER_GATE_JOB = "release_semver_gate"
REGISTRY_JOB = "publish_registries"
# The evidence step is the one that resolves the exact-tree readiness artifact.
EVIDENCE_ARTIFACT_PREFIX = "meerkat-semver-attestation-main-"
# The long measurement the tag path must never rerun.
MEASUREMENT_COMMAND = "make semver-breaks"
PUBLIC_VERIFIER = "scripts/verify-rust-release-public.py"


class ContractError(Exception):
    """A structural precondition the checker cannot see past."""


class UnsupportedExpression(ContractError):
    """The workflow uses expression syntax this evaluator does not model."""


# --------------------------------------------------------------------------
# Workflow extraction (line-oriented, no YAML dependency)
# --------------------------------------------------------------------------

JOB_HEADER = re.compile(r"^  ([A-Za-z0-9_-]+):\s*(?:#.*)?$")
STEP_START = re.compile(r"^      - ")
KEY_LINE = re.compile(r"^(\s*)([A-Za-z0-9_-]+):(.*)$")


def _indent(line: str) -> int:
    return len(line) - len(line.lstrip(" "))


def job_block(text: str, job_name: str) -> list[str]:
    lines = text.splitlines()
    start = None
    for index, line in enumerate(lines):
        match = JOB_HEADER.match(line)
        if match and match.group(1) == job_name:
            start = index
            break
    if start is None:
        raise ContractError(f"job `{job_name}` is not defined in the workflow")
    end = len(lines)
    for index in range(start + 1, len(lines)):
        if JOB_HEADER.match(lines[index]):
            end = index
            break
    return lines[start + 1 : end]


def parse_mapping(lines: list[str], indent: int) -> dict[str, str]:
    """Collect `key: value` pairs at exactly `indent`, folding nested scalars.

    A value on the key line is kept verbatim (plus any more-indented plain
    continuation lines). A block scalar (`|`, `>`, with optional chomping
    indicator) or a nested mapping is folded into one string: literal blocks
    keep newlines, everything else is joined with single spaces. Comment and
    blank lines between keys are skipped.
    """
    mapping: dict[str, str] = {}
    index = 0
    while index < len(lines):
        line = lines[index]
        stripped = line.strip()
        if not stripped or stripped.startswith("#"):
            index += 1
            continue
        match = KEY_LINE.match(line)
        if not match or len(match.group(1)) != indent:
            index += 1
            continue
        key = match.group(2)
        remainder = match.group(3).strip()
        index += 1
        continuation: list[str] = []
        while index < len(lines):
            candidate = lines[index]
            if candidate.strip() and _indent(candidate) <= indent:
                break
            continuation.append(candidate)
            index += 1
        if re.fullmatch(r"[|>][+-]?", remainder):
            joiner = "\n" if remainder.startswith("|") else " "
            body = [entry.strip() for entry in continuation if entry.strip()]
            mapping[key] = joiner.join(body)
        else:
            parts = [remainder] if remainder else []
            parts.extend(entry.strip() for entry in continuation if entry.strip())
            mapping[key] = " ".join(parts)
    return mapping


@dataclass
class Step:
    fields: dict[str, str]

    @property
    def name(self) -> str:
        return self.fields.get("name", "<unnamed step>")

    @property
    def condition(self) -> str | None:
        return self.fields.get("if")

    @property
    def run(self) -> str:
        return self.fields.get("run", "")


def job_steps(block: list[str]) -> list[Step]:
    steps: list[Step] = []
    in_steps = False
    current: list[str] | None = None
    for line in block:
        if re.match(r"^    steps:\s*$", line):
            in_steps = True
            continue
        if not in_steps:
            continue
        if line.strip() and _indent(line) < 6:
            break
        if STEP_START.match(line):
            if current is not None:
                steps.append(Step(parse_mapping(current, 8)))
            current = ["        " + line[8:]]
            continue
        if current is not None:
            current.append(line)
    if current is not None:
        steps.append(Step(parse_mapping(current, 8)))
    if not steps:
        raise ContractError("job defines no steps")
    return steps


# --------------------------------------------------------------------------
# GitHub Actions expression evaluation (the subset release.yml uses)
# --------------------------------------------------------------------------

TOKEN = re.compile(
    r"\s*(?:"
    r"(?P<string>'(?:[^']|'')*')"
    r"|(?P<op>&&|\|\||==|!=|!|\(|\)|,)"
    r"|(?P<number>\d+(?:\.\d+)?)"
    r"|(?P<ident>[A-Za-z_][A-Za-z0-9_.-]*)"
    r")"
)


@dataclass(frozen=True)
class EventContext:
    """The `github` and `needs` contexts of one hypothetical workflow run."""

    label: str
    event_name: str
    ref: str = "refs/tags/v0.0.0"
    inputs: dict[str, str] = field(default_factory=dict)
    needs_result: str = "success"

    def resolve(self, path: str) -> str:
        if path == "github.event_name":
            return self.event_name
        if path == "github.ref":
            return self.ref
        if path == "github.ref_name":
            return self.ref.rsplit("/", 1)[-1]
        if path.startswith("github.event.inputs."):
            # Unset dispatch inputs and push events both read as empty.
            return self.inputs.get(path[len("github.event.inputs.") :], "")
        needs = re.fullmatch(r"needs\.[A-Za-z0-9_-]+\.result", path)
        if needs:
            return self.needs_result
        raise UnsupportedExpression(
            f"context `{path}` is not modelled by the release doctor"
        )


def truthy(value: object) -> bool:
    if isinstance(value, bool):
        return value
    if value is None:
        return False
    return value != ""


def _equal(left: object, right: object) -> bool:
    # GitHub compares strings case-insensitively and coerces null to ''.
    def norm(value: object) -> str:
        if value is None:
            return ""
        if isinstance(value, bool):
            return "true" if value else "false"
        return str(value).lower()

    return norm(left) == norm(right)


FUNCTIONS: dict[str, Callable[[list[object]], object]] = {
    "always": lambda args: True,
    "startsWith": lambda args: str(args[0]).lower().startswith(str(args[1]).lower()),
    "endsWith": lambda args: str(args[0]).lower().endswith(str(args[1]).lower()),
    "contains": lambda args: str(args[1]).lower() in str(args[0]).lower(),
}


class _Parser:
    def __init__(self, expression: str, context: EventContext) -> None:
        self.context = context
        self.tokens: list[tuple[str, str]] = []
        position = 0
        expression = expression.strip()
        while position < len(expression):
            match = TOKEN.match(expression, position)
            if not match or match.end() == position:
                raise UnsupportedExpression(
                    f"cannot tokenise expression near `{expression[position : position + 20]}`"
                )
            position = match.end()
            kind = match.lastgroup
            if kind is None:
                continue
            self.tokens.append((kind, match.group(kind)))
        self.index = 0

    def peek(self) -> tuple[str, str] | None:
        return self.tokens[self.index] if self.index < len(self.tokens) else None

    def take(self) -> tuple[str, str]:
        token = self.peek()
        if token is None:
            raise UnsupportedExpression("unexpected end of expression")
        self.index += 1
        return token

    def expect_op(self, op: str) -> None:
        token = self.take()
        if token != ("op", op):
            raise UnsupportedExpression(f"expected `{op}`, found `{token[1]}`")

    def parse(self) -> object:
        value = self.parse_or()
        if self.peek() is not None:
            raise UnsupportedExpression(f"trailing token `{self.peek()[1]}`")
        return value

    def parse_or(self) -> object:
        left = self.parse_and()
        while self.peek() == ("op", "||"):
            self.take()
            right = self.parse_and()
            left = left if truthy(left) else right
        return left

    def parse_and(self) -> object:
        left = self.parse_equality()
        while self.peek() == ("op", "&&"):
            self.take()
            right = self.parse_equality()
            left = right if truthy(left) else left
        return left

    def parse_equality(self) -> object:
        left = self.parse_unary()
        while self.peek() in (("op", "=="), ("op", "!=")):
            _, op = self.take()
            right = self.parse_unary()
            equal = _equal(left, right)
            left = equal if op == "==" else not equal
        return left

    def parse_unary(self) -> object:
        if self.peek() == ("op", "!"):
            self.take()
            return not truthy(self.parse_unary())
        return self.parse_primary()

    def parse_primary(self) -> object:
        kind, text = self.take()
        if kind == "op" and text == "(":
            value = self.parse_or()
            self.expect_op(")")
            return value
        if kind == "string":
            return text[1:-1].replace("''", "'")
        if kind == "number":
            return text
        if kind == "ident":
            lowered = text.lower()
            if lowered in ("true", "false"):
                return lowered == "true"
            if lowered == "null":
                return None
            if self.peek() == ("op", "("):
                self.take()
                args: list[object] = []
                if self.peek() != ("op", ")"):
                    args.append(self.parse_or())
                    while self.peek() == ("op", ","):
                        self.take()
                        args.append(self.parse_or())
                self.expect_op(")")
                function = FUNCTIONS.get(text)
                if function is None:
                    raise UnsupportedExpression(f"function `{text}()` is not modelled")
                return function(args)
            return self.context.resolve(text)
        raise UnsupportedExpression(f"unexpected token `{text}`")


EXPRESSION = re.compile(r"\$\{\{(.*?)\}\}", re.DOTALL)


def evaluate(expression: str, context: EventContext) -> object:
    """Evaluate one expression, with or without the `${{ }}` wrapper."""
    expression = " ".join(expression.split())
    match = re.fullmatch(r"\$\{\{(.*)\}\}", expression)
    if match:
        expression = match.group(1)
    return _Parser(expression, context).parse()


def step_runs(step: Step, context: EventContext) -> bool:
    condition = step.condition
    if condition is None:
        return True
    return truthy(evaluate(condition, context))


def render(template: str, context: EventContext) -> str:
    """Substitute every evaluable `${{ }}` in a run body; leave the rest."""

    def substitute(match: re.Match[str]) -> str:
        try:
            value = evaluate(match.group(1), context)
        except UnsupportedExpression:
            return match.group(0)
        if value is None:
            return ""
        if isinstance(value, bool):
            return "true" if value else "false"
        return str(value)

    return EXPRESSION.sub(substitute, template)


# --------------------------------------------------------------------------
# Event contexts
# --------------------------------------------------------------------------

TAG_PUSH = EventContext(label="a tag push", event_name="push")
PACKAGE_RECOVERY = EventContext(
    label="a package-recovery dispatch",
    event_name="workflow_dispatch",
    inputs={"release_tag": "v0.0.0", "publish_release_packages": "true"},
)
HISTORICAL_EVIDENCE = EventContext(
    label="an explicit historical-evidence dispatch",
    event_name="workflow_dispatch",
    inputs={
        "release_tag": "v0.0.0",
        "publish_release_packages": "true",
        "semver_evidence_run_id": "1",
        "semver_evidence_job_id": "2",
    },
)


# --------------------------------------------------------------------------
# Checks
# --------------------------------------------------------------------------


def _job_enabled(block: list[str], job_name: str, context: EventContext) -> list[str]:
    job_fields = parse_mapping(block, 4)
    condition = job_fields.get("if")
    if condition is not None and not truthy(evaluate(condition, context)):
        return [f"job `{job_name}` is skipped on {context.label}"]
    return []


def check_semver_evidence(text: str) -> list[str]:
    """Tag releases consume exact-tree pre-tag evidence, never re-measure."""
    block = job_block(text, SEMVER_GATE_JOB)
    violations = _job_enabled(block, SEMVER_GATE_JOB, TAG_PUSH)
    steps = job_steps(block)

    evidence_steps = [step for step in steps if EVIDENCE_ARTIFACT_PREFIX in step.run]
    if not evidence_steps:
        violations.append(
            f"job `{SEMVER_GATE_JOB}` has no step that resolves the exact-tree "
            f"`{EVIDENCE_ARTIFACT_PREFIX}<tree>` readiness artifact"
        )
    for step in evidence_steps:
        for context in (TAG_PUSH, PACKAGE_RECOVERY):
            if not step_runs(step, context):
                violations.append(
                    f"step `{step.name}` does not run on {context.label}, so the "
                    "release would not reuse exact-tree pre-tag semver evidence"
                )
        if step_runs(step, HISTORICAL_EVIDENCE):
            violations.append(
                f"step `{step.name}` also runs on {HISTORICAL_EVIDENCE.label}, "
                "which must verify the explicit measurement instead"
            )

    for step in steps:
        if MEASUREMENT_COMMAND not in step.run:
            continue
        for context in (TAG_PUSH, PACKAGE_RECOVERY):
            if step_runs(step, context):
                violations.append(
                    f"step `{step.name}` reruns `{MEASUREMENT_COMMAND}` on "
                    f"{context.label}; the long measurement belongs before the tag"
                )
    return violations


def check_registry_slo(text: str) -> list[str]:
    """Tag releases verify every crate is public within the 30 minute SLO."""
    block = job_block(text, REGISTRY_JOB)
    violations = _job_enabled(block, REGISTRY_JOB, TAG_PUSH)
    steps = job_steps(block)

    verifier_steps = [step for step in steps if PUBLIC_VERIFIER in step.run]
    if not verifier_steps:
        violations.append(
            f"job `{REGISTRY_JOB}` has no step that runs `{PUBLIC_VERIFIER}`"
        )
    for step in verifier_steps:
        if not step_runs(step, TAG_PUSH):
            violations.append(f"step `{step.name}` does not run on {TAG_PUSH.label}")
            continue
        rendered = render(step.run, TAG_PUSH)
        match = re.search(r"--slo-seconds[\s=]+(\S+)", rendered)
        if not match:
            violations.append(
                f"step `{step.name}` invokes `{PUBLIC_VERIFIER}` without `--slo-seconds`"
            )
            continue
        value = match.group(1)
        if "${{" in value:
            violations.append(
                f"step `{step.name}` passes `--slo-seconds` as an expression the "
                f"release doctor cannot evaluate: `{value}`"
            )
        elif value != str(TAG_SLO_SECONDS):
            violations.append(
                f"step `{step.name}` passes `--slo-seconds {value}` on {TAG_PUSH.label}; "
                f"the tag-to-public SLO is {TAG_SLO_SECONDS} seconds"
            )
    return violations


CHECKS: dict[str, Callable[[str], list[str]]] = {
    "semver-evidence": check_semver_evidence,
    "registry-slo": check_registry_slo,
}


def run_checks(text: str, names: Iterable[str]) -> list[str]:
    violations: list[str] = []
    for name in names:
        try:
            violations.extend(CHECKS[name](text))
        except ContractError as error:
            violations.append(f"{name}: {error}")
    return violations


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--workflow",
        type=Path,
        default=DEFAULT_WORKFLOW,
        help=f"release workflow to inspect (default: {DEFAULT_WORKFLOW})",
    )
    parser.add_argument(
        "checks",
        nargs="*",
        choices=[*CHECKS, "all"],
        default=["all"],
        help="which contract checks to run (default: all)",
    )
    args = parser.parse_args(argv)
    names = list(CHECKS) if "all" in args.checks else args.checks
    try:
        text = args.workflow.read_text(encoding="utf-8")
    except OSError as error:
        print(f"cannot read {args.workflow}: {error}")
        return 2
    violations = run_checks(text, names)
    for violation in violations:
        print(violation)
    return 1 if violations else 0


if __name__ == "__main__":
    sys.exit(main())
