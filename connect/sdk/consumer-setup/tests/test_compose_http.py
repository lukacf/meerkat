"""Actual loopback HTTP admission for the same Compose bridge."""
import http.client
import json
import threading
import unittest

from test_compose_bridge import ComposeFixture, backend


class ComposeHTTPTests(ComposeFixture, unittest.TestCase):
    def test_successor_routes_use_session_boundary_and_exact_request_replay(self):
        initial = self.controller(); plan = self.prepared(initial)
        self.successor_host(plan); owner = self.controller()
        proposal = self.prepared_successor(owner, plan)
        observation = self.action(owner, "observe")["backend"]
        adopted = self.job(owner, "adopt_successor", predecessor_plan_sha256=plan,
            transition_sha256=proposal["transition_sha256"], observation_sha256=observation["observation_sha256"])
        self.assertEqual(adopted["status"], "succeeded")
        server = backend.Server(owner); thread = threading.Thread(target=server.serve_forever); thread.start()
        try:
            request = {"request_id": "e" * 32, "plan_sha256": proposal["successor"]["plan_sha256"],
                       "transition_sha256": proposal["transition_sha256"]}
            def post(token):
                client = http.client.HTTPConnection(*server.server_address, timeout=5)
                try:
                    client.request("POST", "/api/compose_rebind", json.dumps(request).encode(),
                        {"Host": server.host, "Origin": server.origin, "Content-Type": "application/json", "X-Toolkit-Session": token})
                    response=client.getresponse(); return response.status,json.loads(response.read())
                finally: client.close()
            self.assertEqual(post("wrong"), (403, {"error": "request_not_allowed"}))
            status, first = post(server.capability); self.assertEqual(status, 200)
            owner.join(); status, replay = post(server.capability)
            self.assertEqual(status, 200); self.assertEqual(first["job"]["id"], replay["job"]["id"])
            self.assertEqual(replay["job"]["status"], "succeeded")
            self.assertEqual(replay["job"]["result"]["runtime_bindings"][0]["plan_sha256"], request["plan_sha256"])
            self.assertNotIn("PRIVATE_SUCCESSOR", json.dumps(replay))
        finally:
            server.shutdown(); thread.join(); server.server_close()

    def test_browser_metadata_uses_enabled_host_scope(self):
        owner = self.controller()
        browser = backend.BrowserCompanion(owner)
        try:
            self.assertEqual(browser.metadata["scope"], owner.state()["scope"])
            self.assertIn("container-plan execution", browser.metadata["scope"])
            self.assertNotEqual(browser.metadata["scope"], backend.SCOPE)
        finally:
            browser.close()

    def test_session_boundary_prepare_replay_and_disabled_stop(self):
        owner = self.controller()
        server = backend.Server(owner)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()
        def post(route, data, token):
            client = http.client.HTTPConnection(*server.server_address, timeout=5)
            headers = {"Host": server.host, "Origin": server.origin, "Content-Type": "application/json"}
            if token is not None: headers["X-Toolkit-Session"] = token
            try:
                client.request("POST", route, json.dumps(data).encode(), headers)
                response = client.getresponse()
                return response.status, json.loads(response.read())
            finally:
                client.close()
        try:
            request = {"request_id": "5" * 32, **self.stage()}
            for token in (None, "wrong"):
                status, value = post("/api/compose_prepare", request, token)
                self.assertEqual(status, 403)
                self.assertEqual(value, {"error": "request_not_allowed"})
            self.assertFalse(self.graph.exists())
            status, first = post("/api/compose_prepare", request, server.capability)
            self.assertEqual(status, 200)
            owner.join()
            status, replay = post("/api/compose_prepare", request, server.capability)
            self.assertEqual(replay["job"]["id"], first["job"]["id"])
            self.assertEqual(replay["job"]["status"], "succeeded")
            self.assertNotIn("plan", replay["job"]["result"]["backend"])
            self.assertIn("summary", replay["job"]["result"]["backend"])
            self.assertNotIn("exact configuration", json.dumps(replay))
            self.assertNotIn("PRIVATE_COMPOSE_JSON", json.dumps(replay))
            status, refused = post("/api/stop", {"job_id": first["job"]["id"]}, server.capability)
            self.assertEqual(status, 409)
            self.assertEqual(refused, {"error": "job_stop_not_supported"})
            self.assertEqual(self.calls(), ["planner", "prepare"])
            self.configure(exit_code=1, stderr="engine_changed\n")
            start = {"request_id": "7" * 32,
                     "plan_sha256": replay["job"]["result"]["backend"]["plan_sha256"]}
            status, _ = post("/api/compose_start", start, server.capability)
            self.assertEqual(status, 200)
            owner.join()
            status, failed = post("/api/compose_start", start, server.capability)
            self.assertEqual(status, 200)
            self.assertEqual(failed["job"]["runtime_error"], "engine_changed")
            self.assertEqual(failed["job"]["error"], "compose_operation_unconfirmed")
            self.assertEqual(failed["job"]["status"], "unconfirmed")
            self.assertEqual(self.calls().count("start"), 1)
            status, refused = post("/api/compose_observe", {}, server.capability)
            self.assertEqual(status, 409)
            self.assertEqual(refused, {"error": "compose_host_reopen_required"})
        finally:
            server.shutdown(); thread.join(); server.server_close()


if __name__ == "__main__": unittest.main()
