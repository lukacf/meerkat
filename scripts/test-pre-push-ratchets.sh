#!/usr/bin/env bash
# Contracts for the changed-path ratchets hook, which replaced the hook's
# Clippy body and is now a manual hook: no push-stage hook runs the ratchets,
# Clippy, the deterministic test lanes or the machine codegen/TLC lanes.
#
# Dropping them from the push keeps coverage because CI runs each before
# merge: Clippy is a required CI check (the CI planner,
# scripts/ci-cargo-lanes.mjs, gives every package the agent gate would lint a
# Clippy lane, or plans the whole workspace; those lanes run
# `clippy --no-deps <targets> --all-features -- -D warnings`, every target
# except a pull request's meerkat-mob chain shards, which lint their lib and
# bin targets; and the CI gate requires them whenever Rust changed), and the
# ratchets and governance-gates jobs, example-web and wasm-check run the
# ratchets for the same changed paths. This test pins those facts, checks the
# package claim on sample changes against the agent gate's own selection,
# checks that every ratchet the agent gate selects for a sample change has a
# CI step the plan selects for that change on both events, and checks that
# the manual hook keeps the agent gate's ratchets and runs no Clippy and no
# tests. CI runs it (make ci-lanes-selftest, in Format + governance), and it
# needs no Python package beyond the standard library.
set -euo pipefail

REPO_ROOT="${REPO_ROOT:-$(cd "$(dirname "$0")/.." && pwd)}"
cd "$REPO_ROOT"

fail() {
  echo "pre-push ratchets contract: $*" >&2
  exit 1
}

config=.pre-commit-config.yaml
ci=.github/workflows/ci.yml

# 1. The ratchets hook stays as a manual hook, the push stage runs none of
#    the lanes CI runs, and nothing runs the old Clippy body.
grep -Fq -- '- id: changed-path-ratchets' "$config" || fail "the changed-path-ratchets hook is missing"
grep -Fq 'entry: scripts/pre-push-ratchets.sh' "$config" || fail "the ratchets hook does not run scripts/pre-push-ratchets.sh"
if grep -Eq 'pre-push-clippy|id: cargo-clippy' "$config"; then
  fail "a pre-push hook still runs the Clippy body"
fi
python3 - "$config" <<'PY' || fail "a push-stage hook runs a lane CI runs before merge"
import re
import sys

# The config's own layout, read without a YAML package: each hook starts at
# `      - id: NAME` and its keys are indented eight spaces; stages are flow
# lists. Anything else is refused rather than guessed.
text = open(sys.argv[1]).read()
default = re.search(r"(?m)^default_stages: \[([^\]]*)\]$", text)
assert default, "default_stages is not a flow list"

def listed(value):
    return [item.strip() for item in value.split(",") if item.strip()]

stages = {}
for block in re.split(r"(?m)^      - id: ", text)[1:]:
    hook_id = block.split("\n", 1)[0].strip()
    keys = re.findall(r"(?m)^        stages:(.*)$", block)
    assert len(keys) <= 1, f"{hook_id} names its stages twice"
    if keys:
        flow = re.fullmatch(r" \[([^\]]*)\]", keys[0])
        assert flow, f"{hook_id}: stages is not a flow list"
        stages[hook_id] = listed(flow.group(1))
    else:
        stages[hook_id] = listed(default.group(1))
for hook_id in ("changed-path-ratchets", "machine-codegen-drift", "machine-codegen-verify", "cargo-test"):
    assert stages.get(hook_id) == ["manual"], f"{hook_id} runs in {stages.get(hook_id)}, not only manually"
PY
[[ ! -e scripts/pre-push-clippy.sh ]] || fail "scripts/pre-push-clippy.sh exists again"
grep -Fq 'exec "$AGENT_GATE" --committed --ratchets-only --machine-drift-by-hook "$@"' scripts/pre-push-ratchets.sh ||
  fail "pre-push-ratchets.sh does not run the agent gate in ratchets-only mode"

# 2. The required CI Clippy lanes the hook relies on: flags, trigger, gate.
grep -Fxq '          ./scripts/repo-cargo clippy ${PACKAGES} --no-deps ${TARGETS} --all-features -- -D warnings' "$ci" ||
  fail "the CI Clippy lanes no longer run --no-deps <targets> --all-features -- -D warnings"
grep -Fxq '          TARGETS: ${{ matrix.targets }}' "$ci" ||
  fail "the CI Clippy lanes no longer take their targets from the plan"
python3 - "$ci" <<'PY' || fail "the CI Clippy job or its gate requirement changed"
import re
import sys

text = open(sys.argv[1]).read()
# The lanes run in two waves (long rows first): clippy holds the first,
# clippy-rest the rest, each on a Rust change when its wave holds rows.
for name, count, matrix in (("clippy", "shard_first_count", "shard_matrix"), ("clippy-rest", "shard_rest_count", "shard_rest_matrix")):
    job = re.search(rf"\n  {name}:\n(.*?)\n  [a-z][a-z0-9-]*:\n", text, re.S)
    assert job, f"no {name} job"
    assert f"    if: ${{{{ needs.changes.outputs.rust_changed == 'true' && needs.changes.outputs.{count} != '0' }}}}\n" in job.group(1), f"{name} job condition"
    assert f"      matrix: ${{{{ fromJSON(needs.changes.outputs.{matrix}) }}}}\n" in job.group(1), f"{name} matrix"
required = (
    '          if [[ "${RUST_CHANGED}" == "true" ]]; then\n'
    '            if ! [[ "${SHARD_COUNT}" =~ ^[0-9]+$ ]] || (( SHARD_COUNT < 1 )); then\n'
    "              echo \"::error::a build-relevant change produced no lanes (shard_count='${SHARD_COUNT}').\"\n"
    "              failed=1\n"
    "            fi\n"
    '            require_ran "Clippy" "${CLIPPY_RESULT}"\n'
)
assert required in text, "the gate does not require Clippy lanes on Rust changes"
PY

# 3. On sample changes, the CI Clippy lanes cover every package the agent
#    gate's Clippy selection (what the hook used to lint) would lint.
python3 - <<'PY' || fail "the CI planner does not cover the agent gate's Clippy selection"
import json
import re
import subprocess

def run(*args):
    result = subprocess.run(args, capture_output=True, text=True)
    if result.returncode != 0:
        raise SystemExit(f"{' '.join(args)} failed ({result.returncode}): {result.stderr[-2000:]}")
    return result.stdout

def embedded_input_sample():
    # The selector's own list: (path, package, kind) per embedded input.
    for line in run("scripts/rust-embedded-inputs.mjs").splitlines():
        path, package, kind = line.split("\t")
        if kind == "file":
            return path
    raise SystemExit("scripts/rust-embedded-inputs.mjs lists no embedded input file")

samples = [
    "crates/meerkat-runtime/src/lib.rs",
    "crates/meerkat-openai/Cargo.toml",
    embedded_input_sample(),
    "Cargo.lock",
    ".cargo/config.toml",
    "scripts/repo-cargo",
]
for path in samples:
    gate = run("scripts/cargo-agent-gate", "--clippy-only", "--dry-run", "--", path)
    if "clippy --workspace" in gate:
        hook = "workspace"
    else:
        match = re.search(r"Cargo agent gate packages:\n((?:  \S+\n)+)", gate)
        hook = set(match.group(1).split()) if match else set()
    for event in ("pull_request", "push"):
        plan = json.loads(run("node", "scripts/ci-cargo-lanes.mjs", "--event", event, "--format", "json", "--", path))
        ci = {name for shard in plan["shards"] for name in shard["packages"]}
        # Every target on the push; a pull request's mob-chain shards lint
        # their lib and bin targets.
        for shard in plan["shards"]:
            chain = all(plan["package_model"][name]["heavy_chain"] for name in shard["packages"])
            allowed = {"--all-targets", "--lib", "--bins", "--lib --bins"} if event == "pull_request" and chain else {"--all-targets"}
            if shard["targets"] not in allowed:
                raise SystemExit(f"{path} ({event}): shard {shard['name']} lints {shard['targets']}")
        if plan["mode"] == "workspace":
            print(f"covered: {path} ({event}): CI plans the whole workspace")
            continue
        if hook == "workspace":
            raise SystemExit(f"{path} ({event}): the agent gate lints the workspace, the CI planner only {sorted(ci)}")
        missing = hook - ci
        if missing:
            raise SystemExit(f"{path} ({event}): CI Clippy lanes miss {sorted(missing)} (planned {sorted(ci)})")
        print(f"covered: {path} ({event}): hook {sorted(hook)} within CI {sorted(ci)}")
PY

# 4. --ratchets-only keeps the ratchets and runs no Clippy and no tests.
ratchets() {
  scripts/cargo-agent-gate --ratchets-only --machine-drift-by-hook --dry-run -- "$@" 2>&1
}
expect_quiet() {
  local output
  output="$(ratchets "$@")" || fail "ratchets-only failed for $*"
  [[ "$output" == *"ratchets only"* ]] || fail "ratchets-only did not report itself for $*: ${output}"
  if grep -Eq 'clippy|nextest|fast --no-run' <<<"$output"; then
    fail "ratchets-only ran Clippy or tests for $*: ${output}"
  fi
}
expect_ratchet() {
  local expected="$1" output
  shift
  output="$(ratchets "$@")" || fail "ratchets-only failed for $*"
  grep -Fq -- "$expected" <<<"$output" || fail "ratchets-only lost '${expected}' for $*: ${output}"
  if grep -Eq 'clippy|nextest|fast --no-run' <<<"$output"; then
    fail "ratchets-only ran Clippy or tests for $*: ${output}"
  fi
}
expect_quiet crates/meerkat-runtime/src/lib.rs
expect_quiet Cargo.lock
expect_ratchet 'DRY-RUN make test-sdk-web' sdks/web/src/index.ts
expect_ratchet 'DRY-RUN make wasm-check' crates/meerkat-web-runtime/src/lib.rs
expect_ratchet 'DRY-RUN make machine-authority-docs-gate runtime-authority-bypass seam-inventory rmat-audit' \
  docs/architecture/meerkat-dogma.md
expect_ratchet 'DRY-RUN make verify-schema-freshness' docs/api/rpc.mdx
if scripts/cargo-agent-gate --ratchets-only --clippy-only --dry-run -- Cargo.lock >/dev/null 2>&1; then
  fail "--ratchets-only combined with --clippy-only was accepted"
fi

# 5. Every ratchet the hook ran keeps a CI step with the same trigger: for
#    each sample change, every make target the agent gate's ratchets select
#    maps to a CI job step that runs it, under a condition the plan's flag
#    alone satisfies, and the plan sets that flag for the change on a pull
#    request and on a push.
python3 - "$ci" <<'PY' || fail "a ratchet the hook ran has no CI step with the same trigger"
import json
import re
import subprocess
import sys

ci = open(sys.argv[1]).read()

def run(*args):
    result = subprocess.run(args, capture_output=True, text=True)
    if result.returncode != 0:
        raise SystemExit(f"{' '.join(args)} failed ({result.returncode}): {result.stderr[-2000:]}")
    return result.stdout

# make target -> (CI job, plan flag). test-sdk-web is the Makefile's
# `npm install`, `npm run build` (build:wasm, then build:ts), `npm test`
# (build:ts first) and `npm run test:packed` in sdks/web; example-web runs
# the same commands there: `npm test` on the job's flag, the wasm build and
# the packed-package smoke on example_web_wasm, the plan's flag for
# building the wasm in the job (a pull request that changes a crate the web
# runtime builds on leaves those to the push).
RATCHETS = {
    "machine-authority-docs-gate": ("governance-docs-gate", "governance_gates"),
    "runtime-authority-bypass": ("ratchets", "governance_gates"),
    "seam-inventory": ("governance-gates", "governance_gates"),
    "rmat-audit": ("governance-gates", "governance_gates"),
    "verify-schema-freshness": ("ratchets", "generated_contract"),
    "verify-sdk-codegen-freshness": ("ratchets", "generated_contract"),
    "verify-sdk-event-inventory": ("ratchets", "generated_contract"),
    "verify-rpc-surface-alignment": ("ratchets", "generated_contract"),
    "verify-rest-surface-alignment": ("ratchets", "generated_contract"),
    "test-sdk-web": ("example-web", "example_web"),
    "wasm-check": ("wasm-check", "wasm"),
}
WEB_SDK_COMMANDS = {
    "npm run build:wasm": "example_web_wasm",
    "npm test": "example_web",
    "npm run test:packed": "example_web_wasm",
}
# Flags beyond the job's that a ratchet's CI steps need.
STEP_FLAGS = {"test-sdk-web": ("example_web_wasm",)}

def job(name):
    match = re.search(rf"\n  {re.escape(name)}:\n(.*?)(?=\n  [a-z][a-z0-9-]*:\n|\Z)", ci, re.S)
    assert match, f"ci.yml has no {name} job"
    return match.group(1)

def condition(block, indent):
    lines = re.findall(rf"(?m)^{' ' * indent}if:(.*)$", block)
    if not lines:
        return None
    assert len(lines) == 1, f"two conditions at indent {indent}"
    match = re.fullmatch(r" \$\{\{ (.*) \}\}", lines[0])
    assert match, f"a condition this test cannot read: if:{lines[0]}"
    return match.group(1)

def disjuncts(expression):
    parts, depth, start = [], 0, 0
    for index, char in enumerate(expression):
        if char == "(":
            depth += 1
        elif char == ")":
            depth -= 1
        elif depth == 0 and expression.startswith("||", index):
            parts.append(expression[start:index].strip())
            start = index + 2
    parts.append(expression[start:].strip())
    return parts

def runs_on_flag(expression, flag):
    return expression is None or f"needs.changes.outputs.{flag} == 'true'" in disjuncts(expression)

def steps(block):
    found = []
    for chunk in re.split(r"(?m)^      - name: ", block)[1:]:
        lines = chunk.split("\n")
        directory = re.search(r"(?m)^        working-directory: (\S+)$", chunk)
        commands = []
        for index, line in enumerate(lines):
            if line.startswith("        run: "):
                value = line[len("        run: "):]
                if value == "|":
                    for body in lines[index + 1:]:
                        if body.strip() and not body.startswith("          "):
                            break
                        if body.strip():
                            commands.append(body.strip())
                else:
                    commands.append(value.strip())
        found.append({
            "name": lines[0].strip(),
            "if": condition(chunk, 8),
            "directory": directory.group(1) if directory else None,
            "commands": commands,
        })
    return found

for target, (job_name, flag) in sorted(RATCHETS.items()):
    block = job(job_name)
    head = block.split("\n    steps:\n", 1)[0]
    assert runs_on_flag(condition(head, 4), flag), f"job {job_name} does not run on {flag} alone"
    selected = [step for step in steps(block) if runs_on_flag(step["if"], flag)]
    if target == "test-sdk-web":
        missing = [
            command
            for command, step_flag in WEB_SDK_COMMANDS.items()
            if not any(
                step["directory"] == "sdks/web" and command in step["commands"] and runs_on_flag(step["if"], step_flag)
                for step in steps(block)
            )
        ]
        assert not missing, f"job {job_name} no longer runs {missing} in sdks/web on its flags"
        continue
    assert any(
        command.startswith("make ") and target in command.split()[1:]
        for step in selected
        for command in step["commands"]
    ), f"job {job_name} has no step that runs make {target} on {flag}"

samples = [
    "docs/api/rpc.mdx",
    "artifacts/schemas/version.json",
    "sdks/python/meerkat/generated/types.py",
    "crates/meerkat-machine-schema/src/lib.rs",
    "specs/machines/sample.yaml",
    "docs/architecture/meerkat-dogma.md",
    "docs/architecture/meerkat-dogma-commentary.md",
    "docs/internal/dogma-audits/sample.md",
    ".claude/skills/meerkat-dogma-inquisition/SKILL.md",
    "sdks/web/src/index.ts",
    "sdks/web/package.json",
    "crates/meerkat-web-runtime/src/lib.rs",
    "crates/meerkat-web-runtime/Cargo.toml",
]
for path in samples:
    gate = run("scripts/cargo-agent-gate", "--ratchets-only", "--machine-drift-by-hook", "--dry-run", "--", path)
    targets = sorted({
        target
        for line in gate.splitlines()
        if line.startswith("DRY-RUN make ")
        for target in line[len("DRY-RUN make "):].split()
    })
    if not targets:
        raise SystemExit(f"{path}: the agent gate selects no ratchet; the sample proves nothing")
    for event in ("pull_request", "push"):
        plan = json.loads(run("node", "scripts/ci-cargo-lanes.mjs", "--event", event, "--format", "json", "--", path))
        for target in targets:
            if target not in RATCHETS:
                raise SystemExit(f"{path}: the hook runs make {target}, which this test maps to no CI step")
            job_name, flag = RATCHETS[target]
            for needed in (flag, *STEP_FLAGS.get(target, ())):
                if plan.get(needed) is not True:
                    raise SystemExit(f"{path} ({event}): the hook runs make {target}, but the plan's {needed} is {plan.get(needed)}")
        print(f"covered: {path} ({event}): {' '.join(targets)}")
PY

echo "pre-push ratchets contracts hold"
