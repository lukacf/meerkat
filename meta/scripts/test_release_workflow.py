#!/usr/bin/env python3
"""Offline behavioral contracts for release protocol v1 and registry publication."""

import hashlib
import importlib.util
import itertools
import json
import os
import re
import shutil
import subprocess
import sys
import unittest
import uuid
from contextlib import contextmanager
from pathlib import Path


ROOT = Path(__file__).resolve().parents[1]
RELEASE_WORKFLOW = ROOT / ".github/workflows/release.yml"
_CANDIDATE_SPEC = importlib.util.spec_from_file_location("release_candidate", ROOT / "scripts/release-candidate.py")
release_candidate = importlib.util.module_from_spec(_CANDIDATE_SPEC)
_CANDIDATE_SPEC.loader.exec_module(release_candidate)


@contextmanager
def fixture_directory():
    """Keep shell/Node fixtures inside the checkout, never in system scratch."""
    directory = ROOT / f".release-workflow-fixture-{uuid.uuid4().hex}"
    directory.mkdir()
    try:
        yield directory
    finally:
        shutil.rmtree(directory)


def typescript_publish_script() -> str:
    """Extract the TypeScript publication shell from the workflow verbatim."""
    script_lines = step_script("Publish TypeScript SDK", "publish_sdk_packages").splitlines()
    # GitHub resolves this expression before invoking the shell. Tests expose the
    # resolved value through an environment variable instead.
    script_lines = [
        'DRY_RUN="${REGISTRY_DRY_RUN:-false}"'
        if line.startswith('DRY_RUN="${{')
        else line
        for line in script_lines
    ]
    # Only replace scratch allocation, not registry/error handling. The real
    # workflow uses system mktemp; offline fixtures must stay in the checkout.
    script_lines = [
        '  VIEW_STDERR="$NPM_VIEW_STDERR"' if line.strip() == "VIEW_STDERR=$(mktemp)" else line
        for line in script_lines
    ]
    return "\n".join(script_lines)


def job_block(name: str) -> str:
    """The workflow text for exactly one job, bounded by the next job header.

    Slicing to end-of-file is only safe for the last job. `publish_sdk_packages`
    is followed by `publish_registries`, whose `if` already contains the very
    strings this file asserts about the SDK gate, so an unbounded slice would
    pass against unmodified code.
    """
    lines = RELEASE_WORKFLOW.read_text().splitlines()
    start = lines.index(f"  {name}:")
    for offset, line in enumerate(lines[start + 1 :], start=start + 1):
        if re.fullmatch(r"  [A-Za-z_][A-Za-z0-9_-]*:", line):
            return "\n".join(lines[start:offset])
    return "\n".join(lines[start:])


def job_condition(name: str) -> str:
    """The raw `if:` expression of one job, with block scalar folding undone."""
    block = job_block(name).splitlines()
    matches = [i for i, line in enumerate(block) if line.startswith("    if:")]
    if not matches:
        return "true"
    if len(matches) != 1:
        raise AssertionError(f"multiple conditions for {name}")
    start = matches[0]
    value = block[start].split(":", 1)[1].strip()
    if value not in (">-", ">", "|", "|-"):
        return value
    body: list[str] = []
    for line in block[start + 1 :]:
        if not line.startswith("      "):
            break
        body.append(line.strip())
    return " ".join(body)


def evaluate_condition(expression: str, *, statuses=(), run_cancelled=False, **context) -> bool:
    """Evaluate a GitHub `if:` expression under an explicit context.

    String assertions cannot tell a conjunction from a disjunction, which is the
    entire defect class here, so the gate is exercised rather than pattern
    matched. Any name the expression uses that the caller did not supply raises,
    so a renamed input fails loudly instead of silently reading as falsey.
    `run_cancelled` models a run cancelled before the job starts. GitHub's
    cancelled() reads the run, not the needs ("Returns true if the workflow
    was canceled"), so a need cancelled on its own, by a timeout, does not
    make it true. A cancelled run makes success() false too; always() stays
    true.
    """
    expression = expression.strip()
    if expression.startswith("${{") and expression.endswith("}}"):
        expression = expression[3:-2].strip()
    functions = {
        "always": lambda: True,
        "success": lambda: not run_cancelled and all(status == "success" for status in statuses),
        "failure": lambda: "failure" in statuses,
        "cancelled": lambda: run_cancelled,
        "startsWith": lambda value, prefix: value.startswith(prefix),
        "contains": lambda value, part: part in value,
    }
    tokens = re.findall(
        r"\s+|'(?:[^']|'')*'|(?:github|needs|inputs)(?:\.[A-Za-z_][A-Za-z0-9_-]*)+"
        r"|[A-Za-z_][A-Za-z0-9_]*|&&|\|\||==|!=|[!(),]", expression
    )
    if "".join(tokens) != expression:
        raise AssertionError(f"unsupported expression syntax: {expression}")
    translated = []
    operators = {"&&": " and ", "||": " or ", "!": " not ",
                 "true": "True", "false": "False"}
    for token in tokens:
        if token.startswith(("github.", "needs.", "inputs.")):
            name = token.replace(".", "_").replace("-", "_")
            if name not in context:
                raise AssertionError(f"condition references unsupplied name: {name}")
            translated.append(name)
        elif re.fullmatch(r"[A-Za-z_][A-Za-z0-9_]*", token):
            if token not in functions and token not in operators:
                raise AssertionError(f"unsupported expression identifier: {token}")
            translated.append(operators.get(token, token))
        else:
            translated.append(operators.get(token, token))
    return bool(eval("".join(translated).strip(), {"__builtins__": {}}, {**context, **functions}))


def job_needs(name: str) -> list[str]:
    lines = [line for line in job_block(name).splitlines() if line.startswith("    needs:")]
    if not lines:
        return []
    if len(lines) != 1 or not re.fullmatch(r"    needs: \[[a-z_, ]+\]", lines[0]):
        raise AssertionError(f"unsupported needs declaration for {name}: {lines}")
    return [value.strip() for value in lines[0].split("[", 1)[1][:-1].split(",")]


def job_ancestors(name: str) -> list[str]:
    """Every job upstream of `name` through `needs`, transitively, in workflow order.

    GitHub evaluates a job's status functions over this whole chain, not only
    over the declared needs, and so does the implicit success() of a condition
    without a status function. A job with no status function is skipped when
    any ancestor was skipped, failed or cancelled, even if every declared need
    succeeded: v0.9.2 promotion run 38983785773 skipped release_validate, and
    every job after it, because build_binaries (a need of require_ci_green,
    skipped outside candidate mode) is an ancestor, although both of
    release_validate's own needs succeeded.
    """
    seen: set[str] = set()
    pending = list(job_needs(name))
    while pending:
        job = pending.pop()
        if job not in seen:
            seen.add(job)
            pending.extend(job_needs(job))
    if name in seen:
        raise AssertionError(f"{name} depends on itself")
    return sorted(seen, key=lambda job: (JOBS.index(job) if job in JOBS else len(JOBS), job))


def job_runs(name: str, *, run_cancelled=False, **context) -> bool:
    """Apply GitHub's status semantics: status functions, explicit or the
    implicit success() of a condition without one, read every ancestor."""
    expression = job_condition(name)
    dependencies = job_needs(name)
    references = set(re.findall(r"\bneeds\.([A-Za-z_][A-Za-z0-9_]*)\.", expression))
    if references - set(dependencies):
        raise AssertionError(f"{name} references undeclared needs: {references - set(dependencies)}")
    statuses = []
    for dependency in job_ancestors(name):
        key = f"needs_{dependency}_result"
        if key not in context:
            raise AssertionError(f"unsupplied dependency status: {key}")
        statuses.append(context[key])
    # Evaluate first even if implicit success is false: missing contexts must
    # never disappear behind a skipped dependency or short-circuited branch.
    condition = evaluate_condition(expression, statuses=statuses, run_cancelled=run_cancelled, **context)
    explicit_status = re.search(r"\b(?:always|success|failure|cancelled)\s*\(", expression)
    return condition and (bool(explicit_status)
                          or (not run_cancelled and all(s == "success" for s in statuses)))


# Dependency order: a candidate's build legs overlap exact-main CI, so they
# precede the gate that collection waits on.
JOBS = (
    "resolve_release", "build_binaries", "require_ci_green", "release_validate",
    "collect_candidate", "verify_release_assets", "publish_github_release",
    "publish_sdk_packages", "verify_crate_packages", "publish_registries",
)
WORK_JOBS = (
    "build_binaries", "collect_candidate", "verify_release_assets", "publish_github_release",
    "publish_sdk_packages", "verify_crate_packages", "publish_registries",
)


def protocol_context(mode="promote", publish=True, dry=False, *, event="workflow_dispatch",
                     tag="v0.8.31"):
    return {
        "github_event_name": event,
        "github_ref": "refs/tags/v0.8.31" if event == "push" else "refs/heads/main",
        "inputs_release_mode": mode,
        "inputs_publish_release_packages": publish,
        "inputs_registry_dry_run": dry,
        "inputs_release_tag": tag,
        "github_event_inputs_publish_release_packages": str(publish).lower(),
        "github_event_inputs_registry_dry_run": str(dry).lower(),
        "github_event_inputs_release_tag": tag,
        "needs_resolve_release_outputs_mode": mode,
        "needs_resolve_release_outputs_publish_packages": str(publish).lower(),
        "needs_resolve_release_outputs_dry_run": str(dry).lower(),
        "needs_resolve_release_outputs_tag": tag,
        **{f"needs_{job}_result": "success" for job in JOBS},
    }


def release_results(context, failures=None):
    """Run the actual job DAG with successful steps unless explicitly refused."""
    context = dict(context)
    results = {}
    for job in JOBS:
        result = (failures or {}).get(job, "success") if job_runs(job, **context) else "skipped"
        results[job] = result
        context[f"needs_{job}_result"] = result
    return results


def ci_gate_script() -> str:
    """The require_ci_green github-script body, extracted verbatim."""
    lines = step_block("Refuse unless exact-main CI is already green for this commit",
                       "require_ci_green").splitlines()
    start = lines.index("          script: |")
    body: list[str] = []
    for line in lines[start + 1 :]:
        if line and not line.startswith("            "):
            break
        body.append(line[12:] if line else "")
    return "\n".join(body)


def run_ci_gate(runs, release_sha="deadbeef", *, ci_bypass="false", raw_env=None):
    """Execute the gate under node against a stubbed workflow-run listing.

    The gate's decision lives in JavaScript, so asserting on its source text
    cannot distinguish "requires success" from "requires completion" - exactly
    the mutation that a string assertion lets through. This runs the real body.
    """
    harness = """
const RUNS = %s;
const calls = [];
const core = {
  info() {},
  setFailed(message) { calls.push({ kind: "setFailed", message }); },
  setOutput(name, value) { calls.push({ kind: "setOutput", name, value }); },
};
const context = { repo: { owner: "lukacf", repo: "meerkat-meta" } };
const github = {
  rest: { actions: { listWorkflowRuns: async (args) => {
    calls.push({ kind: "listWorkflowRuns", args });
    return { data: { workflow_runs: RUNS } };
  } } },
};
process.env.RELEASE_SHA = %s;
process.env.CI_BYPASS = %s;
Object.assign(process.env, %s);
(async () => {
%s
})().then(() => console.log(JSON.stringify(calls)));
""" % (
        json.dumps(runs),
        json.dumps(release_sha),
        json.dumps(ci_bypass),
        json.dumps(raw_env or {}),
        ci_gate_script(),
    )
    with fixture_directory() as tmp:
        script = Path(tmp) / "gate.mjs"
        script.write_text(harness)
        out = subprocess.run(
            ["node", str(script)], capture_output=True, text=True, check=True
        )
    return json.loads(out.stdout.strip().splitlines()[-1])


def workflow_permissions_without_comments() -> str:
    """The permissions block with comment lines removed.

    Asserting on raw text matched the explanatory comment above the key, so
    deleting `actions: read` left the test green. The prose is not the config.
    """
    head = RELEASE_WORKFLOW.read_text().split("jobs:", 1)[0]
    return "\n".join(
        line for line in head.splitlines() if not line.strip().startswith("#")
    )


PORTABILITY_SCRIPT = ROOT / "scripts/check-linux-release-binary-portability.sh"

# glibc shipped by each Debian release the container expression may name. The
# gate's default floor must equal the entry for the image in use; an image move
# to a codename missing here fails the parity test until the map is extended,
# which is the moment to decide the new floor deliberately.
CONTAINER_GLIBC = {"bullseye": "2.31", "bookworm": "2.36", "trixie": "2.41"}

# readelf --dyn-syms output shapes. GLIBC_2.4 (__stack_chk_fail) is in nearly
# every binary and sorts ABOVE "2.31" as text, below it as a version.
DYNSYMS_WITHIN_FLOOR = """\
Symbol table '.dynsym' contains 3 entries:
   Num:    Value          Size Type    Bind   Vis      Ndx Name
     1: 0000000000000000     0 FUNC    GLOBAL DEFAULT  UND memcpy@GLIBC_2.14 (2)
     2: 0000000000000000     0 FUNC    GLOBAL DEFAULT  UND __stack_chk_fail@GLIBC_2.4 (3)
     3: 0000000000000000     0 FUNC    GLOBAL DEFAULT  UND gettid@GLIBC_2.30 (4)
"""
# The v0.8.30 shape: std's pidfd spawn path bound to the runner's glibc 2.39.
DYNSYMS_ABOVE_FLOOR = DYNSYMS_WITHIN_FLOOR + (
    "     4: 0000000000000000     0 FUNC    WEAK   DEFAULT  UND pidfd_spawnp@GLIBC_2.39 (5)\n"
)
DYNAMIC_RUSTLS = """\
Dynamic section at offset 0x1000 contains 3 entries:
  Tag        Type                         Name/Value
 0x0000000000000001 (NEEDED)             Shared library: [libgcc_s.so.1]
 0x0000000000000001 (NEEDED)             Shared library: [libm.so.6]
 0x0000000000000001 (NEEDED)             Shared library: [libc.so.6]
"""
DYNAMIC_OPENSSL = DYNAMIC_RUSTLS + (
    " 0x0000000000000001 (NEEDED)             Shared library: [libssl.so.3]\n"
    " 0x0000000000000001 (NEEDED)             Shared library: [libcrypto.so.3]\n"
)


def step_block(step_name: str, job: str = "build_binaries") -> str:
    """The text of exactly one step, bounded by the next `- name:`."""
    lines = job_block(job).splitlines()
    start = lines.index(f"      - name: {step_name}")
    for offset, line in enumerate(lines[start + 1 :], start=start + 1):
        if line.startswith("      - name: "):
            return "\n".join(lines[start:offset])
    return "\n".join(lines[start:])


def step_script(step_name: str, job: str = "build_binaries") -> str:
    """One step's `run: |` body, de-indented, verbatim."""
    lines = step_block(step_name, job).splitlines()
    run = lines.index("        run: |")
    body: list[str] = []
    for line in lines[run + 1 :]:
        if line and not line.startswith("          "):
            break
        body.append(line[10:] if line else "")
    return "\n".join(body)


def scalar(block: str, key: str, indent: str) -> str:
    """The raw value of the single `<indent><key>: value` line in a block."""
    matches = [line for line in block.splitlines() if line.startswith(f"{indent}{key}: ")]
    if len(matches) != 1:
        raise AssertionError(f"expected exactly one `{key}` line, found {len(matches)}")
    return matches[0].split(": ", 1)[1]


def matrix_targets(job: str = "build_binaries") -> list[str]:
    """Every build leg's target: the resolver emits one tier's legs from
    release-candidate.py BUILD_LEGS as the build_binaries matrix."""
    assert job == "build_binaries"
    return [leg["target"] for leg in release_candidate.BUILD_LEGS]


def evaluate_expression(expression: str, **context):
    """Evaluate a GitHub expression that yields a VALUE, under an explicit context.

    `evaluate_condition` collapses to bool. The `container:` image and the cache
    `key:` are strings built with the `cond && 'a' || 'b'` idiom, whose result
    is an operand, and Python's `and`/`or` return operands the same way. Any
    `matrix.*` name the caller did not supply raises rather than reading as ''.
    """
    translated = expression.replace("&&", " and ").replace("||", " or ")
    translated = re.sub(
        r"\bmatrix\.([A-Za-z_][A-Za-z0-9_-]*)",
        lambda m: "matrix_" + m.group(1).replace("-", "_"),
        translated,
    )
    translated = re.sub(r"contains\(([^,]+),\s*('[^']*')\)", r"(\2 in \1)", translated)
    missing = {
        name for name in re.findall(r"\bmatrix_[A-Za-z0-9_]+", translated) if name not in context
    }
    if missing:
        raise AssertionError(f"expression references unsupplied names: {sorted(missing)}")
    return eval(translated, {"__builtins__": {}}, dict(context))


def render_template(template: str, **context) -> str:
    """Substitute every `${{ expr }}` in a YAML scalar with its evaluated value."""
    return re.sub(
        r"\$\{\{\s*(.*?)\s*\}\}",
        lambda m: str(evaluate_expression(m.group(1), **context)),
        template,
    )


def run_portability_check(dynsyms: str, dynamic: str, *, floor: str | None = None,
                          binaries: list[str] | None = None) -> subprocess.CompletedProcess[str]:
    """Run the real gate against a fake `readelf`.

    The ELF the gate reads is the release artifact, which no test can build,
    and macOS has no readelf at all, so the readelf OUTPUT is the fixture: the
    script's decision is under test, not binutils. Only `rpc_gateway` exists
    on disk; other names in `binaries` exercise the missing-file path.
    """
    with fixture_directory() as tmp:
        root = Path(tmp)
        bin_dir = root / "bin"
        bin_dir.mkdir()
        fake = bin_dir / "readelf"
        fake.write_text(
            """#!/usr/bin/env bash
set -euo pipefail
case "$1" in
  --dyn-syms) printf '%s' "$FAKE_READELF_DYNSYMS" ;;
  -d) printf '%s' "$FAKE_READELF_DYNAMIC" ;;
  *) echo "fake readelf: unexpected arguments: $*" >&2; exit 99 ;;
esac
"""
        )
        fake.chmod(0o755)
        (root / "rpc_gateway").write_bytes(b"\x7fELF fixture")
        env = os.environ.copy()
        env["PATH"] = f"{bin_dir}:{env['PATH']}"
        env["FAKE_READELF_DYNSYMS"] = dynsyms
        env["FAKE_READELF_DYNAMIC"] = dynamic
        env.pop("META_GLIBC_FLOOR", None)
        if floor is not None:
            env["META_GLIBC_FLOOR"] = floor
        names = ["rpc_gateway"] if binaries is None else binaries
        return subprocess.run(
            [str(PORTABILITY_SCRIPT), *(str(root / name) for name in names)],
            env=env,
            capture_output=True,
            text=True,
        )


def run_portability_step(target: str, *, gate_status: int) -> tuple[subprocess.CompletedProcess[str], list[str]]:
    """Execute the `Check Linux binary portability` step body under bash.

    A fake gate script records what it was asked to check and exits with
    `gate_status`, so the test observes which binaries the step covers and
    whether a refusal propagates, rather than pattern-matching the YAML.
    """
    with fixture_directory() as tmp:
        root = Path(tmp)
        scripts = root / "scripts"
        scripts.mkdir()
        log = root / "calls.log"
        fake = scripts / "check-linux-release-binary-portability.sh"
        fake.write_text(
            '#!/usr/bin/env bash\nprintf "%s\\n" "$@" >> "$FAKE_GATE_LOG"\nexit "$FAKE_GATE_STATUS"\n'
        )
        fake.chmod(0o755)
        env = os.environ.copy()
        env.update({"TARGET": target, "FAKE_GATE_LOG": str(log), "FAKE_GATE_STATUS": str(gate_status)})
        result = subprocess.run(
            ["bash", "-c", step_script("Check Linux binary portability")],
            cwd=root,
            env=env,
            capture_output=True,
            text=True,
        )
        checked = log.read_text().splitlines() if log.exists() else []
    return result, checked


class NpmReleaseWorkflowTests(unittest.TestCase):
    def setUp(self):
        self.fixtures = fixture_directory()
        self.root = self.fixtures.__enter__()
        self.addCleanup(self.fixtures.__exit__, None, None, None)
        self.bin = self.root / "bin"
        self.sdk = self.root / "sdk/typescript"
        self.bin.mkdir()
        self.sdk.mkdir(parents=True)
        (self.sdk / "package.json").write_text(
            json.dumps({"name": "@rkat/meta-sdk", "version": "0.8.0"}) + "\n"
        )
        self.log = self.root / "npm.log"
        npm = self.bin / "npm"
        npm.write_text(
            """#!/usr/bin/env bash
set -euo pipefail
command="$1"
shift
printf '%s\\n' "$command $*" >> "$FAKE_NPM_LOG"
case "$command" in
  install|config|run)
    exit 0
    ;;
  view)
    case "$FAKE_NPM_VIEW" in
      exact)
        printf '%s\\n' "$FAKE_NPM_VERSION"
        ;;
      mismatch)
        printf '%s\\n' "9.9.9"
        ;;
      missing)
        echo "npm error code E404" >&2
        echo "npm error 404 No match found for version 0.8.0" >&2
        exit 1
        ;;
      auth)
        echo "npm error code E401" >&2
        echo "npm error Unable to authenticate" >&2
        exit 1
        ;;
      network)
        echo "npm error code EAI_AGAIN" >&2
        exit 1
        ;;
    esac
    ;;
  publish)
    if [[ "${FAKE_NPM_PUBLISH_STATUS:-0}" != "0" ]]; then
      echo "npm error publish failed" >&2
      exit "$FAKE_NPM_PUBLISH_STATUS"
    fi
    ;;
esac
"""
        )
        npm.chmod(0o755)

    def run_publish(
        self,
        view: str,
        *,
        dry_run: bool = False,
        publish_status: int = 0,
    ) -> subprocess.CompletedProcess[str]:
        env = os.environ.copy()
        env.update(
            {
                "PATH": f"{self.bin}:{env['PATH']}",
                "NODE_AUTH_TOKEN": "test-token",
                "REGISTRY_DRY_RUN": "true" if dry_run else "false",
                "FAKE_NPM_LOG": str(self.log),
                "FAKE_NPM_VIEW": view,
                "FAKE_NPM_VERSION": "0.8.0",
                "FAKE_NPM_PUBLISH_STATUS": str(publish_status),
                "NPM_VIEW_STDERR": str(self.root / "npm.stderr"),
            }
        )
        return subprocess.run(
            ["bash", "-c", typescript_publish_script()],
            cwd=self.sdk,
            env=env,
            capture_output=True,
            text=True,
        )

    def npm_commands(self) -> list[str]:
        return self.log.read_text().splitlines()

    def test_exact_published_version_is_skipped(self):
        result = self.run_publish("exact")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("@rkat/meta-sdk@0.8.0: already published, skipping", result.stdout)
        self.assertTrue(any(command.startswith("view ") for command in self.npm_commands()))
        self.assertFalse(any(command.startswith("publish ") for command in self.npm_commands()))

    def test_explicit_exact_version_404_publishes(self):
        result = self.run_publish("missing")

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(any(command.startswith("publish ") for command in self.npm_commands()))

    def test_registry_auth_failure_is_not_treated_as_missing(self):
        result = self.run_publish("auth")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("E401", result.stderr)
        self.assertFalse(any(command.startswith("publish ") for command in self.npm_commands()))

    def test_network_failure_is_not_treated_as_missing(self):
        result = self.run_publish("network")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("EAI_AGAIN", result.stderr)
        self.assertFalse(any(command.startswith("publish ") for command in self.npm_commands()))

    def test_dry_run_never_queries_or_uploads_a_public_version(self):
        result = self.run_publish("auth", dry_run=True)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertIn("publish --access public --dry-run", self.npm_commands())
        self.assertFalse(any(command.startswith("view ") for command in self.npm_commands()))
        self.assertNotIn("publish --access public", self.npm_commands())

    def test_unexpected_exact_query_response_fails_closed(self):
        result = self.run_publish("mismatch")

        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unexpected version", result.stderr)
        self.assertFalse(any(command.startswith("publish ") for command in self.npm_commands()))

    def test_genuine_publish_failure_is_preserved(self):
        result = self.run_publish("missing", publish_status=23)

        self.assertEqual(result.returncode, 23)
        self.assertIn("publish failed", result.stderr)


class RegistryReadbackWorkflowTests(unittest.TestCase):
    def test_exact_registry_readback_runs_after_all_publish_steps(self):
        workflow = RELEASE_WORKFLOW.read_text()
        verification = "      - name: Verify exact packages are public on every registry"

        self.assertIn(verification, workflow)
        self.assertLess(workflow.index("      - name: Publish Rust crate"), workflow.index(verification))
        self.assertLess(workflow.index("      - name: Publish Python SDK"), workflow.index(verification))
        self.assertLess(workflow.index("      - name: Publish TypeScript SDK"), workflow.index(verification))
        self.assertIn("python scripts/verify-published-registries.py --version", workflow)
        readback = step_block("Verify exact packages are public on every registry", "publish_registries")
        condition = scalar(readback, "if", "        ")
        for dry in ("false", "true", ""):
            with self.subTest(dry=dry):
                self.assertEqual(
                    evaluate_condition(condition, needs_resolve_release_outputs_dry_run=dry),
                    dry == "false",
                )
        self.assertIn("publish_sdk_packages", job_needs("publish_registries"))
        self.assertIn(
            "(needs.publish_sdk_packages.result == 'success' || "
            "needs.publish_sdk_packages.result == 'failure')",
            job_condition("publish_registries"),
        )
        # The crates no longer wait for SDK success, so this readback is what
        # keeps a missing registry failing the run: it stays in the crate job,
        # gated only on a real publication, and runs the all-registry verifier.
        self.assertEqual(condition, "needs.resolve_release.outputs.dry_run == 'false'")
        self.assertIn('python scripts/verify-published-registries.py --version "$VERSION"',
                      step_script("Verify exact packages are public on every registry",
                                  "publish_registries"))
        self.assertNotIn("continue-on-error:", job_block("publish_registries"))


class RegistryPublicationShellTests(unittest.TestCase):
    """No package tooling runs: the workflow shell drives recording stubs."""

    VERIFIED = {"VERIFIED_EXTENSION_SHA256": "e" * 64, "VERIFIED_META_SHA256": "f" * 64}

    def run_script(self, step, job, *, dry=False, status=0, output="published", verify_status=0):
        with fixture_directory() as root:
            bin_dir = root / "bin"
            scripts = root / "scripts"
            bin_dir.mkdir()
            scripts.mkdir()
            log = root / "commands.jsonl"
            stub = (
                f"#!{sys.executable}\n"
                "import json, os, sys\n"
                "with open(os.environ['COMMAND_LOG'], 'a') as stream:\n"
                "    stream.write(json.dumps(sys.argv) + '\\n')\n"
                "if sys.argv[1:2] == ['-c']:\n"
                "    print('0.8.31')\n"
                "elif sys.argv[1:2] == ['scripts/verify-published-registries.py']:\n"
                "    sys.exit(int(os.environ['VERIFY_STATUS']))\n"
                "else:\n"
                "    print(os.environ['COMMAND_OUTPUT'])\n"
                "    sys.exit(int(os.environ['COMMAND_STATUS']))\n"
            )
            for executable in (bin_dir / "python", scripts / "repo-cargo"):
                executable.write_text(stub)
                executable.chmod(0o755)
            script = step_script(step, job).replace(
                "${{ needs.resolve_release.outputs.dry_run }}", str(dry).lower()
            )
            result = subprocess.run(
                ["bash", "-c", script], cwd=root, capture_output=True, text=True,
                env={**os.environ, "PATH": f"{bin_dir}:{os.environ['PATH']}",
                     "COMMAND_LOG": str(log), "COMMAND_STATUS": str(status),
                     "COMMAND_OUTPUT": output, "VERIFY_STATUS": str(verify_status),
                     "RELEASE_VERSION": "0.8.31", **self.VERIFIED},
            )
            commands = ([json.loads(line)[1:] for line in log.read_text().splitlines()]
                        if log.exists() else [])
            return result, commands

    def test_python_rehearsal_builds_and_checks_but_never_uploads(self):
        for dry in (False, True):
            with self.subTest(dry=dry):
                result, commands = self.run_script("Publish Python SDK", "publish_sdk_packages", dry=dry)
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertIn(["-m", "build"], commands)
                self.assertIn(["-m", "twine", "check", "dist/*"], commands)
                self.assertEqual(
                    ["-m", "twine", "upload", "--skip-existing", "dist/*"] in commands, not dry
                )

    def test_crate_rehearsal_passes_dry_run_and_locked_to_the_real_step(self):
        result, commands = self.run_script("Publish Rust crate", "publish_registries", dry=True)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(commands, [["publish", "-p", "meerkat-meta", "--locked", "--no-verify", "--dry-run"]])

    def test_crate_package_exists_is_success_but_other_errors_keep_their_status(self):
        for output, status, expected in (
            ("published", 0, 0), ("already exists", 101, 0),
            ("already uploaded", 101, 0), ("unauthorized", 23, 23), ("network unavailable", 7, 7),
        ):
            with self.subTest(output=output):
                result, commands = self.run_script(
                    "Publish Rust crate", "publish_registries", status=status, output=output
                )
                self.assertEqual(result.returncode, expected, result.stdout + result.stderr)
                self.assertEqual(commands[0], ["publish", "-p", "meerkat-meta", "--locked", "--no-verify"])
                if output.startswith("already"):
                    # A version already on crates.io must be the verified bytes.
                    self.assertEqual(commands[1:], [[
                        "scripts/verify-published-registries.py", "--version", "0.8.31", "--only-crates",
                        "--crate-sha256", "meerkat-meta=" + "f" * 64]])
                else:
                    self.assertEqual(commands[1:], [])

    def test_an_already_published_crate_with_other_bytes_fails_the_step(self):
        for step, crate in (("Publish Rust crate", "meerkat-meta"),
                            ("Publish extension document crate", "meta-extension-state")):
            with self.subTest(step=step):
                result, commands = self.run_script(step, "publish_registries", status=101,
                                                   output="crate version already exists", verify_status=1)
                self.assertNotEqual(result.returncode, 0)
                self.assertIn("--only-crates", commands[-1])
                self.assertIn(crate, commands[-1][-1])

    def test_publication_packages_again_and_matches_the_verified_bytes_before_upload(self):
        bind = step_block("Bind the crates to the verified bytes", "publish_registries")
        block = job_block("publish_registries")
        self.assertLess(block.index("- name: Bind the crates to the verified bytes"),
                        block.index("- name: Publish extension document crate"))
        script = step_script("Bind the crates to the verified bytes", "publish_registries")
        self.assertIn("scripts/repo-cargo package -p meta-extension-state -p meerkat-meta --locked --no-verify",
                      script)
        for variable in ("VERIFIED_EXTENSION_SHA256: ${{ needs.verify_crate_packages.outputs.extension_sha256 }}",
                         "VERIFIED_META_SHA256: ${{ needs.verify_crate_packages.outputs.meta_sha256 }}"):
            self.assertIn(variable, block)
        self.assertNotIn("CARGO_REGISTRY_TOKEN", bind)
        with fixture_directory() as root:
            package = root / "target/package"
            package.mkdir(parents=True)
            scripts = root / "scripts"
            scripts.mkdir()
            (scripts / "repo-cargo").write_text("#!/usr/bin/env bash\nexit 0\n")
            (scripts / "repo-cargo").chmod(0o755)
            files = {}
            for crate in ("meta-extension-state", "meerkat-meta"):
                files[crate] = package / f"{crate}-0.8.31.crate"
                files[crate].write_bytes(crate.encode())
            sums = {crate: hashlib.sha256(path.read_bytes()).hexdigest() for crate, path in files.items()}
            env = {**os.environ, "CARGO_TARGET_DIR": str(root / "target"), "RELEASE_VERSION": "0.8.31",
                   "VERIFIED_EXTENSION_SHA256": sums["meta-extension-state"],
                   "VERIFIED_META_SHA256": sums["meerkat-meta"]}
            ok = subprocess.run(["bash", "-c", script], cwd=root, env=env, capture_output=True, text=True)
            self.assertEqual(ok.returncode, 0, ok.stderr)
            for name, value in (("VERIFIED_META_SHA256", "0" * 64), ("VERIFIED_EXTENSION_SHA256", "")):
                with self.subTest(mismatch=name):
                    bad = subprocess.run(["bash", "-c", script], cwd=root, env={**env, name: value},
                                         capture_output=True, text=True)
                    self.assertNotEqual(bad.returncode, 0)

    def test_crate_verification_records_the_verified_checksums(self):
        block = job_block("verify_crate_packages")
        self.assertIn("extension_sha256: ${{ steps.crates.outputs.extension_sha256 }}", block)
        self.assertIn("meta_sha256: ${{ steps.crates.outputs.meta_sha256 }}", block)
        verify = block.index("- name: Verify the packaged crates build")
        self.assertLess(verify, block.index("- name: Record the verified crate checksums"))

    def test_registry_readback_uses_workspace_version_and_preserves_refusals(self):
        for status in (0, 23):
            with self.subTest(status=status):
                result, commands = self.run_script(
                    "Verify exact packages are public on every registry", "publish_registries",
                    status=status, verify_status=status,
                )
                self.assertEqual(result.returncode, status, result.stderr)
                self.assertEqual(commands[-1],
                                 ["scripts/verify-published-registries.py", "--version", "0.8.31",
                                  "--crate-sha256", "meta-extension-state=" + "e" * 64,
                                  "--crate-sha256", "meerkat-meta=" + "f" * 64])
                self.assertIn('["workspace"]["package"]["version"]', commands[0][-1])


class DocsPublicationWorkflowTests(unittest.TestCase):
    """Documentation mirrors main; releases no longer publish docs."""

    PUBLISH_DOCS_WORKFLOW = RELEASE_WORKFLOW.with_name("publish-docs.yml")

    def test_release_workflow_no_longer_dispatches_documentation(self):
        workflow = RELEASE_WORKFLOW.read_text()
        self.assertNotIn("publish_docs:", workflow)
        self.assertNotIn("meta-release-published", workflow)
        self.assertNotIn("repos/lukacf/meerkat/dispatches", workflow)

    def test_docs_dispatch_fires_on_every_docs_push_to_main(self):
        workflow = self.PUBLISH_DOCS_WORKFLOW.read_text()
        self.assertIn("branches:\n      - main", workflow)
        self.assertIn('- "docs/**"', workflow)
        self.assertIn("workflow_dispatch:", workflow)
        self.assertIn("secrets.DOCS_PUBLISH_TOKEN", workflow)
        self.assertIn('event_type: "meta-docs-updated"', workflow)
        self.assertIn("sha: $sha", workflow)
        self.assertIn('ref: "main"', workflow)
        self.assertIn("SOURCE_SHA: ${{ github.sha }}", workflow)
        self.assertIn("repos/lukacf/meerkat/dispatches", workflow)
        self.assertNotIn("releases/latest", workflow)
        self.assertNotIn("RELEASE_TAG", workflow)

    def test_docs_dispatch_workflow_parses_when_yaml_is_available(self):
        try:
            import yaml
        except ImportError:  # pragma: no cover
            self.skipTest("PyYAML is not installed")
        parsed = yaml.safe_load(self.PUBLISH_DOCS_WORKFLOW.read_text())
        self.assertEqual(parsed[True]["push"]["branches"], ["main"])
        self.assertEqual(parsed[True]["push"]["paths"], ["docs/**"])
        self.assertEqual(parsed["permissions"], {"contents": "read"})
        self.assertEqual(list(parsed["jobs"]), ["dispatch"])


class ReleaseProtocolConditionsTests(unittest.TestCase):
    """Exercise raw conditions and actual dependency skipping, without a resolver.

    The helper suite owns input rejection. Artificial outputs here deliberately
    bypass it to prove the workflow's separate event/mode publication boundary.
    """

    def test_complete_event_mode_boolean_dag_matrix(self):
        for event, mode, publish, dry, tag in itertools.product(
            ("push", "workflow_dispatch"),
            ("tag", "candidate", "promote", "existing", "assets", "unknown", ""),
            (False, True), (False, True), ("", "v0.8.31"),
        ):
            with self.subTest(event=event, mode=mode, publish=publish, dry=dry, tag=tag):
                results = release_results(protocol_context(mode, publish, dry, event=event, tag=tag))
                expected = set()
                if event == "workflow_dispatch":
                    if mode == "candidate":
                        expected = {"build_binaries", "collect_candidate"}
                    elif mode == "promote":
                        expected = {"verify_release_assets"}
                        if not dry:
                            expected.add("publish_github_release")
                        if publish:
                            expected.update(("publish_sdk_packages", "verify_crate_packages",
                                             "publish_registries"))
                    elif mode == "existing" and publish:
                        expected = {"publish_sdk_packages", "verify_crate_packages", "publish_registries"}
                        if not dry:
                            expected.add("verify_release_assets")
                    elif mode == "assets" and not publish and not dry:
                        expected = {"publish_github_release"}
                # Tag/input validity belongs to resolve_release's tests. Here
                # all resolver outputs are trusted artificial values, including
                # an empty tag, so this tests raw job conditions independently.
                actual = {job for job in WORK_JOBS if results[job] == "success"}
                self.assertEqual(actual, expected)

    def test_tag_events_cannot_reach_any_work_job_with_artificial_promotion_outputs(self):
        for mode, publish, dry in itertools.product(
            ("tag", "candidate", "promote", "existing", "assets"),
            ("", False, True), ("", False, True),
        ):
            context = protocol_context(mode, publish, dry, event="push")
            for job in WORK_JOBS:
                with self.subTest(job=job, mode=mode, publish=publish, dry=dry):
                    self.assertFalse(job_runs(job, **context))

    def test_candidate_cannot_publish_even_with_all_upstreams_success_and_any_flags(self):
        for publish, dry in itertools.product((False, True, ""), repeat=2):
            for job in WORK_JOBS[2:]:
                with self.subTest(job=job, publish=publish, dry=dry):
                    self.assertFalse(job_runs(job, **protocol_context("candidate", publish, dry)))

    def test_assets_mode_allows_only_false_false_and_never_packages_or_docs(self):
        for publish, dry in itertools.product((False, True, ""), repeat=2):
            context = protocol_context("assets", publish, dry)
            for job in WORK_JOBS:
                with self.subTest(job=job, publish=publish, dry=dry):
                    self.assertEqual(
                        job_runs(job, **context),
                        job == "publish_github_release" and publish is False and dry is False,
                    )

    def test_failed_or_skipped_gate_cascades_through_every_lane(self):
        for mode, publish, dry, upstream, status in itertools.product(
            ("candidate", "promote", "existing", "assets"),
            (False, True), (False, True),
            ("resolve_release", "require_ci_green", "release_validate"),
            ("failure", "skipped", "cancelled"),
        ):
            with self.subTest(mode=mode, publish=publish, dry=dry, upstream=upstream, status=status):
                results = release_results(
                    protocol_context(mode, publish, dry), failures={upstream: status}
                )
                # A candidate's legs build beside exact-main CI; a failed CI
                # gate or validation still binds, attests and publishes nothing.
                overlap = {"build_binaries"} if mode == "candidate" and upstream != "resolve_release" else set()
                self.assertEqual({job for job in WORK_JOBS if results[job] == "success"}, overlap)

    def test_candidate_builds_overlap_ci_and_collection_refuses_without_green_ci(self):
        """The legs need only the resolver; the CI gate runs after every leg
        succeeded and before validation and collection, and refuses rather
        than waits (ExactMainCiGateTests run its script)."""
        self.assertEqual(job_needs("build_binaries"), ["resolve_release"])
        self.assertEqual(job_needs("require_ci_green"), ["resolve_release", "build_binaries"])
        self.assertEqual(job_needs("collect_candidate"),
                         ["resolve_release", "require_ci_green", "release_validate", "build_binaries"])
        for gate in ("failure", "cancelled", "skipped"):
            with self.subTest(gate=gate):
                results = release_results(protocol_context("candidate"), {"require_ci_green": gate})
                self.assertEqual(results["build_binaries"], "success")
                self.assertEqual(results["collect_candidate"], "skipped")
        # A failed leg never reaches the gate, so nothing is bound either.
        results = release_results(protocol_context("candidate"), {"build_binaries": "failure"})
        self.assertEqual((results["require_ci_green"], results["collect_candidate"]), ("skipped", "skipped"))

    def test_the_ci_gate_runs_after_candidate_builds_and_first_in_every_other_mode(self):
        self.assertIn("!cancelled()", job_condition("require_ci_green"))
        for mode in ("candidate", "promote", "existing", "assets", "tag"):
            for resolved, built in itertools.product(("success", "failure", "skipped", "cancelled"), repeat=2):
                context = protocol_context(mode, event="push" if mode == "tag" else "workflow_dispatch")
                context.update(needs_resolve_release_result=resolved, needs_build_binaries_result=built)
                expected = resolved == "success" and built == ("success" if mode == "candidate" else "skipped")
                with self.subTest(mode=mode, resolved=resolved, built=built):
                    self.assertEqual(job_runs("require_ci_green", **context), expected)

    def test_full_publication_stops_at_each_failed_boundary(self):
        for upstream, downstream in (
            ("verify_release_assets", ("publish_github_release", "publish_sdk_packages",
                                       "publish_registries")),
            ("publish_github_release", ("publish_sdk_packages", "publish_registries")),
            ("publish_sdk_packages", ("publish_registries",)),
            ("verify_crate_packages", ("publish_registries",)),
        ):
            for status in ("failure", "skipped", "cancelled"):
                with self.subTest(upstream=upstream, status=status):
                    results = release_results(protocol_context(), {upstream: status})
                    # A failed SDK registry no longer stops the crates; a
                    # skipped or cancelled SDK job still does.
                    ran = (upstream, status) == ("publish_sdk_packages", "failure")
                    for job in downstream:
                        self.assertEqual(results[job], "success" if ran else "skipped", job)
                    self.assertEqual(results["build_binaries"], "skipped")

    def test_actual_condition_success_guards_across_all_dependency_results(self):
        scenarios = (
            ("publish_github_release", "promote", True, False,
             {"release_validate", "verify_release_assets"}),
            ("publish_github_release", "assets", False, False, {"release_validate"}),
            ("publish_sdk_packages", "promote", True, False,
             {"release_validate", "verify_release_assets", "publish_github_release"}),
            ("publish_sdk_packages", "promote", True, True,
             {"release_validate", "verify_release_assets"}),
            ("publish_sdk_packages", "existing", True, False,
             {"release_validate", "verify_release_assets"}),
            ("publish_sdk_packages", "existing", True, True, {"release_validate"}),
            ("publish_registries", "promote", True, False,
             {"release_validate", "verify_release_assets", "publish_github_release", "publish_sdk_packages",
              "verify_crate_packages"}),
            ("publish_registries", "promote", True, True,
             {"release_validate", "verify_release_assets", "publish_sdk_packages", "verify_crate_packages"}),
            ("publish_registries", "existing", True, False,
             {"release_validate", "verify_release_assets", "publish_sdk_packages", "verify_crate_packages"}),
            ("publish_registries", "existing", True, True,
             {"release_validate", "publish_sdk_packages", "verify_crate_packages"}),
        )
        # publish_registries follows an SDK job that ran, whether it succeeded
        # or failed; a skipped or cancelled SDK job still stops it.
        accepted = {("publish_registries", "publish_sdk_packages"): ("success", "failure")}
        for job, mode, publish, dry, required in scenarios:
            self.assertIn("!cancelled()", job_condition(job))
            # resolve_release is already required by validation (and by the
            # registry DAG for docs). Its failure cascade is tested above.
            dependencies = [need for need in job_needs(job) if need != "resolve_release"]
            for statuses in itertools.product(("success", "failure", "skipped", "cancelled"),
                                              repeat=len(dependencies)):
                context = protocol_context(mode, publish, dry)
                results = dict(zip(dependencies, statuses))
                context.update({f"needs_{need}_result": value for need, value in results.items()})
                with self.subTest(job=job, mode=mode, dry=dry, results=results):
                    self.assertEqual(
                        job_runs(job, **context),
                        all(results[need] in accepted.get((job, need), ("success",))
                            for need in required),
                    )

    def test_jobs_run_only_when_every_declared_need_succeeded(self):
        # build_binaries keeps the implicit success(): its only ancestor is the
        # resolver. Every other job has an ancestor that some mode skips, so it
        # states each need's result under !cancelled() (GitHubSkipPropagationTests).
        self.assertNotIn("always()", job_condition("build_binaries"))
        for job, mode in (("release_validate", "candidate"), ("release_validate", "promote"),
                          ("build_binaries", "candidate"), ("collect_candidate", "candidate"),
                          ("verify_release_assets", "promote"), ("verify_crate_packages", "promote")):
            if job != "build_binaries":
                self.assertIn("!cancelled()", job_condition(job))
            for statuses in itertools.product(("success", "failure", "skipped", "cancelled"),
                                              repeat=len(job_needs(job))):
                context = protocol_context(mode)
                context.update({f"needs_{need}_result": value
                                for need, value in zip(job_needs(job), statuses)})
                with self.subTest(job=job, mode=mode, statuses=statuses):
                    self.assertEqual(job_runs(job, **context), all(s == "success" for s in statuses))

    def test_tag_mode_stops_before_heavy_validation(self):
        results = release_results(protocol_context("tag", event="push"))
        self.assertEqual(results["require_ci_green"], "success")
        self.assertEqual(results["release_validate"], "skipped")
        self.assertTrue(all(results[job] == "skipped" for job in WORK_JOBS))

    def test_registry_retry_never_calls_the_github_publisher(self):
        for dry in (False, True):
            context = protocol_context("existing", True, dry)
            results = release_results(context)
            self.assertEqual(results["publish_github_release"], "skipped")
            self.assertEqual(results["publish_sdk_packages"], "success")
            self.assertEqual(results["publish_registries"], "success")
            self.assertEqual(results["verify_release_assets"], "skipped" if dry else "success")
            for status in ("failure", "skipped"):
                failed = release_results(context, {"verify_release_assets": status})
                self.assertEqual(failed["publish_sdk_packages"], "success" if dry else "skipped")

    def test_promotion_rehearsals_always_require_verified_candidate(self):
        for publish in (False, True):
            for status in ("failure", "skipped", "cancelled"):
                results = release_results(protocol_context("promote", publish, True),
                                          {"verify_release_assets": status})
                for job in ("publish_github_release", "publish_sdk_packages",
                            "publish_registries"):
                    self.assertEqual(results[job], "skipped", (publish, status, job))


class GitHubSkipPropagationTests(unittest.TestCase):
    """GitHub reads a job's status functions over every ancestor, not only its
    declared needs, and a condition without one gets an implicit success().
    A job below a skipped ancestor therefore skips unless it states its needs'
    results under an explicit status function. v0.9.2 promotion run 38983785773
    and asset recovery run 38984295785 ran nothing after the CI gate this way,
    while the declared-needs-only model here still passed.
    """

    # Each dispatch mode, and the tag event, with the exact jobs that run.
    # Both candidate tiers share one DAG: no job condition reads the tier.
    BASE = {"resolve_release", "require_ci_green"}
    MODES = (
        ("candidate core", "candidate", False, False, "workflow_dispatch",
         {"build_binaries", "release_validate", "collect_candidate"}),
        ("candidate trailing", "candidate", False, False, "workflow_dispatch",
         {"build_binaries", "release_validate", "collect_candidate"}),
        ("promote", "promote", True, False, "workflow_dispatch",
         {"release_validate", "verify_release_assets", "publish_github_release",
          "publish_sdk_packages", "verify_crate_packages", "publish_registries"}),
        ("promote without registries", "promote", False, False, "workflow_dispatch",
         {"release_validate", "verify_release_assets", "publish_github_release"}),
        ("promote rehearsal", "promote", True, True, "workflow_dispatch",
         {"release_validate", "verify_release_assets", "publish_sdk_packages",
          "verify_crate_packages", "publish_registries"}),
        ("assets", "assets", False, False, "workflow_dispatch",
         {"release_validate", "publish_github_release"}),
        ("existing registry retry", "existing", True, False, "workflow_dispatch",
         {"release_validate", "verify_release_assets", "publish_sdk_packages",
          "verify_crate_packages", "publish_registries"}),
        ("existing rehearsal", "existing", True, True, "workflow_dispatch",
         {"release_validate", "publish_sdk_packages", "verify_crate_packages",
          "publish_registries"}),
        ("tag", "tag", False, False, "push", set()),
    )

    def test_every_mode_runs_its_jobs_under_github_skip_propagation(self):
        for label, mode, publish, dry, event, expected in self.MODES:
            with self.subTest(mode=label):
                results = release_results(protocol_context(mode, publish, dry, event=event))
                self.assertEqual({job for job, result in results.items() if result == "success"},
                                 self.BASE | expected)
                self.assertEqual({job for job, result in results.items() if result == "skipped"},
                                 set(JOBS) - self.BASE - expected)

    def test_no_job_condition_reads_the_candidate_tier(self):
        for job in JOBS:
            with self.subTest(job=job):
                self.assertNotIn("outputs.tier", job_condition(job))

    def test_status_functions_read_every_ancestor_not_only_declared_needs(self):
        self.assertEqual(job_ancestors("release_validate"),
                         ["resolve_release", "build_binaries", "require_ci_green"])
        self.assertEqual(job_ancestors("build_binaries"), ["resolve_release"])
        # The facts of promotion run 38983785773: the resolver and the CI gate
        # succeeded, the candidate legs were skipped.
        context = protocol_context("promote")
        context.update(needs_build_binaries_result="skipped")
        statuses = [context[f"needs_{job}_result"] for job in job_ancestors("release_validate")]
        self.assertTrue(all(context[f"needs_{need}_result"] == "success"
                            for need in job_needs("release_validate")))
        # The v0.9.2 condition had no status function: GitHub skipped it.
        self.assertFalse(evaluate_condition(
            "success() && needs.resolve_release.outputs.mode != 'tag'", statuses=statuses, **context))
        self.assertTrue(job_runs("release_validate", **context))

    def test_jobs_below_a_skippable_ancestor_state_every_need_under_a_status_function(self):
        # Only the resolver can never be skipped; any other ancestor is skipped
        # in some mode (build_binaries outside candidate mode, and so on down).
        for job in JOBS:
            if not set(job_ancestors(job)) - {"resolve_release"}:
                continue
            condition = job_condition(job)
            with self.subTest(job=job):
                self.assertRegex(condition, r"\b(?:always|success|failure|cancelled)\s*\(")
                for need in job_needs(job):
                    self.assertIn(f"needs.{need}.result", condition)

    def test_a_cancelled_run_starts_no_job_after_its_cancellation(self):
        # always() starts a job whose needs succeeded even after the run was
        # cancelled, so a cancelled promotion could still publish. Every job
        # condition uses !cancelled() as its status function instead.
        for job in JOBS:
            with self.subTest(job=job):
                self.assertNotRegex(job_condition(job), r"\balways\s*\(")
        for label, mode, publish, dry, event, expected in self.MODES:
            context = protocol_context(mode, publish, dry, event=event)
            results = release_results(context)
            context.update({f"needs_{job}_result": result for job, result in results.items()})
            for job in sorted((self.BASE | expected) - {"resolve_release"}):
                with self.subTest(mode=label, job=job):
                    self.assertTrue(job_runs(job, **context))
                    self.assertFalse(job_runs(job, run_cancelled=True, **context))
        # The model is exercised, not assumed: always() ignores the cancel.
        context = protocol_context("promote")
        self.assertTrue(evaluate_condition("always() && true", run_cancelled=True, **context))
        self.assertFalse(evaluate_condition("!cancelled() && true", run_cancelled=True, **context))


class WorkflowExtractorTests(unittest.TestCase):
    def test_bounded_jobs_and_steps_cannot_borrow_guards_from_their_neighbors(self):
        block = job_block("publish_sdk_packages")
        self.assertNotIn("publish_registries:", block)
        self.assertNotIn("Publish Rust crate", block)
        step = step_block("Checkout immutable source", "verify_release_assets")
        self.assertNotIn("github.workflow_sha", step)
        self.assertNotIn("Checkout immutable workflow tools", step)
        with self.assertRaises(ValueError):
            step_script("Checkout immutable source", "verify_release_assets")

    def test_missing_context_fails_even_in_short_circuited_or_skipped_conditions(self):
        with self.assertRaisesRegex(AssertionError, "unsupplied"):
            evaluate_condition("false && needs.resolve_release.outputs.missing == 'true'")
        context = protocol_context()
        context.pop("needs_resolve_release_outputs_mode")
        context["needs_release_validate_result"] = "skipped"
        with self.assertRaisesRegex(AssertionError, "unsupplied"):
            job_runs("verify_release_assets", **context)
        with self.assertRaisesRegex(AssertionError, "unsupplied"):
            job_runs("require_ci_green")

    def test_boolean_inequality_and_status_functions_are_not_relaxed(self):
        self.assertTrue(evaluate_condition("!false && true != false && always()"))
        self.assertFalse(evaluate_condition("success()", statuses=("skipped",)))
        self.assertTrue(evaluate_condition("failure()", statuses=("failure",)))
        self.assertTrue(evaluate_condition("cancelled()", run_cancelled=True))
        self.assertFalse(evaluate_condition("cancelled()", statuses=("cancelled",)))
        self.assertFalse(evaluate_condition("success()", statuses=("success",), run_cancelled=True))
        with self.assertRaisesRegex(AssertionError, "unsupported"):
            evaluate_condition("unknownFunction()")
        with self.assertRaisesRegex(AssertionError, "unsupported"):
            evaluate_condition("1 + 1")


class ImmutableReleaseWorkflowTests(unittest.TestCase):
    def test_protocol_inputs_have_safe_compatibility_defaults(self):
        head = RELEASE_WORKFLOW.read_text().split("jobs:", 1)[0]
        self.assertIn("options: [existing, candidate, promote, assets]", head)
        self.assertIn("default: existing", head)
        for name in ("source_sha", "artifact_selection", "release_tag"):
            self.assertIn(f"      {name}:", head)
        for name in ("publish_release_packages", "registry_dry_run"):
            start = head.index(f"      {name}:")
            block = re.split(r"\n      \w+:", head[start:], maxsplit=1)[0]
            self.assertIn("default: false", block)
            self.assertIn("type: boolean", block)

    def test_resolver_is_the_only_authority_for_source_and_flags(self):
        resolve = job_block("resolve_release")
        for output in ("mode", "sha", "version", "tag", "publish_packages", "dry_run", "ci_bypass"):
            self.assertIn(f"      {output}: ${{{{ steps.resolve.outputs.{output} }}}}", resolve)
        self.assertIn("run: python3 scripts/release-candidate.py resolve", resolve)
        for variable, input_name in (
            ("RELEASE_MODE", "release_mode"), ("SOURCE_SHA", "source_sha"),
            ("RELEASE_TAG", "release_tag"), ("ARTIFACT_SELECTION", "artifact_selection"),
            ("PUBLISH_RELEASE_PACKAGES", "publish_release_packages"),
            ("REGISTRY_DRY_RUN", "registry_dry_run"),
        ):
            self.assertIn(f"{variable}: ${{{{ inputs.{input_name} }}}}", resolve)
        for job in JOBS[1:]:
            with self.subTest(job=job):
                block = job_block(job)
                self.assertNotRegex(
                    block,
                    r"(?:github\.event\.)?inputs\.(?:release_mode|source_sha|release_tag|"
                    r"publish_release_packages|registry_dry_run)\b",
                )
                self.assertNotIn("github.ref_name", block)
                self.assertNotIn("github.sha", block)

    def test_every_checkout_uses_the_immutable_source_or_separate_workflow_tools(self):
        for job in JOBS:
            lines = job_block(job).splitlines()
            for index, line in enumerate(lines):
                if "uses: actions/checkout@" not in line:
                    continue
                name = lines[index - 1].split("- name: ", 1)[1]
                block = step_block(name, job)
                with self.subTest(job=job, step=name):
                    expected = ("${{ github.workflow_sha }}"
                                if name == "Checkout immutable workflow tools"
                                else "${{ needs.resolve_release.outputs.sha }}")
                    self.assertEqual(scalar(block, "ref", "          "), expected)
                    if name == "Checkout immutable workflow tools" and job != "resolve_release":
                        self.assertEqual(scalar(block, "path", "          "), ".release-tools")
        self.assertIn("Checkout immutable workflow tools", job_block("resolve_release"))

    def test_old_source_verifiers_and_publisher_use_pinned_tools_not_source_scripts(self):
        for job in ("verify_release_assets", "publish_github_release"):
            block = job_block(job)
            self.assertIn("Checkout immutable source", block)
            self.assertIn("Checkout immutable workflow tools", block)
            commands = re.findall(r"python3 (\S*release-candidate\.py) (\S+)", block)
            self.assertTrue(commands)
            self.assertTrue(all(path == ".release-tools/scripts/release-candidate.py"
                                for path, _ in commands))
            for variable, output in (("RELEASE_MODE", "mode"), ("RELEASE_SHA", "sha"),
                                     ("RELEASE_VERSION", "version"), ("RELEASE_TAG", "tag"),
                                     ("REGISTRY_DRY_RUN", "dry_run")):
                self.assertIn(f"{variable}: ${{{{ needs.resolve_release.outputs.{output} }}}}", block)

    def test_candidate_preserves_full_locked_matrix_and_signs_before_archiving(self):
        self.assertEqual(set(matrix_targets()), {
            "x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu", "aarch64-apple-darwin",
            "x86_64-apple-darwin", "x86_64-pc-windows-msvc",
        })
        # Each candidate run builds exactly its tier's legs, from the resolver.
        self.assertIn("      matrix: ${{ fromJSON(needs.resolve_release.outputs.build_matrix) }}\n",
                      job_block("build_binaries") + "\n")
        self.assertIn("      build_matrix: ${{ steps.resolve.outputs.build_matrix }}\n", job_block("resolve_release"))
        self.assertIn("      tier: ${{ steps.resolve.outputs.tier }}\n", job_block("resolve_release"))
        self.assertIn("RELEASE_TIER: ${{ inputs.candidate_tier }}",
                      step_block("Resolve release mode and source", "resolve_release"))
        for tier, targets in release_candidate.assets.TIER_TARGETS.items():
            matrix = release_candidate.build_matrix(tier)
            self.assertEqual([leg["target"] for leg in matrix["include"]], list(targets))
            for leg in matrix["include"]:
                self.assertEqual(set(leg), {"runs-on", "target", "archive_ext"})
        script = step_script("Build gateway binaries")
        self.assertEqual(set(re.findall(r"--bin (\w+)", script)),
                         {"meta_gateway", "rpc_gateway", "meta_repair"})
        self.assertIn("--locked --release --target", script)
        self.assertRegex(script.replace("\\\n", " "),
                         r"cargo build [^\n]*--features openai-live --locked")
        packaging = step_block("Package artifacts")
        self.assertIn("VERSION: ${{ needs.resolve_release.outputs.version }}", packaging)
        self.assertNotIn("GITHUB_REF", packaging)
        self.assertIn('artifact="${prefix}-${VERSION}-${TARGET}.${ARCHIVE_EXT}"', packaging)
        signing = step_block("Ad-hoc codesign macOS binaries")
        self.assertEqual(scalar(signing, "if", "        "), "runner.os == 'macOS'")
        self.assertIn("for bin in meta_gateway rpc_gateway meta_repair", signing)
        self.assertIn('codesign --force --sign - "target/${{ matrix.target }}/release/${bin}"', signing)
        block = job_block("build_binaries")
        order = ("Apply release build profile overrides", "Build gateway binaries",
                 "Verify macOS binary architecture",
                 "Ad-hoc codesign macOS binaries", "Package artifacts", "Attest build provenance",
                 "Retain original provenance bundle", "Upload artifacts")
        positions = [block.index(f"      - name: {name}") for name in order]
        self.assertEqual(positions, sorted(positions))

    def test_original_bundles_are_retained_before_immutable_attempt_qualified_uploads(self):
        for job, attest, retain, upload, expected_name, directory in (
            ("build_binaries", "Attest build provenance", "Retain original provenance bundle",
             "Upload artifacts", "meta-gateway-binaries-${{ matrix.target }}-${{ github.run_attempt }}", "dist"),
            ("collect_candidate", "Attest candidate manifest", "Retain manifest provenance bundle",
             "Upload immutable candidate manifest", "meta-candidate-manifest-${{ github.run_attempt }}", "candidate"),
        ):
            with self.subTest(job=job):
                block = job_block(job)
                self.assertLess(block.index(f"- name: {attest}"), block.index(f"- name: {retain}"))
                self.assertLess(block.index(f"- name: {retain}"), block.index(f"- name: {upload}"))
                retained = step_block(retain, job)
                self.assertIn("BUNDLE_PATH: ${{ steps.provenance.outputs.bundle-path }}", retained)
                self.assertIn(f'run: cp "$BUNDLE_PATH" {directory}/provenance.jsonl', retained)
                artifact = step_block(upload, job)
                self.assertRegex(artifact, r"uses: actions/upload-artifact@[0-9a-f]{40} # v4\n")
                for field, value in (("name", expected_name), ("overwrite", "false"),
                                     ("retention-days", "90"), ("if-no-files-found", "error"),
                                     ("path", f"{directory}/*")):
                    self.assertEqual(scalar(artifact, field, "          "), value)
        manifest = step_block("Attest candidate manifest", "collect_candidate")
        # Either tier's manifest; a candidate run collects exactly one.
        self.assertEqual(scalar(manifest, "subject-path", "          "),
                         "candidate/candidate-manifest*.json")
        for tier in release_candidate.assets.TIERS:
            self.assertTrue(release_candidate.tier_proof_names(tier)[1].startswith("candidate-manifest"))

    def test_collection_binds_ci_and_upload_identity_before_emitting_acceptance(self):
        block = job_block("collect_candidate")
        self.assertEqual(job_needs("collect_candidate"),
                         ["resolve_release", "require_ci_green", "release_validate", "build_binaries"])
        self.assertIn("RELEASE_TIER: ${{ needs.resolve_release.outputs.tier }}", block)
        for field in ("run_id", "run_attempt"):
            self.assertIn(f"${{{{ needs.require_ci_green.outputs.{field} }}}}", block)
        collect = step_script("Collect verified original artifacts", "collect_candidate")
        self.assertIn('--ci-run-id "$CI_RUN_ID" --ci-run-attempt "$CI_RUN_ATTEMPT" --tier "$RELEASE_TIER"', collect)
        emit = step_script("Emit explicit acceptance selection", "collect_candidate")
        for tier in release_candidate.assets.TIERS:
            # The selection reads the manifest under its tier's release name.
            expected = release_candidate.tier_proof_names(tier)[1]
            result = subprocess.run(["bash", "-c", emit.replace("python3 scripts/release-candidate.py selection",
                                                                'echo "$manifest" #')],
                                    env={**os.environ, "RELEASE_TIER": tier, "ARTIFACT_ID": "1",
                                         "ARTIFACT_DIGEST": "a"}, capture_output=True, text=True)
            self.assertEqual(result.stdout.strip(), f"candidate/{expected}", result.stderr)
        selection = step_block("Emit explicit acceptance selection", "collect_candidate")
        self.assertIn("ARTIFACT_ID: ${{ steps.manifest.outputs.artifact-id }}", selection)
        self.assertIn("ARTIFACT_DIGEST: ${{ steps.manifest.outputs.artifact-digest }}", selection)
        self.assertIn('--artifact-id "$ARTIFACT_ID" --artifact-digest "$ARTIFACT_DIGEST"', selection)
        self.assertLess(block.index("- name: Upload immutable candidate manifest"),
                        block.index("- name: Emit explicit acceptance selection"))

    def test_publisher_requires_strict_restaging_in_its_own_job_before_any_publish(self):
        job = "publish_github_release"
        stage = step_block("Reverify original candidate before publication", job)
        publish = step_block("Publish exact original archives", job)
        self.assertEqual(scalar(stage, "if", "        "), "needs.resolve_release.outputs.mode == 'promote'")
        self.assertEqual(scalar(stage, "if", "        "), scalar(publish, "if", "        "))
        self.assertIn(" stage --output-dir release-assets --proof-dir release-proofs", stage)
        self.assertIn(" publish --assets-dir release-assets --proof-dir release-proofs", publish)
        self.assertLess(job_block(job).index(stage), job_block(job).index(publish))
        self.assertNotIn("continue-on-error:", job_block(job))
        self.assertNotIn("always()", scalar(publish, "if", "        "))
        recover = step_block("Recover explicitly selected missing original assets", job)
        self.assertEqual(scalar(recover, "if", "        "), "needs.resolve_release.outputs.mode == 'assets'")
        self.assertIn(".release-tools/scripts/release-candidate.py recover", recover)

    def test_registry_retry_uses_read_only_public_verification_not_candidate_selection(self):
        block = step_block("Verify existing published assets without writes", "verify_release_assets")
        self.assertEqual(scalar(block, "if", "        "), "needs.resolve_release.outputs.mode == 'existing'")
        self.assertIn("release-candidate.py verify-published", block)
        selected = step_block("Verify selected candidate", "verify_release_assets")
        self.assertEqual(scalar(selected, "if", "        "), "needs.resolve_release.outputs.mode == 'promote'")
        self.assertIn("stage --output-dir release-assets --proof-dir release-proofs", selected)

    def test_promotion_and_recovery_never_rebuild_resign_repack_or_reattest_binaries(self):
        for job in ("verify_release_assets", "publish_github_release", "publish_sdk_packages",
                    "publish_registries"):
            with self.subTest(job=job):
                block = job_block(job)
                self.assertNotRegex(block, r"(?m)^\s*(?:cargo|scripts/repo-cargo) build\b")
                self.assertNotRegex(block, r"\b(?:codesign|strip|7z)\b")
                self.assertNotRegex(block, r"\btar\s+-[cz]")
                self.assertNotIn("attest-build-provenance", block)
                self.assertNotIn("download-artifact@", block)
                self.assertNotIn("build_binaries", job_needs(job))

    def test_release_writes_are_serialized_by_explicit_tag_without_cancellation(self):
        head = RELEASE_WORKFLOW.read_text().split("jobs:", 1)[0]
        concurrency = head.split("concurrency:", 1)[1].split("\nenv:", 1)[0]
        self.assertIn("inputs.release_tag", scalar(concurrency, "group", "  "))
        self.assertIn("github.run_id", scalar(concurrency, "group", "  "))
        self.assertEqual(scalar(concurrency, "cancel-in-progress", "  "), "false")


class ReleasePermissionsTests(unittest.TestCase):
    @staticmethod
    def permissions(block, indent):
        lines = block.splitlines()
        marker = f"{indent}permissions:"
        if marker not in lines:
            return None
        grants = {}
        for line in lines[lines.index(marker) + 1:]:
            if not line.strip() or line.strip().startswith("#"):
                continue
            if not line.startswith(indent + "  "):
                break
            key, value = line.strip().split(": ", 1)
            grants[key] = value
        return grants

    def test_global_permissions_are_read_only(self):
        self.assertEqual(self.permissions(RELEASE_WORKFLOW.read_text(), ""),
                         {"actions": "read", "contents": "read"})

    def test_attestation_grants_are_candidate_only_and_contents_write_is_publisher_only(self):
        for job in JOBS:
            grants = self.permissions(job_block(job), "    ") or {}
            writes = {key for key, value in grants.items() if value == "write"}
            expected = ({"id-token", "attestations"} if job in ("build_binaries", "collect_candidate")
                        else {"contents"} if job == "publish_github_release" else set())
            with self.subTest(job=job):
                self.assertEqual(writes, expected)
                if job in ("build_binaries", "collect_candidate"):
                    self.assertEqual(grants.get("contents"), "read")
        self.assertEqual(self.permissions(job_block("collect_candidate"), "    ").get("actions"), "read")

    def test_no_registry_or_docs_credentials_reach_candidate_jobs(self):
        for job in ("resolve_release", "require_ci_green", "release_validate",
                    "build_binaries", "collect_candidate"):
            with self.subTest(job=job):
                self.assertNotIn("secrets.", job_block(job))
        for secret, owner in (("PYPI_API_TOKEN", "publish_sdk_packages"),
                              ("NPM_TOKEN", "publish_sdk_packages"),
                              ("CARGO_REGISTRY_TOKEN", "publish_registries")):
            for job in JOBS:
                self.assertEqual(f"secrets.{secret}" in job_block(job), job == owner, (secret, job))


class ExactMainCiGateTests(unittest.TestCase):
    """Execute the real JavaScript with a read-only Actions API stub."""

    GREEN_RUN = {
        "head_sha": "deadbeef", "head_branch": "main", "event": "push",
        "status": "completed", "conclusion": "success", "run_attempt": 2,
        "id": 42, "html_url": "https://example.invalid/actions/runs/42",
    }

    def assert_refused(self, calls):
        self.assertEqual([c["kind"] for c in calls], ["listWorkflowRuns", "setFailed"])

    def test_release_validate_waits_on_the_gate_rather_than_merely_coexisting(self):
        block = job_block("release_validate")
        self.assertEqual(job_needs("release_validate"), ["resolve_release", "require_ci_green"])
        # The gate waits on the candidate builds (which skip in every other
        # mode), never on validation.
        self.assertEqual(job_needs("require_ci_green"), ["resolve_release", "build_binaries"])
        self.assertIn("RELEASE_SHA: ${{ needs.resolve_release.outputs.sha }}",
                      job_block("require_ci_green"))
        self.assertIn("CI_BYPASS: ${{ needs.resolve_release.outputs.ci_bypass }}",
                      job_block("require_ci_green"))
        self.assertNotIn("inputs.", job_block("require_ci_green"))

    def test_the_gate_refuses_rather_than_polls(self):
        # A poll converts "this commit was never verified" into "this is taking
        # a while", and burns the release budget on a precondition that should
        # already hold. If someone reintroduces waiting, this should fail.
        block = job_block("require_ci_green")
        self.assertIn("Refuse unless exact-main CI is already green", block)
        self.assertNotIn("setTimeout", block)

    def test_the_gate_can_read_workflow_runs(self):
        # A `permissions:` block sets every unlisted scope to none, so without
        # `actions: read` the gate fails on an API 403 - a refusal for a reason
        # unrelated to CI, which reads identical to a real red at a glance.
        self.assertIn("actions: read", workflow_permissions_without_comments())

    def test_a_green_exact_main_run_is_accepted(self):
        calls = run_ci_gate([self.GREEN_RUN])
        self.assertEqual([c["kind"] for c in calls].count("setFailed"), 0)
        self.assertEqual(calls[0], {
            "kind": "listWorkflowRuns",
            "args": {"owner": "lukacf", "repo": "meerkat-meta", "workflow_id": "ci.yml",
                     "branch": "main", "event": "push", "head_sha": "deadbeef", "per_page": 100},
        })
        self.assertIn({"kind": "setOutput", "name": "run_id", "value": "42"}, calls)
        self.assertIn({"kind": "setOutput", "name": "run_attempt", "value": "2"}, calls)

    def test_a_completed_but_failed_run_is_refused(self):
        # THE assertion of the whole gate. Dropping `conclusion === "success"`
        # leaves a check that accepts any terminal state, including failure.
        for conclusion in ("failure", "cancelled", "skipped", "neutral", "timed_out", None):
            with self.subTest(conclusion=conclusion):
                self.assert_refused(run_ci_gate([{**self.GREEN_RUN, "conclusion": conclusion}]))

    def test_a_commit_with_no_exact_main_run_is_refused(self):
        self.assert_refused(run_ci_gate([]))

    def test_a_pull_request_run_on_the_same_sha_is_not_accepted(self):
        # The v0.8.26 shape: the PR head was green on pull_request and had zero
        # push-to-main runs. A merge-result run answers a different question.
        self.assert_refused(run_ci_gate([{**self.GREEN_RUN, "event": "pull_request"}]))

    def test_only_the_resolver_can_authorize_the_ci_bypass(self):
        calls = run_ci_gate([], ci_bypass="true")
        self.assertEqual(calls, [])

        for bypass, dispatch, dry, tag in itertools.product(
            ("false", "", "TRUE", "1"), ("true", "false"), ("true", "false"), ("", "v0.8.31"),
        ):
            with self.subTest(bypass=bypass, dispatch=dispatch, dry=dry, tag=tag):
                self.assert_refused(run_ci_gate([], ci_bypass=bypass, raw_env={
                    "IS_DISPATCH": dispatch, "REGISTRY_DRY_RUN": dry, "RELEASE_TAG_INPUT": tag,
                }))

    def test_wrong_source_and_wrong_main_are_rejected_independently(self):
        for changes in ({"head_sha": "wrong"}, {"head_branch": "feature"},
                        {"event": "workflow_dispatch"}):
            with self.subTest(changes=changes):
                self.assert_refused(run_ci_gate([{**self.GREEN_RUN, **changes}]))

    def test_resolved_source_not_dispatch_sha_is_the_ci_query_identity(self):
        calls = run_ci_gate([{**self.GREEN_RUN, "head_sha": "old-tag-source"}],
                            release_sha="old-tag-source",
                            raw_env={"GITHUB_SHA": "current-main"})
        self.assertEqual(calls[0]["args"]["head_sha"], "old-tag-source")
        self.assertFalse(any(c["kind"] == "setFailed" for c in calls))

    def test_an_in_flight_run_is_refused_rather_than_awaited(self):
        for status in ("in_progress", "queued", "waiting"):
            with self.subTest(status=status):
                self.assert_refused(run_ci_gate([{**self.GREEN_RUN, "status": status}]))

    def test_the_gate_requires_main_and_push_not_merely_the_sha(self):
        # A pull_request run on the same SHA is a different question: it tests
        # the merge result, not the commit as it lands on main.
        block = job_block("require_ci_green")
        self.assertIn('run.head_branch === "main"', block)
        self.assertIn('run.event === "push"', block)


class LinuxBinaryPortabilityGateTests(unittest.TestCase):
    """The gate reads the produced binary and refuses one the floor cannot load.

    A Linux GNU binary's glibc floor is whatever glibc it was linked against,
    and until this gate nothing read the result: the v0.8.30 gateways, built
    directly on ubuntu-latest, carried a hard GLIBC_2.39 version requirement
    (pidfd_spawnp/pidfd_getpid) and the loader refused them on Debian bookworm
    (2.36), Ubuntu 22.04 (2.35) and bullseye (2.31). Every job read green.
    """

    def test_a_glibc_reference_above_the_floor_is_refused(self):
        result = run_portability_check(DYNSYMS_ABOVE_FLOOR, DYNAMIC_RUSTLS)

        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn(
            "references GLIBC_2.39, above the declared floor GLIBC_2.31", result.stderr
        )

    def test_a_binary_within_the_floor_passes_and_names_its_maximum(self):
        result = run_portability_check(DYNSYMS_WITHIN_FLOOR, DYNAMIC_RUSTLS)

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("PASS", result.stdout)
        # The input also carries GLIBC_2.4, which a string sort would report as
        # the maximum and then compare above 2.31. Only a version sort names 2.30.
        self.assertIn("max glibc ref GLIBC_2.30, floor GLIBC_2.31", result.stdout)

    def test_the_floor_is_the_declared_parameter_not_a_constant(self):
        result = run_portability_check(DYNSYMS_ABOVE_FLOOR, DYNAMIC_RUSTLS, floor="2.39")

        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
        self.assertIn("floor GLIBC_2.39", result.stdout)

    def test_a_dynamic_openssl_dependency_is_refused(self):
        # All first-party TLS is rustls; a NEEDED libssl means a dependency
        # regressed into native-tls, as meerkat v0.8.21 shipped via oai-rt-rs.
        result = run_portability_check(DYNSYMS_WITHIN_FLOOR, DYNAMIC_OPENSSL)

        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("dynamically links OpenSSL", result.stderr)
        self.assertIn("libssl.so.3", result.stderr)

    def test_one_missing_binary_fails_the_whole_set(self):
        result = run_portability_check(
            DYNSYMS_WITHIN_FLOOR, DYNAMIC_RUSTLS, binaries=["rpc_gateway", "meta_gateway"]
        )

        self.assertEqual(result.returncode, 1, result.stdout + result.stderr)
        self.assertIn("PASS", result.stdout)
        self.assertIn("meta_gateway: file not found", result.stderr)

    def test_the_gate_is_executable_from_a_checkout(self):
        # The workflow invokes it as ./scripts/...; a 644 file fails the step
        # with "Permission denied", which reads as a broken gate, not a floor.
        self.assertTrue(os.access(PORTABILITY_SCRIPT, os.X_OK))
        self.assertTrue(PORTABILITY_SCRIPT.read_text().startswith("#!/usr/bin/env bash\n"))


class LinuxReleaseContainerTests(unittest.TestCase):
    """Linux GNU release legs build inside a pinned-glibc container and every
    produced binary passes the portability gate before it is packaged.

    These evaluate the real `container:` and cache `key:` expressions per
    matrix target and execute the gate step's shell, rather than asserting
    that the word "bullseye" appears somewhere in the file.
    """

    def container_expression(self) -> str:
        return scalar(job_block("build_binaries"), "container", "    ")

    def linux_image(self) -> str:
        return render_template(self.container_expression(), matrix_target="x86_64-unknown-linux-gnu")

    def test_linux_gnu_legs_build_inside_the_pinned_container_and_others_do_not(self):
        targets = matrix_targets()
        self.assertEqual(len(targets), 5, targets)
        for target in targets:
            with self.subTest(target=target):
                image = render_template(self.container_expression(), matrix_target=target)
                if target.endswith("-unknown-linux-gnu"):
                    self.assertRegex(image, r"^docker\.io/library/buildpack-deps:bullseye@sha256:[0-9a-f]{64}$")
                else:
                    # macOS and Windows legs cannot run a Linux container; an
                    # empty string is how the expression opts them out.
                    self.assertEqual(image, "")

    def test_the_declared_floor_matches_the_container_image(self):
        # The floor is a fact about the image; the two are declared in two
        # files and this is the only thing that holds them together.
        codename = self.linux_image().split("@", 1)[0].rsplit(":", 1)[1]
        self.assertIn(
            codename, CONTAINER_GLIBC,
            f"unknown Debian codename {codename!r}: extend CONTAINER_GLIBC and decide the floor",
        )
        declared = re.search(
            r'^META_GLIBC_FLOOR="\$\{META_GLIBC_FLOOR:-([0-9.]+)\}"$',
            PORTABILITY_SCRIPT.read_text(),
            re.M,
        )
        self.assertIsNotNone(declared, "the gate must declare an overridable default floor")
        self.assertEqual(declared.group(1), CONTAINER_GLIBC[codename])

    def test_the_gate_runs_on_linux_after_the_build_and_before_anything_ships(self):
        block = job_block("build_binaries")
        gate = block.index("      - name: Check Linux binary portability")

        self.assertLess(block.index("      - name: Build gateway binaries"), gate)
        for later in ("Package artifacts", "Attest build provenance", "Upload artifacts"):
            with self.subTest(step=later):
                self.assertLess(gate, block.index(f"      - name: {later}"))
        step = step_block("Check Linux binary portability")
        self.assertIn("        if: runner.os == 'Linux'", step)
        self.assertIn("          TARGET: ${{ matrix.target }}", step)

    def test_the_gate_checks_every_binary_the_build_step_produces(self):
        built = re.findall(r"--bin (\w+)", step_script("Build gateway binaries"))
        self.assertTrue(built, "the build step names no --bin; the parser is wrong")
        target = "aarch64-unknown-linux-gnu"

        result, checked = run_portability_step(target, gate_status=0)

        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(
            sorted(checked), sorted(f"target/{target}/release/{name}" for name in built)
        )

    def test_a_refused_binary_fails_the_step(self):
        result, checked = run_portability_step("x86_64-unknown-linux-gnu", gate_status=1)

        self.assertEqual(result.returncode, 1, result.stderr)
        self.assertTrue(checked, "the gate was never invoked")


def matrix_entries(job: str = "build_binaries", text: str | None = None) -> list[dict[str, str]]:
    """The build legs: the resolver's table for build_binaries, or a static
    `include:` matrix parsed from `text`, as `key: value` maps."""
    if text is None:
        assert job == "build_binaries"
        return [dict(leg) for leg in release_candidate.BUILD_LEGS]
    entries: list[dict[str, str]] = []
    for line in text.splitlines():
        start = re.fullmatch(r"          - ([a-z_-]+): (\S+)", line)
        field = re.fullmatch(r"            ([a-z_-]+): (\S+)", line)
        if start:
            entries.append({start.group(1): start.group(2)})
        elif field and entries:
            entries[-1][field.group(1)] = field.group(2)
    return entries


MACH_O_FILE_DESCRIPTIONS = {
    "arm64": "Mach-O 64-bit executable arm64",
    "x86_64": "Mach-O 64-bit executable x86_64",
}


def run_macos_arch_step(target: str, *, lipo: str, file: str,
                        overrides: dict[str, tuple[str, str]] | None = None,
                        libraries: dict[str, str] | None = None,
                        ) -> tuple[subprocess.CompletedProcess[str], list[str]]:
    """Execute the `Verify macOS binary architecture` step body under bash.

    Fake `lipo` and `file` print the given readings (per binary name when
    `overrides` names it) and record which paths were read, so the test
    observes the step's decisions on the binaries a cross build produced
    rather than pattern-matching the YAML. No macOS toolchain is needed.
    """
    with fixture_directory() as tmp:
        root = Path(tmp)
        bin_dir = root / "fake-bin"
        bin_dir.mkdir()
        log = root / "lipo.log"
        readings = {"*": (lipo, file), **(overrides or {})}
        (root / "readings.json").write_text(json.dumps(readings))
        for tool, index, flag in (("lipo", 0, "-archs"), ("file", 1, "-b")):
            record = 'printf "%s\\n" "$2" >> "$FAKE_LIPO_LOG"' if tool == "lipo" else ":"
            fake = bin_dir / tool
            fake.write_text(
                f"""#!/usr/bin/env bash
set -euo pipefail
[[ "$1" == "{flag}" ]] || {{ echo "fake {tool}: unexpected arguments: $*" >&2; exit 99; }}
{record}
python3 - "$2" <<'PY'
import json, os, sys
readings = json.load(open(os.environ["FAKE_READINGS"]))
name = os.path.basename(sys.argv[1])
print(readings.get(name, readings["*"])[{index}])
PY
"""
            )
            fake.chmod(0o755)
        # The bundled C libraries a build leaves under target/<triple>/release/build.
        arch = {"x86_64-apple-darwin": "x86_64"}.get(target, "arm64")
        for relative, archs in (libraries if libraries is not None else {
            "audiopus_sys-1/out/lib/libopus.a": arch,
            "ring-2/out/libring_core_0_17_14_.a": arch,
            "libsqlite3-sys-3/out/libsqlite3.a": arch,
        }).items():
            library = root / "target" / target / "release" / "build" / relative
            library.parent.mkdir(parents=True, exist_ok=True)
            library.write_bytes(b"!<arch>\n")
            readings[library.name] = (archs, "current ar archive")
        (root / "readings.json").write_text(json.dumps(readings))
        env = os.environ.copy()
        env.update({
            "TARGET": target,
            "FAKE_LIPO_LOG": str(log),
            "FAKE_READINGS": str(root / "readings.json"),
        })
        # The step names its tools by absolute path; the fakes stand in.
        script = (step_script("Verify macOS binary architecture")
                  .replace("lipo=/usr/bin/lipo", f"lipo={bin_dir / 'lipo'}")
                  .replace("file=/usr/bin/file", f"file={bin_dir / 'file'}"))
        result = subprocess.run(
            ["bash", "-c", script],
            cwd=root,
            env=env,
            capture_output=True,
            text=True,
        )
        checked = log.read_text().splitlines() if log.exists() else []
    return result, checked


class MacosReleaseLegTests(unittest.TestCase):
    """Both macOS legs run on the free arm64 macos-15 label: x86_64 is a
    cross build there, and every produced binary's Mach-O architecture is
    read back before anything is signed, packaged, attested or uploaded."""

    def test_x86_64_macos_cross_compiles_on_the_free_arm64_label(self):
        runners = {entry["target"]: entry["runs-on"] for entry in matrix_entries()}
        self.assertEqual(set(runners), set(matrix_targets()))
        self.assertEqual(runners["aarch64-apple-darwin"], "macos-15")
        self.assertEqual(runners["x86_64-apple-darwin"], "macos-15")
        block = job_block("build_binaries")
        # Neither the slow Intel label nor a billed larger label comes back.
        self.assertNotRegex(block, r"(?m)^\s+- runs-on: macos-\S*(intel|large|xlarge)")
        # The toolchain step installs the cross standard library.
        toolchain = step_block("Install Rust")
        self.assertIn("uses: dtolnay/rust-toolchain@", toolchain)
        self.assertEqual(scalar(toolchain, "targets", "          "), "${{ matrix.target }}")
        self.assertIn('--release --target "${{ matrix.target }}"', step_script("Build gateway binaries"))

    def test_the_check_runs_on_macos_after_the_build_and_before_anything_ships(self):
        block = job_block("build_binaries")
        check = block.index("      - name: Verify macOS binary architecture")
        self.assertLess(block.index("      - name: Build gateway binaries"), check)
        for later in ("Ad-hoc codesign macOS binaries", "Package artifacts",
                      "Attest build provenance", "Upload artifacts"):
            with self.subTest(step=later):
                self.assertLess(check, block.index(f"      - name: {later}"))
        step = step_block("Verify macOS binary architecture")
        self.assertEqual(scalar(step, "if", "        "), "runner.os == 'macOS'")
        self.assertIn("          TARGET: ${{ matrix.target }}", step)
        self.assertNotIn("continue-on-error", step)

    def test_every_binary_the_build_step_produces_is_read_for_each_macos_target(self):
        built = re.findall(r"--bin (\w+)", step_script("Build gateway binaries"))
        self.assertTrue(built, "the build step names no --bin; the parser is wrong")
        for target, arch in (("aarch64-apple-darwin", "arm64"), ("x86_64-apple-darwin", "x86_64")):
            with self.subTest(target=target):
                result, checked = run_macos_arch_step(
                    target, lipo=arch, file=MACH_O_FILE_DESCRIPTIONS[arch])
                self.assertEqual(result.returncode, 0, result.stderr)
                binaries = [path for path in checked if "/build/" not in path]
                self.assertEqual(sorted(binaries),
                                 sorted(f"target/{target}/release/{name}" for name in built))
                # Each bundled C library's architecture is read and logged.
                self.assertEqual(len([path for path in checked if "/build/" in path]), 3)
                self.assertIn(f"libopus.a: configured for '{arch}'", result.stdout)

    def test_the_tools_are_named_by_absolute_path(self):
        script = step_script("Verify macOS binary architecture")
        self.assertIn("lipo=/usr/bin/lipo\n", script)
        self.assertIn("file=/usr/bin/file\n", script)
        self.assertNotRegex(script, r"(?m)^\s*(lipo|file) -")
        self.assertIn("/usr/bin/codesign --force --sign -", step_block("Ad-hoc codesign macOS binaries"))
        self.assertIn("/usr/bin/arch -x86_64", step_script("Smoke-run macOS gateways"))

    def test_a_host_architecture_c_library_or_a_missing_one_is_refused(self):
        for name, libraries in (
            ("host-architecture opus", {"audiopus_sys-1/out/lib/libopus.a": "arm64",
                                        "ring-2/out/libring_core_0_17_14_.a": "x86_64",
                                        "libsqlite3-sys-3/out/libsqlite3.a": "x86_64"}),
            ("missing sqlite", {"audiopus_sys-1/out/lib/libopus.a": "x86_64",
                                "ring-2/out/libring_core_0_17_14_.a": "x86_64"}),
        ):
            with self.subTest(case=name):
                result, _ = run_macos_arch_step("x86_64-apple-darwin", lipo="x86_64",
                                                file=MACH_O_FILE_DESCRIPTIONS["x86_64"], libraries=libraries)
                self.assertNotEqual(result.returncode, 0)

    def run_smoke(self, target: str, outputs: dict[str, str]) -> tuple[subprocess.CompletedProcess[str], list[str]]:
        with fixture_directory() as tmp:
            root = Path(tmp)
            log = root / "arch.log"
            fake_arch = root / "arch"
            fake_arch.write_text('#!/usr/bin/env bash\nprintf "%s\\n" "$*" >> "$FAKE_ARCH_LOG"\n'
                                 '[[ "$1" == "-x86_64" ]] || exit 98\nshift\nexec "$@"\n')
            fake_arch.chmod(0o755)
            for name, output in outputs.items():
                binary = root / "target" / target / "release" / name
                binary.parent.mkdir(parents=True, exist_ok=True)
                binary.write_text(f'#!/usr/bin/env bash\n[[ "$1" == "--version" ]] && echo "{output}"\n')
                binary.chmod(0o755)
            script = step_script("Smoke-run macOS gateways").replace("/usr/bin/arch", str(fake_arch)) \
                .replace("/usr/bin/true", "true")
            env = {**os.environ, "TARGET": target, "VERSION": "0.9.2", "FAKE_ARCH_LOG": str(log)}
            result = subprocess.run(["bash", "-c", script], cwd=root, env=env, capture_output=True, text=True)
            calls = log.read_text().splitlines() if log.exists() else []
        return result, calls

    def test_gateways_load_and_report_the_release_version_under_rosetta_or_natively(self):
        good = {"meta_gateway": "meta_gateway 0.9.2 (console)", "rpc_gateway": "rpc_gateway 0.9.2 (rpc)"}
        result, calls = self.run_smoke("x86_64-apple-darwin", good)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertTrue(any("release/meta_gateway --version" in call for call in calls), calls)
        result, calls = self.run_smoke("aarch64-apple-darwin", good)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse([call for call in calls if "--version" in call], "arm64 runs natively")
        result, _ = self.run_smoke("x86_64-apple-darwin", {**good, "rpc_gateway": "rpc_gateway 0.9.1 (rpc)"})
        self.assertNotEqual(result.returncode, 0)

    def test_host_architecture_code_from_a_cross_build_is_refused(self):
        result, _ = run_macos_arch_step(
            "x86_64-apple-darwin", lipo="arm64", file=MACH_O_FILE_DESCRIPTIONS["arm64"])
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("expected exactly 'x86_64'", result.stderr)

    def test_a_universal_binary_is_refused(self):
        result, _ = run_macos_arch_step(
            "x86_64-apple-darwin", lipo="x86_64 arm64",
            file="Mach-O universal binary with 2 architectures: [x86_64:Mach-O 64-bit executable x86_64]")
        self.assertNotEqual(result.returncode, 0)

    def test_one_wrong_binary_fails_the_whole_set(self):
        result, checked = run_macos_arch_step(
            "x86_64-apple-darwin", lipo="x86_64", file=MACH_O_FILE_DESCRIPTIONS["x86_64"],
            overrides={"meta_repair": ("arm64", MACH_O_FILE_DESCRIPTIONS["arm64"])})
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("target/x86_64-apple-darwin/release/meta_repair", checked)

    def test_file_must_agree_with_lipo(self):
        for description in (
            MACH_O_FILE_DESCRIPTIONS["arm64"],
            "Mach-O 64-bit executable x86_64h",
            "Mach-O 64-bit dynamically linked shared library x86_64",
            "ELF 64-bit LSB pie executable, x86-64",
        ):
            with self.subTest(description=description):
                result, _ = run_macos_arch_step("x86_64-apple-darwin", lipo="x86_64", file=description)
                self.assertNotEqual(result.returncode, 0)

    def test_both_spellings_of_file_are_accepted(self):
        for description in (
            "Mach-O 64-bit executable x86_64",
            "Mach-O 64-bit x86_64 executable, flags:<NOUNDEFS|DYLDLINK|TWOLEVEL|PIE>",
        ):
            with self.subTest(description=description):
                result, _ = run_macos_arch_step("x86_64-apple-darwin", lipo="x86_64", file=description)
                self.assertEqual(result.returncode, 0, result.stderr)

    def test_a_target_without_a_known_architecture_is_refused(self):
        result, checked = run_macos_arch_step(
            "x86_64-unknown-linux-gnu", lipo="x86_64", file=MACH_O_FILE_DESCRIPTIONS["x86_64"])
        self.assertNotEqual(result.returncode, 0)
        self.assertEqual(checked, [])


def hashed_env(block: str, indent: str) -> dict[str, str]:
    """The variables rust-cache hashes into its key, from one `env:` level.

    rust-cache hashes every variable whose name starts with one of these
    prefixes, with the Rust version, into the key; the toolchain action sets
    CARGO_HOME and CARGO_INCREMENTAL the same way in both workflows.
    """
    pattern = re.compile(rf"(?m)^{indent}((?:CARGO|CC|CFLAGS|CXX|CMAKE|RUST)\w*): (.*)$")
    return dict(pattern.findall(block))


class ReleaseCacheReuseTests(unittest.TestCase):
    def test_release_runs_save_no_rust_cache(self):
        """Release jobs saved entries keyed on Cargo.lock that no later
        release restored (every v0.9.2 leg found "No cache found") while they
        took a third of the repository's cache budget. The only rust cache
        left is verify_crate_packages' read-only restore of main CI's entry,
        and it restores no ~/.cargo/bin executable."""
        source = RELEASE_WORKFLOW.read_text()
        self.assertNotIn("cache-on-failure", source)
        steps = re.findall(r"- (?:name: [^\n]+\n\s+)?uses: Swatinem/rust-cache@[0-9a-f]{40} # v2\n((?: {8}.*\n)*)",
                           source)
        self.assertEqual(len(steps), source.count("uses: Swatinem/rust-cache@"))
        self.assertEqual(len(steps), 1)
        self.assertIn("          save-if: false\n          cache-bin: false\n", steps[0])
        for job in ("build_binaries", "release_validate", "publish_registries"):
            with self.subTest(job=job):
                self.assertNotIn("rust-cache", job_block(job))
        self.assertFalse((ROOT / ".github/workflows/release-prewarm.yml").exists())

    def test_crate_verification_restores_the_exact_main_ci_dependency_build(self):
        """cargo package's verification build restores the meta-dev-bins
        entry CI saved for the same commit, in a job that holds no secret.

        rust-cache keys on the shared key, the runner and the hashed
        environment, and the cache paths follow cache-bin, so every one of
        them must equal what test-python and flow-editor use in ci.yml, or
        the restore silently misses.
        """
        ci_workflow = ROOT / ".github/workflows/ci.yml"
        ci_source = ci_workflow.read_text()

        def ci_job(name: str) -> str:
            lines = ci_source.splitlines()
            start = lines.index(f"  {name}:")
            for offset, line in enumerate(lines[start + 1:], start=start + 1):
                if re.fullmatch(r"  [A-Za-z_][A-Za-z0-9_-]*:", line):
                    return "\n".join(lines[start:offset])
            return "\n".join(lines[start:])

        verify = job_block("verify_crate_packages")
        cache = step_block("Restore the exact-main CI dependency build", "verify_crate_packages")
        self.assertEqual(scalar(cache, "shared-key", "          "), "meta-dev-bins")
        self.assertEqual(scalar(cache, "save-if", "          "), "false")
        self.assertEqual(scalar(cache, "cache-bin", "          "), "false")
        release_header = RELEASE_WORKFLOW.read_text().split("\njobs:\n", 1)[0]
        ci_header = ci_source.split("\njobs:\n", 1)[0]
        release_env = {**hashed_env(release_header, "  "), **hashed_env(verify, "      ")}
        self.assertEqual(release_env["CARGO_TARGET_DIR"], "${{ github.workspace }}/target")
        for name in ("test-python", "flow-editor"):
            with self.subTest(ci_job=name):
                block = ci_job(name)
                self.assertIn("          shared-key: meta-dev-bins\n", block)
                self.assertIn("          cache-bin: false\n", block)
                self.assertEqual(
                    {**hashed_env(ci_header, "  "), **hashed_env(block, "      ")}, release_env)
                self.assertEqual(scalar(block, "runs-on", "    "), scalar(verify, "runs-on", "    "))
        self.assertIn("scripts/repo-cargo package -p meta-extension-state -p meerkat-meta --locked",
                      step_block("Verify the packaged crates build", "verify_crate_packages"))
        self.assertNotIn("secrets.", verify)
        self.assertNotIn("permissions:", verify)

    # Jobs that hold a secret or a write grant and still restore a cache, and
    # why that cache is trusted. e2e-live restores without saving and without
    # executables, and its provider secret reaches only the step that makes
    # the real calls.
    CACHE_HOLDER_ALLOWLIST = {
        ("e2e-live.yml", "e2e-live"),
    }

    def test_no_job_holding_a_secret_or_write_grant_restores_a_cache(self):
        """A cache entry can carry a planted executable or object, so no job
        in any workflow that holds a secret or a write permission (id-token,
        attestations, contents, actions or any other scope) may restore one,
        except the allowlisted jobs above. Publication verifies with
        --no-verify only what the secret-free verification job built."""
        holders_seen = set()
        for workflow in sorted((ROOT / ".github/workflows").glob("*.yml")):
            source = workflow.read_text()
            code_lines = [line for line in source.splitlines() if not line.strip().startswith("#")]
            header = "\n".join(code_lines).split("\njobs:\n", 1)[0]
            jobs_text = "\n".join(code_lines).split("\njobs:\n", 1)[1]
            names = re.findall(r"(?m)^  ([A-Za-z_][A-Za-z0-9_-]*):$", jobs_text)
            for name in names:
                block = jobs_text.split(f"\n  {name}:\n", 1)[1] if not jobs_text.startswith(f"  {name}:") \
                    else jobs_text[len(f"  {name}:\n"):]
                block = re.split(r"(?m)^  [A-Za-z_][A-Za-z0-9_-]*:$", block, 1)[0]
                holds = ("secrets." in block or re.search(r"(?m)^\s+[a-z-]+: write$", block)
                         or re.search(r"(?m)^  [a-z-]+: write$", header))
                restores = ("rust-cache" in block or "actions/cache" in block
                            or re.search(r"(?m)^\s+cache(-dependency-path)?:", block))
                with self.subTest(workflow=workflow.name, job=name):
                    if holds and restores:
                        self.assertIn((workflow.name, name), self.CACHE_HOLDER_ALLOWLIST)
                    if holds:
                        holders_seen.add((workflow.name, name))
        # The scan sees the jobs it must (and the allowlist is no wider).
        for expected in (("release.yml", "publish_registries"), ("release.yml", "publish_sdk_packages"),
                         ("release.yml", "collect_candidate"), ("release.yml", "publish_github_release"),
                         ("ci.yml", "drop-superseded-caches"), ("publish-docs.yml", "dispatch")):
            self.assertIn(expected, holders_seen)
        self.assertLessEqual(self.CACHE_HOLDER_ALLOWLIST, holders_seen)
        e2e = (ROOT / ".github/workflows/e2e-live.yml").read_text()
        self.assertEqual(e2e.count("${{ secrets.ANTHROPIC_API_KEY }}"), 1)
        self.assertIn("          save-if: false\n          cache-bin: false\n", e2e)
        self.assertIn("verify_crate_packages", job_needs("publish_registries"))
        self.assertIn("needs.verify_crate_packages.result == 'success'", job_condition("publish_registries"))
        for step in ("Publish extension document crate", "Publish Rust crate"):
            script = step_script(step, "publish_registries")
            self.assertEqual(script.count("--locked --no-verify"), 2, step)


if __name__ == "__main__":
    unittest.main()
