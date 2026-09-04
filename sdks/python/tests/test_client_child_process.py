"""Tests for how ``MeerkatClient`` handles the rkat-rpc child process itself.

The transport is a child process with three pipes, and the child can exit at
any moment, most often right at startup. ``close()`` usually runs from the
caller's ``finally`` after the ``MeerkatError`` that explained the exit, so a
child that is already gone must be treated as already closed instead of
raising ``ProcessLookupError`` over that error (meerkat issue #1103).

The fake children here are tiny ``sh`` scripts written into ``tmp_path``.
"""

from __future__ import annotations

import asyncio
import stat
from pathlib import Path

import pytest

from meerkat import client as client_module
from meerkat.client import MeerkatClient
from meerkat.errors import MeerkatError


def _write_fake_rpc(tmp_path: Path, body: str) -> str:
    script = tmp_path / "fake-rkat-rpc"
    script.write_text("#!/bin/sh\n" + body)
    script.chmod(script.stat().st_mode | stat.S_IXUSR)
    return str(script)


class _FakeStdin:
    def __init__(self) -> None:
        self.closed = False

    def close(self) -> None:
        self.closed = True


class _FakeProcess:
    """Stand-in for ``asyncio.subprocess.Process`` with scripted signals."""

    def __init__(
        self,
        *,
        returncode: int | None,
        terminate_raises: bool = False,
        kill_raises: bool = False,
        exits_on_terminate: bool = True,
    ) -> None:
        self.returncode = returncode
        self.stdin = _FakeStdin()
        self.terminate_calls = 0
        self.kill_calls = 0
        self._terminate_raises = terminate_raises
        self._kill_raises = kill_raises
        self._exits_on_terminate = exits_on_terminate
        self._exited = asyncio.Event()
        if returncode is not None:
            self._exited.set()

    def terminate(self) -> None:
        self.terminate_calls += 1
        if self._terminate_raises:
            # The pid is gone; the reaper reports the exit a moment later.
            self.returncode = 1
            self._exited.set()
            raise ProcessLookupError()
        if self._exits_on_terminate:
            self.returncode = -15
            self._exited.set()

    def kill(self) -> None:
        self.kill_calls += 1
        if self._kill_raises:
            raise ProcessLookupError()
        self.returncode = -9
        self._exited.set()

    async def wait(self) -> int:
        await self._exited.wait()
        assert self.returncode is not None
        return self.returncode


def _client_with(process: _FakeProcess) -> MeerkatClient:
    client = MeerkatClient()
    client._process = process  # type: ignore[assignment]
    return client


@pytest.mark.asyncio
async def test_connect_failure_survives_close_in_finally(tmp_path: Path) -> None:
    """The shape every host writes: connect in try, close in finally. The
    child exits at startup; the caller must see CONNECTION_CLOSED, not the
    ProcessLookupError that close() used to raise over it."""
    fake = _write_fake_rpc(tmp_path, "echo 'refusing to start' >&2\nexit 1\n")

    async def run() -> None:
        client = MeerkatClient(fake)
        try:
            await client.connect()
        finally:
            await client.close()

    with pytest.raises(MeerkatError) as excinfo:
        await run()

    assert excinfo.value.code == "CONNECTION_CLOSED"


@pytest.mark.asyncio
async def test_close_after_child_exit_leaves_client_disconnected(tmp_path: Path) -> None:
    fake = _write_fake_rpc(tmp_path, "exit 0\n")
    client = MeerkatClient(fake)
    with pytest.raises(MeerkatError):
        await client.connect()

    await client.close()
    await client.close()

    assert client._process is None
    assert client._dispatcher is None
    with pytest.raises(MeerkatError) as excinfo:
        await client.list_realms()
    assert excinfo.value.code == "NOT_CONNECTED"


@pytest.mark.asyncio
async def test_close_skips_signals_for_a_reaped_child() -> None:
    process = _FakeProcess(returncode=1)
    client = _client_with(process)

    await client.close()

    assert process.terminate_calls == 0
    assert process.kill_calls == 0
    assert process.stdin.closed
    assert client._process is None


@pytest.mark.asyncio
async def test_close_treats_process_lookup_error_from_terminate_as_closed() -> None:
    """returncode is still None (asyncio has not reaped yet) but the pid is
    gone: terminate() raises ProcessLookupError, which is "already closed"."""
    process = _FakeProcess(returncode=None, terminate_raises=True)
    client = _client_with(process)

    await client.close()

    assert process.terminate_calls == 1
    assert process.kill_calls == 0
    assert client._process is None


@pytest.mark.asyncio
async def test_close_terminates_a_live_child() -> None:
    process = _FakeProcess(returncode=None)
    client = _client_with(process)

    await client.close()

    assert process.terminate_calls == 1
    assert process.kill_calls == 0
    assert process.returncode == -15


@pytest.mark.asyncio
async def test_close_kills_after_terminate_grace_and_tolerates_vanished_child(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(client_module, "CHILD_EXIT_TIMEOUT_SECS", 0.05)
    process = _FakeProcess(returncode=None, exits_on_terminate=False, kill_raises=True)
    client = _client_with(process)

    await asyncio.wait_for(client.close(), timeout=5)

    assert process.terminate_calls == 1
    assert process.kill_calls == 1
    assert client._process is None


@pytest.mark.asyncio
async def test_close_kills_and_reaps_after_terminate_grace(
    monkeypatch: pytest.MonkeyPatch,
) -> None:
    monkeypatch.setattr(client_module, "CHILD_EXIT_TIMEOUT_SECS", 0.05)
    process = _FakeProcess(returncode=None, exits_on_terminate=False)
    client = _client_with(process)

    await asyncio.wait_for(client.close(), timeout=5)

    assert process.terminate_calls == 1
    assert process.kill_calls == 1
    assert process.returncode == -9
