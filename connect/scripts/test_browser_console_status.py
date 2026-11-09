"""Inert controls for the real Browser observer, not browser/native acceptance."""
import asyncio
import contextlib
import dataclasses
import importlib.util
import json
from pathlib import Path
import tempfile
from types import SimpleNamespace
import unittest
from unittest import mock


SPEC = importlib.util.spec_from_file_location(
    "browser_status_subject", Path(__file__).with_name("verify_installed_console.py"))
C = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(C)
UNAVAILABLE = {"error": "Protected setup is unavailable"}


def observation(status=200, *, method="GET", suffix="/data", value=None):
    return {"method": method, "path": C.PANEL + suffix,
            "request": {} if method == "POST" else None, "status": status,
            "response": value if value is not None else
            ({"review": None} if status == 200 else UNAVAILABLE)}


@dataclasses.dataclass
class Input:
    channel: str = "host-authenticated-console"
    principal: str = C.EMAIL
    identity: str = "original"
    content: str = "fixture input"
    interaction_id: str = "interaction"
    session_id: str = "session"
    input_frame_id: str = "frame"
    cursor: str = "console:1"
    conversation_id: str = "conversation"


@dataclasses.dataclass
class Retirement:
    fenced: bool = True
    reservations_drained: bool = True
    uncertain: bool = False


class ObserverHarness:
    """Supply transport observations; replace unrelated journey qualification."""

    def __init__(self, directory, batches, late):
        self.directory, self.batches, self.late = directory, list(batches), late
        self.clock, self.reads, self.closes = 0, 0, 0
        self.retirements = 0
        self.close_error = self.retire_error = None
        self.retirement = Retirement()
        self.events = mock.Mock()
        self.process = SimpleNamespace(poll=lambda: None)
        self.provider = SimpleNamespace(lock=contextlib.nullcontext(), requests=[],
            errors=[], results={}, business_observation=None, enabled=False,
            finished=SimpleNamespace(is_set=lambda: True))
        self.client = SimpleNamespace(event_hooks={"response": []})
        self.front = SimpleNamespace(application=self.application, client=self.client,
            start=self.start, close=self.close, failures=[], key=None,
            task=SimpleNamespace(done=lambda: False))
        self.scenario = object.__new__(C.BrowserConsoleScenario)
        self.scenario.v = SimpleNamespace(ACQUISITION_LIMIT=3, save_json=self.save,
            unique_json=json.loads, exact_bytes=lambda path: path.read_bytes())
        self.scenario.front = self.front
        self.scenario.origin = "http://127.0.0.1:12345"
        self.scenario.password = "public-fixture"
        self.scenario.source_receipt, self.scenario.forge_sources = {}, []
        self.scenario.auth = SimpleNamespace(ISSUER="fixture")
        self.scenario.login_module = SimpleNamespace(
            NativeConsoleLogin=lambda *args, **kwargs: SimpleNamespace(close=self.retire))
        self.scenario.original_request = lambda: "fixture input"
        # This suite tests status-observer acceptance only. Actual timeline,
        # approval and native qualification are deliberately not simulated.
        self.scenario.completed = lambda *args: {"fixture_only": True}
        self.scenario.finish_report = self.finish
        self.finished_at = []
        self.save(directory / "browser-release.json", {"finish": True})

    @staticmethod
    def save(path, value):
        path.write_text(json.dumps(value))

    def finish(self, report, results, provider, completion):
        self.finished_at.append(self.clock)
        report.update(status="passed", completion=completion)

    async def retire(self):
        self.retirements += 1
        if self.retire_error is not None:
            raise self.retire_error
        return self.retirement

    def application(self, _native_url):
        async def app(scope, receive, send):
            await send({"type": "http.response.start", "status": scope["fixture_status"]})
        return app

    async def emit(self, rows):
        for row in rows:
            async def read(value=row["response"]):
                self.reads += 1
                return json.dumps(value).encode()
            response = SimpleNamespace(status_code=row["status"], aread=read,
                request=SimpleNamespace(url=SimpleNamespace(path=row["path"]),
                    method=row["method"], content=json.dumps(row["request"]).encode()))
            for observer in self.client.event_hooks["response"]:
                await observer(response)

    async def start(self, native_url):
        app = self.front.application(native_url)
        self.front.console_login_factory(object())
        self.front.on_input(Input())
        # Satisfy unrelated front-route guards with inert ASGI responses.
        routes = [("GET", "/login", 200), ("POST", "/login", 303),
                  ("GET", C.PANEL, 200), ("POST", C.PANEL + "/data", 200),
                  ("POST", C.PANEL + "/answer", 202),
                  ("POST", C.PANEL + "/answer", 202),
                  ("POST", C.PANEL + "/start", 202)]
        async def discard(_message):
            pass
        for method, path, status in routes:
            await app({"type": "http", "method": method, "path": path,
                       "fixture_status": status}, None, discard)
        await self.emit(self.batches.pop(0) if self.batches else [])

    async def sleep(self, _duration):
        self.clock += 1
        await self.emit(self.batches.pop(0) if self.batches else [])

    async def close(self):
        self.closes += 1
        await self.emit(self.late)
        if self.close_error is not None:
            raise self.close_error

    def run(self):
        error = None
        with mock.patch.object(C.time, "monotonic", side_effect=lambda: self.clock), \
                mock.patch.object(C.asyncio, "sleep", self.sleep), \
                mock.patch.object(C.socket, "socket", side_effect=AssertionError("no sockets")), \
                mock.patch.object(C, "HTTPConnection", side_effect=AssertionError("no HTTP")):
            work = self.scenario.run(None, self.provider, self.directory, self.events,
                                     self.process, "http://127.0.0.1:54321")
            try:
                work.send(None)
            except StopIteration:
                pass
            except BaseException as caught:
                error = caught
            finally:
                work.close()
        return json.loads((self.directory / "console-journey.json").read_text()), error


class BrowserStatusTests(unittest.TestCase):
    def exercise(self, batches, *, late=()):
        with tempfile.TemporaryDirectory(prefix="inert-browser-status-") as scratch:
            harness = ObserverHarness(Path(scratch), batches, late)
            report, error = harness.run()
        self.assertEqual(harness.closes, 1)
        self.assertEqual(report["decisions"], [])
        self.assertEqual(report["starts"], [])
        self.assertEqual(harness.provider.requests, [])
        self.assertEqual(harness.client.__dict__.keys(), {"event_hooks"})
        return harness, report, error

    def test_get_and_post_unavailable_wait_for_later_success(self):
        for method in ("GET", "POST"):
            with self.subTest(method=method):
                pending, ready = observation(503, method=method), observation()
                harness, report, error = self.exercise([[pending], [ready]])
                self.assertIsNone(error)
                self.assertEqual(report["status"], "passed")
                self.assertEqual(report["http"], [pending, ready])
                self.assertEqual(harness.finished_at, [1])
                self.assertEqual(harness.reads, 2)
                harness.events.add.assert_called_once()

    def test_earlier_success_does_not_clear_later_unavailable(self):
        ready, pending = observation(), observation(503)
        harness, report, error = self.exercise([[ready, pending]])
        self.assertRegex(str(error), "did not complete and release")
        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["http"], [ready, pending])
        self.assertEqual(harness.finished_at, [])
        self.assertEqual(harness.reads, 2)
        harness.events.add.assert_not_called()

    def test_other_failures_stay_strict_and_retained(self):
        cases = [
            observation(503, value={"error": "different"}),
            observation(503, value={**UNAVAILABLE, "extra": True}),
            observation(403), observation(500),
            observation(503, method="DELETE"),
            observation(503, method="POST", suffix="/answer"),
            observation(503, method="POST", suffix="/start"),
        ]
        for row in cases:
            with self.subTest(row=row):
                harness, report, error = self.exercise([[row]])
                self.assertIsNotNone(error)
                self.assertEqual(report["status"], "failed")
                self.assertEqual(report["http"], [row])
                self.assertEqual(harness.finished_at, [])
                harness.events.add.assert_not_called()

    def test_late_unavailable_invalidates_apparent_completion(self):
        ready, pending = observation(), observation(503, method="POST")
        harness, report, error = self.exercise([[ready]], late=[pending])
        self.assertRegex(str(error), "remained unavailable after completion")
        self.assertEqual(harness.finished_at, [0])
        self.assertEqual(report["status"], "failed")
        self.assertEqual(report["cleanup_failure_phase"], "post_close_observation")
        self.assertEqual(report["http"], [ready, pending])
        harness.events.add.assert_not_called()


class BrowserCleanupTests(unittest.TestCase):
    def test_original_failure_survives_front_close_failure_and_login_is_observed(self):
        with tempfile.TemporaryDirectory(prefix="inert-browser-cleanup-") as scratch:
            harness = ObserverHarness(Path(scratch), [[observation(403)]], [])
            harness.close_error = RuntimeError("private exception payload must not be retained")
            report, error = harness.run()
        self.assertRegex(str(error), "observed public Browser request failed")
        self.assertEqual((harness.closes, harness.retirements), (1, 1))
        self.assertEqual(report["login_retirement"], [dataclasses.asdict(Retirement())])
        self.assertEqual(report["cleanup_failure_phase"], "front_close")
        self.assertEqual(report["cleanup_failure_category"], "other_error")
        self.assertFalse(report["front_joined"])
        self.assertEqual(report["status"], "failed")
        self.assertNotIn("private exception payload", json.dumps(report))
        harness.events.add.assert_not_called()

    def test_front_close_failure_is_not_masked_by_successful_login_retirement(self):
        with tempfile.TemporaryDirectory(prefix="inert-browser-cleanup-") as scratch:
            harness = ObserverHarness(Path(scratch), [[observation()]], [])
            failure = RuntimeError("Console access cleanup is unconfirmed.")
            harness.close_error = failure
            report, error = harness.run()
        self.assertIs(error, failure)
        self.assertEqual((harness.closes, harness.retirements), (1, 1))
        self.assertEqual(report["cleanup_failure_category"], "access_cleanup_unconfirmed")
        self.assertFalse(report["front_joined"])
        self.assertEqual(report["status"], "failed")
        harness.events.add.assert_not_called()

    def test_failed_login_does_not_skip_other_retained_login_results(self):
        class Refusal(RuntimeError):
            code = "cleanup_unconfirmed"
        with tempfile.TemporaryDirectory(prefix="inert-browser-cleanup-") as scratch:
            harness = ObserverHarness(Path(scratch), [[observation(403)]], [])
            failure = Refusal("private enrollment details")
            first_calls = []
            async def first_close():
                first_calls.append("closed")
                raise failure
            owners = iter((SimpleNamespace(close=first_close), SimpleNamespace(close=harness.retire)))
            harness.scenario.login_module.NativeConsoleLogin = lambda *args, **kwargs: next(owners)
            original_start = harness.start
            async def start(native_url):
                await original_start(native_url)
                harness.front.console_login_factory(object())
            harness.front.start = start
            report, error = harness.run()
        self.assertRegex(str(error), "observed public Browser request failed")
        self.assertEqual(first_calls, ["closed"])
        self.assertEqual(harness.retirements, 1)
        self.assertEqual(report["login_retirement"], [dataclasses.asdict(Retirement())])
        self.assertEqual(report["login_retirement_failures"], {"native_login_cleanup_unconfirmed": 1})
        self.assertEqual(report["cleanup_failure_phase"], "login_retirement")
        self.assertFalse(report["front_joined"])
        self.assertNotIn("private enrollment", json.dumps(report))
        harness.events.add.assert_not_called()

    def test_uncertain_retirement_is_retained_and_remains_failure(self):
        with tempfile.TemporaryDirectory(prefix="inert-browser-cleanup-") as scratch:
            harness = ObserverHarness(Path(scratch), [[observation()]], [])
            harness.retirement = Retirement(uncertain=True)
            report, error = harness.run()
        self.assertRegex(str(error), "shipping login retirement unconfirmed")
        self.assertEqual(report["login_retirement"], [dataclasses.asdict(Retirement(uncertain=True))])
        self.assertEqual(report["login_retirement_failures"], {"assertion_failed": 1})
        self.assertFalse(report["front_joined"])
        self.assertEqual(report["status"], "failed")
        harness.events.add.assert_not_called()

    def test_front_cancellation_stays_cancelled_after_retained_login_observation(self):
        with tempfile.TemporaryDirectory(prefix="inert-browser-cleanup-") as scratch:
            harness = ObserverHarness(Path(scratch), [[observation()]], [])
            failure = asyncio.CancelledError()
            harness.close_error = failure
            report, error = harness.run()
        self.assertIs(error, failure)
        self.assertEqual(harness.retirements, 1)
        self.assertEqual(report["cleanup_failure_category"], "cancelled")
        self.assertFalse(report["front_joined"])
        harness.events.add.assert_not_called()

    def test_child_snapshot_observes_original_tasks_without_reentry(self):
        async def check():
            calls = []
            async def succeeded():
                calls.append("succeeded")
            async def failed():
                calls.append("failed")
                raise RuntimeError("Console upstream cleanup is unconfirmed.")
            async def pending():
                await asyncio.Event().wait()
            good, bad, cancelled, waiting = (
                asyncio.create_task(operation(), name="private task identifier")
                for operation in (succeeded, failed, pending, pending))
            try:
                cancelled.cancel()
                await asyncio.gather(good, bad, cancelled, return_exceptions=True)
                front = SimpleNamespace(_close_task=bad, task=waiting)
                login = SimpleNamespace(retirement_task=bad, console_login_close=good)
                subscription = SimpleNamespace(task=cancelled, close_task=bad)
                rows = C.BrowserConsoleScenario.cleanup_children(front, [login], [subscription])
                self.assertEqual(rows, {
                    "front_close": {"upstream_cleanup_unconfirmed": 1},
                    "server": {"pending": 1},
                    "retirement_task": {"upstream_cleanup_unconfirmed": 1},
                    "console_login_close": {"succeeded": 1},
                    "subscription": {"cancelled": 1},
                    "subscription_close": {"upstream_cleanup_unconfirmed": 1}})
                self.assertEqual(calls, ["succeeded", "failed"])
                self.assertFalse(waiting.done())
                self.assertNotIn("private", json.dumps(rows))
            finally:
                waiting.cancel()
                await asyncio.gather(waiting, return_exceptions=True)
        asyncio.run(check())


if __name__ == "__main__":
    unittest.main()
