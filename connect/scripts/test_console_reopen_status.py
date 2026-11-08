"""Installed reopen must not mistake a completed failure notice for health."""
import importlib.util
import copy
import json
from pathlib import Path
from types import SimpleNamespace
import unittest


spec = importlib.util.spec_from_file_location(
    "reopen_console", Path(__file__).with_name("verify_installed_console.py"))
console = importlib.util.module_from_spec(spec)
spec.loader.exec_module(console)
check = console.ReopenConsoleScenario.require_no_setup_startup_failure


def notice(server="toolkit-setup", phase="failed", settled=True):
    # Typed fields from the actual failed Restore timeline. The outer frame
    # was completed even though its required MCP server never started.
    return {"kind": "system_notice", "status": "completed", "payload": {
        "kind": "mcp", "blocks": [{"type": "mcp", "server_id": server,
            "operation": "add", "phase": phase, "settled": settled}]}}


class ReopenStartupStatusTests(unittest.TestCase):
    def test_completed_frame_and_settled_attempt_do_not_hide_setup_failure(self):
        for settled in (True, False):
            with self.subTest(settled=settled), self.assertRaisesRegex(
                    AssertionError, "required setup helper failed MCP startup"):
                check([notice(settled=settled)])

    def test_unrelated_notice_does_not_claim_required_setup_failure(self):
        check([notice(server="unrelated"), {"payload": {"kind": "warning"}}])

    def test_no_failure_notice_is_only_a_negative_check(self):
        check([])
        check([notice(phase="ready")])

    def test_malformed_mcp_notice_is_not_silently_ignored(self):
        for blocks in (None, {}, [None]):
            with self.subTest(blocks=blocks), self.assertRaises(AssertionError):
                check([{"payload": {"kind": "mcp", "blocks": blocks}}])


class ReopenNativeExecutionTests(unittest.TestCase):
    def setUp(self):
        self.accepted = {"identity": "original", "session_id": "original-session",
            "interaction_id": "send-reservation", "input_frame_id": "send"}
        self.helper_session = "restored-helper-session"
        self.find = {"operation": "actual-acquisition", "created": True, "worker": {
            "state": "input_admitted", "deduplicated": False,
            "source_settlement": "fresh_acceptance_promoted"}}
        self.result = {"schema_version": 1, "scope": "supplied_schema_evidence_only",
            "schema_digest": console.REOPEN_DIGEST, "bytes": 2, "hash_algorithm": "sha256",
            "byte_convention": "exact_supplied_utf8", "runtime_observation": "not_established",
            "schema_validation": "not_performed", "notice": "Supplied bytes only."}
        self.verifier = SimpleNamespace(unique_json=lambda raw: json.loads(raw))

        def event(identifier, kind, payload, *, identity="helper", session=False):
            # Native tool events carry run lineage. Only the native start and
            # completion provide the session to which this run belongs.
            run = identity + "-run"
            frame = {"id": identifier, "source_event_id": identifier, "kind": kind,
                "source": {"kind": "console_event"}, "identity": identity, "run_id": run,
                "interaction_id": None, "payload": {"identity": {"run_id": run,
                    "interaction_id": None}, **payload}}
            if session:
                selected = self.accepted["session_id"] if identity == "original" else self.helper_session
                frame["session_id"] = frame["payload"]["session_id"] = selected
            if kind == "interaction_complete":
                frame["status"] = "completed"
            return frame

        self.frames = [{"id": "send", "kind": "user_input", "source": {"kind": "send"},
                **self.accepted, "status": "accepted", "payload": {"content": console.REOPEN_REQUEST}},
            event("original-start", "run_started", {"source_event_type": "run_started",
                "input": {"kind": "content", "content": console.REOPEN_REQUEST}}, identity="original", session=True),
            event("find-call", "tool_call_requested", {"id": "original-request", "name": "find_and_install",
                "args": {"request": console.REOPEN_REQUEST, "user_context": [], "caller_observations": []}},
                identity="original"),
            event("find-result", "tool_result_received", {"id": "original-request", "name": "find_and_install",
                "is_error": False, "content": [{"type": "text", "text": json.dumps(self.find)}]}, identity="original"),
            event("original-done", "interaction_complete", {"source_event_type": "run_completed",
                "result": console.REOPEN_ORIGINAL_ANSWER, "assistant_message_id": "original-answer"},
                identity="original", session=True),
            event("start", "run_started", {"source_event_type": "run_started",
                "input": {"kind": "content", "content": json.dumps({"request": console.REOPEN_REQUEST,
                    "user_context": [], "caller_observations": []})}}, session=True),
            event("call", "tool_call_requested", {"id": console.REOPEN_CALL,
                "name": console.REOPEN_TOOL, "args": {"schema_json": "{}"}}),
            event("result", "tool_result_received", {"id": console.REOPEN_CALL,
                "name": console.REOPEN_TOOL, "is_error": False,
                "content": [{"type": "structured", "data": copy.deepcopy(self.result)}]}),
            event("done", "interaction_complete", {"source_event_type": "run_completed",
                "result": console.REOPEN_ANSWER, "assistant_message_id": "committed-answer"}, session=True)]

    def frame(self, identifier):
        return next(frame for frame in self.frames if frame["id"] == identifier)

    def completion(self, frames=None):
        return console.completed_reopen_tool(self.frames if frames is None else frames,
            self.accepted, self.result, self.verifier, self.helper_session)

    def test_requires_original_find_and_actual_restored_helper_tool_and_answer(self):
        result = self.completion()
        self.assertEqual(result["session_id"], self.helper_session)
        self.assertEqual(result["original_session_id"], "original-session")
        self.assertEqual(result["find_result"], self.find)
        self.assertEqual(result["tool_result_frame"], "result")
        for omitted in range(len(self.frames)):
            with self.subTest(omitted=omitted):
                self.assertIsNone(self.completion(self.frames[:omitted] + self.frames[omitted + 1:]))

    def test_native_success_requires_the_model_to_receive_its_result(self):
        self.assertIsNone(console.completed_reopen_tool(
            self.frames, self.accepted, None, self.verifier, self.helper_session))
        self.result = {**self.result, "notice": "A different result."}
        with self.assertRaisesRegex(AssertionError, "actual native result"):
            self.completion()

    def test_identical_snapshot_update_is_one_event_but_conflicting_bytes_are_refused(self):
        update = {"kind": "frame_updated", "payload": {"frame": copy.deepcopy(self.frames[0])}}
        self.frames.insert(1, console.frame_record(update))
        self.assertEqual(self.completion()["input_frame_id"], "send")
        self.frames[1]["status"] = "delivery_failed"
        with self.assertRaisesRegex(AssertionError, "conflicting restored native frame"):
            self.completion()

    def test_delivery_failure_is_not_masked_by_its_snapshot_update(self):
        self.frames[0]["status"] = "delivery_failed"
        self.frames.insert(1, copy.deepcopy(self.frames[0]))
        with self.assertRaisesRegex(AssertionError, "Send failed native delivery"):
            self.completion()

    def test_false_tool_success_or_wrong_schema_is_refused(self):
        original = copy.deepcopy(self.frames)
        for field, value in (("is_error", True), ("name", "setup_install")):
            with self.subTest(field=field):
                self.frames = copy.deepcopy(original)
                self.frame("result")["payload"][field] = value
                with self.assertRaises(AssertionError):
                    self.completion()
        self.frames = original
        self.frame("result")["payload"]["content"][0]["data"] = {**self.result, "schema_digest": "0" * 64}
        with self.assertRaisesRegex(AssertionError, "supplied bytes"):
            self.completion()

    def test_native_schema_requires_one_structured_block_not_a_text_fallback(self):
        structured = {"type": "structured", "data": copy.deepcopy(self.result)}
        text = {"type": "text", "text": json.dumps(self.result)}
        for content in (None, {}, [], [None], [text], [structured, text],
                [structured, structured], [{**structured, "text": text["text"]}],
                [{"type": "structured"}], [{"type": "text", "data": self.result}]):
            with self.subTest(content=content):
                self.frame("result")["payload"]["content"] = copy.deepcopy(content)
                # A correct display projection cannot replace the canonical result.
                self.frame("result")["payload"]["result"] = text["text"]
                with self.assertRaisesRegex(AssertionError, "exact structured block"):
                    self.completion()

    def test_structured_schema_data_must_match_supplied_bytes_and_model_result(self):
        for value in (None, [], json.dumps(self.result), {**self.result, "bytes": 3},
                {**self.result, "runtime_observation": "established"},
                {**self.result, "unexpected": True}):
            with self.subTest(value=value):
                self.frame("result")["payload"]["content"][0]["data"] = value
                with self.assertRaisesRegex(AssertionError, "supplied bytes"):
                    self.completion()
        self.frame("result")["payload"]["content"][0]["data"] = {**self.result, "notice": "Different native result."}
        with self.assertRaisesRegex(AssertionError, "actual native result"):
            self.completion()

    def test_find_refusal_or_foreign_worker_cannot_complete(self):
        original = copy.deepcopy(self.frames)
        for value in ({**self.find, "created": False}, {**self.find, "worker": {"state": "unavailable"}}):
            with self.subTest(value=value):
                self.frames = copy.deepcopy(original)
                self.frame("find-result")["payload"]["content"][0]["text"] = json.dumps(value)
                with self.assertRaisesRegex(AssertionError, "did not admit its actual helper"):
                    self.completion()

    def test_same_request_text_with_changed_context_cannot_complete(self):
        for field in ("user_context", "caller_observations"):
            with self.subTest(field=field):
                request = {"request": console.REOPEN_REQUEST, "user_context": [], "caller_observations": []}
                request[field] = ["different context"]
                self.frame("start")["payload"]["input"]["content"] = json.dumps(request)
                with self.assertRaisesRegex(AssertionError, "complete Find request"):
                    self.completion()

    def test_replaced_session_or_native_lineage_cannot_complete(self):
        original = copy.deepcopy(self.frames)
        for identifier in ("send", "original-start", "original-done", "start", "done"):
            with self.subTest(identifier=identifier):
                self.frames = copy.deepcopy(original)
                self.frame(identifier)["session_id"] = "new-session"
                with self.assertRaises(AssertionError):
                    self.completion()
        self.frames = original
        self.frame("result")["payload"]["identity"]["run_id"] = "other-run"
        with self.assertRaisesRegex(AssertionError, "native run lineage"):
            self.completion()

    def test_replayed_tool_and_answer_before_result_are_refused(self):
        duplicate = copy.deepcopy(self.frame("call"))
        duplicate["id"] = duplicate["source_event_id"] = "second-call"
        self.frames.insert(-2, duplicate)
        with self.assertRaisesRegex(AssertionError, "repeated"):
            self.completion()
        self.frames.remove(duplicate)
        self.frames[-1], self.frames[-2] = self.frames[-2], self.frames[-1]
        with self.assertRaisesRegex(AssertionError, "native order"):
            self.completion()


if __name__ == "__main__":
    unittest.main()
