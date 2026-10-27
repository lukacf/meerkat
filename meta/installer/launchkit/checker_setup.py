"""Wizard-owned preparation and isolated readiness of the pinned checker."""
from __future__ import annotations

import asyncio
import os
from pathlib import Path
import shutil
import stat
import subprocess
import sys
import tempfile

from . import checker_runtime
from .implementation_workspace import _sandbox_executable

_DETAILS = {
    "ready": "The reviewed JavaScript checker is ready. This does not mean an application has passed its checks.",
    "needs_preparation": "Prepare the reviewed Node.js 25.9.0 checker before building an application with JavaScript or DOM tests.",
    "preparing": "Preparing the reviewed JavaScript checker. Keep Meta Forge running while preparation finishes.",
    "unsupported": "Checker preparation requires supported macOS sandbox-exec or functional Linux bubblewrap isolation. Checks on this host remain unavailable.",
    "runtime": "The verified Node checker could not start inside isolation. Check this computer's runtime libraries before building.",
    "cache": "The existing checker cache could not be verified. Preserve it and prepare a fresh installer copy.",
    "failed": "Checker preparation did not finish. Recheck its status before choosing Prepare again.",
    "closed": "Checker setup is shutting down. No new preparation can start.",
}


def _public(status, *, can_prepare=False, detail=None):
    return {"status": status, "can_prepare": can_prepare,
            "detail": _DETAILS[detail or status]}


def _sandbox_ready(sandbox):
    from .implementation_workspace import _check_environment, _run_sandboxed, _sandbox_profile
    try:
        with tempfile.TemporaryDirectory(prefix="mobkit-checker-probe-") as directory:
            staging = Path(directory).resolve()
            snapshot, scratch = staging / "snapshot", staging / "scratch"
            snapshot.mkdir(mode=0o700)
            scratch.mkdir(mode=0o700)
            result = _run_sandboxed(
                [str(Path(sys.executable).resolve()), "-I", "-S", "-c", "print('MOBKIT_CHECKER_SANDBOX_READY')"],
                sandbox, _sandbox_profile(snapshot, scratch, staging), scratch,
                _check_environment(scratch), timeout=2)
            return result["status"] == "pass" and result["detail"] == "MOBKIT_CHECKER_SANDBOX_READY"
    except Exception:
        return False


def _probe(root):
    try:
        checker_runtime.platform_target()
    except checker_runtime.CheckerRuntimeError:
        return _public("unavailable", detail="unsupported")
    sandbox = _sandbox_executable()
    if not sandbox or not _sandbox_ready(sandbox):
        return _public("unavailable", detail="unsupported")
    try:
        prepared = checker_runtime.prepared_node(root)
        if prepared is not None:
            return _public("ready") if _node_ready(prepared, sandbox) else _public("unavailable", detail="runtime")
    except (checker_runtime.CheckerRuntimeError, OSError):
        # A damaged private cache never falls through to another executable.
        return _public("unavailable", detail="cache")
    node = shutil.which("node")
    if node:
        try:
            binary = Path(node).resolve(strict=True)
            info = binary.stat()
            if (stat.S_ISREG(info.st_mode) and info.st_uid in {0, os.getuid()}
                    and not info.st_mode & 0o022 and info.st_mode & 0o111):
                # Probe only the trusted executable's version. In particular,
                # NODE_OPTIONS, provider keys and authored project code are absent.
                if _node_ready(str(binary), sandbox):
                    return _public("ready")
        except (OSError, subprocess.SubprocessError, UnicodeError):
            pass
    return _public("needs_preparation", can_prepare=True)


def _node_ready(node, sandbox):
    from .implementation_workspace import _check_environment, _run_sandboxed, _sandbox_profile
    try:
        binary = Path(node).resolve(strict=True)
        with tempfile.TemporaryDirectory(prefix="mobkit-checker-runtime-probe-") as directory:
            staging = Path(directory).resolve()
            snapshot, scratch = staging / "snapshot", staging / "scratch"
            snapshot.mkdir(mode=0o700)
            scratch.mkdir(mode=0o700)
            result = _run_sandboxed([str(binary), "--version"], sandbox,
                _sandbox_profile(snapshot, scratch, staging, protected_reads=[binary]),
                scratch, _check_environment(scratch), timeout=2)
            return result["status"] == "pass" and result["detail"] == "v" + checker_runtime.VERSION
    except Exception:
        return False


class CheckerSetupService:
    def __init__(self, root=None):
        self.root = Path(root) if root is not None else Path(__file__).resolve().parents[1]
        self._lock = asyncio.Lock()
        self._task = None
        self._failed = False
        self._closed = False

    async def _status(self):
        result = await asyncio.to_thread(_probe, self.root)
        if result["status"] == "needs_preparation" and self._failed:
            return _public("failed", can_prepare=True)
        return result

    async def status(self):
        async with self._lock:
            if self._closed:
                return _public("unavailable", detail="closed")
            if self._task is not None and not self._task.done():
                return _public("preparing")
            return await self._status()

    async def prepare(self):
        async with self._lock:
            if self._closed:
                return _public("unavailable", detail="closed")
            if self._task is not None and not self._task.done():
                return _public("preparing")
            current = await self._status()
            if not current["can_prepare"]:
                return current
            self._failed = False
            # Detached ownership survives a lost HTTP response. Repeated
            # requests reuse this task until its thread and readback finish.
            self._task = asyncio.create_task(self._prepare(), name="launch-checker-preparation")
            return _public("preparing")

    async def _prepare(self):
        try:
            await asyncio.to_thread(checker_runtime.prepare, self.root)
            current = await asyncio.to_thread(_probe, self.root)
            self._failed = current["status"] != "ready"
        except Exception:
            # Downloader and filesystem errors never project arbitrary paths
            # or network text through the wizard.
            self._failed = True

    async def shutdown(self):
        async with self._lock:
            self._closed = True
            task = self._task
        if task is not None:
            cancellation = None
            while not task.done():
                try:
                    await asyncio.shield(task)
                except asyncio.CancelledError as error:
                    # Cancelling to_thread loses download ownership without
                    # stopping its thread. Drain the same owner first.
                    cancellation = error
            if cancellation is not None:
                raise cancellation
