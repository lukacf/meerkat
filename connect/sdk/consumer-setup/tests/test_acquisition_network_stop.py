"""Real slow HTTP parsing stays inside the existing setup process owner.

Only the test child substitutes a loopback transport for the validated artifact
URL. Production fetch, subprocess execution, deadline and stop handling run.
These cases establish direct-child settlement, not descendant confinement.
"""
from __future__ import annotations

import importlib.util
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

from test_backend import FixtureMixin, backend


SOURCE = Path(__file__).resolve().parents[1] / "python_acquisition.py"
spec = importlib.util.spec_from_file_location("network_stop_acquisition", SOURCE)
acquisition = importlib.util.module_from_spec(spec)
spec.loader.exec_module(acquisition)


class SlowResponse:
    """Keep one real header or chunk-extension line incomplete until closed."""
    def __init__(self, chunked):
        self.ready = threading.Event()
        self.dripped = threading.Event()
        self.finish = threading.Event()
        self.sent = 0
        self.listener = socket.socket()
        self.listener.bind(("127.0.0.1", 0))
        self.listener.listen(1)
        self.listener.settimeout(0.1)
        self.url = "http://127.0.0.1:%d/wheel" % self.listener.getsockname()[1]
        self.thread = threading.Thread(target=self.serve, args=(chunked,), daemon=True)
        self.thread.start()

    def serve(self, chunked):
        try:
            while not self.finish.is_set():
                try:
                    connection, _ = self.listener.accept()
                    break
                except socket.timeout:
                    continue
            else:
                return
            with connection:
                connection.settimeout(2)
                request = bytearray()
                while b"\r\n\r\n" not in request and len(request) < 16384:
                    data = connection.recv(1024)
                    if not data:
                        return
                    request.extend(data)
                prefix = (b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n1;slow="
                          if chunked else b"HTTP/1.1 200 OK\r\nX-Slow: ")
                connection.sendall(prefix)
                self.ready.set()
                while not self.finish.wait(0.02):
                    connection.sendall(b"a")
                    self.sent += 1
                    if self.sent >= 5:
                        self.dripped.set()
        except OSError:
            # The expected kill closes the peer while this server is writing.
            pass

    def close(self):
        self.finish.set()
        self.listener.close()
        self.thread.join(3)
        if self.thread.is_alive():
            raise AssertionError("loopback fixture did not close")


class AcquisitionNetworkStopTests(FixtureMixin, unittest.TestCase):
    def setUp(self):
        super().setUp()
        self.servers = []
        self.children = []

    def tearDown(self):
        for server in self.servers:
            server.close()
        # Bound cleanup even if a regression prevented normal job settlement.
        for child in self.children:
            if child.poll() is None:
                child.kill()
            child.wait(timeout=3)
        for controller in self.controllers:
            for thread in controller.threads:
                thread.join(3)
                self.assertFalse(thread.is_alive(), "setup worker did not settle")
        self.temp.cleanup()

    def exercise(self, *, chunked, stop):
        server = SlowResponse(chunked)
        self.servers.append(server)
        controller = self.controller(runner_factory=backend.Runner)
        job_directory = controller.workspace / "network-job"
        job_directory.mkdir()
        (job_directory / "payloads").mkdir()
        assets = controller.workspace / "network-assets"
        assets.mkdir()
        # Keep the real artifact admission check. Substitute only the network
        # destination inside this fixture child, never in production code.
        (assets / "worker.py").write_text(
            "import importlib.util, sys\n"
            f"spec = importlib.util.spec_from_file_location('child_acquisition', {str(SOURCE)!r})\n"
            "module = importlib.util.module_from_spec(spec)\n"
            "spec.loader.exec_module(module)\n"
            "opener = module.urllib.request.build_opener(module.urllib.request.ProxyHandler({}))\n"
            "class LocalTransport:\n"
            "    def open(self, request, timeout):\n"
            f"        response = opener.open({server.url!r}, timeout=timeout)\n"
            "        response.geturl = lambda: request.full_url\n"
            "        return response\n"
            "module.urllib.request.build_opener = lambda *args, **kwargs: LocalTransport()\n"
            "assert sys.argv[1] == '--fetch'\n"
            "module._fetch_worker(sys.argv[2], sys.argv[3])\n",
            encoding="ascii")
        owner = acquisition.PythonAcquisition.__new__(acquisition.PythonAcquisition)
        owner.python = Path(sys.executable).resolve()
        owner.assets = assets
        owner.io = SimpleNamespace(write_new=backend.write_new, read_file=backend.read_file,
                                   Error=backend.SetupError)
        row = {"url": "https://files.pythonhosted.org/packages/test/slow-1-py3-none-any.whl",
               "sha256": "0" * 64}
        deadline = time.monotonic() + (10 if stop else 1.5)

        def checkpoint():
            controller._check_stop()
            remaining = deadline - time.monotonic()
            if remaining <= 0:
                raise backend.SetupError("python_acquisition_timeout", 409)
            return remaining

        real_popen = subprocess.Popen
        def spawn(*args, **kwargs):
            child = real_popen(*args, **kwargs)
            self.children.append(child)
            return child

        started = time.monotonic()
        with patch.object(backend.subprocess, "Popen", side_effect=spawn):
            with controller.lock:
                job = controller.start_job("acquire", "selected-python", lambda:
                    owner.fetch_retained(row, checkpoint, controller._run, job_directory))
            self.assertTrue(server.ready.wait(1), "child never entered real HTTP parsing")
            if stop:
                self.assertTrue(server.dripped.wait(1), "peer did not keep the response incomplete")
                self.assertEqual(controller.job_detail({"job_id": job["id"]})["job"]["status"], "running")
                requested = controller.stop({"job_id": job["id"]})["job"]
                self.assertEqual(requested["stop"]["status"], "requested")
            for thread in controller.threads:
                thread.join(3)
                self.assertFalse(thread.is_alive(), "trickling HTTP escaped process deadline/stop")
        self.assertLess(time.monotonic() - started, 4)
        observed = controller.job_detail({"job_id": job["id"]})["job"]
        self.assertEqual(observed["status"], "unconfirmed")
        self.assertEqual(observed["error"], "operation_stopped" if stop else "inspection_unavailable")
        if stop:
            self.assertEqual(observed["stop"]["status"], "settled")
        else:
            self.assertGreater(server.sent, 5, "timeout test did not receive a trickling response")
        self.assertFalse(controller.operation_lock.locked())
        self.assertEqual(len(self.children), 1)
        child = self.children[0]
        self.assertEqual(child.returncode, -signal.SIGKILL)
        with self.assertRaises(ChildProcessError):
            os.waitpid(child.pid, os.WNOHANG)
        with self.assertRaises(ProcessLookupError):
            os.kill(child.pid, 0)
        self.assertEqual(list((job_directory / "payloads").iterdir()), [])

    def test_slow_headers_obey_runner_deadline(self):
        self.exercise(chunked=False, stop=False)

    def test_slow_headers_obey_explicit_stop(self):
        self.exercise(chunked=False, stop=True)

    def test_slow_chunk_header_obeys_runner_deadline(self):
        self.exercise(chunked=True, stop=False)

    def test_slow_chunk_header_obeys_explicit_stop(self):
        self.exercise(chunked=True, stop=True)


if __name__ == "__main__":
    unittest.main()
