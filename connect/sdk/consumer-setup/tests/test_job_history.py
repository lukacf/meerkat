"""Read-only retained jobs use the existing controller records and observations."""
import copy
import http.client
import json
import threading
import unittest

from test_reconciliation import ReconciliationMixin, backend


class JobHistoryTests(ReconciliationMixin, unittest.TestCase):
    def test_prior_result_and_observation_survive_second_install_without_work(self):
        controller, first_request, first = self.installed()
        observed = controller.reconcile({"job_id": first})["observation"]
        second = controller.install(self.checked(controller))["job"]["id"]
        controller.join()
        current = copy.deepcopy(controller.job)
        calls = list(controller.runner.calls)
        page = controller.dispatch("/api/jobs", {"before": None})
        self.assertEqual([row["id"] for row in page["jobs"]][:2], [second, first])
        self.assertEqual(page["jobs"][1]["profile"], "searxng")
        self.assertNotIn("result", page["jobs"][1])
        detail = controller.dispatch("/api/job", {"job_id": first})
        self.assertEqual(detail["job"], controller.install(first_request)["job"])
        self.assertEqual(detail["reconciliation"]["observation"], observed)
        detail["job"]["result"]["mcp_configuration"].clear()
        detail["reconciliation"]["observation"]["result"].clear()
        again = controller.job_detail({"job_id": first})
        self.assertTrue(again["job"]["result"]["mcp_configuration"])
        self.assertTrue(again["reconciliation"]["observation"]["result"])
        self.assertEqual(controller.job, current)
        self.assertIsNone(controller.state()["reconciliation"]["observation"])
        self.assertEqual(controller.runner.calls, calls)

    def test_bounded_pages_refuse_unknown_cursor_and_do_not_leak_request_fields(self):
        controller = self.controller()
        # Populate existing-owner records to exercise page size without 53 installs.
        for index in range(53):
            identifier = f"{index:032x}"
            controller.jobs[identifier] = {"id": identifier, "kind": "install", "family": "agno", "status": "unconfirmed", "private_fixture": "excluded"}
        first = controller.job_summaries({"before": None})
        second = controller.job_summaries({"before": first["next_before"]})
        self.assertEqual((len(first["jobs"]), len(second["jobs"])), (50, 3))
        self.assertIsNone(second["next_before"])
        self.assertEqual(len({row["id"] for row in first["jobs"] + second["jobs"]}), 53)
        self.assertTrue(all(set(row) == {"id", "kind", "family", "status", "profile"} for row in first["jobs"]))
        self.assertEqual(controller.state()["job_history"], {"available": True, "count": 53})
        for value in (None, [], {}, "unknown"):
            self.error("unknown_job", lambda: controller.job_detail({"job_id": value}))
        self.error("unknown_job", lambda: controller.job_summaries({"before": "unknown"}))
        self.error("invalid_request", lambda: controller.job_detail({"job_id": "0" * 32, "path": "ignored"}))
        self.error("invalid_request", lambda: controller.job_summaries({}))

    def test_prior_reconciliation_is_visible_and_global_busy_while_current_stays_fixed(self):
        controller, _, first = self.installed()
        second = controller.install(self.checked(controller))["job"]["id"]
        controller.join()
        controller.runner.verify_entered.clear()
        controller.runner.verify_block = threading.Event()
        worker = threading.Thread(target=lambda: controller.reconcile({"job_id": first}))
        worker.start()
        try:
            self.assertTrue(controller.runner.verify_entered.wait(2))
            self.assertTrue(controller.state()["operation_in_progress"])
            self.assertEqual(controller.state()["job"]["id"], second)
            self.assertEqual(controller.job_detail({"job_id": first})["reconciliation"]["observation"]["status"], "checking")
        finally:
            controller.runner.verify_block.set()
            worker.join(2)
        self.assertFalse(worker.is_alive())
        self.assertFalse(controller.state()["operation_in_progress"])
        self.assertEqual(controller.job_detail({"job_id": first})["reconciliation"]["observation"]["status"], "matched")

    def test_other_controller_and_recovery_view_only_expose_known_records(self):
        original, _, identifier = self.installed()
        other = self.controller()
        self.error("unknown_job", lambda: other.job_detail({"job_id": identifier}))
        self.assertEqual(other.job_summaries({"before": None}), {"jobs": [], "next_before": None})
        recovery = original.job["recovery_request"]
        recovered = self.controller(recover_workspace=original.workspace,
                                    recover_request=recovery["id"], recover_request_sha256=recovery["sha256"])
        self.assertEqual(recovered.state()["mode"], "recovery")
        self.assertEqual([row["id"] for row in recovered.job_summaries({"before": None})["jobs"]], [identifier])
        self.assertEqual(recovered.job_detail({"job_id": identifier})["job"]["status"], "unconfirmed")
        self.assertEqual(len(recovered.runner.calls), 0)

    def test_http_history_keeps_session_origin_and_read_only_boundaries(self):
        controller, _, identifier = self.installed()
        expected = copy.deepcopy(controller.job)
        calls = list(controller.runner.calls)
        server = backend.Server(controller)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()

        def call(path, body, token, origin):
            connection = http.client.HTTPConnection(*server.server_address, timeout=3)
            try:
                connection.request("POST", path, backend.encoded(body), headers={
                    "Origin": origin, "X-Toolkit-Session": token, "Content-Type": "application/json"})
                response = connection.getresponse()
                return response.status, json.loads(response.read())
            finally:
                connection.close()

        try:
            for path, body in (("/api/jobs", {"before": None}), ("/api/job", {"job_id": identifier})):
                self.assertEqual(call(path, body, "wrong", server.origin), (403, {"error": "request_not_allowed"}))
                self.assertEqual(call(path, body, server.capability, "https://foreign.invalid"), (403, {"error": "request_not_allowed"}))
                self.assertEqual(call(path, body, server.capability, server.origin)[0], 200)
            self.assertEqual(call("/api/job", {"job_id": identifier}, server.capability, server.origin)[1]["job"], expected)
            self.assertEqual(call("/api/job", {"job_id": "foreign"}, server.capability, server.origin), (404, {"error": "unknown_job"}))
            self.assertEqual(call("/api/jobs", {"before": None, "path": "/other"}, server.capability, server.origin), (400, {"error": "invalid_request"}))
            self.assertEqual(controller.job, expected)
            self.assertEqual(controller.runner.calls, calls)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(3)
        self.assertFalse(thread.is_alive())


if __name__ == "__main__":
    unittest.main()
