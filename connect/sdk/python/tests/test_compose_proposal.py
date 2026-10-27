"""Independent proposal tests using the real planner and inert engine observations.

Set TOOLKIT_COMPOSE_PLANNER to an existing canonical planner executable. These
tests never invoke Docker or start an integration.
"""
from __future__ import annotations

import asyncio
import copy
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import stat
import subprocess
import sys

import pytest

import meerkat_toolkit.compose as compose_module
from meerkat_toolkit.compose import CommandResult, ComposeError, ComposeRuntime, ProcessRunner


ROOT = Path(__file__).resolve().parents[3]
EXAMPLE = ROOT / "docs/verification/20261004-compose-lifecycle/canonical/request.example.json"


def wire(value):
    return (json.dumps(value, sort_keys=True, ensure_ascii=False, separators=(",", ":")) + "\n").encode()


def sha(raw):
    return hashlib.sha256(raw).hexdigest()


@pytest.fixture
def planner():
    path = os.environ.get("TOOLKIT_COMPOSE_PLANNER")
    if not path:
        pytest.skip("set TOOLKIT_COMPOSE_PLANNER to exercise the real canonical planner")
    resolved = Path(path).resolve(strict=True)
    assert resolved.is_file() and os.access(resolved, os.X_OK)
    return resolved, sha(resolved.read_bytes())


@pytest.fixture
def proposal(tmp_path, planner):
    example = json.loads(EXAMPLE.read_bytes())
    profile = json.loads(example["profile_json"])
    tools = {}
    for name in ("docker", "compose"):
        path = tmp_path / name
        path.write_bytes(b"#!/bin/sh\nexit 99\n")
        path.chmod(0o700)
        tools[name] = {"path": str(path), "sha256": sha(path.read_bytes())}
    engine = dict(profile["engine"], **tools, id="proposal-test-engine")
    host = {key: profile[key] for key in ("schema_version", "owner", "placement", "host", "project")}
    host.update(scope="prepare_agent_selected_local_container_plan", engine=engine)
    selection = {key: example[key] for key in (
        "schema_version", "package_json", "manifest_json", "configuration", "policy", "files")}
    selection["components"] = {
        key: {field: value for field, value in component.items() if field != "image_id"}
        for key, component in profile["components"].items()
    }
    selection["volumes"] = profile["volumes"]
    # Non-ASCII and significant whitespace must survive the full handoff.
    selection["configuration"] = '{ "purpose": "observed \u00c5 schema" }\n\n'
    manifest = json.loads(selection["manifest_json"])
    manifest["configuration"]["content_digest"] = sha(selection["configuration"].encode())
    selection["manifest_json"] = json.dumps(manifest, ensure_ascii=False, indent=2) + "\n"
    host_raw = json.dumps(host, indent=2).encode() + b"\n\n"
    selection_raw = json.dumps(selection, ensure_ascii=False, indent=2).encode() + b"\n"
    return host, selection, host_raw, selection_raw, planner


class ProbeRunner(ProcessRunner):
    """The actual planner runs; only explicit read-only engine calls are simulated."""

    def __init__(self, planner, *, failure=None):
        self.planner = str(planner)
        self.failure = failure
        self.calls = []
        self.plan_requests = []
        self.info_count = 0
        self.entered = asyncio.Event()

    async def run(self, argv, *, cwd, env, **kwargs):
        self.calls.append((list(argv), dict(env)))
        if argv == [self.planner]:
            self.plan_requests.append(kwargs["stdin"])
            return await super().run(argv, cwd=cwd, env=env, **kwargs)
        assert argv[1] == "--host" and argv[3] == "--config"
        assert argv[4] == str(cwd / ".docker")
        args = argv[5:]
        assert args == ["info", "--format", "{{json .ID}}"] or (
            len(args) == 5 and args[:3] == ["image", "inspect", "--platform"]
        ), f"proposal attempted a non-observation operation: {args!r}"
        if args[0] == "info":
            self.info_count += 1
            identity = "other-engine" if self.failure == "engine_changed" and self.info_count > 1 else "proposal-test-engine"
            return CommandResult(0, wire(identity))
        image = args[-1]
        identity = image if image.startswith("sha256:") else "sha256:" + "f" * 64
        if self.failure == "image_changed" and len(self.plan_requests) > 1:
            identity = "sha256:" + "9" * 64
        item = {"Id": identity, "Os": "linux", "Architecture": "arm64", "Config": {"Env": ["BASE=observed"]}}
        if self.failure == "wrong_platform":
            item["Architecture"] = "amd64"
        if self.failure == "unbound_volume":
            item["Config"]["Volumes"] = {"/not-selected": {}}
        if self.failure == "bad_environment":
            item["Config"]["Env"] = ["DUP=one", "DUP=two"]
        if self.failure == "bad_descriptor":
            item["Descriptor"] = {"digest": "sha256:" + "8" * 64}
        if self.failure == "tamper_tool":
            Path(argv[0]).write_bytes(b"changed after first observation")
        if self.failure == "missing":
            return CommandResult(1, b"", b"private engine failure body")
        if self.failure == "malformed":
            return CommandResult(0, wire({"unexpected": "private observation body"}))
        return CommandResult(0, wire([item]))


def runtime_for(tmp_path, proposal, failure=None):
    planner, digest = proposal[-1]
    runner = ProbeRunner(planner, failure=failure)
    runtime = ComposeRuntime(tmp_path / "proposal", planner, digest, runner=runner)
    return runtime, runner


def no_publication(runtime):
    assert not (runtime.workspace / "proposal.json").exists()
    assert not (runtime.workspace / "record.json").exists()


@pytest.mark.asyncio
async def test_proposal_uses_actual_planner_and_preserves_exact_setup_handoff(tmp_path, proposal, monkeypatch):
    runtime, runner = runtime_for(tmp_path, proposal)
    host, selection, host_raw, selection_raw, _ = proposal
    monkeypatch.setenv("DOCKER_HOST", "tcp://must-not-be-used:1234")
    monkeypatch.setenv("DOCKER_CONTEXT", "must-not-be-used")
    monkeypatch.setenv("PROPOSAL_PRIVATE_CANARY", "must-not-reach-probe")
    receipt = await runtime.propose(host_raw, selection_raw)
    assert set(receipt) == {"schema_version", "status", "scope", "plan_sha256", "files"}
    assert receipt["schema_version"] == 1
    assert receipt["status"] == "proposed"
    assert receipt["scope"] == "compose_plan_for_host_review"
    assert set(receipt["files"]) == {
        "host-profile.json", "selection.json", "backend-profile.json", "inputs.json", "request.json", "prepared.json"
    }
    for name, expected in receipt["files"].items():
        assert sha((runtime.workspace / name).read_bytes()) == expected
    assert (runtime.workspace / "host-profile.json").read_bytes() == host_raw
    assert (runtime.workspace / "selection.json").read_bytes() == selection_raw
    assert json.loads((runtime.workspace / "proposal.json").read_bytes()) == receipt
    assert not (runtime.workspace / "record.json").exists()
    assert not (runtime.workspace / "compose.json").exists()
    docker_config = runtime.workspace / ".docker/config.json"
    assert docker_config.read_bytes() == b"{}\n"
    assert stat.S_IMODE(docker_config.stat().st_mode) & 0o222 == 0
    profile_raw = (runtime.workspace / "backend-profile.json").read_bytes()
    profile = json.loads(profile_raw)
    assert profile["engine"] == host["engine"]
    assert profile["components"]["mcp"]["image_id"] == "sha256:" + "e" * 64
    assert profile["components"]["database"]["image_id"] == "sha256:" + "f" * 64
    assert profile["components"]["api"]["environment"] == {}
    assert profile["files"] == {key: sha(value.encode()) for key, value in selection["files"].items()}
    inputs = json.loads((runtime.workspace / "inputs.json").read_bytes())
    assert set(inputs) == {"schema_version", "package_json", "manifest_json", "configuration", "policy", "files"}
    assert inputs == {key: selection[key] for key in inputs}
    spec = importlib.util.spec_from_file_location("proposal_test_compose_bridge", ROOT / "sdk/consumer-setup/compose_bridge.py")
    bridge = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(bridge)
    handoff = bridge.wire(dict(inputs, profile_json=profile_raw.decode(), profile_sha256=sha(profile_raw)))
    assert handoff == (runtime.workspace / "request.json").read_bytes()
    # The actual unchanged bridge encoding must regenerate exactly the reviewed plan.
    regenerated = await ProcessRunner().run([str(proposal[-1][0])], cwd=runtime.workspace,
        env=runtime._env(), stdin=handoff)
    assert regenerated.returncode == 0 and not regenerated.stderr
    prepared = json.loads((runtime.workspace / "prepared.json").read_bytes())
    assert json.loads(regenerated.stdout) == prepared
    assert prepared["plan_sha256"] == receipt["plan_sha256"]
    assert prepared["plan"]["request_sha256"] == sha(handoff)
    assert len(runner.plan_requests) == 2
    assert runner.plan_requests[0] != runner.plan_requests[1]
    for argv, environment in runner.calls:
        assert not {"DOCKER_HOST", "DOCKER_CONTEXT", "PROPOSAL_PRIVATE_CANARY"} & environment.keys()


@pytest.mark.parametrize("failure", [
    "engine_changed", "image_changed", "wrong_platform", "unbound_volume",
    "bad_environment", "bad_descriptor", "tamper_tool", "missing", "malformed",
])
@pytest.mark.asyncio
async def test_proposal_refuses_changed_or_invalid_observations_without_publication(tmp_path, proposal, failure):
    runtime, runner = runtime_for(tmp_path, proposal, failure)
    with pytest.raises(ComposeError) as error:
        await runtime.propose(proposal[2], proposal[3])
    assert "private" not in str(error.value)
    no_publication(runtime)
    assert runner.calls


@pytest.mark.parametrize("change", ["host_scope", "host_owner", "image_id", "profile_json", "component_set", "source_digest"])
@pytest.mark.asyncio
async def test_proposal_rejects_invalid_authority_or_selection_before_engine(tmp_path, proposal, change):
    runtime, runner = runtime_for(tmp_path, proposal)
    host, selection = copy.deepcopy(proposal[:2])
    if change == "host_scope":
        host["scope"] = "execute_everything"
    elif change == "host_owner":
        host["owner"] = "other-owner"
    elif change == "image_id":
        selection["components"]["database"]["image_id"] = "sha256:" + "a" * 64
    elif change == "profile_json":
        selection["profile_json"] = "{}"
    elif change == "component_set":
        del selection["components"]["database"]
    else:
        manifest = json.loads(selection["manifest_json"])
        manifest["components"][0]["source"]["content_digest"] = "7" * 64
        selection["manifest_json"] = wire(manifest).decode()
    with pytest.raises(ComposeError):
        await runtime.propose(wire(host), wire(selection))
    no_publication(runtime)
    assert all(argv == [str(proposal[-1][0])] for argv, _ in runner.calls)


@pytest.mark.parametrize("document", ["host", "selection"])
@pytest.mark.asyncio
async def test_proposal_rejects_duplicate_input_keys_before_observation(tmp_path, proposal, document):
    runtime, runner = runtime_for(tmp_path, proposal)
    host_raw, selection_raw = proposal[2:4]
    if document == "host":
        host_raw = host_raw.rstrip()[:-1] + b',"owner":"substituted"}'
    else:
        selection_raw = selection_raw.rstrip()[:-1] + b',"configuration":"substituted"}'
    with pytest.raises(ComposeError, match="^invalid_json$"):
        await runtime.propose(host_raw, selection_raw)
    assert not runner.calls
    no_publication(runtime)


@pytest.mark.asyncio
async def test_existing_or_aliased_proposal_destination_preserves_prior_files(tmp_path, proposal):
    runtime, runner = runtime_for(tmp_path, proposal)
    runtime.workspace.mkdir()
    retained = runtime.workspace / "evidence"
    retained.write_bytes(b"owner files")
    with pytest.raises(ComposeError, match="^workspace_exists$"):
        await runtime.propose(proposal[2], proposal[3])
    assert retained.read_bytes() == b"owner files"
    assert not runner.calls
    alias = tmp_path / "alias"
    alias.symlink_to(runtime.workspace, target_is_directory=True)
    runtime.workspace = alias
    with pytest.raises(ComposeError, match="^workspace_unavailable$"):
        await runtime.propose(proposal[2], proposal[3])
    assert retained.read_bytes() == b"owner files"
    assert not runner.calls


@pytest.mark.asyncio
async def test_failed_publication_keeps_evidence_without_success_receipt(tmp_path, proposal, monkeypatch):
    runtime, _ = runtime_for(tmp_path, proposal)
    original_write = compose_module._write

    def interrupted_write(path, data, **kwargs):
        if path.name == "request.json":
            raise OSError("simulated publication interruption")
        return original_write(path, data, **kwargs)

    monkeypatch.setattr(compose_module, "_write", interrupted_write)
    with pytest.raises(OSError, match="publication interruption"):
        await runtime.propose(proposal[2], proposal[3])
    no_publication(runtime)
    assert (runtime.workspace / "host-profile.json").read_bytes() == proposal[2]
    assert (runtime.workspace / "selection.json").read_bytes() == proposal[3]
    retained = {p.name: p.read_bytes() for p in runtime.workspace.iterdir() if p.is_file()}
    with pytest.raises(ComposeError, match="^workspace_exists$"):
        await runtime.propose(proposal[2], proposal[3])
    assert retained == {p.name: p.read_bytes() for p in runtime.workspace.iterdir() if p.is_file()}


@pytest.mark.asyncio
async def test_concurrent_same_output_attempt_does_not_join_or_replace_first(tmp_path, proposal):
    runtime, runner = runtime_for(tmp_path, proposal)
    entered, release = asyncio.Event(), asyncio.Event()
    original_run = runner.run

    async def held_probe(argv, **kwargs):
        if argv[5:7] == ["image", "inspect"] and not entered.is_set():
            entered.set()
            await release.wait()
        return await original_run(argv, **kwargs)

    runner.run = held_probe
    first = asyncio.create_task(runtime.propose(proposal[2], proposal[3]))
    try:
        async with asyncio.timeout(10):
            await entered.wait()
        before = len(runner.calls)
        with pytest.raises(ComposeError, match="^workspace_exists$"):
            await runtime.propose(proposal[2], proposal[3])
        assert len(runner.calls) == before
        no_publication(runtime)
        release.set()
        receipt = await first
        assert json.loads((runtime.workspace / "proposal.json").read_bytes()) == receipt
        assert len(runner.plan_requests) == 2
    finally:
        release.set()
        if not first.done():
            first.cancel()
            await asyncio.gather(first, return_exceptions=True)


def subprocess_probe(proposal, mode):
    """Replace the inert executable with an actual bounded, non-Docker process."""
    host = copy.deepcopy(proposal[0])
    path = Path(host["engine"]["docker"]["path"])
    program = "#!" + sys.executable + "\n" + (
        "import json, os, pathlib, sys, time\n"
        "pathlib.Path('probe.pid').write_text(str(os.getpid()))\n"
        "args = sys.argv[5:]\n"
        "if args == ['info', '--format', '{{json .ID}}']:\n"
        "    print(json.dumps('proposal-test-engine'))\n"
        "elif args[:3] == ['image', 'inspect', '--platform']:\n"
        "    pathlib.Path('probe.started').write_text('image')\n"
    )
    if mode == "success":
        program += (
            "    identity = args[-1] if args[-1].startswith('sha256:') else 'sha256:' + 'f' * 64\n"
            "    print(json.dumps([{'Id': identity, 'Os': 'linux', 'Architecture': 'arm64', 'Config': {'Env': ['BASE=observed']}}]))\n"
        )
    elif mode == "overflow":
        program += "    sys.stdout.buffer.write(b'x' * (5 * 1024 * 1024)); sys.stdout.flush()\n"
    else:
        program += "    time.sleep(60)\n"
    program += "else:\n    raise RuntimeError('unexpected effect')\n"
    path.write_text(program)
    path.chmod(0o700)
    host["engine"]["docker"]["sha256"] = sha(path.read_bytes())
    return wire(host)


@pytest.mark.asyncio
async def test_real_observation_output_overflow_keeps_proposal_unpublished(tmp_path, proposal):
    host_raw = subprocess_probe(proposal, "overflow")
    runtime = ComposeRuntime(tmp_path / "proposal", *proposal[-1])
    with pytest.raises(ComposeError, match="^command_output_limit$"):
        await runtime.propose(host_raw, proposal[3])
    no_publication(runtime)
    pid = int((runtime.workspace / "probe.pid").read_text())
    with pytest.raises(ProcessLookupError):
        os.kill(pid, 0)


@pytest.mark.asyncio
async def test_real_observation_cancellation_joins_child_before_return(tmp_path, proposal):
    host_raw = subprocess_probe(proposal, "wait")
    runtime = ComposeRuntime(tmp_path / "proposal", *proposal[-1])
    task = asyncio.create_task(runtime.propose(host_raw, proposal[3]))
    try:
        async with asyncio.timeout(10):
            while not (runtime.workspace / "probe.started").exists():
                await asyncio.sleep(0.01)
        pid = int((runtime.workspace / "probe.pid").read_text())
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        with pytest.raises(ProcessLookupError):
            os.kill(pid, 0)
        no_publication(runtime)
    finally:
        if not task.done():
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)


def test_cli_rejects_wrong_host_pin_without_disclosing_inputs(tmp_path, proposal):
    host_path = tmp_path / "host.json"
    host_path.write_bytes(proposal[2])
    planner, planner_sha = proposal[-1]
    result = subprocess.run([sys.executable, "-m", "meerkat_toolkit.compose",
        "--workspace", str(tmp_path / "cli-proposal"), "--planner", str(planner),
        "--planner-sha256", planner_sha, "propose", "--host-profile", str(host_path),
        "--host-profile-sha256", "0" * 64], input=proposal[3], capture_output=True,
        env=dict(os.environ, PYTHONPATH=str(ROOT / "sdk/python/src")), timeout=15)
    assert result.returncode != 0
    assert not result.stdout
    assert b"proposal-test-engine" not in result.stderr
    assert b"observed" not in result.stderr
    assert not (tmp_path / "cli-proposal/proposal.json").exists()


def test_cli_success_returns_only_hash_receipt_using_real_processes(tmp_path, proposal):
    host_raw = subprocess_probe(proposal, "success")
    host_path = tmp_path / "host.json"
    host_path.write_bytes(host_raw)
    planner, planner_sha = proposal[-1]
    workspace = tmp_path / "cli-proposal"
    result = subprocess.run([sys.executable, "-m", "meerkat_toolkit.compose",
        "--workspace", str(workspace), "--planner", str(planner),
        "--planner-sha256", planner_sha, "propose", "--host-profile", str(host_path),
        "--host-profile-sha256", sha(host_raw)], input=proposal[3], capture_output=True,
        env=dict(os.environ, PYTHONPATH=str(ROOT / "sdk/python/src")), timeout=15)
    assert result.returncode == 0, result.stderr
    assert not result.stderr
    receipt = json.loads(result.stdout)
    assert receipt == json.loads((workspace / "proposal.json").read_bytes())
    assert receipt["status"] == "proposed"
    assert b"observed" not in result.stdout
    assert b"db-password" not in result.stdout
    assert b"BASE=" not in result.stdout
    assert not (workspace / "record.json").exists()
