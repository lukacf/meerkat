"""Inert inherited pipes only; no native review, authentication or settlement."""
from __future__ import annotations

import dataclasses
import json
import os
import select
import subprocess
import sys
import threading
import time

import pytest

from meerkat_toolkit.host_control import HostControlError, PrivateHostClient, RemoteControllerLogin
from test_host_submit import submission


# This child deliberately does not read the command pipe until the test releases
# it on stdin. It then records complete frames and any uncertain trailing bytes.
CHILD = r'''
import json, os, sys
command_fd, response_fd = map(int, sys.argv[1:])
print("paused", flush=True)
assert sys.stdin.readline() == "drain\n"
data = bytearray()
while True:
    part = os.read(command_fd, 65536)
    if not part: break
    data.extend(part)
lines = data.split(b"\n")
for line in lines[:-1]:
    value = json.loads(line)
    command = value["command"]
    print(json.dumps({"id": value["id"], "kind": command["kind"],
        "original": command.get("submission", {}).get("original", {}).get("original")}), flush=True)
print(json.dumps({"trailing_bytes": len(lines[-1])}), flush=True)
os.close(command_fd)
os.close(response_fd)
'''


def await_until(predicate, timeout=1.0):
    deadline = time.monotonic() + timeout
    while not predicate():
        assert time.monotonic() < deadline, "inert pipe did not reach the required state"
        time.sleep(0.005)


class PausedPipe:
    def __init__(self, timeout):
        read_command, write_command = os.pipe()
        read_response, write_response = os.pipe()
        self.child = subprocess.Popen(
            [sys.executable, "-u", "-c", CHILD, str(read_command), str(write_response)],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE,
            pass_fds=(read_command, write_response), env={"PATH": "/usr/bin:/bin"},
        )
        os.close(read_command)
        os.close(write_response)
        self.client = PrivateHostClient(write_command, read_response, timeout=timeout)
        self.command_fd = write_command
        self.threads = []
        self.handles = []
        self.errors = []
        self.rows = None
        assert select.select([self.child.stdout], [], [], 2)[0]
        assert self.child.stdout.readline() == b"paused\n"

    def run(self, operation):
        outcome = []
        def owned():
            try:
                outcome.append(operation())
            except BaseException as error:
                outcome.append(error)
        thread = threading.Thread(target=owned, daemon=True)
        self.threads.append(thread)
        thread.start()
        return thread, outcome

    def saturate(self):
        original, _, authentication = submission()
        def write_reviews():
            for index in range(16):
                try:
                    review = self.client.begin_remote_controller_review(
                        dataclasses.replace(original, original=f"request-{index}"), "x" * 60_000,
                        authentication, RemoteControllerLogin("paired-login", 1900000000000), timeout=3600,
                    )
                    self.handles.append(review)
                except BaseException as error:
                    self.errors.append(error)
                    return
            raise AssertionError("inert pipe did not exert write backpressure")
        worker, outcome = self.run(write_reviews)
        await_until(lambda: self.client._write_lock.locked() and not select.select([], [self.command_fd], [], 0)[1])
        assert worker.is_alive()
        return worker, outcome

    def finish(self):
        closer, closed = self.run(self.client.close)
        closer.join(timeout=1)
        close_joined = not closer.is_alive()
        # Release the inert child even on a failing assertion, then join all our
        # threads. Cleanup success must not mask an unbounded SDK close.
        self.child.stdin.write(b"drain\n")
        self.child.stdin.flush()
        forced = False
        try:
            stdout, stderr = self.child.communicate(timeout=2)
        except subprocess.TimeoutExpired:
            forced = True
            self.child.kill()
            stdout, stderr = self.child.communicate(timeout=2)
        for thread in self.threads:
            thread.join(timeout=1)
        self.rows = [json.loads(line) for line in stdout.splitlines()]
        assert close_joined, "SDK close exceeded its bounded local join"
        assert closed == [None]
        assert not forced, "forced child cleanup is not successful lifecycle qualification"
        assert self.child.returncode == 0 and stderr == b""
        assert all(not thread.is_alive() for thread in self.threads)
        assert not self.client._reader.is_alive()
        assert self.client._command_fd is None
        print(json.dumps({"child_pid": self.child.pid, "exit": self.child.returncode,
                          "forced_cleanup": forced, "owned_threads_joined": True}))
        complete = self.rows[:-1]
        assert self.rows[-1].keys() == {"trailing_bytes"}
        assert len({row["id"] for row in complete}) == len(complete)
        assert len({row["original"] for row in complete}) == len(complete)
        assert all(row["kind"] == "review_remote_controller_parent" for row in complete)
        # The partial tail is deliberately not classified as an admitted review.
        assert 1 <= len(complete) <= 16


def test_long_human_timeout_does_not_extend_stalled_command_write():
    pipe = PausedPipe(timeout=0.3)
    try:
        started = time.monotonic()
        worker, outcome = pipe.saturate()
        retiring, retired = pipe.run(lambda: pipe.client.retire_controller_review(1))
        assert retiring.is_alive()
        worker.join(timeout=1)
        retiring.join(timeout=1)
        assert not worker.is_alive(), "human timeout retained the SDK write lock"
        assert not retiring.is_alive(), "write timeout left a queued retirement running"
        assert len(retired) == 1 and isinstance(retired[0], HostControlError)
        assert outcome == [None]
        assert time.monotonic() - started < 1
        assert len(pipe.errors) == 1 and isinstance(pipe.errors[0], HostControlError)
        assert "settlement is unknown" in str(pipe.errors[0])
        for review in pipe.handles:
            with pytest.raises(HostControlError) as first:
                review.result()
            with pytest.raises(HostControlError) as second:
                review.result()
            assert first.value is second.value  # Stored error, no resend.
        with pytest.raises(HostControlError):
            pipe.client.retire_controller_review(1)
    finally:
        pipe.finish()


def test_concurrent_retire_and_logout_release_stalled_writer_without_later_admission():
    pipe = PausedPipe(timeout=2)
    try:
        worker, outcome = pipe.saturate()
        retiring, retired = pipe.run(lambda: pipe.client.retire_controller_review(1))
        assert retiring.is_alive()
        closer, closed = pipe.run(pipe.client.close)
        for thread in [closer, retiring, worker]:
            thread.join(timeout=1)
            assert not thread.is_alive(), "logout left an owned write or control waiter running"
        assert closed == [None]
        assert outcome == [None]
        assert len(retired) == 1 and isinstance(retired[0], HostControlError)
        assert len(pipe.errors) == 1 and isinstance(pipe.errors[0], HostControlError)
    finally:
        pipe.finish()


def test_successfully_written_review_can_outlive_the_control_timeout():
    from test_host_control import receive, respond
    read_command, write_command = os.pipe()
    read_response, write_response = os.pipe()
    client = PrivateHostClient(write_command, read_response, timeout=0.03)
    reader = os.fdopen(read_command, "rb")
    writer = os.fdopen(write_response, "wb", buffering=0)
    try:
        original, content, authentication = submission()
        review = client.begin_remote_controller_review(original, content, authentication,
            RemoteControllerLogin("paired-login", 1900000000000), timeout=1)
        request = receive(reader)
        time.sleep(0.06)
        respond(writer, request, {"kind": "controller_parent_reviewed", "outcome": {"state": "unavailable"}})
        assert review.result() == {"state": "unavailable"}
        assert review.result() == {"state": "unavailable"}
    finally:
        client.close()
        assert not client._reader.is_alive()
        assert reader.read() == b""
        reader.close()
        writer.close()
