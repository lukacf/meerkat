"""Tests for how ``MeerkatClient`` handles the rkat-rpc child process itself.

The transport is a child process with three pipes, and the child can exit at
any moment, most often right at startup. Two failure modes lived in the gaps
between the pipes (meerkat issue #1103):

* ``close()`` usually runs from the caller's ``finally`` after the
  ``MeerkatError`` that explained the exit, so a child that is already gone
  must be treated as already closed instead of raising ``ProcessLookupError``
  over that error;
* stderr was never read, so a child that refused to start left the caller
  with a bare ``CONNECTION_CLOSED`` and no reason, and a chatty child could
  block on the full pipe. The drain keeps a bounded tail and the
  ``CONNECTION_CLOSED`` error carries it.

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
from meerkat.streaming import STDERR_TAIL_LIMIT_BYTES, _StderrTail

REFUSAL_LINE = (
    "Error: session-store has objects outside the meerkat_schema ledger; "
    "run `rkat storage migrate --apply --bridge-pre-0-8-10`"
)


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


# -- stderr drain and CONNECTION_CLOSED tail -----------------------------


@pytest.mark.asyncio
async def test_stderr_tail_is_bounded_and_keeps_the_latest_bytes() -> None:
    reader = asyncio.StreamReader()
    tail = _StderrTail(reader, limit=32)
    tail.start()

    reader.feed_data(b"a" * 40)
    reader.feed_data(b"b" * 8)
    reader.feed_data(b"tail-end")
    reader.feed_eof()
    await tail.wait_closed(timeout=5)

    text = tail.text()
    assert len(text.encode()) == 32
    assert text.endswith("bbbbbbbbtail-end")
    assert "a" * 17 not in text
    await tail.stop()


@pytest.mark.asyncio
async def test_stderr_tail_stop_cancels_a_drain_that_never_saw_eof() -> None:
    reader = asyncio.StreamReader()
    tail = _StderrTail(reader)
    tail.start()
    reader.feed_data(b"partial")
    await asyncio.sleep(0)

    await tail.stop()

    assert tail.text() == "partial"
    assert tail._task is None


@pytest.mark.asyncio
async def test_connect_reports_child_stderr_tail_when_child_exits(tmp_path: Path) -> None:
    fake = _write_fake_rpc(
        tmp_path,
        f"echo 'rkat-rpc: starting' >&2\necho '{REFUSAL_LINE}' >&2\nexit 1\n",
    )
    client = MeerkatClient(fake)

    with pytest.raises(MeerkatError) as excinfo:
        await client.connect()
    await client.close()

    err = excinfo.value
    assert err.code == "CONNECTION_CLOSED"
    assert err.message.startswith("rkat-rpc process closed")
    assert REFUSAL_LINE in err.message
    assert isinstance(err.details, dict)
    assert err.details["stderr_tail"].splitlines() == [
        "rkat-rpc: starting",
        REFUSAL_LINE,
    ]


@pytest.mark.asyncio
async def test_later_requests_after_child_exit_carry_the_same_stderr_tail(
    tmp_path: Path,
) -> None:
    fake = _write_fake_rpc(tmp_path, f"echo '{REFUSAL_LINE}' >&2\nexit 1\n")
    client = MeerkatClient(fake)

    with pytest.raises(MeerkatError):
        await client.connect()
    with pytest.raises(MeerkatError) as excinfo:
        await client.list_realms()
    await client.close()

    assert excinfo.value.code == "CONNECTION_CLOSED"
    assert excinfo.value.details == {"stderr_tail": REFUSAL_LINE}


@pytest.mark.asyncio
async def test_chatty_child_stderr_is_drained_and_tail_is_bounded(tmp_path: Path) -> None:
    # Roughly 400 KB of stderr, several times the OS pipe buffer. An undrained
    # pipe would block the child on the write forever and the connect below
    # would hang instead of failing.
    line_count = 4000
    fake = _write_fake_rpc(
        tmp_path,
        "i=0\n"
        f"while [ $i -lt {line_count} ]; do\n"
        "  echo \"noise line $i ...................................................................................\" >&2\n"
        "  i=$((i+1))\n"
        "done\n"
        "echo 'final reason' >&2\n"
        "exit 1\n",
    )
    client = MeerkatClient(fake)

    with pytest.raises(MeerkatError) as excinfo:
        await asyncio.wait_for(client.connect(), timeout=60)
    await client.close()

    err = excinfo.value
    assert err.code == "CONNECTION_CLOSED"
    tail = err.details["stderr_tail"]
    assert len(tail.encode()) <= STDERR_TAIL_LIMIT_BYTES
    assert tail.endswith("final reason")
    assert f"noise line {line_count - 1} " in tail
    assert "noise line 0 " not in tail


@pytest.mark.asyncio
async def test_connection_closed_without_stderr_output_keeps_plain_message(
    tmp_path: Path,
) -> None:
    fake = _write_fake_rpc(tmp_path, "exit 0\n")
    client = MeerkatClient(fake)

    with pytest.raises(MeerkatError) as excinfo:
        await client.connect()
    await client.close()

    assert excinfo.value.code == "CONNECTION_CLOSED"
    assert excinfo.value.message == "rkat-rpc process closed"
    assert excinfo.value.details is None


@pytest.mark.asyncio
async def test_close_releases_the_stderr_drain_task(tmp_path: Path) -> None:
    fake = _write_fake_rpc(tmp_path, "echo boom >&2\nexit 1\n")
    client = MeerkatClient(fake)

    with pytest.raises(MeerkatError):
        await client.connect()
    drain = client._stderr_tail
    assert drain is not None and drain._task is not None
    await client.close()

    assert client._stderr_tail is None
    assert drain._task is None
