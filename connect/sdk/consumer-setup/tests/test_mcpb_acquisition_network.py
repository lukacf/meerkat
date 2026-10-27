"""Real local HTTP transport under the unchanged public HTTPS admission checks.

Only a test worker maps approved public origins to loopback. No production
HTTP/TLS bypass exists. These tests make no public network requests.
"""
from __future__ import annotations

from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import threading
import time
import unittest
from unittest.mock import patch
import urllib.parse
import urllib.request

from test_mcpb_acquisition import AcquisitionFixture, B, M
from test_acquisition_network_stop import SlowResponse
from test_backend import FixtureMixin, backend


class NetworkFixture:
    def __init__(self, routes):
        self.routes, self.calls = routes, []
        fixture = self
        class Handler(BaseHTTPRequestHandler):
            def do_GET(self):
                fixture.calls.append((self.path, dict(self.headers)))
                status, headers, body = fixture.routes[self.path]
                self.send_response(status)
                for name, value in headers:
                    self.send_header(name, value)
                self.end_headers()
                self.wfile.write(body)
            def log_message(self, *_):
                pass
        self.server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
        self.url = "http://127.0.0.1:%d" % self.server.server_port
        self.thread = threading.Thread(target=self.server.serve_forever, daemon=True)
        self.thread.start()

    def close(self):
        self.server.shutdown()
        self.server.server_close()
        self.thread.join(2)

    def transport(self):
        opener = urllib.request.build_opener(urllib.request.ProxyHandler({}), M.NoRedirect())
        base = self.url
        class LocalTransport:
            def open(self, request, timeout):
                parts = urllib.parse.urlsplit(request.full_url)
                local = base + parts.path + ("?" + parts.query if parts.query else "")
                response = opener.open(urllib.request.Request(local, headers=dict(request.header_items())), timeout=timeout)
                response.geturl = lambda: request.full_url
                return response
        return LocalTransport()


def worker_source(url, *, fixed=False):
    return (
        "import importlib.util, sys, urllib.parse, urllib.request\n"
        f"spec=importlib.util.spec_from_file_location('owner', {str(Path(M.__file__))!r})\n"
        "m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m)\n"
        "opener=urllib.request.build_opener(urllib.request.ProxyHandler({}),m.NoRedirect())\n"
        "class LocalTransport:\n"
        " def open(self, request, timeout):\n"
        "  p=urllib.parse.urlsplit(request.full_url)\n"
        f"  local={url!r}" + ("\n" if fixed else "+p.path+('?' + p.query if p.query else '')\n") +
        "  r=opener.open(urllib.request.Request(local,headers=dict(request.header_items())),timeout=timeout)\n"
        "  r.geturl=lambda:request.full_url\n"
        "  return r\n"
        "m.urllib.request.build_opener=lambda *a,**k:LocalTransport()\n"
        "try:\n"
        " m._fetch_worker(sys.argv[2],sys.argv[3])\n"
        "except Exception:\n"
        " print('mcpb_acquisition_worker_failed',file=sys.stderr);sys.exit(75)\n"
    ).encode("ascii")


class McpbNetworkTests(AcquisitionFixture, unittest.TestCase):
    def serve(self, routes):
        fixture = NetworkFixture(routes)
        self.addCleanup(fixture.close)
        return fixture

    def test_real_worker_signed_redirect_query_is_not_retained(self):
        query = "temporary-signature=private-test-value"
        fixture = self.serve({"/releases/package.mcpb": (302, [("Location", "https://assets.example/download?" + query)], b""),
                              "/download?" + query: (200, [("Content-Length", str(len(self.payload)))], self.payload)})
        owner = self.owner()
        wrapper = self.root / "transport-worker.py"
        wrapper.write_bytes(worker_source(fixture.url))
        owner.worker_path, owner.worker_raw = wrapper, wrapper.read_bytes()
        owner.frozen["worker.py"] = owner.worker_raw
        (owner.assets / "worker.py").chmod(0o600)
        (owner.assets / "worker.py").write_bytes(owner.worker_raw)
        captured = []
        def run(argv, **kwargs):
            value = subprocess.run(argv, capture_output=True, timeout=kwargs["inspection_timeout"],
                env={"HTTP_PROXY": "http://127.0.0.1:1", "HTTPS_PROXY": "http://127.0.0.1:1", "ALL_PROXY": "http://127.0.0.1:1"})
            captured.append(value)
            return value.returncode, value.stdout
        result = self.execute(owner, run)
        self.assertEqual(result["primary"]["redirect_count"], 1)
        self.assertEqual(result["primary"]["final_origin"], "https://assets.example")
        self.assertEqual(len(fixture.calls), 2)
        self.assertEqual(captured[0].stderr, b"")
        self.assertEqual(captured[0].stdout, b"")
        for path in self.root.rglob("*"):
            if path.is_file():
                self.assertNotIn(query.encode(), path.read_bytes(), str(path))
        self.assertNotIn(query, json.dumps(result))
        self.assertNotIn("Authorization", fixture.calls[1][1])
        self.assertNotIn("Cookie", fixture.calls[1][1])
        self.assertEqual(M.readback(self.io, self.record(owner), self.job), result)

    def test_unapproved_or_unsafe_redirect_is_never_requested(self):
        for target in ("https://other.example/hidden?key=test", "http://assets.example/hidden",
                       "https://user:password@assets.example/hidden", "https://assets.example/hidden#fragment"):
            with self.subTest(target=target):
                fixture = self.serve({"/releases/package.mcpb": (302, [("Location", target)], b"")})
                transport = fixture.transport()
                with patch.object(M.urllib.request, "build_opener", return_value=transport):
                    with self.assertRaises(ValueError):
                        M.fetch(self.request, self.profile, lambda: 10)
                self.assertEqual(len(fixture.calls), 1)

    def test_redirect_bound_and_loop_refuse_without_extra_request(self):
        routes = {"/releases/package.mcpb": (302, [("Location", "https://assets.example/1")], b"")}
        routes.update({"/" + str(i): (302, [("Location", "https://assets.example/" + str(i + 1))], b"") for i in range(1, 6)})
        fixture = self.serve(routes)
        with patch.object(M.urllib.request, "build_opener", return_value=fixture.transport()):
            with self.assertRaises(ValueError):
                M.fetch(self.request, self.profile, lambda: 10)
        self.assertEqual(len(fixture.calls), 6)
        fixture.routes["/1"] = (302, [("Location", "https://assets.example/1")], b"")
        fixture.calls.clear()
        with patch.object(M.urllib.request, "build_opener", return_value=fixture.transport()):
            with self.assertRaises(ValueError):
                M.fetch(self.request, self.profile, lambda: 10)
        self.assertEqual(len(fixture.calls), 2)

    def test_content_length_encoding_hash_and_size_refusals(self):
        cases = [(200, [("Content-Length", "999")], self.payload),
                 (200, [("Content-Length", str(M.ARTIFACT_LIMIT + 1))], b""),
                 (200, [("Content-Length", "1"), ("Content-Length", "1")], b"x"),
                 (200, [("Content-Encoding", "gzip")], self.payload),
                 (200, [], b"wrong hash"), (200, [], self.payload)]
        for i, response in enumerate(cases):
            with self.subTest(case=i):
                fixture = self.serve({"/releases/package.mcpb": response})
                transport = fixture.transport()
                with patch.object(M.urllib.request, "build_opener", return_value=transport):
                    with patch.object(M, "ARTIFACT_LIMIT", 8 if i == 5 else M.ARTIFACT_LIMIT):
                        with self.assertRaises(ValueError):
                            M.fetch(self.request, self.profile, lambda: 10)
                self.assertEqual(len(fixture.calls), 1)

    def test_duplicate_location_refused(self):
        fixture = self.serve({"/releases/package.mcpb": (302, [("Location", "https://assets.example/one"),
                                                               ("Location", "https://assets.example/two")], b"")})
        with patch.object(M.urllib.request, "build_opener", return_value=fixture.transport()):
            with self.assertRaises(ValueError):
                M.fetch(self.request, self.profile, lambda: 10)
        self.assertEqual(len(fixture.calls), 1)


class McpbSlowNetworkTests(FixtureMixin, unittest.TestCase):
    def tearDown(self):
        # FixtureMixin's default teardown is specific to its mock runner.
        for controller in self.controllers:
            controller.join()
        self.temp.cleanup()

    def exercise(self, *, chunked, stop):
        server = SlowResponse(chunked)
        self.addCleanup(server.close)
        controller = self.controller(runner_factory=backend.Runner)
        stage = self.root / "mcpb-staging"
        stage.mkdir()
        profile = {"schema_version": 1, "scope": M.SCOPE,
                   "python": {"path": str(Path(sys.executable).resolve()),
                              "sha256": M.sha(Path(sys.executable).resolve().read_bytes())},
                   "allowed_origins": ["https://source.example"], "allowed_redirect_origins": []}
        profile_path = self.root / "mcpb-profile.json"
        profile_path.write_bytes(M.wire(profile))
        io_context = type("IO", (), {"Error": backend.SetupError, "read_file": staticmethod(backend.read_file),
            "write_new": staticmethod(backend.write_new), "canonical_directory": staticmethod(backend.canonical_directory),
            "decode": staticmethod(backend.decode), "keys": staticmethod(backend.keys)})
        owner = M.McpbAcquisition(profile_path, M.sha(profile_path.read_bytes()), stage, io=io_context)
        wrapper = self.root / "slow-worker.py"
        wrapper.write_bytes(worker_source(server.url, fixed=True))
        owner.worker_path, owner.worker_raw = wrapper, wrapper.read_bytes()
        owner.frozen["worker.py"] = owner.worker_raw
        owner.bind(controller.workspace)
        parent = controller.workspace / "acquisitions"
        parent.mkdir(exist_ok=True)
        directory = parent / ("e" * 32)
        directory.mkdir()
        request = {"request_id": "f" * 32, "url": "https://source.example/slow", "artifact_sha256": "0" * 64}
        (directory / "request.json").write_bytes(M.wire(request))
        children = []
        real_popen = subprocess.Popen
        def spawn(*args, **kwargs):
            child = real_popen(*args, **kwargs)
            children.append(child)
            return child
        started = time.monotonic()
        try:
            with patch.object(backend.subprocess, "Popen", side_effect=spawn), patch.object(M, "TIMEOUT", 10 if stop else 1.5):
                with controller.lock:
                    job = controller.start_job("acquire", "selected-mcpb", lambda:
                        owner.run(request, directory, controller._run, controller._check_stop))
                self.assertTrue(server.ready.wait(1))
                if stop:
                    self.assertTrue(server.dripped.wait(1))
                    controller.stop({"job_id": job["id"]})
                for thread in controller.threads:
                    thread.join(3)
                    self.assertFalse(thread.is_alive())
            observed = controller.job_detail({"job_id": job["id"]})["job"]
            self.assertEqual(observed["status"], "unconfirmed")
            self.assertEqual(observed["error"], "operation_stopped" if stop else "inspection_unavailable")
            self.assertLess(time.monotonic() - started, 4)
            self.assertEqual(len(children), 1)
            self.assertEqual(children[0].returncode, -signal.SIGKILL)
            self.assertFalse((stage / directory.name / "acquisition.json").exists())
            with self.assertRaises(ProcessLookupError):
                os.kill(children[0].pid, 0)
        finally:
            for child in children:
                if child.poll() is None:
                    child.kill()
                child.wait(timeout=3)

    def test_slow_headers_obey_deadline(self):
        self.exercise(chunked=False, stop=False)

    def test_slow_chunks_obey_deadline(self):
        self.exercise(chunked=True, stop=False)

    def test_slow_headers_obey_stop(self):
        self.exercise(chunked=False, stop=True)


if __name__ == "__main__":
    unittest.main()
