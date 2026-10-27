"""Checker readiness and task ownership with no real downloads or execution."""
import asyncio
import os
from pathlib import Path
import subprocess
import sys
import threading
from types import SimpleNamespace

import pytest

from launchkit import checker_setup as module
from launchkit.checker_setup import CheckerSetupService


@pytest.fixture
def host(tmp_path, monkeypatch):
    state = SimpleNamespace(cache=None, corrupt=False, node=None, calls=0, probes=[],
                            entered=threading.Event(), release=threading.Event(), failure=False)
    state.release.set()
    monkeypatch.setattr(module.checker_runtime, "platform_target", lambda: "arm64")
    monkeypatch.setattr(module, "_sandbox_executable", lambda: "/usr/bin/test-os-isolator")
    monkeypatch.setattr(module.shutil, "which", lambda name: "/usr/bin/sandbox-exec" if name == "sandbox-exec" else state.node)
    monkeypatch.setattr(module, "_sandbox_ready", lambda sandbox: True)
    def cached(root):
        assert root == tmp_path
        if state.corrupt:
            raise module.checker_runtime.CheckerRuntimeError("fictional-private-cache-path")
        return state.cache
    def prepare(root):
        assert root == tmp_path
        state.calls += 1
        state.entered.set()
        assert state.release.wait(3)
        if state.failure:
            raise OSError("fictional-provider-key-network-text")
        state.cache = "fictional-checked-cache-node"
        return state.cache
    monkeypatch.setattr(module.checker_runtime, "prepared_node", cached)
    monkeypatch.setattr(module.checker_runtime, "prepare", prepare)
    def probe(command, **kwargs):
        state.probes.append((command, kwargs))
        return SimpleNamespace(returncode=0, stdout="v25.9.0\n")
    monkeypatch.setattr(module.subprocess, "run", probe)
    def node_ready(node, sandbox):
        result = module.subprocess.run([node, "--version"], cwd="/", env={"PATH": os.defpath},
                                       capture_output=True, text=True, timeout=2, check=False)
        return result.returncode == 0 and result.stdout.strip() == "v" + module.checker_runtime.VERSION
    monkeypatch.setattr(module, "_node_ready", node_ready)
    return CheckerSetupService(tmp_path), state


@pytest.mark.asyncio
async def test_status_never_prepares_and_verified_cache_still_needs_executable_readiness(host):
    service, state = host
    assert (await service.status())["status"] == "needs_preparation"
    assert state.calls == 0 and not state.probes
    state.cache = "fictional-verified-node"
    public = await service.status()
    assert public["status"] == "ready" and public["can_prepare"] is False
    assert state.calls == 0 and len(state.probes) == 1 and "fictional" not in str(public)


@pytest.mark.asyncio
async def test_verified_cache_with_missing_runtime_libraries_is_not_ready(host, monkeypatch):
    service, state = host
    state.cache = "fictional-verified-node"
    monkeypatch.setattr(module, "_node_ready", lambda *_: False)
    public = await service.prepare()
    assert public["status"] == "unavailable" and public["can_prepare"] is False
    assert state.calls == 0


@pytest.mark.asyncio
async def test_exact_path_version_uses_scrubbed_environment_without_authored_code(host, tmp_path, monkeypatch):
    service, state = host
    binary = tmp_path / "path-node"
    binary.write_bytes(b"fictional executable never run")
    binary.chmod(0o700)
    state.node = str(binary)
    monkeypatch.setenv("OPENAI_API_KEY", "fictional-secret")
    monkeypatch.setenv("NODE_OPTIONS", "--require fictional-authored-script")
    assert (await service.status())["status"] == "ready"
    assert state.calls == 0
    command, options = state.probes[0]
    assert command == [str(binary), "--version"]
    assert set(options["env"]) == {"PATH"} and options["cwd"] == "/"
    assert options["timeout"] == 2
    assert "fictional" not in str(options["env"])


@pytest.mark.asyncio
@pytest.mark.parametrize("reason", ["different_version", "timeout", "error"])
async def test_invalid_path_probe_can_prepare_without_raw_error(host, tmp_path, monkeypatch, reason):
    service, state = host
    binary = tmp_path / "path-node"
    binary.write_bytes(b"not executed")
    binary.chmod(0o700)
    state.node = str(binary)
    def probe(*args, **kwargs):
        if reason == "timeout":
            raise subprocess.TimeoutExpired("fictional-private-path", 2)
        return SimpleNamespace(returncode=1 if reason == "error" else 0, stdout="v20.0.0 fictional-private-text")
    monkeypatch.setattr(module.subprocess, "run", probe)
    public = await service.status()
    assert public["status"] == "needs_preparation" and public["can_prepare"] is True
    assert "fictional" not in str(public) and state.calls == 0


@pytest.mark.asyncio
@pytest.mark.parametrize("unsupported", ["platform", "sandbox_missing"])
async def test_unsupported_host_refuses_preparation(host, monkeypatch, unsupported):
    service, state = host
    if unsupported == "platform":
        def platform():
            raise module.checker_runtime.CheckerRuntimeError("fictional-platform-detail")
        monkeypatch.setattr(module.checker_runtime, "platform_target", platform)
    else:
        monkeypatch.setattr(module, "_sandbox_executable", lambda: None)
    public = await service.prepare()
    assert public["status"] == "unavailable" and public["can_prepare"] is False
    assert state.calls == 0 and not state.probes and "fictional" not in str(public)


@pytest.mark.asyncio
async def test_corrupt_cache_refuses_fallback_and_overwrite(host):
    service, state = host
    state.corrupt = True
    state.node = "fictional-path-node"
    public = await service.prepare()
    assert public["status"] == "unavailable" and public["can_prepare"] is False
    assert state.calls == 0 and not state.probes and "fictional" not in str(public)


@pytest.mark.asyncio
async def test_present_but_nonfunctional_sandbox_refuses_preparation(host, monkeypatch):
    service, state = host
    state.cache = "fictional-otherwise-ready-node"
    monkeypatch.setattr(module, "_sandbox_ready", lambda sandbox: False)
    public = await service.prepare()
    assert public["status"] == "unavailable" and public["can_prepare"] is False
    assert state.calls == 0 and not state.probes and "fictional" not in str(public)


def test_existing_sandbox_runs_only_trusted_print_in_clean_isolated_scratch(monkeypatch):
    import shutil
    from launchkit import implementation_workspace as workspace
    sandbox = workspace._sandbox_executable()
    if not sandbox:
        pytest.skip("Functional sandbox proof requires supported OS isolation")
    monkeypatch.setenv("OPENAI_API_KEY", "fictional-secret-not-forwarded")
    monkeypatch.setenv("NODE_OPTIONS", "--require fictional-authored-script")
    observed = []
    original = workspace._run_sandboxed
    def run(command, executable, profile, cwd, environment, **kwargs):
        observed.append((command, executable, profile, cwd, environment, kwargs))
        return original(command, executable, profile, cwd, environment, **kwargs)
    monkeypatch.setattr(workspace, "_run_sandboxed", run)
    assert module._sandbox_ready(sandbox) is True
    command, executable, profile, scratch, environment, options = observed[0]
    assert command == [str(Path(sys.executable).resolve()), "-I", "-S", "-c", "print('MOBKIT_CHECKER_SANDBOX_READY')"]
    assert executable == sandbox
    if sys.platform == "linux":
        assert profile.startswith("mobkit-linux-bwrap:")
    else:
        assert "(deny network*)" in profile
    assert environment == workspace._check_environment(scratch)
    assert "OPENAI_API_KEY" not in environment and "NODE_OPTIONS" not in environment
    assert options == {"timeout": 2} and not scratch.exists()


@pytest.mark.asyncio
async def test_concurrent_prepares_and_lost_readback_keep_one_owner(host):
    service, state = host
    state.release.clear()
    responses = await asyncio.gather(service.prepare(), service.prepare(), service.prepare())
    assert all(response["status"] == "preparing" for response in responses)
    task = service._task
    assert await asyncio.to_thread(state.entered.wait, 30)
    assert state.calls == 1 and (await service.status())["status"] == "preparing"
    assert (await service.prepare())["status"] == "preparing" and service._task is task
    # No response consumer needs to retain the task: the service owns it.
    state.release.set()
    await service.shutdown()
    assert task.done() and state.calls == 1 and state.cache is not None


@pytest.mark.asyncio
async def test_completion_requires_checked_readback_and_reuses_ready_cache(host):
    service, state = host
    assert (await service.prepare())["status"] == "preparing"
    await service._task
    assert (await service.status())["status"] == "ready"
    assert (await service.prepare())["status"] == "ready" and state.calls == 1


@pytest.mark.asyncio
async def test_prepare_return_without_verified_cache_does_not_claim_ready(host, monkeypatch):
    service, state = host
    def incomplete(root):
        state.calls += 1
        return "fictional-unverified-binary"
    monkeypatch.setattr(module.checker_runtime, "prepare", incomplete)
    await service.prepare()
    await service._task
    public = await service.status()
    assert public["status"] == "failed" and public["can_prepare"] is True
    assert state.cache is None and state.calls == 1 and "fictional" not in str(public)


@pytest.mark.asyncio
async def test_failure_has_safe_status_and_retry_is_another_explicit_action(host):
    service, state = host
    state.failure = True
    await service.prepare()
    await service._task
    public = await service.status()
    assert public["status"] == "failed" and public["can_prepare"] is True
    assert "fictional" not in str(public) and state.calls == 1
    await service.status()
    assert state.calls == 1
    state.failure = False
    await service.prepare()
    await service._task
    assert (await service.status())["status"] == "ready" and state.calls == 2


@pytest.mark.asyncio
async def test_cancelled_shutdown_drains_thread_and_refuses_new_preparation(host):
    service, state = host
    state.release.clear()
    await service.prepare()
    assert await asyncio.to_thread(state.entered.wait, 30)
    task = service._task
    shutdown = asyncio.create_task(service.shutdown())
    await asyncio.sleep(0)
    shutdown.cancel()
    await asyncio.sleep(0)
    assert not shutdown.done() and not task.done()
    assert (await service.prepare())["status"] == "unavailable" and state.calls == 1
    state.release.set()
    with pytest.raises(asyncio.CancelledError):
        await shutdown
    assert task.done() and state.cache is not None and state.calls == 1
