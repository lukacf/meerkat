"""A process that drops an adopted gateway's runtime still exits.

The program below builds a runtime over a gateway child it plays itself (a
thread over two pipes), lets the child exit on its own, and drops the
runtime without ``shutdown()``. The runtime and its transport hold each other
(through the callback handler), so nothing frees them before the collection
at interpreter exit, which then runs the transport's finalizer. That
finalizer must not start a thread: on Python 3.10 and 3.11 a thread started
while the interpreter shuts down never reports that it started, and the
process hangs. The test runs the program on the interpreter that runs the
tests; run it on 3.10 or 3.11 to cover that case.

The program is started as a child Python process, so this module has no
trap on ``subprocess.Popen``.
"""
from __future__ import annotations

import os
from pathlib import Path
import subprocess
import sys
import textwrap
import time

import pytest

import meerkat_meta

_PROGRAM = """
import asyncio, gc, json, os, sys, threading, time

record = os.write
marker = os.open(sys.argv[1], os.O_WRONLY | os.O_CREAT | os.O_APPEND, 0o600)

from meerkat_meta import AdoptedGatewayChild, AdoptedGatewayTransport, Meta

def pipe():
    return os.pipe2(os.O_CLOEXEC) if hasattr(os, "pipe2") else os.pipe()

stdin_read, stdin_write = pipe()
stdout_read, stdout_write = pipe()
child_stdin = os.fdopen(stdin_read, "rb")
child_stdout = os.fdopen(stdout_write, "wb")
exited = threading.Event()

def child():
    # Answers init as an older gateway does, then exits on its own.
    init = json.loads(child_stdin.readline())
    child_stdout.write((json.dumps({"jsonrpc": "2.0", "id": init["id"],
        "result": {"http_base_url": "http://127.0.0.1:9"}}) + "\\n").encode())
    child_stdout.flush()
    child_stdout.close()
    exited.set()

threading.Thread(target=child, daemon=True).start()

def request_retirement():
    record(marker, b"retirement requested\\n")

def wait_process(timeout):
    if exited.wait(timeout):
        return 0
    raise TimeoutError

async def main():
    transport = AdoptedGatewayTransport(AdoptedGatewayChild(
        input=os.fdopen(stdin_write, "wb", buffering=0),
        output=os.fdopen(stdout_read, "rb", buffering=0),
        request_retirement=request_retirement,
        wait_process=wait_process,
    ))
    runtime = await Meta.builder().gateway_transport(transport).build()
    deadline = time.monotonic() + 10
    while any(thread.name in ("meta-reader", "meta-exit-watch") and thread.is_alive()
              for thread in threading.enumerate()):
        if time.monotonic() > deadline:
            raise SystemExit("the SDK's threads never saw the child exit")
        await asyncio.sleep(0.01)
    # Dropped without shutdown(): only the collection at exit frees them.
    gc.disable()
    del runtime, transport

asyncio.run(main())
print("exiting", flush=True)
"""


@pytest.mark.skipif(os.name != "posix", reason="the program uses POSIX pipe ends")
def test_a_runtime_dropped_without_shutdown_lets_the_process_exit(tmp_path):
    program = tmp_path / "dropped_runtime.py"
    program.write_text(textwrap.dedent(_PROGRAM))
    marker = tmp_path / "marker"
    sdk = Path(meerkat_meta.__file__).resolve().parents[1]
    env = {**os.environ, "PYTHONPATH": str(sdk)}
    env.pop("META_GATEWAY_BIN", None)
    started = time.monotonic()
    result = subprocess.run(
        [sys.executable, str(program), str(marker)],
        env=env,
        capture_output=True,
        text=True,
        timeout=60,
    )
    assert result.returncode == 0, result.stderr[-2000:]
    assert "exiting" in result.stdout
    assert time.monotonic() - started < 30
    # The finalizer handed the child back, at exit, without a thread.
    assert marker.read_text() == "retirement requested\n"
