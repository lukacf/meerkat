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
SLO, or making the SDK packages wait on it all fail and name the defect.

Only the Python standard library is used, so the doctor and its contract test
run wherever `python3` does.
"""

from __future__ import annotations

import argparse
import os
import re
import subprocess
import sys
import tempfile
from collections.abc import Callable, Iterable
from dataclasses import dataclass, field
from pathlib import Path

DEFAULT_WORKFLOW = Path(".github/workflows/release.yml")
READINESS_WORKFLOW = Path(".github/workflows/release-semver-readiness.yml")
TAG_SLO_SECONDS = 1800

SEMVER_GATE_JOB = "release_semver_gate"
CI_GREEN_JOB = "require_ci_green"
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

    def resolve(self, path: str) -> str:
        if path == "github.event_name":
            return self.event_name
        if path == "github.ref":
            return self.ref
        if path == "github.ref_name":
            return github_ref_name(self.ref)
        if path.startswith("github.event.inputs."):
            # Unset dispatch inputs and push events both read as empty.
            return self.inputs.get(path[len("github.event.inputs.") :], "")
        needs = re.fullmatch(r"needs\.[A-Za-z0-9_-]+\.result", path)
        if needs:
            return self.needs_result
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
    for ref in ("refs/heads/main", "refs/heads/release/v0.0.0", "refs/heads/hotfix/0.0.0")
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
        (other_alpha_on_tag, ("alpha/v9.9.9",), checkout_for(other_alpha_on_tag), ""),
        (other_alpha_named, ("alpha/v9.9.9",), checkout_for(other_alpha_named), ""),
        (alpha_without_lane, (alpha_tag,), checkout_for(alpha_without_lane), ""),
        (alpha_named_without_lane, (alpha_tag,), checkout_for(alpha_named_without_lane), ""),
        # release_tag names the version but no such tag exists (a branch).
        (named_tag, (), None, " (no such tag exists)"),
        # The tag exists but the checked-out commit is a later one.
        (named_tag, (tag,), None, " (the tag is not the checked-out commit)"),
        *((context, (), checkout_for(context), "") for context in BRANCH_DISPATCHES),
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
        choices=[*CHECKS, *READINESS_CHECKS, "all"],
        default=["all"],
        help="which contract checks to run (default: all release.yml checks)",
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
