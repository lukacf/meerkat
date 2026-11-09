"""Installed helper-reset catalog consumption through selected Forge adapters.

Imported only by the explicit installed-console-helper-reset case. No I/O at
import. Native owns preparation, reset, catalog authority, dispatch and stop.
This fixture proves supplied-schema inspection only, not installation, business
use, Browser interaction, old-work recovery or revocation of historical grants.
"""
from __future__ import annotations

import asyncio
import copy
import dataclasses
import hashlib
from http.client import HTTPConnection
import importlib.util
import json
import os
from pathlib import Path
import sqlite3
import sys
import time
import types
import uuid
from urllib.parse import urlencode

REQUEST = "Inspect only the supplied empty JSON schema using the setup helper. Do not install software, request service access or read any external service."
ORIGINAL_FINISH = "The request was submitted to the setup helper; inspection completion is checked separately."
HELPER_FINISH = "The supplied two-byte JSON schema was inspected. No runtime schema validation, installation or external service read was performed."
INSPECT = "setup_inspect_schema"
FIND_ID, SEARCH_ID, LOAD_ID, INSPECT_ID = ("reset-original-find", "reset-helper-search", "reset-helper-load", "reset-helper-inspect")
SCHEMA_RESULT = {"schema_version": 1, "scope": "supplied_schema_evidence_only",
    "schema_digest": "44136fa355b3678a1146ad16f7e8649e94fb4fc21fe77e8310c060f61caaff8a",
    "bytes": 2, "hash_algorithm": "sha256", "byte_convention": "exact_supplied_utf8",
    "runtime_observation": "not_established", "schema_validation": "not_performed"}
FORGE_NAMES = ("connect_policy_input", "connect_installer_caller", "connect_application_plan")


def require(value, message):
    if not value:
        raise AssertionError(message)


def nonempty(value):
    return isinstance(value, str) and bool(value.strip())


def search_call_id(index):
    require(type(index) is int and 0 <= index < 512, "catalog search observation exceeds existing model bound")
    return SEARCH_ID if index == 0 else f"{SEARCH_ID}-{index + 1}"


def pending_setup_search(value):
    return (isinstance(value, dict)
            and set(value) == {"catalog_exact", "total_matches", "results", "pending_sources", "empty_result_status"}
            and value["catalog_exact"] is True
            and type(value["total_matches"]) is int and value["total_matches"] == 0
            and value["results"] == [] and value["pending_sources"] == ["toolkit-setup"]
            and value["empty_result_status"] == "pending_source")


def checked_catalog_searches(results, *, terminal=False):
    searches = []
    for index in range(512):
        identifier = search_call_id(index)
        if identifier not in results:
            break
        searches.append({"id": identifier, "result": results[identifier]})
    require(all(pending_setup_search(row["result"]) for row in searches[:-1]),
            "another Search followed a result other than the exact pending setup source")
    if terminal:
        require(searches, "native catalog search result is missing")
        search = searches[-1]["result"]
        rows = [row for row in search.get("results", []) if row.get("name") == INSPECT]
        require(search.get("catalog_exact") is True and not search.get("pending_sources", [])
                and len(rows) == 1 and rows[0]["visibility_status"] in ("deferred", "loaded"),
                "new helper catalog did not resolve the exact schema tool")
    return searches


def schema_result(value):
    require(isinstance(value, dict) and set(value) == set(SCHEMA_RESULT) | {"notice"}
            and all(value[key] == expected for key, expected in SCHEMA_RESULT.items())
            and nonempty(value["notice"]), "actual supplied-schema evidence or limits differ")


def checked_reset_result(reply, request_id, old_session, original_session, identities):
    require(reply.get("id") == request_id and "error" not in reply, "native reset correlation/refusal")
    reset = reply["result"]
    require(reset["identity"] == "helper" and nonempty(reset["session_id"]) and reset["session_id"] != old_session
            and type(reset["generation"]) is int and reset["generation"] > 0
            and nonempty(reset.get("agent_runtime_id")), "native reset did not change helper session")
    rows = identities["identities"]
    require(len(rows) == 2 and {row["identity"] for row in rows} == {"original", "helper"},
            "reset member inventory differs")
    current = {row["identity"]: row for row in rows}
    require(current["helper"]["session_id"] == reset["session_id"]
            and current["helper"]["runtime_member_id"] == reset["agent_runtime_id"]
            and current["original"]["session_id"] == original_session, "reset readback mismatch")
    return reset


def checked_idle_initial_settlement(records, old_session, new_session, member):
    # These are the selected diagnostic build's existing receipts after shutdown.
    # A callback-entry/Drop record alone never proves native settlement.
    require(nonempty(old_session) and nonempty(new_session) and old_session != new_session,
            "idle attempt sessions are not distinct")
    counts = {"registration": 2, "begin_enter": 2, "begin": 2, "finish_enter": 2,
              "finish_return": 2, "native_join_report": 1, "native_join_record": 4, "local_join_return": 1}
    require(isinstance(records, list) and len(records) == sum(counts.values()),
            "idle diagnostic record inventory differs")
    events, sequences = {name: [] for name in counts}, set()
    for row in records:
        require(row.get("diagnostic") == "connect_reset_attempt_v1" and type(row.get("sequence")) is int
                and 0 <= row["sequence"] < 256 and row["sequence"] not in sequences,
                "idle diagnostics are missing, duplicated or truncated")
        record = row["record"]
        require(record.get("event") in counts and isinstance(record.get("fields"), dict),
                "idle diagnostic event differs")
        sequences.add(row["sequence"])
        events[record["event"]].append((row["sequence"], record["fields"]))
    require(sequences == set(range(len(records))) and all(len(events[k]) == n for k, n in counts.items()),
            "idle diagnostic event inventory is incomplete")
    def identifier(value):
        require(nonempty(value) and len(value) <= 160, "idle diagnostic identity exceeds source bound")
        return {"truncated": False, "value": value}
    def one(event, predicate):
        rows = [row for row in events[event] if predicate(row[1])]
        require(len(rows) == 1, "idle diagnostic owner or attempt is ambiguous: " + event)
        return rows[0]
    attempts = []
    for session in (old_session, new_session):
        entered_seq, entered = one("begin_enter", lambda f: f.get("session") == identifier(session))
        key = entered["key"]
        require(set(key) == {"attempt", "construction", "server"}
                and all(type(key[k]) is int and key[k] > 0 for k in ("attempt", "construction"))
                and key["server"] == identifier("toolkit-setup") and entered["operation"] == "Add"
                and entered["member"] == {k: identifier(member[k]) for k in ("mob_id", "role", "member")},
                "idle attempt does not name the actual helper member")
        registration_seq, registration = one("registration", lambda f: f.get("session") == identifier(session))
        begin_seq, begin = one("begin", lambda f: f.get("key") == key)
        finish_seq, finish = one("finish_enter", lambda f: f.get("key") == key)
        returned_seq, returned = one("finish_return", lambda f: f.get("key") == key)
        require(registration_seq < entered_seq < begin_seq < finish_seq < returned_seq
                and registration["stage"] == "registered" and registration["unwinding"] is False
                and begin["stage"] == "guard_returned" and begin["unwinding"] is False
                and begin["round"] == finish["round"] == "Initial"
                and begin["session"] == entered["session"] and begin["member"] == entered["member"]
                and begin["operation"] == entered["operation"]
                and begin["state"] == {"active": None, "closed": False, "owner_pending": False,
                    "owner_poisoned": False, "owner_present": True,
                    "registered_session": identifier(session), "started": False},
                "idle source did not return an unused Initial attempt")
        require(returned["pending_before_restore"] is False and returned["poisoned_before_restore"] is False
                and all(returned[k] is True for k in ("restored", "reusable", "round_result_ok")),
                "idle attempt owner was not restored cleanly")
        attempts.append((key, finish, returned, returned_seq, entered_seq))
    old, new = attempts
    require(old[0] != new[0] and old[3] < new[4], "replacement preceded exact old-attempt restoration")
    cancelled = {"key": old[0], "open": {"cause": "RouterClosed", "kind": "Cancelled"},
                 "process": {"kind": "NotSpawned"}}
    require(old[1]["settle"] is False and old[1]["disposition"] == cancelled
            and old[2]["healthy"] is False and old[2]["initial_cancellation"] == "NotSpawned",
            "idle reset did not cancel the old Initial before spawn")
    disposition = new[1]["disposition"]
    process = disposition["process"]
    require(new[1]["settle"] is True and new[2]["healthy"] is True and new[2]["initial_cancellation"] == "None"
            and disposition["key"] == new[0] and disposition["open"]["kind"] == "Installed"
            and type(disposition["open"]["connection_generation"]) is int
            and disposition["open"]["connection_generation"] > 0
            and process["kind"] == "Closed" and process["normal_close"] is True
            and process["leader"] == {"code": 0, "kind": "Reaped", "success": True}
            and process["sigterm"] == process["sigkill"] == "None",
            "replacement Initial did not install and close normally")
    report_seq, joined = events["native_join_report"][0]
    require(new[3] < report_seq and joined == {"before_deadline": True, "records": 4},
            "idle native guard join did not settle")
    join_sequences = []
    for key, stage, completion, native_disposition in (
            (old[0], "Begin", "Cancelled", None), (new[0], "Begin", "Completed", None),
            (old[0], "Cancel", "Completed", cancelled), (new[0], "Settle", "Completed", disposition)):
        seq, record = one("native_join_record", lambda f: f.get("key") == key and f.get("stage") == stage)
        require(seq > report_seq and record == {"key": key, "stage": stage, "completion": completion,
                "disposition": native_disposition}, "idle native joined outcome differs")
        join_sequences.append(seq)
    local_seq, local = events["local_join_return"][0]
    require(local_seq > max(join_sequences) and local == {"failed": False, "native_drained": True,
            "owner_clean": True, "owner_pending": False, "owner_present": True},
            "idle local owner did not finish its actual drain")
    return {"old_attempt": old[0], "replacement_attempt": new[0], "old_cancel": cancelled,
            "replacement_settle": disposition, "native_join_records": 4,
            "scope": "Exact selected diagnostic receipts plus separate native completion and joined shutdown; no initialized-old-transport claim."}


def create_scenario(v, console, origin, forge_path, forge_sha256):
    class ResetScenario(console.ConsoleScenario):
        name = "installed-console-helper-reset-success"
        scope = ("installed helper-reset catalog consumption through reviewed Forge plan and retained-manifest adapters; "
                 "actual native changed helper session, Search/Load and supplied-schema result completion; "
                 "controlled Console API and model, no full Forge wizard or Browser UX, installation or business-use claim")

        def __init__(self):
            super().__init__(v, origin)
            self.forge_receipt = v.bytes_pin(forge_path, v.exact_bytes(forge_path, forge_sha256))
            selected = v.unique_json(v.exact_bytes(forge_path, forge_sha256))
            self.forge_sources, paths = [], {}
            for name in FORGE_NAMES:
                rows = [row for row in selected["forge_files"]
                        if row["repository_path"] == "installer/launchkit/" + name + ".py"]
                require(len(rows) == 1, "Forge source module selection is ambiguous")
                row, path = rows[0], Path(rows[0]["path"])
                require(path.is_absolute() and not path.is_symlink(), "Forge source path is not exact")
                raw = v.exact_bytes(path, row["sha256"])
                require(len(raw) == row["bytes"], "Forge source length differs")
                paths[name] = path
                self.forge_sources.append(v.bytes_pin(path, raw))
            require(len({path.parent for path in paths.values()}) == 1, "Forge sources are not one package")
            package_name = "connect_selected_helper_reset_forge"
            require(package_name not in sys.modules, "Forge fixture package already exists")
            package = types.ModuleType(package_name)
            package.__path__ = [str(next(iter(paths.values())).parent)]
            sys.modules[package_name] = package
            modules = {}
            for name in FORGE_NAMES:
                full = package_name + "." + name
                require(full not in sys.modules, "Forge module was imported outside the frozen order")
                spec = importlib.util.spec_from_file_location(full, paths[name])
                module = importlib.util.module_from_spec(spec)
                sys.modules[full] = module
                spec.loader.exec_module(module)
                require(Path(module.__file__).resolve() == paths[name].resolve(), "Forge import escaped source")
                modules[name] = module
            self.caller_module = modules["connect_installer_caller"]
            self.plan_module = modules["connect_application_plan"]
            self.check_sources()

        def check_sources(self):
            for pin in [self.forge_receipt, *self.forge_sources]:
                v.exact_bytes(Path(pin["path"]), pin["sha256"])

        def native_admin_subjects(self):
            # Explicit controlled fixture operator, verified by the same native
            # Console authentication/authorization as the existing reset path.
            return [console.EMAIL]

        def configure_owner(self, owner, source):
            super().configure_owner(owner, source)
            source["purpose"] = "installed helper reset followed by catalog and supplied-schema consumption"
            source["helper_reset_scope"] = {"request": REQUEST, "schema_json": "{}",
                "external_reads": False, "installation": False, "old_work_recovery": False}
            self.source_owner = copy.deepcopy(owner)

        def configure_plan(self, plan):
            super().configure_plan(plan)
            nonce = uuid.uuid4().hex
            plan["authorities"]["helper_catalog"] = {
                "issuance_id": "reset-catalog-parent-" + nonce, "scope": "current_helper_successors"}
            requests = plan["authorities"]["agent_setup"]
            require(len(requests) == 1, "expected the actual single Original setup route")
            requests[0]["issuance_id"] = "reset-original-request-" + nonce
            bases = [target for target in requests[0]["helper_targets"] if target["target"] is None]
            require(len(bases) == 1, "Original setup request lacks its separate catalog target")
            bases[0]["issuance_id"] = "reset-original-catalog-" + nonce
            require(plan["application"]["helper"]["operation_basis"]["base"] is None,
                    "successor parent must not be copied into an operation basis")
            # The existing installed fixture keeps its normal helper setup
            # bounds. This controlled schedule issues none of their effects.
            self.source_application = copy.deepcopy(plan["application"])

        def prepare_plan(self, directory, plan, plan_pin, source_pin):
            self.check_sources()
            exact = self.caller_module.ExactFile(Path(plan_pin["path"]), plan_pin["sha256"])
            raw = self.plan_module.application_preparation_plan(exact, require_current=self.check_sources)
            prepared = v.unique_json(raw)
            require(prepared == plan, "Forge assembler changed the fresh native fixture declarations")
            selected = v.new_artifact(directory / "forge-application-plan.json", raw)
            self.plan, self.plan_pin, self.source_pin = copy.deepcopy(prepared), selected, source_pin
            self.draft_pin = plan_pin
            self.realm_state_root = Path(prepared["application"]["realm"]["state_root"])
            return prepared, selected, source_pin

        def observe_preparation(self, attempt, finalizer, gateway, process, observe):
            require(attempt.manifest is not None and attempt.selection is not None
                    and attempt.settlement is not None
                    and dataclasses.asdict(attempt.selection) == finalizer.selection,
                    "Forge retention must use the same actual native preparation attempt")
            self.live_process = process
            self.observe_children = observe

            def current():
                self.check_sources()
                require(process.poll() is None, "original preparation owner is no longer live")
                for pin in (self.draft_pin, self.plan_pin, self.source_pin):
                    v.exact_bytes(Path(pin["path"]), pin["sha256"])

            async def no_command(_argv):
                raise AssertionError("retained-manifest observation must not execute a Forge command")

            exact = self.caller_module.ExactFile
            caller = self.caller_module.NativeInstallerCaller(
                exact(gateway, v.digest(gateway)), execute=no_command, require_current=current,
                require_filesystem_current=self.check_sources,
                provision_attempt_path=Path(self.plan_pin["path"]).parent / "unused-forge-provision-attempt.json")
            actual = dataclasses.asdict(attempt.manifest)
            require(actual == finalizer.evidence["completed_manifest"], "native manifest identities differ")
            self.bundle = caller.retain_completed_application_preparation(
                exact(Path(actual["path"]), actual["sha256"]),
                preparation_plan=exact(Path(self.plan_pin["path"]), self.plan_pin["sha256"]),
                source_files=tuple(exact(Path(pin["path"]), pin["sha256"])
                                   for pin in (self.source_pin, *self.forge_sources)))
            self.bundle.authenticate()
            self.forge_observation = {"manifest": actual, "preparation_plan": self.plan_pin,
                "draft": self.draft_pin, "source": self.source_pin, "forge_sources": self.forge_sources,
                "scope": "same live native attempt retained by the selected Forge adapter; no alternate native owner"}
            self.server_configuration = copy.deepcopy(finalizer.evidence["completed_server"])

        def observe_completed_setup(self, setup, artifact):
            source = self.source_application
            helper = setup["helper"]
            require(setup["source_helper_member"] == source["helper"]["member"]
                    and setup["helper_profile"] == source["helper_profile"] == "dedicated"
                    and setup["definition_source"] == source["definition"], "completed helper source changed")
            rows = [row for row in setup["native_members"] if row["member"] == helper["member"]]
            selected = [row for row in source["members"] if row["member"] == source["helper"]["member"]]
            require(len(rows) == len(selected) == 1
                    and rows[0]["logical_owner"] == helper["logical_executor"] == selected[0]["logical_owner"]
                    and rows[0]["audience"] == selected[0]["audience"]
                    and helper["member"]["member"] == "helper"
                    and helper["member"]["role"] in setup["definition"]["profiles"],
                    "completed helper member/executor/audience differs")
            capability = helper.get("catalog_capability")
            require(isinstance(capability, dict) and set(capability) == {"parent"}
                    and isinstance(capability["parent"], list) and len(capability["parent"]) == 1
                    and helper["operation_basis"]["base"] is None,
                    "completed helper lacks the explicit catalog-only parent")
            self.completed_helper = copy.deepcopy(helper)
            self.completed_artifact = copy.deepcopy(artifact)
            self.preparation_artifacts = Path(setup["resources"]["artifact_directory"])
            require(self.preparation_artifacts.is_absolute() and self.preparation_artifacts.is_dir()
                    and not self.preparation_artifacts.is_symlink(), "prepared artifact owner path differs")

        def local_round_requests(self):
            # Observe only artifacts of the preparation returned by this same
            # native compiler. Files identify entered offers, not permission or
            # successful native/process settlement.
            paths = sorted(self.preparation_artifacts.glob("round-*/request.json"))
            require(len(paths) <= 1, "one reset must not create repeated local offers")
            rows = []
            prepared = self.preparation_artifacts / "prepared"
            for path in paths:
                require(path.is_file() and not path.is_symlink() and not path.parent.is_symlink(),
                        "local round request is not a retained regular artifact")
                require(path.stat().st_size <= 4096, "local round request exceeds fixed scope")
                raw = v.exact_bytes(path, v.digest(path))
                request = v.unique_json(raw)
                require(request == {"prepared": str(prepared),
                    "realm_state_root": str(self.realm_state_root),
                    "realm": self.source_application["realm"]["realm_id"],
                    "descriptor_sha256": v.digest(prepared / "product-mcp.json"),
                    "preparation_sha256": v.digest(prepared / "product-prepared.json")},
                    "offer does not retain the exact current preparation")
                rows.append(v.bytes_pin(path, raw))
            return rows

        def provider(self, directory, events):
            class ResetProvider(v.InstalledAcquisitionProvider):
                def response(self, body):
                    lane = body["model"]
                    require(lane in ("fixture-original-model", "fixture-helper-model"), "foreign model owner")
                    names = {row["function"]["name"] for row in body["tools"]}
                    messages = body["messages"]
                    require(REQUEST in json.dumps(messages), "actual controller input lost the fixed request")
                    permitted = ({FIND_ID} if lane == "fixture-original-model" else
                                 {LOAD_ID, INSPECT_ID, *(search_call_id(i) for i in range(512))})
                    results = {}
                    for message in messages:
                        if message["role"] == "tool":
                            identifier = message["tool_call_id"]
                            require(identifier in permitted and identifier in self.issued and identifier not in results,
                                    "unissued, repeated or foreign tool result")
                            value = v.unique_json(message["content"].encode())
                            require(identifier not in self.results or self.results[identifier] == value,
                                    "native result changed across controller requests")
                            self.results[identifier] = results[identifier] = value
                    def call(identifier, name, args):
                        require(identifier not in self.issued and name in names, "tool not actually offered or was replayed")
                        self.issued.add(identifier)
                        return {"delta": {"tool_calls": [{"index": 0, "id": identifier, "type": "function",
                            "function": {"name": name, "arguments": json.dumps(args)}}]}, "finish_reason": "tool_calls"}
                    if lane == "fixture-original-model":
                        if FIND_ID not in results:
                            return call(FIND_ID, "find_and_install", {"request": REQUEST, "user_context": [], "caller_observations": []})
                        result = results[FIND_ID]
                        require(result.get("created") is True and nonempty(result.get("operation"))
                                and result.get("worker") == {"state": "input_admitted", "deduplicated": False,
                                                            "source_settlement": "fresh_acceptance_promoted"},
                                "actual helper input was not freshly admitted")
                        return {"delta": {"content": ORIGINAL_FINISH}, "finish_reason": "stop"}
                    searches = checked_catalog_searches(results)
                    if not searches:
                        return call(SEARCH_ID, "tool_catalog_search", {"query": INSPECT, "limit": 50})
                    if pending_setup_search(searches[-1]["result"]):
                        # Only the native pending-source result permits another observation.
                        # Pace it within the existing case deadline; elapsed time never proves readiness.
                        require(time.monotonic() < self.catalog_deadline, "pending catalog exceeded case deadline")
                        time.sleep(min(1.0, max(0.0, self.catalog_deadline - time.monotonic())))
                        require(time.monotonic() < self.catalog_deadline, "pending catalog exceeded case deadline")
                        return call(search_call_id(len(searches)), "tool_catalog_search", {"query": INSPECT, "limit": 50})
                    checked_catalog_searches(results, terminal=True)
                    if LOAD_ID not in results:
                        return call(LOAD_ID, "tool_catalog_load", {"names": [INSPECT]})
                    v.exact_catalog_load(results[LOAD_ID], [INSPECT])
                    if INSPECT_ID not in results:
                        return call(INSPECT_ID, INSPECT, {"schema_json": "{}"})
                    schema_result(results[INSPECT_ID])
                    self.finished.set()
                    return {"delta": {"content": HELPER_FINISH}, "finish_reason": "stop"}
            return ResetProvider(directory, events)

        def completion(self, frames, accepted, reset, results):
            runs, done, pairs = {}, {}, {}
            for identity, session, finish in (("original", accepted["session_id"], ORIGINAL_FINISH),
                                               ("helper", reset["session_id"], HELPER_FINISH)):
                native = [f for f in frames[identity] if f.get("source", {}).get("kind") == "console_event"]
                starts = [f for f in native if f.get("kind") == "run_started"]
                require(len(starts) <= 1, "unexpected additional native run")
                if not starts:
                    return None
                start = starts[0]
                run = console.run_id(start)
                require(start["session_id"] == session == start["payload"]["session_id"]
                        and start["payload"].get("source_event_type") == "run_started", "native start session mismatch")
                if identity == "original":
                    require(console.input_text(start) == REQUEST, "Original native input differs")
                else:
                    require(REQUEST in v.canonical(start["payload"]["input"]).decode(), "helper input lost request")
                require(not any(f.get("kind") in ("interaction_failed", "run_failed") for f in native),
                        "actual native run failed")
                completed = [f for f in native if f.get("kind") == "interaction_complete"
                             and f["payload"].get("source_event_type") == "run_completed"]
                require(len(completed) <= 1, "duplicate native completion")
                if not completed:
                    return None
                end = completed[0]
                require(console.run_id(end) == run and end["session_id"] == session
                        and end["payload"]["session_id"] == session and end["status"] == "completed"
                        and end["payload"]["result"] == finish and nonempty(end["payload"].get("assistant_message_id"))
                        and end["payload"].get("extraction_required", False) is False, "native completion mismatch")
                runs[identity], done[identity] = run, {"start": start["id"], "completion": end["id"]}
            anchor = [f for f in frames["original"] if f["id"] == accepted["input_frame_id"]]
            require(len(anchor) == 1 and anchor[0]["source"]["kind"] == "send"
                    and anchor[0]["payload"]["content"] == REQUEST and anchor[0]["identity"] == "original"
                    and anchor[0]["session_id"] == accepted["session_id"]
                    and anchor[0]["interaction_id"] == accepted["interaction_id"], "Send anchor mismatch")
            searches = checked_catalog_searches(results, terminal=True)
            search_ids = {row["id"] for row in searches}
            selected = [("original", FIND_ID, "find_and_install", {"request": REQUEST, "user_context": [], "caller_observations": []})]
            selected += [("helper", row["id"], "tool_catalog_search", {"query": INSPECT, "limit": 50}) for row in searches]
            selected += [("helper", LOAD_ID, "tool_catalog_load", {"names": [INSPECT]}),
                         ("helper", INSPECT_ID, INSPECT, {"schema_json": "{}"})]
            for identity, identifier, name, args in selected:
                # History projections remain in frames; only native events attest an entered tool.
                fs = [f for f in frames[identity] if f.get("source", {}).get("kind") == "console_event"]
                calls = [f for f in fs if f.get("kind") == "tool_call_requested" and f["payload"].get("id") == identifier]
                replies = [f for f in fs if f.get("kind") == "tool_result_received" and f["payload"].get("id") == identifier]
                require(len(calls) <= 1 and len(replies) <= 1, "native tool replay")
                if not calls or not replies or identifier not in results:
                    return None
                call, reply = calls[0], replies[0]
                require(all(console.run_id(f) == runs[identity] and f["session_id"] ==
                            (accepted["session_id"] if identity == "original" else reset["session_id"])
                            and f["payload"]["name"] == name for f in (call, reply))
                        and call["payload"]["args"] == args and reply["payload"]["is_error"] is False,
                        "native catalog/schema tool execution mismatch")
                content = reply["payload"]["content"]
                if identifier == INSPECT_ID:
                    require(content == [{"type": "structured", "data": results[identifier]}],
                            "native supplied-schema structured result differs from controller-visible value")
                else:
                    require(len(content) == 1 and set(content[0]) == {"type", "text"}
                            and content[0]["type"] == "text"
                            and v.unique_json(content[0]["text"].encode()) == results[identifier],
                            "native catalog/admission text result differs from controller-visible value")
                positions = {f["id"]: i for i, f in enumerate(fs)}
                require(positions[done[identity]["start"]] < positions[call["id"]] < positions[reply["id"]]
                        < positions[done[identity]["completion"]],
                        "native effect/result/completion order differs")
                pairs[identifier] = {"call": call["id"], "result": reply["id"]}
            positions = {f["id"]: i for i, f in enumerate(frames["original"])}
            require(positions[anchor[0]["id"]] < positions[done["original"]["start"]], "run preceded Send")
            for identity, expected in (("original", {FIND_ID}), ("helper", search_ids | {LOAD_ID, INSPECT_ID})):
                actual = [f["payload"]["id"] for f in frames[identity]
                          if f.get("source", {}).get("kind") == "console_event" and f.get("kind") == "tool_call_requested"]
                require(len(actual) == len(expected) and set(actual) == expected, "unexpected native tool effect")
            require(set(results) == search_ids | {FIND_ID, LOAD_ID, INSPECT_ID}, "unexpected effect result")
            schema_result(results[INSPECT_ID])
            return {"runs": runs, "native_completions": done, "tool_pairs": pairs,
                    "catalog_searches": searches,
                    "schema_result": results[INSPECT_ID], "load_result": results[LOAD_ID]}

        def helper_transport_snapshot(self, process):
            # Match only exact descendants against the actual native descriptor.
            # env execs the pinned Python argv, so compare its literal post-env
            # argv rather than treating a process name as helper ownership.
            require(sys.platform.startswith("linux"), "selected installed transport observation requires Linux")
            server = self.server_configuration
            argv = [server["command"], *server.get("args", [])]
            candidates = [argv]
            if server["command"] == "/usr/bin/env":
                args = list(server["args"])
                require(args and args.pop(0) == "-i", "unexpected native setup env invocation")
                while args and "=" in args[0] and not args[0].startswith("/"):
                    args.pop(0)
                require(args and Path(args[0]).is_absolute(), "native setup interpreter missing")
                candidates.append(args)
            matches = []
            for row in v.child_snapshot(process.pid):
                path = Path("/proc") / str(row["pid"]) / "cmdline"
                try:
                    with path.open("rb") as stream:
                        raw = stream.read(65537)
                except FileNotFoundError:
                    continue
                require(len(raw) <= 65536, "owned process command exceeds bound")
                actual = [value.decode() for value in raw.rstrip(b"\0").split(b"\0")] if raw else []
                if actual in candidates:
                    observed = v.descendant_process_stat(row["pid"])
                    require(observed.get("status") == "observed", "matched helper transport identity is uncertain")
                    matches.append({**row, "process_stat": observed})
            return matches

        async def run(self, private, provider, directory, events, process, base_url):
            require(base_url.rstrip("/") == self.origin, "Console listener mismatch")
            report = {"scope": self.scope, "status": "running", "http": [], "intents": [], "timeline": [],
                "forge_preparation": self.forge_observation, "completed_helper": self.completed_helper,
                "login_retirement": "not_observed", "grant_revocation": "not attempted; no displayed service leaf was requested",
                "historical_work_recovery": False, "input_receipt_scope": "Console native runs and tool results only"}
            frames, indices, cursors = {name: [] for name in ("original", "helper")}, {name: {} for name in ("original", "helper")}, {}
            deadline = time.monotonic() + v.ACQUISITION_LIMIT
            enrollment = token = error = None
            expires, scope = int(time.time() + v.ACQUISITION_LIMIT + 60), "installed-reset-" + uuid.uuid4().hex

            def request(method, path, body=None, *, bearer=True, expected=200, request_limit=10):
                require(time.monotonic() < deadline and process.poll() is None, "reset Console deadline/owner ended")
                headers = {"Accept": "application/json"}
                if bearer:
                    headers["Authorization"] = "Bearer " + token
                if body is not None:
                    headers.update({"Origin": self.origin, "Content-Type": "application/json"})
                    report["intents"].append({"method": method, "path": path, "body": copy.deepcopy(body)})
                    v.save_json(directory / "reset-intents.json", report["intents"])
                connection = HTTPConnection("127.0.0.1", self.port, timeout=min(request_limit, max(.1, deadline-time.monotonic())))
                try:
                    connection.request(method, path, body=None if body is None else v.canonical(body), headers=headers)
                    response = connection.getresponse()
                    raw = response.read(4*1024*1024+1)
                    require(len(raw) <= 4*1024*1024, "Console reply exceeds bound")
                    value = v.unique_json(raw)
                    report["http"].append({"method": method, "path": path, "request": body,
                                            "status": response.status, "response": value})
                    require(response.status == expected, "actual Console request refused: " + path)
                    return value
                finally:
                    connection.close()
            async def http(*args, **kwargs):
                return await asyncio.to_thread(request, *args, **kwargs)
            async def observed_reset(body):
                # The native guard may prepare for 60s, then connect for 10s.
                # Keep ordinary HTTP reads at 10s and the original case deadline.
                started = time.monotonic()
                window = min(75.0, max(.1, deadline-started))
                observation = report["reset_observation"] = {
                    "limit_seconds": window, "samples": [], "request_joined": False,
                    "scope": "Owned process counters only; progress is not readiness or completion."}
                owned = v.descendant_process_stat(process.pid)
                task = asyncio.create_task(http("POST", "/console/rpc", body, request_limit=window))
                def sample():
                    row = {"elapsed_seconds": round(time.monotonic()-started, 3),
                           "gateway": v.descendant_process_stat(process.pid)}
                    if row["gateway"].get("start_ticks") == owned.get("start_ticks") and owned.get("status") == "observed":
                        try:
                            with Path(f"/proc/{process.pid}/stat").open("rb") as stream:
                                raw = stream.read(4097)
                            fields = raw.rpartition(b")")[2].split()
                            if len(raw) <= 4096 and int(fields[19]) == owned["start_ticks"]:
                                row["cpu_ticks"] = {"user": int(fields[11]), "system": int(fields[12])}
                            with Path(f"/proc/{process.pid}/io").open("rb") as stream:
                                raw = stream.read(4097)
                            if len(raw) <= 4096:
                                fields = dict(line.split(b":", 1) for line in raw.splitlines())
                                row["io"] = {key: int(fields[key.encode()]) for key in ("rchar", "syscr", "read_bytes")}
                        except (OSError, ValueError, KeyError, IndexError):
                            row["counters"] = "unavailable"
                    try:
                        row["descendants"] = [{**child, "state": v.descendant_process_stat(child["pid"])}
                                              for child in v.child_snapshot(process.pid)]
                    except Exception:
                        row["descendants"] = "unavailable"
                    observation["samples"].append(row)
                    v.save_json(directory / "reset-observation.json", observation)
                try:
                    while not task.done():
                        if len(observation["samples"]) < 40:
                            sample()
                        await asyncio.wait({task}, timeout=2)
                    reply = await task
                    require(time.monotonic() < deadline, "reset Console deadline ended")
                    return reply
                finally:
                    # Retain and join the same HTTP observer before host cleanup.
                    try:
                        if not task.done():
                            await asyncio.shield(task)
                    finally:
                        observation["request_joined"] = task.done()
                        observation["elapsed_seconds"] = round(time.monotonic()-started, 3)
                        sample()

            async def timeline(identity):
                page = await http("GET", "/console/timeline?" + urlencode({"identity": identity, "mode": "since", "limit": 400,
                    **({"after": cursors[identity]} if cursors.get(identity) else {})}))
                require(page.get("available", True) is True and isinstance(page.get("frames"), list), "timeline unavailable")
                report["timeline"].append({"identity": identity, "page": page})
                for raw in page["frames"]:
                    frame = console.frame_record(raw)
                    index = indices[identity].get(frame["id"])
                    if index is None:
                        require(len(frames[identity]) < 4000, "timeline exceeds bound")
                        indices[identity][frame["id"]] = len(frames[identity]); frames[identity].append(frame)
                    else:
                        old = frames[identity][index]
                        require(all(old.get(key) == frame.get(key) for key in
                            ("identity", "session_id", "run_id", "kind", "source", "source_event_id")), "frame owner changed")
                        frames[identity][index] = frame
                if page["frames"]:
                    cursors[identity] = page.get("next_cursor") or page.get("latest_cursor")
                    require(nonempty(cursors[identity]), "timeline continuation missing")
            try:
                enrollment = private.begin_console_login(v.SDK.ConsoleLoginSelection(issuer=console.ISSUER, subject=console.SUBJECT,
                    login_scope_id=scope, expires_at_ms=expires*1000), timeout=10)
                await asyncio.to_thread(enrollment.result)
                token = console.signed_login(scope, expires)
                report["login_enrollment"] = {"id": enrollment.enrollment_id, "acknowledged": True}
                await http("GET", "/console/experience", bearer=False, expected=401)
                before = await http("GET", "/console/experience")
                require(before["access"]["can_administer"] is True, "fixture operator lacks native reset authority")
                rows = {row["identity"]: row for row in before["identity_status"]["rows"]}
                require(set(rows) == {"helper", "original"} and all(row["progress"]["in_flight_work"] == 0
                    and row["progress"]["run_state"] == "idle" for row in rows.values()), "fresh native members are not idle")
                old = rows["helper"]["session_id"]
                require(nonempty(old), "old helper session missing")
                await timeline("helper"); await timeline("original")
                require(not any(f.get("kind") in ("run_started", "user_input", "assistant_message", "tool_call_requested",
                    "tool_result_received") for fs in frames.values() for f in fs), "fresh history is not empty")
                self.observe_children()
                old_transports = self.helper_transport_snapshot(process)
                require(not self.local_round_requests(), "initial construction must not offer a successor round")
                report["old_helper_transport"] = {"matching_before_reset": old_transports,
                    "observation": "started_before_reset" if old_transports else "not_observed_in_pre_reset_snapshot",
                    "absence_scope": "A missing snapshot match is not proof that no transport ever started.",
                    "joined": False}
                report["pre_reset"] = {"experience": before, "empty_model_history": True,
                    "timeline": copy.deepcopy(frames), "descendants": v.child_snapshot(process.pid),
                    "transport_scope": "descendants observed; no old-helper transport start/join inferred from empty model history"}
                with provider.lock:
                    require(not provider.requests and not provider.errors and not provider.enabled, "model ran before reset")
                self.bundle.authenticate()
                request_id = "helper-reset-" + uuid.uuid4().hex
                reply = await observed_reset({"jsonrpc": "2.0", "id": request_id,
                    "method": "meta/reset", "params": {"identity": "helper"}})
                identities = await http("GET", "/console/identities")
                reset = checked_reset_result(reply, request_id, old, rows["original"]["session_id"], identities)
                report["reset"] = {"old_session_id": old, "result": reset, "identities": identities}
                await timeline("helper")
                require(not any(f.get("kind") in ("run_started", "user_input", "assistant_message",
                    "tool_call_requested", "tool_result_received") for f in frames["helper"]),
                    "reset replayed work or introduced new helper history")
                report["post_reset_empty_history"] = copy.deepcopy(frames["helper"])
                self.observe_children()
                old_after = [{"pid": row["pid"], "before": row["process_stat"],
                              "after": v.descendant_process_stat(row["pid"])} for row in old_transports]
                report["old_helper_transport"].update(
                    matching_after_reset=self.helper_transport_snapshot(process), old_identity_observations=old_after,
                    joined=False,
                    settlement_scope="Reset observations do not attest transport joins. The existing final host shutdown checks all observed children.")
                self.bundle.authenticate()
                with provider.lock:
                    require(not provider.requests and not provider.errors, "reset introduced model work")
                    provider.catalog_deadline = deadline
                    provider.enabled = True
                accepted = await http("POST", "/console/send", {"identity": "original", "content": REQUEST,
                    "origin": "console", "origin_kind": "operator", "idempotency_key": "reset-find-" + uuid.uuid4().hex})
                require(accepted["status"] == "accepted" and accepted["identity"] == "original"
                        and accepted["session_id"] == rows["original"]["session_id"]
                        and all(nonempty(accepted.get(key)) for key in ("interaction_id", "input_frame_id")), "Send was not admitted")
                report["accepted"] = accepted
                while time.monotonic() < deadline:
                    await timeline("original"); await timeline("helper")
                    with provider.lock:
                        require(not provider.errors, "controlled reset provider refused actual request")
                        results = copy.deepcopy(provider.results)
                    require(not any(f.get("session_id") == old and f.get("kind") in
                        ("run_started", "assistant_message", "tool_call_requested", "tool_result_received")
                        for f in frames["helper"]), "old helper history received new work")
                    completed = self.completion(frames, accepted, reset, results)
                    if completed is not None:
                        experience = await http("GET", "/console/experience")
                        live = {row["identity"]: row for row in experience["identity_status"]["rows"]}
                        if all(live[name]["progress"]["in_flight_work"] == 0 and live[name]["progress"]["run_state"] == "idle"
                               for name in ("helper", "original")):
                            self.bundle.authenticate()
                            offers = self.local_round_requests()
                            require(not offers, "idle Initial reuse must not enter a successor offer")
                            old_observations = [{"pid": row["pid"], "before": row["process_stat"],
                                "after": v.descendant_process_stat(row["pid"])} for row in old_transports]
                            require(all(row["after"] == {"status": "unavailable", "reason": "not_found"}
                                or (row["after"].get("status") == "observed"
                                    and row["after"]["start_ticks"] != row["before"]["start_ticks"])
                                for row in old_observations), "prior helper transport remains after successor use")
                            report.update(status="passed", completion=completed, final_experience=experience,
                                results=results, old_session_received_no_new_work=True,
                                local_round_requests=offers, old_transport_after_completion=old_observations,
                                local_round_scope="Zero successor offers for idle Initial reuse; exact NotSpawned and replacement Initial settlement receipts are required after joined shutdown.")
                            break
                    await asyncio.sleep(.1)
                require(report["status"] == "passed", "native reset catalog/schema completion was not observed")
            except BaseException as caught:
                error = caught; report.update(status="failed", error=repr(caught)); raise
            finally:
                with provider.lock:
                    report["catalog_searches"] = [{"id": search_call_id(i), "result": copy.deepcopy(provider.results[search_call_id(i)])}
                        for i in range(512) if search_call_id(i) in provider.results]
                    provider.deadline = 0
                if enrollment is not None:
                    try:
                        retired = await asyncio.to_thread(enrollment.retire, timeout=10)
                        report["login_retirement"] = dataclasses.asdict(retired)
                        require(retired.fenced and retired.reservations_drained and not retired.uncertain,
                                "Console login retirement did not settle")
                    except BaseException as caught:
                        report.update(status="failed", retirement_error=repr(caught))
                        if error is None:
                            self.final_report = report; v.save_json(directory / "console-journey.json", report); raise
                self.final_report = report
                v.save_json(directory / "console-journey.json", report)
            events.add("installed_helper_reset_catalog_completed", **report["completion"])
            return report

        def check_final_artifacts(self, directory, report):
            self.check_sources()
            if report.get("status") != "passed":
                return
            require(report.get("shutdown_response", {}).get("result", {}).get("host_cleanup_completed") is True
                    and report.get("sdk_readers_joined") is True and report.get("provider_joined") is True,
                    "cold source evidence requires the real joined shutdown")
            require(self.local_round_requests() == self.final_report["local_round_requests"],
                    "retained local offer changed during actual shutdown")
            raw = v.exact_bytes(directory / "gateway.stderr", report["gateway_stderr"]["sha256"])
            records = []
            for line in raw.splitlines():
                if b'"connect_reset_attempt_v1"' in line:
                    require(len(line) <= 8192 and len(records) < 256, "idle diagnostic output exceeds source bound")
                    records.append(v.unique_json(line))
            report["idle_initial_settlement"] = checked_idle_initial_settlement(records,
                self.final_report["reset"]["old_session_id"], self.final_report["reset"]["result"]["session_id"],
                self.completed_helper["member"])
            locator = self.preparation_artifacts / "prepared/local-session/round-handoff.json"
            require(not locator.exists() and not locator.is_symlink(),
                    "same local round owner did not remove its completed rendezvous")
            realm, owner = self.source_owner["native_host"]["realm"], self.source_owner["deployment"]
            database = directory / "deployment/shared/data/toolkit-native" / hashlib.sha256(realm.encode()).hexdigest() / "originals.sqlite3"
            require(database.is_file() and not database.is_symlink(), "exact native originals store missing")
            before = {p.name: v.artifact_pin(p) for p in (database, Path(str(database)+"-wal"), Path(str(database)+"-shm")) if p.exists()}
            connection = sqlite3.connect(database.resolve().as_uri()+"?mode=ro", uri=True, timeout=2)
            cutoff = time.monotonic()+5
            try:
                connection.execute("PRAGMA query_only=ON")
                connection.set_progress_handler(lambda: int(time.monotonic() >= cutoff), 1000)
                require(connection.execute("PRAGMA user_version").fetchone()[0] == 1, "Toolkit store schema changed")
                rows = connection.execute("SELECT id,revision,digest,data FROM toolkit_records WHERE realm=? AND owner=? LIMIT 2",
                    (realm, "toolkit/helper-originals/"+owner)).fetchall()
                require(len(rows) == 1, "fresh helper source is absent or ambiguous")
                key, revision, digest, raw = rows[0]
                require(isinstance(raw, str) and len(raw.encode()) <= 2*1024*1024, "helper source exceeds bound")
                record = v.unique_json(raw.encode())
                require(record["schema"] == "meerkat_toolkit.helper_original" and record["version"] == 3
                        and revision == 2 and hashlib.sha256(v.canonical(record)).hexdigest() == digest,
                        "helper source schema/revision/digest mismatch")
                body = record["body"]
                require(body["state"]["phase"] == "bound", "helper source did not bind")
                binding, association = body["state"]["binding"], body["state"]["association"]
                facts = body["facts"]
                require(facts["realm"] == realm and facts["operation"]["acquisition"]["id"] ==
                        self.final_report["results"][FIND_ID]["operation"]
                        and facts["selection"]["member"]["member"] == self.completed_helper["member"]
                        and facts["selection"]["helper"]["catalog_capability"] == self.completed_helper["catalog_capability"],
                        "historical helper source differs from the observed acquisition or selected helper")
                basis = binding["authority_basis"]
                parent = self.completed_helper["catalog_capability"]["parent"]
                require(binding["target"]["context"] == self.final_report["reset"]["result"]["session_id"]
                        and association["target"] == binding["target"]
                        and association["authority_basis"] == basis and basis["kind"] == "targeted_grant_lineages"
                        and all(target in basis["targets"] for target in self.completed_helper["operation_basis"]["targets"])
                        and isinstance(basis["base"], list) and len(basis["base"]) == len(parent)+1
                        and basis["base"][:-1] == parent and basis["base"][-1] not in parent
                        and binding["logical_executor"] == self.completed_helper["logical_executor"],
                        "cold helper source does not retain the actual successor parent and new-session child")
                evidence = {"scope": "historical owner record only; no current-grant, private native input receipt or revocation claim",
                    "key": {"realm": realm, "owner": "toolkit/helper-originals/"+owner, "id": key},
                    "revision": revision, "digest": digest, "record": record}
            finally:
                connection.close()
            after = {p.name: v.artifact_pin(p) for p in (database, Path(str(database)+"-wal"), Path(str(database)+"-shm")) if p.exists()}
            require(before[database.name] == after[database.name], "cold read changed the database bytes")
            evidence.update(database_before=before, database_after=after,
                sidecar_scope="mode=ro may create WAL/SHM bookkeeping; exact before/after retained, no cleanup mutation")
            report["helper_source_lineage"] = v.new_artifact(directory / "helper-reset-source.json", v.canonical(evidence))
            report["historical_catalog_children_revoked"] = False

    return ResetScenario()
