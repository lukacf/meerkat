#!/usr/bin/env bash
# Contract test for the owner-only fast-runner pool. Nothing here reaches
# GitHub or GCP: `gcloud` and `curl` are fakes on PATH that record every
# call, and the GitHub Actions planner below evaluates the workflows' job
# `if:`, `needs` and `runs-on` expressions the way GitHub schedules them.
#
#   1. Workflows (ultrafast.yml calling fast-runners.yml):
#      - nothing starts in GCP while MEERKAT_FAST_RUNNERS_ENABLED is unset,
#        for any event, and every lane resolves to its GitHub-hosted runner;
#      - only an owner workflow_dispatch on refs/heads/main plans the GCP
#        jobs; a fork or same-repository pull request, a push, a branch
#        dispatch, a collaborator dispatch and a collaborator re-run never
#        do, and no other workflow can reach the launcher or the pool label;
#      - runners-down is planned whatever up, the lanes or the run did, and
#        needs every pool lane;
#      - a re-run attempt the pool was not made for runs GitHub-hosted.
#   2. The job-started hook admits only the pinned run's owner dispatch on
#      main and refuses everything else before any step.
#   3. scripts/fast-runners up/down/fallback against the fakes: one unique
#      label per runner (no `self-hosted`), the registration only ever
#      encrypted, outbound-only VMs with a GCE TTL, clean-up on every
#      failure, and down deleting by deterministic names.
#   4. The rendered WIF condition and approval commands.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 - "${REPO_ROOT}" <<'PY'
import base64
import hashlib
import json
import os
import re
import stat
import subprocess
import sys
import tempfile

ROOT = sys.argv[1]
WORKFLOWS = os.path.join(ROOT, ".github", "workflows")
LAUNCHER_SCRIPT = os.path.join(ROOT, "scripts", "fast-runners")
HOOK = os.path.join(ROOT, "scripts", "fast-runners-job-started-hook")
GATE_VAR = "MEERKAT_FAST_RUNNERS_ENABLED"
OWNER = "lukacf"
CORE = "lukacf/meerkat"
ULTRAFAST_REF = f"{CORE}/.github/workflows/ultrafast.yml@refs/heads/main"
FAILURES = []


def fail(message):
    FAILURES.append(message)
    print(f"FAIL: {message}")


def ok(message):
    print(f"ok: {message}")


def ok_since(start, message):
    if len(FAILURES) == start:
        ok(message)


# --- a deliberately small reader for this repository's workflow style --------

def read_workflow(name):
    with open(os.path.join(WORKFLOWS, name), encoding="utf-8") as handle:
        lines = handle.read().splitlines()
    top = {}
    current = None
    for line in lines:
        match = re.match(r"^([A-Za-z_][A-Za-z0-9_-]*):(.*)$", line)
        if match:
            current = match.group(1)
            top[current] = []
            continue
        if current is not None:
            top[current].append(line)
    jobs, order, job = {}, [], None
    for line in top.get("jobs", []):
        match = re.match(r"^  ([A-Za-z0-9_-]+):\s*$", line)
        if match:
            job = match.group(1)
            order.append(job)
            jobs[job] = []
            continue
        if job is not None:
            jobs[job].append(line)
    return top, order, jobs


def indent(line):
    return len(line) - len(line.lstrip(" "))


def key_value(lines, key, depth=4):
    prefix = " " * depth
    for idx, line in enumerate(lines):
        match = re.match("^" + prefix + re.escape(key) + r":(.*)$", line)
        if not match:
            continue
        rest = match.group(1).strip()
        if rest in (">-", ">", "|", "|-"):
            block = []
            for nxt in lines[idx + 1:]:
                if nxt.strip() and indent(nxt) <= depth:
                    break
                if nxt.strip():
                    block.append(nxt.strip())
            return " ".join(block)
        if rest == "":
            block = []
            for nxt in lines[idx + 1:]:
                if not nxt.strip():
                    continue
                if indent(nxt) <= depth:
                    break
                block.append(nxt)
            return block
        return rest
    return None


def needs_of(lines):
    value = key_value(lines, "needs")
    if value is None:
        return []
    if isinstance(value, list):
        return [item.strip()[2:].strip() for item in value if item.strip().startswith("- ")]
    return [item.strip() for item in value.strip("[]").split(",") if item.strip()]


def mapping(block, depth):
    """KEY: VALUE children of a block, one level below `depth`."""
    result = {}
    for line in block or []:
        if indent(line) == depth + 2:
            key, _, value = line.strip().partition(":")
            value = value.strip()
            if len(value) >= 2 and value[0] == value[-1] and value[0] in "'\"":
                value = value[1:-1]
            result[key] = value
    return result


def strip_expr(value):
    value = (value or "").strip()
    if value.startswith("${{") and value.endswith("}}"):
        return value[3:-2].strip()
    return value


def interpolate(value):
    """Turns a `with:`/`runs-on:` value into one expression."""
    value = (value or "").strip()
    if value.startswith("${{") and value.endswith("}}") and value.count("${{") == 1:
        return value[3:-2].strip()
    return "'" + value.replace("'", "''") + "'"


# --- the GitHub Actions expression subset these workflows use ----------------

TOKEN = re.compile(
    r"\s*(?:('(?:[^']|'')*')|(==|!=|&&|\|\||[!()\[\],.*])|([A-Za-z_][A-Za-z0-9_\-]*)|(\d+))"
)


def tokenize(text):
    tokens, pos = [], 0
    text = text.strip()
    while pos < len(text):
        match = TOKEN.match(text, pos)
        if not match or match.end() == pos:
            sys.exit(f"cannot tokenize {text[pos:]!r} in {text!r}")
        pos = match.end()
        string, op, ident, number = match.groups()
        if string is not None:
            tokens.append(("str", string[1:-1].replace("''", "'")))
        elif op is not None:
            tokens.append(("op", op))
        elif ident is not None:
            tokens.append(("id", ident))
        else:
            tokens.append(("num", int(number)))
    return tokens


class Parser:
    def __init__(self, text):
        self.text, self.tokens, self.pos = text, tokenize(text), 0

    def peek(self):
        return self.tokens[self.pos] if self.pos < len(self.tokens) else (None, None)

    def take(self, value=None):
        token = self.peek()
        if token[0] is None or (value is not None and token[1] != value):
            sys.exit(f"unexpected {token!r} in {self.text!r}")
        self.pos += 1
        return token

    def parse(self):
        node = self.parse_or()
        if self.pos != len(self.tokens):
            sys.exit(f"trailing tokens in {self.text!r}")
        return node

    def parse_or(self):
        node = self.parse_and()
        while self.peek() == ("op", "||"):
            self.take()
            node = ("or", node, self.parse_and())
        return node

    def parse_and(self):
        node = self.parse_unary()
        while self.peek() == ("op", "&&"):
            self.take()
            node = ("and", node, self.parse_unary())
        return node

    def parse_unary(self):
        if self.peek() == ("op", "!"):
            self.take()
            return ("not", self.parse_unary())
        node = self.parse_postfix()
        if self.peek() in (("op", "=="), ("op", "!=")):
            op = self.take()[1]
            node = (op, node, self.parse_postfix())
        return node

    def parse_postfix(self):
        node = self.parse_primary()
        while self.peek() in (("op", "["), ("op", ".")):
            if self.take()[1] == "[":
                node = ("index", node, self.parse_or())
                self.take("]")
            else:
                kind, name = self.take()
                node = ("index", node, ("lit", "*" if name == "*" else name))
        return node

    def parse_primary(self):
        kind, value = self.peek()
        if (kind, value) == ("op", "("):
            self.take()
            node = self.parse_or()
            self.take(")")
            return node
        if kind in ("str", "num"):
            self.take()
            return ("lit", value)
        if kind == "id":
            self.take()
            if self.peek() == ("op", "("):
                self.take()
                args = []
                while self.peek() != ("op", ")"):
                    args.append(self.parse_or())
                    if self.peek() == ("op", ","):
                        self.take()
                self.take(")")
                return ("call", value, args)
            if value in ("true", "false"):
                return ("lit", value == "true")
            if value == "null":
                return ("lit", None)
            return ("ctx", value)
        sys.exit(f"unexpected {value!r} in {self.text!r}")


def truthy(value):
    return value not in (None, "", False, 0) and value != {} and value != []


def loose_equal(left, right):
    if left is None or right is None:
        return left is right
    return str(left).lower() == str(right).lower()


class Context:
    def __init__(self, github, variables, inputs, needs, results):
        self.github, self.vars, self.inputs = github, variables, inputs
        self.needs, self.results = needs, results


def evaluate(node, ctx):
    kind = node[0]
    if kind == "lit":
        return node[1]
    if kind == "and":
        left = evaluate(node[1], ctx)
        return evaluate(node[2], ctx) if truthy(left) else left
    if kind == "or":
        left = evaluate(node[1], ctx)
        return left if truthy(left) else evaluate(node[2], ctx)
    if kind == "not":
        return not truthy(evaluate(node[1], ctx))
    if kind in ("==", "!="):
        equal = loose_equal(evaluate(node[1], ctx), evaluate(node[2], ctx))
        return equal if kind == "==" else not equal
    if kind == "index":
        base, key = evaluate(node[1], ctx), evaluate(node[2], ctx)
        if isinstance(base, dict):
            return base.get(key)
        return None
    if kind == "call":
        name, args = node[1], node[2]
        if name == "always":
            return True
        if name == "cancelled":
            return ctx.results.get("__run__") == "cancelled"
        if name == "success":
            return all(ctx.results.get(need) == "success" for need in ctx.needs)
        if name == "failure":
            return any(ctx.results.get(need) == "failure" for need in ctx.needs)
        values = [evaluate(arg, ctx) for arg in args]
        if name == "fromJSON":
            return json.loads(values[0])
        if name == "startsWith":
            return str(values[0] or "").lower().startswith(str(values[1] or "").lower())
        if name == "contains":
            haystack = values[0]
            if isinstance(haystack, list):
                return any(loose_equal(item, values[1]) for item in haystack)
            return str(values[1] or "").lower() in str(haystack or "").lower()
        sys.exit(f"unsupported function {name}()")
    if kind == "ctx":
        root = node[1]
        if root == "github":
            return ctx.github
        if root == "vars":
            return ctx.vars
        if root == "inputs":
            return ctx.inputs
        if root == "needs":
            return {job: {"result": ctx.results.get(job, "skipped"),
                          "outputs": ctx.results.get(f"{job}.outputs", {})} for job in ctx.needs}
        if root == "jobs":
            return {job: {"outputs": value} for job, value in ctx.results.get("__jobs__", {}).items()}
        sys.exit(f"unsupported context {root!r}")
    sys.exit(f"unsupported node {node!r}")


def expr(text, ctx):
    return evaluate(Parser(text).parse(), ctx)


STATUS = re.compile(r"\b(always|success|failure|cancelled)\s*\(")

# --- planning ------------------------------------------------------------------

FAST_TOP, FAST_ORDER, FAST_JOBS = read_workflow("fast-runners.yml")
ULTRA_TOP, ULTRA_ORDER, ULTRA_JOBS = read_workflow("ultrafast.yml")
CALL_OUTPUTS = {}
_outputs_block = None
for idx, line in enumerate(FAST_TOP["on"]):
    if line.strip() == "outputs:":
        _outputs_block = FAST_TOP["on"][idx + 1:]
        break
_name = None
for line in _outputs_block or []:
    if indent(line) == 6 and line.strip().endswith(":"):
        _name = line.strip()[:-1]
    elif indent(line) == 8 and line.strip().startswith("value:") and _name:
        CALL_OUTPUTS[_name] = strip_expr(line.strip()[len("value:"):])
if set(CALL_OUTPUTS) != {"backend", "runs_on", "attempt"}:
    sys.exit(f"fast-runners.yml workflow_call outputs not understood: {CALL_OUTPUTS}")

POOL_LABEL = "meerkat-fast-9001-1-0123456789abcdef-x64-xl"


def plan_called(github, variables, inputs, up_outcome):
    """Plans fast-runners.yml; returns (planned jobs, call result, call outputs)."""
    results, planned, jobs_outputs = {}, [], {}
    for job in FAST_ORDER:
        lines = FAST_JOBS[job]
        condition = strip_expr(key_value(lines, "if") or "")
        if needs_of(lines):
            sys.exit(f"fast-runners.yml job {job} has needs; the planner does not model it")
        ctx = Context(github, variables, inputs, [], results)
        runs = truthy(expr(condition, ctx)) if condition else True
        if not runs:
            results[job] = "skipped"
            continue
        planned.append(job)
        if job == "runners-up":
            outcome = up_outcome
            if outcome == "fast":
                jobs_outputs[job] = {"backend": "fast", "runs_on": json.dumps({"x64-xl": POOL_LABEL}),
                                     "attempt": github["run_attempt"]}
                results[job] = "success"
            elif outcome == "hosted":
                jobs_outputs[job] = {"backend": "hosted", "runs_on": "{}", "attempt": github["run_attempt"]}
                results[job] = "success"
            else:
                results[job] = outcome
        else:
            results[job] = "success"
    out_ctx = Context(github, variables, inputs, [], {"__jobs__": {job: jobs_outputs.get(job, {}) for job in FAST_ORDER}})
    outputs = {name: expr(text, out_ctx) for name, text in CALL_OUTPUTS.items()}
    status = "success"
    for job in planned:
        if results[job] in ("failure", "cancelled"):
            status = results[job]
    return planned, status, outputs


def plan(github, variables, inputs, up_outcome="fast", lane_outcome="success", run_cancelled=False):
    """Plans ultrafast.yml with its calls; returns a report."""
    results = {"__run__": "cancelled" if run_cancelled else "running"}
    report = {"gcp_jobs": [], "runs_on": {}, "planned": [], "called": {}}
    for job in ULTRA_ORDER:
        lines = ULTRA_JOBS[job]
        needs = needs_of(lines)
        condition = strip_expr(key_value(lines, "if") or "")
        ctx = Context(github, variables, inputs, needs, results)
        if not condition or not STATUS.search(condition):
            if any(results.get(need) != "success" for need in needs) or run_cancelled:
                results[job] = "skipped"
                continue
        runs = truthy(expr(condition, ctx)) if condition else True
        if not runs:
            results[job] = "skipped"
            continue
        report["planned"].append(job)
        uses = key_value(lines, "uses")
        if uses:
            if uses != "./.github/workflows/fast-runners.yml":
                sys.exit(f"ultrafast job {job} calls {uses}")
            with_block = mapping(key_value(lines, "with"), 4)
            call_inputs = {key: expr(interpolate(value), ctx) for key, value in with_block.items()}
            planned, status, outputs = plan_called(github, variables, call_inputs, up_outcome)
            report["called"][job] = {"inputs": call_inputs, "planned": planned}
            report["gcp_jobs"] += [f"{job}/{inner}" for inner in planned]
            results[job] = status
            results[f"{job}.outputs"] = outputs
            continue
        runs_on = key_value(lines, "runs-on")
        report["runs_on"][job] = expr(interpolate(runs_on), ctx)
        results[job] = lane_outcome if job != "gate" else "success"
    report["results"] = results
    return report


def github_ctx(event="workflow_dispatch", ref="refs/heads/main", actor=OWNER, triggering_actor=None,
               repository=CORE, attempt="1"):
    return {
        "event_name": event, "ref": ref, "actor": actor,
        "triggering_actor": triggering_actor or actor,
        "repository": repository, "repository_owner": repository.split("/")[0],
        "run_attempt": attempt,
    }


def pool_lanes(report):
    return {job: label for job, label in report["runs_on"].items() if str(label).startswith("meerkat-fast-")}


ENABLED = {GATE_VAR: "true"}

# 1a. variable unset: nothing in GCP for any trigger; lanes hosted.
_s0 = len(FAILURES)
for name, github in (
    ("owner dispatch on main", github_ctx()),
    ("owner dispatch on a branch", github_ctx(ref="refs/heads/feature")),
    ("collaborator dispatch", github_ctx(actor="collaborator")),
):
    for backend in ("auto", "fast", "hosted"):
        report = plan(github, {}, {"backend": backend})
        if report["gcp_jobs"]:
            fail(f"{GATE_VAR} unset, {name}, backend {backend}: GCP jobs planned {report['gcp_jobs']}")
        elif pool_lanes(report):
            fail(f"{GATE_VAR} unset, {name}, backend {backend}: lanes on the pool {pool_lanes(report)}")
        elif any(label != "ubuntu-24.04" for job, label in report["runs_on"].items()):
            fail(f"{GATE_VAR} unset, {name}: a lane is not GitHub-hosted: {report['runs_on']}")
ok_since(_s0, f"{GATE_VAR} unset: no GCP job is planned for any dispatch or backend and every lane is GitHub-hosted")

# 1b. the eligible route.
report = plan(github_ctx(), ENABLED, {"backend": "auto"})
expected_gcp = ["fast-up/runners-up", "fast-down/runners-down"]
if report["gcp_jobs"] != expected_gcp:
    fail(f"owner dispatch on main: GCP jobs {report['gcp_jobs']}, expected {expected_gcp}")
lanes = pool_lanes(report)
if lanes != {"workspace-lib": POOL_LABEL}:
    fail(f"owner dispatch on main: pool lanes {lanes}, expected the example lane on {POOL_LABEL}")
else:
    ok("owner workflow_dispatch on main with the variable set: up, the example lane on this run's label, down")
report = plan(github_ctx(), ENABLED, {"backend": "auto"}, up_outcome="hosted")
if pool_lanes(report) or report["runs_on"].get("workspace-lib") != "ubuntu-24.04":
    fail(f"a pool that did not come up still routes lanes to it: {report['runs_on']}")
else:
    ok("a pool that does not come up (auto) leaves every lane on GitHub-hosted runners, decided before queueing")

# 1c. ineligible triggers with the variable set: nothing in GCP, lanes hosted.
_s1 = len(FAILURES)
for name, github, inputs in (
    ("backend hosted", github_ctx(), {"backend": "hosted"}),
    ("owner dispatch on a branch", github_ctx(ref="refs/heads/feature"), {"backend": "auto"}),
    ("owner dispatch on a tag", github_ctx(ref="refs/tags/v9.9.9"), {"backend": "fast"}),
    ("collaborator dispatch on main", github_ctx(actor="collaborator"), {"backend": "auto"}),
    ("collaborator re-run of an owner dispatch", github_ctx(triggering_actor="collaborator"), {"backend": "auto"}),
    ("owner push to main", github_ctx(event="push"), {"backend": "auto"}),
    ("fork pull request", github_ctx(event="pull_request", ref="refs/pull/7/merge", actor="attacker"), {"backend": "fast"}),
    ("same-repository owner pull request", github_ctx(event="pull_request", ref="refs/pull/8/merge"), {"backend": "fast"}),
    ("pull_request_target", github_ctx(event="pull_request_target", actor="attacker"), {"backend": "fast"}),
    ("another repository", github_ctx(repository="attacker/meerkat", actor="attacker"), {"backend": "fast"}),
):
    report = plan(github, ENABLED, inputs)
    if report["gcp_jobs"] or pool_lanes(report):
        fail(f"{name}: GCP jobs {report['gcp_jobs']}, pool lanes {pool_lanes(report)}")
ok_since(_s1, "with the variable set, only an owner dispatch on main plans GCP jobs: forks, same-repo PRs, pushes, branches, "
   "tags, collaborators, collaborator re-runs and other repositories never do")

# 1d. a re-run attempt the pool was not made for runs GitHub-hosted.
report = plan(github_ctx(attempt="2"), ENABLED, {"backend": "auto"})
report_lane = report["runs_on"].get("workspace-lib")
github2 = github_ctx(attempt="2")
stale = {"fast-up": "success", "fast-up.outputs": {"backend": "fast", "attempt": "1",
                                                   "runs_on": json.dumps({"x64-xl": POOL_LABEL})}}
lane_runs_on = interpolate(key_value(ULTRA_JOBS["workspace-lib"], "runs-on"))
resolved = expr(lane_runs_on, Context(github2, ENABLED, {"backend": "auto"}, ["fast-up"], stale))
if resolved != "ubuntu-24.04":
    fail(f"a re-run with the previous attempt's pool outputs resolves to {resolved!r}")
else:
    ok("re-running failed jobs (attempt 2 with attempt 1's pool outputs) resolves the lane to GitHub-hosted")

# 1e. runners-down always runs.
_s2 = len(FAILURES)
for up_outcome, lane_outcome, cancelled in (
    ("fast", "success", False), ("fast", "failure", False), ("fast", "cancelled", False),
    ("failure", "skipped", False), ("cancelled", "skipped", False), ("hosted", "success", False),
    ("fast", "cancelled", True),
):
    report = plan(github_ctx(), ENABLED, {"backend": "fast" if up_outcome == "failure" else "auto"},
                  up_outcome=up_outcome, lane_outcome=lane_outcome, run_cancelled=cancelled)
    if "fast-down/runners-down" not in report["gcp_jobs"]:
        fail(f"runners-down not planned after up={up_outcome}, lane={lane_outcome}, run cancelled={cancelled}")
ok_since(_s2, "runners-down is planned after a successful, failed, cancelled or fallen-back up, a failed or cancelled lane "
   "and a cancelled run")

# 1f. static wiring.
_s3 = len(FAILURES)
def triggers(top):
    return [re.match(r"^  ([A-Za-z_]+):", line).group(1) for line in top["on"] if re.match(r"^  [A-Za-z_]+:", line)]


if triggers(ULTRA_TOP) != ["workflow_dispatch"]:
    fail(f"ultrafast.yml triggers {triggers(ULTRA_TOP)}; only workflow_dispatch is allowed")
if triggers(FAST_TOP) != ["workflow_call"]:
    fail(f"fast-runners.yml triggers {triggers(FAST_TOP)}; only workflow_call is allowed")
for name in sorted(os.listdir(WORKFLOWS)):
    if name in ("ultrafast.yml", "fast-runners.yml") or not name.endswith((".yml", ".yaml")):
        continue
    with open(os.path.join(WORKFLOWS, name), encoding="utf-8") as handle:
        text = handle.read()
    for needle in ("fast-runners.yml", "meerkat-fast-", "ultrafast", "MEERKAT_FAST_RUNNERS"):
        if needle in text:
            fail(f"{name} references {needle!r}; only ultrafast.yml may use the pool")
for name, lines in (("ultrafast.yml", ULTRA_TOP), ("fast-runners.yml", FAST_TOP)):
    with open(os.path.join(WORKFLOWS, name), encoding="utf-8") as handle:
        text = handle.read()
    if "pull_request" in text.replace("pull requests", "").replace("pull-request", ""):
        fail(f"{name} mentions pull_request")
    if name == "ultrafast.yml" and any("self-hosted" in line for line in text.splitlines()
                                       if line.strip().startswith("runs-on:")):
        fail("ultrafast.yml must target the unique run label, never `self-hosted`")

pool_jobs = [job for job in ULTRA_ORDER if "needs.fast-up.outputs.runs_on" in (key_value(ULTRA_JOBS[job], "runs-on") or "")]
down_needs = needs_of(ULTRA_JOBS["fast-down"])
down_if = strip_expr(key_value(ULTRA_JOBS["fast-down"], "if") or "")
if not pool_jobs:
    fail("ultrafast.yml has no pool lane")
for job in pool_jobs:
    lines = ULTRA_JOBS[job]
    if "fast-up" not in needs_of(lines):
        fail(f"pool lane {job} does not need fast-up")
    if job not in down_needs:
        fail(f"fast-down does not need pool lane {job}, so it could delete the VM under it")
    text = "\n".join(lines)
    if "secrets." in text or "id-token" in text:
        fail(f"pool lane {job} gets secrets or an id-token")
    if "github.run_attempt" not in (key_value(lines, "runs-on") or ""):
        fail(f"pool lane {job} does not pin the attempt the pool was made for")
if down_if != "always()":
    fail(f"fast-down must run under always(); if: {down_if!r}")
if "fast-up" not in down_needs:
    fail("fast-down must need fast-up")
up_with = mapping(key_value(ULTRA_JOBS["fast-up"], "with"), 4)
down_with = mapping(key_value(ULTRA_JOBS["fast-down"], "with"), 4)
if up_with.get("runners") != down_with.get("runners"):
    fail("fast-up and fast-down must pass the same runners spec (down deletes by its names)")
lanes_per_class = {}
for job in pool_jobs:
    match = re.search(r"\['([a-z0-9-]+)'\]", key_value(ULTRA_JOBS[job], "runs-on") or "")
    if not match:
        fail(f"pool lane {job} does not name its runner class")
        continue
    lanes_per_class[match.group(1)] = lanes_per_class.get(match.group(1), 0) + 1
spec_counts = {entry["class"]: entry.get("count", 1) for entry in json.loads(up_with.get("runners") or "[]")}
if lanes_per_class != spec_counts:
    fail(f"each JIT runner takes one job: pool lanes per class {lanes_per_class} must equal the runner spec {spec_counts}")
if down_with.get("up_backend") != "${{ needs.fast-up.outputs.backend }}":
    fail("fast-down must pass up's reported backend")
gate_if = strip_expr(key_value(ULTRA_JOBS["gate"], "if") or "")
if gate_if != "always()" or "fast-down" not in needs_of(ULTRA_JOBS["gate"]):
    fail("the ultrafast gate must run under always() after fast-down")

REQUIRED_GUARDS = (
    f"vars.{GATE_VAR} == 'true'",
    "github.event_name == 'workflow_dispatch'",
    "github.ref == 'refs/heads/main'",
    "github.repository_owner == 'lukacf'",
    "github.actor == 'lukacf'",
    "github.triggering_actor == 'lukacf'",
    "inputs.backend != 'hosted'",
)
guards = {}
for job in ("runners-up", "runners-down"):
    condition = strip_expr(key_value(FAST_JOBS[job], "if") or "")
    conjuncts = [part.strip() for part in condition.split("&&")]
    for guard in REQUIRED_GUARDS:
        if guard not in conjuncts:
            fail(f"fast-runners.yml {job} lacks the conjunct {guard!r}")
    if re.search(r"\b(always|success|failure|cancelled)\s*\(", condition):
        fail(f"fast-runners.yml {job} uses a status function")
    guards[job] = [part for part in conjuncts if not part.startswith("${{ inputs.action") and "inputs.action" not in part]
if guards.get("runners-up") != guards.get("runners-down"):
    fail("runners-up and runners-down must share one eligibility guard")
fast_text = "\n".join(FAST_JOBS["runners-up"] + FAST_JOBS["runners-down"])
for needle in ("repository: lukacf/meerkat", "ref: main", "run: scripts/fast-runners up", "run: scripts/fast-runners down",
               "run: scripts/fast-runners fallback", "vars.MEERKAT_FAST_RUNNERS_WIF_PROVIDER"):
    if needle not in fast_text:
        fail(f"fast-runners.yml lacks {needle!r}")
if "secrets." in fast_text:
    fail("fast-runners.yml must not read repository secrets (the App key lives in Secret Manager behind WIF)")
for job in ("runners-up", "runners-down"):
    if key_value(FAST_JOBS[job], "environment") != "meerkat-fast-runners":
        fail(f"fast-runners.yml {job} must use the main-only meerkat-fast-runners environment")
ok_since(_s3, "static wiring: dispatch-only caller, call-only launcher, no other workflow reaches the pool, down needs every "
   "pool lane under always(), identical guards and runner specs for up and down")


# --- 2. the job-started hook --------------------------------------------------------------------

POLICY = """OWNER=lukacf
OWNER_ID=16124748
REPOSITORIES=lukacf/meerkat lukacf/meerkat-meta
WORKFLOWS=lukacf/meerkat/.github/workflows/ultrafast.yml@refs/heads/main lukacf/meerkat-meta/.github/workflows/ultrafast.yml@refs/heads/main
RUN_ID=9001
RUN_ATTEMPT=1
"""


def hook_env(tmp, overrides=None, payload_overrides=None, drop_payload=()):
    payload = {
        "ref": "refs/heads/main",
        "workflow": ".github/workflows/ultrafast.yml",
        "inputs": {"backend": "auto"},
        "repository": {"full_name": CORE},
        "sender": {"login": OWNER, "id": 16124748},
    }
    for key, value in (payload_overrides or {}).items():
        payload[key] = value
    for key in drop_payload:
        payload.pop(key, None)
    event_path = os.path.join(tmp, "event.json")
    with open(event_path, "w", encoding="utf-8") as handle:
        json.dump(payload, handle)
    values = {
        "PATH": os.environ["PATH"],
        "MEERKAT_FAST_RUNNER_POLICY": os.path.join(tmp, "policy.env"),
        "GITHUB_EVENT_NAME": "workflow_dispatch",
        "GITHUB_REF": "refs/heads/main",
        "GITHUB_ACTOR": OWNER,
        "GITHUB_TRIGGERING_ACTOR": OWNER,
        "GITHUB_REPOSITORY": CORE,
        "GITHUB_REPOSITORY_OWNER": OWNER,
        "GITHUB_WORKFLOW_REF": ULTRAFAST_REF,
        "GITHUB_RUN_ID": "9001",
        "GITHUB_RUN_ATTEMPT": "1",
        "GITHUB_EVENT_PATH": event_path,
        "RUNNER_ENVIRONMENT": "self-hosted",
    }
    values.update(overrides or {})
    return values


def run_hook(tmp, **kwargs):
    proc = subprocess.run(["bash", HOOK], env=hook_env(tmp, **kwargs), capture_output=True, text=True)
    return proc.returncode, proc.stdout + proc.stderr


with tempfile.TemporaryDirectory(prefix="fast-runners-hook.") as tmp:
    with open(os.path.join(tmp, "policy.env"), "w", encoding="utf-8") as handle:
        handle.write(POLICY)
    code, output = run_hook(tmp)
    if code != 0:
        fail(f"the hook refuses the pinned owner dispatch on main:\n{output}")
    else:
        ok("the hook admits the pinned run's owner dispatch of ultrafast.yml on main")
    fork_pr = {"pull_request": {"head": {"repo": {"full_name": "attacker/meerkat"}}, "number": 7}}
    refusals = (
        ("a fork pull request", dict(overrides={"GITHUB_EVENT_NAME": "pull_request", "GITHUB_REF": "refs/pull/7/merge",
                                               "GITHUB_ACTOR": "attacker", "GITHUB_TRIGGERING_ACTOR": "attacker"},
                                     payload_overrides=dict(fork_pr, sender={"login": "attacker", "id": 1}))),
        ("a same-repository owner pull request", dict(overrides={"GITHUB_EVENT_NAME": "pull_request",
                                                                 "GITHUB_REF": "refs/pull/8/merge"},
                                                      payload_overrides={"pull_request": {"number": 8}})),
        ("pull_request_target", dict(overrides={"GITHUB_EVENT_NAME": "pull_request_target"})),
        ("an owner push to main", dict(overrides={"GITHUB_EVENT_NAME": "push"})),
        ("a dispatch on a branch", dict(overrides={"GITHUB_REF": "refs/heads/feature"},
                                        payload_overrides={"ref": "refs/heads/feature"})),
        ("a collaborator dispatch", dict(overrides={"GITHUB_ACTOR": "collaborator", "GITHUB_TRIGGERING_ACTOR": "collaborator"},
                                         payload_overrides={"sender": {"login": "collaborator", "id": 2}})),
        ("a collaborator re-run", dict(overrides={"GITHUB_TRIGGERING_ACTOR": "collaborator", "GITHUB_RUN_ATTEMPT": "1"})),
        ("another workflow file", dict(overrides={"GITHUB_WORKFLOW_REF": f"{CORE}/.github/workflows/ci.yml@refs/heads/main"},
                                       payload_overrides={"workflow": ".github/workflows/ci.yml"})),
        ("the allowlisted file at another ref", dict(overrides={"GITHUB_WORKFLOW_REF": f"{CORE}/.github/workflows/ultrafast.yml@refs/heads/feature"})),
        ("another repository", dict(overrides={"GITHUB_REPOSITORY": "attacker/meerkat", "GITHUB_REPOSITORY_OWNER": "attacker"})),
        ("Meta's workflow in Core's run", dict(overrides={"GITHUB_WORKFLOW_REF": "lukacf/meerkat-meta/.github/workflows/ultrafast.yml@refs/heads/main"})),
        ("another run", dict(overrides={"GITHUB_RUN_ID": "9002"})),
        ("another attempt of the run", dict(overrides={"GITHUB_RUN_ATTEMPT": "2"})),
        ("a payload that is a pull request behind dispatch variables", dict(payload_overrides=fork_pr)),
        ("a payload from another sender", dict(payload_overrides={"sender": {"login": "attacker", "id": 1}})),
        ("a payload sender id that is not the owner's", dict(payload_overrides={"sender": {"login": OWNER, "id": 1}})),
        ("a payload for another repository", dict(payload_overrides={"repository": {"full_name": "attacker/meerkat"}})),
        ("a payload for another workflow", dict(payload_overrides={"workflow": ".github/workflows/ci.yml"})),
        ("a payload without a workflow", dict(drop_payload=("workflow",))),
        ("a missing event payload", dict(overrides={"GITHUB_EVENT_PATH": os.path.join(tmp, "missing.json")})),
    )
    for name, kwargs in refusals:
        code, output = run_hook(tmp, **kwargs)
        if code == 0 or "Fast runner refused this job" not in output:
            fail(f"the hook admits {name}:\n{output}")
    ok(f"the hook refuses, before any step, {len(refusals)} non-owner/non-dispatch/non-main/other-run cases "
       "including fork and same-repository pull requests")
    for name, policy in (
        ("no policy file", None),
        ("a policy without a pinned run or PERSISTENT=1", POLICY.replace("RUN_ID=9001\n", "").replace("RUN_ATTEMPT=1\n", "")),
        ("a policy without workflows", POLICY.replace("WORKFLOWS=", "#")),
    ):
        path = os.path.join(tmp, "policy.env")
        if os.path.exists(path):
            os.unlink(path)
        if policy is not None:
            with open(path, "w", encoding="utf-8") as handle:
                handle.write(policy)
        code, output = run_hook(tmp)
        if code == 0:
            fail(f"the hook admits a job with {name}")
    with open(os.path.join(tmp, "policy.env"), "w", encoding="utf-8") as handle:
        handle.write(POLICY.replace("RUN_ID=9001\nRUN_ATTEMPT=1\n", "PERSISTENT=1\n"))
    code, output = run_hook(tmp, overrides={"GITHUB_RUN_ID": "12345"})
    code_pr, _ = run_hook(tmp, overrides={"GITHUB_EVENT_NAME": "pull_request"}, payload_overrides=fork_pr)
    if code != 0 or code_pr == 0:
        fail(f"a persistent-runner policy must admit any owner dispatch run on main and still refuse pull requests: {output}")
    ok("the hook fails closed without a policy, without a run pin, or without workflows; a PERSISTENT=1 policy "
       "(the owner's Mac) admits any owner dispatch on main and still refuses pull requests")


# --- 3. scripts/fast-runners against recording fakes --------------------------------------------

FAKE_GCLOUD = r'''#!/usr/bin/env python3
import hashlib, json, os, sys
state_dir = os.environ["FAKE_STATE"]
argv = sys.argv[1:]
record = {"tool": "gcloud", "argv": argv}
stdin = b""
if "--plaintext-file" in argv and argv[argv.index("--plaintext-file") + 1] == "-":
    stdin = sys.stdin.buffer.read()
    record["stdin"] = stdin.decode()
for item in argv:
    pass
if "--metadata-from-file" in argv:
    files = {}
    for pair in argv[argv.index("--metadata-from-file") + 1].split(","):
        key, _, path = pair.partition("=")
        with open(path, encoding="utf-8") as handle:
            files[key] = handle.read()
    record["files"] = files
with open(os.path.join(state_dir, "calls.jsonl"), "a", encoding="utf-8") as handle:
    handle.write(json.dumps(record) + "\n")
with open(os.path.join(state_dir, "state.json"), encoding="utf-8") as handle:
    state = json.load(handle)
words = [a for a in argv if not a.startswith("--")]
def save():
    with open(os.path.join(state_dir, "state.json"), "w", encoding="utf-8") as handle:
        json.dump(state, handle)
if argv[:4] == ["secrets", "versions", "access", "latest"]:
    sys.stdout.write(open(state["app_key_path"], encoding="utf-8").read())
    sys.exit(0)
if argv[:2] == ["kms", "encrypt"]:
    sys.stdout.buffer.write(b"KMS(" + hashlib.sha256(stdin).hexdigest().encode() + b")")
    sys.exit(0)
if argv[:3] == ["compute", "instances", "create"]:
    name = argv[3]
    if name in state.get("fail_create", []):
        sys.stderr.write("ERROR: (gcloud.compute.instances.create) Quota 'C4D_CPUS' exceeded.\n")
        sys.exit(1)
    state.setdefault("instances", []).append(name)
    save()
    sys.exit(0)
if argv[:3] == ["compute", "instances", "describe"]:
    name = argv[3]
    late = state.setdefault("late_instances", {})
    if name in late and late[name] > 0:
        late[name] -= 1
        save()
        sys.stderr.write(f"ERROR: The resource 'projects/p/zones/z/instances/{name}' was not found\n")
        sys.exit(1)
    if name in late:
        state.setdefault("instances", []).append(name)
        del late[name]
        save()
    if name in state.get("instances", []):
        print(name)
        sys.exit(0)
    sys.stderr.write(f"ERROR: The resource 'projects/p/zones/z/instances/{name}' was not found\n")
    sys.exit(1)
if argv[:3] == ["compute", "instances", "delete"]:
    names = [a for a in argv[3:] if not a.startswith("--") and a not in (state.get("project"), state.get("zone"), "all")]
    if state.get("fail_delete"):
        sys.stderr.write("ERROR: (gcloud.compute.instances.delete) backend error\n")
        sys.exit(1)
    state["instances"] = [n for n in state.get("instances", []) if n not in argv]
    save()
    sys.exit(0)
sys.stderr.write(f"fake gcloud: unexpected {argv}\n")
sys.exit(2)
'''

FAKE_CURL = r'''#!/usr/bin/env python3
import json, os, re, stat, sys
state_dir = os.environ["FAKE_STATE"]
argv = sys.argv[1:]
method, url, headers, body, header_files = "GET", None, [], None, []
i = 0
while i < len(argv):
    a = argv[i]
    if a == "-X":
        method = argv[i + 1]; i += 2; continue
    if a == "-H":
        h = argv[i + 1]
        if h.startswith("@"):
            mode = stat.S_IMODE(os.stat(h[1:]).st_mode)
            header_files.append(mode)
            headers += open(h[1:], encoding="utf-8").read().splitlines()
        else:
            headers.append(h)
        i += 2; continue
    if a == "--data-binary":
        body = json.loads(sys.stdin.read()) if argv[i + 1] == "@-" else None; i += 2; continue
    if a in ("-w",):
        i += 2; continue
    if a.startswith("-"):
        i += 1; continue
    url = a; i += 1
auth = [h for h in headers if h.lower().startswith("authorization:")]
record = {"tool": "curl", "argv": argv, "method": method, "url": url, "body": body,
          "header_file_modes": header_files, "auth_scheme": auth[0].split()[1] if auth else None}
with open(os.path.join(state_dir, "calls.jsonl"), "a", encoding="utf-8") as handle:
    handle.write(json.dumps(record) + "\n")
with open(os.path.join(state_dir, "state.json"), encoding="utf-8") as handle:
    state = json.load(handle)
def save():
    with open(os.path.join(state_dir, "state.json"), "w", encoding="utf-8") as handle:
        json.dump(state, handle)
def reply(obj, status=200):
    sys.stdout.write((json.dumps(obj) if obj is not None else "") + "\n" + str(status))
    sys.exit(0)
path = url.replace("https://api.github.com", "")
m = re.fullmatch(r"/repos/([^/]+/[^/]+)/installation", path)
if m and method == "GET":
    reply({"id": 4242})
if path == "/app/installations/4242/access_tokens" and method == "POST":
    reply({"token": "ghs_FAKEINSTALLATIONTOKEN"}, 201)
m = re.fullmatch(r"/repos/([^/]+/[^/]+)/actions/runners/generate-jitconfig", path)
if m and method == "POST":
    rid = 100 + len(state.setdefault("runners", []))
    state["runners"].append({"id": rid, "name": body["name"], "status": state.get("runner_status", "online"),
                             "labels": [{"name": label} for label in body["labels"]]})
    save()
    reply({"runner": {"id": rid, "name": body["name"]}, "encoded_jit_config": "JITCONFIG-SECRET-" + body["name"]}, 201)
m = re.fullmatch(r"/repos/([^/]+/[^/]+)/actions/runners\?per_page=100", path)
if m and method == "GET":
    reply({"total_count": len(state.get("runners", [])), "runners": state.get("runners", [])})
m = re.fullmatch(r"/repos/([^/]+/[^/]+)/actions/runners/(\d+)", path)
if m and method == "DELETE":
    state["runners"] = [r for r in state.get("runners", []) if str(r["id"]) != m.group(2)]
    save()
    reply(None, 204)
reply({"message": "Not Found"}, 404)
'''


def setup_fakes(tmp, state):
    bin_dir = os.path.join(tmp, "bin")
    os.makedirs(bin_dir, exist_ok=True)
    for name, body in (("gcloud", FAKE_GCLOUD), ("curl", FAKE_CURL)):
        path = os.path.join(bin_dir, name)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(body)
        os.chmod(path, 0o755)
    key_path = os.path.join(tmp, "app-key.pem")
    if not os.path.exists(key_path):
        subprocess.run(["openssl", "genrsa", "-out", key_path, "2048"], check=True, capture_output=True)
    state = dict(state, app_key_path=key_path, project="king-dnn-training-dev", zone="europe-west4-a")
    with open(os.path.join(tmp, "state.json"), "w", encoding="utf-8") as handle:
        json.dump(state, handle)
    open(os.path.join(tmp, "calls.jsonl"), "w").close()
    return bin_dir


def run_launcher(tmp, command, extra_env=None, state=None):
    bin_dir = setup_fakes(tmp, state or {})
    output = os.path.join(tmp, "github_output")
    open(output, "w").close()
    runner_temp = os.path.join(tmp, "runner_temp")
    os.makedirs(runner_temp, exist_ok=True)
    values = {
        "PATH": bin_dir + os.pathsep + os.environ["PATH"],
        "FAKE_STATE": tmp,
        "GITHUB_OUTPUT": output,
        "RUNNER_TEMP": runner_temp,
        "MEERKAT_FAST_RUNNERS_ENABLED": "true",
        "MEERKAT_FAST_RUNNERS_APP_ID": "123456",
        "FAST_RUNNERS_BACKEND": "auto",
        "FAST_RUNNERS_SPEC": '[{"class":"x64-xl","count":1}]',
        "MEERKAT_FAST_RUNNERS_ONLINE_TIMEOUT": "2",
        "MEERKAT_FAST_RUNNERS_ONLINE_POLL": "0.2",
        "MEERKAT_FAST_RUNNERS_DOWN_SETTLE": "1",
        "GITHUB_EVENT_NAME": "workflow_dispatch",
        "GITHUB_REF": "refs/heads/main",
        "GITHUB_REPOSITORY": CORE,
        "GITHUB_REPOSITORY_OWNER": OWNER,
        "GITHUB_ACTOR": OWNER,
        "GITHUB_TRIGGERING_ACTOR": OWNER,
        "GITHUB_WORKFLOW_REF": ULTRAFAST_REF,
        "GITHUB_RUN_ID": "9001",
        "GITHUB_RUN_ATTEMPT": "1",
        "RUNNER_ENVIRONMENT": "github-hosted",
    }
    values.update(extra_env or {})
    proc = subprocess.run([sys.executable, LAUNCHER_SCRIPT, command], env=values, capture_output=True, text=True)
    with open(os.path.join(tmp, "calls.jsonl"), encoding="utf-8") as handle:
        calls = [json.loads(line) for line in handle if line.strip()]
    with open(output, encoding="utf-8") as handle:
        outputs = dict(line.split("=", 1) for line in handle.read().splitlines() if "=" in line)
    with open(os.path.join(tmp, "state.json"), encoding="utf-8") as handle:
        final_state = json.load(handle)
    return proc, calls, outputs, final_state


def scenario(command, extra_env=None, state=None):
    with tempfile.TemporaryDirectory(prefix="fast-runners-launcher.") as tmp:
        return run_launcher(tmp, command, extra_env, state)


def gcloud_calls(calls, *words):
    return [c for c in calls if c["tool"] == "gcloud" and c["argv"][:len(words)] == list(words)]


def curl_calls(calls, method=None, suffix=None):
    return [c for c in calls if c["tool"] == "curl" and (method is None or c["method"] == method)
            and (suffix is None or c["url"].endswith(suffix))]


# 3a. variable unset or backend hosted or ineligible: no call at all.
_s4 = len(FAILURES)
for name, extra in (
    ("the variable unset", {"MEERKAT_FAST_RUNNERS_ENABLED": ""}),
    ("backend hosted", {"FAST_RUNNERS_BACKEND": "hosted"}),
    ("a fork pull request", {"GITHUB_EVENT_NAME": "pull_request", "GITHUB_REF": "refs/pull/7/merge",
                             "GITHUB_ACTOR": "attacker", "GITHUB_TRIGGERING_ACTOR": "attacker"}),
    ("a collaborator re-run", {"GITHUB_TRIGGERING_ACTOR": "collaborator"}),
    ("an owner push to main", {"GITHUB_EVENT_NAME": "push"}),
    ("an owner dispatch on a branch", {"GITHUB_REF": "refs/heads/feature"}),
    ("a collaborator actor", {"GITHUB_ACTOR": "collaborator"}),
    ("another repository", {"GITHUB_REPOSITORY": "lukacf/other"}),
    ("another repository owner", {"GITHUB_REPOSITORY_OWNER": "attacker"}),
    ("a self-hosted launcher", {"RUNNER_ENVIRONMENT": "self-hosted"}),
    ("ci.yml as the caller", {"GITHUB_WORKFLOW_REF": f"{CORE}/.github/workflows/ci.yml@refs/heads/main"}),
):
    for command in ("up", "down"):
        proc, calls, outputs, _ = scenario(command, extra)
        if calls:
            fail(f"{command} with {name} called {[c['tool'] + ' ' + ' '.join(c['argv'][:3]) for c in calls]}")
        if command == "up" and (proc.returncode != 0 or outputs.get("backend") != "hosted" or outputs.get("runs_on") != "{}"):
            fail(f"up with {name} (auto) must select GitHub-hosted runners: rc={proc.returncode} {outputs} {proc.stdout[-300:]}")
proc, calls, outputs, _ = scenario("up", {"FAST_RUNNERS_BACKEND": "fast", "GITHUB_EVENT_NAME": "pull_request",
                                          "GITHUB_ACTOR": "attacker", "GITHUB_TRIGGERING_ACTOR": "attacker"})
if proc.returncode == 0 or calls:
    fail("up with backend fast for a pull request must fail without calling anything")
ok_since(_s4, "up and down call nothing (no GitHub registration, no GCP) while the variable is unset, for backend hosted, "
   "a fork pull request, a collaborator re-run, a self-hosted launcher or another caller workflow")

# 3b. the eligible up.
_s5 = len(FAILURES)
proc, calls, outputs, final_state = scenario("up")
transcript = proc.stdout + proc.stderr
if proc.returncode != 0 or outputs.get("backend") != "fast":
    fail(f"eligible up failed: rc={proc.returncode}\n{transcript[-800:]}")
else:
    runs_on = json.loads(outputs["runs_on"])
    label = runs_on.get("x64-xl", "")
    if not re.fullmatch(r"meerkat-fast-9001-1-[0-9a-f]{16}-x64-xl", label) or set(runs_on) != {"x64-xl"}:
        fail(f"runs_on {runs_on} is not one unique run/attempt/nonce label per class")
    registrations = curl_calls(calls, "POST", "/actions/runners/generate-jitconfig")
    if len(registrations) != 1:
        fail(f"{len(registrations)} JIT registrations for one runner")
    else:
        body = registrations[0]["body"]
        if body.get("labels") != [label]:
            fail(f"the JIT runner labels are {body.get('labels')}, expected exactly [{label!r}] (no self-hosted)")
        if body.get("runner_group_id") != 1 or body.get("name") != "meerkat-fast-9001-1-0":
            fail(f"unexpected registration body {body}")
    tokens = curl_calls(calls, "POST", "/app/installations/4242/access_tokens")
    if not tokens or tokens[0]["body"] != {"repositories": ["meerkat"], "permissions": {"administration": "write"}}:
        fail(f"the installation token is not limited to this repository and Administration: write: {tokens}")
    for call in curl_calls(calls):
        if any("ghs_" in arg or "Bearer" in arg for arg in call["argv"]):
            fail("a GitHub token travelled in curl's argv")
        if call["header_file_modes"] != [0o600]:
            fail(f"curl header file mode {call['header_file_modes']}, expected 0600")
        if call["auth_scheme"] != "Bearer":
            fail("a GitHub call without a bearer token")
    creates = gcloud_calls(calls, "compute", "instances", "create")
    if len(creates) != 1:
        fail(f"{len(creates)} VM creations for one runner")
    else:
        argv = creates[0]["argv"]
        joined = " ".join(argv)
        for needle in ("--network-interface=subnet=meerkat-fast-runners-europe-west4,no-address",
                       "--max-run-duration 120m", "--instance-termination-action DELETE",
                       "--provisioning-model STANDARD", "--no-restart-on-failure", "--shielded-secure-boot",
                       "--service-account meerkat-fast-runner@king-dnn-training-dev.iam.gserviceaccount.com",
                       "--machine-type c4d-standard-192", "block-project-ssh-keys=TRUE", "enable-oslogin=TRUE",
                       "meerkat-fast-run-id=9001", "meerkat-fast-run-attempt=1", "purpose=meerkat-fast-runner"):
            if needle not in joined:
                fail(f"VM creation lacks {needle!r}")
        for forbidden in ("--tags", "--address", "--network-tier", "--can-ip-forward", "--scopes default"):
            if forbidden in joined:
                fail(f"VM creation has {forbidden!r}")
        files = creates[0].get("files", {})
        startup = subprocess.run([sys.executable, LAUNCHER_SCRIPT, "render-startup"], capture_output=True, text=True).stdout
        with open(HOOK, encoding="utf-8") as handle:
            hook_text = handle.read()
        if files.get("startup-script") != startup:
            fail("the VM startup script is not the rendered one")
        if files.get("meerkat-fast-hook") != hook_text:
            fail("the VM hook is not scripts/fast-runners-job-started-hook")
        expected_ct = base64.b64encode(b"KMS(" + hashlib.sha256(b"JITCONFIG-SECRET-meerkat-fast-9001-1-0").hexdigest().encode() + b")").decode()
        if files.get("meerkat-fast-jit-config-kms") != expected_ct:
            fail("the VM metadata does not carry the KMS ciphertext of its own registration")
    encrypts = gcloud_calls(calls, "kms", "encrypt")
    if [c.get("stdin") for c in encrypts] != ["JITCONFIG-SECRET-meerkat-fast-9001-1-0"]:
        fail("the registration must reach KMS on stdin, exactly once")
    leaks = []
    for call in calls:
        surfaces = [" ".join(call["argv"])] + list(call.get("files", {}).values())
        if call["tool"] == "curl":
            surfaces.append(json.dumps(call.get("body")))
        if any("JITCONFIG-SECRET" in text for text in surfaces):
            leaks.append(" ".join(call["argv"][:3]))
    if leaks:
        fail(f"the plaintext registration leaked into {leaks}")
    if re.search(r"JITCONFIG-SECRET|ghs_FAKE", transcript.replace("::add-mask::JITCONFIG-SECRET-meerkat-fast-9001-1-0", "")
                 .replace("::add-mask::ghs_FAKEINSTALLATIONTOKEN", "")):
        fail("up printed the registration or the token outside an ::add-mask:: command")
    for value in ("JITCONFIG-SECRET-meerkat-fast-9001-1-0", "ghs_FAKEINSTALLATIONTOKEN", label.split("-")[-3]):
        if f"::add-mask::{value}" not in transcript:
            fail(f"up does not mask {value[:12]}...")
    if any("JITCONFIG" in value for value in outputs.values()):
        fail("the registration reached GITHUB_OUTPUT")
    if final_state.get("instances") != ["meerkat-fast-9001-1-0"]:
        fail(f"unexpected instances after up: {final_state.get('instances')}")
    ok_since(_s5, "eligible up: one installation token limited to the repository and Administration: write (header file, never "
       "argv), one JIT runner with exactly the unique label, its registration only on KMS stdin and as ciphertext in "
       "metadata, an outbound-only shielded VM with the runner identity, the rendered boot script and hook, a "
       "120m GCE TTL that deletes it, and masked secrets")

# 3c. failures in auto clean up and fall back; in fast they fail after cleaning up.
_s6 = len(FAILURES)
for name, state, extra in (
    ("runners that never come online", {"runner_status": "offline"}, {}),
    ("a VM creation failure", {"fail_create": ["meerkat-fast-9001-1-1"]},
     {"FAST_RUNNERS_SPEC": '[{"class":"x64-xl","count":2}]'}),
):
    for backend in ("auto", "fast"):
        proc, calls, outputs, final_state = scenario("up", dict(extra, FAST_RUNNERS_BACKEND=backend), state)
        if final_state.get("instances") or final_state.get("runners"):
            fail(f"{name} ({backend}) leaves instances {final_state.get('instances')} / runners {final_state.get('runners')}")
        if backend == "auto" and (proc.returncode != 0 or outputs.get("backend") != "hosted" or outputs.get("runs_on") != "{}"):
            fail(f"{name} (auto) must clean up and select GitHub-hosted runners: rc={proc.returncode} {outputs}")
        if backend == "fast" and (proc.returncode == 0 or outputs.get("backend") == "fast"):
            fail(f"{name} (fast) must fail")
ok_since(_s6, "runners that never come online or a failed VM creation: up deletes every VM and registration it made, then "
   "selects GitHub-hosted runners (auto) or fails (fast)")

proc, calls, outputs, _ = scenario("up", {"FAST_RUNNERS_SPEC": '[{"class":"x64-xl","count":9}]'})
if proc.returncode == 0 or calls:
    fail("up must refuse more than 8 runners before any call")
proc, calls, outputs, _ = scenario("up", {"FAST_RUNNERS_SPEC": '[{"class":"n2-highcpu-4","count":1}]'})
if proc.returncode == 0 or calls:
    fail("up must refuse an unknown runner class before any call")
proc, calls, outputs, _ = scenario("fallback", {"FAST_RUNNERS_BACKEND": "fast"})
if proc.returncode == 0 or outputs.get("backend") != "hosted":
    fail("fallback for backend fast must fail")
ok("up refuses oversized or unknown runner specs before any call; fallback fails for backend fast")

# 3d. down.
_s7 = len(FAILURES)
state = {"instances": ["meerkat-fast-9001-1-0", "meerkat-fast-9001-1-1", "meerkat-fast-9000-1-0"],
         "runners": [{"id": 7, "name": "meerkat-fast-9001-1-0", "status": "offline"},
                     {"id": 8, "name": "meerkat-fast-9001-1-1", "status": "online"},
                     {"id": 9, "name": "someone-else", "status": "online"}]}
proc, calls, outputs, final_state = scenario("down", {"FAST_RUNNERS_SPEC": '[{"class":"x64-xl","count":2}]',
                                                      "FAST_RUNNERS_UP_BACKEND": "fast"}, state)
if proc.returncode != 0:
    fail(f"down failed: {proc.stdout[-500:]}")
if final_state.get("instances") != ["meerkat-fast-9000-1-0"]:
    fail(f"down must delete exactly this run's VMs; left {final_state.get('instances')}")
if [r["name"] for r in final_state.get("runners", [])] != ["someone-else"]:
    fail(f"down must delete exactly this run's registrations; left {final_state.get('runners')}")
if gcloud_calls(calls, "compute", "instances", "list"):
    fail("down must not list the project's instances")
proc, calls, outputs, final_state = scenario("down", {"FAST_RUNNERS_UP_BACKEND": ""},
                                             {"late_instances": {"meerkat-fast-9001-1-0": 1}})
if proc.returncode != 0 or final_state.get("instances"):
    fail(f"down after a cancelled up must find and delete a VM whose insert was still in flight: {final_state}")
proc, calls, outputs, final_state = scenario("down", {"FAST_RUNNERS_SPEC": '[{"class":"x64-xl","count":2}]'},
                                             dict(state, fail_delete=True))
if proc.returncode == 0:
    fail("down must fail when a VM delete fails")
if any(r["name"].startswith("meerkat-fast-9001-1-") for r in final_state.get("runners", [])):
    fail("down must still delete the registrations after a failed VM delete")
ok_since(_s7, "down deletes exactly this run's VMs and registrations by name (no project listing), finds an insert still in "
   "flight after a cancelled up, and keeps going but fails when a delete fails")


# --- 4. WIF condition and approval commands ----------------------------------------------------

_s8 = len(FAILURES)

wif = subprocess.run([sys.executable, LAUNCHER_SCRIPT, "wif"], capture_output=True, text=True).stdout.splitlines()
condition = wif[wif.index("attribute-condition:") + 1].strip()
mapping_lines = [line.strip() for line in wif[1:wif.index("attribute-condition:")]]
role_mapping = next(line.split("=", 1)[1] for line in mapping_lines if line.startswith("attribute.fast_role="))

CEL = re.compile(r"\s*(?:('(?:[^'\\]|\\.)*')|(==|!=|&&|\|\||[?:()\[\],])|(assertion\.[a-z_]+|in\b))")


def cel(text, claims):
    tokens, pos = [], 0
    while pos < len(text):
        match = CEL.match(text, pos)
        if not match or match.end() == pos:
            sys.exit(f"cannot read CEL at {text[pos:]!r}")
        pos = match.end()
        tokens.append(match.group(0).strip())
    tokens = [t for t in tokens if t]
    index = [0]

    def peek():
        return tokens[index[0]] if index[0] < len(tokens) else None

    def take(expected=None):
        token = tokens[index[0]]
        if expected and token != expected:
            sys.exit(f"CEL: expected {expected}, got {token} in {text}")
        index[0] += 1
        return token

    def primary():
        token = take()
        if token == "(":
            value = ternary()
            take(")")
            return value
        if token == "[":
            items = []
            while peek() != "]":
                items.append(primary())
                if peek() == ",":
                    take()
            take("]")
            return items
        if token.startswith("'"):
            return token[1:-1]
        if token.startswith("assertion."):
            key = token.split(".", 1)[1]
            if key not in claims:
                sys.exit(f"CEL reads a claim the sample lacks: {key}")
            return claims[key]
        sys.exit(f"CEL: unexpected {token}")

    def comparison():
        left = primary()
        op = peek()
        if op in ("==", "!="):
            take()
            right = primary()
            return (left == right) if op == "==" else (left != right)
        if op == "in":
            take()
            return left in primary()
        return left

    def conjunction():
        value = comparison()
        while peek() == "&&":
            take()
            right = comparison()
            value = value and right
        return value

    def disjunction():
        value = conjunction()
        while peek() == "||":
            take()
            right = conjunction()
            value = value or right
        return value

    def ternary():
        value = disjunction()
        if peek() == "?":
            take()
            yes = ternary()
            take(":")
            no = ternary()
            return yes if value else no
        return value

    result = ternary()
    if index[0] != len(tokens):
        sys.exit(f"CEL: trailing tokens in {text}")
    return result


BASE = {
    "repository_owner_id": "16124748", "repository_owner": "lukacf", "repository": CORE,
    "event_name": "workflow_dispatch", "ref": "refs/heads/main", "actor_id": "16124748", "actor": "lukacf",
    "workflow_ref": ULTRAFAST_REF, "runner_environment": "github-hosted",
    "job_workflow_ref": "lukacf/meerkat/.github/workflows/fast-runners.yml@refs/heads/main",
}
admitted = (
    ("the Core launcher job", {}),
    ("the Meta launcher job", {"repository": "lukacf/meerkat-meta",
                               "workflow_ref": "lukacf/meerkat-meta/.github/workflows/ultrafast.yml@refs/heads/main"}),
)
rejected = (
    ("a fork pull request", {"event_name": "pull_request", "ref": "refs/pull/7/merge", "actor": "attacker", "actor_id": "1"}),
    ("an owner same-repository pull request", {"event_name": "pull_request", "ref": "refs/pull/8/merge"}),
    ("pull_request_target", {"event_name": "pull_request_target"}),
    ("an owner push to main", {"event_name": "push"}),
    ("an owner dispatch on a branch", {"ref": "refs/heads/feature"}),
    ("a collaborator dispatch", {"actor": "collaborator", "actor_id": "2"}),
    ("a renamed account reusing the login", {"actor_id": "3", "repository_owner_id": "3"}),
    ("another account using the owner's login as actor", {"actor_id": "3"}),
    ("the owner's id with another login", {"actor": "collaborator"}),
    ("a repository owner id that is not the owner's", {"repository_owner_id": "3"}),
    ("a repository owner login that is not the owner's", {"repository_owner": "attacker"}),
    ("ci.yml on main", {"workflow_ref": f"{CORE}/.github/workflows/ci.yml@refs/heads/main"}),
    ("ultrafast.yml on a branch", {"workflow_ref": f"{CORE}/.github/workflows/ultrafast.yml@refs/heads/feature"}),
    ("another repository", {"repository": "lukacf/other"}),
)
for name, change in admitted:
    if cel(condition, dict(BASE, **change)) is not True:
        fail(f"the WIF condition rejects {name}")
for name, change in rejected:
    if cel(condition, dict(BASE, **change)) is not False:
        fail(f"the WIF condition admits {name}")
roles = (
    ("the launcher job", {}, "launcher"),
    ("a GitHub-hosted job outside the launcher", {"job_workflow_ref": ULTRAFAST_REF}, "none"),
    ("the launcher file at a branch", {"job_workflow_ref": "lukacf/meerkat/.github/workflows/fast-runners.yml@refs/heads/feature"}, "none"),
    ("a pool lane", {"runner_environment": "self-hosted", "job_workflow_ref": ULTRAFAST_REF}, "lane"),
)
for name, change, want in roles:
    got = cel(role_mapping, dict(BASE, **change))
    if got != want:
        fail(f"WIF fast_role for {name} is {got!r}, expected {want!r}")
ok_since(_s8, f"the WIF condition admits only the owner's dispatch of an allowlisted ultrafast.yml on main ({len(rejected)} "
   "rejections incl. fork and same-repo PRs, pushes, branches, collaborators, a reused login), and only the "
   "launcher file at main on a GitHub-hosted runner maps to the launcher role")

commands = subprocess.run([sys.executable, LAUNCHER_SCRIPT, "approval-commands"], capture_output=True, text=True).stdout.splitlines()
runnable = [line for line in commands if line.startswith("gcloud ")]
project_bindings = [line for line in runnable if line.startswith("gcloud projects add-iam-policy-binding")]
if len(project_bindings) != 1 or "meerkat-fast-launcher@" not in project_bindings[0] \
        or "--condition=" not in project_bindings[0] or "instances/meerkat-fast-" not in project_bindings[0]:
    fail(f"the only project-level binding must be the launcher's name-prefix-conditioned custom role: {project_bindings}")
for line in runnable:
    for forbidden in ("roles/owner", "roles/editor", "roles/compute.admin", "roles/compute.instanceAdmin",
                      "roles/iam.serviceAccountTokenCreator", "roles/storage.admin", "allUsers", "allAuthenticatedUsers"):
        if forbidden in line:
            fail(f"approval command grants {forbidden}: {line}")
    if "meerkat-fast-runner@" in line and "--member=serviceAccount:meerkat-fast-runner@" in line \
            and "roles/cloudkms.cryptoKeyDecrypter" not in line:
        fail(f"the runner identity gets more than the KMS decrypter: {line}")
firewalls = [line for line in runnable if line.startswith("gcloud compute firewall-rules")]
if len(firewalls) != 1 or "--action=DENY" not in firewalls[0] or "--direction=INGRESS" not in firewalls[0] \
        or "--priority=0" not in firewalls[0]:
    fail(f"the network must have exactly one firewall rule, an explicit priority-0 deny of all ingress: {firewalls}")
if any("--action=ALLOW" in line or "--allow" in line for line in runnable):
    fail("an approval command opens ingress")
role_lines = [line for line in runnable if line.startswith("gcloud iam roles create meerkatFastRunnerLauncher")]
if not role_lines or "compute.zoneOperations.get" not in role_lines[0]:
    fail("the launcher role must let gcloud poll the zone operations of its creates and deletes")
if not project_bindings or "/operations/" not in project_bindings[0]:
    fail("the launcher role condition must cover the zone's operations, or gcloud cannot wait for its inserts")
for needle in ("--enable-private-ip-google-access", "routers nats create", "--attribute-condition=",
               "attribute.fast_role/launcher", "attribute.fast_role/lane", "--public-access-prevention"):
    if not any(needle in line for line in runnable):
        fail(f"approval commands lack {needle!r}")
ok("approval commands: no project-wide role (one name-prefix-conditioned launcher role), the runner identity only "
   "decrypts its registration, one priority-0 deny-all-ingress rule and no allow, NAT egress, a dedicated WIF pool")

if FAILURES:
    print(f"{len(FAILURES)} failure(s)")
    sys.exit(1)
print("PASS: owner-only fast-runner pool contract")
PY
