#!/usr/bin/env bash
# Contract test for the owner-only fast-runner pool. Nothing here reaches
# GitHub or GCP: `gcloud` and `curl` are fakes on PATH that record every
# call, and the GitHub Actions planner below evaluates the workflows' job
# `if:`, `needs` and `runs-on` expressions the way GitHub schedules them,
# including cancellation (a job without a status function does not run in a
# cancelled run).
#
#   1. Workflows (ultrafast.yml calling fast-runners.yml):
#      - nothing starts in GCP while MEERKAT_FAST_RUNNERS_ENABLED is unset,
#        for any event, and every lane resolves to its GitHub-hosted runner;
#      - only an owner workflow_dispatch on refs/heads/main plans the GCP
#        jobs; fork and same-repository pull requests, pushes, branch
#        dispatches, collaborators and collaborator re-runs never do, and no
#        other workflow can reach the launcher or a pool label;
#      - runners-down runs whatever up, the lanes or the run did, cancelled
#        runs included, and needs every pool lane;
#      - every pool lane has its own runner, label and explicit condition;
#      - the gate fails on any skipped or failed job, and when the pool came
#        up, on any pool lane that did not run on its own pool runner;
#      - a re-run attempt the pool was not made for runs GitHub-hosted.
#   2. The job-started hook admits only the pinned run's owner dispatch on
#      main and refuses everything else before any step.
#   3. scripts/fast-runners against the fakes: KMS-signed App JWTs, revoked
#      installation tokens, the review tripwire, one unique label per
#      lane and runner (no `self-hosted`, never masked, outputs never
#      printed), the registration only ever encrypted, outbound-only VMs
#      with a GCE TTL, clean-up on every failure or exception, a delete-only
#      down by deterministic names, paginated listings, the gate's pool
#      check, and a prefix-bound reaper that deletes stale VMs in any state
#      and survives gcloud's comma splitting of its job arguments.
#   4. The rendered WIF condition and approval commands.

set -euo pipefail

REPO_ROOT="$(cd "$(dirname "$0")/.." && pwd)"
exec python3 - "${REPO_ROOT}" <<'PY'
import base64
import datetime
import hashlib
import importlib.machinery
import importlib.util
import json
import os
import re
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
META = "lukacf/meerkat-meta"
CORE_ID = "1139800076"
META_ID = "1171358591"
ULTRAFAST_REF = f"{CORE}/.github/workflows/ultrafast.yml@refs/heads/main"
META_ULTRAFAST_REF = f"{META}/.github/workflows/ultrafast.yml@refs/heads/main"
LAUNCHER_REF = f"{CORE}/.github/workflows/fast-runners.yml@refs/heads/main"
FAILURES = []

_loader = importlib.machinery.SourceFileLoader("fast_runners", LAUNCHER_SCRIPT)
_spec = importlib.util.spec_from_loader("fast_runners", _loader)
LAUNCHER = importlib.util.module_from_spec(_spec)
_loader.exec_module(LAUNCHER)


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
                node = ("index", node, ("lit", name))
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
            return ctx.results.get("__run__") != "cancelled" and all(ctx.results.get(need) == "success" for need in ctx.needs)
        if name == "failure":
            return any(ctx.results.get(need) == "failure" for need in ctx.needs)
        values = [evaluate(arg, ctx) for arg in args]
        if name == "fromJSON":
            return json.loads(values[0])
        if name == "toJSON":
            return json.dumps(values[0])
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

UP_WITH = mapping(key_value(ULTRA_JOBS["fast-up"], "with"), 4)
SPEC = json.loads(UP_WITH.get("runners") or "[]")
POOL_LABELS = {entry["lane"]: f"meerkat-fast-9001-1-0123456789abcdef-{entry['lane']}" for entry in SPEC}


def plan_called(github, variables, inputs, up_outcome, run_cancelled):
    """Plans fast-runners.yml; returns (planned jobs, call result, call outputs)."""
    results = {"__run__": "cancelled" if run_cancelled else "running"}
    planned, jobs_outputs = [], {}
    for job in FAST_ORDER:
        lines = FAST_JOBS[job]
        condition = strip_expr(key_value(lines, "if") or "")
        if needs_of(lines):
            sys.exit(f"fast-runners.yml job {job} has needs; the planner does not model it")
        if run_cancelled and not STATUS.search(condition):
            results[job] = "skipped"  # the implicit success() is false in a cancelled run
            continue
        ctx = Context(github, variables, inputs, [], results)
        runs = truthy(expr(condition, ctx)) if condition else True
        if not runs:
            results[job] = "skipped"
            continue
        planned.append(job)
        if job == "runners-up":
            if up_outcome == "fast":
                jobs_outputs[job] = {"backend": "fast", "runs_on": json.dumps(POOL_LABELS),
                                     "attempt": github["run_attempt"]}
                results[job] = "success"
            elif up_outcome == "hosted":
                jobs_outputs[job] = {"backend": "hosted", "runs_on": "{}", "attempt": github["run_attempt"]}
                results[job] = "success"
            else:
                results[job] = up_outcome
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
            planned, status, outputs = plan_called(github, variables, call_inputs, up_outcome, run_cancelled)
            report["called"][job] = {"inputs": call_inputs, "planned": planned}
            report["gcp_jobs"] += [f"{job}/{inner}" for inner in planned]
            results[job] = status
            results[f"{job}.outputs"] = outputs
            continue
        runs_on = key_value(lines, "runs-on")
        report["runs_on"][job] = expr(interpolate(runs_on), ctx)
        results[job] = "success" if job == "gate" else lane_outcome
    report["results"] = results
    return report


def github_ctx(event="workflow_dispatch", ref="refs/heads/main", actor=OWNER, triggering_actor=None,
               repository=CORE, attempt="1"):
    return {
        "event_name": event, "ref": ref, "actor": actor,
        "triggering_actor": triggering_actor or actor,
        "repository": repository, "repository_owner": repository.split("/")[0],
        "run_attempt": attempt, "run_id": "9001",
    }


def pool_lanes(report):
    return {job: label for job, label in report["runs_on"].items() if str(label).startswith("meerkat-fast-")}


ENABLED = {GATE_VAR: "true"}
POOL_JOBS = [job for job in ULTRA_ORDER if "needs.fast-up.outputs.runs_on" in (key_value(ULTRA_JOBS[job], "runs-on") or "")]
GATE_ENV = mapping(next((key_value(ULTRA_JOBS["gate"], "env", depth=8) for _ in [0]), None), 8)


def gate_env_block():
    lines = ULTRA_JOBS["gate"]
    for idx, line in enumerate(lines):
        if line.strip() == "env:" and indent(line) == 8:
            block = []
            for nxt in lines[idx + 1:]:
                if nxt.strip() and indent(nxt) <= 8:
                    break
                block.append(nxt)
            return mapping(block, 8)
    return {}


GATE_ENV = gate_env_block()


def run_gate(report, inputs, extra_env=None):
    """Runs the gate's verify-lanes step the way the workflow wires it (no API: up was not fast)."""
    results = report["results"]
    needs = needs_of(ULTRA_JOBS["gate"])
    ctx = Context(github_ctx(), ENABLED, inputs, needs, results)
    values = {key: str(expr(interpolate(value), ctx)) for key, value in GATE_ENV.items() if key != "GITHUB_TOKEN"}
    values["FAST_RUNNERS_NEEDS"] = json.dumps({job: {"result": results.get(job, "skipped"),
                                                     "outputs": results.get(f"{job}.outputs", {})} for job in needs})
    values["GITHUB_RUN_ATTEMPT"] = "1"
    values.update(extra_env or {})
    proc = subprocess.run([sys.executable, LAUNCHER_SCRIPT, "verify-lanes"], capture_output=True, text=True,
                          env=dict(os.environ, **values))
    return proc.returncode, proc.stdout + proc.stderr


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
        elif any(label != "ubuntu-24.04" for job, label in report["runs_on"].items() if job in POOL_JOBS):
            fail(f"{GATE_VAR} unset, {name}: a lane is not GitHub-hosted: {report['runs_on']}")
ok_since(_s0, f"{GATE_VAR} unset: no GCP job is planned for any dispatch or backend and every lane is GitHub-hosted")

# 1b. the eligible route.
_s1 = len(FAILURES)
report = plan(github_ctx(), ENABLED, {"backend": "auto"})
expected_gcp = ["fast-up/runners-up", "fast-down/runners-down"]
if report["gcp_jobs"] != expected_gcp:
    fail(f"owner dispatch on main: GCP jobs {report['gcp_jobs']}, expected {expected_gcp}")
if pool_lanes(report) != {job: POOL_LABELS[job] for job in POOL_JOBS}:
    fail(f"owner dispatch on main: pool lanes {pool_lanes(report)}, expected each lane on its own label {POOL_LABELS}")
report = plan(github_ctx(), ENABLED, {"backend": "auto"}, up_outcome="hosted")
if pool_lanes(report) or any(report["runs_on"].get(job) != "ubuntu-24.04" for job in POOL_JOBS):
    fail(f"a pool that did not come up still routes lanes to it: {report['runs_on']}")
ok_since(_s1, "owner workflow_dispatch on main with the variable set: up, each lane on its own pool label, down; "
         "a pool that does not come up (auto) leaves every lane GitHub-hosted, decided before queueing")

# 1c. ineligible triggers with the variable set: nothing in GCP, lanes hosted.
_s2 = len(FAILURES)
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
ok_since(_s2, "with the variable set, only an owner dispatch on main plans GCP jobs: forks, same-repo PRs, pushes, "
         "branches, tags, collaborators, collaborator re-runs and other repositories never do")

# 1d. a re-run attempt the pool was not made for runs GitHub-hosted.
_s3 = len(FAILURES)
github2 = github_ctx(attempt="2")
stale = {"fast-up": "success", "fast-up.outputs": {"backend": "fast", "attempt": "1", "runs_on": json.dumps(POOL_LABELS)}}
for job in POOL_JOBS:
    resolved = expr(interpolate(key_value(ULTRA_JOBS[job], "runs-on")), Context(github2, ENABLED, {"backend": "auto"}, ["fast-up"], stale))
    if resolved != "ubuntu-24.04":
        fail(f"a re-run with the previous attempt's pool outputs resolves {job} to {resolved!r}")
ok_since(_s3, "re-running failed jobs (attempt 2 with attempt 1's pool outputs) resolves every lane to GitHub-hosted")

# 1e. runners-down always runs, cancelled runs included.
_s4 = len(FAILURES)
for up_outcome, lane_outcome, cancelled in (
    ("fast", "success", False), ("fast", "failure", False), ("fast", "cancelled", False),
    ("failure", "skipped", False), ("cancelled", "skipped", False), ("hosted", "success", False),
    ("fast", "cancelled", True), ("cancelled", "skipped", True),
):
    report = plan(github_ctx(), ENABLED, {"backend": "fast" if up_outcome == "failure" else "auto"},
                  up_outcome=up_outcome, lane_outcome=lane_outcome, run_cancelled=cancelled)
    if "fast-down/runners-down" not in report["gcp_jobs"]:
        fail(f"runners-down not planned after up={up_outcome}, lane={lane_outcome}, run cancelled={cancelled}")
report = plan(github_ctx(), ENABLED, {"backend": "auto"}, run_cancelled=True)
if "fast-up/runners-up" in report["gcp_jobs"]:
    fail("runners-up must not start in a cancelled run")
ok_since(_s4, "runners-down is planned after a successful, failed, cancelled or fallen-back up, a failed or cancelled "
         "lane and a cancelled run (inner job under always()); runners-up never starts in a cancelled run")

# 1f. the gate fails on any skipped or failed job, and on fast requested but not delivered.
_s5 = len(FAILURES)
cases = (
    ("all lanes ran (variable unset, hosted)", plan(github_ctx(), {}, {"backend": "auto"}), {"backend": "auto"}, True),
    ("all lanes ran (pool fell back)", plan(github_ctx(), ENABLED, {"backend": "auto"}, up_outcome="hosted"), {"backend": "auto"}, True),
    ("up failed and the lane was skipped", plan(github_ctx(), ENABLED, {"backend": "fast"}, up_outcome="failure"), {"backend": "fast"}, False),
    ("a lane was skipped", plan(github_ctx(), ENABLED, {"backend": "auto"}, up_outcome="hosted", lane_outcome="skipped"), {"backend": "auto"}, False),
    ("a lane failed", plan(github_ctx(), ENABLED, {"backend": "auto"}, up_outcome="hosted", lane_outcome="failure"), {"backend": "auto"}, False),
    ("fast requested, pool fell back", plan(github_ctx(), ENABLED, {"backend": "fast"}, up_outcome="hosted"), {"backend": "fast"}, False),
)
for name, report, inputs, should_pass in cases:
    code, output = run_gate(report, inputs)
    if (code == 0) != should_pass:
        fail(f"gate with {name}: rc={code}, expected {'pass' if should_pass else 'fail'}:\n{output[-400:]}")
hosted_report = plan(github_ctx(), {}, {"backend": "auto"})
skipped_infra = dict(hosted_report, results=dict(hosted_report["results"], **{"fast-up": "skipped", "fast-down": "skipped"}))
pool_report = plan(github_ctx(), ENABLED, {"backend": "auto"})
skipped_pool_infra = dict(pool_report, results=dict(pool_report["results"], **{"fast-down": "skipped"}))
rerun = {"GITHUB_RUN_ATTEMPT": "2"}
cases = (
    ("fast-up and fast-down skipped while the pool was off", skipped_infra, {"backend": "auto"}, {}, True),
    ("fast-down skipped while the pool was up", skipped_pool_infra, {"backend": "auto"}, {}, False),
    ("a re-run attempt the pool was not made for (lanes hosted by design)", pool_report, {"backend": "auto"}, rerun, True),
    ("a re-run attempt of a fast dispatch", plan(github_ctx(), ENABLED, {"backend": "fast"}), {"backend": "fast"}, rerun, True),
)
for name, report, inputs, extra, should_pass in cases:
    code, output = run_gate(report, inputs, extra)
    if (code == 0) != should_pass:
        fail(f"gate with {name}: rc={code}, expected {'pass' if should_pass else 'fail'}:\n{output[-400:]}")
for job in POOL_JOBS:
    if strip_expr(key_value(ULTRA_JOBS[job], "if") or "") != "!cancelled() && needs.fast-up.result != 'failure'":
        fail(f"pool lane {job} needs the explicit condition `!cancelled() && needs.fast-up.result != 'failure'`")
ok_since(_s5, "the gate fails when any lane was skipped or failed or when fast was requested and not delivered, "
         "passes when every lane ran, tolerates skipped pool jobs only while the pool was off, and treats a re-run "
         "attempt the pool was not made for as hosted; every pool lane has the explicit !cancelled() condition")

# 1g. static wiring.
_s6 = len(FAILURES)


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
for name in ("ultrafast.yml", "fast-runners.yml"):
    with open(os.path.join(WORKFLOWS, name), encoding="utf-8") as handle:
        text = handle.read()
    if "pull_request" in text.replace("pull requests", "").replace("pull-request", ""):
        fail(f"{name} mentions pull_request")
    for line in text.splitlines():
        if line.strip().startswith("runs-on:") and "self-hosted" in line:
            fail(f"{name} targets `self-hosted`; lanes use their own run label")
        match = re.match(r"^\s*(?:- )?uses: ([^\s#]+)", line)
        if match and not match.group(1).startswith("./"):
            if not re.fullmatch(r"[^@]+@[0-9a-f]{40}", match.group(1)):
                fail(f"{name} uses {match.group(1)}, not pinned to a commit SHA")
    if "setup-gcloud" in text:
        fail(f"{name} uses setup-gcloud; gcloud ships on the hosted image")


def composite_uses(action_dir, seen=None):
    """(action, uses) pairs of a local composite action, recursively."""
    seen = set() if seen is None else seen
    if action_dir in seen:
        return []
    seen.add(action_dir)
    path = os.path.join(ROOT, action_dir[2:] if action_dir.startswith("./") else action_dir, "action.yml")
    found = []
    with open(path, encoding="utf-8") as handle:
        for line in handle:
            match = re.match(r"^\s*(?:- )?uses: ([^\s#]+)", line)
            if not match:
                continue
            found.append((action_dir, match.group(1)))
            if match.group(1).startswith("./"):
                found += composite_uses(match.group(1), seen)
    return found


for job in POOL_JOBS:
    for line in ULTRA_JOBS[job]:
        match = re.match(r"^\s*(?:- )?uses: (\./[^\s#]+)", line)
        if match:
            for action, uses in composite_uses(match.group(1)):
                if not uses.startswith("./") and not re.fullmatch(r"[^@]+@[0-9a-f]{40}", uses):
                    fail(f"pool lane {job} runs {uses} through {action}, not pinned to a commit SHA")

spec_lanes = [entry.get("lane") for entry in SPEC]
if sorted(spec_lanes) != sorted(POOL_JOBS) or len(set(spec_lanes)) != len(spec_lanes):
    fail(f"each pool lane gets exactly one runner: spec lanes {spec_lanes}, pool lanes {POOL_JOBS}")
down_needs = needs_of(ULTRA_JOBS["fast-down"])
down_if = strip_expr(key_value(ULTRA_JOBS["fast-down"], "if") or "")
if not POOL_JOBS:
    fail("ultrafast.yml has no pool lane")
up_timeout = int(key_value(FAST_JOBS["runners-up"], "timeout-minutes") or 0)
for job in POOL_JOBS:
    lines = ULTRA_JOBS[job]
    if "fast-up" not in needs_of(lines):
        fail(f"pool lane {job} does not need fast-up")
    if job not in down_needs:
        fail(f"fast-down does not need pool lane {job}, so it could delete the VM under it")
    text = "\n".join(lines)
    if "secrets." in text or "id-token" in text:
        fail(f"pool lane {job} gets secrets or an id-token")
    runs_on = key_value(lines, "runs-on") or ""
    if "github.run_attempt" not in runs_on:
        fail(f"pool lane {job} does not pin the attempt the pool was made for")
    if f"['{job}']" not in runs_on:
        fail(f"pool lane {job} must route by its own lane key ['{job}'], not a shared class label")
    timeout = int(key_value(lines, "timeout-minutes") or 0)
    if not timeout or up_timeout + timeout + 10 > LAUNCHER.MAX_RUN_MINUTES:
        fail(f"pool lane {job}: up {up_timeout} + lane {timeout} + 10 min margin exceed the {LAUNCHER.MAX_RUN_MINUTES}-minute VM TTL")
if down_if != "always()":
    fail(f"fast-down must run under always(); if: {down_if!r}")
if "fast-up" not in down_needs:
    fail("fast-down must need fast-up")
down_with = mapping(key_value(ULTRA_JOBS["fast-down"], "with"), 4)
if UP_WITH.get("runners") != down_with.get("runners"):
    fail("fast-up and fast-down must pass the same runners spec (down deletes by its names)")
if down_with.get("up_backend") != "${{ needs.fast-up.outputs.backend }}":
    fail("fast-down must pass up's reported backend")
gate_if = strip_expr(key_value(ULTRA_JOBS["gate"], "if") or "")
gate_needs = needs_of(ULTRA_JOBS["gate"])
if gate_if != "always()" or not {"fast-up", "fast-down", *POOL_JOBS} <= set(gate_needs):
    fail("the ultrafast gate must run under always() and need fast-up, fast-down and every pool lane")
gate_text = "\n".join(ULTRA_JOBS["gate"])
if "run: scripts/fast-runners verify-lanes" not in gate_text or "actions: read" not in gate_text:
    fail("the gate must run scripts/fast-runners verify-lanes with actions: read")
if json.loads(GATE_ENV.get("FAST_RUNNERS_POOL_LANES") or "[]") != POOL_JOBS:
    fail("the gate's FAST_RUNNERS_POOL_LANES must list exactly the pool lanes")
if GATE_ENV.get("FAST_RUNNERS_SPEC") != UP_WITH.get("runners"):
    fail("the gate's FAST_RUNNERS_SPEC must equal the up spec")
for key, want in (("FAST_RUNNERS_NEEDS", "${{ toJSON(needs) }}"), ("FAST_RUNNERS_UP_BACKEND", "${{ needs.fast-up.outputs.backend }}"),
                  ("FAST_RUNNERS_RUNS_ON", "${{ needs.fast-up.outputs.runs_on }}")):
    if GATE_ENV.get(key) != want:
        fail(f"the gate's {key} must be {want}")

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
    statuses = STATUS.findall(condition)
    if job == "runners-up" and statuses:
        fail("fast-runners.yml runners-up must not use a status function")
    if job == "runners-down" and (statuses != ["always"] or "always()" not in conjuncts):
        fail("fast-runners.yml runners-down needs always() as a conjunct, or a cancelled run skips teardown")
    guards[job] = [part for part in conjuncts if "inputs.action" not in part and part != "always()"]
    want_env = "meerkat-fast-runners" if job == "runners-up" else "meerkat-fast-teardown"
    if key_value(FAST_JOBS[job], "environment") != want_env:
        fail(f"fast-runners.yml {job} must use the main-only {want_env} environment")
    job_text = "\n".join(FAST_JOBS[job])
    if ("job_workflow_sha" not in job_text or "compare/${sha}...main" not in job_text
            or "ref: ${{ steps.launcher.outputs.sha }}" not in job_text or re.search(r"^\s+ref: main\s*$", job_text, re.M)):
        fail(f"fast-runners.yml {job} must check out the launcher at its own job_workflow_sha, verified on main, never main's tip")
    want_sa = "MEERKAT_FAST_RUNNERS_LAUNCHER_SA" if job == "runners-up" else "MEERKAT_FAST_RUNNERS_TEARDOWN_SA"
    if f"vars.{want_sa}" not in job_text or ("MEERKAT_FAST_RUNNERS_LAUNCHER_SA" in job_text and job == "runners-down"):
        fail(f"fast-runners.yml {job} must authenticate as {want_sa} only")
if guards.get("runners-up") != guards.get("runners-down"):
    fail("runners-up and runners-down must share one eligibility guard")
fast_text = "\n".join(FAST_JOBS["runners-up"] + FAST_JOBS["runners-down"])
for needle in ("repository: lukacf/meerkat", "fast-runners-launcher/scripts/fast-runners up",
               "fast-runners-launcher/scripts/fast-runners down", "fast-runners-launcher/scripts/fast-runners fallback",
               "vars.MEERKAT_FAST_RUNNERS_WIF_PROVIDER", "FAST_RUNNERS_CODEOWNERS_CALLER", "FAST_RUNNERS_CODEOWNERS_LAUNCHER"):
    if needle not in fast_text:
        fail(f"fast-runners.yml lacks {needle!r}")
if "secrets." in fast_text:
    fail("fast-runners.yml must not read repository secrets")
with open(os.path.join(ROOT, ".github", "CODEOWNERS"), encoding="utf-8") as handle:
    problems = LAUNCHER.codeowners_problems(CORE, handle.read())
if problems:
    fail(f"the repository's CODEOWNERS does not hold the pool files for the owner: {problems}")
ok_since(_s6, "static wiring: dispatch-only caller, call-only launcher with SHA-pinned actions, no other workflow "
         "reaches the pool, one runner per lane routed by its own key, lane timeouts inside the VM TTL, down under "
         "always() needing every lane, the gate wired to verify-lanes, and CODEOWNERS holding the pool files")


# --- 2. the job-started hook --------------------------------------------------------------------

_s7 = len(FAILURES)
POLICY = """OWNER=lukacf
OWNER_ID=16124748
REPOSITORIES=lukacf/meerkat lukacf/meerkat-meta
WORKFLOWS=lukacf/meerkat/.github/workflows/ultrafast.yml@refs/heads/main lukacf/meerkat-meta/.github/workflows/ultrafast.yml@refs/heads/main
RUN_ID=9001
RUN_ATTEMPT=1
"""
with open(HOOK, encoding="utf-8") as handle:
    HOOK_TEXT = handle.read()
POLICY_LINE = "policy=/etc/meerkat-fast-runner/policy.env\n"
if HOOK_TEXT.count(POLICY_LINE) != 1 or "MEERKAT_FAST_RUNNER_POLICY" in HOOK_TEXT:
    fail("the hook must read its policy from the fixed path only")


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


OWNER_LINE = "POLICY_OWNER_UID = 0  # the policy must be root-owned and writable by its owner only\n"
if HOOK_TEXT.count(OWNER_LINE) != 1:
    fail("the hook must require a root-owned policy")


def run_hook(tmp, owner_uid=None, **kwargs):
    # Test seam: a copy of the hook whose fixed policy path points at the
    # scratch policy and whose required owner is this test's user.
    hook = os.path.join(tmp, "hook")
    uid = os.getuid() if owner_uid is None else owner_uid
    with open(hook, "w", encoding="utf-8") as handle:
        handle.write(HOOK_TEXT.replace(POLICY_LINE, f"policy={os.path.join(tmp, 'policy.env')}\n")
                     .replace(OWNER_LINE, f"POLICY_OWNER_UID = {uid}\n"))
    proc = subprocess.run(["bash", hook], env=hook_env(tmp, **kwargs), capture_output=True, text=True)
    return proc.returncode, proc.stdout + proc.stderr


def write_policy(tmp, text, mode=0o644):
    path = os.path.join(tmp, "policy.env")
    with open(path, "w", encoding="utf-8") as handle:
        handle.write(text)
    os.chmod(path, mode)  # explicit: the hook refuses group/other-writable policies


with tempfile.TemporaryDirectory(prefix="fast-runners-hook.") as tmp:
    write_policy(tmp, POLICY)
    code, output = run_hook(tmp)
    if code != 0:
        fail(f"the hook refuses the pinned owner dispatch on main:\n{output}")
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
        ("a collaborator re-run", dict(overrides={"GITHUB_TRIGGERING_ACTOR": "collaborator"})),
        ("another workflow file", dict(overrides={"GITHUB_WORKFLOW_REF": f"{CORE}/.github/workflows/ci.yml@refs/heads/main"},
                                       payload_overrides={"workflow": ".github/workflows/ci.yml"})),
        ("the allowlisted file at another ref", dict(overrides={"GITHUB_WORKFLOW_REF": f"{CORE}/.github/workflows/ultrafast.yml@refs/heads/feature"})),
        ("another repository", dict(overrides={"GITHUB_REPOSITORY": "attacker/meerkat", "GITHUB_REPOSITORY_OWNER": "attacker"})),
        ("Meta's workflow in Core's run", dict(overrides={"GITHUB_WORKFLOW_REF": META_ULTRAFAST_REF})),
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
    for name, policy in (
        ("no policy file", None),
        ("a policy without a pinned run or PERSISTENT=1", POLICY.replace("RUN_ID=9001\n", "").replace("RUN_ATTEMPT=1\n", "")),
        ("a policy without workflows", POLICY.replace("WORKFLOWS=", "#")),
    ):
        path = os.path.join(tmp, "policy.env")
        if os.path.exists(path):
            os.unlink(path)
        if policy is not None:
            write_policy(tmp, policy)
        code, output = run_hook(tmp)
        if code == 0:
            fail(f"the hook admits a job with {name}")
    write_policy(tmp, POLICY, mode=0o664)
    code, _ = run_hook(tmp)
    if code == 0:
        fail("the hook admits a group-writable policy")
    write_policy(tmp, POLICY)
    code, _ = run_hook(tmp, owner_uid=os.getuid() + 1)
    if code == 0:
        fail("the hook admits a policy owned by another user")
    write_policy(tmp, POLICY.replace("RUN_ID=9001\nRUN_ATTEMPT=1\n", "PERSISTENT=1\n"))
    code, output = run_hook(tmp, overrides={"GITHUB_RUN_ID": "12345"})
    code_pr, _ = run_hook(tmp, overrides={"GITHUB_EVENT_NAME": "pull_request"}, payload_overrides=fork_pr)
    if code != 0 or code_pr == 0:
        fail(f"a persistent-runner policy must admit any owner dispatch run on main and still refuse pull requests: {output}")
ok_since(_s7, f"the hook (fixed path, root-owned non-writable policy) admits the pinned run's owner dispatch on main, "
         f"refuses {len(refusals)} other cases before any step, fails closed on a missing, incomplete, writable or "
         "foreign-owned policy, and a PERSISTENT=1 policy (the owner's Mac) still refuses pull requests")


# --- 3. scripts/fast-runners against recording fakes --------------------------------------------

FAKE_GCLOUD = r'''#!/usr/bin/env python3
import fcntl, hashlib, json, os, sys
state_dir = os.environ["FAKE_STATE"]
_lock = open(os.path.join(state_dir, "fake.lock"), "w")
fcntl.flock(_lock, fcntl.LOCK_EX)  # the launcher runs VM creates in parallel
argv = sys.argv[1:]
record = {"tool": "gcloud", "argv": argv}
stdin = b""
if "--plaintext-file" in argv and argv[argv.index("--plaintext-file") + 1] == "-":
    stdin = sys.stdin.buffer.read()
    record["stdin"] = stdin.decode()
if "--metadata-from-file" in argv:
    files = {}
    for pair in argv[argv.index("--metadata-from-file") + 1].split(","):
        key, _, path = pair.partition("=")
        with open(path, encoding="utf-8") as handle:
            files[key] = handle.read()
    record["files"] = files
if argv[:2] == ["kms", "asymmetric-sign"]:
    with open(argv[argv.index("--input-file") + 1], "rb") as handle:
        record["sign_input"] = handle.read().decode()
with open(os.path.join(state_dir, "calls.jsonl"), "a", encoding="utf-8") as handle:
    handle.write(json.dumps(record) + "\n")
with open(os.path.join(state_dir, "state.json"), encoding="utf-8") as handle:
    state = json.load(handle)
def save():
    with open(os.path.join(state_dir, "state.json"), "w", encoding="utf-8") as handle:
        json.dump(state, handle)
if argv[:2] == ["kms", "asymmetric-sign"]:
    with open(argv[argv.index("--signature-file") + 1], "wb") as handle:
        handle.write(b"SIG(" + hashlib.sha256(record["sign_input"].encode()).hexdigest().encode() + b")")
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
if argv[:3] == ["compute", "instances", "list"]:
    for row in state.get("listing", []):
        print("\t".join(row))
    sys.exit(0)
if argv[:3] == ["compute", "instances", "delete"]:
    if state.get("fail_delete"):
        sys.stderr.write("ERROR: (gcloud.compute.instances.delete) backend error\n")
        sys.exit(1)
    state["instances"] = [n for n in state.get("instances", []) if n not in argv]
    state["listing"] = [row for row in state.get("listing", []) if row[0] not in argv]
    names = []
    for a in argv[3:]:
        if a.startswith("--"):
            break
        names.append(a)
    state.setdefault("deleted", []).extend(names)
    save()
    sys.exit(0)
sys.stderr.write(f"fake gcloud: unexpected {argv}\n")
sys.exit(2)
'''

FAKE_CURL = r'''#!/usr/bin/env python3
import fcntl, json, os, re, stat, sys
state_dir = os.environ["FAKE_STATE"]
_lock = open(os.path.join(state_dir, "fake.lock"), "w")
fcntl.flock(_lock, fcntl.LOCK_EX)
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
            header_files.append(stat.S_IMODE(os.stat(h[1:]).st_mode))
            headers += open(h[1:], encoding="utf-8").read().splitlines()
        else:
            headers.append(h)
        i += 2; continue
    if a == "--data-binary":
        body = json.loads(sys.stdin.read()) if argv[i + 1] == "@-" else None; i += 2; continue
    if a == "-w":
        i += 2; continue
    if a.startswith("-"):
        i += 1; continue
    url = a; i += 1
auth = [h for h in headers if h.lower().startswith("authorization:")]
token = auth[0].split(None, 2)[2] if auth else None
record = {"tool": "curl", "argv": argv, "method": method, "url": url, "body": body,
          "header_file_modes": header_files, "auth_scheme": auth[0].split()[1] if auth else None, "token": token}
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
def page(items, query):
    params = dict(p.split("=", 1) for p in query.split("&") if "=" in p)
    per, number = int(params.get("per_page", 30)), int(params.get("page", 1))
    return items[(number - 1) * per: number * per]
path, _, query = url.replace("https://api.github.com", "").partition("?")
repo_match = re.match(r"^/repos/([^/]+/[^/]+)(/.*)?$", path)
repo, rest = (repo_match.group(1), repo_match.group(2) or "") if repo_match else (None, path)
repos = state.setdefault("repos", {})
r = repos.setdefault(repo or "", {})
if repo and rest == "" and method == "GET":
    reply({"full_name": repo, "default_branch": r.get("default_branch", "main")})
if repo and rest == "/installation" and method == "GET":
    reply({"id": 4242 if repo == "lukacf/meerkat" else 4343})
if re.fullmatch(r"/app/installations/\d+/access_tokens", path) and method == "POST":
    count = state.get("token_count", 0) + 1
    state["token_count"] = count
    save()
    reply({"token": f"ghs_FAKETOKEN{count}"}, 201)
if path == "/installation/token" and method == "DELETE":
    reply(None, 204)
if repo and rest == "/collaborators" and method == "GET":
    reply(page(r.get("collaborators", [{"login": "lukacf", "permissions": {"admin": True, "push": True}}]), query))
if repo and rest == "/keys" and method == "GET":
    reply(page(r.get("keys", []), query))
if repo and rest == "/rulesets" and method == "GET":
    reply(page([{"id": rs["id"]} for rs in r.get("rulesets", [])], query))
m = re.fullmatch(r"/rulesets/(\d+)", rest)
if repo and m and method == "GET":
    reply(next(rs for rs in r.get("rulesets", []) if str(rs["id"]) == m.group(1)))
if repo and rest == "/actions/runners/generate-jitconfig" and method == "POST":
    runners = r.setdefault("runners", [])
    rid = 100 + len(runners)
    labels = [{"name": label} for label in body["labels"] + state.get("extra_labels", [])]
    runners.append({"id": rid, "name": body["name"], "status": state.get("runner_status", "online"), "labels": labels})
    save()
    if state.get("jit_missing_after") is not None and len(runners) > state["jit_missing_after"]:
        reply({"runner": {"id": rid, "name": body["name"]}}, 201)  # malformed: no encoded_jit_config
    reply({"runner": {"id": rid, "name": body["name"]}, "encoded_jit_config": "JITCONFIG-SECRET-" + body["name"]}, 201)
if repo and rest == "/actions/runners" and method == "GET":
    runners = r.get("padding", []) + r.get("runners", [])
    reply({"total_count": len(runners), "runners": page(runners, query)})
m = re.fullmatch(r"/actions/runners/(\d+)", rest)
if repo and m and method == "DELETE":
    r["runners"] = [x for x in r.get("runners", []) if str(x["id"]) != m.group(1)]
    r["padding"] = [x for x in r.get("padding", []) if str(x["id"]) != m.group(1)]
    save()
    reply(None, 204)
m = re.fullmatch(r"/actions/runs/(\d+)/attempts/(\d+)/jobs", rest)
if repo and m and method == "GET":
    jobs = state.get("jobs", [])
    reply({"total_count": len(jobs), "jobs": page(jobs, query)})
reply({"message": "Not Found"}, 404)
'''

CORE_CODEOWNERS = "* @lukacf\n/.github/ @lukacf\n/scripts/fast-runners* @lukacf\n/scripts/test-fast-runners-contract.sh @lukacf\n"
GOOD_RULESET = {
    "id": 11, "target": "branch", "enforcement": "active",
    "conditions": {"ref_name": {"include": ["~DEFAULT_BRANCH"], "exclude": []}},
    "bypass_actors": [{"actor_id": 5, "actor_type": "RepositoryRole", "bypass_mode": "always"}],
    "rules": [{"type": "pull_request", "parameters": {"require_code_owner_review": True, "dismiss_stale_reviews_on_push": True,
                                                      "require_last_push_approval": True, "required_approving_review_count": 1}}],
}


def merged(base, extra):
    out = json.loads(json.dumps(base))
    for key, value in (extra or {}).items():
        if isinstance(value, dict) and isinstance(out.get(key), dict):
            out[key] = merged(out[key], value)
        else:
            out[key] = value
    return out


DEFAULT_STATE = {"repos": {CORE: {"rulesets": [GOOD_RULESET]}, META: {"rulesets": [GOOD_RULESET]}}}


def setup_fakes(tmp, state):
    bin_dir = os.path.join(tmp, "bin")
    os.makedirs(bin_dir, exist_ok=True)
    for name, body in (("gcloud", FAKE_GCLOUD), ("curl", FAKE_CURL)):
        path = os.path.join(bin_dir, name)
        with open(path, "w", encoding="utf-8") as handle:
            handle.write(body)
        os.chmod(path, 0o755)
    for name in ("caller", "launcher"):
        os.makedirs(os.path.join(tmp, name), exist_ok=True)
        text = state.pop(f"codeowners_{name}", CORE_CODEOWNERS)
        if text is not None:
            with open(os.path.join(tmp, name, "CODEOWNERS"), "w", encoding="utf-8") as handle:
                handle.write(text)
    with open(os.path.join(tmp, "state.json"), "w", encoding="utf-8") as handle:
        json.dump(state, handle)
    open(os.path.join(tmp, "calls.jsonl"), "w").close()
    return bin_dir


def run_launcher(tmp, command, extra_env=None, state=None):
    bin_dir = setup_fakes(tmp, merged(DEFAULT_STATE, state))
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
        "FAST_RUNNERS_SPEC": '[{"lane":"workspace-lib","class":"x64-xl"}]',
        "FAST_RUNNERS_CODEOWNERS_CALLER": os.path.join(tmp, "caller", "CODEOWNERS"),
        "FAST_RUNNERS_CODEOWNERS_LAUNCHER": os.path.join(tmp, "launcher", "CODEOWNERS"),
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
        "GITHUB_TOKEN": "ghs_RUNTOKEN",
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


def curl_calls(calls, method=None, contains=None):
    return [c for c in calls if c["tool"] == "curl" and (method is None or c["method"] == method)
            and (contains is None or contains in c["url"])]


def runners_of(final_state, repo=CORE):
    return [r["name"] for r in final_state.get("repos", {}).get(repo, {}).get("runners", [])]


def minted(calls):
    return [f"ghs_FAKETOKEN{i + 1}" for i in range(len(curl_calls(calls, "POST", "/access_tokens")))]


def revoked(calls):
    return [c["token"] for c in curl_calls(calls, "DELETE", "/installation/token")]


# 3a. variable unset, backend hosted or an ineligible trigger: no call at all.
_s8 = len(FAILURES)
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
    ("the unsupported shared project", {"MEERKAT_FAST_RUNNERS_PROJECT": "king-dnn-training-dev"}),
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
ok_since(_s8, "up and down call nothing (no GitHub, no GCP) while the variable is unset, for backend hosted, a fork "
         "pull request, a collaborator re-run, a push, a branch, another actor, repository or owner, a self-hosted "
         "launcher, another caller workflow or the unsupported shared project")

# 3b. the eligible up.
_s9 = len(FAILURES)
proc, calls, outputs, final_state = scenario("up")
transcript = proc.stdout + proc.stderr
if proc.returncode != 0 or outputs.get("backend") != "fast":
    fail(f"eligible up failed: rc={proc.returncode}\n{transcript[-800:]}")
else:
    runs_on = json.loads(outputs["runs_on"])
    label = runs_on.get("workspace-lib", "")
    if set(runs_on) != {"workspace-lib"} or not re.fullmatch(r"meerkat-fast-9001-1-[0-9a-f]{16}-workspace-lib", label):
        fail(f"runs_on {runs_on} is not one run/attempt/nonce label per lane")
    masked = re.findall(r"::add-mask::(\S+)", transcript)
    for key, value in outputs.items():
        for secret in masked:
            if secret in value:
                fail(f"output {key} contains the masked value {secret[:16]}...; GitHub drops such outputs")
    for value in outputs.values():
        if len(value) > 4 and value in transcript:
            fail("up printed a job output value")
    if label and label in transcript:
        fail("up printed the lane label")
    registrations = curl_calls(calls, "POST", "/actions/runners/generate-jitconfig")
    if len(registrations) != 1:
        fail(f"{len(registrations)} JIT registrations for one lane")
    else:
        body = registrations[0]["body"]
        if body.get("labels") != [label]:
            fail(f"the JIT runner labels are {body.get('labels')}, expected exactly [{label!r}] (no self-hosted)")
        if body.get("runner_group_id") != 1 or body.get("name") != "meerkat-fast-9001-1-0" or body.get("work_folder") != "_work":
            fail(f"unexpected registration body {body}")
    if gcloud_calls(calls, "secrets"):
        fail("up read Secret Manager; the App key lives only in Cloud KMS")
    signs = gcloud_calls(calls, "kms", "asymmetric-sign")
    if not signs or any("--key" not in c["argv"] or c["argv"][c["argv"].index("--key") + 1] != "github-app-jwt" for c in signs):
        fail("the App JWT must be signed by the Cloud KMS key github-app-jwt")
    tokens = curl_calls(calls, "POST", "/access_tokens")
    if [c["body"] for c in tokens] != [{"repositories": ["meerkat"], "permissions": {"administration": "write"}}]:
        fail(f"the installation token is not limited to this repository and Administration: write: {[c['body'] for c in tokens]}")
    if revoked(calls) != minted(calls) or curl_calls(calls)[-1]["url"] != "https://api.github.com/installation/token":
        fail(f"up must revoke every installation token at the end: minted {minted(calls)}, revoked {revoked(calls)}")
    if not curl_calls(calls, "GET", "/rulesets") or not curl_calls(calls, "GET", "/repos/lukacf/meerkat"):
        fail("up must check the review tripwire (default branch and rulesets)")
    if curl_calls(calls, "GET", "/collaborators"):
        fail("up must not judge writers by the collaborator list; the ruleset is the boundary")
    for call in curl_calls(calls):
        if any("ghs_" in arg or "Bearer" in arg for arg in call["argv"]):
            fail("a GitHub token travelled in curl's argv")
        if call["header_file_modes"] != [0o600] or call["auth_scheme"] != "Bearer":
            fail(f"curl header file mode {call['header_file_modes']} / auth {call['auth_scheme']}")
    creates = gcloud_calls(calls, "compute", "instances", "create")
    if len(creates) != 1:
        fail(f"{len(creates)} VM creations for one lane")
    else:
        argv = creates[0]["argv"]
        joined = " ".join(argv)
        for needle in ("--project meerkat-fast-runners", "--network-interface=subnet=meerkat-fast-runners-europe-west4,no-address",
                       "--max-run-duration 120m", "--instance-termination-action DELETE",
                       "--provisioning-model STANDARD", "--no-restart-on-failure", "--shielded-secure-boot",
                       "--service-account meerkat-fast-runner@meerkat-fast-runners.iam.gserviceaccount.com",
                       "--machine-type c4d-standard-192", "block-project-ssh-keys=TRUE", "enable-oslogin=TRUE",
                       "meerkat-fast-run-id=9001", "meerkat-fast-run-attempt=1", "purpose=meerkat-fast-runner",
                       "lane=workspace-lib"):
            if needle not in joined:
                fail(f"VM creation lacks {needle!r}")
        for forbidden in ("--tags", "--address", "--network-tier", "--can-ip-forward", "--scopes default"):
            if forbidden in joined:
                fail(f"VM creation has {forbidden!r}")
        files = creates[0].get("files", {})
        if files.get("startup-script") != LAUNCHER.render_startup():
            fail("the VM startup script is not the rendered one")
        if files.get("meerkat-fast-hook") != HOOK_TEXT:
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
    unmasked = transcript
    for secret in masked:
        unmasked = unmasked.replace(f"::add-mask::{secret}", "")
    if re.search(r"JITCONFIG-SECRET|ghs_FAKE", unmasked):
        fail("up printed the registration or a token outside an ::add-mask:: command")
    for value in ("JITCONFIG-SECRET-meerkat-fast-9001-1-0", "ghs_FAKETOKEN1"):
        if value not in masked:
            fail(f"up does not mask {value[:12]}...")
    if final_state.get("instances") != ["meerkat-fast-9001-1-0"]:
        fail(f"unexpected instances after up: {final_state.get('instances')}")
ok_since(_s9, "eligible up: KMS-signed App JWT (no Secret Manager), a repository-scoped Administration token revoked at "
         "the end, the review tripwire checked, one JIT runner with exactly its lane's label in the relative work "
         "folder, outputs that contain no masked value and are never printed, the registration only on KMS stdin and "
         "as ciphertext in metadata, an outbound-only shielded VM in the dedicated project with a 120m GCE TTL")

# 3c. one runner and one label per lane; runners with any extra label are refused.
_s10 = len(FAILURES)
two_lanes = '[{"lane":"inventory","class":"x64-xl"},{"lane":"clippy","class":"x64-l"}]'
proc, calls, outputs, final_state = scenario("up", {"FAST_RUNNERS_SPEC": two_lanes})
runs_on = json.loads(outputs.get("runs_on") or "{}")
bodies = [c["body"] for c in curl_calls(calls, "POST", "/actions/runners/generate-jitconfig")]
if (outputs.get("backend") != "fast" or set(runs_on) != {"inventory", "clippy"} or len(set(runs_on.values())) != 2
        or [b["labels"] for b in bodies] != [[runs_on["inventory"]], [runs_on["clippy"]]]
        or not runs_on.get("inventory", "").endswith("-inventory") or not runs_on.get("clippy", "").endswith("-clippy")):
    fail(f"two lanes must get two runners with one distinct label each: {runs_on} {bodies}")
for backend in ("auto", "fast"):
    proc, calls, outputs, final_state = scenario("up", {"FAST_RUNNERS_BACKEND": backend},
                                                 {"extra_labels": ["self-hosted", "Linux", "X64"]})
    if final_state.get("instances") or runners_of(final_state):
        fail(f"a runner with default labels ({backend}) was left behind")
    if (backend == "auto" and outputs.get("backend") != "hosted") or (backend == "fast" and proc.returncode == 0):
        fail(f"a runner carrying default labels must be refused ({backend}): rc={proc.returncode} {outputs}")
for spec in ('[{"lane":"a","class":"x64-xl"},{"lane":"a","class":"x64-l"}]', '[{"class":"x64-xl","count":2}]',
             '[{"lane":"Bad_Lane","class":"x64-xl"}]', '[{"lane":"a","class":"n2-highcpu-4"}]',
             json.dumps([{"lane": f"l{i}", "class": "x64-l"} for i in range(9)])):
    proc, calls, outputs, _ = scenario("up", {"FAST_RUNNERS_SPEC": spec})
    if proc.returncode == 0 or calls:
        fail(f"up must refuse the spec {spec[:60]} before any call")
ok_since(_s10, "every pool lane gets its own runner and label; a runner reporting any label besides its own (such as "
         "default self-hosted/OS/arch labels) is deleted and refused; repeated lanes, class counts, bad ids, unknown "
         "classes and oversized specs are refused before any call")

# 3d. the review tripwire: always CODEOWNERS plus the review ruleset.
_s11 = len(FAILURES)
stale = dict(GOOD_RULESET, rules=[{"type": "pull_request", "parameters": dict(GOOD_RULESET["rules"][0]["parameters"], dismiss_stale_reviews_on_push=False)}])
no_last_push = dict(GOOD_RULESET, rules=[{"type": "pull_request", "parameters": dict(GOOD_RULESET["rules"][0]["parameters"], require_last_push_approval=False)}])
hidden_bypass = {key: value for key, value in GOOD_RULESET.items() if key != "bypass_actors"}
precondition_cases = (
    ("the review ruleset and CODEOWNERS", {}, True),
    ("no review ruleset (whoever the collaborators are)", {"repos": {CORE: {"rulesets": []}}}, False),
    ("a ruleset that keeps stale reviews", {"repos": {CORE: {"rulesets": [stale]}}}, False),
    ("a ruleset without last-push approval", {"repos": {CORE: {"rulesets": [no_last_push]}}}, False),
    ("a ruleset the write role bypasses", {"repos": {CORE: {"rulesets": [dict(GOOD_RULESET, bypass_actors=[{"actor_id": 4, "actor_type": "RepositoryRole", "bypass_mode": "always"}])]}}}, False),
    ("a ruleset whose bypass actors the token cannot see", {"repos": {CORE: {"rulesets": [hidden_bypass]}}}, False),
    ("a ruleset with an exclude list", {"repos": {CORE: {"rulesets": [dict(GOOD_RULESET, conditions={"ref_name": {"include": ["~DEFAULT_BRANCH"], "exclude": ["refs/heads/release/*"]}})]}}}, False),
    ("a default branch other than main", {"repos": {CORE: {"default_branch": "trunk"}}}, False),
    ("an evaluate-only ruleset", {"repos": {CORE: {"rulesets": [dict(GOOD_RULESET, enforcement="evaluate")]}}}, False),
    ("CODEOWNERS handing workflows to someone else", {"codeowners_caller": CORE_CODEOWNERS + "/.github/workflows/ @collaborator\n"}, False),
    ("no CODEOWNERS", {"codeowners_caller": None}, False),
)
for name, state, admitted in precondition_cases:
    for backend in ("auto", "fast"):
        proc, calls, outputs, final_state = scenario("up", {"FAST_RUNNERS_BACKEND": backend}, state)
        registered = bool(curl_calls(calls, "POST", "generate-jitconfig")) or bool(gcloud_calls(calls, "compute", "instances", "create"))
        if admitted and outputs.get("backend") != "fast":
            fail(f"tripwire met ({name}, {backend}) but up did not bring the pool up: {proc.stdout[-300:]}")
        if not admitted:
            if registered:
                fail(f"tripwire not met ({name}, {backend}) but up registered runners or created VMs")
            if backend == "auto" and outputs.get("backend") != "hosted":
                fail(f"tripwire not met ({name}): auto must fall back to hosted")
            if backend == "fast" and proc.returncode == 0:
                fail(f"tripwire not met ({name}): fast must fail")
        if revoked(calls) != minted(calls):
            fail(f"{name} ({backend}): tokens not all revoked: minted {minted(calls)}, revoked {revoked(calls)}")
meta_env = {"GITHUB_REPOSITORY": META, "GITHUB_WORKFLOW_REF": META_ULTRAFAST_REF}
meta_owners = "* @lukacf\n/.github/ @lukacf\n"
proc, calls, outputs, final_state = scenario("up", meta_env, {"codeowners_caller": meta_owners})
token_bodies = [c["body"] for c in curl_calls(calls, "POST", "/access_tokens")]
if outputs.get("backend") != "fast" or token_bodies != [
        {"repositories": ["meerkat-meta"], "permissions": {"administration": "write"}},
        {"repositories": ["meerkat"], "permissions": {"administration": "write"}}] or revoked(calls) != minted(calls):
    fail(f"a Meta run must check Meta and the Core launcher with write-scoped tokens (so bypass actors show) and revoke both: {token_bodies}")
for name, core in (("no review ruleset", {"rulesets": []}), ("hidden bypass actors", {"rulesets": [hidden_bypass]})):
    proc, calls, outputs, final_state = scenario("up", meta_env, {"codeowners_caller": meta_owners, "repos": {CORE: core}})
    if outputs.get("backend") != "hosted" or curl_calls(calls, "POST", "generate-jitconfig"):
        fail(f"a Meta run must refuse while the Core launcher's ruleset is not proven ({name})")
ok_since(_s11, "review tripwire: always required (no writer-count shortcut); up launches only with CODEOWNERS giving the "
         "pool files to the owner alone and an active main ruleset (default branch main, empty exclude list, code-owner "
         "review, stale-review dismissal, last-push approval, visible admin-only bypass); a ruleset whose bypass actors "
         "are hidden counts as missing; Meta runs also check the Core launcher with a write-scoped token; every token is "
         "revoked")

# 3e. failures clean up and revoke; in fast they fail after cleaning up.
_s12 = len(FAILURES)
for name, state, extra in (
    ("runners that never come online", {"runner_status": "offline"}, {}),
    ("a VM creation failure", {"fail_create": ["meerkat-fast-9001-1-1"]},
     {"FAST_RUNNERS_SPEC": '[{"lane":"a","class":"x64-xl"},{"lane":"b","class":"x64-xl"}]'}),
):
    for backend in ("auto", "fast"):
        proc, calls, outputs, final_state = scenario("up", dict(extra, FAST_RUNNERS_BACKEND=backend), state)
        if final_state.get("instances") or runners_of(final_state):
            fail(f"{name} ({backend}) leaves instances {final_state.get('instances')} / runners {runners_of(final_state)}")
        if revoked(calls) != minted(calls):
            fail(f"{name} ({backend}): tokens not revoked")
        if backend == "auto" and (proc.returncode != 0 or outputs.get("backend") != "hosted" or outputs.get("runs_on") != "{}"):
            fail(f"{name} (auto) must clean up and select GitHub-hosted runners: rc={proc.returncode} {outputs}")
        if backend == "fast" and (proc.returncode == 0 or outputs.get("backend") == "fast"):
            fail(f"{name} (fast) must fail")
for backend in ("auto", "fast"):
    proc, calls, outputs, final_state = scenario("up", {"FAST_RUNNERS_BACKEND": backend,
                                                        "FAST_RUNNERS_SPEC": '[{"lane":"a","class":"x64-xl"},{"lane":"b","class":"x64-xl"}]'},
                                                 {"jit_missing_after": 1})
    if final_state.get("instances") or runners_of(final_state) or revoked(calls) != minted(calls):
        fail(f"an unexpected exception ({backend}) must still delete every VM and registration and revoke the tokens: "
             f"{final_state.get('instances')} {runners_of(final_state)}")
    if (backend == "auto" and outputs.get("backend") != "hosted") or (backend == "fast" and proc.returncode == 0):
        fail(f"an unexpected exception ({backend}) must fall back (auto) or fail (fast)")
proc, calls, outputs, _ = scenario("fallback", {"FAST_RUNNERS_BACKEND": "fast"})
if proc.returncode == 0 or outputs.get("backend") != "hosted":
    fail("fallback for backend fast must fail")
ok_since(_s12, "runners that never come online, a failed VM creation or an unexpected exception: up deletes every VM "
         "and registration it made and revokes its tokens, then selects GitHub-hosted runners (auto) or fails (fast)")

# 3f. down, with paginated listings.
_s13 = len(FAILURES)
padding = [{"id": 1000 + i, "name": f"someone-else-{i}", "status": "online", "labels": []} for i in range(150)]
state = {"instances": ["meerkat-fast-9001-1-0", "meerkat-fast-9001-1-1", "meerkat-fast-9000-1-0"],
         "repos": {CORE: {"padding": padding, "runners": [
             {"id": 7, "name": "meerkat-fast-9001-1-0", "status": "offline", "labels": []},
             {"id": 8, "name": "meerkat-fast-9001-1-1", "status": "online", "labels": []}]}}}
two = '[{"lane":"a","class":"x64-xl"},{"lane":"b","class":"x64-xl"}]'
proc, calls, outputs, final_state = scenario("down", {"FAST_RUNNERS_SPEC": two, "FAST_RUNNERS_UP_BACKEND": "fast"}, state)
if proc.returncode != 0:
    fail(f"down failed: {proc.stdout[-500:]}")
if final_state.get("instances") != ["meerkat-fast-9000-1-0"]:
    fail(f"down must delete exactly this run's VMs; left {final_state.get('instances')}")
if curl_calls(calls) or gcloud_calls(calls, "kms"):
    fail("down runs as the delete-only teardown identity: no GitHub call, no installation token, no KMS")
if runners_of(final_state) != ["meerkat-fast-9001-1-0", "meerkat-fast-9001-1-1"]:
    fail("down must leave runner registrations to up and GitHub's expiry")
if gcloud_calls(calls, "compute", "instances", "list"):
    fail("down must not list the project's instances")
proc, calls, outputs, final_state = scenario("down", {"FAST_RUNNERS_UP_BACKEND": ""},
                                             {"late_instances": {"meerkat-fast-9001-1-0": 1}})
if proc.returncode != 0 or final_state.get("instances"):
    fail(f"down after a cancelled up must find and delete a VM whose insert was still in flight: {final_state}")
proc, calls, outputs, final_state = scenario("down", {"FAST_RUNNERS_SPEC": two}, dict(state, fail_delete=True))
if proc.returncode == 0:
    fail("down must fail when a VM delete fails")
state = {"repos": {CORE: {"padding": padding, "runners": []}}}
proc, calls, outputs, final_state = scenario("up", {}, state)
if outputs.get("backend") != "fast" or len(curl_calls(calls, "GET", "/actions/runners?")) < 2:
    fail("up must find its runner beyond the first page of the runner listing")
ok_since(_s13, "down deletes exactly this run's VMs by name as a delete-only identity (no GitHub, token or KMS call, no "
         "project listing), finds an insert still in flight after a cancelled up and fails when a delete fails; up finds "
         "its runners beyond the first page of the runner listing")

# 3g. the gate's pool check against this run's jobs.
_s14 = len(FAILURES)
LABEL = "meerkat-fast-9001-1-0123456789abcdef-workspace-lib"
gate_base = {
    "FAST_RUNNERS_NEEDS": json.dumps({"fast-up": {"result": "success"}, "workspace-lib": {"result": "success"},
                                      "fast-down": {"result": "success"}}),
    "FAST_RUNNERS_POOL_LANES": '["workspace-lib"]',
    "FAST_RUNNERS_REQUESTED": "auto",
    "FAST_RUNNERS_UP_BACKEND": "fast",
    "FAST_RUNNERS_UP_ATTEMPT": "1",
    "FAST_RUNNERS_RUNS_ON": json.dumps({"workspace-lib": LABEL}),
}
for name, jobs, should_pass in (
    ("the lane on its pool runner", [{"name": "Workspace lib tests", "labels": [LABEL], "runner_name": "meerkat-fast-9001-1-0", "conclusion": "success"}], True),
    ("the lane silently GitHub-hosted while the pool was up", [{"name": "Workspace lib tests", "labels": ["ubuntu-24.04"], "runner_name": "GitHub Actions 7", "conclusion": "success"}], False),
    ("the lane on another runner", [{"name": "Workspace lib tests", "labels": [LABEL], "runner_name": "meerkat-fast-9001-1-5", "conclusion": "success"}], False),
    ("the pool lane's job missing", [], False),
):
    proc, calls, outputs, _ = scenario("verify-lanes", gate_base, {"jobs": [{"name": "Fast runners up / Runners up", "labels": ["ubuntu-24.04"], "runner_name": "GitHub Actions 1", "conclusion": "success"}] * 120 + jobs})
    if (proc.returncode == 0) != should_pass:
        fail(f"verify-lanes with {name}: rc={proc.returncode}, expected {'pass' if should_pass else 'fail'}\n{proc.stdout[-300:]}")
ok_since(_s14, "the gate's pool check (paginated jobs listing) passes only when each pool lane ran on its own pool "
         "runner and fails when a lane silently ran GitHub-hosted, ran elsewhere or is missing")

# 3h. boot script hardening and the reaper.
_s15 = len(FAILURES)
startup = LAUNCHER.render_startup()
if re.search(r"NOPASSWD|>\s*/etc/sudoers", startup) or "rm -f /etc/sudoers.d/runner" not in startup:
    fail("the job user must have no sudo (it could stop its VM and clear the TTL)")
startup_code = "\n".join(line for line in startup.splitlines() if not line.lstrip().startswith("#"))
if any(startup_code[m.end():m.end() + 16] != " -n -l -U runner" for m in re.finditer(r"\bsudo\b(?!ers)", startup_code)):
    fail("the boot script may call sudo only to assert the job user has none (never to start the runner)")
for assertion in ('if [[ "$(id -nG runner)" != "runner" ]]; then',
                  "if sudo -n -l -U runner 2>/dev/null | grep -q 'may run the following'; then"):
    if assertion not in startup_code.splitlines() or startup_code.count(assertion) != 1:
        fail(f"the boot script must assert that the job user has no extra groups and no sudo: {assertion!r}")
if "--jitconfig" in startup or "ACTIONS_RUNNER_INPUT_JITCONFIG" not in startup or "runuser -u runner" not in startup:
    fail("the registration must reach the runner through ACTIONS_RUNNER_INPUT_JITCONFIG from a runner-only file, never argv")
if "chmod 0400 /run/meerkat-fast-runner/jit" not in startup or "rm -f /run/meerkat-fast-runner/jit" not in startup:
    fail("the registration file must be runner-only (0400) and removed before the runner starts")
if "--uid-owner runner -j REJECT" not in startup:
    fail("job processes must not reach the metadata token endpoint")
reaper = LAUNCHER.render_reaper()
now = datetime.datetime.now(datetime.timezone.utc)
old = (now - datetime.timedelta(minutes=LAUNCHER.REAP_AFTER_MINUTES + 30)).strftime("%Y-%m-%dT%H:%M:%S.000-00:00")
young = (now - datetime.timedelta(minutes=10)).strftime("%Y-%m-%dT%H:%M:%S.000-00:00")
listing = [["meerkat-fast-1-1-0", "europe-west4-a", old, "RUNNING"], ["meerkat-fast-1-1-1", "europe-west4-a", old, "TERMINATED"],
           ["meerkat-fast-2-1-0", "europe-west4-b", old, "SUSPENDED"], ["meerkat-fast-3-1-0", "europe-west4-a", young, "RUNNING"],
           ["somebody-elses-vm", "europe-west4-a", old, "RUNNING"]]
commands_text = subprocess.run([sys.executable, LAUNCHER_SCRIPT, "approval-commands"], capture_output=True, text=True).stdout
job_line = next((line for line in commands_text.splitlines() if line.startswith("gcloud run jobs create meerkat-fast-reaper")), "")
args_match = re.search(r'--args="([^"]*)"', job_line)
job_args = args_match.group(1).split(",") if args_match else []  # gcloud splits list flags on commas
if len(job_args) != 2 or job_args[0] != "-c":
    fail(f"the reaper job's --args must survive gcloud's comma splitting as exactly [-c, script]: {job_args[:3]}")
else:
    encoded = re.fullmatch(r"echo ([A-Za-z0-9+/=]+) \| base64 -d \| bash", job_args[1])
    if not encoded or base64.b64decode(encoded.group(1)).decode() != reaper:
        fail("the reaper job must run exactly the rendered reaper script")
if "@sha256:" not in job_line:
    fail("the reaper job image must be pinned by digest")
with tempfile.TemporaryDirectory(prefix="fast-runners-reaper.") as tmp:
    bin_dir = setup_fakes(tmp, {"listing": listing})
    # Run it the way the Cloud Run job does: bash -c "<the second --args element>".
    proc = subprocess.run(["bash", "-c", job_args[1] if len(job_args) == 2 else "exit 1"],
                          env=dict(os.environ, PATH=bin_dir + os.pathsep + os.environ["PATH"], FAKE_STATE=tmp),
                          capture_output=True, text=True)
    with open(os.path.join(tmp, "state.json"), encoding="utf-8") as handle:
        reaped = json.load(handle).get("deleted", [])
    with open(os.path.join(tmp, "calls.jsonl"), encoding="utf-8") as handle:
        lists = [json.loads(line)["argv"] for line in handle if '"list"' in line]
    if proc.returncode != 0 or sorted(reaped) != ["meerkat-fast-1-1-0", "meerkat-fast-1-1-1", "meerkat-fast-2-1-0"]:
        fail(f"the reaper must delete stale pool VMs in any state, keep young ones and never touch other names: rc={proc.returncode} {reaped} {proc.stdout[-300:]}")
    if not lists or any("status" in arg.lower() and "filter" in arg.lower() for argv in lists for arg in argv):
        fail("the reaper must list pool VMs by label in every state (no status filter)")
    if not any("name~^meerkat-fast-" in arg for argv in lists for arg in argv):
        fail("the reaper must list only pool names")
ok_since(_s15, "boot hardening: no sudo and no extra groups for the job user (asserted at boot), the registration via a "
         "runner-only file into ACTIONS_RUNNER_INPUT_JITCONFIG (never argv or sudo), metadata token endpoint blocked; "
         "the reaper job (digest-pinned image, base64 script that survives gcloud's comma splitting, run exactly as "
         "Cloud Run does) deletes stale pool VMs in any state, keeps young ones and never touches other names")


# --- 4. WIF condition and approval commands ----------------------------------------------------

_s16 = len(FAILURES)
wif = subprocess.run([sys.executable, LAUNCHER_SCRIPT, "wif"], capture_output=True, text=True).stdout.splitlines()
condition = wif[wif.index("attribute-condition:") + 1].strip()
mapping_lines = [line.strip() for line in wif[1:wif.index("attribute-condition:")]]
role_mapping = next(line.split("=", 1)[1] for line in mapping_lines if line.startswith("attribute.fast_role="))

CEL = re.compile(r"\s*(?:('(?:[^'\\]|\\.)*')|(==|!=|&&|\|\||[?:()\[\],+])|(assertion\.[a-z_]+|assertion\b|in\b))")


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
        if token == "assertion":
            return claims
        if token.startswith("assertion."):
            key = token.split(".", 1)[1]
            if key not in claims:
                raise KeyError(key)  # a missing claim is a CEL error
            return claims[key]
        sys.exit(f"CEL: unexpected {token}")

    def additive():
        value = primary()
        while peek() == "+":
            take()
            value = value + primary()
        return value

    def comparison():
        left = additive()
        op = peek()
        if op in ("==", "!="):
            take()
            right = additive()
            return (left == right) if op == "==" else (left != right)
        if op == "in":
            take()
            return left in additive()
        return left

    def conjunction():
        # CEL &&: false wins over an error on either side.
        values, errors = [], 0
        while True:
            try:
                values.append(comparison())
            except KeyError:
                errors += 1
                skip_operand()
            if peek() != "&&":
                break
            take()
        if any(v is False for v in values):
            return False
        if errors:
            raise KeyError("missing claim")
        return True if len(values) > 1 else values[0]

    def skip_operand():
        depth = 0
        while peek() is not None:
            token = peek()
            if depth == 0 and token in ("&&", "||", "?", ":", ")", ","):
                return
            if token in ("(", "["):
                depth += 1
            if token in (")", "]"):
                depth -= 1
            take()

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
    "repository_owner_id": "16124748", "repository_owner": "lukacf", "repository": CORE, "repository_id": CORE_ID,
    "event_name": "workflow_dispatch", "ref": "refs/heads/main", "actor_id": "16124748", "actor": "lukacf",
    "workflow_ref": ULTRAFAST_REF, "runner_environment": "github-hosted",
    "job_workflow_ref": LAUNCHER_REF, "environment": "meerkat-fast-runners",
}
admitted = (
    ("the Core launcher job", {}),
    ("the Meta launcher job", {"repository": META, "repository_id": META_ID, "workflow_ref": META_ULTRAFAST_REF}),
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
    ("a recreated repository under the same name", {"repository_id": "42"}),
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


def role(change, drop=()):
    claims = dict(BASE, **change)
    for key in drop:
        claims.pop(key, None)
    try:
        return cel(role_mapping, claims)
    except KeyError:
        return "error"


roles = (
    ("the launcher job", {}, (), "launcher"),
    ("the launcher job without its environment", {}, ("environment",), "none"),
    ("the launcher job in another environment", {"environment": "staging - docs"}, (), "none"),
    ("the teardown job", {"environment": "meerkat-fast-teardown"}, (), "teardown"),
    ("the teardown environment outside the launcher", {"environment": "meerkat-fast-teardown", "job_workflow_ref": ULTRAFAST_REF}, (), "none"),
    ("a GitHub-hosted job outside the launcher", {"job_workflow_ref": ULTRAFAST_REF}, ("environment",), "none"),
    ("the launcher file at a branch", {"job_workflow_ref": f"{CORE}/.github/workflows/fast-runners.yml@refs/heads/feature"}, (), "none"),
    ("a Core pool lane", {"runner_environment": "self-hosted", "job_workflow_ref": ULTRAFAST_REF}, ("environment",), f"ci-{CORE_ID}"),
    ("a Meta pool lane", {"runner_environment": "self-hosted", "repository": META, "repository_id": META_ID,
                          "job_workflow_ref": META_ULTRAFAST_REF}, ("environment",), f"ci-{META_ID}"),
    ("a Core release lane", {"runner_environment": "self-hosted", "job_workflow_ref": ULTRAFAST_REF,
                             "environment": "meerkat-fast-release"}, (), f"release-{CORE_ID}"),
)
for name, change, drop, want in roles:
    got = role(change, drop)
    if got != want:
        fail(f"WIF fast_role for {name} is {got!r}, expected {want!r}")
ok_since(_s16, f"the WIF condition admits only the owner's dispatch of an allowlisted ultrafast.yml on main in an "
         f"allowlisted repository id ({len(rejected)} rejections); the launcher role needs the launcher file at main, "
         "a GitHub-hosted runner and the meerkat-fast-runners environment; cache roles are per repository and tier")

_s17 = len(FAILURES)
commands = subprocess.run([sys.executable, LAUNCHER_SCRIPT, "approval-commands"], capture_output=True, text=True).stdout.splitlines()
runnable = [line for line in commands if line.startswith("gcloud ")]
if any("king-dnn-training-dev" in line for line in commands):
    fail("the shared project is unsupported; the approval plan must not provision anything there")
if LAUNCHER.PROJECT == "king-dnn-training-dev" or not any(line.startswith(f"gcloud projects create {LAUNCHER.PROJECT}") for line in runnable):
    fail("the plan must create a dedicated pool project")
if not any(line.startswith(f"gcloud projects get-iam-policy {LAUNCHER.PROJECT}") for line in runnable):
    fail("the dedicated project's IAM policy must be audited")
if any(line.startswith("gcloud secrets") for line in commands):
    fail("the App key must not live in Secret Manager")
app_key = [line for line in runnable if line.startswith("gcloud kms keys create github-app-jwt")]
if not app_key or not all(flag in app_key[0] for flag in ("--purpose=asymmetric-signing", "--import-only", "--protection-level=hsm")):
    fail("the App key must be an imported, non-exportable HSM signing key")
if (not any("kms keys versions import" in line for line in runnable) or any("shred" in line for line in commands)
        or not any("hdiutil attach -nomount ram://" in line for line in commands) or "diskutil eject /Volumes/FastRunnerKey" not in commands):
    fail("the App key must be imported from a macOS RAM disk that is ejected afterwards (no shred on macOS)")
signers = [line for line in runnable if "roles/cloudkms.signer" in line]
if len(signers) != 1 or "github-app-jwt" not in signers[0] or "meerkat-fast-launcher@" not in signers[0]:
    fail("only the launcher may sign with the App key")
for line in runnable:
    if "meerkat-fast-teardown@" in line and "roles/cloudkms" in line:
        fail("the teardown identity must have no KMS rights")
project_bindings = [line for line in runnable if line.startswith("gcloud projects add-iam-policy-binding")]
bound = sorted((re.search(r"--member=serviceAccount:([^@]+)@", line).group(1), re.search(r"roles/(\w+)", line).group(1),
                "--condition=None" not in line and "--condition=" in line) for line in project_bindings)
if bound != [("meerkat-fast-launcher", "meerkatFastRunnerLauncher", True), ("meerkat-fast-reaper", "meerkatFastRunnerDelete", True),
             ("meerkat-fast-reaper", "meerkatFastRunnerList", False), ("meerkat-fast-teardown", "meerkatFastRunnerDelete", True)]:
    fail(f"project bindings must be the conditioned launcher and delete roles plus the reaper's list-only role: {bound}")
for line in project_bindings:
    if "--condition=None" not in line and ("instances/meerkat-fast-" not in line or "/operations/" not in line):
        fail(f"a conditioned project binding is not bound to the pool names: {line[:160]}")
launcher_binding = next((line for line in project_bindings if "meerkat-fast-launcher@" in line), "")
if (f'resource.name == "projects/{LAUNCHER.PROJECT}/regions/{LAUNCHER.REGION}/subnetworks/{LAUNCHER.SUBNET}"' not in launcher_binding
        or "/global/images/" in launcher_binding):
    fail("the launcher binding must match the exact subnet, with no image clause")
roles_created = {re.search(r"roles create (\w+)", line).group(1): line for line in runnable if "gcloud iam roles create" in line}
if "compute.zoneOperations.get" not in roles_created.get("meerkatFastRunnerLauncher", ""):
    fail("the launcher role must let gcloud poll the zone operations of its creates and deletes")
if "--permissions=compute.instances.list" not in roles_created.get("meerkatFastRunnerList", "") or "," in roles_created.get("meerkatFastRunnerList", "").split("--permissions=")[-1]:
    fail("the reaper's unconditioned role must allow listing only")
if "compute.instances.create" in roles_created.get("meerkatFastRunnerDelete", "") or "compute.instances.delete" not in roles_created.get("meerkatFastRunnerDelete", ""):
    fail("the delete role must delete and never create")
for line in commands:
    for forbidden in ("roles/owner", "roles/editor", "roles/compute.admin", "roles/compute.instanceAdmin",
                      "roles/iam.serviceAccountTokenCreator", "roles/storage.admin", "allUsers", "allAuthenticatedUsers"):
        if forbidden in line:
            fail(f"approval command grants {forbidden}: {line}")
    if "--member=serviceAccount:meerkat-fast-runner@" in line and "roles/cloudkms.cryptoKeyDecrypter" not in line:
        fail(f"the runner identity gets more than the KMS decrypter: {line}")
    if "roles/storage.objectUser" in line and "sccache_check" not in line:
        fail(f"cache writers may create but never overwrite objects: {line}")
buckets = sorted(set(re.findall(r"gs://(meerkat-fast-cache-[a-z0-9-]+)", "\n".join(runnable))))
if buckets != sorted(f"meerkat-fast-cache-ci-{short}-{LAUNCHER.PROJECT}" for short in ("core", "meta")):
    fail(f"one CI cache bucket per repository: {buckets}")
for short in ("core", "meta"):
    bucket = f"meerkat-fast-cache-ci-{short}-{LAUNCHER.PROJECT}"
    sa = f"meerkat-fast-cache-{short}@"
    granted = [line for line in runnable if f"gs://{bucket} " in line + " " and "add-iam-policy-binding" in line]
    if any(sa not in line for line in granted) or not any("objectCreator" in line for line in granted) or not any("objectViewer" in line for line in granted):
        fail(f"bucket {bucket} must be writable (create only) and readable by {sa} alone")
for repo_id, short in ((CORE_ID, "core"), (META_ID, "meta")):
    if not any(f"meerkat-fast-cache-{short}@" in line and f"attribute.fast_role/ci-{repo_id}" in line for line in runnable):
        fail(f"the {short} cache account must trust only that repository's CI lanes")
for sa, role_name in (("meerkat-fast-launcher@", "launcher"), ("meerkat-fast-teardown@", "teardown")):
    if not any(sa in line and f"attribute.fast_role/{role_name}" in line and "workloadIdentityUser" in line for line in runnable):
        fail(f"{sa} must trust only the {role_name} role")
firewalls = [line for line in runnable if line.startswith("gcloud compute firewall-rules")]
if len(firewalls) != 1 or "--action=DENY" not in firewalls[0] or "--direction=INGRESS" not in firewalls[0] or "--priority=0" not in firewalls[0]:
    fail(f"the network must have exactly one firewall rule, an explicit priority-0 deny of all ingress: {firewalls}")
if any("--action=ALLOW" in line or "--allow" in line for line in commands):
    fail("an approval command opens ingress")
for needle in ("--enable-private-ip-google-access", "routers nats create", "--attribute-condition=", "gcloud run jobs create meerkat-fast-reaper",
               "gcloud scheduler jobs create http meerkat-fast-reaper", "gcloud monitoring channels create", "monitoring policies create"):
    if not any(needle in line for line in commands):
        fail(f"approval commands lack {needle!r}")
ok_since(_s17, "approval commands: a dedicated owner-only project with an IAM audit and nothing in the shared project, the "
         "App key imported from a macOS RAM disk into a non-exportable HSM signing key only the launcher can use, only "
         "name-conditioned launcher and delete roles plus a list-only reaper role at project level, a delete-only "
         "teardown identity without KMS, exact subnet, per-repository create-only cache buckets, a digest-pinned "
         "reaper with an alert on failed executions, and one deny-all ingress rule")

if FAILURES:
    print(f"{len(FAILURES)} failure(s)")
    sys.exit(1)
print("PASS: owner-only fast-runner pool contract")
PY
