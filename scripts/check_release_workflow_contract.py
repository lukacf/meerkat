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
re-enabling the long measurement on tags, relaxing the 30 minute publication
SLO, making the SDK packages wait on it, re-running Cargo validation on a
tag whose exact-tree CI attestation already covers it, trusting CI or
semver evidence from anything but a push to main or to the release branch
of the tag's version, or letting a pre-tag
prebuild publish anything or a tag publish a prebuild's artifacts without the
attested-archive verification, making a job wait for the semver baseline
rustdoc, or letting a dispatch save a cache (also inside a local action, or
from another workflow) that a token-holding or attesting job restores all
fail and name the defect.

Only the Python standard library is used, so the doctor and its contract test
run wherever `python3` does.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import shlex
import subprocess
import sys
import tempfile
import zipfile
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field, replace
from functools import lru_cache
from pathlib import Path

DEFAULT_WORKFLOW = Path(".github/workflows/release.yml")
READINESS_WORKFLOW = Path(".github/workflows/release-semver-readiness.yml")
TAG_SLO_SECONDS = 1800

SEMVER_GATE_JOB = "release_semver_gate"
CI_GREEN_JOB = "require_ci_green"
CARGO_VALIDATION_JOB = "release_validate_cargo"
VALIDATION_GATE_JOB = "release_validate_gate"
# require_ci_green's output naming the verified exact-tree attestation schema.
ATTESTATION_SCHEMA_OUTPUT = "ci_attestation_schema"
# require_ci_green's output: the attestation proves the Cargo validation lanes ran.
COVERS_OUTPUT = "ci_covers_release_validation"
GCP_GRAPH_VARIABLE = "MEERKAT_GCP_BUILDBUDDY_ENABLED"
RELEASE_VERSION_STEP_ID = "release_version"
REGISTRY_JOB = "publish_registries"
# The evidence step is the one that resolves the exact-tree readiness artifact.
EVIDENCE_ARTIFACT_PREFIX = "meerkat-semver-attestation-main-"
# The long measurement the tag path must never rerun.
MEASUREMENT_COMMAND = "make semver-breaks"
# The release's own measurement must refuse a post-release tree: measured
# against its own tag it would pass and publish main's tip as that version.
RELEASE_TREE_ENV = "MEERKAT_SEMVER_REQUIRE_RELEASE_TREE"
READINESS_JOB = "semver"
NOTES_STEP_ID = "notes"
NOTES_OUTPUT = "baseline"
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
    lines: list[str] = field(default_factory=list)

    @property
    def name(self) -> str:
        return self.fields.get("name", "<unnamed step>")

    @property
    def condition(self) -> str | None:
        return self.fields.get("if")

    @property
    def run(self) -> str:
        return self.fields.get("run", "")

    @property
    def raw_run(self) -> str:
        """The `run:` block scalar with its relative indentation kept.

        `run` strips every line, which is enough for shell; an embedded
        heredoc in an indentation-sensitive language needs the original
        layout.
        """
        for index, line in enumerate(self.lines):
            match = KEY_LINE.match(line)
            if not (match and len(match.group(1)) == 8 and match.group(2) == "run"):
                continue
            body: list[str] = []
            for candidate in self.lines[index + 1 :]:
                if candidate.strip() and _indent(candidate) <= 8:
                    break
                body.append(candidate)
            width = min((_indent(line) for line in body if line.strip()), default=0)
            return "\n".join(line[width:] for line in body).strip("\n") + "\n"
        return ""

    @property
    def env(self) -> dict[str, str]:
        """The step's `env:` mapping, values unquoted."""
        return self.nested("env")

    def nested(self, name: str) -> dict[str, str]:
        """The step's `<name>:` mapping (`env`, `with`), values unquoted."""
        for index, line in enumerate(self.lines):
            match = KEY_LINE.match(line)
            if match and len(match.group(1)) == 8 and match.group(2) == name:
                nested: list[str] = []
                for candidate in self.lines[index + 1 :]:
                    if candidate.strip() and _indent(candidate) <= 8:
                        break
                    nested.append(candidate)
                return {
                    key: value.strip().strip("'\"")
                    for key, value in parse_mapping(nested, 10).items()
                }
        return {}

    def written_outputs(self) -> set[str]:
        """Output names the step's `run:` writes into `$GITHUB_OUTPUT`."""
        names: set[str] = set()
        for line in self.run.splitlines():
            if "GITHUB_OUTPUT" not in line:
                continue
            match = re.search(r"""echo\s+["']?([A-Za-z_][A-Za-z0-9_-]*)=""", line)
            if match:
                names.add(match.group(1))
        return names


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
                steps.append(Step(parse_mapping(current, 8), current))
            current = ["        " + line[8:]]
            continue
        if current is not None:
            current.append(line)
    if current is not None:
        steps.append(Step(parse_mapping(current, 8), current))
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


def github_ref_name(ref: str) -> str:
    """`github.ref_name` / `GITHUB_REF_NAME` as GitHub sets it: the ref with
    its `refs/heads/` or `refs/tags/` prefix removed (for other refs, its
    `refs/<kind>/` prefix), so `refs/tags/alpha/v1.2.3` is `alpha/v1.2.3`,
    not its last path segment."""
    for prefix in ("refs/heads/", "refs/tags/"):
        if ref.startswith(prefix):
            return ref[len(prefix) :]
    parts = ref.split("/", 2)
    if len(parts) == 3 and parts[0] == "refs":
        return parts[2]
    return ref


@dataclass(frozen=True)
class EventContext:
    """The `github` and `needs` contexts of one hypothetical workflow run."""

    label: str
    event_name: str
    ref: str = "refs/tags/v0.0.0"
    inputs: dict[str, str] = field(default_factory=dict)
    needs_result: str = "success"
    # `<step id>.<output>` -> value, for the outputs this context models. Any
    # other `steps.*.outputs.*` reference raises UnsupportedExpression.
    step_outputs: dict[str, str] = field(default_factory=dict)
    actor: str = "release-bot"
    sha: str = "0" * 40
    run_id: str = "1"
    # Repository variables; an unset variable reads as empty, as on GitHub.
    variables: dict[str, str] = field(default_factory=dict)
    # job -> result, for upstream jobs whose result differs from needs_result.
    job_results: dict[str, str] = field(default_factory=dict)
    # `<job>.<output>` -> value, for the upstream outputs this context models.
    # Any other `needs.*.outputs.*` reference raises UnsupportedExpression.
    job_outputs: dict[str, str] = field(default_factory=dict)

    def resolve(self, path: str) -> str:
        if path == "github.event_name":
            return self.event_name
        if path == "github.ref":
            return self.ref
        if path == "github.ref_name":
            return github_ref_name(self.ref)
        if path == "github.actor":
            return self.actor
        if path == "github.sha":
            return self.sha
        if path == "github.run_id":
            return self.run_id
        if path == "github.token":
            # Never a real credential: scripts run here only against stubs.
            return "contract-check-token"
        if path.startswith("github.event.inputs."):
            # Unset dispatch inputs and push events both read as empty.
            return self.inputs.get(path[len("github.event.inputs.") :], "")
        variable = re.fullmatch(r"vars\.([A-Za-z0-9_]+)", path)
        if variable:
            return self.variables.get(variable.group(1), "")
        needs = re.fullmatch(r"needs\.([A-Za-z0-9_-]+)\.result", path)
        if needs:
            return self.job_results.get(needs.group(1), self.needs_result)
        output = re.fullmatch(r"needs\.([A-Za-z0-9_-]+)\.outputs\.([A-Za-z0-9_-]+)", path)
        if output and f"{output.group(1)}.{output.group(2)}" in self.job_outputs:
            return self.job_outputs[f"{output.group(1)}.{output.group(2)}"]
        # Earlier steps of the same job are evaluated on the happy path, the
        # same assumption `needs_result` makes for upstream jobs.
        if re.fullmatch(r"steps\.[A-Za-z0-9_-]+\.(?:outcome|conclusion)", path):
            return self.needs_result
        # Only the step outputs a context models resolve; anything else stays
        # unsupported, so a gate added on an unmodelled output fails the check
        # closed instead of reading as an empty string.
        output = re.fullmatch(r"steps\.([A-Za-z0-9_-]+)\.outputs\.([A-Za-z0-9_-]+)", path)
        if output and f"{output.group(1)}.{output.group(2)}" in self.step_outputs:
            return self.step_outputs[f"{output.group(1)}.{output.group(2)}"]
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
    "success": lambda args: True,
    "failure": lambda args: False,
    "cancelled": lambda args: False,
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


def render_strict(template: str, context: EventContext) -> str:
    """Substitute every `${{ }}`; raise if any cannot be evaluated.

    For checks that must fail closed: an expression the checker does not
    model is a contract error, never a literal left in place.
    """

    def substitute(match: re.Match[str]) -> str:
        value = evaluate(match.group(1), context)
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


BRANCH_DISPATCHES = [
    EventContext(
        label=f"{'an' if mode[0] in 'aeiou' else 'a'} {mode} dispatch from {ref} without release_tag",
        event_name="workflow_dispatch",
        ref=ref,
        inputs=inputs,
    )
    # Branches named after the version, including exactly the allowed tag
    # names (github.ref_name is then an allowed tag, so a binding that reads
    # ref_name without checking refs/tags/ would take the branch for the tag).
    for ref in (
        "refs/heads/main",
        "refs/heads/release/v0.0.0",
        "refs/heads/hotfix/0.0.0",
        "refs/heads/v0.0.0",
        "refs/heads/alpha/v0.0.0",
    )
    for mode, inputs in (
        ("package", {"publish_release_packages": "true"}),
        ("alpha crate", {"publish_release_packages": "true", "alpha_crates_only": "true"}),
        ("Web-SDK-only", {"publish_web_sdk_only": "true"}),
        ("asset-only", {"publish_release_assets_only": "true"}),
    )
]


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
    """Tag releases consume exact-tree pre-tag evidence, never re-measure.

    The evidence must come from a push to main or to the release branch of
    the tag's version (a maintenance release), never from another branch, a
    pull request or a dispatch.
    """
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
        violations.extend(_check_evidence_refs(step))

    for step in steps:
        if MEASUREMENT_COMMAND not in step.run:
            continue
        if step.env.get(RELEASE_TREE_ENV) != "1":
            violations.append(
                f"step `{step.name}` runs `{MEASUREMENT_COMMAND}` without "
                f"`{RELEASE_TREE_ENV}: \"1\"`, so a post-release tree (notes above the "
                "stamped version) would pass and publish as the tagged version"
            )
        for context in (TAG_PUSH, PACKAGE_RECOVERY):
            if step_runs(step, context):
                violations.append(
                    f"step `{step.name}` reruns `{MEASUREMENT_COMMAND}` on "
                    f"{context.label}; the long measurement belongs before the tag"
                )
    return violations


# The version the binding scenarios run against.
BINDING_VERSION = "0.0.0"
CHECKOUT_ACTION = "actions/checkout"


def _run_binding_step(
    step: Step,
    context: EventContext,
    tags: tuple[str, ...],
    checkout_ref: str | None,
    later_refs: tuple[str, ...] = (),
) -> int:
    """Run the step's script as the runner would, in a scratch checkout.

    The repository has two commits: the release commit, which every tag in
    `tags` points at, and a later commit, which every branch points at, as do
    the `refs/tags/` and `refs/heads/` refs in `later_refs`. HEAD
    is what the job's checkout step selects for `context` (`checkout_ref`,
    rendered from its `with.ref`); `None` puts HEAD on the later commit, a
    checkout that did not land on the tag. A ref that does not resolve fails
    the checkout, which refuses the run as the runner would. Every `${{ }}`
    in the step's env and run must evaluate under `context`; one that does
    not raises UnsupportedExpression, so the check fails closed.
    """
    env = {key: render_strict(value, context) for key, value in step.env.items()}
    script = render_strict(step.run, context)
    with tempfile.TemporaryDirectory(prefix="release-binding-") as checkout:
        git_env = {
            "PATH": os.environ.get("PATH", ""),
            "HOME": checkout,
            "GIT_AUTHOR_NAME": "binding",
            "GIT_AUTHOR_EMAIL": "binding@example.invalid",
            "GIT_COMMITTER_NAME": "binding",
            "GIT_COMMITTER_EMAIL": "binding@example.invalid",
        }

        def git(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
            return subprocess.run(
                ["git", "-c", "commit.gpgsign=false", "-c", "tag.gpgsign=false", *args],
                cwd=checkout,
                env=git_env,
                check=check,
                capture_output=True,
                text=True,
            )

        cargo = Path(checkout, "Cargo.toml")
        cargo.write_text(f'[workspace.package]\nversion = "{BINDING_VERSION}"\n', encoding="utf-8")
        git("init", "-q", "-b", "binding-base")
        git("add", "Cargo.toml")
        git("commit", "-q", "-m", "release")
        for tag in tags:
            git("tag", tag)
        cargo.write_text(
            f'[workspace.package]\nversion = "{BINDING_VERSION}"\n# later\n', encoding="utf-8"
        )
        git("commit", "-q", "-am", "later")
        for later_ref in (context.ref, *later_refs):
            if later_ref.startswith("refs/heads/"):
                git("branch", "-f", later_ref[len("refs/heads/") :], "HEAD")
            elif later_ref.startswith("refs/tags/") and later_ref != context.ref:
                git("tag", "-f", later_ref[len("refs/tags/") :], "HEAD")
        if checkout_ref is not None:
            if git("checkout", "-q", "--detach", checkout_ref, check=False).returncode != 0:
                return 1
        result = subprocess.run(
            ["bash", "-c", script],
            cwd=checkout,
            env={
                **git_env,
                **env,
                "GITHUB_REF": context.ref,
                "GITHUB_REF_NAME": github_ref_name(context.ref),
                "GITHUB_EVENT_NAME": context.event_name,
            },
            capture_output=True,
            text=True,
            check=False,
        )
        return result.returncode


def check_dispatch_binding(text: str) -> list[str]:
    """Every publishing run is bound to an allowed TAG of its version.

    `require_ci_green` gates everything that publishes. Its version step must
    run on a tag push, on a dispatch that names release_tag, and on every
    publishing dispatch that names none, and it must refuse unless the run is
    bound to an allowed tag: `v<version>` in every mode, `alpha/v<version>`
    only for the crates-only alpha canary. The step's script runs against
    scratch checkouts in which HEAD is whatever the job's checkout step
    selects, so what is checked is what the job does, not how it is spelled:
    a branch named after the version, an alpha tag outside the alpha lane,
    and a named tag that is not the checked-out commit must not pass.
    """
    block = job_block(text, CI_GREEN_JOB)
    steps = job_steps(block)
    binding = [step for step in steps if step.fields.get("id") == RELEASE_VERSION_STEP_ID]
    if len(binding) != 1:
        return [
            f"job `{CI_GREEN_JOB}` has {len(binding)} steps with `id: {RELEASE_VERSION_STEP_ID}`; "
            "exactly one must bind the release ref to the workspace version"
        ]
    step = binding[0]
    checkouts = [
        candidate
        for candidate in steps[: steps.index(step)]
        if CHECKOUT_ACTION in candidate.fields.get("uses", "")
    ]
    if len(checkouts) != 1 or "ref" not in checkouts[0].nested("with"):
        return [
            f"job `{CI_GREEN_JOB}` must check out the release ref with exactly one "
            f"`{CHECKOUT_ACTION}` step carrying `with.ref` before `{step.name}`"
        ]
    checkout_expression = checkouts[0].nested("with")["ref"]

    def checkout_for(context: EventContext) -> str:
        return render_strict(checkout_expression, context)

    tag = f"v{BINDING_VERSION}"
    alpha_tag = f"alpha/{tag}"

    def dispatch(label: str, ref: str, **inputs: str) -> EventContext:
        return EventContext(label=label, event_name="workflow_dispatch", ref=ref, inputs=inputs)

    tag_push = EventContext(label=f"a {tag} tag push", event_name="push", ref=f"refs/tags/{tag}")
    named_tag = dispatch(
        f"a package dispatch naming release_tag {tag}",
        "refs/heads/main",
        release_tag=tag,
        publish_release_packages="true",
    )
    alpha_on_tag = dispatch(
        f"an alpha crate dispatch on the {alpha_tag} tag",
        f"refs/tags/{alpha_tag}",
        publish_release_packages="true",
        alpha_crates_only="true",
    )
    alpha_named = dispatch(
        f"an alpha crate dispatch naming release_tag {alpha_tag}",
        "refs/heads/main",
        release_tag=alpha_tag,
        publish_release_packages="true",
        alpha_crates_only="true",
    )
    alpha_without_lane = dispatch(
        f"a package dispatch on the {alpha_tag} tag without alpha_crates_only",
        f"refs/tags/{alpha_tag}",
        publish_release_packages="true",
    )
    alpha_named_without_lane = dispatch(
        f"a package dispatch naming release_tag {alpha_tag} without alpha_crates_only",
        "refs/heads/main",
        release_tag=alpha_tag,
        publish_release_packages="true",
    )
    other_tag_push = EventContext(
        label="a tag push of another version", event_name="push", ref="refs/tags/v9.9.9"
    )
    # A run on the version's tag that names another release_tag: the named
    # tag is what is checked out and published, so it must bind. A binding
    # that preferred github.ref would pass these and publish the named ref.
    tag_ref_naming_branch = dispatch(
        f"a package dispatch on the {tag} tag naming release_tag main (a branch)",
        f"refs/tags/{tag}",
        release_tag="main",
        publish_release_packages="true",
    )
    tag_ref_naming_other_tag = dispatch(
        f"a package dispatch on the {tag} tag naming release_tag v9.9.9 "
        "(a tag on another commit)",
        f"refs/tags/{tag}",
        release_tag="v9.9.9",
        publish_release_packages="true",
    )
    # The same with both tags on one commit (as an alpha tag and its
    # version's tag usually are): outside the alpha lane the named alpha tag
    # must not bind, even though the version's tag is the checked-out commit.
    tag_ref_naming_alpha_same_commit = dispatch(
        f"a package dispatch on the {tag} tag naming release_tag {alpha_tag} "
        "(both tags on one commit, no alpha lane)",
        f"refs/tags/{tag}",
        release_tag=alpha_tag,
        publish_release_packages="true",
    )
    # Near misses of the allowed names: exact matches only.
    near_miss_push = EventContext(
        label=f"a tag push of {tag}1 (a near miss of {tag})",
        event_name="push",
        ref=f"refs/tags/{tag}1",
    )
    near_miss_alpha_rc = dispatch(
        f"an alpha crate dispatch on the {alpha_tag}-rc.1 tag (a near miss of {alpha_tag})",
        f"refs/tags/{alpha_tag}-rc.1",
        publish_release_packages="true",
        alpha_crates_only="true",
    )
    near_miss_alpha = dispatch(
        f"an alpha crate dispatch on the {alpha_tag}1 tag (a near miss of {alpha_tag})",
        f"refs/tags/{alpha_tag}1",
        publish_release_packages="true",
        alpha_crates_only="true",
    )
    # The alpha lane binds only its own version's alpha tag.
    other_alpha_on_tag = dispatch(
        "an alpha crate dispatch on the alpha/v9.9.9 tag (another version)",
        "refs/tags/alpha/v9.9.9",
        publish_release_packages="true",
        alpha_crates_only="true",
    )
    other_alpha_named = dispatch(
        "an alpha crate dispatch naming release_tag alpha/v9.9.9 (another version)",
        "refs/heads/main",
        release_tag="alpha/v9.9.9",
        publish_release_packages="true",
        alpha_crates_only="true",
    )
    # (context, tags, head) where head None means "HEAD off the tag".
    must_accept = [
        (tag_push, (tag,), checkout_for(tag_push)),
        (named_tag, (tag,), checkout_for(named_tag)),
        (alpha_on_tag, (alpha_tag,), checkout_for(alpha_on_tag)),
        (alpha_named, (alpha_tag,), checkout_for(alpha_named)),
    ]
    must_refuse = [
        (other_tag_push, ("v9.9.9",), checkout_for(other_tag_push), ""),
        (
            tag_ref_naming_alpha_same_commit,
            (tag, alpha_tag),
            checkout_for(tag_ref_naming_alpha_same_commit),
            "",
        ),
        (near_miss_push, (f"{tag}1",), checkout_for(near_miss_push), ""),
        (near_miss_alpha_rc, (f"{alpha_tag}-rc.1",), checkout_for(near_miss_alpha_rc), ""),
        (near_miss_alpha, (f"{alpha_tag}1",), checkout_for(near_miss_alpha), ""),
        (other_alpha_on_tag, ("alpha/v9.9.9",), checkout_for(other_alpha_on_tag), ""),
        (other_alpha_named, ("alpha/v9.9.9",), checkout_for(other_alpha_named), ""),
        (alpha_without_lane, (alpha_tag,), checkout_for(alpha_without_lane), ""),
        (alpha_named_without_lane, (alpha_tag,), checkout_for(alpha_named_without_lane), ""),
        # release_tag names the version but no such tag exists (a branch).
        (named_tag, (), None, " (no such tag exists)"),
        # The tag exists but the checked-out commit is a later one.
        (named_tag, (tag,), None, " (the tag is not the checked-out commit)"),
        # Every branch dispatch runs with the allowed tags present on the
        # release commit (the branch is on the later one), so a binding that
        # takes a branch named like a tag for that tag, because the tag
        # exists, is refused too.
        *(
            (context, (tag, alpha_tag), checkout_for(context), "")
            for context in BRANCH_DISPATCHES
        ),
    ]
    violations: list[str] = []
    for context in [
        *(entry[0] for entry in must_accept + must_refuse),
        tag_ref_naming_branch,
        tag_ref_naming_other_tag,
    ]:
        if not step_runs(step, context):
            violations.append(
                f"step `{step.name}` does not run on {context.label}, so nothing binds "
                "that publication to its version's tag"
            )
    if violations:
        return violations
    for context, tags, head in must_accept:
        if _run_binding_step(step, context, tags, head) != 0:
            violations.append(f"step `{step.name}` refuses {context.label}")
    for context, tags, head, detail in must_refuse:
        if _run_binding_step(step, context, tags, head) == 0:
            violations.append(
                f"step `{step.name}` accepts {context.label}{detail}; only an allowed tag "
                f"({tag}, or {alpha_tag} in the alpha lane) may publish {BINDING_VERSION}"
            )
    # The named ref sits on the later commit, where the checkout lands.
    for context, later_ref in (
        (tag_ref_naming_branch, "refs/heads/main"),
        (tag_ref_naming_other_tag, "refs/tags/v9.9.9"),
    ):
        if _run_binding_step(step, context, (tag,), checkout_for(context), (later_ref,)) == 0:
            violations.append(
                f"step `{step.name}` accepts {context.label}; the named release_tag is "
                f"what is checked out, and only an allowed tag ({tag}, or {alpha_tag} in "
                f"the alpha lane) may publish {BINDING_VERSION}"
            )
    return violations


READBACK_FLAG = "--readback-only"


def _is_sdk_publish(step: Step) -> bool:
    return step.name.startswith("Publish ") and step.name.endswith(" SDK")


def check_registry_slo(text: str) -> list[str]:
    """Tag releases read every crate back before the SDK packages publish, then
    enforce the 30 minute crates.io publication SLO without blocking them."""
    block = job_block(text, REGISTRY_JOB)
    violations = _job_enabled(block, REGISTRY_JOB, TAG_PUSH)
    steps = job_steps(block)

    verifier = [
        (index, step) for index, step in enumerate(steps) if PUBLIC_VERIFIER in step.run
    ]
    readback = [(i, s) for i, s in verifier if READBACK_FLAG in s.run]
    slo = [(i, s) for i, s in verifier if READBACK_FLAG not in s.run]
    sdk_indexes = [i for i, s in enumerate(steps) if _is_sdk_publish(s)]

    if not readback:
        violations.append(
            f"job `{REGISTRY_JOB}` has no step that reads every crate back with "
            f"`{PUBLIC_VERIFIER} {READBACK_FLAG}` before the SDK packages publish"
        )
    for index, step in readback:
        if not step_runs(step, TAG_PUSH):
            violations.append(f"step `{step.name}` does not run on {TAG_PUSH.label}")
        if sdk_indexes and index > min(sdk_indexes):
            violations.append(
                f"step `{step.name}` runs after an SDK publish step; the SDK "
                "packages must follow the crate readback"
            )

    if not slo:
        violations.append(
            f"job `{REGISTRY_JOB}` has no step that enforces the publication SLO "
            f"with `{PUBLIC_VERIFIER} --slo-seconds`"
        )
    for index, step in slo:
        if not step_runs(step, TAG_PUSH):
            violations.append(f"step `{step.name}` does not run on {TAG_PUSH.label}")
            continue
        if sdk_indexes and index < max(sdk_indexes):
            violations.append(
                f"step `{step.name}` runs before an SDK publish step; SDK "
                "publication must not wait on the publication SLO"
            )
        rendered = render(step.run, TAG_PUSH)
        if "--window-started-at" not in rendered and "--tag-pushed-at" not in rendered:
            violations.append(
                f"step `{step.name}` gives `{PUBLIC_VERIFIER}` no SLO window start"
            )
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
                f"the publication SLO is {TAG_SLO_SECONDS} seconds"
            )
    return violations


def _attested_tag(covers: str, schema: str = "4", **kwargs: object) -> EventContext:
    """A tag push whose require_ci_green verified an attestation of `schema`
    that does (covers == "true") or does not cover the Cargo lanes."""
    return EventContext(
        label=kwargs.pop("label", f"a tag push whose attestation covers the Cargo lanes ({covers})"),
        event_name="push",
        job_outputs={
            f"{CI_GREEN_JOB}.{ATTESTATION_SCHEMA_OUTPUT}": schema,
            f"{CI_GREEN_JOB}.{COVERS_OUTPUT}": covers,
        },
        **kwargs,
    )


def _runner_env(context: EventContext) -> dict[str, str]:
    """The default environment a runner gives every step of `context`."""
    return {
        "PATH": os.environ.get("PATH", ""),
        "GITHUB_SHA": context.sha,
        "GITHUB_REF": context.ref,
        "GITHUB_REF_NAME": github_ref_name(context.ref),
        "GITHUB_EVENT_NAME": context.event_name,
        "GITHUB_REPOSITORY": "example/meerkat",
        "GITHUB_RUN_ID": context.run_id,
    }


def _run_gate_script(step: Step, context: EventContext) -> tuple[int, str]:
    """Run a gate job's shell step as the runner would."""
    env = {key: render_strict(value, context) for key, value in step.env.items()}
    script = render_strict(step.run, context)
    result = subprocess.run(
        ["bash", "-c", script],
        env={**_runner_env(context), **env},
        capture_output=True,
        text=True,
        check=False,
    )
    return result.returncode, result.stdout + result.stderr


def _attestation(**overrides: object) -> dict:
    """A schema-4 attestation for the binding scenario, every lane green."""
    document: dict = {
        "schema_version": 4,
        "repository": "example/meerkat",
        "commit_sha": "1" * 40,
        "tree_sha": "2" * 40,
        "workflow": "CI",
        "workflow_run_id": "11",
        "workflow_run_attempt": "1",
        "event": "push",
        "ref": "refs/heads/main",
        "validation_backend": "github-hosted-cargo",
        "validation_result": "success",
        "plan": {"mode": "workspace", "generated_contract": True},
        "component_results": {
            "changes": "success",
            "fmt_governance": "success",
            "ratchets": "success",
            "sdk_host": "success",
        },
    }
    for path, value in overrides.items():
        target = document
        keys = path.split("__")
        for key in keys[:-1]:
            target = target.setdefault(key, {})
        if value is None:
            target.pop(keys[-1], None)
        else:
            target[keys[-1]] = value
    return document


def _reused_evidence(**overrides: str) -> dict:
    """reused_test_evidence as CI writes it for a release-projection plan."""
    return {
        "classifier": "release-projection-only",
        "from_version": "0.0.0",
        "to_version": "0.0.1",
        "base_commit_sha": "3" * 40,
        "base_tree_sha": "4" * 40,
        "workflow": "CI",
        "workflow_run_id": "9",
        "workflow_run_attempt": "1",
        "attestation_artifact": "meerkat-ci-attestation-" + "3" * 40 + "-1",
        **overrides,
    }


def _run_attestation_step(step: Step, attestation: dict) -> tuple[int, dict[str, str], str]:
    """Run a require_ci_green attestation step on `attestation`, in a scratch
    checkout whose Cargo.toml carries BINDING_VERSION (the tag's version):
    its exit, the outputs it wrote and its log."""
    context = EventContext(
        label=TAG_PUSH.label,
        event_name="push",
        step_outputs={
            "ci_run.run_id": "11",
            "ci_run.run_attempt": "1",
            "release_ref.sha": "1" * 40,
            "release_ref.tree": "2" * 40,
        },
    )
    env = {key: render_strict(value, context) for key, value in step.env.items()}
    with tempfile.TemporaryDirectory(prefix="release-attestation-") as scratch:
        Path(scratch, "Cargo.toml").write_text(
            f'[workspace.package]\nversion = "{BINDING_VERSION}"\n', encoding="utf-8"
        )
        directory = Path(scratch, "meerkat-ci-attestation")
        directory.mkdir()
        Path(directory, "attestation.json").write_text(json.dumps(attestation), encoding="utf-8")
        output = Path(scratch, "github-output")
        output.write_text("", encoding="utf-8")
        result = subprocess.run(
            ["bash", "-c", render_strict(step.raw_run, context)],
            cwd=scratch,
            env={
                **_runner_env(context),
                "GITHUB_REPOSITORY": "example/meerkat",
                "RUNNER_TEMP": scratch,
                "GITHUB_OUTPUT": str(output),
                **env,
            },
            capture_output=True,
            text=True,
            check=False,
        )
        outputs = dict(
            line.split("=", 1) for line in output.read_text(encoding="utf-8").splitlines() if "=" in line
        )
        return result.returncode, outputs, result.stdout + result.stderr


def _coverage_of(step: Step, attestation: dict) -> tuple[int, dict[str, str]]:
    """Run the attestation step on `attestation`; its exit and outputs."""
    status, outputs, _ = _run_attestation_step(step, attestation)
    return status, outputs


# The pushes whose CI attestation or readiness evidence a tag of
# BINDING_VERSION trusts: main, and release/<version>, the branch a
# maintenance release of that version is tagged on. (label, ref).
TRUSTED_PUSHES = (
    ("a push to main", "refs/heads/main"),
    (
        f"a push to release/{BINDING_VERSION} (a maintenance release of the tag's version)",
        f"refs/heads/release/{BINDING_VERSION}",
    ),
)
# Pushes no tag of BINDING_VERSION trusts: another version's release branch,
# near misses of the version's own, and other branches, some named after it.
UNTRUSTED_PUSHES = (
    ("a push to release/9.9.9 (another version's release branch)", "refs/heads/release/9.9.9"),
    (f"a push to release/v{BINDING_VERSION}", f"refs/heads/release/v{BINDING_VERSION}"),
    (f"a push to release/{BINDING_VERSION}-next", f"refs/heads/release/{BINDING_VERSION}-next"),
    (f"a push to release/{BINDING_VERSION}/next", f"refs/heads/release/{BINDING_VERSION}/next"),
    (f"a push to hotfix/{BINDING_VERSION}", f"refs/heads/hotfix/{BINDING_VERSION}"),
    (f"a push to v{BINDING_VERSION} (a branch)", f"refs/heads/v{BINDING_VERSION}"),
    ("a push to feature/next", "refs/heads/feature/next"),
)
TRUSTED_ONLY = (
    f"only a push to main or to release/{BINDING_VERSION}, the release branch of the "
    "tag's version, qualifies"
)


def _check_attestation_refs(ci_block: list[str]) -> list[str]:
    """The CI attestation require_ci_green verifies names a trusted push.

    It must come from a push to main or to the release branch of the
    checked-out version (a maintenance release, tagged there without reaching
    main); another branch, a tag, a pull request or a dispatch never
    qualifies. The verifying step is the one whose output require_ci_green
    exposes as the attestation schema, run on scratch attestations.
    """
    outputs = parse_mapping(ci_block, 4).get("outputs", "")
    wiring = re.search(
        rf"\b{ATTESTATION_SCHEMA_OUTPUT}:\s*\$\{{\{{\s*steps\.([A-Za-z0-9_-]+)\.outputs\.[A-Za-z0-9_-]+\s*\}}\}}",
        outputs,
    )
    if wiring is None:
        return [f"job `{CI_GREEN_JOB}` does not expose `{ATTESTATION_SCHEMA_OUTPUT}` from a step output"]
    verifiers = [step for step in job_steps(ci_block) if step.fields.get("id") == wiring.group(1)]
    if len(verifiers) != 1:
        return [f"job `{CI_GREEN_JOB}` has {len(verifiers)} steps with `id: {wiring.group(1)}`"]
    verifier = verifiers[0]
    violations: list[str] = []
    if not step_runs(verifier, TAG_PUSH):
        violations.append(f"step `{verifier.name}` does not run on {TAG_PUSH.label}")
    refused = [
        *((label, _attestation(ref=ref)) for label, ref in UNTRUSTED_PUSHES),
        (f"the tag v{BINDING_VERSION}", _attestation(ref=f"refs/tags/v{BINDING_VERSION}")),
        (
            "a pull request into a release branch",
            _attestation(event="pull_request", ref="refs/pull/1/merge"),
        ),
        (
            f"a dispatch on release/{BINDING_VERSION}",
            _attestation(event="workflow_dispatch", ref=f"refs/heads/release/{BINDING_VERSION}"),
        ),
    ]
    for label, ref in TRUSTED_PUSHES:
        status, _, log = _run_attestation_step(verifier, _attestation(ref=ref))
        if status != 0:
            violations.append(
                f"step `{verifier.name}` refuses the CI attestation of {label}: "
                + " | ".join(line for line in log.splitlines() if line.strip())
            )
    for label, attestation in refused:
        status, _, _ = _run_attestation_step(verifier, attestation)
        if status == 0:
            violations.append(
                f"step `{verifier.name}` accepts the CI attestation of {label}; {TRUSTED_ONLY}"
            )
    return violations


def _run_evidence_step(
    step: Step, evidence: dict, run: dict, newer: tuple[dict, dict] | None = None
) -> tuple[int, str]:
    """Run the semver gate's evidence step on one readiness artifact.

    The release tree is a scratch checkout of BINDING_VERSION. A stub `gh`
    answers the step's API reads as GitHub would: the unexpired artifact
    named after that tree, its zip (an attestation.json that `evidence`
    overrides, and the report it hashes) and the run that uploaded it, which
    `run` overrides; the artifact listing names that run and its branch.
    `newer`, an (evidence, run) pair, adds a second artifact of the same name
    uploaded later, as a push of the same tree to another branch uploads it.
    `python3` is this interpreter, which has tomllib. Returns the step's exit
    and log.
    """
    env = {key: render_strict(value, TAG_PUSH) for key, value in step.env.items()}
    script = render_strict(step.raw_run, TAG_PUSH)
    with tempfile.TemporaryDirectory(prefix="release-semver-evidence-") as scratch:
        checkout = Path(scratch, "checkout")
        checkout.mkdir()
        git_env = {
            "HOME": scratch,
            "GIT_AUTHOR_NAME": "evidence",
            "GIT_AUTHOR_EMAIL": "evidence@example.invalid",
            "GIT_COMMITTER_NAME": "evidence",
            "GIT_COMMITTER_EMAIL": "evidence@example.invalid",
        }

        def git(*args: str) -> str:
            return subprocess.run(
                ["git", "-c", "commit.gpgsign=false", *args],
                cwd=checkout,
                env={"PATH": os.environ.get("PATH", ""), **git_env},
                check=True,
                capture_output=True,
                text=True,
            ).stdout.strip()

        Path(checkout, "Cargo.toml").write_text(
            f'[workspace.package]\nversion = "{BINDING_VERSION}"\n', encoding="utf-8"
        )
        git("init", "-q", "-b", "release")
        git("add", "Cargo.toml")
        git("commit", "-q", "-m", "release")
        tree = git("rev-parse", "HEAD^{tree}")
        commit = git("rev-parse", "HEAD")

        api = Path(scratch, "api")
        api.mkdir()
        report = b"semver report\n"
        uploads = [("2026-01-01T00:00:00Z", evidence, run)]
        if newer is not None:
            uploads.append(("2026-01-02T00:00:00Z", *newer))
        artifacts: list[dict] = []
        routes: list[str] = []
        for index, (created_at, attested, producing) in enumerate(uploads):
            artifact_id, run_id = 7 + index, 21 + index
            document = {
                "schema_version": 1,
                "repository": "example/meerkat",
                "tree_sha": tree,
                "commit_sha": commit,
                "version": BINDING_VERSION,
                "workflow": "Release semver readiness",
                "workflow_run_id": str(run_id),
                "workflow_run_attempt": "1",
                "event": "push",
                "ref": "refs/heads/main",
                "result": "success",
                "report_sha256": hashlib.sha256(report).hexdigest(),
                **attested,
            }
            with zipfile.ZipFile(api / f"evidence-{artifact_id}.zip", "w") as archive:
                archive.writestr("attestation.json", json.dumps(document))
                archive.writestr("report.txt", report)
            producer = {
                "id": run_id,
                "name": "Release semver readiness",
                "event": "push",
                "head_branch": "main",
                "conclusion": "success",
                **producing,
            }
            Path(api, f"run-{run_id}.json").write_text(json.dumps(producer), encoding="utf-8")
            # Newest first, as GitHub lists them.
            artifacts.insert(
                0,
                {
                    "id": artifact_id,
                    "name": f"{EVIDENCE_ARTIFACT_PREFIX}{tree}",
                    "expired": False,
                    "created_at": created_at,
                    "workflow_run": {"id": run_id, "head_branch": producer["head_branch"]},
                },
            )
            routes.append(
                f'  "api "*"/actions/artifacts/{artifact_id}/zip") cat "$api/evidence-{artifact_id}.zip" ;;\n'
                f'  "api "*"/actions/runs/{run_id}") cat "$api/run-{run_id}.json" ;;\n'
            )
        listing = {"total_count": len(artifacts), "artifacts": artifacts}
        Path(api, "artifacts.json").write_text(json.dumps(listing), encoding="utf-8")

        tools = Path(scratch, "bin")
        tools.mkdir()
        gh = tools / "gh"
        gh.write_text(
            "#!/usr/bin/env bash\n"
            f"api={shlex.quote(str(api))}\n"
            'case "$1 $2" in\n'
            f'  "api "*"/actions/artifacts?name={EVIDENCE_ARTIFACT_PREFIX}{tree}"*) cat "$api/artifacts.json" ;;\n'
            + "".join(routes)
            + '  *) echo "unexpected gh call: $*" >&2; exit 2 ;;\n'
            "esac\n",
            encoding="utf-8",
        )
        gh.chmod(0o755)
        python = tools / "python3"
        python.write_text(f'#!/usr/bin/env bash\nexec {shlex.quote(sys.executable)} "$@"\n', encoding="utf-8")
        python.chmod(0o755)
        runner_temp = Path(scratch, "runner-temp")
        runner_temp.mkdir()
        result = subprocess.run(
            ["bash", "-c", script],
            cwd=checkout,
            env={
                **_runner_env(TAG_PUSH),
                **git_env,
                "PATH": f"{tools}{os.pathsep}{os.environ.get('PATH', '')}",
                "RUNNER_TEMP": str(runner_temp),
                **env,
            },
            capture_output=True,
            text=True,
            check=False,
        )
        return result.returncode, result.stdout + result.stderr


def _check_evidence_refs(step: Step) -> list[str]:
    """The readiness evidence the semver gate accepts names a trusted push.

    Its attestation's ref and its run's branch must each be main or the
    release branch of the tag's version (a maintenance release); evidence
    from another branch, a pull request or a dispatch never qualifies. Every
    such push names its evidence after the tree, so newer evidence of the
    same tree from another branch must not hide the trusted push's.
    """
    violations: list[str] = []
    for label, ref in TRUSTED_PUSHES:
        status, log = _run_evidence_step(step, {"ref": ref}, {"head_branch": github_ref_name(ref)})
        if status != 0:
            violations.append(
                f"step `{step.name}` refuses the readiness evidence of {label}: "
                + " | ".join(line for line in log.splitlines() if line.strip())
            )
        status, log = _run_evidence_step(
            step,
            {"ref": ref},
            {"head_branch": github_ref_name(ref)},
            newer=({"ref": "refs/heads/release/9.9.9"}, {"head_branch": "release/9.9.9"}),
        )
        if status != 0:
            violations.append(
                f"step `{step.name}` refuses the readiness evidence of {label} once a push of the same "
                "tree to release/9.9.9 (another version's release branch) uploaded newer evidence: "
                + " | ".join(line for line in log.splitlines() if line.strip())
            )
    refused = [
        *(
            (label, {"ref": ref}, {"head_branch": github_ref_name(ref)})
            for label, ref in UNTRUSTED_PUSHES
        ),
        (
            "a main-push attestation whose run ran on release/9.9.9",
            {"ref": "refs/heads/main"},
            {"head_branch": "release/9.9.9"},
        ),
        (
            "a release/9.9.9 attestation whose run ran on main",
            {"ref": "refs/heads/release/9.9.9"},
            {"head_branch": "main"},
        ),
        (
            "a pull request into a release branch",
            {"event": "pull_request", "ref": "refs/pull/1/merge"},
            {"event": "pull_request", "head_branch": f"release/{BINDING_VERSION}"},
        ),
        (
            f"a dispatch on release/{BINDING_VERSION}",
            {"event": "workflow_dispatch", "ref": f"refs/heads/release/{BINDING_VERSION}"},
            {"event": "workflow_dispatch", "head_branch": f"release/{BINDING_VERSION}"},
        ),
    ]
    for label, evidence, run in refused:
        status, _ = _run_evidence_step(step, evidence, run)
        if status == 0:
            violations.append(f"step `{step.name}` accepts the readiness evidence of {label}; {TRUSTED_ONLY}")
    return violations


def check_tag_validation(text: str) -> list[str]:
    """A tag skips Cargo validation only on an attestation that covers it.

    `release_validate_cargo` repeats fmt-governance, the generated-contract
    ratchets (which main CI runs only when its plan selects them) and the SDK
    suites. require_ci_green verifies the exact-tree attestation and exposes
    whether it proves all of those ran (`ci_covers_release_validation`); only
    then, while the self-hosted graph is off, is the job skipped and does the
    gate pass on the attestation. Otherwise, a legacy attestation included,
    the tag validates with Cargo, and manual dispatches keep their lane. The
    attestation itself must come from a push to main or to the release
    branch of the tag's version (a maintenance release), never from another
    branch, a pull request or a dispatch.
    """
    violations: list[str] = []

    # 1. The attestation step computes the coverage from the attestation it
    #    verified, and require_ci_green exposes it.
    ci_block = job_block(text, CI_GREEN_JOB)
    outputs = parse_mapping(ci_block, 4).get("outputs", "")
    wiring = re.search(
        rf"\b{COVERS_OUTPUT}:\s*\$\{{\{{\s*steps\.([A-Za-z0-9_-]+)\.outputs\.([A-Za-z0-9_-]+)\s*\}}\}}",
        outputs,
    )
    if wiring is None:
        return [f"job `{CI_GREEN_JOB}` does not expose `{COVERS_OUTPUT}` from a step output"]
    step_id, output = wiring.groups()
    producers = [step for step in job_steps(ci_block) if step.fields.get("id") == step_id]
    if len(producers) != 1:
        return [f"job `{CI_GREEN_JOB}` has {len(producers)} steps with `id: {step_id}`"]
    producer = producers[0]
    if not step_runs(producer, TAG_PUSH):
        violations.append(f"step `{producer.name}` does not run on {TAG_PUSH.label}")
    cases = (
        ("every lane ran", _attestation(), "true"),
        ("the generated-contract ratchets did not run", _attestation(plan__generated_contract=False), "false"),
        ("the plan does not record the ratchets", _attestation(plan__generated_contract=None), "false"),
        ("the SDK suites were skipped", _attestation(component_results__sdk_host="skipped"), "false"),
        (
            "a release-projection plan reused a verified parent run for the SDK lanes",
            _attestation(
                plan__mode="release-projection",
                component_results__sdk_host="skipped",
                reused_test_evidence=_reused_evidence(),
            ),
            "true",
        ),
        (
            "a release-projection plan names no reused run",
            _attestation(plan__mode="release-projection", component_results__sdk_host="skipped"),
            "false",
        ),
        (
            "a release-projection plan whose evidence comes from another classifier",
            _attestation(
                plan__mode="release-projection",
                component_results__sdk_host="skipped",
                reused_test_evidence=_reused_evidence(classifier="something-else"),
            ),
            "false",
        ),
        (
            "a release-projection plan whose evidence names no run attempt",
            _attestation(
                plan__mode="release-projection",
                component_results__sdk_host="skipped",
                reused_test_evidence=_reused_evidence(workflow_run_attempt=""),
            ),
            "false",
        ),
    )
    for label, attestation, expected in cases:
        status, written = _coverage_of(producer, attestation)
        if status != 0:
            violations.append(f"step `{producer.name}` rejects a valid attestation where {label}")
            continue
        if written.get(output) != expected:
            violations.append(
                f"step `{producer.name}` writes {output}={written.get(output, '<nothing>')} where {label}; "
                f"expected {expected}"
            )
    status, written = _coverage_of(producer, _attestation(component_results__ratchets="failure"))
    if status == 0 and written.get(output) == "true":
        violations.append(f"step `{producer.name}` reports coverage for an attestation it should refuse")

    # 1b. The attestation itself comes from a push to main or to the release
    #     branch of the tag's version, and from nothing else.
    violations.extend(_check_attestation_refs(ci_block))

    # 2. The Cargo validation job is skipped exactly where the attestation
    #    stands in for it.
    cargo_if = parse_mapping(job_block(text, CARGO_VALIDATION_JOB), 4).get("if")
    graph_on = {GCP_GRAPH_VARIABLE: "true"}
    manual = EventContext(
        label="a package dispatch on the GitHub-hosted backend",
        event_name="workflow_dispatch",
        inputs={
            "release_tag": "v0.0.0",
            "publish_release_packages": "true",
            "release_backend": "github-hosted",
        },
        job_outputs={f"{CI_GREEN_JOB}.{ATTESTATION_SCHEMA_OUTPUT}": "", f"{CI_GREEN_JOB}.{COVERS_OUTPUT}": ""},
    )
    routes = (
        (_attested_tag("true"), False),
        (_attested_tag("false", label="a tag push whose schema-4 attestation does not cover the Cargo lanes"), True),
        (_attested_tag("false", "1", label="a tag push with a verified legacy (schema 1) attestation"), True),
        (_attested_tag("true", label="a tag push while the self-hosted graph is enabled", variables=graph_on), False),
        (manual, True),
    )
    for context, must_run in routes:
        runs = cargo_if is None or truthy(evaluate(cargo_if, context))
        if runs and not must_run:
            violations.append(
                f"job `{CARGO_VALIDATION_JOB}` runs on {context.label}; the exact-tree CI "
                "attestation already covers it"
            )
        if must_run and not runs:
            violations.append(f"job `{CARGO_VALIDATION_JOB}` is skipped on {context.label}")

    # 3. The gate accepts the attestation route and nothing weaker.
    gate_steps = [step for step in job_steps(job_block(text, VALIDATION_GATE_JOB)) if step.run]
    if len(gate_steps) != 1:
        violations.append(
            f"job `{VALIDATION_GATE_JOB}` has {len(gate_steps)} run steps; expected one gate script"
        )
        return violations
    gate = gate_steps[0]
    skipped = {
        CARGO_VALIDATION_JOB: "skipped",
        "release_validate_buildbuddy": "skipped",
        "release_validate_buildbuddy_full": "skipped",
    }
    ran = {**skipped, CARGO_VALIDATION_JOB: "success"}
    gate_cases = (
        (_attested_tag("true", job_results=skipped), True),
        (_attested_tag("true", label="a covering attestation with Cargo validation run", job_results=ran), False),
        (_attested_tag("false", label="a non-covering attestation with Cargo validation run", job_results=ran), True),
        (
            _attested_tag(
                "false",
                label="a tag push whose attestation does not cover the Cargo lanes and Cargo validation skipped",
                job_results=skipped,
            ),
            False,
        ),
        (
            _attested_tag(
                "false", "1",
                label="a tag push with a verified legacy (schema 1) attestation and Cargo validation skipped",
                job_results=skipped,
            ),
            False,
        ),
        (
            _attested_tag(
                "true",
                label="a tag push whose exact-main CI did not succeed",
                job_results={**skipped, CI_GREEN_JOB: "failure"},
            ),
            False,
        ),
    )
    for context, must_accept in gate_cases:
        status, output_text = _run_gate_script(gate, context)
        if must_accept and status != 0:
            violations.append(
                f"step `{gate.name}` refuses {context.label}: "
                + " | ".join(line for line in output_text.splitlines() if line.strip())
            )
        if not must_accept and status == 0:
            violations.append(f"step `{gate.name}` accepts {context.label}")
    return violations


PREBUILD_INPUT = "prebuild_release_artifacts"
PREBUILD_LOOKUP_JOB = "release_prebuild"
PREBUILD_GATE_JOB = "release_prebuild_gate"
PREBUILD_LOOKUP_SCRIPT = "scripts/release-find-prebuild"
PREBUILD_BACKEND_REFUSAL = "A release prebuild builds with the backend its tag run selects"
PROVENANCE_VERIFIER = "scripts/verify-release-asset-provenance"
BINARY_BUILD_JOBS = (
    "build_binaries",
    "build_binaries_buildbuddy",
    "build_binaries_windows_cross",
    "build_binaries_macos_x86_cross",
)
CRITICAL_PUBLISH_JOB = "publish_critical_release_assets"
WEB_SDK_BUILD_JOB = "build_web_sdk_package"
WEB_SDK_PUBLISH_JOB = "publish_web_sdk"
ASSET_PUBLISH_JOB = "publish_github_release"
# A job whose block contains one of these writes to a public surface.
PUBLISHING_MARKERS = (
    "softprops/action-gh-release",
    "gh release upload",
    "update-homebrew",
    "release-publish-rust-crates.sh",
    "twine upload",
    "npm publish",
    "release-publish-web-sdk-package.sh",
)
PREBUILD_RUN = "424242"
PREBUILD_ATTEMPT = "2"


def _job_names(text: str) -> list[str]:
    lines = text.splitlines()
    try:
        start = lines.index("jobs:")
    except ValueError as error:
        raise ContractError("the workflow has no top-level `jobs:`") from error
    return [
        match.group(1)
        for line in lines[start + 1 :]
        if (match := JOB_HEADER.match(line))
    ]


def _job_runs(text: str, job: str, context: EventContext) -> bool:
    condition = parse_mapping(job_block(text, job), 4).get("if")
    return condition is None or truthy(evaluate(condition, context))


def _running_steps(text: str, job: str, context: EventContext) -> list[tuple[int, Step]]:
    return [
        (index, step)
        for index, step in enumerate(job_steps(job_block(text, job)))
        if step_runs(step, context)
    ]


def _checkout_ref(text: str, job: str, context: EventContext) -> str | None:
    for step in job_steps(job_block(text, job)):
        if CHECKOUT_ACTION in step.fields.get("uses", "") and "ref" in step.nested("with"):
            return render_strict(step.nested("with")["ref"], context)
    return None


def _verifier_invocations(step: Step, context: EventContext) -> list[list[str]]:
    """Run the step with a stub verifier and return the argv of each call."""
    env = {key: render_strict(value, context) for key, value in step.env.items()}
    script = render_strict(step.run, context)
    with tempfile.TemporaryDirectory(prefix="release-prebuild-") as scratch:
        record = Path(scratch, "calls")
        stub = Path(scratch, PROVENANCE_VERIFIER)
        stub.parent.mkdir(parents=True)
        stub.write_text(
            "#!/usr/bin/env bash\n"
            f"printf '%s\\x1f' \"$@\" >> '{record}'\n"
            f"printf '\\n' >> '{record}'\n",
            encoding="utf-8",
        )
        stub.chmod(0o755)
        subprocess.run(
            ["bash", "-c", script],
            cwd=scratch,
            env={**_runner_env(context), **env},
            capture_output=True,
            text=True,
            check=False,
        )
        if not record.exists():
            return []
        return [
            [arg for arg in line.split("\x1f") if arg]
            for line in record.read_text(encoding="utf-8").splitlines()
        ]


def check_prebuild(text: str) -> list[str]:
    """A pre-tag prebuild publishes nothing; a tag reuses it only verified.

    A `prebuild_release_artifacts` dispatch on main builds every release
    binary and the Web SDK package of the exact dispatched commit, runs no
    release validation or semver measurement, and publishes nothing. A tag
    push looks the prebuild of its own commit up (`release_prebuild`); when
    one is found it builds nothing, publishes that run's archives only after
    `verify-release-asset-provenance --source-run` (the assets_run_id trust
    path) and the Web SDK package only after its attestation is verified;
    when none is found it builds as before.
    """
    violations: list[str] = []
    names = _job_names(text)
    for job in (PREBUILD_LOOKUP_JOB, PREBUILD_GATE_JOB, *BINARY_BUILD_JOBS, WEB_SDK_BUILD_JOB):
        if job not in names:
            return [f"job `{job}` is not defined in the workflow"]

    prebuild_outputs = {
        f"{PREBUILD_LOOKUP_JOB}.run_id": "",
        f"{PREBUILD_LOOKUP_JOB}.run_attempt": "",
        f"{CI_GREEN_JOB}.{ATTESTATION_SCHEMA_OUTPUT}": "",
        f"{CI_GREEN_JOB}.{COVERS_OUTPUT}": "",
        f"{CI_GREEN_JOB}.prebuild_tag": "v0.0.0",
    }
    prebuild = EventContext(
        label="a prebuild dispatch on main",
        event_name="workflow_dispatch",
        ref="refs/heads/main",
        sha="1" * 40,
        inputs={PREBUILD_INPUT: "true", "release_backend": "github-hosted"},
        job_outputs=prebuild_outputs,
    )

    # 1. A prebuild publishes nothing, whatever its upstream jobs report.
    for job in names:
        block = "\n".join(job_block(text, job))
        if any(marker in block for marker in PUBLISHING_MARKERS) and _job_runs(text, job, prebuild):
            violations.append(f"job `{job}` publishes on {prebuild.label}")

    # 2. It runs no release validation or semver measurement, does not wait
    #    for CI, and verifies its request.
    for job in ("release_validate_cargo", "release_validate_buildbuddy", "release_validate_gate", SEMVER_GATE_JOB):
        if job in names and _job_runs(text, job, prebuild):
            violations.append(f"job `{job}` runs on {prebuild.label}, which validates nothing")
    ci_steps = job_steps(job_block(text, CI_GREEN_JOB))
    for step in ci_steps:
        if step.fields.get("id") == "ci_run" and step_runs(step, prebuild):
            violations.append(f"step `{step.name}` waits for exact-main CI on {prebuild.label}")
    requests = [step for step in ci_steps if step.fields.get("id") == "prebuild_request"]
    if len(requests) != 1 or not step_runs(requests[0], prebuild) or "tag" not in requests[0].written_outputs():
        violations.append(
            f"job `{CI_GREEN_JOB}` must verify the request with one step `id: prebuild_request` "
            f"that runs on {prebuild.label} and writes its `tag`"
        )
    elif "refs/heads/main" not in requests[0].run:
        violations.append(f"step `{requests[0].name}` does not refuse a prebuild off refs/heads/main")

    # 3. It builds every artifact from exactly the dispatched commit.
    realistic = EventContext(
        **{
            **prebuild.__dict__,
            "job_results": {
                "release_validate_gate": "skipped",
                SEMVER_GATE_JOB: "skipped",
                PREBUILD_LOOKUP_JOB: "skipped",
            },
        }
    )
    for job in (
        "build_binaries",
        "build_binaries_windows_cross",
        "build_binaries_macos_x86_cross",
        WEB_SDK_BUILD_JOB,
        "build_binaries_critical_gate",
        "build_binaries_gate",
        PREBUILD_GATE_JOB,
    ):
        if not _job_runs(text, job, realistic):
            violations.append(f"job `{job}` is skipped on {prebuild.label}")
    for job in (*BINARY_BUILD_JOBS, WEB_SDK_BUILD_JOB):
        ref = _checkout_ref(text, job, prebuild)
        if ref != prebuild.sha:
            violations.append(
                f"job `{job}` checks out `{ref}` on {prebuild.label}, not the dispatched commit "
                "(github.sha): a moving main would be built instead"
            )

    # 4. The prebuild gate passes only on the critical binaries and the Web
    #    SDK package, and the lookup requires exactly that job.
    gate_steps = [step for step in job_steps(job_block(text, PREBUILD_GATE_JOB)) if step.run]
    gate_name = parse_mapping(job_block(text, PREBUILD_GATE_JOB), 4).get("name", "")
    if len(gate_steps) != 1:
        violations.append(f"job `{PREBUILD_GATE_JOB}` must have exactly one run step")
    else:
        for failed in ("build_binaries_critical_gate", WEB_SDK_BUILD_JOB):
            context = EventContext(**{**realistic.__dict__, "job_results": {failed: "failure"}})
            status, _ = _run_gate_script(gate_steps[0], context)
            if status == 0:
                violations.append(f"job `{PREBUILD_GATE_JOB}` passes although `{failed}` failed")
        status, output = _run_gate_script(gate_steps[0], realistic)
        if status != 0:
            violations.append(f"job `{PREBUILD_GATE_JOB}` refuses a complete prebuild: {output.strip()}")
    lookup_script = Path(__file__).resolve().with_name(Path(PREBUILD_LOOKUP_SCRIPT).name)
    try:
        lookup_text = lookup_script.read_text(encoding="utf-8")
    except OSError:
        lookup_text = ""
    if f'PREBUILD_GATE_JOB="{gate_name}"' not in lookup_text:
        violations.append(
            f"{PREBUILD_LOOKUP_SCRIPT} does not require the `{gate_name}` job that marks a complete prebuild"
        )

    # 5. A tag looks its own commit's prebuild up; dispatches never do.
    if not _job_runs(text, PREBUILD_LOOKUP_JOB, TAG_PUSH):
        violations.append(f"job `{PREBUILD_LOOKUP_JOB}` does not run on {TAG_PUSH.label}")
    for context in (PACKAGE_RECOVERY, prebuild):
        if _job_runs(text, PREBUILD_LOOKUP_JOB, context):
            violations.append(f"job `{PREBUILD_LOOKUP_JOB}` runs on {context.label}")
    if not any(PREBUILD_LOOKUP_SCRIPT in step.run for step in job_steps(job_block(text, PREBUILD_LOOKUP_JOB))):
        violations.append(f"job `{PREBUILD_LOOKUP_JOB}` does not run `{PREBUILD_LOOKUP_SCRIPT}`")

    # 6. A tag whose commit was prebuilt builds nothing and publishes the
    #    prebuild's artifacts only through the attested-archive verification.
    def tag_push(run_id: str) -> EventContext:
        return EventContext(
            label=f"a tag push {'with a prebuild found' if run_id else 'without a prebuild'}",
            event_name="push",
            step_outputs={"manifest.upload": "true"},
            job_outputs={
                f"{PREBUILD_LOOKUP_JOB}.run_id": run_id,
                f"{PREBUILD_LOOKUP_JOB}.run_attempt": PREBUILD_ATTEMPT if run_id else "",
                f"{CI_GREEN_JOB}.{ATTESTATION_SCHEMA_OUTPUT}": "4",
                f"{CI_GREEN_JOB}.{COVERS_OUTPUT}": "true",
                f"{CI_GREEN_JOB}.prebuild_tag": "",
            },
        )

    reused, fresh = tag_push(PREBUILD_RUN), tag_push("")

    # Reusing a prebuild never bypasses a gate: with exact-main CI, release
    # validation or the semver gate failed, nothing publishes.
    for failed in (CI_GREEN_JOB, VALIDATION_GATE_JOB, SEMVER_GATE_JOB):
        context = EventContext(
            **{**reused.__dict__, "label": f"{reused.label} and `{failed}` failed", "job_results": {failed: "failure"}}
        )
        for job in (CRITICAL_PUBLISH_JOB, ASSET_PUBLISH_JOB, WEB_SDK_PUBLISH_JOB, REGISTRY_JOB):
            if job in names and _job_runs(text, job, context):
                violations.append(f"job `{job}` publishes on {context.label}")
    for job in (*BINARY_BUILD_JOBS, WEB_SDK_BUILD_JOB):
        if _job_runs(text, job, reused):
            violations.append(f"job `{job}` builds on {reused.label}")
    for job in ("build_binaries_windows_cross", "build_binaries_macos_x86_cross", WEB_SDK_BUILD_JOB):
        if not _job_runs(text, job, fresh):
            violations.append(f"job `{job}` is skipped on {fresh.label}")

    for job in (CRITICAL_PUBLISH_JOB, ASSET_PUBLISH_JOB, WEB_SDK_PUBLISH_JOB):
        if not _job_runs(text, job, reused):
            violations.append(f"job `{job}` does not run on {reused.label}")
            continue
        steps = _running_steps(text, job, reused)
        downloads = [
            step for _, step in steps if "actions/download-artifact" in step.fields.get("uses", "")
        ]
        if not downloads:
            violations.append(f"job `{job}` downloads nothing on {reused.label}")
        for step in downloads:
            run_id = render_strict(step.nested("with").get("run-id", ""), reused)
            if run_id != PREBUILD_RUN:
                violations.append(
                    f"step `{step.name}` downloads from run `{run_id or 'this run'}` on "
                    f"{reused.label}, not the prebuild run"
                )

    for job in (CRITICAL_PUBLISH_JOB, ASSET_PUBLISH_JOB):
        verify_steps = [
            step for _, step in _running_steps(text, job, reused) if PROVENANCE_VERIFIER in step.run
        ]
        if len(verify_steps) != 1:
            violations.append(
                f"job `{job}` must run `{PROVENANCE_VERIFIER}` in exactly one step on {reused.label}"
            )
            continue
        calls = _verifier_invocations(verify_steps[0], reused)
        if (
            len(calls) != 1
            or calls[0][:2] != ["--source-run", PREBUILD_RUN]
            or "--stamps" not in calls[0]
            or "--prebuild" not in calls[0]
            or "--source-attempt" not in calls[0]
            or calls[0][calls[0].index("--source-attempt") + 1] != PREBUILD_ATTEMPT
        ):
            violations.append(
                f"step `{verify_steps[0].name}` verifies {reused.label} with {calls}; expected "
                f"`{PROVENANCE_VERIFIER} --source-run {PREBUILD_RUN} --source-attempt {PREBUILD_ATTEMPT} "
                "--prebuild --stamps ...`, the assets_run_id trust path for the tag's own prebuild, "
                "pinned to the attempt the lookup examined"
            )
        else:
            # The critical archives are verified against the prebuild's
            # critical gate (its trailing lanes may still run); the complete
            # set against its full binary build gate, the default.
            args = calls[0]
            gate = args[args.index("--source-gate") + 1] if "--source-gate" in args else "Release binary build gate"
            wanted = (
                "Release critical binary build gate" if job == CRITICAL_PUBLISH_JOB else "Release binary build gate"
            )
            if gate != wanted:
                violations.append(
                    f"step `{verify_steps[0].name}` requires the prebuild's `{gate}` on {reused.label}; "
                    f"expected `{wanted}`"
                )
        fresh_calls = _verifier_invocations(verify_steps[0], fresh)
        if len(fresh_calls) != 1 or "--source-run" in fresh_calls[0]:
            violations.append(
                f"step `{verify_steps[0].name}` verifies {fresh.label} with {fresh_calls}; "
                "expected the exact-tag verification"
            )

    # The trailing publisher downloads the prebuild's archives only after
    # that run's full binary build gate passed (the trailing lanes may still
    # be running when the prebuild is selected).
    publisher_steps = _running_steps(text, ASSET_PUBLISH_JOB, reused)
    wait_index = next(
        (
            index
            for index, step in publisher_steps
            if "Release binary build gate" in step.run
            and "PREBUILD_RUN_ID" in step.run
            and "PREBUILD_RUN_ATTEMPT" in step.run
        ),
        None,
    )
    download_index = next(
        (
            index
            for index, step in publisher_steps
            if "actions/download-artifact" in step.fields.get("uses", "")
        ),
        None,
    )
    if wait_index is None or download_index is None or wait_index > download_index:
        violations.append(
            f"job `{ASSET_PUBLISH_JOB}` must wait for the prebuild run's `Release binary build gate` "
            f"before downloading its archives on {reused.label}"
        )

    web_steps = _running_steps(text, WEB_SDK_PUBLISH_JOB, reused)
    verify_index = next(
        (
            index
            for index, step in web_steps
            if "gh attestation verify" in step.run and "--source-ref refs/heads/main" in step.run
        ),
        None,
    )
    publish_index = next(
        (index for index, step in web_steps if "release-publish-web-sdk-package.sh" in step.run),
        None,
    )
    if verify_index is None or publish_index is None or verify_index > publish_index:
        violations.append(
            f"job `{WEB_SDK_PUBLISH_JOB}` must verify the prebuilt package's attestation "
            f"(`gh attestation verify ... --source-ref refs/heads/main`) before publishing on {reused.label}"
        )

    # 7. A prebuild builds with the backend its tag run selects: on the other
    #    one its critical lanes queue behind other runners and miss the tag's
    #    bounded wait. The request refuses the other backend, for the owner
    #    and for others, with MEERKAT_RELEASE_BUILDBUDDY true, 1, false and
    #    unset; the tag's choice is read from whether its GitHub-hosted
    #    critical lane runs.
    if len(requests) == 1:
        request = requests[0]
        for actor in ("lukacf", "release-bot"):
            for flag in ("true", "1", "false", ""):
                variables = {"MEERKAT_RELEASE_BUILDBUDDY": flag} if flag else {}
                tag_run = EventContext(**{**fresh.__dict__, "actor": actor, "variables": variables})
                selected = "github-hosted" if _job_runs(text, "build_binaries", tag_run) else "buildbuddy"
                for backend in ("github-hosted", "buildbuddy"):
                    context = EventContext(
                        **{
                            **prebuild.__dict__,
                            "label": (
                                f"a prebuild by {actor} with release_backend={backend} "
                                f"(MEERKAT_RELEASE_BUILDBUDDY {flag or 'unset'})"
                            ),
                            "actor": actor,
                            "variables": variables,
                            "inputs": {**prebuild.inputs, "release_backend": backend},
                        }
                    )
                    status, output = _run_gate_script(request, context)
                    refused = status != 0 and PREBUILD_BACKEND_REFUSAL in output
                    if refused != (backend != selected):
                        violations.append(
                            f"step `{request.name}` {'refuses' if refused else 'accepts'} {context.label}, "
                            f"whose tag run builds on {selected}"
                        )
    return violations

SEMVER_BASELINE_JOB = "publish_semver_baseline"
SEMVER_BASELINE_ATTACH_JOB = "attach_semver_baseline"


def _job_needs(text: str, job: str) -> list[str]:
    raw = parse_mapping(job_block(text, job), 4).get("needs", "").strip()
    if raw.startswith("[") and raw.endswith("]"):
        raw = raw[1:-1]
    return [item for item in re.split(r"[\s,]+", raw) if item and item != "-"]


def check_semver_baseline(text: str) -> list[str]:
    """The semver baseline rustdoc is off every path a user waits on.

    Its rustdoc build starts with the release (it waits for nothing that
    publishes), no job waits for it except the attach job, nothing waits for
    the attach job, and only the attach job writes to the GitHub release,
    after the release is published, from the artifact the build uploaded.
    """
    violations: list[str] = []
    names = _job_names(text)
    for job in (SEMVER_BASELINE_JOB, SEMVER_BASELINE_ATTACH_JOB):
        if job not in names:
            return [f"job `{job}` is not defined in the workflow"]
    needs = {job: _job_needs(text, job) for job in names}

    for job, upstream in needs.items():
        if job != SEMVER_BASELINE_ATTACH_JOB and SEMVER_BASELINE_JOB in upstream:
            violations.append(f"job `{job}` waits for the semver baseline rustdoc (`{SEMVER_BASELINE_JOB}`)")
        if SEMVER_BASELINE_ATTACH_JOB in upstream:
            violations.append(f"job `{job}` waits for the semver baseline attachment (`{SEMVER_BASELINE_ATTACH_JOB}`)")

    closure: set[str] = set()
    frontier = list(needs[SEMVER_BASELINE_JOB])
    while frontier:
        job = frontier.pop()
        if job in closure or job not in needs:
            continue
        closure.add(job)
        frontier.extend(needs[job])
    blocking = sorted(
        job
        for job in closure
        if any(marker in "\n".join(job_block(text, job)) for marker in PUBLISHING_MARKERS)
        or job.startswith("build_")
    )
    if blocking:
        violations.append(
            f"job `{SEMVER_BASELINE_JOB}` waits for {', '.join(f'`{job}`' for job in blocking)}; "
            "the rustdoc build must run in parallel with publication, not after it"
        )

    if "publish_github_release" not in needs[SEMVER_BASELINE_ATTACH_JOB]:
        violations.append(
            f"job `{SEMVER_BASELINE_ATTACH_JOB}` does not wait for `publish_github_release`, "
            "so it could attach to a release that is not published"
        )
    build_steps = job_steps(job_block(text, SEMVER_BASELINE_JOB))
    attach_steps = job_steps(job_block(text, SEMVER_BASELINE_ATTACH_JOB))
    if any("gh release upload" in step.run for step in build_steps):
        violations.append(f"job `{SEMVER_BASELINE_JOB}` writes to the GitHub release itself")
    if not any("gh release upload" in step.run for step in attach_steps):
        violations.append(f"job `{SEMVER_BASELINE_ATTACH_JOB}` does not attach the baseline")
    if any(GH_UPLOAD_CLOBBER.search(step.run) for step in attach_steps):
        violations.append(
            f"job `{SEMVER_BASELINE_ATTACH_JOB}` attaches with `--clobber`, replacing a baseline "
            "already on the release"
        )
    uploaded = {
        step.nested("with").get("name")
        for step in build_steps
        if "actions/upload-artifact" in step.fields.get("uses", "")
    }
    downloaded = {
        step.nested("with").get("name")
        for step in attach_steps
        if "actions/download-artifact" in step.fields.get("uses", "")
    }
    if not uploaded or not downloaded or not downloaded <= uploaded:
        violations.append(
            f"job `{SEMVER_BASELINE_ATTACH_JOB}` must attach the artifact `{SEMVER_BASELINE_JOB}` "
            f"uploads (uploads {sorted(filter(None, uploaded))}, downloads {sorted(filter(None, downloaded))})"
        )
    for job in (SEMVER_BASELINE_JOB, SEMVER_BASELINE_ATTACH_JOB):
        if not _job_runs(text, job, TAG_PUSH):
            violations.append(f"job `{job}` does not run on {TAG_PUSH.label}")
    return violations

# A `gh release upload` that replaces an asset of the same name.
GH_UPLOAD_CLOBBER = re.compile(r"gh release upload(?:[^\n]|\\\n)*--clobber")
CRITICAL_TARGETS = ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "aarch64-apple-darwin")
TRAILING_TARGETS = ("x86_64-apple-darwin", "x86_64-pc-windows-msvc")
TRAILING_BUILD_JOBS = ("build_binaries_windows_cross", "verify_windows_binaries", "build_binaries_macos_x86_cross")
RELEASE_BINARIES = ("rkat", "rkat-rpc", "rkat-rest", "rkat-mcp")
GH_RELEASE_ACTION = "softprops/action-gh-release"


def _published_archives(text: str, job: str, context: EventContext, version: str) -> set[str]:
    """Which archives the job's release upload would publish under `context`.

    Every release archive name (critical and trailing targets) is laid out
    where the job's artifact downloads put them; the job's shell steps run in
    order up to its provenance verification (selection precedes
    verification), and the release upload's `files` patterns are then
    resolved against the result.
    """
    steps = [step for step in job_steps(job_block(text, job)) if step_runs(step, context)]
    with tempfile.TemporaryDirectory(prefix="release-trailing-") as scratch:
        for step in steps:
            if "actions/download-artifact" not in step.fields.get("uses", ""):
                continue
            root = Path(scratch, step.nested("with").get("path", "."))
            for target in (*CRITICAL_TARGETS, *TRAILING_TARGETS):
                extension = "zip" if "windows" in target else "tar.gz"
                for binary in RELEASE_BINARIES:
                    lane = root / f"meerkat-{target}-{binary}"
                    lane.mkdir(parents=True, exist_ok=True)
                    Path(lane, f"{binary}-{version}-{target}.{extension}").write_bytes(b"archive")
        for step in steps:
            if PROVENANCE_VERIFIER in step.run:
                break
            if not step.raw_run.strip():
                continue
            env = {key: render_strict(value, context) for key, value in step.env.items()}
            subprocess.run(
                ["bash", "-c", render_strict(step.raw_run, context)],
                cwd=scratch,
                env={**_runner_env(context), **env},
                capture_output=True,
                text=True,
                check=False,
            )
        published: set[str] = set()
        for step in steps:
            if GH_RELEASE_ACTION not in step.fields.get("uses", ""):
                continue
            for pattern in step.nested("with").get("files", "").split():
                published.update(path.name for path in Path(scratch).glob(pattern))
        return published


def _steps_of(text: str, job: str) -> list[Step]:
    """The job's steps; none for a job that calls a reusable workflow."""
    try:
        return job_steps(job_block(text, job))
    except ContractError:
        return []


def check_trailing_platforms(text: str) -> list[str]:
    """Intel macOS and Windows trail; nothing replaces a published asset.

    The critical publisher puts exactly the Linux (x86_64, aarch64) and macOS
    arm64 archives on the release as soon as their lane passes, waiting for no
    trailing (Intel macOS, Windows) job. Every archive upload is add-only, the
    complete manifest is written only by publish_github_release after every
    lane (and refuses to change a published one), and Homebrew is updated only
    after that trailing publisher.
    """
    violations: list[str] = []
    names = _job_names(text)
    for job in (CRITICAL_PUBLISH_JOB, ASSET_PUBLISH_JOB, "build_binaries_critical_gate", "update_homebrew"):
        if job not in names:
            return [f"job `{job}` is not defined in the workflow"]
    needs = {job: _job_needs(text, job) for job in names}

    def closure(job: str) -> set[str]:
        seen: set[str] = set()
        frontier = list(needs[job])
        while frontier:
            upstream = frontier.pop()
            if upstream in seen or upstream not in needs:
                continue
            seen.add(upstream)
            frontier.extend(needs[upstream])
        return seen

    for job in (CRITICAL_PUBLISH_JOB, PREBUILD_GATE_JOB):
        if job not in needs:
            continue
        waits = sorted(set(TRAILING_BUILD_JOBS + ("build_binaries_gate",)) & closure(job))
        if waits:
            violations.append(
                f"job `{job}` waits for {', '.join(f'`{upstream}`' for upstream in waits)}; "
                "the trailing Intel macOS and Windows lanes must not gate the critical assets"
            )
    if not _job_runs(text, CRITICAL_PUBLISH_JOB, TAG_PUSH):
        violations.append(f"job `{CRITICAL_PUBLISH_JOB}` does not run on {TAG_PUSH.label}")

    tag = EventContext(
        label=TAG_PUSH.label,
        event_name="push",
        job_outputs={f"{PREBUILD_LOOKUP_JOB}.run_id": ""},
    )
    expected = {
        f"{binary}-0.0.0-{target}.tar.gz" for binary in RELEASE_BINARIES for target in CRITICAL_TARGETS
    }
    published = _published_archives(text, CRITICAL_PUBLISH_JOB, tag, "0.0.0")
    if published != expected:
        violations.append(
            f"job `{CRITICAL_PUBLISH_JOB}` publishes {sorted(published - expected) or 'no extra archive'} "
            f"beyond the critical set and misses {sorted(expected - published) or 'none of it'}; "
            "it must publish exactly the Linux and macOS arm64 archives"
        )

    for job in names:
        for step in _steps_of(text, job):
            if GH_UPLOAD_CLOBBER.search(step.raw_run):
                violations.append(
                    f"step `{step.name}` uploads with `gh release upload --clobber`; "
                    "release uploads are add-only and never replace an asset"
                )
            if GH_RELEASE_ACTION not in step.fields.get("uses", ""):
                continue
            files = step.nested("with").get("files", "")
            overwrite = step.nested("with").get("overwrite_files", "")
            manifest = "checksums.sha256" in files or "index.json" in files
            if manifest and job != ASSET_PUBLISH_JOB:
                violations.append(f"job `{job}` writes the release manifest; only `{ASSET_PUBLISH_JOB}` may")
            if overwrite != "false":
                violations.append(
                    f"step `{step.name}` uploads with overwrite_files `{overwrite or 'unset'}`; "
                    "release uploads are add-only (`overwrite_files: false`) and never replace an asset"
                )
            if manifest and "steps." not in (step.condition or ""):
                violations.append(
                    f"step `{step.name}` writes the manifest unconditionally; it must run only when "
                    "the manifest is absent from the release"
                )
    if "build_binaries_gate" not in closure(ASSET_PUBLISH_JOB):
        violations.append(f"job `{ASSET_PUBLISH_JOB}` writes the manifest before every lane finished")
    publisher = job_steps(job_block(text, ASSET_PUBLISH_JOB))
    guard = next((i for i, step in enumerate(publisher) if "cmp -s" in step.run and "gh release download" in step.run), None)
    upload = next(
        (
            i
            for i, step in enumerate(publisher)
            if GH_RELEASE_ACTION in step.fields.get("uses", "") and "checksums.sha256" in step.nested("with").get("files", "")
        ),
        None,
    )
    if guard is None or upload is None or guard > upload:
        violations.append(
            f"job `{ASSET_PUBLISH_JOB}` must refuse to change a published manifest before writing it"
        )

    for job in names:
        uses_homebrew = any("update-homebrew" in step.fields.get("uses", "") for step in _steps_of(text, job))
        if uses_homebrew and ASSET_PUBLISH_JOB not in closure(job):
            violations.append(f"job `{job}` updates Homebrew before the trailing publisher `{ASSET_PUBLISH_JOB}`")
    return violations

# Jobs allowed each elevated token scope (everything else is read-only).
WRITE_SCOPES_ALLOWED = {
    "contents": {"publish_critical_release_assets", "publish_github_release", "attach_semver_baseline"},
    "id-token": {*BINARY_BUILD_JOBS, WEB_SDK_BUILD_JOB, "release_validate_buildbuddy_full"},
    "attestations": {*BINARY_BUILD_JOBS, WEB_SDK_BUILD_JOB},
}


def _permission_scopes(lines: list[str], indent: int) -> dict[str, str]:
    """`scope: level` pairs of a permissions mapping at `indent`."""
    for index, line in enumerate(lines):
        if line == " " * indent + "permissions:":
            scopes: dict[str, str] = {}
            for candidate in lines[index + 1 :]:
                if candidate.strip() and _indent(candidate) <= indent:
                    break
                match = re.match(r"^\s+([a-z-]+):\s*([a-z]+)\s*$", candidate)
                if match:
                    scopes[match.group(1)] = match.group(2)
            return scopes
    return {}


def check_token_permissions(text: str) -> list[str]:
    """The workflow token is read-only except where a job needs more.

    The workflow default grants no write scope. Only the build jobs (and the
    full-graph caller, for buildbuddy.yml's OIDC) may mint OIDC tokens, only
    the build jobs may write attestations, and only the release publishers
    may write repository contents. No checkout in a job granted a write
    scope persists the token. No job of any workflow in the repository
    passes every secret to a reusable workflow (`secrets: inherit`): a
    caller lists the secrets the called workflow reads.
    """
    violations: list[str] = []
    lines = text.splitlines()
    top = _permission_scopes(lines[: lines.index("jobs:")], 0)
    for scope, level in sorted(top.items()):
        if level == "write":
            violations.append(f"the workflow default grants `{scope}: write` to every job")
    for job in _job_names(text):
        block = job_block(text, job)
        scopes = _permission_scopes(block, 4)
        for scope, level in sorted(scopes.items()):
            if level == "write" and job not in WRITE_SCOPES_ALLOWED.get(scope, set()):
                violations.append(f"job `{job}` is granted `{scope}: write`")
        if parse_mapping(block, 4).get("secrets", "").strip() == "inherit":
            violations.append(f"job `{job}` passes every repository secret to a reusable workflow (`secrets: inherit`)")
        if "write" not in scopes.values():
            continue
        for step in _steps_of(text, job):
            inputs = step.nested("with")
            if CHECKOUT_ACTION in step.fields.get("uses", "") and inputs.get("persist-credentials") != "false":
                violations.append(f"job `{job}` step `{step.name}` persists the workflow token in its checkout")
    # The other workflows hand a called workflow the same repository secrets.
    for path in sorted((REPO_ROOT / ".github" / "workflows").glob("*.y*ml")):
        if path.name == DEFAULT_WORKFLOW.name:
            continue
        source = path.read_text(encoding="utf-8")
        for job in _job_names(source):
            if parse_mapping(job_block(source, job), 4).get("secrets", "").strip() == "inherit":
                violations.append(
                    f"{path.name} job `{job}` passes every repository secret to a reusable workflow "
                    "(`secrets: inherit`)"
                )
    return violations

# Jobs that hold a registry or publish token (or write the GitHub release):
# they run no cache step at all. Any job granted `contents: write` counts too.
TOKEN_JOBS = (
    "registry_credentials_preflight",
    REGISTRY_JOB,
    WEB_SDK_PUBLISH_JOB,
    CRITICAL_PUBLISH_JOB,
    ASSET_PUBLISH_JOB,
    "update_homebrew",
    "attach_semver_baseline",
)
# setup-* actions that restore and save a package cache when given a `cache`
# input (setup-go by default, setup-node also when package.json names a
# package manager).
SETUP_CACHE_ACTIONS = (
    "actions/setup-node",
    "actions/setup-python",
    "actions/setup-java",
    "actions/setup-go",
    "actions/setup-dotnet",
)
# The triggers whose saves cannot reach a tag run, or come from reviewed
# code alone: a push (to main, or to a branch scope tags never read), a pull
# request or merge-queue run (their own scopes) and a schedule (main's code).
# Every other trigger is dispatch-like: it saves into the default branch's
# cache scope, which every tag run restores, on behalf of someone other than
# a reviewed push to main (a dispatch, repository_dispatch, workflow_call,
# whose caller's event is unknown here, pull_request_target, workflow_run,
# issue_comment, issues, ...).
TRUSTED_SAVE_TRIGGERS = ("push", "pull_request", "schedule", "merge_group")
# The tree the local actions and the other workflows are read from; the
# release doctor's fixtures point it at a mutated copy (--repo-root).
REPO_ROOT = Path(__file__).resolve().parent.parent
# The Bazel external repository cache setup-buildbuddy-ci restores and
# saves: a tag run restores none of it, and only trusted runs save it.
BAZEL_REPOSITORY_CACHE = ("key", "buildbuddy-bazel-repository-*")
# The workflows whose runs produce what a tag run trusts: ci.yml's exact-tree
# attestation (require_ci_green), release-semver-readiness.yml's semver
# attestation (release_semver_gate) and release.yml's prebuild artifacts. A
# dispatch-like run of one of them, or of any workflow that checks out a ref
# its inputs choose, saves no cache: it would save into the scope the next
# trusted run restores from.
TAG_TRUSTED_WORKFLOWS = ("ci.yml", "release-semver-readiness.yml", "release.yml")
# The release.yml jobs that may restore a cache: none. No release job
# restores one, on a tag or a dispatch (a prebuild, a recovery or the
# semver gate's measurement): nothing on main saves their keys, so a
# restore could only return an entry some other run wrote. The semver
# measurement builds through repo-cargo in a per-run target directory, so a
# target cache never reached it. release-semver-readiness.yml's measurement,
# which builds the same way, restores none either, on any of its triggers.
RELEASE_RESTORE_JOBS: tuple[str, ...] = ()
# The sccache setup every CI lane runs: the GCS objects it writes are what
# the push to main compiles from before it attests, so only push and
# schedule runs, and a dispatch on a release branch (rel/** or release/**),
# set the sccache client to write them (a client-side setting: a later step
# can still read the job's GCS credentials file, SCCACHE_GCS_KEY_PATH, whose
# identity can write). The branch prefix is no authorization control: any
# writer can create such a branch and dispatch it, as any writer can push to
# release/**, so the write boundary stays repository write access. It
# installs a release checked against a pinned sha256 itself, not through
# sccache-action, which would export the job's Actions cache token to the
# run steps after it.
SCCACHE_ACTION = "./.github/actions/setup-sccache-ci"
# The (event, ref) runs whose client writes, and those whose client only
# reads: a dispatch on any other ref (main, a near-miss branch name, a tag,
# a tag named like a release branch) and every other event, on a release
# branch's ref too.
SCCACHE_WRITERS = (
    ("push", "refs/heads/main"),
    ("push", "refs/heads/release/next"),
    ("schedule", "refs/heads/main"),
    ("workflow_dispatch", "refs/heads/rel/next"),
    ("workflow_dispatch", "refs/heads/release/next"),
)
SCCACHE_READERS = (
    ("pull_request", "refs/pull/1/merge"),
    ("pull_request", "refs/heads/rel/next"),
    ("pull_request_target", "refs/heads/main"),
    ("workflow_dispatch", "refs/heads/main"),
    ("workflow_dispatch", "refs/heads/feature/rel/next"),
    ("workflow_dispatch", "refs/heads/rel-next"),
    ("workflow_dispatch", "refs/heads/released"),
    ("workflow_dispatch", "refs/tags/v1.0.0"),
    ("workflow_dispatch", "refs/tags/rel/next"),
    ("workflow_dispatch", "refs/tags/release/1.0"),
    ("merge_group", "refs/heads/gh-readonly-queue/main/pr-1"),
    ("workflow_run", "refs/heads/rel/next"),
)


@dataclass(frozen=True)
class _Unknown:
    """A value a static reading cannot know (a dispatch input, a repository
    variable, a step output, the runner); `truthy` is set when only its
    truthiness is known."""

    truthy: bool | None = None


UNKNOWN = _Unknown()


def _known_truthy(value: object) -> bool | None:
    if isinstance(value, _Unknown):
        return value.truthy
    return truthy(value)


@dataclass(frozen=True)
class OpenFrame:
    """One run as the cache check reads it: its event (None for a reusable
    workflow, whose caller's event is unknown), its ref, the inputs of the
    local action or reusable workflow being read (None outside one), and the
    job's static env (_static_env). Everything else is unknown, so a
    condition counts as false only when it is false whatever the unknowns
    are."""

    label: str
    event_name: str | None
    ref: str = "refs/heads/main"
    inputs: dict[str, object] | None = None
    env: dict[str, str] | None = None

    def resolve(self, path: str) -> object:
        if path == "github.event_name":
            return UNKNOWN if self.event_name is None else self.event_name
        if path == "github.ref":
            return self.ref
        if path == "github.ref_name":
            return github_ref_name(self.ref)
        if path.startswith("inputs.") and self.inputs is not None:
            # An input the action does not declare reads as empty.
            return self.inputs.get(path[len("inputs.") :], "")
        if path.startswith("env.") and self.env is not None and path[len("env.") :] in self.env:
            return self.env[path[len("env.") :]]
        return UNKNOWN


class _OpenParser(_Parser):
    """`_Parser` over three values: what the frame cannot know stays unknown."""

    def parse_or(self) -> object:
        left = self.parse_and()
        while self.peek() == ("op", "||"):
            self.take()
            right = self.parse_and()
            known = _known_truthy(left)
            if known is None:
                left = _Unknown(True) if _known_truthy(right) is True else UNKNOWN
            elif not known:
                left = right
        return left

    def parse_and(self) -> object:
        left = self.parse_equality()
        while self.peek() == ("op", "&&"):
            self.take()
            right = self.parse_equality()
            known = _known_truthy(left)
            if known is None:
                left = _Unknown(False) if _known_truthy(right) is False else UNKNOWN
            elif known:
                left = right
        return left

    def parse_equality(self) -> object:
        left = self.parse_unary()
        while self.peek() in (("op", "=="), ("op", "!=")):
            _, op = self.take()
            right = self.parse_unary()
            if isinstance(left, _Unknown) or isinstance(right, _Unknown):
                left = UNKNOWN
            else:
                equal = _equal(left, right)
                left = equal if op == "==" else not equal
        return left

    def parse_unary(self) -> object:
        if self.peek() == ("op", "!"):
            self.take()
            known = _known_truthy(self.parse_unary())
            return UNKNOWN if known is None else not known
        return self.parse_primary()

    def parse_primary(self) -> object:
        token = self.peek()
        following = self.tokens[self.index + 1] if self.index + 1 < len(self.tokens) else None
        if token is None or token[0] != "ident" or following != ("op", "("):
            return super().parse_primary()
        name = self.take()[1]
        self.take()
        args: list[object] = []
        if self.peek() != ("op", ")"):
            args.append(self.parse_or())
            while self.peek() == ("op", ","):
                self.take()
                args.append(self.parse_or())
        self.expect_op(")")
        if name == "always":
            return True
        if name in ("startsWith", "endsWith", "contains") and not any(
            isinstance(arg, _Unknown) for arg in args
        ):
            return FUNCTIONS[name](args)
        # success(), failure(), hashFiles(), format(), fromJSON(), ...
        return UNKNOWN


def _open_evaluate(expression: str, frame: OpenFrame) -> object:
    expression = " ".join(expression.split())
    match = re.fullmatch(r"\$\{\{(.*)\}\}", expression)
    if match and "}}" not in match.group(1):
        expression = match.group(1)
    try:
        return _OpenParser(expression, frame).parse()
    except UnsupportedExpression:
        # Syntax this reader does not model (an index, a filter) is unknown.
        return UNKNOWN


def _may_hold(condition: str | None, frame: OpenFrame) -> bool:
    """False only when `condition` is false whatever the unknowns are."""
    if condition is None:
        return True
    return _known_truthy(_open_evaluate(condition, frame)) is not False


def _as_input(value: object) -> object:
    """A value as an action input receives it (`true`, not True)."""
    if isinstance(value, _Unknown):
        return value
    if value is None:
        return ""
    if isinstance(value, bool):
        return "true" if value else "false"
    return str(value)


def _open_template(template: str, frame: OpenFrame, unknown: str | None) -> object:
    """Render a `with:` value: a lone expression keeps its value; otherwise
    each unknown part becomes `unknown` (a `*` for a cache-key pattern), or
    the whole value is unknown when `unknown` is None."""
    pieces = EXPRESSION.split(template.strip())
    if unknown is None and len(pieces) == 3 and pieces[0] == "" and pieces[2] == "":
        return _as_input(_open_evaluate(pieces[1], frame))
    rendered: list[str] = []
    for index, piece in enumerate(pieces):
        if index % 2 == 0:
            rendered.append(piece)
            continue
        value = _as_input(_open_evaluate(piece, frame))
        if isinstance(value, _Unknown):
            if unknown is None:
                return UNKNOWN
            rendered.append(unknown)
        else:
            rendered.append(value)
    return "".join(rendered)


def _specs_overlap(restored: tuple[str, str], saved: tuple[str, str]) -> bool:
    """Whether an entry saved under `saved` can be restored under `restored`.

    Two families match only when equal (the suffix after them has the same
    shape for both); a family matches a whole key that starts with it.
    """
    (left_kind, left), (right_kind, right) = restored, saved
    if left_kind == "family" and right_kind == "key":
        left += "-*"
    elif left_kind == "key" and right_kind == "family":
        right += "-*"
    return _globs_overlap(left, right)


def _globs_overlap(left: str, right: str) -> bool:
    """Whether two `*` patterns match one common string (case-insensitive)."""
    left, right = left.lower(), right.lower()
    memo: dict[tuple[int, int], bool] = {}

    def match(i: int, j: int) -> bool:
        if (i, j) in memo:
            return memo[(i, j)]
        if i == len(left) and j == len(right):
            result = True
        elif i < len(left) and left[i] == "*":
            result = match(i + 1, j) or (j < len(right) and match(i, j + 1))
        elif j < len(right) and right[j] == "*":
            result = match(i, j + 1) or (i < len(left) and match(i + 1, j))
        else:
            result = i < len(left) and j < len(right) and left[i] == right[j] and match(i + 1, j + 1)
        memo[(i, j)] = result
        return result

    return match(0, 0)


@dataclass(frozen=True)
class _LocalAction:
    """A local composite action: where it lives, its input defaults, its steps."""

    name: str
    defaults: dict[str, str]
    steps: tuple[Step, ...]


def _local_action(uses: str, root: Path) -> _LocalAction | None:
    """The composite action a `uses: ./...` step runs, read from `root`.

    `./.release-tooling/` is release.yml's checkout of its own commit, so it
    resolves to the same tree.
    """
    if not uses.startswith("./"):
        return None
    relative = uses[2:].rstrip("/")
    if relative.startswith(".release-tooling/"):
        relative = relative[len(".release-tooling/") :]
    for candidate in ("action.yml", "action.yaml"):
        path = root / relative / candidate
        if path.is_file():
            break
    else:
        raise ContractError(f"local action `{uses}` has no action.yml under {root}")
    lines = path.read_text(encoding="utf-8").splitlines()

    defaults: dict[str, str] = {}
    current: str | None = None
    section: str | None = None
    for line in lines:
        if line.strip() and not line.startswith(" "):
            section = line.split(":", 1)[0]
            continue
        if section != "inputs":
            continue
        name = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", line)
        if name:
            current = name.group(1)
            continue
        default = re.match(r"^    default:\s*(.*?)\s*$", line)
        if default and current is not None:
            defaults[current] = default.group(1).strip("'\"")

    try:
        start = lines.index("runs:")
        steps_at = next(
            index for index in range(start + 1, len(lines)) if lines[index] == "  steps:"
        )
    except (ValueError, StopIteration) as error:
        raise ContractError(f"local action `{uses}` is not a composite action with steps") from error
    body: list[str] = []
    for line in lines[steps_at + 1 :]:
        if line.strip() and _indent(line) < 4:
            break
        # The workflow layout one level deeper, which job_steps reads.
        body.append("  " + line if line.strip() else line)
    steps = job_steps(["    steps:", *body])
    return _LocalAction(relative, defaults, tuple(steps))


@dataclass(frozen=True)
class _CacheUse:
    """One cache step a job runs, directly or through local actions."""

    workflow: str
    job: str
    via: tuple[str, ...]
    step: Step
    kind: str  # cache | restore | save | rust-cache | setup
    frame: OpenFrame
    # The release.yml job that runs this step through a reusable workflow.
    caller: str = ""

    @property
    def where(self) -> str:
        chain = " > ".join(f"`{name}`" for name in self.via)
        if self.caller:
            return f"job `{self.caller}` (through {self.workflow} job `{self.job}`" + (f" in {chain}" if chain else "") + ")"
        return f"job `{self.job}`" + (f" (in {chain})" if chain else "")

    @property
    def uses(self) -> str:
        return self.step.fields.get("uses", "").strip()

    def _with(self, name: str, unknown: str | None = None) -> object:
        value = self.step.nested("with").get(name)
        return None if value is None else _open_template(value, self.frame, unknown)

    def key_specs(self) -> tuple[list[tuple[str, str]], list[tuple[str, str]]]:
        """What it may restore and what it may save, as (`key`, pattern) for
        `*` patterns over a whole cache key, or (`family`, pattern) for the
        part of a rust-cache or setup-* key before the runner and
        environment suffix every such key ends with."""
        if self.kind == "rust-cache":
            # Swatinem/rust-cache: <prefix-key>-<shared-key | key-job>-<os>-...
            prefix = self._with("prefix-key", "*") or "v0-rust"
            shared = self._with("shared-key", "*")
            if shared:
                family = f"{prefix}-{shared}"
            else:
                family = str(prefix)
                key = self._with("key", "*")
                if key:
                    family += f"-{key}"
                if str(self._with("add-job-id-key") or "true").lower() != "false":
                    family += f"-{self.job}"
            return [("family", family)], [("family", family)]
        if self.kind == "setup":
            family = f"setup-{self.uses.split('@', 1)[0]}"
            return [("family", family)], [("family", family)]
        key = str(self._with("key", "*") or "*")
        restores = [("key", key)]
        for line in (self.step.nested("with").get("restore-keys") or "").splitlines():
            if line.strip():
                restores.append(("key", f"{_open_template(line.strip(), self.frame, '*')}*"))
        return (restores if self.kind != "save" else []), ([("key", key)] if self.kind != "restore" else [])

    def may_save(self) -> bool:
        if self.kind in ("cache", "save", "setup"):
            return True
        if self.kind == "rust-cache":
            save_if = self.step.nested("with").get("save-if")
            return save_if is None or _may_hold(save_if, self.frame)
        return False

    def may_restore(self) -> bool:
        return self.kind != "save"


def _cache_kind(uses: str, step: Step, root: Path) -> str | None:
    action = uses.split("@", 1)[0]
    if action == "actions/cache":
        return "cache"
    if action == "actions/cache/restore":
        return "restore"
    if action == "actions/cache/save":
        return "save"
    if action == "Swatinem/rust-cache":
        return "rust-cache"
    if action in SETUP_CACHE_ACTIONS:
        inputs = step.nested("with")
        cache = inputs.get("cache", "")
        if action == "actions/setup-go":
            return None if cache.lower() == "false" else "setup"
        if cache:
            return "setup"
        if action == "actions/setup-node" and inputs.get("package-manager-cache", "").lower() != "false":
            package = root / "package.json"
            if package.is_file() and re.search(
                r'"(?:packageManager|devEngines)"', package.read_text(encoding="utf-8")
            ):
                return "setup"
    return None


def _walk_cache_steps(
    steps: Iterable[Step],
    frame: OpenFrame,
    root: Path,
    workflow: str,
    job: str,
    via: tuple[str, ...] = (),
) -> Iterable[_CacheUse]:
    for step in steps:
        if not _may_hold(step.condition, frame):
            continue
        uses = step.fields.get("uses", "").strip()
        kind = _cache_kind(uses, step, root)
        if kind is not None:
            yield _CacheUse(workflow, job, via, step, kind, frame)
            continue
        action = _local_action(uses, root)
        if action is None:
            continue
        if action.name in via or len(via) > 8:
            raise ContractError(f"local action `{action.name}` calls itself")
        inputs: dict[str, object] = dict(action.defaults)
        for name, value in step.nested("with").items():
            inputs[name] = _open_template(value, frame, None)
        inner = OpenFrame(frame.label, frame.event_name, frame.ref, inputs, frame.env)
        yield from _walk_cache_steps(action.steps, inner, root, workflow, job, (*via, action.name))


def _workflow_triggers(text: str) -> set[str]:
    lines = text.splitlines()
    for index, line in enumerate(lines):
        match = re.match(r"""^["']?on["']?:\s*(.*?)\s*(?:#.*)?$""", line)
        if not match:
            continue
        if match.group(1):
            return {name.strip() for name in match.group(1).strip("[]").split(",") if name.strip()}
        triggers: set[str] = set()
        for candidate in lines[index + 1 :]:
            if candidate.strip() and not candidate.startswith(" "):
                break
            # `workflow_dispatch:`, `workflow_dispatch: {}`, `- push`.
            named = re.match(r"^  (?:- )?([A-Za-z_]+)\s*(?::.*)?$", candidate)
            if named:
                triggers.add(named.group(1))
        return triggers
    raise ContractError("the workflow has no `on:` triggers")


def _env_mapping(lines: list[str], indent: int) -> dict[str, str]:
    """The `env:` mapping at `indent` (a workflow's at 0, a job's at 4)."""
    for index, line in enumerate(lines):
        if line == " " * indent + "env:":
            nested: list[str] = []
            for candidate in lines[index + 1 :]:
                if candidate.strip() and _indent(candidate) <= indent:
                    break
                nested.append(candidate)
            return {key: value.strip().strip("'\"") for key, value in parse_mapping(nested, indent + 2).items()}
    return {}


@lru_cache(maxsize=None)
def _github_env_writes(root: Path) -> frozenset[str]:
    """Every name a run script under .github may set through $GITHUB_ENV: a
    superset, each NAME= or NAME<< in a file that mentions GITHUB_ENV."""
    names: set[str] = set()
    for path in sorted((root / ".github").rglob("*")):
        if path.suffix not in (".yml", ".yaml") or not path.is_file():
            continue
        text = path.read_text(encoding="utf-8")
        if "GITHUB_ENV" in text:
            names.update(re.findall(r"\b([A-Za-z_][A-Za-z0-9_]*)(?:=|<<)", text))
    return frozenset(names)


def _static_env(text: str, block: list[str], root: Path) -> dict[str, str]:
    """A job's env that a static reading knows: the workflow's and the job's
    `env:` values that hold no expression and that no run script can
    replace through $GITHUB_ENV."""
    lines = text.splitlines()
    top = lines[: lines.index("jobs:")] if "jobs:" in lines else lines
    env = {**_env_mapping(top, 0), **_env_mapping(block, 4)}
    written = _github_env_writes(root)
    return {key: value for key, value in env.items() if "${{" not in value and key not in written}


def _job_cache_uses(
    text: str, workflow: str, job: str, frame: OpenFrame, root: Path
) -> list[_CacheUse] | None:
    """The cache steps `job` may run under `frame`; None for a reusable-workflow call."""
    block = job_block(text, job)
    fields = parse_mapping(block, 4)
    if "uses" in fields:
        return None
    if not _may_hold(fields.get("if"), frame):
        return []
    framed = OpenFrame(frame.label, frame.event_name, frame.ref, frame.inputs, _static_env(text, block, root))
    return list(_walk_cache_steps(job_steps(block), framed, root, workflow, job))


def _job_with(block: list[str]) -> dict[str, str]:
    """A reusable-workflow call's `with:` inputs, values unquoted."""
    for index, line in enumerate(block):
        match = KEY_LINE.match(line)
        if match and len(match.group(1)) == 4 and match.group(2) == "with":
            nested: list[str] = []
            for candidate in block[index + 1 :]:
                if candidate.strip() and _indent(candidate) <= 4:
                    break
                nested.append(candidate)
            return {key: value.strip().strip("'\"") for key, value in parse_mapping(nested, 6).items()}
    return {}


def _workflow_call_defaults(text: str) -> dict[str, str]:
    """The defaults of a reusable workflow's `on: workflow_call: inputs:`."""
    defaults: dict[str, str] = {}
    lines = text.splitlines()
    start = next((index for index, line in enumerate(lines) if re.match(r"^  workflow_call:\s*$", line)), None)
    if start is None:
        return defaults
    in_inputs = False
    current: str | None = None
    for line in lines[start + 1 :]:
        if line.strip() and _indent(line) <= 2:
            break
        if re.match(r"^    \S", line):
            in_inputs = line.strip() == "inputs:"
            continue
        if not in_inputs:
            continue
        name = re.match(r"^      ([A-Za-z0-9_-]+):\s*$", line)
        if name:
            current = name.group(1)
            continue
        default = re.match(r"^        default:\s*(.*?)\s*$", line)
        if default and current is not None:
            defaults[current] = default.group(1).strip("'\"")
    return defaults


def _reusable_cache_uses(
    text: str, job: str, frame: OpenFrame, root: Path
) -> list[tuple[_CacheUse, bool]] | None:
    """The cache steps a reusable-workflow call runs under `frame`.

    Each job of the called workflow is read with the caller's `with:` over
    the workflow's input defaults, in the caller's event and ref, and paired
    with whether it may mint OIDC tokens or attest (its own permissions or
    its workflow's). None when the call cannot be read: not a workflow of
    this repository, or one that calls another reusable workflow.
    """
    block = job_block(text, job)
    uses = parse_mapping(block, 4).get("uses", "").split("#", 1)[0].strip()
    if not uses.startswith("./.github/workflows/"):
        return None
    path = root / uses[2:]
    if not path.is_file():
        return None
    source = path.read_text(encoding="utf-8")
    inputs: dict[str, object] = dict(_workflow_call_defaults(source))
    for name, value in _job_with(block).items():
        inputs[name] = _open_template(value, frame, None)
    called_frame = OpenFrame(frame.label, frame.event_name, frame.ref, inputs)
    lines = source.splitlines()
    workflow_scopes = _permission_scopes(lines[: lines.index("jobs:")], 0) if "jobs:" in lines else {}
    found: list[tuple[_CacheUse, bool]] = []
    for called in _job_names(source):
        uses_found = _job_cache_uses(source, path.name, called, called_frame, root)
        if uses_found is None:
            return None
        scopes = {**workflow_scopes, **_permission_scopes(job_block(source, called), 4)}
        sensitive = scopes.get("id-token") == "write" or scopes.get("attestations") == "write"
        found.extend((replace(use, caller=job), sensitive) for use in uses_found)
    return found


def _sccache_problems(root: Path) -> list[str]:
    """Which runs set the sccache client to write, and how the setup installs
    sccache (see SCCACHE_ACTION)."""
    where = SCCACHE_ACTION[2:]
    try:
        action = _local_action(SCCACHE_ACTION, root)
    except ContractError as error:
        return [f"{where}: {error}"]
    if action is None:
        return [f"{where} is not a local action"]
    problems: list[str] = []
    steps = list(action.steps)
    if any("mozilla-actions/sccache-action" in step.fields.get("uses", "") for step in steps):
        problems.append(
            f"{where} installs sccache through sccache-action, which exports the job's Actions cache token"
            " to the run steps after it"
        )
    pinned = [
        step
        for step in steps
        if re.fullmatch(r"v\d+\.\d+\.\d+", step.env.get("SCCACHE_VERSION", ""))
        and re.fullmatch(r"[0-9a-f]{64}", step.env.get("SCCACHE_SHA256", ""))
        and re.search(r'^\s*echo "\$\{SCCACHE_SHA256\}  .*" \| sha256sum --check --strict -$', step.run, re.M)
    ]
    if not pinned:
        problems.append(
            f"{where} installs no sccache release checked against a pinned sha256"
            " (SCCACHE_VERSION, SCCACHE_SHA256, sha256sum --check --strict -)"
        )
    writes = [step for step in steps if "SCCACHE_GCS_RW_MODE=" in step.run]
    if not writes:
        problems.append(f"{where} sets no SCCACHE_GCS_RW_MODE")
    for step in writes:
        if "SCCACHE_GCS_RW_MODE=${RW_MODE}" not in step.run or step.run.count("SCCACHE_GCS_RW_MODE=") != 1:
            problems.append(f"{where} step `{step.name}` sets SCCACHE_GCS_RW_MODE other than from RW_MODE")
            continue
        mode = step.env.get("RW_MODE")
        if mode is None:
            problems.append(f"{where} step `{step.name}` has no RW_MODE")
            continue
        for event, ref in (*SCCACHE_WRITERS, *SCCACHE_READERS):
            got = _open_evaluate(mode, OpenFrame(label=f"a {event} run on {ref}", event_name=event, ref=ref))
            wanted = "READ_WRITE" if (event, ref) in SCCACHE_WRITERS else "READ_ONLY"
            if got != wanted:
                shown = "unknown" if isinstance(got, _Unknown) else got
                problems.append(f"{where} gives a {event} run on {ref} sccache mode {shown}, not {wanted}")
    return problems


def _checks_out_input_ref(text: str) -> bool:
    """Does any job check out a ref that the run's inputs choose?"""
    for job in _job_names(text):
        for step in _steps_of(text, job):
            if CHECKOUT_ACTION in step.fields.get("uses", "") and "inputs." in step.nested("with").get("ref", ""):
                return True
    return False


def check_release_caches(text: str) -> list[str]:
    """What release.yml's caches can carry, read through every local action.

    Covers each release.yml job and the local composite actions it calls,
    recursively, and on a tag the reusable workflows it calls, read with the
    caller's inputs (a call that can run on a dispatch, or one this check
    cannot read, fails). A dispatch on main saves into the default branch's
    cache scope, which every tag run restores, so no cache step saves on a
    dispatch; no rust-cache caches ~/.cargo/bin, so a restored binary cannot
    shadow a tool on PATH; a job holding a registry or publish token (or
    `contents: write`) runs no cache step; no release job restores a cache,
    on a tag or a dispatch (RELEASE_RESTORE_JOBS is empty), and no job of
    release-semver-readiness.yml restores one on any of its triggers; and a job that
    mints OIDC tokens or attests
    artifacts
    (`id-token` or `attestations: write`) restores only caches that no
    workflow in the repository saves on a dispatch-like run (any trigger but
    push, pull_request, schedule and merge_group), through the same local
    action or an overlapping key. No workflow saves the Bazel repository
    cache on a dispatch-like run at all: such runs only restore it. A
    workflow that produces what a tag run trusts (TAG_TRUSTED_WORKFLOWS),
    or that checks out a ref its inputs choose, saves no cache at all on a
    dispatch-like run. Only push and schedule runs, and a dispatch on a
    release branch (rel/** or release/**), set the CI lanes' sccache
    client to write its bucket (a client-side setting, not a credential),
    and the lanes install a sha256-pinned sccache release themselves, so no
    step exports the Actions cache token to their run steps
    (_sccache_problems).
    """
    root = REPO_ROOT
    violations: list[str] = []
    dispatch = OpenFrame(label="a dispatch", event_name="workflow_dispatch")
    tag = OpenFrame(label="a tag push", event_name="push", ref="refs/tags/v0.0.0")
    names = _job_names(text)
    scopes = {job: _permission_scopes(job_block(text, job), 4) for job in names}
    token_jobs = set(TOKEN_JOBS) | {job for job in names if scopes[job].get("contents") == "write"}
    sensitive = {
        job
        for job in names
        if scopes[job].get("id-token") == "write" or scopes[job].get("attestations") == "write"
    }

    def report(message: str) -> None:
        if message not in violations:
            violations.append(message)

    restored: list[tuple[_CacheUse, str]] = []

    def inspect(use: _CacheUse, frame: OpenFrame, token_job: bool, sensitive_job: bool) -> None:
        if token_job:
            report(f"{use.where} holds a registry or publish token and restores a cache (`{use.uses}`)")
            return
        if use.kind == "rust-cache" and str(use._with("cache-bin")).lower() != "false":
            report(f"{use.where} caches ~/.cargo/bin (`cache-bin` is not false)")
        if frame is dispatch and use.may_save():
            if use.kind == "rust-cache":
                report(f"{use.where} saves its rust-cache on a dispatch")
            else:
                saved = ", ".join(f"`{pattern}`" for _, pattern in use.key_specs()[1])
                report(f"{use.where} saves cache {saved} on a dispatch (`{use.uses}`)")
        if use.may_restore() and (frame is tag or use.job not in RELEASE_RESTORE_JOBS):
            for spec in use.key_specs()[0]:
                report(f"{use.where} restores `{spec[1]}` on {frame.label}; a release job restores no cache")
        if sensitive_job and use.may_restore():
            restored.extend((use, spec) for spec in use.key_specs()[0])

    for job in names:
        fields = parse_mapping(job_block(text, job), 4)
        if "uses" in fields:
            if job in token_jobs:
                report(f"job `{job}` holds a registry or publish token and calls a reusable workflow")
            if _may_hold(fields.get("if"), dispatch):
                report(
                    f"job `{job}` calls a reusable workflow on a dispatch, whose cache saves "
                    "this check does not read"
                )
            if not _may_hold(fields.get("if"), tag):
                continue
            called = _reusable_cache_uses(text, job, tag, root)
            if called is None:
                report(f"job `{job}` calls a reusable workflow on a tag that this check cannot read")
                continue
            for use, called_sensitive in called:
                inspect(use, tag, job in token_jobs, job in sensitive or called_sensitive)
            continue
        for frame in (dispatch, tag):
            for use in _job_cache_uses(text, DEFAULT_WORKFLOW.name, job, frame, root) or []:
                inspect(use, frame, job in token_jobs, job in sensitive)

    workflows = sorted((root / ".github" / "workflows").glob("*.y*ml"))
    sources = {path.name: path.read_text(encoding="utf-8") for path in workflows}
    # The workflow under check, which may be a mutated copy.
    sources[DEFAULT_WORKFLOW.name] = text
    for problem in _sccache_problems(root):
        report(problem)
    for trusted in TAG_TRUSTED_WORKFLOWS:
        if trusted not in sources:
            report(f"{trusted}, a workflow whose runs a tag trusts, is missing; update TAG_TRUSTED_WORKFLOWS")
    for workflow, source in sorted(sources.items()):
        triggers = _workflow_triggers(source)
        if workflow == READINESS_WORKFLOW.name:
            for trigger in sorted(triggers):
                frame = OpenFrame(
                    label=f"a {trigger} run",
                    event_name=None if trigger == "workflow_call" else trigger,
                )
                for job in _job_names(source):
                    uses = _job_cache_uses(source, workflow, job, frame, root)
                    if uses is None:
                        report(f"{workflow} job `{job}` calls a reusable workflow, whose caches this check does not read")
                    for use in uses or []:
                        for spec in use.key_specs()[0] if use.may_restore() else []:
                            report(
                                f"{workflow} {use.where} restores `{spec[1]}` on {frame.label}; "
                                "the semver measurement restores no cache"
                            )
        # release.yml's own jobs are checked on a dispatch above.
        guarded = [] if workflow == DEFAULT_WORKFLOW.name else [
            reason
            for reason, applies in (
                ("produces what a tag run trusts", workflow in TAG_TRUSTED_WORKFLOWS),
                ("checks out a ref its inputs choose", _checks_out_input_ref(source)),
            )
            if applies
        ]
        for trigger in sorted(triggers - set(TRUSTED_SAVE_TRIGGERS)):
            frame = OpenFrame(
                label=f"a {trigger} run",
                event_name=None if trigger == "workflow_call" else trigger,
            )
            for job in _job_names(source):
                for writer in _job_cache_uses(source, workflow, job, frame, root) or []:
                    if not writer.may_save():
                        continue
                    if guarded:
                        report(
                            f"{workflow} {writer.where} saves a cache on {frame.label} (`{writer.uses}`); "
                            f"a workflow that {' and '.join(guarded)} saves none on such a run"
                        )
                    for saved in writer.key_specs()[1]:
                        if _specs_overlap(BAZEL_REPOSITORY_CACHE, saved):
                            report(
                                f"{workflow} {writer.where} saves `{saved[1]}` on {frame.label}; "
                                "such runs only restore the Bazel repository cache"
                            )
                        for use, spec in restored:
                            if _specs_overlap(spec, saved):
                                report(
                                    f"{use.where} restores `{spec[1]}`, which {workflow} "
                                    f"{writer.where} saves on {frame.label}"
                                )
    return violations


WEB_SDK_PACK_MODE_ENV = "MEERKAT_WEB_WASM_PACK_MODE"


def _locked_version(crate: str) -> str | None:
    """The first version of `crate` in the checkout's Cargo.lock."""
    lines = (REPO_ROOT / "Cargo.lock").read_text(encoding="utf-8").splitlines()
    for index, line in enumerate(lines[:-1]):
        if line == f'name = "{crate}"':
            match = re.fullmatch(r'version = "([^"]+)"', lines[index + 1])
            if match:
                return match.group(1)
    return None


def check_web_sdk_tools(text: str) -> list[str]:
    """The attesting Web SDK build downloads nothing unchecked.

    Every download in its steps is checked against a pinned sha256, and
    wasm-pack runs with `--mode no-install` (so it uses the pinned
    wasm-bindgen and wasm-opt on PATH instead of fetching its own). The
    pinned wasm-bindgen is the version Cargo.lock locks, since wasm-pack
    refuses any other: a lock bump without a pin bump fails here, not at
    release time.
    """
    violations: list[str] = []
    steps = _steps_of(text, WEB_SDK_BUILD_JOB)
    if not steps:
        return [f"job `{WEB_SDK_BUILD_JOB}` is not defined in the workflow"]
    for step in steps:
        run = step.raw_run
        downloads = len(re.findall(r"\bcurl\b", run))
        checks = run.count("sha256sum --check")
        if downloads > checks:
            violations.append(
                f"step `{step.name}` downloads {downloads} file(s) but checks {checks} sha256"
            )
    build = [step for step in steps if "release-build-web-sdk-package.sh" in step.run]
    if len(build) != 1:
        violations.append(f"job `{WEB_SDK_BUILD_JOB}` has {len(build)} steps that build the package, not one")
    elif build[0].env.get(WEB_SDK_PACK_MODE_ENV) != "no-install":
        violations.append(
            f"step `{build[0].name}` lets wasm-pack download its own wasm-bindgen and wasm-opt "
            f"(`{WEB_SDK_PACK_MODE_ENV}` is not `no-install`)"
        )
    pins = [
        match.group(1)
        for step in steps
        if (match := re.search(r'^bindgen_version="([^"]+)"$', step.raw_run, re.MULTILINE))
    ]
    locked = _locked_version("wasm-bindgen")
    if len(pins) != 1:
        violations.append(f"job `{WEB_SDK_BUILD_JOB}` pins wasm-bindgen {len(pins)} times, not once")
    elif locked is None:
        violations.append("Cargo.lock locks no wasm-bindgen")
    elif pins[0] != locked:
        violations.append(
            f"job `{WEB_SDK_BUILD_JOB}` pins wasm-bindgen {pins[0]}, but Cargo.lock locks {locked}; "
            "wasm-pack --mode no-install would fail the release"
        )
    if not any("bin/wasm-opt" in step.raw_run and "GITHUB_PATH" in step.raw_run for step in steps):
        violations.append(f"job `{WEB_SDK_BUILD_JOB}` puts no pinned wasm-opt on PATH")
    return violations


def _readiness_push(needed: str, baseline: str) -> EventContext:
    return EventContext(
        label=f"a main push with needed={needed!r} and notes baseline {baseline!r}",
        event_name="push",
        ref="refs/heads/main",
        step_outputs={"unpublished.needed": needed, f"{NOTES_STEP_ID}.{NOTES_OUTPUT}": baseline},
    )


def check_readiness_attestation(text: str) -> list[str]:
    """Only a release tree becomes release evidence (release-semver-readiness.yml).

    A post-release tree is measured against its own tag, so its green result
    says nothing about the breaks since the release before. `release_semver_gate`
    trusts the main-push attestation by tree and version alone, so the
    attestation and its upload must run for a release tree and never for a
    post-release one, keyed on the output the classifying step really writes.
    """
    block = job_block(text, READINESS_JOB)
    steps = job_steps(block)
    violations: list[str] = []

    notes = [step for step in steps if step.fields.get("id") == NOTES_STEP_ID]
    if len(notes) != 1:
        violations.append(
            f"job `{READINESS_JOB}` has {len(notes)} steps with `id: {NOTES_STEP_ID}`; "
            "exactly one must classify the measured notes"
        )
    else:
        written = notes[0].written_outputs()
        if NOTES_OUTPUT not in written:
            violations.append(
                f"step `{notes[0].name}` writes {sorted(written) or 'no outputs'} to "
                f"$GITHUB_OUTPUT, not `{NOTES_OUTPUT}`, which the attestation is gated on"
            )
        if not step_runs(notes[0], _readiness_push("true", "published")):
            violations.append(f"step `{notes[0].name}` does not run when a measurement is needed")

    attestation = [step for step in steps if "attestation.json" in step.run]
    upload = [
        step
        for step in steps
        if "upload-artifact" in step.fields.get("uses", "")
        and EVIDENCE_ARTIFACT_PREFIX in step.fields.get("with", "")
    ]
    if not attestation:
        violations.append(f"job `{READINESS_JOB}` has no step that writes attestation.json")
    if not upload:
        violations.append(
            f"job `{READINESS_JOB}` has no step that uploads the "
            f"`{EVIDENCE_ARTIFACT_PREFIX}<tree>` artifact"
        )
    release_tree = _readiness_push("true", "published")
    post_release = _readiness_push("true", "workspace-version")
    not_needed = _readiness_push("false", "published")
    for step in attestation + upload:
        if not step_runs(step, release_tree):
            violations.append(f"step `{step.name}` does not run on {release_tree.label}")
        for context in (post_release, not_needed):
            if step_runs(step, context):
                violations.append(f"step `{step.name}` also runs on {context.label}")
    return violations


CHECKS: dict[str, Callable[[str], list[str]]] = {
    "semver-evidence": check_semver_evidence,
    "registry-slo": check_registry_slo,
    "dispatch-binding": check_dispatch_binding,
    "tag-validation": check_tag_validation,
    "prebuild": check_prebuild,
    "semver-baseline": check_semver_baseline,
    "trailing-platforms": check_trailing_platforms,
    "token-permissions": check_token_permissions,
    "release-caches": check_release_caches,
    "web-sdk-tools": check_web_sdk_tools,
}

# Checks of release-semver-readiness.yml; run them by name with
# `--workflow .github/workflows/release-semver-readiness.yml`.
READINESS_CHECKS: dict[str, Callable[[str], list[str]]] = {
    "readiness-attestation": check_readiness_attestation,
}


def run_checks(text: str, names: Iterable[str]) -> list[str]:
    violations: list[str] = []
    for name in names:
        try:
            violations.extend({**CHECKS, **READINESS_CHECKS}[name](text))
        except ContractError as error:
            violations.append(f"{name}: {error}")
    return violations


def main(argv: list[str] | None = None) -> int:
    global REPO_ROOT
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument(
        "--workflow",
        type=Path,
        default=DEFAULT_WORKFLOW,
        help=f"release workflow to inspect (default: {DEFAULT_WORKFLOW})",
    )
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=REPO_ROOT,
        help="tree the local actions and the other workflows are read from (default: this checkout)",
    )
    parser.add_argument(
        "checks",
        nargs="*",
        choices=[*CHECKS, *READINESS_CHECKS, "all"],
        default=["all"],
        help="which contract checks to run (default: all release.yml checks)",
    )
    args = parser.parse_args(argv)
    REPO_ROOT = args.repo_root.resolve()
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
