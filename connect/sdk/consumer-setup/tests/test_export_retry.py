"""Explicit export retries through the existing Controller and real MCP SDK."""
import asyncio
import copy
import http.client
import json
import os
import threading
import time
import unittest
from unittest.mock import patch

from mcp import Client
import test_hosted_recipes as hosted

backend, adapter = hosted.backend, hosted.adapter


def tree(root):
    return {str(path.relative_to(root)): path.read_bytes()
            for path in root.rglob("*") if path.is_file()}


class ExportRetryFixture(hosted.HostedFixture):
    def failed(self, controller=None):
        controller = controller or self.retained()
        controller.runner.mode = "lost_export"
        job = controller.export_recipe(hosted.EXPORT)["job"]
        controller.join()
        controller.runner.mode = None
        self.assertEqual(controller.jobs[job["id"]]["status"], "unconfirmed")
        return controller, copy.deepcopy(controller.jobs[job["id"]])

    def calls(self, controller):
        return [argv for argv, _ in controller.runner.calls if argv[1] == "export"]


class ExportRetryTests(ExportRetryFixture, unittest.TestCase):
    def test_http_retry_route_keeps_session_origin_and_replay_boundaries(self):
        controller, old = self.failed()
        server = backend.Server(controller)
        thread = threading.Thread(target=server.serve_forever)
        thread.start()

        def call(body, token, origin):
            connection = http.client.HTTPConnection(*server.server_address, timeout=3)
            try:
                headers = {"Origin": origin, "Content-Type": "application/json"}
                if token is not None:
                    headers["X-Toolkit-Session"] = token
                connection.request("POST", "/api/retry_export", backend.encoded(body), headers=headers)
                response = connection.getresponse()
                return response.status, json.loads(response.read())
            finally:
                connection.close()

        try:
            request = {"job_id": old["id"]}
            self.assertEqual(call(request, None, server.origin), (403, {"error": "request_not_allowed"}))
            self.assertEqual(call(request, "wrong", server.origin), (403, {"error": "request_not_allowed"}))
            self.assertEqual(call(request, server.capability, "https://foreign.invalid"),
                             (403, {"error": "request_not_allowed"}))
            self.assertEqual(call({**request, "destination": "/tmp"}, server.capability, server.origin),
                             (400, {"error": "invalid_request"}))
            self.assertEqual(len(self.calls(controller)), 1)
            status, response = call(request, server.capability, server.origin)
            self.assertEqual(status, 200)
            identifier = response["job"]["id"]
            controller.join()
            status, replay = call(request, server.capability, server.origin)
            self.assertEqual(status, 200)
            self.assertEqual(replay["job"], controller.jobs[identifier])
            self.assertEqual(replay["job"]["retry_of"], old["id"])
            self.assertEqual(replay["job"]["status"], "succeeded")
            self.assertEqual(len(self.calls(controller)), 2)
        finally:
            server.shutdown()
            server.server_close()
            thread.join(3)
        self.assertFalse(thread.is_alive())

    def test_retry_keeps_old_bytes_observation_and_original_request_mapping(self):
        controller, old = self.failed()
        original = controller.export_intents[old["id"]]["directory"]
        previous = tree(original)
        observation = controller.reconcile({"job_id": old["id"]})["observation"]
        retry = controller.dispatch("/api/retry_export", {"job_id": old["id"]})["job"]
        controller.join()
        result = controller.jobs[retry["id"]]
        self.assertEqual(result["status"], "succeeded")
        self.assertEqual(result["retry_of"], old["id"])
        self.assertEqual(result["request_sha256"], old["request_sha256"])
        self.assertNotEqual(result["result"]["destination"], str(original / "output"))
        self.assertEqual(tree(original), previous)
        self.assertEqual(controller.jobs[old["id"]], old)
        self.assertEqual(controller.observations[old["id"]], observation)
        self.assertEqual(controller.export_recipe(hosted.EXPORT)["job"], old)
        self.assertEqual(controller.retry_export({"job_id": old["id"]})["job"], result)
        self.assertEqual(len(self.calls(controller)), 2)
        self.assertTrue(controller.state()["export_retry"]["available"])
        record = json.loads((controller.workspace / "requests" / (retry["id"] + ".json")).read_bytes())
        self.assertEqual(record["request"], hosted.EXPORT)
        self.assertEqual(record["profile_sha256"], controller.local_recipes_profile_sha256)

    def test_retry_chain_cannot_branch_and_replay_is_only_observation(self):
        controller, old = self.failed()
        controller.runner.mode = "lost_export"
        first = controller.retry_export({"job_id": old["id"]})["job"]
        controller.join()
        first = copy.deepcopy(controller.jobs[first["id"]])
        controller.runner.mode = None
        second = controller.retry_export({"job_id": first["id"]})["job"]
        controller.join()
        self.assertEqual(second["retry_of"], first["id"])
        # Accepted replay survives later file changes and unrelated busy work.
        (controller.workspace / "local-recipes-profile.json").write_bytes(b"changed")
        controller.operation_lock.acquire()
        try:
            self.assertEqual(controller.retry_export({"job_id": old["id"]})["job"], first)
            self.assertEqual(controller.retry_export({"job_id": first["id"]})["job"]["id"], second["id"])
        finally:
            controller.operation_lock.release()
        self.assertEqual(len(self.calls(controller)), 3)
        self.assertEqual(controller.export_recipe(hosted.EXPORT)["job"], old)
        controller.allow_remote_recipes = False
        self.assert_refused(lambda: controller.retry_export({"job_id": old["id"]}), "remote_recipes_unavailable")
        self.assertFalse(controller.state()["export_retry"]["available"])

    def test_new_retry_refuses_busy_without_allocating(self):
        controller, old = self.failed()
        before = tree(controller.workspace)
        controller.operation_lock.acquire()
        try:
            self.assert_refused(lambda: controller.retry_export({"job_id": old["id"]}), "operation_in_progress")
        finally:
            controller.operation_lock.release()
        self.assertEqual(tree(controller.workspace), before)
        self.assertEqual(controller.export_retries, {})
        self.assertEqual(list(controller.jobs), [old["id"]])

    def test_invalid_foreign_succeeded_running_and_wrong_kind_refuse(self):
        controller, old = self.failed()
        for request in ({}, {"job_id": old["id"], "activation_json": "{}"}):
            self.assert_refused(lambda: controller.retry_export(request), "invalid_request")
        foreign = self.controller()
        for identifier in (None, "missing", old["id"]):
            self.assert_refused(lambda: foreign.retry_export({"job_id": identifier}), "unknown_job")
        for key, value in (("status", "running"), ("status", "succeeded"), ("kind", "install"),
                           ("family", "selected-python"), ("recovered", True)):
            controller.jobs[old["id"]] = {**old, key: value}
            self.assert_refused(lambda: controller.retry_export({"job_id": old["id"]}), "export_retry_unavailable")
        controller.jobs[old["id"]] = old
        self.assertEqual(controller.export_retries, {})
        self.assertEqual(len(self.calls(controller)), 1)

    def test_recovery_only_cannot_retry_even_with_opt_in(self):
        controller, old = self.failed()
        recovered = self.controller(recover_workspace=controller.workspace,
            recover_request=old["recovery_request"]["id"],
            recover_request_sha256=old["recovery_request"]["sha256"])
        self.assertTrue(recovered.recovery_only)
        self.assertFalse(recovered.state()["export_retry"]["available"])
        self.assert_refused(lambda: recovered.retry_export({"job_id": old["id"]}), "recovery_read_only")
        self.assertEqual(recovered.runner.calls, [])

    def test_mutated_frozen_request_profile_or_tool_refuses_before_new_files(self):
        for target in ("request", "activation", "record", "profile", "tool"):
            with self.subTest(target=target):
                controller, old = self.failed()
                directory = controller.export_intents[old["id"]]["directory"]
                path = {"request": directory / "request.json", "activation": directory / "activation.json",
                        "record": controller.workspace / "requests" / (old["id"] + ".json"),
                        "profile": controller.workspace / "local-recipes-profile.json",
                        "tool": controller.workspace / "local-recipes-tool"}[target]
                path.write_bytes(b"{}")
                before = tree(controller.workspace)
                self.assert_refused(lambda: controller.retry_export({"job_id": old["id"]}))
                self.assertEqual(tree(controller.workspace), before)
                self.assertEqual(controller.export_retries, {})
                self.assertEqual(len(self.calls(controller)), 1)

    def test_every_sync_failure_preserves_original_and_publishes_no_retry(self):
        for point in range(1, 7):
            with self.subTest(point=point):
                controller, old = self.failed()
                original_tree = tree(controller.export_intents[old["id"]]["directory"])
                original_file, original_directory = backend.sync_file, backend.sync_directory
                seen = []
                def invoke(function, *args):
                    seen.append(args)
                    if len(seen) == point:
                        raise OSError("inert sync failure")
                    return function(*args)
                with patch.object(backend, "sync_file", side_effect=lambda *a: invoke(original_file, *a)), \
                     patch.object(backend, "sync_directory", side_effect=lambda *a: invoke(original_directory, *a)):
                    with self.assertRaises(OSError):
                        controller.retry_export({"job_id": old["id"]})
                self.assertEqual(controller.export_retries, {})
                self.assertEqual(list(controller.jobs), [old["id"]])
                self.assertFalse(controller.operation_lock.locked())
                self.assertEqual(len(self.calls(controller)), 1)
                self.assertEqual(tree(controller.export_intents[old["id"]]["directory"]), original_tree)
                # A new explicit call may try again after an unadmitted reservation.
                retry = controller.retry_export({"job_id": old["id"]})["job"]
                controller.join()
                self.assertEqual(controller.jobs[retry["id"]]["status"], "succeeded")

    def test_worker_observes_fully_synced_recovery_record_and_exact_request(self):
        controller, old = self.failed()
        original_run, original_sync = controller.runner.run, backend.sync_directory
        synced = []
        def sync(path):
            original_sync(path)
            synced.append(path)
        def run(argv, **kwargs):
            if argv[1] == "export":
                identifier = controller.export_retries[old["id"]]
                anchor = controller.jobs[identifier]["recovery_request"]
                record, intent = backend.load_export_recovery(controller.workspace, anchor["id"],
                    anchor["sha256"], controller.local_recipes_profile_sha256)
                self.assertEqual(record["request"], hosted.EXPORT)
                self.assertEqual(intent["request"], hosted.EXPORT)
                self.assertEqual(synced[-2:], [controller.workspace, controller.workspace.parent])
            return original_run(argv, **kwargs)
        with patch.object(backend, "sync_directory", side_effect=sync), patch.object(controller.runner, "run", side_effect=run):
            retry = controller.retry_export({"job_id": old["id"]})["job"]
            controller.join()
        self.assertEqual(controller.jobs[retry["id"]]["status"], "succeeded")

    def test_thread_start_failure_keeps_one_observable_retry(self):
        for after_start in (False, True):
            with self.subTest(after_start=after_start):
                controller, old = self.failed()
                original_start = threading.Thread.start
                def fail(thread):
                    if after_start:
                        original_start(thread)
                    raise RuntimeError("inert thread failure")
                with patch.object(threading.Thread, "start", fail):
                    retry = controller.retry_export({"job_id": old["id"]})["job"]
                controller.join()
                self.assertEqual(retry["status"], "unconfirmed")
                self.assertEqual(controller.retry_export({"job_id": old["id"]})["job"], retry)
                self.assertEqual(controller.export_recipe(hosted.EXPORT)["job"], old)
                self.assertFalse(controller.operation_lock.locked())
                self.assertEqual(len(self.calls(controller)), 1)
                next_job = controller.retry_export({"job_id": retry["id"]})["job"]
                controller.join()
                self.assertEqual(controller.jobs[next_job["id"]]["status"], "succeeded")

    def test_same_call_concurrency_reserves_only_one_successor(self):
        controller, old = self.failed()
        controller.runner.block = threading.Event()
        gate, responses = threading.Barrier(3), []
        def retry():
            gate.wait(5)
            responses.append(controller.retry_export({"job_id": old["id"]})["job"])
        threads = [threading.Thread(target=retry) for _ in range(2)]
        for thread in threads:
            thread.start()
        gate.wait(5)
        for thread in threads:
            thread.join(5)
            self.assertFalse(thread.is_alive())
        self.assertEqual(len(responses), 2)
        self.assertEqual(responses[0]["id"], responses[1]["id"])
        self.assertEqual(controller.export_recipe(hosted.EXPORT)["job"], old)
        controller.stop({"job_id": old["id"]})
        self.assertNotIn("stop", controller.jobs[responses[0]["id"]])
        controller.runner.block.set()
        controller.join()
        self.assertEqual(len(self.calls(controller)), 2)

    def test_stopped_export_child_can_retry_only_after_settlement(self):
        controller = self.retained()
        controller.runner.mode = "sleep"
        old = controller.export_recipe(hosted.EXPORT)["job"]
        marker = controller.workspace / "child.pid"
        deadline = time.monotonic() + 5
        while not marker.exists() and time.monotonic() < deadline:
            time.sleep(0.01)
        self.assertTrue(marker.exists())
        pid = int(marker.read_text())
        self.assert_refused(lambda: controller.retry_export({"job_id": old["id"]}), "export_retry_unavailable")
        controller.stop({"job_id": old["id"]})
        controller.join()
        with self.assertRaises(ProcessLookupError):
            os.kill(pid, 0)
        self.assertEqual(controller.jobs[old["id"]]["stop"]["status"], "settled")
        controller.runner.mode = None
        retry = controller.retry_export({"job_id": old["id"]})["job"]
        controller.join()
        self.assertEqual(controller.jobs[retry["id"]]["status"], "succeeded")


class ExportRetryMcpTests(ExportRetryFixture, unittest.IsolatedAsyncioTestCase):
    async def test_tool_surface_and_current_opt_in(self):
        controller, old = self.failed()
        owner = adapter.SetupAdapter(controller)
        try:
            async with Client(adapter.create_server(owner)) as client:
                tool = next(t for t in (await client.list_tools()).tools if t.name == "setup_retry_export")
                self.assertFalse(tool.annotations.read_only_hint)
                self.assertTrue(tool.annotations.idempotent_hint)
                self.assertFalse(tool.annotations.open_world_hint)
                for request in ({}, {"job_id": "unknown"}, {"job_id": old["id"], "destination": "/tmp"}):
                    self.assertTrue((await client.call_tool("setup_retry_export", request)).is_error)
                result = await client.call_tool("setup_retry_export", {"job_id": old["id"]})
                self.assertFalse(result.is_error, result)
                self.assertEqual(result.structured_content["job"]["retry_of"], old["id"])
                await asyncio.to_thread(controller.join)
                controller.allow_remote_recipes = False
                self.assertTrue((await client.call_tool("setup_retry_export", {"job_id": old["id"]})).is_error)
        finally:
            await owner.aclose()
        disabled = self.controller(allow_remote_recipes=False)
        owner = adapter.SetupAdapter(disabled)
        try:
            async with Client(adapter.create_server(owner)) as client:
                self.assertNotIn("setup_retry_export", {t.name for t in (await client.list_tools()).tools})
        finally:
            await owner.aclose()

    async def test_cancelled_lost_reply_still_observes_one_owned_retry(self):
        controller, old = self.failed()
        controller.runner.block = threading.Event()
        owner = adapter.SetupAdapter(controller)
        entered, release = threading.Event(), threading.Event()
        original = controller.dispatch
        def dispatch(route, arguments):
            result = original(route, arguments)
            if route == "/api/retry_export":
                entered.set()
                if not release.wait(5):
                    raise AssertionError("retry reply not released")
            return result
        try:
            async with Client(adapter.create_server(owner)) as client:
                with patch.object(controller, "dispatch", side_effect=dispatch):
                    pending = asyncio.create_task(client.call_tool("setup_retry_export", {"job_id": old["id"]}))
                    self.assertTrue(await asyncio.to_thread(entered.wait, 5))
                    pending.cancel()
                    release.set()
                    with self.assertRaises(asyncio.CancelledError):
                        await pending
                    async with asyncio.timeout(5):
                        while owner._inflight:
                            await asyncio.sleep(0.01)
                identifier = controller.export_retries[old["id"]]
                self.assertEqual(controller.jobs[identifier]["status"], "running")
                self.assertNotIn("stop", controller.jobs[identifier])
                result = await client.call_tool("setup_retry_export", {"job_id": old["id"]})
                self.assertEqual(result.structured_content["job"]["id"], identifier)
                controller.runner.block.set()
                await asyncio.to_thread(controller.join)
                self.assertEqual(controller.jobs[identifier]["status"], "succeeded")
                self.assertEqual(len(self.calls(controller)), 2)
                self.assertEqual(controller.export_recipe(hosted.EXPORT)["job"], old)
        finally:
            release.set()
            controller.runner.block.set()
            await owner.aclose()


if __name__ == "__main__":
    unittest.main()
