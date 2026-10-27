"""Image acquisition uses the canonical planner and existing process owner.

All engine responses are inert fixtures; real subprocess tests run no Docker.
"""
import asyncio
import copy
import json
import os
from pathlib import Path
import signal
import subprocess
import sys

import pytest

import meerkat_toolkit.compose as module
from meerkat_toolkit.compose import CommandResult, ComposeError, ComposeRuntime, ProcessRunner
import test_compose_proposal as fixtures


ProbeRunner = fixtures.ProbeRunner
planner = fixtures.planner
proposal = fixtures.proposal
sha = fixtures.sha
wire = fixtures.wire


SCOPE = "prepare_agent_selected_local_container_plan_with_image_pull"


def acquisition_host(proposal):
    host = copy.deepcopy(proposal[0])
    host.update(scope=SCOPE, allowed_registries=["registry.example"])
    return host


def test_host_preflight_uses_runtime_scope_without_planner_or_filesystem(monkeypatch):
    def forbidden(*args, **kwargs):
        pytest.fail("host preflight attempted an effect")

    monkeypatch.setattr(subprocess, "run", forbidden)
    monkeypatch.setattr(Path, "read_bytes", forbidden)
    host = {"schema_version": 1, "scope": SCOPE, "owner": "test-owner",
            "placement": {}, "host": {}, "project": "test-project", "engine": {},
            "allowed_registries": ["docker.io"]}
    # Component/engine semantics still belong to the canonical planner.
    assert module.validate_proposal_host(wire(host), acquire_images=True) == host
    host["scope"] = "prepare_agent_selected_local_container_plan"
    del host["allowed_registries"]
    assert module.validate_proposal_host(wire(host)) == host
    with pytest.raises(ComposeError, match="image_acquisition_not_authorized"):
        module.validate_proposal_host(wire(host), acquire_images=True)
    host.update(scope=SCOPE, allowed_registries=["docker.io", "docker.io"])
    with pytest.raises(ComposeError, match="invalid_image_acquisition"):
        module.validate_proposal_host(wire(host), acquire_images=True)


class PullRunner(ProbeRunner):
    def __init__(self, planner, *, pull_failure=None, **kwargs):
        super().__init__(planner, **kwargs)
        self.pull_failure = pull_failure
        self.pulls = []

    async def run(self, argv, *, cwd, env, **kwargs):
        if argv[5:7] != ["image", "pull"]:
            return await super().run(argv, cwd=cwd, env=env, **kwargs)
        record = json.loads((cwd / "image-acquisition.json").read_bytes())
        assert record["status"] == "pending"
        assert record["host_profile_sha256"] == sha((cwd / "host-profile.json").read_bytes())
        assert record["selection_sha256"] == sha((cwd / "selection.json").read_bytes())
        item = next(item for item in record["images"] if item["image"] == argv[-1] and item["platform"] == argv[-2])
        assert item["status"] == "pending"
        assert argv[7] == "--platform" and kwargs["timeout"] == module.IMAGE_PULL_TIMEOUT
        assert not (cwd / "proposal.json").exists()
        self.pulls.append(list(argv))
        self.calls.append((list(argv), dict(env)))
        self.entered.set()
        if self.pull_failure == "cancel":
            await asyncio.Event().wait()
        if self.pull_failure == "fail":
            return CommandResult(1, b"", b"PRIVATE_REGISTRY_FAILURE")
        if self.pull_failure == "config":
            (cwd / ".docker/config.json").chmod(0o600)
            (cwd / ".docker/config.json").write_bytes(b'{"auths":{"private":"secret"}}')
        return CommandResult(0, b"opaque pull output is not an image observation")


def runtime_for(tmp_path, proposal, **kwargs):
    runner = PullRunner(proposal[-1][0], **kwargs)
    return ComposeRuntime(tmp_path / "proposal", *proposal[-1], runner=runner), runner


@pytest.mark.asyncio
async def test_pull_intent_precedes_effect_deduplicates_and_binds_final_handoff(tmp_path, proposal):
    runtime, runner = runtime_for(tmp_path, proposal)
    host_raw = wire(acquisition_host(proposal))
    result = await runtime.propose(host_raw, proposal[3], acquire_images=True)
    assert len(runner.pulls) == 1  # Three components share this image; local image is not pulled.
    record = json.loads((runtime.workspace / "image-acquisition.json").read_bytes())
    assert record["status"] == "observed"
    assert record["images"][0]["status"] == "observed"
    assert record["images"][0]["image_id"] == "sha256:" + "f" * 64
    assert record["plan_sha256"] == result["plan_sha256"]
    assert record["images"][0]["image"].endswith("@sha256:" + "a" * 64)
    assert (runtime.workspace / "host-profile.json").read_bytes() == host_raw
    assert (runtime.workspace / "selection.json").read_bytes() == proposal[3]
    assert len(result["files"]) == 7
    for name, digest in result["files"].items():
        assert sha((runtime.workspace / name).read_bytes()) == digest
    assert not (runtime.workspace / "record.json").exists()
    assert not (runtime.workspace / "compose.json").exists()
    rerun = await ProcessRunner().run([str(proposal[-1][0])], cwd=runtime.workspace,
        env=runtime._env(), stdin=(runtime.workspace / "request.json").read_bytes())
    assert json.loads(rerun.stdout)["plan_sha256"] == result["plan_sha256"]


@pytest.mark.asyncio
async def test_pull_authority_does_not_change_no_pull_default(tmp_path, proposal):
    runtime, runner = runtime_for(tmp_path, proposal)
    result = await runtime.propose(wire(acquisition_host(proposal)), proposal[3])
    assert runner.pulls == []
    assert "image-acquisition.json" not in result["files"]
    assert not (runtime.workspace / "image-acquisition.json").exists()


@pytest.mark.parametrize("change", ["old_scope", "scope_type", "missing_registries", "wrong_registry", "duplicate", "url", "bad_option"])
@pytest.mark.asyncio
async def test_scope_and_registry_refusals_precede_engine(tmp_path, proposal, change):
    runtime, runner = runtime_for(tmp_path, proposal)
    host = acquisition_host(proposal)
    option = True
    if change == "old_scope":
        host = proposal[0]
    elif change == "scope_type":
        host = {**proposal[0], "scope": []}
    elif change == "missing_registries":
        host.pop("allowed_registries")
    elif change == "wrong_registry":
        host["allowed_registries"] = ["other.example"]
    elif change == "duplicate":
        host["allowed_registries"] *= 2
    elif change == "url":
        host["allowed_registries"] = ["https://registry.example"]
    else:
        option = 1
    with pytest.raises(ComposeError):
        await runtime.propose(wire(host), proposal[3], acquire_images=option)
    assert all(argv == [str(proposal[-1][0])] for argv, _ in runner.calls)
    assert not (runtime.workspace / "proposal.json").exists()


@pytest.mark.parametrize("failure", ["fail", "config"])
@pytest.mark.asyncio
async def test_failed_pull_or_post_pull_checks_preserve_uncertainty(tmp_path, proposal, failure):
    runtime, runner = runtime_for(tmp_path, proposal, pull_failure=failure)
    with pytest.raises(ComposeError) as error:
        await runtime.propose(wire(acquisition_host(proposal)), proposal[3], acquire_images=True)
    assert "PRIVATE" not in str(error.value) and "secret" not in str(error.value)
    record = json.loads((runtime.workspace / "image-acquisition.json").read_bytes())
    assert record["status"] == "unconfirmed"
    assert record["images"][0]["status"] == ("pending" if failure == "fail" else "client_completed")
    assert not (runtime.workspace / "proposal.json").exists()
    with pytest.raises(ComposeError, match="^workspace_exists$"):
        await runtime.propose(wire(acquisition_host(proposal)), proposal[3], acquire_images=True)
    assert len(runner.pulls) == 1


@pytest.mark.asyncio
async def test_cancelled_pull_retains_pending_effect_and_releases_workspace_lock(tmp_path, proposal):
    runtime, runner = runtime_for(tmp_path, proposal, pull_failure="cancel")
    task = asyncio.create_task(runtime.propose(wire(acquisition_host(proposal)), proposal[3], acquire_images=True))
    async with asyncio.timeout(10):
        await runner.entered.wait()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    record = json.loads((runtime.workspace / "image-acquisition.json").read_bytes())
    assert record["status"] == "unconfirmed" and record["images"][0]["status"] == "pending"
    with runtime._lock():
        assert not (runtime.workspace / "proposal.json").exists()


def slow_pull_host(proposal):
    host = acquisition_host(proposal)
    path = Path(host["engine"]["docker"]["path"])
    path.write_text("#!" + sys.executable + "\n" +
        "import json, os, pathlib, sys, time\n"
        "args = sys.argv[5:]\n"
        "if args[0] == 'info': print(json.dumps('proposal-test-engine'))\n"
        "elif args[:2] == ['image', 'pull']:\n"
        " pathlib.Path('pull.pid').write_text(str(os.getpid()))\n"
        " time.sleep(60)\n"
        "else: raise RuntimeError('unexpected call')\n")
    path.chmod(0o700)
    host["engine"]["docker"]["sha256"] = sha(path.read_bytes())
    return host


@pytest.mark.asyncio
async def test_real_pull_timeout_joins_child_and_leaves_no_success(tmp_path, proposal, monkeypatch):
    monkeypatch.setattr(module, "IMAGE_PULL_TIMEOUT", 0.1)
    runtime = ComposeRuntime(tmp_path / "proposal", *proposal[-1])
    with pytest.raises(TimeoutError):
        await runtime.propose(wire(slow_pull_host(proposal)), proposal[3], acquire_images=True)
    pid = int((runtime.workspace / "pull.pid").read_text())
    with pytest.raises(ProcessLookupError):
        os.kill(pid, 0)
    assert json.loads((runtime.workspace / "image-acquisition.json").read_bytes())["status"] == "unconfirmed"
    assert not (runtime.workspace / "proposal.json").exists()


@pytest.mark.asyncio
async def test_actual_cli_term_joins_pull_process_and_retains_pending(tmp_path, proposal):
    host = tmp_path / "host.json"
    host.write_bytes(wire(slow_pull_host(proposal)))
    workspace = tmp_path / "cli"
    child = subprocess.Popen([sys.executable, "-m", "meerkat_toolkit.compose", "--workspace", str(workspace),
        "--planner", str(proposal[-1][0]), "--planner-sha256", proposal[-1][1], "propose",
        "--host-profile", str(host), "--host-profile-sha256", sha(host.read_bytes()), "--acquire-images"],
        stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    child.stdin.write(proposal[3])
    child.stdin.close()
    child.stdin = None
    try:
        async with asyncio.timeout(10):
            while not (workspace / "pull.pid").exists():
                assert child.poll() is None
                await asyncio.sleep(0.01)
        pid = int((workspace / "pull.pid").read_text())
        child.send_signal(signal.SIGTERM)
        stdout, stderr = await asyncio.to_thread(child.communicate, timeout=10)
        assert child.returncode == 1 and stdout == b"" and stderr == b"compose_interrupted\n"
        with pytest.raises(ProcessLookupError):
            os.kill(pid, 0)
        record = json.loads((workspace / "image-acquisition.json").read_bytes())
        assert record["status"] == "unconfirmed" and record["images"][0]["status"] == "pending"
        assert not (workspace / "proposal.json").exists()
    finally:
        if child.poll() is None:
            child.kill()
            child.wait()
