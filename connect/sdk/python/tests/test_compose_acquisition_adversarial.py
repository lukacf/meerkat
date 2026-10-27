"""Independent image-acquisition tests; real planner, inert engine processes.

These tests do not contact Docker or any registry. TOOLKIT_COMPOSE_PLANNER must
name the existing canonical planner used by the proposal suite.
"""
from __future__ import annotations

import asyncio
import copy
import importlib.util
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import time

import pytest

from meerkat_toolkit.compose import CommandResult, ComposeError, ComposeRuntime, ProcessRunner
from test_compose_proposal import ProbeRunner, no_publication, sha, wire
import test_compose_proposal as fixtures


planner = fixtures.planner
proposal = fixtures.proposal
ROOT = Path(__file__).resolve().parents[3]
SCOPE = "prepare_agent_selected_local_container_plan_with_image_pull"


def authority(proposal, registries=None):
    host = copy.deepcopy(proposal[0])
    host.update(scope=SCOPE, allowed_registries=["registry.example"] if registries is None else registries)
    return wire(host)


class AcquisitionRunner(ProbeRunner):
    def __init__(self, planner, *, failure=None):
        super().__init__(planner, failure=failure)
        self.pulls = []
        self.intent_snapshots = []
        self.on_pull = None

    async def run(self, argv, *, cwd, env, **kwargs):
        if argv[5:7] != ["image", "pull"]:
            return await super().run(argv, cwd=cwd, env=env, **kwargs)
        self.calls.append((list(argv), dict(env)))
        intent = json.loads((cwd / "image-acquisition.json").read_bytes())
        assert intent["status"] == "pending"
        assert intent["host_profile_sha256"] == sha((cwd / "host-profile.json").read_bytes())
        assert intent["selection_sha256"] == sha((cwd / "selection.json").read_bytes())
        assert any(row["image"] == argv[-1] and row["platform"] == argv[-2] and row["status"] == "pending"
                   for row in intent["images"])
        assert (cwd / ".docker/config.json").read_bytes() == b"{}\n"
        assert kwargs["timeout"] <= 120
        self.intent_snapshots.append(intent)
        self.pulls.append(argv[5:])
        if self.on_pull:
            await self.on_pull(cwd, len(self.pulls))
        return CommandResult(1 if self.failure == "pull_failed" else 0, b"private registry detail")


def make_runtime(tmp_path, proposal, *, failure=None):
    runner = AcquisitionRunner(proposal[-1][0], failure=failure)
    return ComposeRuntime(tmp_path / "proposal", *proposal[-1], runner=runner), runner


@pytest.mark.asyncio
async def test_explicit_pull_scope_records_deduplicated_intent_and_exact_setup_handoff(tmp_path, proposal):
    runtime, runner = make_runtime(tmp_path, proposal)
    host = authority(proposal)
    receipt = await runtime.propose(host, proposal[3], acquire_images=True)
    assert len(runner.pulls) == 1  # Three components share one immutable image/platform.
    record = json.loads((runtime.workspace / "image-acquisition.json").read_bytes())
    assert record["status"] == "observed"
    assert record["images"][0]["status"] == "observed"
    assert record["images"][0]["image_id"] == "sha256:" + "f" * 64
    assert record["images"][0]["image"].endswith("@sha256:" + "a" * 64)
    assert record["plan_sha256"] == receipt["plan_sha256"]
    for name, expected in receipt["files"].items():
        assert sha((runtime.workspace / name).read_bytes()) == expected
    profile = (runtime.workspace / "backend-profile.json").read_bytes()
    inputs = json.loads((runtime.workspace / "inputs.json").read_bytes())
    spec = importlib.util.spec_from_file_location("image_acquisition_bridge", ROOT / "sdk/consumer-setup/compose_bridge.py")
    bridge = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(bridge)
    request = bridge.wire(dict(inputs, profile_json=profile.decode(), profile_sha256=sha(profile)))
    assert request == (runtime.workspace / "request.json").read_bytes()
    result = await ProcessRunner().run([str(proposal[-1][0])], cwd=runtime.workspace, env=runtime._env(), stdin=request)
    assert result.returncode == 0
    assert json.loads(result.stdout)["plan_sha256"] == receipt["plan_sha256"]
    assert not (runtime.workspace / "record.json").exists()
    assert not any("private" in str(value) for value in receipt.values())


@pytest.mark.asyncio
async def test_scope_does_not_implicitly_activate_image_acquisition(tmp_path, proposal):
    runtime, runner = make_runtime(tmp_path, proposal)
    receipt = await runtime.propose(authority(proposal), proposal[3])
    assert runner.pulls == []
    assert "image-acquisition.json" not in receipt["files"]
    assert not (runtime.workspace / "image-acquisition.json").exists()


@pytest.mark.parametrize("flag", [True, 1, "true"])
@pytest.mark.asyncio
async def test_legacy_authority_never_permits_pull(tmp_path, proposal, flag):
    runtime, runner = make_runtime(tmp_path, proposal)
    with pytest.raises(ComposeError):
        await runtime.propose(proposal[2], proposal[3], acquire_images=flag)
    assert runner.calls == []
    no_publication(runtime)


@pytest.mark.parametrize("registries", [[], ["registry.example.evil"], ["REGISTRY.EXAMPLE"],
    ["registry.example", "registry.example"], ["https://registry.example"], ["user@registry.example"],
    ["registry.example/path"], ["*"], None, "registry.example"])
@pytest.mark.asyncio
async def test_origin_authority_refuses_before_any_engine_entry(tmp_path, proposal, registries):
    runtime, runner = make_runtime(tmp_path, proposal)
    host = json.loads(authority(proposal))
    host["allowed_registries"] = registries
    with pytest.raises(ComposeError):
        await runtime.propose(wire(host), proposal[3], acquire_images=True)
    assert all(argv == [str(proposal[-1][0])] for argv, _ in runner.calls)
    no_publication(runtime)


@pytest.mark.asyncio
async def test_later_unauthorized_image_prevents_earlier_authorized_pull(tmp_path, proposal):
    runtime, runner = make_runtime(tmp_path, proposal)
    selection = copy.deepcopy(proposal[1])
    manifest = json.loads(selection["manifest_json"])
    manifest["components"][2]["source"]["location"] = "https://other.example/v2/example/server/manifests/sha256:" + "a" * 64
    selection["manifest_json"] = wire(manifest).decode()
    with pytest.raises(ComposeError, match="image_registry_not_authorized"):
        await runtime.propose(authority(proposal), wire(selection), acquire_images=True)
    assert all(argv == [str(proposal[-1][0])] for argv, _ in runner.calls)
    assert runner.pulls == []


@pytest.mark.asyncio
async def test_invalid_graph_is_not_given_network_or_engine_access(tmp_path, proposal):
    runtime, runner = make_runtime(tmp_path, proposal)
    selection = copy.deepcopy(proposal[1])
    selection["policy"] = "changed without corresponding canonical digest"
    with pytest.raises(ComposeError, match="planning_failed"):
        await runtime.propose(authority(proposal), wire(selection), acquire_images=True)
    assert all(argv == [str(proposal[-1][0])] for argv, _ in runner.calls)
    no_publication(runtime)


@pytest.mark.parametrize("failure", ["pull_failed", "image_changed", "wrong_platform", "unbound_volume", "bad_descriptor", "tamper_tool"])
@pytest.mark.asyncio
async def test_pull_exit_or_invalid_final_observation_never_publishes(tmp_path, proposal, failure):
    runtime, runner = make_runtime(tmp_path, proposal, failure=failure)
    with pytest.raises(ComposeError):
        await runtime.propose(authority(proposal), proposal[3], acquire_images=True)
    assert len(runner.pulls) == 1
    record = json.loads((runtime.workspace / "image-acquisition.json").read_bytes())
    assert record["status"] == "unconfirmed"
    no_publication(runtime)


@pytest.mark.asyncio
async def test_cancellation_preserves_pending_intent_and_has_no_retry(tmp_path, proposal):
    runtime, runner = make_runtime(tmp_path, proposal)
    entered = asyncio.Event()
    async def hold(cwd, number):
        entered.set()
        await asyncio.Future()
    runner.on_pull = hold
    task = asyncio.create_task(runtime.propose(authority(proposal), proposal[3], acquire_images=True))
    try:
        async with asyncio.timeout(10):
            await entered.wait()
        task.cancel()
        with pytest.raises(asyncio.CancelledError):
            await task
        record = json.loads((runtime.workspace / "image-acquisition.json").read_bytes())
        assert record["status"] == "unconfirmed"
        assert record["images"][0]["status"] == "pending"
        with pytest.raises(ComposeError, match="workspace_exists"):
            await runtime.propose(authority(proposal), proposal[3], acquire_images=True)
        assert len(runner.pulls) == 1
        no_publication(runtime)
    finally:
        if not task.done():
            task.cancel()
            await asyncio.gather(task, return_exceptions=True)


@pytest.mark.asyncio
async def test_changed_retained_selection_cannot_be_published(tmp_path, proposal):
    runtime, runner = make_runtime(tmp_path, proposal)
    async def edit(cwd, number):
        (cwd / "selection.json").write_bytes(b"{}")
    runner.on_pull = edit
    with pytest.raises(ComposeError, match="proposal_inputs_changed"):
        await runtime.propose(authority(proposal), proposal[3], acquire_images=True)
    no_publication(runtime)


@pytest.mark.asyncio
async def test_empty_registry_authority_allows_local_only_without_pulls(tmp_path, proposal):
    runtime, runner = make_runtime(tmp_path, proposal)
    selection = copy.deepcopy(proposal[1])
    manifest = json.loads(selection["manifest_json"])
    for component in manifest["components"]:
        digest = component["source"]["content_digest"]
        component["source"]["location"] = "docker-image:///sha256/" + digest
    selection["manifest_json"] = wire(manifest).decode()
    await runtime.propose(authority(proposal, []), wire(selection), acquire_images=True)
    assert not runner.pulls
    record = json.loads((runtime.workspace / "image-acquisition.json").read_bytes())
    assert record["images"] == [] and record["status"] == "observed"


@pytest.mark.parametrize("failure", ["second_pull", "changed_config"])
@pytest.mark.asyncio
async def test_multi_image_failure_retains_completed_pending_and_unstarted_steps(tmp_path, proposal, failure):
    runtime, runner = make_runtime(tmp_path, proposal)
    selection = copy.deepcopy(proposal[1])
    manifest = json.loads(selection["manifest_json"])
    for number, component in enumerate(manifest["components"][:3]):
        component["source"]["location"] = (
            "https://registry.example/v2/ordered/image" + str(number) + "/manifests/sha256:" + "a" * 64)
    selection["manifest_json"] = wire(manifest).decode()

    async def fail(cwd, number):
        if failure == "second_pull" and number == 2:
            raise ComposeError("image_acquisition_failed")
        if failure == "changed_config" and number == 1:
            path = cwd / ".docker/config.json"
            path.chmod(0o600)
            path.write_bytes(b'{"auths":{"unexpected":"do-not-use"}}')
    runner.on_pull = fail
    with pytest.raises(ComposeError):
        await runtime.propose(authority(proposal), wire(selection), acquire_images=True)
    record = json.loads((runtime.workspace / "image-acquisition.json").read_bytes())
    assert record["status"] == "unconfirmed"
    if failure == "second_pull":
        assert len(runner.pulls) == 2
        assert [row["status"] for row in record["images"]] == ["client_completed", "pending", "not_started"]
    else:
        assert len(runner.pulls) == 1
        assert [row["status"] for row in record["images"]] == ["client_completed", "not_started", "not_started"]
    no_publication(runtime)


def test_real_cli_sigterm_joins_active_pull_child(tmp_path, proposal):
    host = json.loads(authority(proposal))
    docker = Path(host["engine"]["docker"]["path"])
    docker.write_text("#!" + sys.executable + "\n" +
        "import json,os,pathlib,sys,time\n"
        "args=sys.argv[5:]\n"
        "if args[:2]==['image','pull']:\n"
        " pathlib.Path('active-pull.pid').write_text(str(os.getpid()))\n"
        " time.sleep(60)\n"
        "elif args==['info','--format','{{json .ID}}']:\n"
        " print(json.dumps('proposal-test-engine'))\n"
        "else: raise RuntimeError('unexpected engine call')\n")
    docker.chmod(0o700)
    host["engine"]["docker"]["sha256"] = sha(docker.read_bytes())
    host_raw = wire(host)
    host_path = tmp_path / "host.json"
    host_path.write_bytes(host_raw)
    workspace = tmp_path / "cli-proposal"
    process = subprocess.Popen([sys.executable, "-m", "meerkat_toolkit.compose", "--workspace", str(workspace),
        "--planner", str(proposal[-1][0]), "--planner-sha256", proposal[-1][1], "propose", "--acquire-images",
        "--host-profile", str(host_path), "--host-profile-sha256", sha(host_raw)],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
        env={"PATH": "/usr/bin:/bin", "PYTHONPATH": str(ROOT / "sdk/python/src")})
    try:
        process.stdin.write(proposal[3])
        process.stdin.close()
        process.stdin = None
        deadline = time.monotonic() + 10
        while not (workspace / "active-pull.pid").exists():
            assert process.poll() is None
            assert time.monotonic() < deadline
            time.sleep(0.01)
        pid = int((workspace / "active-pull.pid").read_text())
        process.send_signal(signal.SIGTERM)
        stdout, _ = process.communicate(timeout=10)
        assert process.returncode != 0
        assert not stdout
        with pytest.raises(ProcessLookupError):
            os.kill(pid, 0)
        assert not (workspace / "proposal.json").exists()
        record = json.loads((workspace / "image-acquisition.json").read_bytes())
        assert record["status"] == "unconfirmed"
        assert record["images"][0]["status"] == "pending"
    finally:
        if process.poll() is None:
            process.kill()
            process.wait(timeout=5)
