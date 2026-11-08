#!/usr/bin/env python3
"""Opt-in D-configured post-stop completion and governed helper refusal.

No build, alternate shutdown owner or product configuration is introduced.
Completion requires native source/run frames and stopped native audit readback.
"""
from __future__ import annotations

import asyncio
from http.client import HTTPConnection
import importlib.util
import json
from pathlib import Path
import socket
import sqlite3
import sys
import threading
import time
import uuid
from urllib.parse import urlencode

DRAIN_MS = 30000
BOUND = 90.0
CALL = "drain-find-and-install"
ANSWER = "The entered request completed after the host refused new setup work during shutdown."


class Timeline:
    """One authenticated stream opened before shutdown, never reconnected."""
    def __init__(self, v, c, port, token):
        self.v, self.c = v, c
        self.frames, self.by_id, self.events, self.errors = [], {}, [], []
        self.lock = threading.Lock()
        self.snapshot = threading.Event()
        self.finished = threading.Event()
        self.total = 0
        self.access_revision = None
        self.connection = HTTPConnection("127.0.0.1", port, timeout=BOUND)
        self.response = None
        try:
            self.connection.request("GET", "/console/timeline/stream?" + urlencode({
                "identity": "original", "mode": "since", "limit": 400}),
                headers={"Accept": "text/event-stream", "Authorization": "Bearer " + token})
            self.socket = self.connection.sock
            self.response = self.connection.getresponse()
            v.require(self.response.status == 200 and
                      self.response.getheader("Content-Type", "").startswith("text/event-stream"),
                      "authenticated native timeline stream did not open")
            self.thread = threading.Thread(target=self.read, name="drain-native-timeline", daemon=False)
            self.thread.start()
        except BaseException:
            if self.response is not None:
                self.response.close()
            self.connection.close()
            raise

    def read(self):
        try:
            data, event = [], None
            while True:
                line = self.response.readline(1024 * 1024 + 1)
                if not line:
                    break
                self.total += len(line)
                self.v.require(len(line) <= 1024 * 1024 and self.total <= 16 * 1024 * 1024,
                               "native stream exceeded its evidence bound")
                line = line.decode("utf-8").rstrip("\r\n")
                if line.startswith("event:"):
                    event = line[6:].strip()
                elif line.startswith("data:"):
                    data.append(line[5:].lstrip())
                elif not line and data:
                    value = self.v.unique_json("\n".join(data).encode())
                    with self.lock:
                        self.events.append({"event": event, "data": value})
                        if event in ("console_frame", "frame_updated"):
                            frame = self.c.frame_record(value)
                            old = self.by_id.get(frame["id"])
                            if old is None:
                                self.by_id[frame["id"]] = len(self.frames)
                                self.frames.append(frame)
                            else:
                                self.frames[old] = frame
                        elif event == "snapshot_complete":
                            self.snapshot.set()
                        elif event == "access_changed":
                            self.v.require(isinstance(value, dict) and set(value) == {"type", "revision"}
                                           and value["type"] == "access_changed"
                                           and type(value["revision"]) is int and value["revision"] >= 0
                                           and (self.access_revision is None or
                                                value["revision"] > self.access_revision),
                                           "native access revision notification is malformed or regressed")
                            self.access_revision = value["revision"]
                        else:
                            self.v.require(event == "snapshot_started", "native stream lost replay or access")
                    data, event = [], None
        except BaseException as error:
            self.errors.append(repr(error))
        finally:
            self.finished.set()

    def rows(self):
        with self.lock:
            return list(self.frames)

    def join(self):
        # Called only after the native child has settled. Closing this one
        # retained stream is cleanup, never a new request after listener stop.
        self.thread.join(5.0)
        if self.thread.is_alive():
            self.errors.append("native stream did not end with its exited owner")
            sock = self.socket
            if sock is not None:
                try:
                    sock.shutdown(socket.SHUT_RDWR)
                except OSError:
                    pass
        self.thread.join(5.0)
        self.connection.close()
        self.v.require(not self.thread.is_alive(), "native timeline reader did not join")


def factory(Base, v, c, origin, observations):
    class Provider(v.InstalledAcquisitionProvider):
        def __init__(self, directory, events):
            self.entered, self.release, self.abort = threading.Event(), threading.Event(), threading.Event()
            self.coordinator = None
            self.coordinator_error = None
            self.timeline = None
            self.expected = None
            super().__init__(directory, events)

        def response(self, body):
            v.require(body["model"] == "fixture-original-model", "drain admitted a helper model")
            users = [row["content"] for row in body["messages"] if row["role"] == "user"]
            v.require(users == [self.request_text], "drain model input is not the exact Console request")
            tools = [row for row in body["messages"] if row["role"] == "tool"]
            if not self.entered.is_set():
                v.require(len(self.requests) == 1 and not tools, "entered original was replayed")
                self.entered.set()
                self.lock.release()
                try:
                    v.require(self.release.wait(BOUND) and not self.abort.is_set(),
                              "actual stop callback did not release the entered model response")
                finally:
                    self.lock.acquire()
                v.require("find_and_install" in {row["function"]["name"] for row in body["tools"]},
                          "actual original lacks governed find_and_install")
                self.issued.add(CALL)
                self.events.add("drain_response_released", call_id=CALL)
                return {"delta": {"tool_calls": [{"index": 0, "id": CALL, "type": "function",
                    "function": {"name": "find_and_install", "arguments": json.dumps({
                        "request": self.request_text, "user_context": [], "caller_observations": []})}}]},
                    "finish_reason": "tool_calls"}
            v.require(len(self.requests) == 2 and len(tools) == 1 and tools[0]["tool_call_id"] == CALL,
                      "drain feedback did not return once to the same original")
            value = v.unique_json(tools[0]["content"].encode())
            self.results[CALL] = value
            v.require(value == self.expected, "actual native helper result is not the exact typed refusal")
            return {"delta": {"content": ANSWER}, "finish_reason": "stop"}

        def close(self, *, drain):
            # Every harness exit releases a held provider before joining it.
            if not self.release.is_set():
                self.abort.set()
                self.release.set()
            if self.coordinator is not None:
                self.coordinator.join(5.0)
            try:
                super().close(drain=drain)
            finally:
                if self.timeline is not None:
                    self.timeline.join()
            v.require(self.coordinator is not None and not self.coordinator.is_alive()
                      and self.coordinator_error is None, "stop-observation coordinator did not join cleanly")

    class Scenario(Base):
        scope = "D-configured post-stop lifecycle completion with exact native entered Console turn"

        def __init__(self):
            super().__init__(origin, False, observations)
            self.name = "nonzero-drain-entered-console"
            self.enrollment = None
            self.accepted = None
            self.provider_owner = None

        def configure_runtime(self, config, directory):
            super().configure_runtime(config, directory)
            config.shutdown_in_flight_drain_ms = DRAIN_MS

        def provider(self, directory, events):
            self.provider_owner = Provider(directory, events)
            return self.provider_owner

        async def run(self, private, provider, directory, events, process, base_url):
            # Retain the existing real Ready/acquisition-owner checks first.
            await super().run(private, provider, directory, events, process, base_url)
            provider.expected = self.observed("expected-refusal")
            expires = int(time.time() + BOUND + 60)
            login = "nonzero-drain-" + uuid.uuid4().hex
            self.enrollment = private.begin_console_login(v.SDK.ConsoleLoginSelection(
                issuer=c.ISSUER, subject=c.SUBJECT, login_scope_id=login, expires_at_ms=expires * 1000), timeout=10)
            await asyncio.to_thread(self.enrollment.result)
            token = c.signed_login(login, expires)
            provider.timeline = await asyncio.to_thread(Timeline, v, c, self.port, token)
            v.require(await asyncio.to_thread(provider.timeline.snapshot.wait, 10),
                      "native stream did not finish its initial snapshot")
            with provider.lock:
                provider.enabled = True

            def send():
                connection = HTTPConnection("127.0.0.1", self.port, timeout=15)
                try:
                    body = {"identity": "original", "content": self.original_request(),
                        "origin": "console", "origin_kind": "operator", "idempotency_key": login}
                    connection.request("POST", "/console/send", body=v.canonical(body), headers={
                        "Authorization": "Bearer " + token, "Origin": self.origin, "Content-Type": "application/json"})
                    response = connection.getresponse()
                    raw = response.read(1024 * 1024 + 1)
                    v.require(len(raw) <= 1024 * 1024 and response.status == 200, "actual Console send failed")
                    accepted = v.unique_json(raw)
                    v.save_json(directory / "drain-console-send.json", {"request": body, "accepted": accepted})
                    return accepted
                finally:
                    connection.close()
            self.accepted = await asyncio.to_thread(send)
            v.require(self.accepted.get("status") == "accepted" and self.accepted.get("identity") == "original"
                      and all(c.nonempty(self.accepted.get(key)) for key in
                              ("session_id", "interaction_id", "input_frame_id")), "missing native Send anchor")
            v.require(await asyncio.to_thread(provider.entered.wait, 15), "actual original never entered the model")
            deadline = time.monotonic() + 15
            while time.monotonic() < deadline:
                if any(row.get("kind") == "run_started" for row in provider.timeline.rows()):
                    break
                await asyncio.sleep(0.02)
            self.selected_frames(provider.timeline.rows(), complete=False)
            v.require(not (observations / "stop-admitting.json").exists(), "stop preceded the held entered turn")

            def coordinate():
                try:
                    deadline = time.monotonic() + BOUND
                    while not provider.abort.is_set() and time.monotonic() < deadline:
                        path = observations / "stop-admitting.json"
                        if path.exists():
                            observed = v.unique_json(v.exact_bytes(path))
                            v.require(observed["same_installed_owner"] is True and observed["closed"] is False
                                      and observed["admission_stopped"] is True
                                      and observed["recovery"]["stopped"] is True
                                      and observed["recovery"]["selected_owner_control"] is True,
                                      "actual stop observation is not the selected open owner's fence")
                            events.add("drain_actual_stop_observed", observation=v.artifact_pin(path))
                            provider.release.set()
                            return
                        provider.abort.wait(0.01)
                    raise AssertionError("Meta never invoked the successful real stop callback")
                except BaseException as error:
                    provider.coordinator_error = repr(error)
                    provider.abort.set()
                    provider.release.set()
            provider.coordinator = threading.Thread(target=coordinate, name="drain-stop-observer", daemon=False)
            provider.coordinator.start()
            # The existing harness now performs its ONE shutdown. Keep this
            # actual login enrolled until that owner and its drain have joined.
            return {"status": "entered_response_held", "accepted": self.accepted,
                    "stream_open_before_stop": True, "login_retirement": "retained_through_native_shutdown"}

        def selected_frames(self, frames, *, complete):
            a = self.accepted
            anchors = [f for f in frames if f["id"] == a["input_frame_id"]]
            v.require(len(anchors) == 1, "exact native Send anchor absent")
            anchor = anchors[0]
            v.require(anchor["kind"] == "user_input" and anchor["source"]["kind"] == "send"
                      and anchor["identity"] == a["identity"] and anchor["session_id"] == a["session_id"]
                      and anchor["interaction_id"] == a["interaction_id"]
                      and anchor["payload"]["content"] == self.original_request(), "native Send anchor mismatch")
            live = [f for f in frames if f.get("source", {}).get("kind") == "console_event"
                    and f.get("identity") == a["identity"]]
            starts = [f for f in live if f["kind"] == "run_started"]
            v.require(len(starts) == 1, "expected one entered native run")
            start = starts[0]
            run = c.run_id(start)
            v.require(start["session_id"] == a["session_id"] and c.input_text(start) == self.original_request(),
                      "native entered run differs from the accepted input")
            v.require(not any(f["kind"] in ("run_failed", "interaction_failed") for f in live),
                      "native entered turn failed")
            if not complete:
                v.require(not any(f["kind"] == "interaction_complete" for f in live), "held run already ended")
                return run
            selected = [f for f in live if f.get("run_id") == run]
            calls = [f for f in selected if f["kind"] == "tool_call_requested"]
            results = [f for f in selected if f["kind"] == "tool_result_received"]
            ended = [f for f in selected if f["kind"] == "interaction_complete"]
            v.require(len(calls) == len(results) == len(ended) == 1, "native call/result/completion incomplete")
            for frame in calls + results + ended:
                v.require(c.run_id(frame) == run and frame.get("interaction_id") == start.get("interaction_id"),
                          "native tool or answer crossed the entered run")
            call, result, done = calls[0], results[0], ended[0]
            v.require(call["payload"]["id"] == result["payload"]["id"] == CALL
                      and call["payload"]["name"] == result["payload"]["name"] == "find_and_install"
                      and call["payload"]["args"] == {"request": self.original_request(),
                          "user_context": [], "caller_observations": []} and result["payload"]["is_error"] is True,
                      "native governed call/result differs from the selected request")
            blocks = result["payload"]["content"]
            v.require(isinstance(blocks, list) and len(blocks) == 1 and blocks[0].get("type") == "text"
                      and v.unique_json(blocks[0]["text"].encode()) == self.provider_owner.expected,
                      "native stream did not carry the actual typed refusal")
            v.require(done["session_id"] == a["session_id"] and done["status"] == "completed"
                      and done["payload"].get("source_event_type") == "run_completed"
                      and done["payload"]["session_id"] == a["session_id"] and done["payload"]["result"] == ANSWER
                      and done["payload"].get("extraction_required", False) is False
                      and c.nonempty(done["payload"].get("assistant_message_id")), "exact native committed answer missing")
            positions = {f["id"]: i for i, f in enumerate(frames)}
            v.require(positions[anchor["id"]] < positions[start["id"]] < positions[call["id"]]
                      < positions[result["id"]] < positions[done["id"]], "native causal order differs")
            return run

        def check_final_artifacts(self, directory, result):
            try:
                self.validate_final_artifacts(directory, result)
            except BaseException as error:
                v.save_json(directory / "drain-evidence-refusal.json", {"error": repr(error),
                    "meaning": "native completion and drain are not accepted"})
                raise

        def validate_final_artifacts(self, directory, result):
            p = self.provider_owner
            timeline = p.timeline
            if timeline is not None:
                v.save_json(directory / "drain-native-stream.json", {"events": timeline.events,
                    "frames": timeline.rows(), "errors": timeline.errors, "reader_joined": not timeline.thread.is_alive()})
            v.require(self.observed("joined") == {"same_installed_owner": True,
                "host_source_local_shared_joined": True, "recovery": {
                    "stopped": True, "selected_owner_control": True, "task": "Finished"}}
                and self.observed("driver-return") == {"returned_ok": True, "error": None},
                "actual recovery/native/private cleanup did not finish")
            v.require(p.coordinator is not None and not p.coordinator.is_alive() and p.coordinator_error is None
                      and timeline is not None and not timeline.thread.is_alive() and not timeline.errors,
                      "drain evidence readers/coordinator failed or remained live")
            run = self.selected_frames(timeline.rows(), complete=True)
            events = v.unique_json(v.exact_bytes(directory / "events.json"))
            init = [row["message"] for row in events if row["event"] == "sdk_wire_out"
                    and row["message"].get("method") == "meta/init"]
            v.require(len(init) == 1 and init[0]["params"].get("shutdown_in_flight_drain_ms") == DRAIN_MS,
                      "actual native init did not select the fixture-only drain window")
            ordered = [row["event"] for row in events]
            v.require(ordered.count("drain_actual_stop_observed") == ordered.count("drain_response_released") == 1
                      and ordered.index("drain_actual_stop_observed") < ordered.index("drain_response_released"),
                      "held model response preceded actual stop observation")
            v.require(result.get("model_request_count") == 2 and result.get("provider_joined") is True
                      and len(result.get("cleanup_observation", {})) == 9
                      and all(result["cleanup_observation"].values()), "drain model/cleanup scope changed")
            drain = result["shutdown_response"]["result"]["in_flight_drain"]
            v.require(type(drain["window_ms"]) is int and 0 < drain["window_ms"] <= DRAIN_MS
                      and drain["outcome"] == "drained"
                      and all(drain[key] == [] for key in ("still_running_at_bound", "live_channel_open",
                          "not_local", "reload_required", "starts_not_held"))
                      and drain.get("unclassified", []) == [],
                      "correlated actual drain was unsuccessful or retained unresolved work")
            path = (directory / "state/runtime.sqlite").resolve(strict=True)
            before = v.artifact_pin(path)
            with sqlite3.connect(path.as_uri() + "?mode=ro", uri=True, timeout=3) as db:
                db.execute("PRAGMA query_only=ON")
                read_deadline = time.monotonic() + 10
                db.set_progress_handler(lambda: int(time.monotonic() >= read_deadline), 10000)
                rows = db.execute("SELECT state_json FROM runtime_input_states WHERE runtime_id=? LIMIT 2",
                                  ("rt:session:" + self.accepted["session_id"],)).fetchall()
            v.require(len(rows) == 1, "drain admitted another original input")
            raw = rows[0][0].encode() if isinstance(rows[0][0], str) else rows[0][0]
            v.require(len(raw) <= 8 * 1024 * 1024 and v.artifact_pin(path) == before,
                      "stopped native input evidence changed or exceeded its bound")
            state = v.unique_json(raw)
            v.new_artifact(directory / "drain-native-input.json", raw)
            v.require(state["last_run_id"] == run and state.get("terminal_outcome") is not None
                      and state.get("terminal_completion") is not None, "native input did not finalize the observed run")
            audits = state["authorization_audit"]
            prepared = [a["observation"] for a in audits if a["observation"]["observation"]["kind"] == "prepared"
                        and a["observation"]["observation"]["target"].get("call_id") == CALL
                        and a["observation"]["observation"]["target"].get("tool_name") == "find_and_install"]
            v.require(len(prepared) == 1, "native exact governed tool preparation is missing")
            op = prepared[0]["operation_id"]
            selected = [a for a in audits if a["observation"]["operation_id"] == op]
            v.require(all(a["observation"]["run_id"] == run and
                          [x["input_id"] for x in a["contributors"]] == [state["input_id"]]
                          and a["observation"]["execution_scope"]["owner_session_id"] == self.accepted["session_id"]
                          and a["observation"]["execution_scope"]["submitted_input_id"] == state["input_id"]
                          and a["observation"]["execution_scope"]["canonical_input_id"] == state["input_id"]
                          for a in selected),
                      "native dispatch audit crossed the exact input/run")
            facts = [a["observation"]["observation"] for a in selected]
            v.require([a["boundary"] for a in facts if a["kind"] == "entry"] ==
                      ["tool_dispatch", "tool_execution_policy", "tool_effect"] and
                      any(a["kind"] == "outcome" for a in facts) and
                      all(a["outcome"] == {"kind": "tool_dispatch_error", "error": "unavailable"}
                          for a in facts if a["kind"] == "outcome") and
                      not any(a["kind"] in ("refused", "authorization_unavailable") for a in facts),
                      "native exact dispatch/policy/effect entry or unavailable outcome differs")
            stderr = v.exact_bytes(directory / "gateway.stderr").decode()
            v.require(stderr.count("AA1 failure: stage=leaf.advance variant=SetupError::NativeExecutionUnavailable") == 1,
                      "the sole governed call lacks the existing actual host-advance failure observation")
            # Mandatory post-stop native call, typed result and exact completed
            # answer above are independent of when the drain snapshots a run.
            # An empty list supplies no captured/waited-by-drain evidence.
            if drain["ended"] == [{"member": "original", "terminal": "completed"}]:
                capture = "captured_ended"
            elif drain["ended"] == []:
                capture = "completed_without_drain_capture_evidence"
            else:
                raise AssertionError("drain ended list names a foreign or non-completed run")
            result["nonzero_drain"] = {"run_id": run, "input_id": state["input_id"], "operation_id": op,
                "accepted": self.accepted, "drain": drain, "coordinator_joined": True,
                "capture": capture,
                "login_retirement": "native_owner_shutdown_after_drain",
                "scope": "D-configured post-stop lifecycle completion; empty ended is not captured/waited-by-drain evidence; no product or SIGTERM claim",
                "fixture_source": v.artifact_pin(Path(__file__).resolve()),
                "evidence": [v.artifact_pin(directory / name) for name in (
                    "drain-console-send.json", "drain-native-stream.json", "drain-native-input.json")] +
                    [v.artifact_pin(observations / name) for name in (
                        "stop-admitting.json", "expected-refusal.json", "joined.json", "driver-return.json")]}

    return Scenario()


if __name__ == "__main__":
    spec = importlib.util.spec_from_file_location("nonzero_driver_shared_fixture",
        Path(__file__).with_name("late_driver_adoption.py"))
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    raise SystemExit(module.main(drain_factory=factory))
