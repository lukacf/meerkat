"""Explicit preparation retries with package-owner fixtures and the real MCP SDK."""
import asyncio
import copy
import json
from pathlib import Path
import threading
import unittest
from unittest.mock import patch

from test_mcp_adapter import adapter, connection, eventually, fixtures
import test_official_github as github
import test_official_playwright as playwright

backend = adapter.backend


def tree(root):
    return {str(path.relative_to(root)): path.read_bytes()
            for path in root.rglob("*") if path.is_file()}


class PreparationRetryTests(fixtures.FixtureMixin, unittest.TestCase):
    def failed(self, controller, family="agno"):
        controller.runner.nonzero_after_publish = True
        job = controller.prepare({"family": family})["job"]
        controller.join()
        self.assertEqual(controller.jobs[job["id"]]["status"], "unconfirmed")
        controller.runner.nonzero_after_publish = False
        return copy.deepcopy(controller.jobs[job["id"]])

    def test_all_library_families_keep_old_attempt_and_publish_new_command(self):
        for family in backend.FAMILIES:
            with self.subTest(family=family):
                controller = self.controller()
                old = self.failed(controller, family)
                original = controller.workspace / "inspection" / family
                retained = tree(original)
                self.assertNotIn(family, controller.commands)
                self.assertEqual(controller.family(family)["profiles"], [])
                retry = controller.retry_prepare({"job_id": old["id"]})["job"]
                controller.join()
                self.assertEqual(controller.jobs[retry["id"]]["status"], "succeeded")
                self.assertEqual(retry["retry_of"], old["id"])
                self.assertEqual(controller.jobs[old["id"]], old)
                self.assertEqual(tree(original), retained)
                destination = controller.workspace / "inspection" / (family + "-" + retry["id"])
                self.assertTrue((destination / "installed.json").is_file())
                command = controller.expected_command(family, destination)
                self.assertEqual(controller.commands[family], command)
                self.assertEqual(controller.family(family)["preparation_job_id"], retry["id"])
                profile = controller.family(family)["profiles"][0]["id"]
                controller.help({"family": family, "profile": profile})
                self.assertEqual(controller.runner.calls[-1][0], command + ["config-help", "--profile", profile])

    def test_receipt_and_profile_failures_never_publish_candidate(self):
        for boundary in ("receipt", "profile_exit", "profile_json", "profile_shape"):
            with self.subTest(boundary=boundary):
                controller = self.controller()
                original = controller.runner.run

                def run(argv, capture=False):
                    code, data = original(argv, capture=capture)
                    if "--destination" in argv and boundary == "receipt":
                        destination = Path(argv[argv.index("--destination") + 1])
                        (destination / "installed.json").write_bytes(b"{}")
                    if argv[-1:] == ["profiles"]:
                        if boundary == "profile_exit":
                            return 2, data
                        if boundary == "profile_json":
                            return 0, b"not JSON"
                        if boundary == "profile_shape":
                            return 0, b'{"profiles":[]}'
                    return code, data

                with patch.object(controller.runner, "run", side_effect=run):
                    old = controller.prepare({"family": "agno"})["job"]
                    controller.join()
                self.assertEqual(controller.jobs[old["id"]]["status"], "unconfirmed")
                self.assertEqual(controller.commands, {})
                self.assertEqual(controller.family("agno")["profiles"], [])
                controller.retry_prepare({"job_id": old["id"]})
                controller.join()
                self.assertEqual(controller.family("agno")["status"], "prepared")

    def test_failed_retry_requires_its_own_id_and_old_replays_are_stable(self):
        controller = self.controller()
        old = self.failed(controller)
        controller.runner.nonzero_after_publish = True
        first = controller.retry_prepare({"job_id": old["id"]})["job"]
        controller.join()
        first = copy.deepcopy(controller.jobs[first["id"]])
        calls = copy.deepcopy(controller.runner.calls)
        self.assertEqual(controller.retry_prepare({"job_id": old["id"]})["job"], first)
        self.assertEqual(controller.runner.calls, calls)
        controller.runner.nonzero_after_publish = False
        second = controller.retry_prepare({"job_id": first["id"]})["job"]
        controller.join()
        self.assertEqual(second["retry_of"], first["id"])
        self.assertNotEqual(second["id"], first["id"])
        self.assertEqual(controller.retry_prepare({"job_id": old["id"]})["job"], first)
        self.assertEqual(controller.jobs[old["id"]], old)
        self.assertEqual(controller.family("agno")["preparation_job_id"], second["id"])
        self.assertEqual(len([a for a, _ in controller.runner.calls if "--destination" in a]), 3)

    def test_thread_start_failure_keeps_reserved_retry_observable_and_replayable(self):
        controller = self.controller()
        old = self.failed(controller)
        calls = copy.deepcopy(controller.runner.calls)
        with patch.object(backend.threading.Thread, "start", side_effect=RuntimeError("fixture thread failure")):
            retry = controller.retry_prepare({"job_id": old["id"]})["job"]
        self.assertEqual(retry["status"], "unconfirmed")
        self.assertEqual(retry["retry_of"], old["id"])
        self.assertFalse(controller.operation_lock.locked())
        self.assertEqual(controller.retry_prepare({"job_id": old["id"]})["job"], retry)
        self.assertEqual(controller.runner.calls, calls)
        next_attempt = controller.retry_prepare({"job_id": retry["id"]})["job"]
        controller.join()
        self.assertEqual(controller.jobs[next_attempt["id"]]["status"], "succeeded")

    def test_operation_lock_refusal_does_not_reserve_retry(self):
        controller = self.controller()
        old = self.failed(controller)
        before = copy.deepcopy(controller.state())
        controller.operation_lock.acquire()
        try:
            self.error("operation_in_progress", lambda: controller.retry_prepare({"job_id": old["id"]}))
            self.assertEqual(controller.preparation_retries, {})
        finally:
            controller.operation_lock.release()
        self.assertEqual(controller.state(), before)
        retry = controller.retry_prepare({"job_id": old["id"]})["job"]
        controller.join()
        self.assertEqual(controller.jobs[retry["id"]]["status"], "succeeded")

    def test_exact_failed_current_preparation_is_required_before_any_effect(self):
        controller = self.controller()
        old = self.failed(controller)
        before = copy.deepcopy(controller.runner.calls)
        for invalid in (None, [], "missing", "f" * 32):
            self.error("unknown_job", lambda: controller.retry_prepare({"job_id": invalid}))
        state = controller.family("agno")
        with patch.dict(state, preparation_job_id="f" * 32):
            self.error("preparation_superseded", lambda: controller.retry_prepare({"job_id": old["id"]}))
        with patch.dict(state, status="prepared"):
            self.error("preparation_retry_unavailable", lambda: controller.retry_prepare({"job_id": old["id"]}))
        for status in ("running", "succeeded"):
            with patch.dict(controller.jobs[old["id"]], status=status):
                self.error("preparation_retry_unavailable", lambda: controller.retry_prepare({"job_id": old["id"]}))
        with patch.dict(controller.jobs[old["id"]], kind="install"):
            self.error("preparation_retry_unavailable", lambda: controller.retry_prepare({"job_id": old["id"]}))
        self.assertEqual(controller.runner.calls, before)
        self.assertEqual(controller.preparation_retries, {})
        self.assertEqual(controller.jobs[old["id"]], old)

    def test_concurrent_requests_reserve_one_retry_and_one_installer(self):
        controller = self.controller()
        old = self.failed(controller)
        controller.runner.block = threading.Event()
        barrier = threading.Barrier(3)
        replies, errors = [], []

        def request():
            barrier.wait(5)
            try:
                replies.append(controller.retry_prepare({"job_id": old["id"]})["job"])
            except Exception as error:
                errors.append(error)

        threads = [threading.Thread(target=request) for _ in range(2)]
        for thread in threads:
            thread.start()
        barrier.wait(5)
        for thread in threads:
            thread.join(5)
            self.assertFalse(thread.is_alive())
        self.assertEqual(errors, [])
        self.assertEqual(len(replies), 2)
        self.assertEqual(replies[0]["id"], replies[1]["id"])
        controller.runner.block.set()
        controller.join()
        self.assertEqual(len([a for a, _ in controller.runner.calls if "--destination" in a]), 2)

    def test_retry_unlocks_normal_check_and_install_without_reusing_inspection(self):
        controller = self.controller()
        old = self.failed(controller)
        retry = controller.retry_prepare({"job_id": old["id"]})["job"]
        controller.join()
        request = self.checked(controller)
        installed = controller.install(request)["job"]
        controller.join()
        self.assertEqual(controller.jobs[installed["id"]]["status"], "succeeded")
        before = copy.deepcopy(controller.runner.calls)
        self.assertEqual(controller.retry_prepare({"job_id": old["id"]})["job"]["id"], retry["id"])
        self.error("preparation_retry_unavailable", lambda: controller.retry_prepare({"job_id": installed["id"]}))
        self.assertEqual(controller.runner.calls, before)


class OfficialRetry:
    def test_metadata_retry_stays_metadata_only(self):
        controller = self.controller()
        family = next(iter(controller.families))
        original = controller.runner.run

        def fail_after_description(argv, capture=False):
            original(argv, capture=capture)
            return 2, b""

        with patch.object(controller.runner, "run", side_effect=fail_after_description):
            first = controller.prepare({"family": family})["job"]
            controller.join()
        old = copy.deepcopy(controller.jobs[first["id"]])
        self.assertEqual(old["status"], "unconfirmed")
        self.assertEqual(controller.commands, {})
        self.assertEqual(controller.family(family)["profiles"], [])
        retry = controller.retry_prepare({"job_id": first["id"]})["job"]
        controller.join()
        result = controller.jobs[retry["id"]]
        self.assertEqual(result["status"], "succeeded")
        self.assertEqual(result["result"]["scope"], "official_metadata_only")
        self.assertEqual(controller.jobs[first["id"]], old)
        self.assertEqual(controller.commands, {})
        self.assertEqual(list((controller.workspace / "inspection").iterdir()), [])
        self.assertTrue(all("--official-describe" in argv for argv in controller.runner.calls))
        self.assertFalse(any("--destination" in argv for argv in controller.runner.calls))
        if family == backend.PLAYWRIGHT_FAMILY:
            self.assertEqual(controller.runner.npm_calls, [])
            self.assertEqual(list(self.parent.iterdir()), [])


class GithubPreparationRetryTests(OfficialRetry, github.Fixture, unittest.TestCase):
    pass


class PlaywrightPreparationRetryTests(OfficialRetry, playwright.Fixture, unittest.TestCase):
    pass


class RetryMcpTests(fixtures.FixtureMixin, unittest.IsolatedAsyncioTestCase):
    async def test_lost_mcp_reply_keeps_one_retry_without_implicit_stop(self):
        controller = self.controller(families=("agno",))
        controller.runner.nonzero_after_publish = True
        initial = controller.prepare({"family": "agno"})["job"]
        await asyncio.to_thread(controller.join)
        controller.runner.nonzero_after_publish = False
        controller.runner.block = threading.Event()
        entered, release = threading.Event(), threading.Event()
        original = controller.dispatch

        def delayed_reply(route, arguments):
            result = original(route, arguments)
            if route == "/api/retry_prepare":
                entered.set()
                if not release.wait(5):
                    raise AssertionError("missing reply release")
            return result

        try:
            async with connection(controller) as (client, owner):
                with patch.object(controller, "dispatch", side_effect=delayed_reply):
                    request = asyncio.create_task(client.call_tool("setup_retry_prepare", {"job_id": initial["id"]}))
                    self.assertTrue(await asyncio.to_thread(entered.wait, 5))
                    request.cancel()
                    release.set()
                    with self.assertRaises(asyncio.CancelledError):
                        await request
                    await eventually(lambda: not owner._inflight)
                retry_id = controller.family("agno")["preparation_job_id"]
                self.assertNotEqual(retry_id, initial["id"])
                observed = await client.call_tool("setup_job", {"job_id": retry_id})
                self.assertFalse(observed.is_error)
                self.assertEqual(observed.structured_content["job"]["status"], "running")
                self.assertNotIn("stop", observed.structured_content["job"])
                replay = await client.call_tool("setup_retry_prepare", {"job_id": initial["id"]})
                self.assertFalse(replay.is_error)
                self.assertEqual(replay.structured_content["job"]["id"], retry_id)
                controller.runner.block.set()
                await asyncio.to_thread(controller.join)
                self.assertEqual(controller.jobs[retry_id]["status"], "succeeded")
                self.assertEqual(len([a for a, _ in controller.runner.calls if "--destination" in a]), 2)
        finally:
            release.set()
            controller.runner.block.set()

    async def test_real_mcp_retry_replay_and_following_install(self):
        controller = self.controller(families=("agno",))
        async with connection(controller) as (client, _):
            listed = {tool.name: tool for tool in (await client.list_tools()).tools}
            tool = listed["setup_retry_prepare"]
            self.assertEqual(tool.input_schema["required"], ["job_id"])
            self.assertEqual(set(tool.input_schema["properties"]), {"job_id"})
            self.assertFalse(tool.input_schema["additionalProperties"])
            self.assertTrue(tool.annotations.idempotent_hint)
            self.assertFalse(tool.annotations.read_only_hint)
            self.assertTrue(tool.annotations.open_world_hint)

            async def call(name, value):
                result = await client.call_tool(name, value)
                self.assertFalse(result.is_error, result)
                self.assertEqual(json.loads(result.content[0].text), result.structured_content)
                return result.structured_content

            controller.runner.nonzero_after_publish = True
            initial = (await call("setup_prepare", {"family": "agno"}))["job"]
            await asyncio.to_thread(controller.join)
            controller.runner.nonzero_after_publish = False
            retry = (await call("setup_retry_prepare", {"job_id": initial["id"]}))["job"]
            self.assertEqual(retry["retry_of"], initial["id"])
            self.assertNotEqual(retry["id"], initial["id"])
            await asyncio.to_thread(controller.join)
            observed = (await call("setup_job", {"job_id": retry["id"]}))["job"]
            self.assertEqual(observed["status"], "succeeded")
            calls = copy.deepcopy(controller.runner.calls)
            self.assertEqual((await call("setup_retry_prepare", {"job_id": initial["id"]}))["job"], observed)
            self.assertEqual(controller.runner.calls, calls)
            state = await call("setup_state", {})
            self.assertTrue(state["preparation_retry"]["available"])
            self.assertEqual(state["families"][0]["preparation_job_id"], retry["id"])
            rejected = await client.call_tool("setup_retry_prepare", {"job_id": initial["id"], "family": "agno"})
            self.assertTrue(rejected.is_error)
            await call("setup_help", {"family": "agno", "profile": "searxng"})
            checked = await call("setup_check", self.request())
            request = {**self.request(), "validation_id": checked["validation_id"],
                       "configuration_sha256": checked["configuration_sha256"]}
            installed = (await call("setup_install", request))["job"]
            await asyncio.to_thread(controller.join)
            self.assertEqual((await call("setup_job", {"job_id": installed["id"]}))["job"]["status"], "succeeded")


if __name__ == "__main__":
    unittest.main()
