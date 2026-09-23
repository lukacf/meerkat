#!/usr/bin/env python3
"""Offline subprocess regressions for examples 004 and 010; never builds Rust."""

import http.server
import json
import os
from pathlib import Path
import shutil
import subprocess
import threading
import unittest
import uuid


HERE = Path(__file__).resolve().parent
REPO = HERE.parent.parent


def cli_binary():
    override = os.environ.get("RKAT_TEST_BIN")
    if override:
        return str(Path(override).resolve())
    output = subprocess.check_output(
        [REPO / "scripts/repo-cargo", "--print-env"], text=True, cwd=REPO
    )
    target = next(line.split("=", 1)[1] for line in output.splitlines()
                  if line.startswith("CARGO_TARGET_DIR="))
    binary = Path(target) / "debug/rkat"
    if not binary.is_file():
        raise RuntimeError("Build the current CLI first, or set RKAT_TEST_BIN")
    return str(binary)


def run(command, *, env=None, cwd=REPO):
    return subprocess.run(command, cwd=cwd, env=env, text=True, capture_output=True,
                          timeout=60)


class McpBoundaryTests(unittest.TestCase):
    def exchange(self, *requests):
        data = "\n".join(item if isinstance(item, str) else json.dumps(item)
                         for item in requests) + "\n"
        result = subprocess.run(
            ["python3", HERE / "demo_mcp_server.py"], input=data,
            text=True, capture_output=True, timeout=5
        )
        self.assertEqual(result.returncode, 0, result.stderr)
        return [json.loads(line) for line in result.stdout.splitlines()]

    def test_invalid_shapes_preserve_following_ping(self):
        ping = {"jsonrpc": "2.0", "id": 2, "method": "ping"}
        cases = [
            ("{", -32700, None),
            ([], -32600, None),
            (None, -32600, None),
            ({"jsonrpc": "2.0", "id": [], "method": "ping"}, -32600, None),
            ({"jsonrpc": "2.0", "id": 1, "method": 3}, -32600, 1),
        ]
        for params in (None, [], "invalid", {"name": "incident_digest", "arguments": None},
                       {"name": "incident_digest", "arguments": []},
                       {"name": None, "arguments": {"service": "checkout-api"}},
                       {"name": "incident_digest", "arguments": {"service": []}},
                       {"name": "incident_digest", "arguments": {}},
                       {"name": "incident_digest", "arguments": {"service": "a", "extra": 1}}):
            cases.append(({"jsonrpc": "2.0", "id": 1, "method": "tools/call",
                           "params": params}, -32602, 1))
        for malformed, code, request_id in cases:
            with self.subTest(request=malformed):
                error, pong = self.exchange(malformed, ping)
                self.assertEqual(error["error"]["code"], code)
                self.assertEqual(error["id"], request_id)
                self.assertEqual(pong, {"jsonrpc": "2.0", "id": 2, "result": {}})

    def test_valid_protocol_tools_and_business_error(self):
        requests = [
            {"jsonrpc": "2.0", "id": 1, "method": "initialize"},
            {"jsonrpc": "2.0", "method": "notifications/initialized"},
            {"jsonrpc": "2.0", "id": 2, "method": "tools/list"},
        ]
        for index, (name, service) in enumerate((
            ("incident_digest", "checkout-api"),
            ("release_readiness", "checkout-api"),
            ("incident_digest", "unknown-service"),
            ("unknown_tool", "checkout-api"),
        ), 3):
            requests.append({"jsonrpc": "2.0", "id": index, "method": "tools/call",
                             "params": {"name": name, "arguments": {"service": service}}})
        requests.append({"jsonrpc": "2.0", "id": None, "method": "ping"})
        responses = self.exchange(*requests)
        self.assertEqual(responses[0]["result"]["serverInfo"]["name"], "incident-kit")
        self.assertEqual(len(responses[1]["result"]["tools"]), 2)
        self.assertIn("severity: sev-1", responses[2]["result"]["content"][0]["text"])
        self.assertIn("release_readiness: hold", responses[3]["result"]["content"][0]["text"])
        self.assertTrue(responses[4]["result"]["isError"])
        self.assertEqual(responses[5]["error"]["code"], -32601)
        self.assertEqual(responses[6], {"jsonrpc": "2.0", "id": None, "result": {}})


class CliTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.binary = cli_binary()

    def setUp(self):
        self.work = HERE / ".work" / f"regression-{uuid.uuid4().hex}"
        self.work.mkdir(parents=True)
        self.addCleanup(shutil.rmtree, self.work)
        self.home = self.work / "home"
        self.home.mkdir()
        self.log = self.work / "argv.jsonl"
        self.wrapper = self.work / "binary with spaces" / "rkat fixture"
        self.wrapper.parent.mkdir()
        self.wrapper.write_text("""#!/usr/bin/env python3
import json, os, pathlib, subprocess, sys
args = sys.argv[1:]
with open(os.environ["ARGV_LOG"], "a") as log:
    log.write(json.dumps(args) + "\\n")
if os.environ.get("FAKE_ALL") == "1":
    sys.exit(0)
if "run" in args:
    if os.environ.get("OFFLINE_PROVIDER") == "1":
        args += ["--auth-binding", "global:offline"]
    elif os.environ.get("NO_PROVIDER_AUTH") == "1":
        os.environ.pop("ANTHROPIC_API_KEY", None)
    else:
        sys.exit(int(os.environ.get("AGENT_EXIT", "0")))
sys.exit(subprocess.call([os.environ["REAL_RKAT"], *args], stdin=subprocess.DEVNULL))
""")
        self.wrapper.chmod(0o755)
        self.env = {
            "PATH": os.environ["PATH"], "HOME": str(self.home),
            "RKAT": str(self.wrapper), "REAL_RKAT": self.binary,
            "ARGV_LOG": str(self.log), "ANTHROPIC_API_KEY": "synthetic-not-a-credential",
        }

    def copy_example(self, name):
        destination = self.work / "checkout with spaces" / name
        shutil.copytree(REPO / "examples" / name, destination,
                        ignore=shutil.ignore_patterns(".work", "__pycache__"))
        return destination

    def assert_ok(self, result):
        self.assertEqual(result.returncode, 0, result.stdout + result.stderr)

    def calls(self):
        return [json.loads(line) for line in self.log.read_text().splitlines()]

    def assert_models(self, calls):
        turns = [args for args in calls if "run" in args]
        self.assertTrue(turns)
        for args in turns:
            if "--resume" in args:
                self.assertNotIn("--model", args)
                self.assertNotIn("--provider", args)
            else:
                self.assertEqual(args[args.index("--model") + 1], "claude-sonnet-4-6")

    def test_004_every_call_accepts_literal_executable_path(self):
        example = self.copy_example("004-cli-one-liners-sh")
        self.assert_ok(run(["bash", example / "examples.sh"],
                           env=self.env | {"FAKE_ALL": "1"}))
        calls = self.calls()
        self.assertEqual(len(calls), 8)
        self.assertEqual(sum("run" in args for args in calls), 6)
        self.assert_models(calls)

    def test_010_absolute_relative_and_path_overrides_and_repeatability(self):
        example = self.copy_example("010-mcp-tool-server-sh")
        env = self.env.copy()
        overrides = [
            str(self.wrapper),
            os.path.relpath(self.wrapper, REPO),
            self.wrapper.name,
        ]
        env["PATH"] = str(self.wrapper.parent) + os.pathsep + env["PATH"]
        for override in overrides:
            with self.subTest(override=override):
                self.assert_ok(run(["bash", example / "setup.sh"],
                                   env=env | {"RKAT": override}))
        # Failure must propagate while its successful registration is still removed.
        failed = run(["bash", example / "setup.sh"], env=env | {"AGENT_EXIT": "23"})
        self.assertEqual(failed.returncode, 23, failed.stdout + failed.stderr)
        retry = run(["bash", example / "setup.sh"], env=env)
        self.assert_ok(retry)
        self.assert_models(self.calls())
        self.assertEqual(sum("run" in args for args in self.calls()), 5)

        roots = ["--state-root", str(example / ".work/state"),
                 "--context-root", str(example / ".work/project"),
                 "--user-config-root", str(example / ".work/user")]
        cwd = example / ".work/project"
        def cli(*args):
            return run([self.binary, *roots, *args], env=env, cwd=cwd)
        self.assert_ok(cli("mcp", "add", "unrelated", "--scope", "project", "--", "python3", "-V"))
        self.assert_ok(cli("mcp", "add", "incident-kit", "--scope", "project", "--",
                           "python3", str(example / "demo_mcp_server.py")))
        cleanup = next(line for line in retry.stdout.splitlines() if line.startswith("(cd "))
        self.assert_ok(run(["bash", "-c", cleanup], env=env))
        self.assert_ok(cli("mcp", "get", "unrelated", "--scope", "project"))
        self.assertNotEqual(cli("mcp", "get", "incident-kit", "--scope", "project").returncode, 0)

        # A pre-existing entry with the demo's name is never claimed by the EXIT trap.
        self.assert_ok(cli("mcp", "add", "incident-kit", "--scope", "project", "--", "python3", "-V"))
        conflict = run(["bash", example / "setup.sh"], env=env)
        self.assertNotEqual(conflict.returncode, 0)
        self.assert_ok(cli("mcp", "get", "incident-kit", "--scope", "project"))

    def test_current_cli_uses_anthropic_and_real_mcp_offline(self):
        requests = []
        failures = []
        requests_by_example = {}

        def named_results(body):
            names = {}
            results = {}
            for message in body["messages"]:
                content = message.get("content")
                if not isinstance(content, list):
                    continue
                for block in content:
                    if block.get("type") == "tool_use":
                        names[block["id"]] = block["name"]
                    elif block.get("type") == "tool_result":
                        results[names[block["tool_use_id"]]] = block
            return results

        def result_json(block):
            content = block["content"]
            if isinstance(content, list):
                content = "\n".join(part["text"] for part in content)
            return json.loads(content)

        class Provider(http.server.BaseHTTPRequestHandler):
            def log_message(self, *_):
                pass

            def do_POST(self):
                body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
                requests.append(body)
                if self.path != "/v1/messages":
                    failures.append(self.path)
                    self.send_error(404)
                    return
                tools = [tool for tool in body.get("tools", [])
                         if tool["name"].endswith(("incident_digest", "release_readiness"))]
                visible_names = {tool["name"] for tool in body.get("tools", [])}
                incident = any("on-call incident coordinator" in json.dumps(message)
                               for message in body["messages"])
                results = named_results(body)
                business_results = [name for name in results
                                    if name.endswith(("incident_digest", "release_readiness"))]
                calls = []
                if incident and len(business_results) != 2:
                    if len(tools) == 2:
                        calls = [(tool["name"], {"service": "checkout-api"}) for tool in tools]
                    elif "tool_catalog_search" in visible_names and "tool_catalog_search" not in results:
                        calls = [("tool_catalog_search", {"query": "", "limit": 50})]
                    elif "tool_catalog_load" in visible_names and "tool_catalog_load" not in results:
                        found = result_json(results["tool_catalog_search"])["results"]
                        names = [entry["name"] for entry in found
                                 if entry["name"].endswith(("incident_digest", "release_readiness"))]
                        if len(names) == 2:
                            calls = [("tool_catalog_load", {"names": names})]
                    if not calls:
                        failures.append({"visible": sorted(visible_names), "results": results})
                        self.send_error(400, "Incident tools could not be discovered and loaded")
                        return
                events = [
                    {"type": "message_start", "message": {
                        "id": "msg_fixture", "type": "message", "role": "assistant",
                        "model": body["model"], "content": [], "stop_reason": None,
                        "usage": {"input_tokens": 5, "output_tokens": 0}}},
                ]
                if calls:
                    for index, (name, arguments) in enumerate(calls):
                        events.extend([
                            {"type": "content_block_start", "index": index, "content_block": {
                                "type": "tool_use", "id": f"call_{len(requests)}_{index}",
                                "name": name, "input": {}}},
                            {"type": "content_block_delta", "index": index, "delta": {
                                "type": "input_json_delta",
                                "partial_json": json.dumps(arguments)}},
                            {"type": "content_block_stop", "index": index},
                        ])
                    stop = "tool_use"
                else:
                    events.extend([
                        {"type": "content_block_start", "index": 0,
                         "content_block": {"type": "text", "text": ""}},
                        {"type": "content_block_delta", "index": 0,
                         "delta": {"type": "text_delta", "text": "OFFLINE_OK"}},
                        {"type": "content_block_stop", "index": 0},
                    ])
                    stop = "end_turn"
                events.extend([
                    {"type": "message_delta", "delta": {"stop_reason": stop},
                     "usage": {"output_tokens": 5}},
                    {"type": "message_stop"},
                ])
                payload = "".join(f"event: {event['type']}\ndata: {json.dumps(event)}\n\n"
                                  for event in events).encode()
                self.send_response(200)
                self.send_header("Content-Type", "text/event-stream")
                self.send_header("Content-Length", str(len(payload)))
                self.end_headers()
                self.wfile.write(payload)

        server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Provider)
        thread = threading.Thread(target=server.serve_forever, daemon=True)
        thread.start()
        def stop_server():
            server.shutdown()
            server.server_close()
            thread.join(timeout=5)
        self.addCleanup(stop_server)
        offline_config = f"""
[realm.global.backend.offline]
provider = "anthropic"
backend_kind = "anthropic_api"
base_url = "http://127.0.0.1:{server.server_port}"
[realm.global.auth.offline]
provider = "anthropic"
auth_method = "api_key"
source = {{ kind = "inline_secret", secret = "synthetic-not-a-credential" }}
[realm.global.binding.offline]
backend_profile = "offline"
auth_profile = "offline"
"""
        env = self.env | {"OFFLINE_PROVIDER": "1"}
        for name, script in (("004-cli-one-liners-sh", "examples.sh"),
                             ("010-mcp-tool-server-sh", "setup.sh")):
            example = self.copy_example(name)
            config = example / ".work/user/.rkat/config.toml"
            config.parent.mkdir(parents=True)
            config.write_text(offline_config)
            before = len(requests)
            result = run(["bash", example / script], env=env)
            self.assert_ok(result)
            self.assertIn("OFFLINE_OK", result.stdout)
            requests_by_example[name] = requests[before:]
        self.assert_models(self.calls())
        self.assertEqual(failures, [])
        self.assertEqual(len(requests_by_example["004-cli-one-liners-sh"]), 6)
        mcp_requests = requests_by_example["010-mcp-tool-server-sh"]
        final_results = named_results(mcp_requests[-1])
        if "tool_catalog_search" in final_results:
            self.assertEqual(len(mcp_requests), 4)  # search, load, actual tools, answer
            loaded = result_json(final_results["tool_catalog_load"])
            self.assertEqual(len(loaded["accepted_names"] + loaded["noop_names"]), 2)
        else:
            self.assertEqual(len(mcp_requests), 2)  # inline actual tools, answer
        self.assertTrue(all(body["model"] == "claude-sonnet-4-6" for body in requests))
        tool_results = [block for name, block in final_results.items()
                        if name.endswith(("incident_digest", "release_readiness"))]
        self.assertEqual(len(tool_results), 2)
        self.assertTrue(all(not block["is_error"] for block in tool_results))
        self.assertIn("severity: sev-1", json.dumps(tool_results))
        self.assertIn("release_readiness: hold", json.dumps(tool_results))

    def test_clean_current_cli_fails_on_anthropic_not_openai(self):
        for name, script in (("004-cli-one-liners-sh", "examples.sh"),
                             ("010-mcp-tool-server-sh", "setup.sh")):
            example = self.copy_example(name)
            result = run(["bash", example / script],
                         env=self.env | {"NO_PROVIDER_AUTH": "1"})
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("provider=anthropic", result.stderr.lower())
            self.assertIn("missing_secret", result.stderr)
            self.assertNotIn("provider=openai", result.stderr.lower())


if __name__ == "__main__":
    unittest.main()
