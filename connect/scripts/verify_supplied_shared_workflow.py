#!/usr/bin/env python3
"""Opt-in supplied toolkit_gateway shared workflow acceptance.

Build nothing. Use the existing installed-product startup, controlled model,
protected review, actual HN read, native audit and joined shutdown. P/G/children
are commissioned through the supplied private SDK on that same gateway. The
fixture supplies application selections and durable desired-roster publication;
it does not replace the native launcher, issue grants or write native records.
The ordinary-customizer-only mode stops after the existing class Add/respawn
proof, requires actual after_create context, and performs no workflow or HN read.
The helperless-restore mode starts with no members, admits one real class member,
then checks its callback again after ordinary installed process Restore.
The separate respawn-shared-g opt-in inserts one real G respawn between A's
finalized context and fresh B work. It does not exercise selected-preparation
replay, cold restoration or direct invocation of a retired setup callback.
The cold-shared-restore opt-in instead closes the original process, restores
its exact completed application and G helper, admits fresh B, then cancels A
through its restored settled selected preparation before B effects. A's package is a
controlled no-build fixture; B separately performs the actual public HN read.
The separate --both-selected-success mode uses the existing controlled local
arithmetic package: two independently granted callers share one selected install
and each completes a real selected_add continuation. It makes no HN/auth claim.

Its separate --withhold-p-completion fault blocks only A's completion metadata
until the physical install input finalizes, then requires existing-owner repair.
"""
from __future__ import annotations

import argparse
import asyncio
import copy
from contextlib import nullcontext
import dataclasses
import hashlib
import importlib
import importlib.util
import json
import os
from pathlib import Path
import shlex
import socket
import sqlite3
import stat
import sys
import threading
import time
import tomllib
import uuid


def load_shared():
    path = Path(__file__).with_name("verify_shared_driver.py")
    spec = importlib.util.spec_from_file_location("supplied_shared_driver", path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


s = load_shared()
v, c, require = s.v, s.c, s.require
G_ID = "planning-lead"
SELECTED_A_ANSWER = "A's selected fixture installation is joined; no service access was requested."
SELECTED_A_TOOLS = ("setup_state", "setup_inspect_package", "setup_selected_check", "setup_selected_install", "setup_job", "setup_reconcile")
CLASS_ID = "shared-planner"
CYCLE = "supplied-shared-cycle"
ADMISSION = "supplied-g-admission"
C3_ID = "acceptance-ordinary-member"
C3_CLASS = "acceptance-ordinary-class"
C3_ADMISSION = "acceptance-ordinary-admission"
C3_TOOL = "acceptance_nonce_read"
C3_DESCRIPTION = "Return the supplied acceptance nonce without external effects"
C3_SCHEMA = {"type": "object", "properties": {"nonce": {"type": "string"}},
             "required": ["nonce"], "additionalProperties": False}


def c3_text(phase):
    return "acceptance ordinary callback nonce " + phase


async def require_c3_stale_scope(runtime, scope):
    errors = importlib.import_module("meerkat_meta.errors")
    try:
        await runtime.dispatch_text(C3_ID, "stale ordinary scope must never execute",
            expected_scope=scope, correlation_id=str(uuid.uuid4()),
            idempotency_key="acceptance-stale-delivery")
    except errors.StaleScopeError as refused:
        return {"state": "refused", "code": refused.code, "mismatch": refused.mismatch}
    raise AssertionError("old ordinary delivery scope was not refused")


async def require_g_stale_scope(runtime, scope):
    errors = importlib.import_module("meerkat_meta.errors")
    try:
        await runtime.dispatch_text(G_ID, "stale shared G scope must never execute",
            expected_scope=scope, correlation_id=str(uuid.uuid4()),
            idempotency_key="acceptance-stale-shared-g-delivery")
    except errors.StaleScopeError as refused:
        require(refused.mismatch == "runtime_id", "old G scope was not refused at native runtime continuity")
        return {"state": "refused", "code": refused.code, "mismatch": refused.mismatch}
    raise AssertionError("old G delivery scope was not refused")


async def require_child_refusal(private, request, expected):
    try:
        await asyncio.to_thread(private.prepare_workflow_child, request, timeout=30)
    except v.SDK.HostReplyError as error:
        require(error.code is expected,
                f"child refusal expected {expected.value}, observed {error.code.value}")
        return error.code.value
    raise AssertionError("foreign or widened workflow request was accepted")


def require_respawn_callbacks(before, returned, current, identity, session_id, *, allow_pending=False):
    """Validate observations bracketed around one real RPC, not a build reason."""
    require(identity in (C3_ID, G_ID) and isinstance(session_id, str) and session_id,
            "respawn callback selection is invalid")
    profile = C3_CLASS if identity == C3_ID else CLASS_ID
    require(all(isinstance(rows, list) for rows in (before, returned, current))
            and all(isinstance(row, dict) for rows in (before, returned, current) for row in rows)
            and len(before) == 2 and returned[:len(before)] == before
            and current[:len(returned)] == returned,
            "respawn callback observations lost their exact prefix")
    require([row.get("kind") for row in before] == ["build", "created"],
            "fresh Add must have exactly one build and one after_create")
    context = before[0].get("context")
    require(isinstance(context, dict) and set(context) == {"model", "labels", "system_prompt"}
            and isinstance(context["model"], str) and context["model"],
            "respawn callback draft context is unavailable")
    builds, created = [], []
    for row in current:
        require(isinstance(row, dict) and row.get("identity") == identity and row.get("context") == context,
                "respawn callback identity or unchanged draft context differs")
        if row.get("kind") == "build":
            require(set(row) == {"kind", "identity", "profile", "build", "context"}
                    and row["profile"] == profile and type(row["build"]) is int
                    and row["build"] == len(builds) + 1,
                    "respawn build observation has a foreign profile or ordinal")
            builds.append(row)
        else:
            require(set(row) == {"kind", "identity", "session_id", "context"}
                    and row.get("kind") == "created" and row["session_id"] == session_id,
                    "respawn after_create has a foreign session or malformed observation")
            created.append({key: value for key, value in row.items() if key != "kind"})
    in_rpc = returned[len(before):]
    require(in_rpc and in_rpc[0]["kind"] == "build"
            and sum(row["kind"] == "build" for row in in_rpc) in (1, 2),
            "respawn must observe exactly one or two new customize_build calls in its RPC window")
    require(all(row["kind"] == "created" for row in current[len(returned):]),
            "customize_build occurred outside the selected respawn RPC window")
    require(len(created) in (1, 2), "respawn gained an extra after_create callback")
    if len(created) == 1:
        require(allow_pending, "respawn after_create is still pending")
        return None
    return {"identity": identity, "session_id": session_id, "new_builds": len(builds) - 1,
            "latest_build": builds[-1]["build"], "before": copy.deepcopy(before),
            "returned": copy.deepcopy(returned), "observed": copy.deepcopy(current), "created": created}


def require_c3_handler(result, phase, latest_build=2):
    require(result == {"nonce": phase, "handler_build": 1 if phase == "before" else latest_build},
            "ordinary callback did not use this actual construction's handler")


class AcceptanceCustomizer(s.ObservedCustomizer):
    """One explicit test callback; G's actual SDK draft stays untouched."""
    def __init__(self, profile_model=None, shared_profile_model=None):
        super().__init__()
        self.calls = []
        self.profile_model = profile_model
        self.build_contexts = []
        self.created_contexts = []
        self.created_changed = threading.Condition()
        self.shared_profile_model = shared_profile_model
        self.shared_build_contexts = []
        self.shared_created_contexts = []
        self.respawn_events = []

    async def customize_build(self, context, spec, draft):
        await super().customize_build(context, spec, draft)
        if spec.identity == G_ID:
            require(spec.profile == CLASS_ID, "shared callback has a foreign profile")
            self.shared_build_contexts.append({"model": draft.model or self.shared_profile_model,
                "labels": copy.deepcopy(draft.labels), "system_prompt": draft.system_prompt})
            self.record_respawn_event({"kind": "build", "identity": G_ID, "profile": spec.profile,
                "build": len(self.shared_build_contexts), "context": self.shared_build_contexts[-1]})
        if spec.identity != C3_ID:
            return
        require(spec.profile == C3_CLASS, "ordinary callback has a foreign profile")
        self.build_contexts.append({"model": draft.model or self.profile_model,
            "labels": copy.deepcopy(draft.labels), "system_prompt": draft.system_prompt})
        build = sum(row["identity"] == C3_ID for row in self.builds)
        async def read_nonce(arguments):
            require(set(arguments) == {"nonce"} and arguments["nonce"] in ("before", "after"),
                    "ordinary test callback arguments exceeded its declared scope")
            result = {"nonce": arguments["nonce"], "handler_build": build}
            self.calls.append(copy.deepcopy(result))
            return result
        draft.register_tool(C3_TOOL, read_nonce, description=C3_DESCRIPTION, input_schema=copy.deepcopy(C3_SCHEMA))
        self.record_respawn_event({"kind": "build", "identity": C3_ID, "profile": spec.profile,
            "build": build, "context": self.build_contexts[-1]})

    async def after_create(self, identity, session_id, context):
        await super().after_create(identity, session_id, context)
        if identity == C3_ID:
            with self.created_changed:
                self.created_contexts.append({"identity": identity, "session_id": session_id,
                    "context": {"model": context.model, "labels": copy.deepcopy(context.labels),
                                "system_prompt": context.system_prompt}})
                self.record_respawn_event({"kind": "created", **self.created_contexts[-1]})
                self.created_changed.notify_all()
        elif identity == G_ID:
            with self.created_changed:
                self.shared_created_contexts.append({"identity": identity, "session_id": session_id,
                    "context": {"model": context.model, "labels": copy.deepcopy(context.labels),
                                "system_prompt": context.system_prompt}})
                self.record_respawn_event({"kind": "created", **self.shared_created_contexts[-1]})
                self.created_changed.notify_all()

    def record_respawn_event(self, row):
        # Observation only. Callback order, SDK ownership and draft behavior stay unchanged.
        with self.created_changed:
            self.respawn_events.append(copy.deepcopy(row))
            self.created_changed.notify_all()

    def respawn_snapshot(self, identity):
        with self.created_changed:
            return copy.deepcopy([row for row in self.respawn_events if row["identity"] == identity])

    async def wait_respawn(self, before, returned, identity, session_id, deadline, *, failure_observer=None):
        def observed():
            watch = failure_observer.wake_condition(self.created_changed) if failure_observer is not None else nullcontext()
            with watch, self.created_changed:
                while True:
                    if failure_observer is not None:
                        failure_observer.require_alive()
                    proof = require_respawn_callbacks(before, returned, self.respawn_snapshot(identity),
                        identity, session_id, allow_pending=True)
                    if proof is not None:
                        if failure_observer is not None:
                            failure_observer.require_alive()
                        return proof
                    remaining = deadline - time.monotonic()
                    if failure_observer is not None:
                        failure_observer.require_alive()
                    require(remaining > 0, "actual respawn after_create deadline elapsed")
                    self.created_changed.wait(remaining)
        proof = await asyncio.to_thread(observed)
        if failure_observer is not None:
            failure_observer.require_alive()
        return proof

    def verify_respawn(self, proof):
        current = require_respawn_callbacks(proof["before"], proof["returned"],
            self.respawn_snapshot(proof["identity"]), proof["identity"], proof["session_id"])
        require(current == proof, "respawn callback snapshot changed before or after actual next-turn work")
        return current

    async def wait_shared_created(self, count, session_id, deadline, *, failure_observer=None):
        def observed():
            watch = failure_observer.wake_condition(self.created_changed) if failure_observer is not None else nullcontext()
            with watch, self.created_changed:
                ready = self.created_changed.wait_for(lambda:
                    (failure_observer is not None and failure_observer.failed.is_set())
                    or len(self.shared_created_contexts) >= count,
                    timeout=max(0, deadline - time.monotonic()))
                if failure_observer is not None:
                    failure_observer.require_alive()
                require(ready, "actual G after_create deadline elapsed")
                return copy.deepcopy(self.shared_created_contexts)
        rows = await asyncio.to_thread(observed)
        if failure_observer is not None:
            failure_observer.require_alive()
        require(len(rows) == len(self.shared_build_contexts) == count
                and all(row["identity"] == G_ID and row["session_id"] == session_id for row in rows)
                and all(isinstance(row["context"]["model"], str) and row["context"]["model"] for row in rows)
                and [row["context"] for row in rows] == self.shared_build_contexts,
                "actual G callbacks differ from the unchanged drafts or canonical session")
        return rows

    async def wait_created(self, count, session_id, deadline, *, failure_observer=None):
        # Meta owns notification timing after the lifecycle operation returns.
        # Observe the real callback; never retry a build or invoke it ourselves.
        def observed():
            watch = failure_observer.wake_condition(self.created_changed) if failure_observer is not None else nullcontext()
            with watch, self.created_changed:
                ready = self.created_changed.wait_for(lambda:
                    (failure_observer is not None and failure_observer.failed.is_set())
                    or len(self.created_contexts) >= count,
                    timeout=max(0, deadline - time.monotonic()))
                if failure_observer is not None:
                    failure_observer.require_alive()
                require(ready, "actual after_create callback deadline elapsed")
                return copy.deepcopy(self.created_contexts)
        rows = await asyncio.to_thread(observed)
        if failure_observer is not None:
            failure_observer.require_alive()
        require(len(rows) == len(self.build_contexts) == count
                and all(row["identity"] == C3_ID and row["session_id"] == session_id for row in rows)
                and all(isinstance(row["context"]["model"], str) and row["context"]["model"] for row in rows)
                and [row["context"] for row in rows] == self.build_contexts,
                "actual after_create context differs from the built draft or selected profile")
        return rows


class AcceptanceModel(s.Model):
    def __init__(self, directory, events, *, ordinary_only=False, selected_a=False):
        super().__init__(directory, events)
        self.c3_histories = {}
        self.c3_after_handler_build = 2  # Cold/helperless Restore keeps its original exact second build.
        self.ordinary_only = ordinary_only
        self.selected_a = selected_a
        self.selected_request = None
        self.selected_result = None
        if selected_a:
            self.selected_root = Path(__file__).parent / "fixtures/shared-selected-mcp"
            self.selected_fixture = s.load("cold_selected_fixture", self.selected_root / "setup_fixture.py")
            v.save_json(directory / "selected-fixture-source.json", {
                "scope": "controlled no-build package; real selected install and cleanup, no discovery or account claim",
                "files": [v.artifact_pin(self.selected_root / name) for name in
                    ("setup_fixture.py", "FIXTURE.json", *self.selected_fixture.PACKAGE_FILES)],
                "archive_producer": v.artifact_pin(self.selected_root.parent / "source-built-mcp/setup_fixture.py")})
        self.original_gates = {tag: {"entered": threading.Event(), "released": threading.Event(),
            "approved": False, "deadline": None} for tag in ("a", "b")}

    def original_a_context(self, body, results, context):
        if not self.selected_a:
            return super().original_a_context(body, results, context)
        return self.selected_context(body, results, context, "a")

    def selected_context(self, body, results, context, tag):
        names = {row["function"]["name"] for row in body.get("tools", [])}
        if not set(SELECTED_A_TOOLS).issubset(names):
            searches = [value for key, value in results.items() if key.startswith(f"{tag}-selected-search-")]
            if searches and set(SELECTED_A_TOOLS).issubset({row["name"] for row in searches[-1]["results"]}):
                return self.call(body, f"{tag}-selected-load", "tool_catalog_load", {"names": list(SELECTED_A_TOOLS)})
            return self.call(body, f"{tag}-selected-search-{self.sequence}", "tool_catalog_search", {"query": "setup", "limit": 50})
        if f"{tag}-selected-state" not in results:
            return self.call(body, f"{tag}-selected-state", "setup_state", {}, context)
        state = results[f"{tag}-selected-state"]["selected_installation"]
        require("node" in state["profiles"], "actual shared helper lacks selected Node installation")
        if f"{tag}-selected-package" not in results:
            return self.call(body, f"{tag}-selected-package", "setup_inspect_package", {
                "package_json": self.selected_fixture.wire(self.selected_fixture.package_document()).decode()}, context)
        if self.selected_request is None:
            self.selected_request = self.selected_fixture.stage_and_check_request(self.selected_root,
                state["staging_root"], results[f"{tag}-selected-package"])
        if f"{tag}-selected-check" not in results:
            return self.call(body, f"{tag}-selected-check", "setup_selected_check", self.selected_request, context)
        checked = results[f"{tag}-selected-check"]
        require(checked["scope"] == "selected_software_inputs_only" and checked["profile"] == "node"
                and checked["component"] == checked["mcp_name"] == self.selected_fixture.COMPONENT
                and checked["declared_service"] == {"authentication": {"kind": "none"}, "operation_tools": {"add": "selected_add"}},
                "selected fixture check differs from its no-auth package")
        if f"{tag}-selected-install" not in results:
            return self.call(body, f"{tag}-selected-install", "setup_selected_install",
                {key: checked[key] for key in ("validation_id", "effect_sha256")}, context)
        job = results[f"{tag}-selected-install"]["job"]["id"]
        jobs = [value["job"] for value in results.values() if isinstance(value.get("job"), dict) and value["job"].get("id") == job]
        require(jobs[-1]["status"] in ("running", "succeeded"), "selected fixture worker did not succeed")
        if jobs[-1]["status"] == "running":
            return self.call(body, f"{tag}-selected-poll-{self.sequence}", "setup_job", {"job_id": job}, context)
        if f"{tag}-selected-reconcile" not in results:
            return self.call(body, f"{tag}-selected-reconcile", "setup_reconcile", {"job_id": job}, context)
        observed = results[f"{tag}-selected-reconcile"]["observation"]
        installed = observed["result"]
        require(observed["job_id"] == job and observed["status"] == "matched"
                and installed["component"] == checked["component"] and installed["effect_sha256"] == checked["effect_sha256"]
                and installed["declared_service"] == checked["declared_service"]
                and valid_sha256(installed["receipt_sha256"])
                and jobs[-1]["result"]["scope"] == "observed_software_installation_only"
                and jobs[-1]["result"]["readback_scope"] == installed["scope"]
                and all(jobs[-1]["result"].get(key) == value for key, value in installed.items() if key != "scope"),
                "selected fixture did not retain its actual installer/readback result")
        self.selected_result = {"checked": copy.deepcopy(checked), "job": copy.deepcopy(jobs[-1]),
            "readback": copy.deepcopy(observed), "context": copy.deepcopy(context)}
        return self.done(SELECTED_A_ANSWER)

    def release_original_responses(self, *, abort=False, tags=("a", "b")):
        with self.lock:
            require(tags and len(set(tags)) == len(tags) and set(tags) <= set(self.original_gates),
                    "unknown or repeated original response selection")
            require(abort or all(self.original_gates[tag]["approved"] for tag in tags),
                    "selected original bound replays must succeed before releasing responses")
            for tag in (self.original_gates if abort else tags):
                gate = self.original_gates[tag]
                if abort and not gate["released"].is_set():
                    gate["approved"] = False
                gate["released"].set()

    def response(self, body):
        texts = [row.get("content") for row in body["messages"] if row["role"] == "user"]
        phase = next((value for value in ("after", "before") if c3_text(value) in texts), None)
        if phase is None:
            require(not self.ordinary_only, "focused callback proof refused a workflow or public-read model request")
            if s.P_TEXT not in texts and s.G_TEXT not in texts:
                tags = [tag for tag in self.original_gates if self.request(tag) in texts]
                require(len(tags) == 1, "model request has no exact original scheduling gate")
                gate = self.original_gates[tags[0]]
                if not gate["released"].is_set():
                    require(gate["deadline"] is not None and not gate["entered"].is_set(),
                            "original model entered without its one selected submission")
                    gate["entered"].set()
                    # Hold only the model response, not a native authority or
                    # the provider lock. The bound replay still may refuse.
                    self.lock.release()
                    try:
                        require(gate["released"].wait(max(0, gate["deadline"] - time.monotonic())),
                                "original bound replay did not release its model response")
                    finally:
                        self.lock.acquire()
                require(gate["approved"], "original response was released without a successful bound replay")
                require(time.monotonic() < gate["deadline"], "original bound replay response deadline elapsed")
            return super().response(body)
        self.sequence += 1
        results = {}
        for row in body["messages"]:
            key = row.get("tool_call_id")
            if row["role"] == "tool" and key in self.issued:
                value = v.unique_json(row["content"].encode())
                require(not value.get("error"), "actual ordinary callback tool refused")
                self.results[key] = results[key] = value
        key = "c3-" + phase + "-call"
        names = {row["function"]["name"] for row in body.get("tools", [])}
        if key not in results:
            if C3_TOOL not in names:
                if any(name.startswith("c3-" + phase + "-search-") for name in results):
                    return self.call(body, f"c3-{phase}-load-{self.sequence}", "tool_catalog_load", {"names": [C3_TOOL]})
                return self.call(body, f"c3-{phase}-search-{self.sequence}", "tool_catalog_search", {"query": C3_TOOL, "limit": 10})
            return self.call(body, key, C3_TOOL, {"nonce": phase})
        require_c3_handler(results[key], phase, self.c3_after_handler_build)
        # Conversation history comes from the actual native model request;
        # build-time system prompts are not persisted transcript messages.
        self.c3_histories[phase] = copy.deepcopy([row for row in body["messages"]
            if row["role"] in ("user", "assistant", "tool")])
        return self.done("Ordinary callback returned " + phase)


def require_c3_transition(before, after, result):
    """Validate native observations; a callback replay alone is insufficient."""
    old, new = before["status"], after["status"]
    require(old["identity"] == new["identity"] == result["identity"] == C3_ID
            and old["state"] == new["state"] == "active"
            and old["session_id"] == new["session_id"] == result["session_id"]
            and type(old["generation"]) is int
            and new["generation"] == result["generation"] == old["generation"] + 1
            and old["agent_runtime_id"] != new["agent_runtime_id"]
            and result["agent_runtime_id"] == new["agent_runtime_id"]
            and result["cleanup_warning"] is None and result["live_respawn_warning"] is None,
            "respawn did not rebuild the same durable session at its successor generation")
    require_ordinary_owner(before, after)


def require_ordinary_owner(before, after):
    old = before["status"]
    for observed in (before, after):
        require(observed["tools"]["identity"] == C3_ID
                and observed["tools"]["session_id"] == old["session_id"]
                and C3_TOOL in observed["tools"]["tools"]
                and observed["canonical"]["authorization"] == {"state": "succeeded"}
                and observed["canonical"]["record"]["phase"]["kind"] == "active",
                "ordinary class is not both current and advertised by the real session")
    a, b = before["canonical"]["record"], after["canonical"]["record"]
    route = a.get("route")
    require(isinstance(route, dict) and isinstance(route.get("controller_parent"), list)
            and route["controller_parent"] and b.get("route") == route,
            "rebuild lost or changed the exact native issued controller route")
    require(all(a.get(field) == b.get(field) for field in ("id", "class", "instance", "workflow", "installed_surface", "native_surface"))
            and a["id"] == C3_ADMISSION and a.get("workflow") is None
            and sorted(before["tools"]["tools"]) == sorted(after["tools"]["tools"]),
            "rebuild changed the canonical class owner, admission or actual surface")


def require_c3_history(before, after):
    require(len(after) >= len(before) and after[:len(before)] == before
            and any(row.get("tool_call_id") == "c3-before-call" for row in before)
            and any(row.get("content") == "Ordinary callback returned before" for row in after)
            and any(row.get("tool_call_id") == "c3-after-call" for row in after),
            "same-session ordinary history or actual before/after tool result was lost")


def context_delivery(directory, source, workflow, session):
    """Read the exact successful transfer receipt, never infer G work from A's answer."""
    path = directory / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
    with sqlite3.connect(path.resolve(strict=True).as_uri() + "?mode=ro", uri=True) as db:
        rows = db.execute("SELECT data FROM toolkit_effects WHERE realm=? ORDER BY owner,record_id,id LIMIT 257",
                          ("mob.fixture",)).fetchall()
    require(len(rows) < 257, "bounded context receipt evidence exceeded")
    selected = []
    for (raw,) in rows:
        effect = v.unique_json(raw.encode())
        receipt = ((effect.get("outcome") or {}).get("Succeeded") or {}).get("receipt")
        if not receipt or receipt.get("schema") != "toolkit.workflow-context-input":
            continue
        body = receipt["body"]
        if body["membership"]["identity"]["source"] != source:
            continue
        require(effect["status"] == "Succeeded" and receipt["version"] == 1
                and body["membership"]["workflow"] == workflow and body["session"] == session,
                "selected context receipt changed workflow or canonical G session")
        selected.append(body)
    require(len(selected) <= 1, "ambiguous native context delivery")
    return selected[0] if selected else None


async def wait_finalized_g_close(directory, workflow, session, input_id, deadline, observation, *, failure_observer):
    """Observe the exact owner close receipt while recovery is still running."""
    key = workflow["key"]
    path = directory / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
    require(key["realm"] == "mob.fixture" and session and input_id, "invalid original G close selection")
    observation.update(key={"record": key, "id": "close"}, session_id=session, input_id=input_id, effect=None)
    while time.monotonic() < deadline:
        failure_observer.require_alive()
        with sqlite3.connect(path.resolve(strict=True).as_uri() + "?mode=ro", uri=True) as db:
            rows = db.execute("SELECT data FROM toolkit_effects WHERE realm=? AND owner=? AND record_id=? AND id=? LIMIT 2",
                (key["realm"], key["owner"], key["id"], "close")).fetchall()
        require(len(rows) <= 1, "ambiguous original G close effect")
        effect = v.unique_json(rows[0][0].encode()) if rows else None
        observation["effect"] = effect
        if effect is not None:
            intent = effect.get("intent") or {}
            require(effect.get("key") == observation["key"] and intent.get("id") == "close"
                    and intent.get("kind") == "setup_workflow_close"
                    and intent.get("request") == {"schema": "meerkat_toolkit.setup_workflow.close", "version": 1,
                        "body": {"workflow": workflow, "payload": workflow}},
                    "original G close effect changed workflow or request")
            status = effect.get("status")
            if status == "Succeeded":
                require(effect.get("outcome") == {"Succeeded": {"receipt": {
                    "schema": "toolkit.setup-workflow-close", "version": 1,
                    "body": {"state": "source_fenced_and_finalized", "session_id": session, "input_id": input_id}}}},
                    "original G close has no exact finalized receipt")
                failure_observer.require_alive()
                return effect
            require(status in ("Pending", "InFlight", "Uncertain"), "original G close failed or has unknown status")
        await asyncio.sleep(0.05)
    failure_observer.require_alive()
    raise AssertionError("original G close did not finalize before shutdown")


def finalized_context(rows, delivery, expected_text):
    selected = [row for row in rows if row["input_id"] == delivery["input"]]
    require(len(selected) <= 1, "duplicate context input")
    if not selected or selected[0].get("terminal_completion") is None:
        return None
    row, completion = selected[0], selected[0]["terminal_completion"]
    if completion["phase"]["phase"] != "finalized":
        return None
    require(completion["input_id"] == completion["owner_input_id"] == delivery["input"]
            and completion["completion_input_ids"] == [delivery["input"]]
            and completion["phase"]["finalization"] == "succeeded"
            and completion["outcome"]["completion_type"] == "completed"
            and completion["outcome"]["session_id"] == delivery["session"]
            and completion["outcome"]["text"] == expected_text,
            "G context input did not finalize its own exact successful answer")
    return row


def require_g_transition(before, after, result):
    old, new = before["status"], after["status"]
    require(old["identity"] == new["identity"] == result["identity"] == G_ID
            and old["state"] == new["state"] == "active"
            and old["session_id"] == new["session_id"] == result["session_id"]
            and type(old["generation"]) is int
            and new["generation"] == result["generation"] == old["generation"] + 1
            and old["agent_runtime_id"] != new["agent_runtime_id"] == result["agent_runtime_id"]
            and result["cleanup_warning"] is None and result["live_respawn_warning"] is None,
            "G respawn did not retain identity/session with exactly one new native generation")
    a, b = before["canonical"]["record"], after["canonical"]["record"]
    require(a["id"] == ADMISSION and a["workflow"] is not None
            and all(a.get(field) == b.get(field) for field in
                    ("id", "class", "instance", "workflow", "route", "installed_surface", "native_surface")),
            "G respawn changed canonical admission, owner, workflow or surface")
    for observed in (before, after):
        record = observed["canonical"]["record"]
        require(observed["canonical"]["authorization"] == {"state": "succeeded"}
                and record["phase"]["kind"] == "active"
                and record["phase"]["context"] == old["session_id"]
                and observed["tools"]["identity"] == G_ID
                and observed["tools"]["session_id"] == old["session_id"]
                and set(s.SETUP).issubset(observed["tools"]["tools"]),
                "G is not active with its real shared setup surface")
    require(sorted(before["tools"]["tools"]) == sorted(after["tools"]["tools"]),
            "G respawn changed its advertised native tools")


def selected_shared_physical(directory, observed):
    """Read the physical identity selected by this actual canonical observation.

    This joins immutable evidence to the live process oracle; it grants no
    authority and makes no claim about physical entry or completed cleanup.
    """
    canonical, status = observed["canonical"], observed["status"]
    record = canonical["record"]
    require(canonical["state"] == "observed" and canonical["authorization"] == {"state": "succeeded"}
            and status["identity"] == G_ID and status["state"] == "active"
            and isinstance(status["session_id"], str) and bool(status["session_id"])
            and record["id"] == ADMISSION and record["phase"]["kind"] == "active"
            and record["phase"]["context"] == status["session_id"],
            "physical selection requires this authorized active G observation")
    selection = record.get("workflow_resource")
    require(isinstance(selection, dict) and set(selection) == {"path", "sha256"}
            and valid_sha256(selection["sha256"]), "canonical workflow resource selection required")
    path = Path(selection["path"])
    require(path.is_absolute() and path.resolve(strict=True).is_relative_to(directory.resolve())
            and path.name == "workflow-resource-" + selection["sha256"] + ".json",
            "canonical resource path escaped this acceptance")
    metadata = path.lstat()
    require(stat.S_ISREG(metadata.st_mode) and stat.S_IMODE(metadata.st_mode) == 0o600
            and metadata.st_uid == os.getuid() and metadata.st_nlink == 1,
            "private canonical resource document required")
    raw = v.exact_bytes(path, selection["sha256"])
    document = v.unique_json(raw)
    admission = document["admission"]
    current = copy.deepcopy(record)
    current["phase"] = copy.deepcopy(admission["phase"])
    current.pop("workflow_resource")
    require(document["schema_version"] == 1 and document.get("closed") is None
            and document["session"] == status["session_id"]
            and admission["phase"]["kind"] == "prepared"
            and admission["phase"]["context"] == status["session_id"]
            and admission.get("workflow") is not None and current == admission,
            "canonical resource differs from the current G admission or is closed")
    resources = document["resources"]
    artifact = Path(resources["artifact_directory"])
    require(artifact == path.parent and artifact.resolve(strict=True) == artifact
            and resources["handoff"]["path"] == str(artifact / "prepare.stdout")
            and valid_sha256(resources["handoff"]["sha256"]), "canonical preparer selection differs")
    v.exact_bytes(artifact / "prepare.stdout", resources["handoff"]["sha256"])
    physical = document.get("physical")
    require(isinstance(physical, dict)
            and set(physical) == {"artifact_directory", "owner", "lifetime_sha256", "fence"}
            and physical["artifact_directory"] == str(artifact)
            and valid_sha256(physical["owner"]) and valid_sha256(physical["lifetime_sha256"])
            and isinstance(physical["fence"], dict), "current canonical physical selection required")
    return physical, v.bytes_pin(path, raw)


def shared_peer(directory, gateway_pid, python, *, physical=None, observed=None):
    """Inspect only this gateway's descendants and the exact native preparer selection."""
    association = None
    if observed is not None:
        require(physical is None, "select physical identity from one canonical source only")
        physical, association = selected_shared_physical(directory, observed)
    descendants = v.child_snapshot(gateway_pid)
    require(len(descendants) <= 64, "bounded owned descendant evidence exceeded")
    commands = []
    for row in descendants:
        result = v.subprocess.run(["/bin/ps", "-p", str(row["pid"]), "-o", "command="],
            capture_output=True, text=True, check=False, timeout=5)
        if result.returncode == 1 and not result.stdout.strip():
            continue  # A completed descendant is not a current peer candidate.
        require(result.returncode == 0 and len(result.stdout) <= 16384, "owned process command unavailable")
        argv = shlex.split(result.stdout.strip())
        # env execs the selected interpreter, so its environment prefix is no
        # longer in ps. Never inspect files from a differently selected process.
        if len(argv) >= 4 and argv[:3] == [python, "-I", "-B"] and Path(argv[3]).name == "local_session.py":
            commands.append((row["pid"], argv))
    owners = []
    for pid, argv in commands:
        if "--shared-peers" not in argv:
            continue
        require(argv[4] == "--configuration", "unexpected shared owner command")
        configuration = Path(argv[5])
        require(configuration.is_absolute() and configuration.resolve(strict=True).is_relative_to(directory.resolve())
                and configuration.name == "mcp.json" and configuration.parent.name == "config",
                "shared owner configuration escaped this acceptance")
        artifact = configuration.parents[2]
        handoff = v.unique_json((artifact / "prepare.stdout").read_bytes())["mcpServers"]["toolkit-setup"]
        args = handoff["args"]
        require(handoff["command"] == "/usr/bin/env" and args[:3] ==
                ["-i", "PATH=/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE=1"]
                and argv[:len(args) - 3] == args[3:], "shared owner differs from native pinned preparer")
        suffix = argv[len(args) - 3:]
        lifetime = None
        if physical is None:
            require(len(suffix) in (4, 6) and suffix[:3] == ["--shared-peers", "1", "--shared-close-id"]
                    and valid_sha256(suffix[3])
                    and (len(suffix) == 4 or suffix[4] == "--shared-restore-sha256" and valid_sha256(suffix[5])),
                    "shared owner differs from exact native close/restore selection")
        else:
            # Current native lifetime arguments are joined to the exact fresh
            # association's typed PhysicalSelection. No arbitrary suffix skip.
            require(len(suffix) in (8, 10) and suffix[:3] == ["--shared-peers", "1", "--shared-close-id"]
                    and valid_sha256(suffix[3]) and suffix[4] == "--shared-lifetime"
                    and suffix[6] == "--shared-lifetime-sha256" and valid_sha256(suffix[7])
                    and (len(suffix) == 8 or suffix[8] == "--shared-restore-sha256" and valid_sha256(suffix[9])),
                    "shared owner differs from exact native lifetime/restore selection")
            lifetime_path = Path(suffix[5])
            require(lifetime_path == artifact / ("shared-lifetime-" + suffix[3] + ".json")
                    and physical["artifact_directory"] == str(artifact) and physical["owner"] == suffix[3]
                    and physical["lifetime_sha256"] == suffix[7], "fresh physical owner identity differs")
            meta = lifetime_path.lstat()
            require(stat.S_ISREG(meta.st_mode) and stat.S_IMODE(meta.st_mode) == 0o600
                    and meta.st_uid == os.getuid() and meta.st_nlink == 1, "private native lifetime required")
            lifetime = v.unique_json(v.exact_bytes(lifetime_path, suffix[7]))
            require(lifetime["schema_version"] == 2 and lifetime["owner"] == physical["owner"]
                    and lifetime["artifact_directory"] == str(artifact) and lifetime["fence"] == physical["fence"]
                    and lifetime["command"] == handoff["command"] and lifetime["arguments"] == args
                    and lifetime["peers"] == 1 and lifetime["restored"] == (suffix[9] if len(suffix) == 10 else None)
                    and lifetime["preparer"] == {"prepare.stdout": v.digest(artifact / "prepare.stdout")},
                    "immutable native lifetime differs from actual launch or fresh association")
        flags = dict(zip(args[7::2], args[8::2]))
        for path, digest in ((configuration, flags["--configuration-sha256"]),
                (Path(argv[3]), flags["--helper-sha256"]),
                (Path(argv[3]).with_name("mcp_adapter.py"), flags["--adapter-sha256"]),
                (Path(argv[3]).with_name("backend.py"), flags["--backend-sha256"])):
            require(path.resolve(strict=True).is_relative_to(artifact.resolve()), "shared preparer pin escaped its owner")
            v.exact_bytes(path, digest)
        owners.append((pid, argv, artifact, suffix, lifetime))
    require(len(owners) == 1, "no unique owned shared controller")
    owner_pid, owner_argv, artifact, suffix, lifetime = owners[0]
    if physical is None:
        bridges = [(pid, argv) for pid, argv in commands if len(argv) == 6
                   and argv[:4] == owner_argv[:4] and argv[4] == "--shared-peer"]
    else:
        bridges = [(pid, argv) for pid, argv in commands if len(argv) == 10
                   and argv[:4] == owner_argv[:4] and argv[4] == "--shared-peer"
                   and argv[6:] == ["--shared-lifetime", suffix[5], "--shared-lifetime-sha256", suffix[7]]]
    require(len(bridges) == 1, "no unique owned shared peer")
    bridge_pid, bridge_argv = bridges[0]
    path = Path(bridge_argv[5])
    parent, node = path.parent.lstat(), path.lstat()
    require(path.is_absolute() and path.parent.parent.resolve() == Path("/tmp").resolve()
            and stat.S_ISDIR(parent.st_mode) and stat.S_IMODE(parent.st_mode) == 0o700
            and stat.S_ISSOCK(node.st_mode) and stat.S_IMODE(node.st_mode) == 0o600
            and parent.st_uid == node.st_uid == os.getuid(), "peer path is not the owned private socket")
    result = {"owner_pid": owner_pid, "owner_argv": owner_argv, "bridge_pid": bridge_pid,
            "artifact_directory": str(artifact), "close_id": suffix[3],
            "restore_sha256": (suffix[5] if len(suffix) == 6 else None) if physical is None
                else (suffix[9] if len(suffix) == 10 else None),
            "path": str(path), "inode": [node.st_dev, node.st_ino], "directory_inode": [parent.st_dev, parent.st_ino]}
    if physical is not None:
        result.update(physical=physical, lifetime=lifetime, lifetime_file=v.artifact_pin(Path(suffix[5])))
    if association is not None:
        result["canonical_resource"] = association
    return result


def valid_sha256(value):
    return isinstance(value, str) and len(value) == 64 and all(c in "0123456789abcdef" for c in value)


def require_cold_transport(before, after):
    require(before["owner_pid"] != after["owner_pid"] and before["bridge_pid"] != after["bridge_pid"]
            and not v.alive(before["owner_pid"]) and not v.alive(before["bridge_pid"])
            and before["artifact_directory"] == after["artifact_directory"]
            and before["close_id"] != after["close_id"] and before["restore_sha256"] is None
            and valid_sha256(after["restore_sha256"])
            and before["path"] != after["path"] and before["inode"] != after["inode"],
            "cold Restore did not replace the joined physical owner and peer from its retained artifact")
    # The live Python owner consumes shared-close into this exact one-use
    # claim before opening fresh peers. A new close snapshot does not exist yet.
    path = Path(after["artifact_directory"]) / "prepared/local-session/shared-restore-claim.json"
    v.exact_bytes(path, after["restore_sha256"])
    return v.artifact_pin(path)


def require_retired_transport(before, after):
    require(before["owner_pid"] == after["owner_pid"] and before["owner_argv"] == after["owner_argv"]
            and before["bridge_pid"] != after["bridge_pid"] and before["path"] != after["path"]
            and before["inode"] != after["inode"] and before["directory_inode"] == after["directory_inode"]
            and Path(before["path"]).parent == Path(after["path"]).parent,
            "respawn did not replace only the physical peer under the retained controller")
    # The old bridge can disappear from ancestry snapshots without exiting.
    # A reused or inaccessible PID cannot establish its exit either.
    require(not v.alive(before["bridge_pid"]), "retired shared peer process still exists")
    path = Path(before["path"])
    try:
        node = path.lstat()
    except FileNotFoundError:
        return {"state": "absent", "scope": "retired transport address only; not direct callback invocation"}
    require(stat.S_ISSOCK(node.st_mode) and [node.st_dev, node.st_ino] == before["inode"],
            "retired socket was replaced by an unknown endpoint")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as probe:
        probe.settimeout(2)
        try:
            probe.connect(str(path))
        except ConnectionRefusedError:
            return {"state": "connection_refused", "scope": "retired transport address only; not direct callback invocation"}
    raise AssertionError("retired transport still accepts connections")


def atomic_json(path, value):
    """One fixture application's durable roster write, never a native record."""
    temporary = path.with_suffix(path.suffix + ".next")
    with temporary.open("xb") as stream:
        stream.write(v.canonical(value))
        stream.flush()
        os.fsync(stream.fileno())
    os.replace(temporary, path)
    descriptor = os.open(path.parent, os.O_RDONLY)
    try:
        os.fsync(descriptor)
    finally:
        os.close(descriptor)


class DesiredRoster:
    """The actual SDK roster provider's single application-owned durable file."""
    def __init__(self, directory, initial, *, helperless=False):
        self.path = directory / "application-desired-roster.json"
        self.admitting = directory / "application-admitting-g.json"
        self.initial = copy.deepcopy(initial)
        self.publications = 0
        self.probe_publications = 0
        self.probe_admitting = directory / "application-admitting-ordinary.json"
        require((initial == [] if helperless else len(initial) == 4)
                and all(row["identity"] != G_ID for row in initial),
                "initial roster does not match the selected acceptance mode")
        atomic_json(self.path, initial)

    def selected(self):
        rows = v.unique_json(self.path.read_bytes())
        expected = self.initial + ([self.probe_spec()] if self.probe_publications else []) + ([self.spec()] if self.publications else [])
        require(rows == expected, "application roster changed outside these exact native publications")
        return rows

    @staticmethod
    def probe_spec():
        return {"identity": C3_ID, "profile": C3_CLASS, "runtime_mode_override": "turn_driven"}

    def prepare_probe(self, request):
        require(self.selected() == self.initial and not self.probe_admitting.exists(), "duplicate ordinary admission")
        self.probe_expected = {"selection": {"admission_id": request.id, "identity": C3_ID, "class": C3_CLASS},
                               "bindings": [], "spec": self.probe_spec(), "state": "admitting"}
        atomic_json(self.probe_admitting, self.probe_expected)

    async def publish_probe(self):
        require(not self.probe_publications and v.unique_json(self.probe_admitting.read_bytes()) == self.probe_expected,
                "ordinary publication does not match its durable admitting row")
        atomic_json(self.path, self.selected() + [self.probe_spec()])
        self.probe_publications += 1

    async def retain_empty_mcp(self, value):
        require(value.wire() == {key: self.probe_expected[key] for key in ("selection", "bindings")}
                and v.unique_json(self.probe_admitting.read_bytes()) == self.probe_expected,
                "ordinary no-MCP retention changed exact admission or empty selection")

    async def lookup_empty_mcp(self, selection):
        row = v.unique_json(self.probe_admitting.read_bytes())
        require(row == self.probe_expected and selection.wire() == row["selection"],
                "ordinary MCP lookup did not name its retained application row")
        module = importlib.import_module("meerkat_toolkit.member_classes")
        return module.RetainedMemberMcpBindings(selection, ())

    @staticmethod
    def spec():
        return {"identity": G_ID, "profile": CLASS_ID, "addressability": "internal_only",
                "runtime_mode_override": "turn_driven"}

    def prepare(self, request):
        require(self.selected() == self.initial + ([self.probe_spec()] if self.probe_publications else [])
                and not self.admitting.exists(), "duplicate application admission")
        self.expected = {"admission_id": request.admission_id, "identity": request.identity,
                         "class": request.instance["class"], "spec": self.spec(), "state": "admitting"}
        require(self.expected["identity"] == G_ID and self.expected["class"] == CLASS_ID,
                "foreign application publication selection")
        atomic_json(self.admitting, self.expected)

    async def publish(self):
        require(self.publications == 0 and v.unique_json(self.admitting.read_bytes()) == self.expected
                and self.selected() == self.initial + ([self.probe_spec()] if self.probe_publications else []),
                "publication is not this exact pending application row")
        # Called only by the SDK's actual native listing-required callback.
        # No await or reconcile lock can allow a second listing mutation.
        atomic_json(self.path, self.selected() + [self.spec()])
        self.publications += 1

    async def roster(self, context):
        require(context.mob_definition is not None, "real native roster context missing")
        return [v.META_IDENTITY.DurableAgentSpec.from_dict(row) for row in self.selected()]

    async def compute_edges(self, target_identities, context):
        return []


def authenticated_event(directory, route, text, contributors=(), *, occurrence=None):
    """The fixture application authenticates its explicit host event occurrence."""
    occurrence = occurrence or "supplied-" + route
    facts = {"route": route, "occurrence": occurrence, "content": text,
             "contributors": list(contributors), "source": "explicit supplied-app acceptance request"}
    pin = v.new_artifact(directory / (occurrence + "-event.json"), v.canonical(facts))
    evidence = v.SDK.PrivateSourceEvidence(occurrence, "application-event-v1", pin["sha256"])
    return v.SDK.PrivateAuthenticatedEvent(route, occurrence, occurrence, evidence, evidence,
        tuple(contributors), hashlib.sha256(text.encode()).hexdigest())


def require_admitted(outcome):
    require(outcome.get("state") == "admitted" and outcome.get("deduplicated") is False
            and outcome.get("source_settlement") == "fresh_acceptance_promoted" and outcome.get("turn_ticket"),
            "actual fresh native input admission and source promotion required")


async def submit_and_check_bound_child(private, provider, submission, request, deadline):
    tag = {"original-a": "a", "original-b": "b"}[submission.original.route]
    gate = provider.original_gates[tag]
    approved = False
    try:
        with provider.lock:
            require(gate["deadline"] is None, "original submission was already selected")
            gate["deadline"] = deadline
        admitted = await asyncio.to_thread(private.submit_authenticated_original, submission.original,
            submission.content, submission.transport_authentication, timeout=60)
        require_admitted(admitted)
        require(await asyncio.to_thread(gate["entered"].wait, max(0, min(30, deadline - time.monotonic()))),
                "admitted original did not enter its first actual model request")
        require(time.monotonic() < deadline, "original bound replay deadline elapsed")
        require(await asyncio.to_thread(private.prepare_workflow_child, request, timeout=30) == submission.original,
                "bound exact replay changed source")
        approved = True
        return admitted
    finally:
        if approved:
            with provider.lock:
                gate["approved"] = True
        else:
            # Wake held or late-arriving requests before outer cleanup,
            # without allowing either one to emit a new tool response.
            provider.release_original_responses(abort=True)


async def observe_commission(attempt, directory):
    result = await asyncio.to_thread(attempt.result)
    # Preserve the native response even if publication joining or a later
    # acceptance assertion refuses. This is evidence, not a success projection.
    v.save_json(directory / "supplied-commission-result.json", result)
    await attempt.join_listing()
    return result


def require_commissioned(result, request):
    require(result["state"] == "prepared" and result["admission_id"] == request.admission_id
            and result["parent"] == dataclasses.asdict(request.parent.original),
            "supplied G commissioning did not prepare the selected parent and plan")
    native = result["native"]
    require(native["state"] == "settled" and native["native"]["state"] == "result"
            and native["authorization"] == {"state": "succeeded"}
            and native["cleanup"] == {"state": "succeeded"}, "G native ownership did not settle successfully")
    outcomes = native["native"]["result"]["outcomes"]
    require(set(outcomes) == {G_ID} and outcomes[G_ID]["restore"] == {"stage": "minted"},
            "G was not actually created by the supplied native held Add")


def exact_source_key(directory, original):
    """Read-only canonical evidence; never reproduce the owner's key algorithm."""
    path = directory / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
    with sqlite3.connect(path.resolve(strict=True).as_uri() + "?mode=ro", uri=True) as db:
        rows = db.execute("SELECT realm,owner,id,data FROM toolkit_records WHERE realm=? "
            "AND json_extract(data,'$.schema')=? AND json_extract(data,'$.body.event.route')=? "
            "AND json_extract(data,'$.body.event.original')=? LIMIT 2",
            ("mob.fixture", "toolkit.authenticated-source-event", original.route, original.original)).fetchall()
    matches = []
    for realm, owner, ident, raw in rows:
        data = json.loads(raw)
        if data["schema"] != "toolkit.authenticated-source-event":
            continue
        event = data["body"]["event"]
        if event["route"] == original.route and event["original"] == original.original:
            require(hashlib.sha256(v.canonical(event)).hexdigest() == original.event_sha256,
                    "canonical event differs from exact native source receipt")
            matches.append({"realm": realm, "owner": owner, "id": ident})
    require(len(matches) == 1, "exact unique canonical source not found")
    return matches[0]


async def submit_console_child(scenario, private, provider, directory, request, text, deadline):
    """Actual authenticated Console send, followed by exact native/source reads."""
    # A cold reader writes new evidence separately while reading the same
    # original native stores. This optional path never selects another owner.
    native_directory = getattr(scenario, "native_evidence_directory", directory)
    tag = {"original-a": "a", "original-b": "b"}[request.route]
    gate = provider.original_gates[tag]
    approved = False
    def send():
        connection = s.HTTPConnection("127.0.0.1", scenario.port, timeout=30)
        try:
            connection.request("POST", "/console/send", body=v.canonical({
                "identity": request.route, "content": text, "origin": "console", "origin_kind": "operator",
                "idempotency_key": request.send_key}), headers={"Authorization": "Bearer " + scenario.console_workflow_token,
                "Content-Type": "application/json", "Accept": "application/json", "Origin": scenario.origin})
            response = connection.getresponse()
            raw = response.read(1024 * 1024 + 1)
            value = None
            if len(raw) <= 1024 * 1024:
                try:
                    value = v.unique_json(raw)
                except (ValueError, UnicodeError, AssertionError):
                    pass
            code = value.get("error") if isinstance(value, dict) else None
            # Keep only fixed error codes, never arbitrary response content or
            # authentication headers, even when the send is refused.
            v.save_json(directory / ("console-child-" + tag + "-http.json"), {
                "status": response.status, "response_bytes": len(raw),
                "error_code": code if code in ("access_denied", "unauthorized") else None})
            require(response.status in (200, 202) and len(raw) <= 1024 * 1024, "actual Console child send refused")
            return v.unique_json(raw)
        finally:
            connection.close()
    try:
        with provider.lock:
            require(gate["deadline"] is None, "Console child was already selected")
            gate["deadline"] = deadline
        accepted = await asyncio.to_thread(send)
        require(accepted.get("status") == "accepted" and accepted.get("identity") == request.route
                and all(isinstance(accepted.get(key), str) and accepted[key]
                        for key in ("session_id", "interaction_id", "input_frame_id")),
                "actual Console acceptance coordinates missing")
        require(await asyncio.to_thread(gate["entered"].wait, max(0, min(30, deadline - time.monotonic()))),
                "Console child did not enter its first native model request")
        original = await asyncio.to_thread(private.prepare_console_workflow_child,
            request, scenario.console_workflow_login, timeout=30)
        require(original is not None and original.route == request.route, "Console child source locator missing")
        require(await asyncio.to_thread(private.prepare_console_workflow_child,
            request, scenario.console_workflow_login, timeout=30) == original,
                "exact Console proposal observation changed source")
        key = exact_source_key(native_directory, original)
        path = native_directory / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
        with sqlite3.connect(path.resolve(strict=True).as_uri() + "?mode=ro", uri=True) as db:
            raw = db.execute("SELECT data FROM toolkit_records WHERE realm=? AND owner=? AND id=?",
                (key["realm"], key["owner"], key["id"])).fetchone()
        record = json.loads(raw[0])["body"]
        bound = record.get("bound_service_input")
        require(record.get("console_child_input") == "native_console" and record.get("workflow_child")
                and record.get("final_association") and isinstance(bound, dict)
                and bound["header"]["id"] == original.original
                and bound["header"]["idempotency_key"] == "console:" + accepted["interaction_id"] + ":" + accepted["input_frame_id"]
                and bound["header"]["authority_association"] == record["final_association"],
                "Console source lacks its exact child binding and native input")
        rows = s.input_rows(native_directory, accepted["session_id"])
        require(len(rows) == 1 and rows[0]["input_id"] == original.original,
                "Console child lacks one exact actually admitted native input")
        approved = True
        # These are actual coordinates plus evidence, not a synthetic private
        # submission result, turn ticket or source-settlement classification.
        return {"original": dataclasses.asdict(original), "source_key": key,
                "admission": {"session_id": accepted["session_id"], "input_id": original.original,
                              "console_acceptance": accepted, "native_inputs": rows}}
    finally:
        if approved:
            with provider.lock:
                gate["approved"] = True
        else:
            provider.release_original_responses(abort=True)


async def observe_ordinary(scenario, member):
    status = await scenario.runtime.status(C3_ID)
    tools = await scenario.runtime.mob_handle().identity_resolved_tools_detail(C3_ID)
    canonical = await asyncio.to_thread(scenario.runtime.member_classes.observe, C3_ADMISSION, member, timeout=30)
    return status, {"status": status.to_dict(), "tools": dataclasses.asdict(tools), "canonical": canonical}


async def invoke_ordinary(scenario, private, directory, deadline, phase):
    scenario.host_loop_failure.require_alive()
    text = c3_text(phase)
    event = authenticated_event(directory, C3_ID, text, occurrence="acceptance-ordinary-" + phase)
    original = (await asyncio.to_thread(private.retain_authenticated_event, event, timeout=30)).original
    admitted = await asyncio.to_thread(private.submit_authenticated_original, original, text,
        event.original_authentication, timeout=60)
    require_admitted(admitted)
    scenario.host_loop_failure.require_alive()
    turn = await scenario.runtime.agent(C3_ID).wait_for_turn(admitted["turn_ticket"],
        timeout=min(60, deadline - time.monotonic()))
    scenario.host_loop_failure.require_alive()
    require(turn.output == "Ordinary callback returned " + phase and not turn.output_truncated,
            "ordinary input did not complete with its actual callback result")
    rows = s.input_rows(directory, admitted["session_id"])
    selected = [row for row in rows if row["input_id"] == admitted["input_id"]]
    require(len(selected) == 1 and selected[0].get("terminal_completion") is not None,
            "ordinary callback input has no exact native completion")
    audit = selected[0]["authorization_audit"]
    require(audit and all([item["input_id"] for item in row["contributors"]] == [admitted["input_id"]]
                         for row in audit), "ordinary callback used another input's authority")
    return {"original": dataclasses.asdict(original), "admission": admitted, "turn": turn.to_dict(),
        "inputs": rows, "operation": s.public_operation_audit(audit, C3_TOOL, "c3-" + phase + "-call")}


class Scenario(s.Scenario):
    ordinary_customizer_only = False
    respawn_shared_g = False
    name = "supplied-shared-workflow-success"
    scope = ("supplied toolkit_gateway private workflow opt-in and typed SDK commands; actual P input, "
             "held G plus durable application listing ACK, two independent preselected callers, cancelled A, "
             "protected B software review, one actual public HN operation and finalized fresh B answer; "
             "same-host ordinary class nonce callback before/after real respawn, controlled model and issuer, "
             "G's explicit SDK pass-through customizer; "
             "no custom Rust launcher, live account, browser or cold recovery claim")

    def configure_error_observer(self, config):
        self.failure_member_id = C3_ID if self.ordinary_customizer_only else G_ID
        super().configure_error_observer(config)

    def application_runtime_options(self):
        return {"private_workflow_launch": True, "member_mcp": self.member_mcp}

    def observe_runtime(self, runtime):
        self.runtime = runtime
        self.commission_attempt = None
        self.probe_attempt = None
        self.c3 = {"status": "not_started", "scope": "host-authored read-only nonce callback; not provider/integration proof"}
        self.g_respawn = {"status": "not_started", "scope": "one same-session G respawn and fresh B work; no cold restore or selected-preparation replay"}

    def configure_runtime(self, config, directory):
        super().configure_runtime(config, directory)
        # These two selected originals now enter through authenticated Console
        # sends. P remains view-only; native send and child authority still apply.
        with Path(config.access_config_path).open("a") as access:
            access.write('\n[[rules]]\nid = "supplied-console-children"\nsubjects = ["' + c.EMAIL + '"]\n'
                'actions = ["agent.send"]\nagents = ["original-a", "original-b"]\n')
        profiles = tomllib.loads(self.plan["application"]["definition"]["text"])["profiles"]
        self.customizer = AcceptanceCustomizer(profiles[C3_CLASS]["model"], profiles[CLASS_ID]["model"])
        config.agent_customizer = self.customizer
        self.roster = DesiredRoster(directory, self.plan["application"]["roster"])
        self.member_types = importlib.import_module("meerkat_toolkit.member_classes")
        self.member_mcp = self.member_types.MemberMcpApplication(self.roster.retain_empty_mcp, self.roster.lookup_empty_mcp)
        config.roster_provider = config.topology_provider = self.roster
        config.workgraph_enabled = True

    def configure_plan(self, plan):
        super().configure_plan(plan)
        app = plan["application"]
        app["definition"]["text"] += '\n[profiles.' + C3_CLASS + ']\nexternal_addressable = true\nmodel = "fixture-original-model"\nprovider = "self_hosted"\nself_hosted_server_id = "local"\nruntime_mode = "turn_driven"\n[profiles.' + C3_CLASS + '.tools]\ncomms = true\n'
        declared = {"id": C3_CLASS, "profile": C3_CLASS, "factory": "acceptance-nonce-factory",
            "callback_account": {"kind": "declared_none"},
            "callbacks": [{"name": C3_TOOL, "description": C3_DESCRIPTION, "input_schema": copy.deepcopy(C3_SCHEMA)}],
            "grants": sorted([{"tool_name": name, "action": "invoke", "consequence": "r0" if name == C3_TOOL else "r2"}
                              for name in (C3_TOOL, "tool_catalog_search", "tool_catalog_load")], key=lambda row: row["tool_name"])}
        app["member_classes"].append(declared)
        plan["owner"]["native_host"]["member_classes"].append(copy.deepcopy(declared))
        app["member_classes"].sort(key=lambda row: row["id"])
        plan["owner"]["native_host"]["member_classes"].sort(key=lambda row: row["id"])

    def provider(self, directory, events):
        return AcceptanceModel(directory, events, ordinary_only=self.ordinary_customizer_only)

    async def observe_shared(self, member):
        status = await self.runtime.status(G_ID)
        tools = await self.runtime.mob_handle().identity_resolved_tools_detail(G_ID)
        canonical = await asyncio.to_thread(self.runtime.member_classes.observe, ADMISSION, member, timeout=30)
        return status, {"status": status.to_dict(), "tools": dataclasses.asdict(tools), "canonical": canonical}

    async def wait_finalized_a(self, child, member, provider, directory, deadline):
        old_status, before = await self.observe_shared(member)
        session = old_status.session_id
        workflow = before["canonical"]["record"]["workflow"]["workflow"]
        require(old_status.delivery_scope is not None and old_status.delivery_scope.session_id == session,
                "G native scope unavailable before close or respawn")
        # This is a bounded read of receipt + native finalization, not a
        # retry of transfer, model work, setup, submission or respawn.
        while time.monotonic() < deadline:
            self.host_loop_failure.require_alive()
            with provider.lock:
                require(not provider.errors and not provider.b_context.is_set()
                        and not any(key.startswith("g-") for key in provider.issued),
                        "B or a service effect entered before the A context completion gate")
            delivery = context_delivery(directory, child["source_key"], workflow, session)
            rows = s.input_rows(directory, session)
            if delivery and finalized_context(rows, delivery,
                    "A context received; no service effect requested for A."):
                self.host_loop_failure.require_alive()
                break
            await asyncio.sleep(0.05)
        else:
            self.host_loop_failure.require_alive()
            raise AssertionError("A's exact delivered G context did not finalize before the continuity check")
        await s.wait_initial_a_completion(directory, child["admission"], deadline,
            failure_observer=self.host_loop_failure)
        self.host_loop_failure.require_alive()
        return old_status, before, delivery, rows

    async def check_shared_rebuild(self, child, member, provider, directory, process, deadline):
        """One actual identity respawn between independently authenticated A and B inputs."""
        evidence = directory / "shared-g-rebuild.json"
        self.g_respawn.update(status="running")
        try:
            old_status, before, delivery, rows = await self.wait_finalized_a(
                child, member, provider, directory, deadline)
            session = old_status.session_id
            self.host_loop_failure.require_alive()
            self.g_respawn.update(before=before, a_delivery=delivery, before_inputs=rows,
                callbacks_before=await self.customizer.wait_shared_created(1, session, deadline,
                    failure_observer=self.host_loop_failure),
                peer_before=shared_peer(directory, process.pid, self.resource_python, observed=before))
            self.host_loop_failure.require_alive()
            require(self.g_respawn["peer_before"]["owner_pid"] not in self.precommission_pids
                    and self.g_respawn["peer_before"]["bridge_pid"] not in self.precommission_pids,
                    "shared physical owner was not created by this G commission")
            roster = copy.deepcopy(self.roster.selected())
            v.save_json(evidence, self.g_respawn)
            callbacks_before_rpc = self.customizer.respawn_snapshot(G_ID)
            self.g_respawn["respawn"] = await self.runtime.respawn(G_ID)
            callbacks_at_return = self.customizer.respawn_snapshot(G_ID)
            self.g_respawn["respawn_callback_window"] = {"before": callbacks_before_rpc, "returned": callbacks_at_return}
            self.host_loop_failure.require_alive()
            self.g_respawn["respawn_callbacks"] = await self.customizer.wait_respawn(
                callbacks_before_rpc, callbacks_at_return, G_ID, session, deadline,
                failure_observer=self.host_loop_failure)
            self.host_loop_failure.require_alive()
            _, after = await self.observe_shared(member)
            self.g_respawn["after"] = after
            require_g_transition(before, after, self.g_respawn["respawn"])
            self.g_respawn["callbacks_after"] = self.g_respawn["respawn_callbacks"]["created"]
            require(self.roster.selected() == roster and self.roster.publications == 1,
                    "G respawn performed another admission or changed the durable desired roster")
            self.g_respawn["peer_after"] = shared_peer(directory, process.pid, self.resource_python, observed=after)
            self.g_respawn["old_transport"] = require_retired_transport(
                self.g_respawn["peer_before"], self.g_respawn["peer_after"])
            require(s.input_rows(directory, session) == rows, "G respawn changed predecessor native inputs")
            self.g_respawn["old_scope"] = await require_g_stale_scope(self.runtime, old_status.delivery_scope)
            require(s.input_rows(directory, session) == rows, "stale G scope added or changed a native input")
            self.customizer.verify_respawn(self.g_respawn["respawn_callbacks"])
            self.g_respawn.update(status="respawn_observed_awaiting_b", stale_scope_inputs_unchanged=True)
        except BaseException as failure:
            self.g_respawn.update(status="failed", error=repr(failure))
            raise
        finally:
            self.g_respawn["callback_observations"] = self.customizer.respawn_snapshot(G_ID)
            v.save_json(evidence, self.g_respawn)

    async def check_ordinary_rebuild(self, private, provider, directory, deadline):
        """Actual supplied Add/input/respawn; no direct invocation of the handler."""
        self.c3.update(status="running")
        evidence = directory / "ordinary-class-rebuild.json"
        native = self.plan["owner"]["native_host"]
        route = copy.deepcopy(self.ordinary_route if getattr(self, "helperless_restore", False) else
                              next(row for row in native["routes"] if row["id"] == "original-a"))
        member = {**route["origin"]["member"], "member": C3_ID, "role": C3_CLASS}
        owner = {"kind": "shared_agent", "id": C3_ID,
                 "qualification": copy.deepcopy(native["ingress_domain"]["authority"]["qualification"])}
        route.update(id=C3_ID, logical_executor=owner,
                     represented_subject=copy.deepcopy(native["remote_operator"]),
                     source_authority=copy.deepcopy(native["ingress_domain"]["authority"]),
                     authentication_domain=copy.deepcopy(native["authentication_domain"]),
                     event_domain=copy.deepcopy(native["event_domain"]))
        route["origin"].update(member=member, requester=copy.deepcopy(native["remote_operator"]),
                               ingress_actor=copy.deepcopy(native["ingress_domain"]["authority"]))
        route.pop("application_parent", None)
        route.pop("controller_parent", None)
        instance = {"class": C3_CLASS, "member": {"member": member, "logical_owner": owner,
            "context_generation": 1, "audience": {"audience": "principal", "principal": route["origin"]["requester"]}}}
        selection = self.member_types.MemberClassAddRequest(C3_ADMISSION, instance,
            v.META_IDENTITY.DurableAgentSpec.from_dict(self.roster.probe_spec()), route,
            {"target": {"kind": "original_route", "route": C3_ID}, "issuance_id": "acceptance-ordinary-controller"})
        self.roster.prepare_probe(selection)
        try:
            self.probe_attempt = self.runtime.member_classes.begin_add(selection, publish=self.roster.publish_probe,
                timeout=min(120, deadline - time.monotonic()))
            self.c3["add"] = await asyncio.to_thread(self.probe_attempt.result)
            added = self.c3["add"]
            if added["state"] == "unsettled":
                self.c3["add_join"] = await asyncio.to_thread(self.runtime.member_classes.join,
                    added["settlement_id"], timeout=60)
                added = self.c3["add_join"]
            await self.probe_attempt.join_listing()
            require(added["state"] == "settled" and added["native"]["state"] == "result"
                    and set(added["native"]["result"]["outcomes"]) == {C3_ID}
                    and added["native"]["result"]["outcomes"][C3_ID]["restore"] == {"stage": "minted"}
                    and added["authorization"] == added["cleanup"] == {"state": "succeeded"}
                    and self.roster.probe_publications == 1, "ordinary held Add/listing did not actually settle")
            self.c3["before_use"] = await invoke_ordinary(self, private, directory, deadline, "before")
            old_status, self.c3["before"] = await observe_ordinary(self, member)
            require(old_status.delivery_scope is not None and old_status.delivery_scope.session_id == old_status.session_id,
                    "ordinary native scope unavailable before respawn")
            require(sum(row["identity"] == C3_ID for row in self.customizer.builds) == 1,
                    "ordinary fresh Add did not run exactly one real SDK build")
            self.host_loop_failure.require_alive()
            self.c3["after_create_contexts_before"] = await self.customizer.wait_created(1, old_status.session_id, deadline,
                failure_observer=self.host_loop_failure)
            self.host_loop_failure.require_alive()
            created_before = [row for row in self.customizer.created if row["identity"] == C3_ID]
            require(created_before == [{"identity": C3_ID, "session_id": old_status.session_id}],
                    "fresh ordinary after_create observations do not match its one actual build")
            self.c3["after_create_before"] = copy.deepcopy(created_before)
            v.save_json(evidence, self.c3)
            if getattr(self, "helperless_restore", False):
                self.old_scope = old_status.delivery_scope
                self.c3["status"] = "added_and_used_before_process_restore"
                return
            # One real native operation. Failure is retained, never retried or
            # replaced by reload, reset, direct callback or another Add.
            callbacks_before_rpc = self.customizer.respawn_snapshot(C3_ID)
            self.c3["respawn"] = await self.runtime.respawn(C3_ID)
            callbacks_at_return = self.customizer.respawn_snapshot(C3_ID)
            self.c3["respawn_callback_window"] = {"before": callbacks_before_rpc, "returned": callbacks_at_return}
            self.host_loop_failure.require_alive()
            self.c3["respawn_callbacks"] = await self.customizer.wait_respawn(
                callbacks_before_rpc, callbacks_at_return, C3_ID, old_status.session_id, deadline,
                failure_observer=self.host_loop_failure)
            self.host_loop_failure.require_alive()
            _, self.c3["after"] = await observe_ordinary(self, member)
            require_c3_transition(self.c3["before"], self.c3["after"], self.c3["respawn"])
            self.c3["after_create_contexts_after"] = self.c3["respawn_callbacks"]["created"]
            latest_build = self.c3["respawn_callbacks"]["latest_build"]
            self.c3["old_scope"] = await require_c3_stale_scope(self.runtime, old_status.delivery_scope)
            require(s.input_rows(directory, old_status.session_id) == self.c3["before_use"]["inputs"],
                    "stale delivery created or changed an actual native input")
            self.customizer.verify_respawn(self.c3["respawn_callbacks"])
            with provider.lock:
                provider.c3_after_handler_build = latest_build
            self.c3["after_use"] = await invoke_ordinary(self, private, directory, deadline, "after")
            self.customizer.verify_respawn(self.c3["respawn_callbacks"])
            require_c3_history(provider.c3_histories["before"], provider.c3_histories["after"])
            old_rows = self.c3["before_use"]["inputs"]
            new_rows = self.c3["after_use"]["inputs"]
            require(len(old_rows) == 1 and len(new_rows) == 2
                    and all(row in new_rows for row in old_rows)
                    and self.customizer.calls == [{"nonce": "before", "handler_build": 1}, {"nonce": "after", "handler_build": latest_build}]
                    and self.roster.probe_publications == 1 and self.roster.publications == 0,
                    "ordinary rebuild changed prior input/history, duplicated Add or failed actual handler dispatch")
            self.c3["history"] = {"before": provider.c3_histories["before"], "after": provider.c3_histories["after"]}
            created = [row for row in self.customizer.created if row["identity"] == C3_ID]
            require(created == created_before * 2,
                    "respawn did not name exactly one new after_create for the same actual session")
            self.c3["after_create"] = {"observed": created, "state": "observed"}
            self.c3["status"] = "native_rebuild_observed"
        except BaseException as failure:
            self.c3.update(status="failed", error=repr(failure))
            raise
        finally:
            self.c3["callback_calls"] = copy.deepcopy(self.customizer.calls)
            self.c3["customizer"] = {"builds": copy.deepcopy(self.customizer.builds), "created": copy.deepcopy(self.customizer.created),
                "build_contexts": copy.deepcopy(self.customizer.build_contexts),
                "created_contexts": copy.deepcopy(self.customizer.created_contexts),
                "respawn_events": self.customizer.respawn_snapshot(C3_ID)}
            v.save_json(evidence, self.c3)

    async def begin_workflow(self, private, provider, directory, events, process, deadline):
        require("--private-workflow-launch" in process.args
                and Path(process.args[0]).name == "toolkit_gateway"
                and self.runtime.host_control is private, "supplied same-child private launcher was not selected")
        require(not (directory / "driver-launch-result.json").exists()
                and not (directory / "begin-workflow").exists(), "embedding marker cannot prove supplied commissioning")
        await self.check_ordinary_rebuild(private, provider, directory, deadline)
        graph = self.runtime.mob_handle()
        snapshot = await graph.workgraph_snapshot()
        require(snapshot.realm_id == self.plan["owner"]["native_host"]["realm"]
                and snapshot.namespace and not snapshot.all_namespaces,
                "actual SDK WorkGraph scope does not match this application")
        scope = {"realm_id": snapshot.realm_id, "namespace": snapshot.namespace}
        # The actual RPC owner fixes the realm and explicitly refuses realm_id
        # on its wire. Retain that returned realm only in WorkItem selectors.
        rpc_scope = {"namespace": snapshot.namespace}
        graph_receipts = []

        async def claimed(title, owner):
            created = await graph.workgraph_create(title, **rpc_scope, labels=[CYCLE])
            row = await graph.workgraph_claim(created.id, created.revision, {"key": owner}, **rpc_scope)
            require(row.realm_id == scope["realm_id"] and row.namespace == scope["namespace"]
                    and row.owner["key"] == owner and CYCLE in row.labels,
                    "actual WorkGraph claim differs from selected application launch")
            graph_receipts.append(dataclasses.asdict(row))
            return {**scope, "item_id": row.id}

        parent_owner = {"kind": "agent", "id": "supplied-parent-owner"}
        parent_work = await claimed("Two independent supplied application requests", parent_owner)
        p_event = authenticated_event(directory, "planning-parent", s.P_TEXT)
        p = (await asyncio.to_thread(private.retain_authenticated_event, p_event, timeout=30)).original
        p_submit = v.SDK.PrivateOriginalSubmit(p, s.P_TEXT, p_event.original_authentication)
        g_event = authenticated_event(directory, G_ID, s.G_TEXT, (p.original,))
        parent_route = next(row for row in self.plan["owner"]["native_host"]["routes"] if row["id"] == p.route)
        parent_member = parent_route["origin"]["member"]
        member = {"mob_id": parent_member["mob_id"], "role": CLASS_ID, "member": G_ID}
        instance = {"class": CLASS_ID, "member": {"member": member,
            "logical_owner": v.installed_fixture_principal(G_ID), "context_generation": 1,
            "audience": {"audience": "principal", "principal": parent_route["origin"]["requester"]}}}
        now = int(time.time() * 1000)
        selected = COMMISSION.WorkflowCommissionRequest(ADMISSION, G_ID, p_submit, instance, g_event,
            s.G_TEXT, "supplied-shared-cohort", parent_work, parent_owner, CYCLE, now, now + 600_000,
            now + 540_000, 2, 60_000, "supplied-g-controller", "supplied-g-root", "supplied-g-leaf")
        prepared_parent = await asyncio.to_thread(private.prepare_workflow_parent, selected,
            parent_root_issuance_id="supplied-p-root", parent_leaf_issuance_id="supplied-p-leaf", timeout=60)
        require(prepared_parent == p, "workflow preparation names a different actual P")
        events.add("supplied_parent_prepared", original=dataclasses.asdict(prepared_parent))
        p_result = await asyncio.to_thread(private.submit_authenticated_original, p, s.P_TEXT,
            p_event.original_authentication, timeout=60)
        require_admitted(p_result)
        self.roster.prepare(selected)
        if self.respawn_shared_g:
            self.precommission_pids = {row["pid"] for row in v.child_snapshot(process.pid)}
        events.add("supplied_commission_written", selection=selected.wire())
        self.commission_attempt = private.begin_workflow_commission(selected,
            publish=self.roster.publish, timeout=min(180, deadline - time.monotonic()))
        result = await observe_commission(self.commission_attempt, directory)
        require_commissioned(result, selected)
        require(self.roster.publications == 1 and self.roster.selected() == self.roster.initial + ([self.roster.probe_spec()] if self.roster.probe_publications else []) + [self.roster.spec()],
                "actual application publication was not joined exactly once")
        observed = await asyncio.to_thread(self.runtime.member_classes.observe, ADMISSION, member, timeout=30)
        record = observed["record"]
        require(record["id"] == ADMISSION and record["workflow"]["parent"] == dataclasses.asdict(p)
                and record["workflow"]["original"] == result["planning"] and record["route"]["id"] == G_ID
                and record["phase"]["kind"] == "active" and record["native_surface"]
                and any((tool.get("provenance") or {}).get("source_id") == "toolkit-setup"
                        and tool["provenance"]["kind"] == "mcp" for tool in record["native_surface"]),
                "actual canonical G surface/route does not match the commissioned peer")
        self.workflow_context = (p, p_result, result, observed, member, parent_work, graph_receipts)
        return await self.launch_children(private, provider, directory, events, process, deadline,
                                          tags=getattr(self, "initial_children", ("a", "b")))

    async def launch_children(self, private, provider, directory, events, process, deadline, *, tags):
        p, p_result, result, observed, member, parent_work, retained_graph = self.workflow_context
        graph = self.runtime.mob_handle()
        snapshot = await graph.workgraph_snapshot()
        require(snapshot.realm_id == parent_work["realm_id"] and snapshot.namespace == parent_work["namespace"]
                and not snapshot.all_namespaces, "child graph scope differs from original P")
        rpc_scope = {"namespace": snapshot.namespace}
        graph_receipts = copy.deepcopy(retained_graph)
        async def claimed(title, owner):
            created = await graph.workgraph_create(title, **rpc_scope, labels=[CYCLE])
            row = await graph.workgraph_claim(created.id, created.revision, {"key": owner}, **rpc_scope)
            require(row.realm_id == snapshot.realm_id and row.namespace == snapshot.namespace
                    and row.owner["key"] == owner and CYCLE in row.labels, "child WorkGraph claim changed")
            graph_receipts.append(dataclasses.asdict(row))
            return {"realm_id": row.realm_id, "namespace": row.namespace, "item_id": row.id}
        planning = v.SDK.PrivateOriginalRef(**result["planning"])
        prepared = []
        negative_controls = []
        for tag in tags:
            name, text = "original-" + tag, self.request(tag)
            child_owner = {"kind": "agent", "id": "supplied-" + tag + "-owner"}
            child = await claimed(name, child_owner)
            edge = await graph.workgraph_link("parent", child["item_id"], parent_work["item_id"], **rpc_scope)
            require(edge.from_id == child["item_id"] and edge.to_id == parent_work["item_id"]
                    and edge.kind == "parent", "actual parent edge mismatch")
            graph_receipts.append(dataclasses.asdict(edge))
            request = v.SDK.ConsoleWorkflowChildRequest(p, planning, name, "supplied-console-" + tag,
                hashlib.sha256(text.encode()).hexdigest(), child, child_owner,
                "supplied-" + tag + "-effects", (
                    {"target": {"kind": "mcp_server", "server": "toolkit-setup"},
                     "issuance_id": "supplied-" + tag + "-mcp"},
                    {"target": {"kind": "registered_tool_bundle_operation", "bundle": "meerkat_toolkit.setup_accounts",
                                "operation": "toolkit_request_public_service"},
                     "issuance_id": "supplied-" + tag + "-public"}), ("fixture-output",))
            async def refused(changed, label, expected=v.SDK.HostReplyCode.DENIED):
                try:
                    await asyncio.to_thread(private.prepare_console_workflow_child,
                        changed, self.console_workflow_login, timeout=30)
                except v.SDK.HostReplyError as error:
                    require(error.code == expected, "Console child refusal changed category")
                    negative_controls.append({"route": name, "case": label, "code": error.code.value})
                else:
                    raise AssertionError("invalid Console child proposal was accepted")
            # P owns the retained G plan; using P as the planning selector has
            # no matching live plan and returns Unavailable.
            await refused(dataclasses.replace(request, planning=p), "foreign_plan", v.SDK.HostReplyCode.UNAVAILABLE)
            require(await asyncio.to_thread(private.prepare_console_workflow_child, request,
                self.console_workflow_login, timeout=30) is None, "proposal fabricated a Console input")
            require(await asyncio.to_thread(private.prepare_console_workflow_child, request,
                self.console_workflow_login, timeout=30) is None, "proposal replay fabricated a Console input")
            await refused(dataclasses.replace(request, child_owner={"kind": "agent", "id": "foreign"}), "changed_claim")
            await refused(dataclasses.replace(request, issuance_id="changed-" + tag), "changed_correlation")
            await refused(dataclasses.replace(request, output_audiences=("fixture-output", "unselected-output")), "widened_audience")
            await refused(dataclasses.replace(request, content_sha256="0" * 64), "changed_content")
            prepared.append((text, request))
        children = []
        submitted = False
        try:
            for text, request in prepared:
                children.append(await submit_console_child(self, private, provider, directory, request, text, deadline))
                if self.respawn_shared_g and request.route == "original-a":
                    # This mode observes A and G finish before submitting B.
                    with provider.lock:
                        provider.original_gates["a"]["released"].set()
                    await self.check_shared_rebuild(children[-1], member, provider, directory, process, deadline)
            submitted = True
        finally:
            # In the base case A stays held through B's exact replay, so A's
            # setup cannot overlap B's current-state check.
            provider.release_original_responses(abort=not submitted, tags=tags)
        if self.respawn_shared_g:
            self.g_b_source = next(row["source_key"] for row in children if row["original"]["route"] == "original-b")
            self.g_directory = directory
        launch = {"parent": {"original": dataclasses.asdict(p), "admission": p_result},
            "children": children, "commission": result, "canonical_g": observed,
            "graph_receipts": graph_receipts, "negative_controls": negative_controls,
            "initial_members": len(self.roster.initial), "initial_g_members": 0, "native_add": "created",
            "application_listing": {"count": self.roster.publications, "joined": True,
                "desired_roster": v.artifact_pin(self.roster.path), "admitting": v.artifact_pin(self.roster.admitting)},
            "transport": "supplied_private_sdk", "no_embedding_marker": True, "ordinary_class_rebuild": self.c3}
        if self.respawn_shared_g:
            launch["shared_g_rebuild"] = self.g_respawn
        v.save_json(directory / ("supplied-launch-" + self.phase + ".json" if hasattr(self, "phase") else "supplied-launch-evidence.json"), launch)
        return launch

    def verify_customizer(self):
        require(self.c3.get("status") == "native_rebuild_observed"
                and self.c3["after_create"]["state"] == "observed",
                "real delegated after_create is pending Meta callback fix; acceptance is not passed")
        self.customizer.verify_respawn(self.c3["respawn_callbacks"])
        if not self.respawn_shared_g:
            return super().verify_customizer()
        result = self.verify_fresh_b(self.g_respawn, "respawn_observed_awaiting_b", "shared-g-rebuild.json")
        result["shared_g_rebuild"] = result.pop("continuity")
        return result

    def verify_fresh_b(self, continuity, expected_status, evidence_name):
        # Called by the existing success gate before its finally releases B or
        # begins teardown. A cleanup-unblocked model result cannot pass here.
        require(continuity["status"] == expected_status, "G continuity was not observed")
        session = continuity["after"]["status"]["session_id"]
        workflow = continuity["after"]["canonical"]["record"]["workflow"]["workflow"]
        delivery = context_delivery(self.g_directory, self.g_b_source, workflow, session)
        require(delivery is not None and delivery["input"] not in
                {row["input_id"] for row in continuity["before_inputs"]},
                "B did not deliver its own fresh G context after the continuity check")
        rows = s.input_rows(self.g_directory, session)
        selected = [row for row in rows if row["input_id"] == delivery["input"]]
        require(len(selected) == 1 and selected[0]["authorization_audit"], "fresh B context never used native G tools")
        require(all([item["input_id"] for item in row["contributors"]] == [delivery["input"]]
                    for row in selected[0]["authorization_audit"]), "G setup audit used another input's authority")
        require(len(rows) == len(continuity["before_inputs"]) + 1
                and all(row in rows for row in continuity["before_inputs"]),
                "G lost the finalized A context or gained an unselected input")
        observations = [row["observation"] for row in selected[0]["authorization_audit"]]
        install = [row for row in observations if row["observation"]["kind"] == "prepared"
            and row["observation"]["target"].get("tool_name") == "setup_install"
            and row["observation"]["target"].get("call_id") == "g-install"]
        require(len(install) == 1 and any(row["operation_id"] == install[0]["operation_id"]
            and row["observation"]["kind"] == "outcome"
            and row["observation"]["outcome"].get("kind") == "tool_dispatch_returned"
            and row["observation"]["outcome"].get("result_is_error") is False
            and row["observation"]["outcome"].get("terminal_error") is None for row in observations),
            "fresh B context did not own exactly one successful native G install call")
        require(any(row["operation_id"] == install[0]["operation_id"] and row["observation"]["kind"] == "entry"
                    for row in observations)
                and not any(row["operation_id"] == install[0]["operation_id"]
                    and row["observation"]["kind"] in ("refused", "authorization_unavailable") for row in observations),
                "fresh B G install did not enter or retained a native refusal")
        if expected_status == "respawn_observed_awaiting_b":
            proof = self.customizer.verify_respawn(continuity["respawn_callbacks"])
            require(proof["identity"] == G_ID and proof["session_id"] == session
                    and self.customizer.shared_created_contexts == continuity["callbacks_after"] == proof["created"]
                    and len(self.customizer.shared_build_contexts) == proof["latest_build"],
                    "G gained an unselected construction")
        else:
            require(self.customizer.shared_created_contexts == continuity["callbacks_after"]
                    and len(self.customizer.shared_build_contexts) == 2, "G gained an unselected construction")
        continuity.update(status="fresh_b_work_observed", b_delivery=delivery, b_context_input=selected[0])
        v.save_json(self.g_directory / evidence_name, continuity)
        return {"builds": [row for row in self.customizer.builds if row["identity"] == G_ID],
                "after_create": continuity["callbacks_after"], "draft_changed": False,
                "application_tools_added": False, "continuity": continuity}

    async def run(self, *args, **kwargs):
        try:
            if self.ordinary_customizer_only:
                return await self.run_ordinary_customizer(*args, **kwargs)
            return await self.run_shared(*args, **kwargs)
        finally:
            if self.probe_attempt is not None:
                await self.probe_attempt.join_listing()
            # An entered application mutation is joined even if observation
            # failed. Native/process teardown stays in the existing verifier.
            if self.commission_attempt is not None:
                await self.commission_attempt.join_listing()

    async def run_shared(self, *args, **kwargs):
        return await super().run(*args, **kwargs)

    async def run_ordinary_customizer(self, private, provider, directory, events, process, base_url):
        require(base_url.rstrip("/") == self.origin and Path(process.args[0]).name == "toolkit_gateway"
                and self.runtime.host_control is private, "focused proof requires the same supplied native host")
        deadline = time.monotonic() + 540
        report = {"scope": self.scope, "status": "running"}
        enrollment = None
        try:
            # Preserve the existing selected Console login owner and its joined
            # retirement, although only authenticated private originals execute.
            scope = "ordinary-customizer-" + uuid.uuid4().hex
            enrollment = private.begin_console_login(v.SDK.ConsoleLoginSelection(
                c.ISSUER, c.SUBJECT, scope, int(time.time() + 650) * 1000), timeout=10)
            await asyncio.to_thread(enrollment.result)
            with provider.lock:
                require(not provider.requests, "model began before actual SDK Ready")
                provider.enabled = True
            await self.check_ordinary_rebuild(private, provider, directory, deadline)
            require(self.c3["status"] == "native_rebuild_observed"
                    and self.c3["after_create"]["state"] == "observed"
                    and len(self.c3["after_create_contexts_before"]) == 1
                    and len(self.c3["after_create_contexts_after"]) == 2,
                    "focused proof requires actual Add and respawn callbacks with decoded context")
            require(self.commission_attempt is None and self.roster.publications == 0
                    and not self.roster.admitting.exists()
                    and not any(row["identity"] == G_ID for row in self.roster.selected())
                    and not any((directory / name).exists() for name in
                        ("begin-workflow", "driver-launch-result.json", "supplied-commission-result.json")),
                    "focused proof entered workflow commissioning")
            with provider.lock:
                require(not provider.errors and provider.business_observation is None
                        and provider.issued and all(key.startswith("c3-") for key in provider.issued),
                        "focused proof entered a workflow or public-read model operation")
            report.update(status="passed", ordinary_class_rebuild=self.c3,
                          workflow_commissioned=False, public_hn_read=False)
        except BaseException as failure:
            report.update(status="failed", error=repr(failure))
            raise
        finally:
            try:
                if enrollment is not None:
                    retired = await asyncio.to_thread(enrollment.retire, timeout=30)
                    require(retired.fenced and retired.reservations_drained and not retired.uncertain,
                            "login retirement unconfirmed")
                    report["login_retirement"] = dataclasses.asdict(retired)
            except BaseException as failure:
                report.update(status="failed", cleanup_error=repr(failure))
                raise
            finally:
                v.save_json(directory / "console-journey.json", report)
        return report


def require_cold_g_transition(before, after):
    old, new = before["status"], after["status"]
    require(old["identity"] == new["identity"] == G_ID and old["state"] == new["state"] == "active"
            and old["session_id"] == new["session_id"] and old["agent_runtime_id"] == new["agent_runtime_id"]
            and old["generation"] == new["generation"]
            and isinstance(old["delivery_scope"], dict) and old["delivery_scope"]
            and old["delivery_scope"] == new["delivery_scope"],
            "cold G changed its retained native identity/session or delivery scope")
    a, b = before["canonical"]["record"], after["canonical"]["record"]
    require(a["id"] == ADMISSION and a["workflow"] is not None
            and isinstance(a.get("destination"), dict) and bool(old["session_id"])
            and a["destination"]["context"] == a["phase"]["context"] == old["session_id"]
            and type(a["destination"]["context_generation"]) is int
            and a["destination"]["context_generation"] == a["phase"]["context_generation"] > 0
            and all(a.get(field) == b.get(field) for field in
                    ("id", "class", "instance", "workflow", "route", "destination", "phase",
                     "installed_surface", "native_surface")),
            "cold G changed its original class, helper lineage, workflow or surface")
    for observed in (before, after):
        require(observed["canonical"]["authorization"] == {"state": "succeeded"}
                and observed["canonical"]["record"]["phase"]["kind"] == "active"
                and observed["canonical"]["record"]["phase"]["context"] == old["session_id"]
                and observed["tools"]["identity"] == G_ID
                and observed["tools"]["session_id"] == old["session_id"]
                and set(s.SETUP).issubset(observed["tools"]["tools"]),
                "cold G lacks current native admission or its actual MCP setup surface")
    require(sorted(before["tools"]["tools"]) == sorted(after["tools"]["tools"]),
            "cold G changed its native tool set")


def require_cold_g_callbacks(customizer, before, session_id):
    created, contexts = before["callbacks_before"], before["build_contexts_before"]
    require(len(created) == len(contexts) == 1
            and created[0]["identity"] == G_ID and created[0]["session_id"] == session_id
            and created[0]["context"] == contexts[0]
            and isinstance(contexts[0]["model"], str) and contexts[0]["model"],
            "cold G lost its exact predecessor build and creation context")
    builds = [row for row in customizer.builds if row["identity"] == G_ID]
    require(builds == [{"identity": G_ID, "profile": CLASS_ID}] * 2
            and customizer.shared_build_contexts == contexts * 2,
            "cold G did not republish exactly one unchanged callback build")
    require(customizer.shared_created_contexts == created,
            "cold G changed its historical creation notification")
    return copy.deepcopy(created)


def planning_lineage(directory, original, parent):
    """Read-only historical correlation, never a source of restore authority."""
    key = exact_source_key(directory, original)
    path = directory / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
    with sqlite3.connect(path.resolve(strict=True).as_uri() + "?mode=ro", uri=True) as db:
        rows = db.execute("SELECT data FROM toolkit_records WHERE realm=? AND owner=? AND id=? LIMIT 2",
                          (key["realm"], key["owner"], key["id"])).fetchall()
    require(len(rows) == 1, "original planning source is absent or ambiguous")
    value = v.unique_json(rows[0][0].encode())["body"]
    helper = value.get("late_workflow_helper")
    require(value.get("planning_source_evidence") == {"role": "lead", "parent": dataclasses.asdict(parent)}
            and isinstance(helper, dict) and helper["admission"]["id"] == ADMISSION
            and helper["admission"]["workflow"]["parent"] == dataclasses.asdict(parent)
            and helper["admission"]["workflow"]["original"] == dataclasses.asdict(original)
            and helper["server"]["name"] == "toolkit-setup" and helper["server"].get("connector") is None
            and value.get("bound_service_input") is not None and value.get("final_association") is not None,
            "G has no exact native-bound original helper provenance; legacy rows cannot qualify")
    return {"source_key": key, **{name: copy.deepcopy(value[name]) for name in
        ("late_workflow_helper", "planning_source_evidence", "bound_service_input", "final_association")}}


def selected_preparation_record(directory, child, selected, *, allow_pending=False, expected_members=1):
    """Correlate real selected dispatch with nonempty canonical P custody.

    This read is not a restore oracle. Only the later owner cancellation reply
    proves that the reopened resource performed its required leave.
    """
    key = child["source_key"]
    path = directory / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
    with sqlite3.connect(path.resolve(strict=True).as_uri() + "?mode=ro", uri=True) as db:
        def read(key, schema):
            rows = db.execute("SELECT data FROM toolkit_records WHERE realm=? AND owner=? AND id=? LIMIT 2",
                              (key["realm"], key["owner"], key["id"])).fetchall()
            require(len(rows) == 1, "exact selected source/P record is absent or ambiguous")
            value = v.unique_json(rows[0][0].encode())
            require(value["schema"] == schema, "selected source/P schema differs")
            return value["body"]
        source = read(key, "toolkit.authenticated-source-event")
        members = source.get("shared_preparations", [])
        require(len(members) == 1, "selected original has no unique P member; cancellation would not exercise restoration")
        member = members[0]
        record = read(member["key"], "meerkat_toolkit.shared_preparation")
    require(member["identity"]["source"] == key
            and member["identity"]["acquisition"] == selected["context"]["membership"]["identity"]["acquisition"]
            and member["member"] in record["members"] and len(record["members"]) == expected_members, "P belongs to another source or acquisition")
    selection, completion = record["selection"], record["completion"]
    require(selection["result"] == selected["checked"], "P selected another checked request")
    require(completion is not None or allow_pending, "canonical P completion remains pending")
    if completion is not None:
        require(completion["controller"] == selection["controller"]
            and completion["validation_id"] == selected["checked"]["validation_id"]
            and completion["job"] == {**{key: selected["job"][key] for key in ("id", "kind", "family", "status", "result")},
                                      "error": selected["job"].get("error")}
            and completion["job"]["status"] == "succeeded" and completion["job"]["error"] is None,
                "canonical P does not retain the exact joined selected installation")
    return {"member": member, "record": record, "operation_cancelled": source["operation_cancelled"]}


def selected_completion_fault(directory, child, *, release=False):
    """Test-only write failure, scoped to this admitted source's unique P.

    No record or outcome is written. The actual store rejects publication until
    the fault is removed; only its existing completion owner can then publish.
    """
    path = directory / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
    with sqlite3.connect(path.resolve(strict=True).as_uri() + "?mode=rw", uri=True) as db:
        if release:
            db.execute("DROP TRIGGER acceptance_p_completion_unavailable")
            return
        key = child["source_key"]
        rows = db.execute("SELECT data FROM toolkit_records WHERE realm=? AND owner=? AND id=? LIMIT 2",
                          (key["realm"], key["owner"], key["id"])).fetchall()
        require(len(rows) == 1, "completion fault requires the exact admitted source")
        source = v.unique_json(rows[0][0].encode())
        members = source["body"].get("shared_preparations", [])
        require(source["schema"] == "toolkit.authenticated-source-event" and len(members) == 1
                and members[0]["identity"]["source"] == key, "completion fault requires its unique P")
        selected = members[0]["key"]
        # SQLite parameters cannot appear in a trigger. quote() produces SQL
        # literals for these already selected owner keys, including apostrophes.
        realm, owner, identifier = db.execute("SELECT quote(?),quote(?),quote(?)",
            (selected["realm"], selected["owner"], selected["id"])).fetchone()
        db.execute(f"""CREATE TRIGGER acceptance_p_completion_unavailable BEFORE UPDATE OF data ON toolkit_records
            WHEN NEW.realm={realm} AND NEW.owner={owner} AND NEW.id={identifier}
              AND json_extract(NEW.data, '$.schema')='meerkat_toolkit.shared_preparation'
              AND json_extract(OLD.data, '$.body.completion') IS NULL
              AND json_extract(NEW.data, '$.body.completion') IS NOT NULL
            BEGIN SELECT RAISE(ABORT, 'acceptance P completion unavailable'); END""")


def require_selected_p_leave(before, after, cancellation, original):
    require(cancellation == {"original": original, "state": "operations_cancelled"},
            "restored selected source cancellation did not finish its physical leave and workflow cleanup")
    expected = copy.deepcopy(before["record"])
    expected["members"][before["member"]["member"]].update(leaving=True, left=True)
    expected["closed"] = True
    require(before["operation_cancelled"] is False and after["operation_cancelled"] is True
            and before["member"] == after["member"] and after["record"] == expected,
            "restored P leave changed its exact selection/completion or did not close the original member")


SELECTED_OPERATION = "selected_add"
SELECTED_ARGUMENTS = {"a": {"left": 2, "right": 3}, "b": {"left": 7, "right": 11}}


def selected_request_text(tag):
    values = SELECTED_ARGUMENTS[tag]
    return (f"driver-shared-selected-{tag}: use the installed selected arithmetic integration "
            f"to add {values['left']} and {values['right']} and report the sum")


class BothSelectedModel(AcceptanceModel):
    request = staticmethod(selected_request_text)

    def __init__(self, directory, events):
        super().__init__(directory, events, selected_a=True)
        self.selected_results = {}
        self.local_results = {}
        self.helper_finished = set()
        self.finish_gates = {tag: threading.Event() for tag in ("a", "b")}
        self.finish_release = threading.Event()
        self.finish_abort = threading.Event()

    @staticmethod
    def helper_answer(tag):
        return f"{tag.upper()}'s reviewed selected instruction is retained; actual use remains its own task."

    def original_context(self, body, results, context):
        tag = {"original-a": "a", "original-b": "b"}[context["recipient"]["original"]["route"]]
        return self.shared_selected_context(body, results, context, tag)

    def shared_selected_context(self, body, results, context, tag):
        initial_state = results.get(f"{tag}-selected-state")
        if initial_state is not None:
            require(initial_state["software_review_observation"] == {
                "scope": "latest_retained_software_attempt_only", "review": None,
            }, "new caller's state disclosed another input's retained software review")
        response = self.selected_context(body, results, context, tag)
        if f"{tag}-selected-reconcile" not in results:
            return response
        selected = self.selected_result
        require(selected["context"]["recipient"]["original"]["route"] == "original-" + tag,
                "selected readback belongs to another caller")
        self.selected_results[tag] = copy.deepcopy(selected)
        reviewed_state = f"{tag}-selected-reviewed-state"
        if reviewed_state not in results:
            return self.call(body, reviewed_state, "setup_state", {}, context)
        observation = results[reviewed_state]["software_review_observation"]
        review = observation["review"]
        require(observation["scope"] == "latest_retained_software_attempt_only"
                and review["call_id"] == f"{tag}-selected-install" and review["tool"] == "setup_selected_install"
                and review["validation_id"] == selected["checked"]["validation_id"]
                and review["retained_install_job_id"] == selected["job"]["id"]
                and review["approval_status"] == "approved" and review["decision"] == "approve"
                and review["dispatch_custody"] == "settled" and review["effect_outcome"] == "committed"
                and review["dispatch_error_kind"] is None,
                "current caller cannot observe its own exact committed software review")
        self.selected_results[tag]["software_review"] = copy.deepcopy(review)
        if f"{tag}-selected-jobs" not in results:
            if "setup_jobs" not in {row["function"]["name"] for row in body.get("tools", [])}:
                return self.call(body, f"{tag}-selected-jobs-load", "tool_catalog_load", {"names": ["setup_jobs"]})
            return self.call(body, f"{tag}-selected-jobs", "setup_jobs", {"before": None}, context)
        jobs = results[f"{tag}-selected-jobs"]
        require(jobs["next_before"] is None and len(jobs["jobs"]) == 1
                and jobs["jobs"][0]["id"] == selected["job"]["id"]
                and jobs["jobs"][0]["kind"] == "install" and jobs["jobs"][0]["status"] == "succeeded",
                "caller acquired more than the one successful selected installation")
        key = f"{tag}-selected-access"
        if key not in results:
            return self.call(body, key, "toolkit_request_public_service", {
                "job_id": selected["job"]["id"], "agent": "original-" + tag,
                "operations": [SELECTED_OPERATION], "expires_in_ms": 180000,
                "setup_acquisition": context["membership"]["identity"]["acquisition"]["id"],
                "tool_policy": {"tools": [], "catalog": "r2"}})
        require(results[key]["grant_issued"] is True and results[key]["fresh_instruction_retained"] is True
                and results[key]["ready"] is False, "selected access did not retain its independent instruction")
        self.helper_finished.add(tag)
        return self.done(self.helper_answer(tag))

    def original_continuation(self, body, results, tag, count):
        require(count == 2, "selected caller received a duplicate or foreign continuation")
        if f"{tag}-local-search" not in results:
            return self.call(body, f"{tag}-local-search", "tool_catalog_search", {"query": SELECTED_OPERATION, "limit": 50})
        require(any(row["name"] == SELECTED_OPERATION for row in results[f"{tag}-local-search"]["results"]),
                "caller cannot discover its own reviewed arithmetic operation")
        if f"{tag}-local-load" not in results:
            return self.call(body, f"{tag}-local-load", "tool_catalog_load", {"names": [SELECTED_OPERATION]})
        if f"{tag}-local-add" not in results:
            return self.call(body, f"{tag}-local-add", SELECTED_OPERATION, SELECTED_ARGUMENTS[tag])
        result = results[f"{tag}-local-add"]
        require(result == {"sum": sum(SELECTED_ARGUMENTS[tag].values())}, "actual arithmetic result differs")
        require(tag not in self.local_results, "selected caller prepared a second final answer")
        answer = f"{tag.upper()} selected arithmetic sum: {result['sum']}"
        self.local_results[tag] = {"arguments": copy.deepcopy(SELECTED_ARGUMENTS[tag]),
                                  "result": copy.deepcopy(result), "answer": answer}
        self.finish_gates[tag].set()
        # Only the caller's final model response is held, after its real tool
        # result. G remains runnable and P remains owned by the unfinished task.
        self.lock.release()
        try:
            require(self.finish_release.wait(max(0, self.original_gates[tag]["deadline"] - time.monotonic())),
                    "both selected callers did not reach the shared preparation observation")
        finally:
            self.lock.acquire()
        require(not self.finish_abort.is_set(), "selected final response aborted during cleanup")
        return self.done(answer)


def require_both_selected_custody(children, selected, preparations, joins, proposals, workflow):
    """Compare owner observations while both real caller responses are held."""
    require(set(children) == set(selected) == set(preparations) == set(joins) == set(proposals) == {"a", "b"},
            "both independently admitted callers are required")
    a, b = preparations["a"], preparations["b"]
    require(a["member"]["key"] == b["member"]["key"] and a["member"]["member"] != b["member"]["member"]
            and a["record"] == b["record"] and not a["record"]["closed"]
            and len(a["record"]["members"]) == 2 and not workflow["sealed"] and len(workflow["waiters"]) == 2,
            "callers did not retain the same open P and workflow")
    require(selected["a"]["checked"] == selected["b"]["checked"]
            and selected["a"]["job"] == selected["b"]["job"]
            and selected["a"]["readback"] == selected["b"]["readback"],
            "selected callers did not reuse the exact physical job and installed receipt")
    require(selected["a"]["software_review"]["approval_id"] != selected["b"]["software_review"]["approval_id"],
            "independent callers reused a software approval")
    require(children["a"]["source_key"] != children["b"]["source_key"]
            and children["a"]["admission"]["session_id"] != children["b"]["admission"]["session_id"]
            and joins["a"]["child"]["grant"]["decision"] != joins["b"]["child"]["grant"]["decision"]
            and joins["a"]["parent_issuance_id"] != joins["b"]["parent_issuance_id"]
            and joins["a"]["source"] != joins["b"]["source"], "caller authority or continuation was shared")
    for tag in ("a", "b"):
        child, preparation, joined = children[tag], preparations[tag], joins[tag]
        member = preparation["record"]["members"][preparation["member"]["member"]]
        require(not preparation["operation_cancelled"] and member["source"] == child["source_key"]
                and member["acquisition"] == selected[tag]["context"]["membership"]["identity"]["acquisition"]
                and not member["leaving"] and not member["left"] and preparation["record"]["completion"] is not None,
                "caller released P before its own native continuation completed")
        waiters = [row for row in workflow["waiters"].values() if row["identity"]["source"] == child["source_key"]]
        require(len(waiters) == 1 and not waiters[0]["departed"], "caller workflow membership departed early")
        require(joined["outcome"] == "succeeded" and joined["custody"] == "joined" and joined["parent_issued"] is True
                and joined["child"]["state"] == "grant_issued" and joined["child"]["grant"]["route"] == child["original"]["route"]
                and joined["child"]["grant"]["decision"] == proposals[tag]["decision"]["decision"]
                and joined["parent_issuance_id"] == proposals[tag]["issuance_id"]
                and proposals[tag]["original"] == child["original"]
                and joined["source"]["route"] == child["original"]["route"], "caller did not receive its own issued native grant")


class BothSelectedScenario(Scenario):
    name = "supplied-shared-selected-both-success"
    scope = ("explicit host-selected shared setup, one actual selected arithmetic package installation, "
             "two independent protected software decisions and native service grants, actual selected_add use and "
             "finalized ordinary continuation for each caller, with both memberships held through use; "
             "controlled local installed integration, no public HN, account/provider authentication, cold recovery or discovery claim")
    request = staticmethod(selected_request_text)
    initial_children = ("a",)

    def configure_owner(self, owner, source):
        super().configure_owner(owner, source)
        owner["native_host"]["service_parent_administration"]["routes"] = ["original-a", "original-b"]
        source["acceptance_selection"].update(requests={tag: self.request(tag) for tag in ("a", "b")},
            public_operation=SELECTED_OPERATION, arguments=copy.deepcopy(SELECTED_ARGUMENTS))

    def configure_plan(self, plan):
        super().configure_plan(plan)
        for member in plan["application"]["tool_policy"]["members"]:
            if member["member_identity"] in ("original-a", "original-b"):
                for grant in member["grants"]:
                    if grant["tool_name"] == s.OPERATION:
                        grant["tool_name"] = SELECTED_OPERATION
                member["grants"].sort(key=lambda row: row["tool_name"])

    def provider(self, directory, events):
        return BothSelectedModel(directory, events)

    async def run_shared(self, private, provider, directory, events, process, base_url):
        deadline = time.monotonic() + 540
        report = {"scope": self.scope, "status": "running", "http": [], "software_decisions": {},
                  "public_proposals": {}, "public_policy_reviews": {}, "public_joins": {}, "native": {}}
        enrollment, token, error = None, None, None
        requests, reservations = {}, {}
        prior_policy = self.finalizer.application_tool_policy
        answered = set()
        released = False
        def http(method, body=None):
            require(process.poll() is None and time.monotonic() < deadline, "selected owner/deadline ended")
            connection = s.HTTPConnection("127.0.0.1", self.port, timeout=10)
            try:
                connection.request(method, c.PANEL + ("/data" if method == "GET" else "/answer"),
                    body=None if body is None else v.canonical(body), headers={"Authorization": "Bearer " + token,
                    "Accept": "application/json", "Origin": self.origin, "Content-Type": "application/json"})
                response = connection.getresponse()
                raw = response.read(4 * 1024 * 1024 + 1)
                require(len(raw) <= 4 * 1024 * 1024, "bounded selected response exceeded")
                value = v.unique_json(raw)
                report["http"].append({"method": method, "request": body, "status": response.status, "response": value})
                if method == "GET" and response.status == 503:
                    require(value == {"error": "Protected setup is unavailable"}, "unknown unavailable result")
                    return None
                require(response.status in (200, 202), "protected selected review refused")
                return value
            finally:
                connection.close()
        try:
            expires = int(time.time() + 650)
            scope = "shared-selected-both-" + uuid.uuid4().hex
            enrollment = private.begin_console_login(v.SDK.ConsoleLoginSelection(c.ISSUER, c.SUBJECT, scope, expires * 1000), timeout=10)
            await asyncio.to_thread(enrollment.result)
            token = c.signed_login(scope, expires)
            self.console_workflow_login, self.console_workflow_token = enrollment, token
            with provider.lock:
                self.begin_model_phase(provider)
            launch = await self.begin_workflow(private, provider, directory, events, process, deadline)
            report["launch"] = launch
            children = {"a": launch["children"][0]}
            g_status, _ = await self.observe_shared(self.workflow_context[4])
            workflow_ref = self.workflow_context[3]["record"]["workflow"]["workflow"]
            while time.monotonic() < deadline:
                self.host_loop_failure.require_alive()
                require(process.poll() is None, "selected gateway exited before both continuations")
                with provider.lock:
                    require(not provider.errors, "selected model refused native observations: " + repr(provider.errors))
                    results, calls, selected, local = copy.deepcopy((provider.results, provider.call_inputs,
                        provider.selected_results, provider.local_results))
                    issued, helpers = set(provider.issued), set(provider.helper_finished)
                data = await asyncio.to_thread(http, "GET")
                review = data["review"] if data else None
                if review and review["prompt"] is not None:
                    key = (review["review_id"], review["prompt"].get("approval_id"))
                    if key not in answered:
                        tags = [tag for tag in children if tag not in report["software_decisions"]
                            and f"{tag}-selected-install" in issued]
                        require(len(tags) == 1, "software review has no unique independently selected caller")
                        tag = tags[0]
                        require(review["can_start"] is False and review["prompt"]["body"] == {
                            "kind": "service_software_install", "check_tool": "setup_selected_check",
                            "call_id": f"{tag}-selected-install", "tool": "setup_selected_install",
                            "scope": "software_installation_only", "selection": calls[f"{tag}-selected-check"]["arguments"],
                            "owner_proposal": results[f"{tag}-selected-check"]}, "selected human review changed source/request")
                        answer = c.approval_answer(review)
                        await asyncio.to_thread(http, "POST", answer)
                        report["software_decisions"][tag] = {"review": review, "answer": answer}
                        answered.add(key)
                for tag in tuple(children):
                    if f"{tag}-selected-access" in issued and tag not in requests:
                        request = await v.observe_installed_service_reply(private.next_service_parent_administration,
                            provider, process, deadline, failure_observer=self.host_loop_failure)
                        requests[tag] = request
                        report["public_proposals"][tag] = copy.deepcopy(request.proposal)
                        review = report["public_policy_reviews"][tag] = {}
                        current_prior = s.capture_public_review(self.finalizer, directory, request, review, prior_policy=prior_policy)
                        reservation = s.PublicReservation(self, provider, request, children[tag], process, deadline,
                            tag=tag, operation=SELECTED_OPERATION, readback_key=f"{tag}-selected-reconcile", prior_policy=current_prior)
                        reservations[tag] = reservation
                        attempt = request.authorize_tool_policy(v.SDK.StandingRuleDecision(reservation), request.tool_policy_sha256)
                        reservation.transferred = True
                        await asyncio.to_thread(attempt.submit, timeout=30)
                        joined = await v.observe_installed_service_reply(attempt.result, provider, process, deadline,
                            failure_observer=self.host_loop_failure)
                        require(joined["outcome"] == "succeeded" and joined["custody"] == "joined" and reservation.released,
                                "selected caller's native authority/source did not join")
                        require(joined["tool_policy"]["native"] == "accepted" and joined["tool_policy"]["retained_as_current"],
                                "complete reviewed caller policy was not retained")
                        report["public_proposals"][tag], report["public_joins"][tag] = request.proposal, joined
                        retired = await asyncio.to_thread(attempt.retire, timeout=30)
                        report.setdefault("public_retirements", {})[tag] = retired
                        require(retired == joined and reservation.released,
                                "selected caller's service exchange did not retire with the same joined observation")
                        prior_policy = request.proposal["tool_policy"]["candidate_canonical_json"].encode()
                if "b" not in children and provider.finish_gates["a"].is_set() and "a" in report["public_joins"]:
                    second = await self.launch_children(private, provider, directory, events, process, deadline, tags=("b",))
                    children["b"] = second["children"][0]
                    report["b_launch"] = second
                if set(local) == helpers == {"a", "b"} and not released:
                    g_rows = s.input_rows(directory, g_status.session_id)
                    g_inputs = {}
                    for tag, child in children.items():
                        delivery = context_delivery(directory, child["source_key"], workflow_ref, g_status.session_id)
                        row = finalized_context(g_rows, delivery, provider.helper_answer(tag)) if delivery else None
                        if row is not None:
                            require(all([item["input_id"] for item in item["contributors"]] == [delivery["input"]]
                                    for item in row["authorization_audit"]), "G setup used foreign caller contributors")
                            g_inputs[tag] = {"delivery": delivery, "native": row,
                                "install": s.public_operation_audit(row["authorization_audit"], "setup_selected_install", f"{tag}-selected-install"),
                                "access": s.public_operation_audit(row["authorization_audit"], "toolkit_request_public_service", f"{tag}-selected-access")}
                    if len(g_inputs) != 2:
                        await asyncio.sleep(0.05)
                        continue
                    # The actual caller contexts have finalized, so G's original
                    # binding must now be published by the existing launch owner.
                    lineage = planning_lineage(directory, v.SDK.PrivateOriginalRef(**launch["commission"]["planning"]),
                                               self.workflow_context[0])
                    header = lineage["bound_service_input"]["header"]
                    require(header.get("authority_association") == lineage["final_association"]
                            and lineage["final_association"]["target"]["context"] == g_status.session_id,
                            "shared G lost its actual original native binding")
                    report["g_lineage"] = lineage
                    require(g_inputs["a"]["delivery"]["input"] != g_inputs["b"]["delivery"]["input"],
                            "caller setup contexts shared one native input")
                    require(len(g_rows) == 3 and {row["input_id"] for row in g_rows} == {header["id"],
                            g_inputs["a"]["delivery"]["input"], g_inputs["b"]["delivery"]["input"]},
                            "G admitted an extra input beyond its original and two caller contexts")
                    preparations = {tag: selected_preparation_record(directory, children[tag], selected[tag],
                        allow_pending=True, expected_members=2) for tag in ("a", "b")}
                    if all(row["record"]["completion"] is not None for row in preparations.values()):
                        workflows = [row["data"]["body"] for row in s.workflow_rows(directory)
                            if row["data"]["schema"] == "meerkat_toolkit.setup_workflow"]
                        require(len(workflows) == 1 and len(answered) == 2, "missing shared workflow or independent software decisions")
                        require_both_selected_custody(children, selected, preparations, report["public_joins"],
                            report["public_proposals"], workflows[0])
                        require(all(selected[tag]["software_review"]["approval_id"]
                                    == report["software_decisions"][tag]["review"]["prompt"]["approval_id"]
                                    for tag in ("a", "b")),
                                "native software observation differs from that caller's actual protected decision")
                        report.update(shared_preparation=preparations, live_workflow=workflows[0], business=local, native_g_inputs=g_inputs)
                        provider.finish_release.set()
                        released = True
                if released:
                    for tag, child in children.items():
                        if tag in report["native"]:
                            continue
                        fresh = v.SDK.PrivateOriginalRef(**report["public_joins"][tag]["source"])
                        observed = await asyncio.to_thread(private.observe_service_input_receipt, fresh, timeout=30)
                        if observed["state"] != "admitted" or observed["receipt"] is None:
                            continue
                        receipt = observed["receipt"]["report"]
                        if receipt["status"] != "finalized":
                            continue
                        require(observed["session_id"] == child["admission"]["session_id"]
                            and observed["input_id"] != child["admission"]["input_id"]
                            and receipt["owner_input_id"] == observed["input_id"]
                            and receipt["recipient_input_ids"] == [observed["input_id"]]
                            and receipt["terminal"] == {"outcome_type": "consumed"}
                            and receipt["outcome"]["completion_type"] == "completed"
                            and receipt["outcome"]["text"] == local[tag]["answer"] and receipt["outcome"]["tool_calls"] == 3,
                            "selected answer is not this caller's own finalized native result")
                        rows = s.input_rows(directory, observed["session_id"])
                        require({row["input_id"] for row in rows} == {child["admission"]["input_id"], observed["input_id"]}
                                and len(rows) == 2, "selected caller has extra native inputs")
                        current = next(row for row in rows if row["input_id"] == observed["input_id"])
                        audit = current["authorization_audit"]
                        require(audit and all([item["input_id"] for item in row["contributors"]] == [observed["input_id"]]
                                and row["observation"]["run_id"] == receipt["run_id"] for row in audit),
                                "selected operation used another caller's authority")
                        report["native"][tag] = {"receipt": observed, "inputs": rows,
                            "operation": s.public_operation_audit(audit, SELECTED_OPERATION, f"{tag}-local-add")}
                    if len(report["native"]) == 2:
                        workflow = [row["data"]["body"] for row in s.workflow_rows(directory)
                            if row["data"]["schema"] == "meerkat_toolkit.setup_workflow"]
                        require(len(workflow) == 1 and len(workflow[0]["waiters"]) == 2, "shared workflow disappeared")
                        if workflow[0]["sealed"] and all(row["departed"] for row in workflow[0]["waiters"].values()):
                            self.host_loop_failure.require_alive()
                            report.update(status="passed", final_workflow=workflow[0], g_customizer=self.verify_customizer())
                            break
                self.host_loop_failure.require_alive()
                await asyncio.sleep(0.05)
            self.host_loop_failure.require_alive()
            require(report["status"] == "passed", "both selected callers did not complete before the deadline")
        except BaseException as failure:
            error = failure
            report.update(status="failed", error=repr(failure),
                selected_host_failure=self.host_loop_failure.observation())
            raise
        finally:
            provider.finish_abort.set()
            provider.finish_release.set()
            provider.release_original_responses(abort=True)
            cleanup_errors = []
            for tag, request in requests.items():
                try:
                    report.setdefault("service_retirement", {})[tag] = await asyncio.to_thread((request.attempt or request).retire, timeout=30)
                except BaseException as cleanup:
                    cleanup_errors.append(repr(cleanup))
            if enrollment is not None:
                try:
                    retired = await asyncio.to_thread(enrollment.retire, timeout=30)
                    require(retired.fenced and retired.reservations_drained and not retired.uncertain, "login retirement unconfirmed")
                    report["login_retirement"] = dataclasses.asdict(retired)
                except BaseException as cleanup:
                    cleanup_errors.append(repr(cleanup))
            if cleanup_errors:
                report.update(status="failed", cleanup_errors=cleanup_errors)
            v.save_json(directory / "console-journey.json", report)
            if cleanup_errors and error is None:
                raise AssertionError("selected cleanup failed: " + repr(cleanup_errors))
        return report


class ColdSharedScenario(Scenario):
    """Two actual native processes, one commission, fresh B after joined Restore."""
    restore_after_shutdown = True
    phase = "prepared"
    initial_children = ("a",)
    withhold_p_completion = False
    name = "supplied-shared-cold-restore-success"
    scope = ("one genuine planning parent/G commission, actual protected selected fixture installation for A, "
             "joined process close and current dynamic-class/physical G Restore, fresh B admission, exact settled preparation P cleanup "
             "through original cancellation before B setup effects and public HN read; controlled fixture/model, "
             "no recommission, lost-reply completion publication, already-cancelled replay, old effect replay, discovery or account claim")

    def provider(self, directory, events):
        return AcceptanceModel(directory, events, selected_a=True)

    async def wait_selected_a(self, child, member, provider, directory, process, deadline, token):
        old, before = await self.observe_shared(member)
        workflow = before["canonical"]["record"]["workflow"]["workflow"]
        answered = None
        completion_fault = False
        pending_preparation = None
        http_rows = []
        def http(method, body=None):
            require(process.poll() is None and time.monotonic() < deadline, "selected A owner/deadline ended")
            connection = s.HTTPConnection("127.0.0.1", self.port, timeout=10)
            try:
                path = c.PANEL + ("/data" if method == "GET" else "/answer")
                connection.request(method, path, body=None if body is None else v.canonical(body), headers={
                    "Authorization": "Bearer " + token, "Accept": "application/json", "Origin": self.origin,
                    "Content-Type": "application/json"})
                response = connection.getresponse()
                raw = response.read(4 * 1024 * 1024 + 1)
                require(len(raw) <= 4 * 1024 * 1024, "bounded selected A response exceeded")
                value = v.unique_json(raw)
                http_rows.append({"method": method, "status": response.status, "request": body, "response": value})
                if method == "GET" and response.status == 503:
                    require(value == {"error": "Protected setup is unavailable"}, "unknown unavailable result")
                    return None
                require(response.status in (200, 202), "protected selected A review refused")
                return value
            finally:
                connection.close()
        try:
            while time.monotonic() < deadline:
                self.host_loop_failure.require_alive()
                with provider.lock:
                    require(not provider.errors and not provider.b_context.is_set()
                            and not any(key.startswith(("b-", "g-")) for key in provider.issued), "unselected work before cold close")
                    results, calls, selected = copy.deepcopy((provider.results, provider.call_inputs, provider.selected_result))
                data = await asyncio.to_thread(http, "GET")
                review = data["review"] if data else None
                if review and review["prompt"] is not None:
                    key = (review["review_id"], review["prompt"].get("approval_id"))
                    if key != answered:
                        require(answered is None, "unexpected second selected A review")
                        body = review["prompt"]["body"]
                        require(review["can_start"] is False and body == {"kind": "service_software_install",
                            "check_tool": "setup_selected_check", "scope": "software_installation_only",
                            "call_id": "a-selected-install", "tool": "setup_selected_install",
                            "selection": calls["a-selected-check"]["arguments"], "owner_proposal": results["a-selected-check"]},
                            "protected selected A review differs from its actual native check")
                        if self.withhold_p_completion:
                            selected_completion_fault(directory, child)
                            completion_fault = True
                        await asyncio.to_thread(http, "POST", c.approval_answer(review))
                        answered = key
                delivery = context_delivery(directory, child["source_key"], workflow, old.session_id)
                rows = s.input_rows(directory, old.session_id)
                if selected is not None and delivery and finalized_context(rows, delivery, SELECTED_A_ANSWER):
                    require(answered is not None, "selected installation skipped protected approval")
                    actual = next(row for row in rows if row["input_id"] == delivery["input"])
                    require(all([item["input_id"] for item in row["contributors"]] == [delivery["input"]]
                                for row in actual["authorization_audit"]), "selected P used foreign native input authority")
                    native_install = s.public_operation_audit(actual["authorization_audit"], "setup_selected_install", "a-selected-install")
                    preparation = selected_preparation_record(directory, child, selected, allow_pending=True)
                    if self.withhold_p_completion:
                        require(results["a-selected-install"]["job"] == selected["job"]
                                and selected["job"]["status"] == "succeeded" and selected["job"].get("error") is None
                                and sum(value["name"] == "setup_selected_install" for value in calls.values()) == 1,
                                "publication recovery requires one actual joined installation, without replay")
                    if completion_fault:
                        require(preparation["record"]["completion"] is None,
                                "P publication bypassed the selected store fault")
                        pending_preparation = copy.deepcopy(preparation)
                        self.cold["selected_p_publication"] = {"pending": pending_preparation,
                            "native_install": native_install, "native_context": actual,
                            "selected": selected,
                            "scope": "native installation finalized while exact P publication was unavailable"}
                        selected_completion_fault(directory, child, release=True)
                        completion_fault = False
                    if preparation["record"]["completion"] is None:
                        await asyncio.sleep(0.05)
                        continue
                    if self.withhold_p_completion:
                        require(pending_preparation is not None, "P completion fault was not exercised")
                        expected = copy.deepcopy(pending_preparation)
                        expected["record"]["completion"] = preparation["record"]["completion"]
                        require(preparation == expected, "P recovery changed selection, membership or request")
                        retained = self.cold["selected_p_publication"]
                        require(retained["native_context"] == actual and retained["native_install"] == native_install
                                and retained["selected"] == selected,
                                "P publication repeated native input or changed the retained physical result")
                        self.cold["selected_p_publication"]["completed"] = preparation
                    self.cold["selected_a"] = {**selected, "native_install": native_install, "preparation": preparation}
                    require(preparation["operation_cancelled"] is False and preparation["record"]["closed"] is False
                            and not any(row["left"] or row["leaving"] for row in preparation["record"]["members"].values()),
                            "A must retain its selected preparation through orderly close")
                    self.host_loop_failure.require_alive()
                    return old, before, delivery, rows
                await asyncio.sleep(0.05)
            self.host_loop_failure.require_alive()
            raise AssertionError("selected A installation/native completion was not observed")
        finally:
            self.cold["selected_a_http"] = http_rows
            if completion_fault:
                primary = sys.exc_info()[1]
                try:
                    selected_completion_fault(directory, child, release=True)
                except BaseException as cleanup:
                    self.cold["selected_p_publication_cleanup_error"] = {
                        "type": type(cleanup).__name__, "message": str(cleanup)}
                    if primary is None:
                        raise
                    primary.add_note("P publication fault cleanup failed: " + str(cleanup))

    async def cancel_a(self, private, directory, launch, deadline):
        child = self.first_launch["children"][0]
        require(self.phase == "restored" and launch["children"][0] == child
                and len(launch["children"]) == 2
                and launch["children"][1]["original"]["route"] == "original-b",
                "restored A cleanup requires the original A and independently admitted B")
        selected = self.cold["selected_a"]
        # File equality is only correlation. The single owner command below
        # must resolve the retained physical P and join its leave before reply.
        require(selected_preparation_record(directory, child, selected) == selected["preparation"],
                "Restore changed A's retained selected preparation before cancellation")
        reply = await asyncio.to_thread(private.cancel_original_operation,
            v.SDK.PrivateOriginalRef(**child["original"]), timeout=30)
        cancellation = dataclasses.asdict(reply)
        self.cold["selected_p_cancellation_reply"] = cancellation
        after = selected_preparation_record(directory, child, selected)
        require_selected_p_leave(selected["preparation"], after, cancellation, child["original"])
        workflow = await s.wait_departed(directory, child["source_key"], deadline,
            remaining_source=launch["children"][1]["source_key"], failure_observer=self.host_loop_failure)
        self.cold["selected_p_cleanup"] = {"cancellation": cancellation, "after": after,
            "workflow_after_departure": workflow,
            "scope": "owner-acknowledged restored settled-P leave; not new completion publication or effect replay"}
        self.cold["departure"] = {"cancel_a": cancellation,
            "a_before": await s.wait_initial_a_completion(directory, child["admission"], deadline,
            failure_observer=self.host_loop_failure)}
        return self.cold["departure"]

    async def check_ordinary_rebuild(self, *args):
        # Ordinary class Add/Restore has its own finite acceptance case.
        self.c3 = {"status": "not_selected"}

    def observe_runtime(self, runtime):
        if self.phase == "prepared":
            super().observe_runtime(runtime)
        else:
            self.runtime = runtime

    def observe_completed_setup(self, setup, pin):
        self.completed_pin = copy.deepcopy(pin)

    def restore_runtime_options(self):
        require(self.cold["status"] == "prepared_for_joined_close" and self.roster.publications == 1
                and self.roster.probe_publications == 0,
                "cold Restore has no unique original application admission")
        for pin in (self.completed_pin, self.cold["roster"], self.cold["admitting"]):
            v.exact_bytes(Path(pin["path"]), pin["sha256"])
        self.member_mcp = self.member_types.MemberMcpApplication(self.roster.retain_empty_mcp, self.roster.lookup_empty_mcp)
        return {"private_workflow_launch": True, "member_mcp": self.member_mcp}

    def begin_model_phase(self, provider):
        require(self.phase == "restored" and self.cold["status"] == "restored_awaiting_b"
                and len(provider.requests) == self.cold["model_requests"] and not provider.errors
                and not provider.b_context.is_set() and "b-find" not in provider.issued,
                "Restore performed unselected model work before fresh B")
        provider.enabled = True

    async def begin_workflow(self, private, provider, directory, events, process, deadline):
        if self.phase == "prepared":
            return await super().begin_workflow(private, provider, directory, events, process, deadline)
        require(self.cold["status"] == "restored_awaiting_b", "B cannot bypass actual Restore observation")
        launch = await self.launch_children(private, provider, directory, events, process, deadline, tags=("b",))
        require([row["original"]["route"] for row in launch["children"]] == ["original-b"],
                "cold phase repeated predecessor input")
        launch["children"].insert(0, copy.deepcopy(self.first_launch["children"][0]))
        self.g_b_source = launch["children"][1]["source_key"]
        self.g_directory = directory
        return launch

    async def run(self, private, provider, directory, events, process, base_url):
        deadline = time.monotonic() + 540
        enrollment = None
        try:
            if self.phase == "prepared":
                self.cold = {"status": "running", "scope": self.scope, "first_pid": process.pid}
                scope, expires = "cold-prepared-" + uuid.uuid4().hex, int(time.time() + 650)
                enrollment = private.begin_console_login(v.SDK.ConsoleLoginSelection(c.ISSUER, c.SUBJECT,
                    scope, expires * 1000), timeout=10)
                await asyncio.to_thread(enrollment.result)
                self.console_workflow_login, self.console_workflow_token = enrollment, c.signed_login(scope, expires)
                with provider.lock:
                    s.Scenario.begin_model_phase(self, provider)
                self.first_launch = await self.begin_workflow(private, provider, directory, events, process, deadline)
                child = self.first_launch["children"][0]
                require(len(self.first_launch["children"]) == 1 and child["original"]["route"] == "original-a",
                        "cold first process admitted B")
                member = self.workflow_context[4]
                old, before, delivery, rows = await self.wait_selected_a(child, member, provider,
                    directory, process, deadline, c.signed_login(scope, expires))
                a_rows = await s.wait_initial_a_completion(directory, child["admission"], deadline,
                    failure_observer=self.host_loop_failure)
                require(finalized_context(a_rows, {"input": child["admission"]["input_id"],
                    "session": child["admission"]["session_id"]},
                    "The independently authenticated task is waiting for setup."),
                    "A did not finalize its own successful waiting answer")
                parent = self.first_launch["parent"]["admission"]
                parent_rows = s.input_rows(directory, parent["session_id"])
                require(len(parent_rows) == 1 and finalized_context(parent_rows,
                    {"input": parent["input_id"], "session": parent["session_id"]}, "Planning parent is ready."),
                    "P did not finalize its own admitted planning input")
                p, _, result, _, _, _, _ = self.workflow_context
                planning = v.SDK.PrivateOriginalRef(**result["planning"])
                self.host_loop_failure.require_alive()
                self.cold.update(status="prepared_for_joined_close", before=before, a_delivery=delivery,
                    before_inputs=rows, parent_inputs=parent_rows,
                    callbacks_before=await self.customizer.wait_shared_created(1, old.session_id, deadline,
                        failure_observer=self.host_loop_failure),
                    build_contexts_before=copy.deepcopy(self.customizer.shared_build_contexts),
                    lineage=planning_lineage(directory, planning, p),
                    peer_before=shared_peer(directory, process.pid, self.resource_python, observed=before),
                    roster=v.artifact_pin(self.roster.path), admitting=v.artifact_pin(self.roster.admitting))
                self.host_loop_failure.require_alive()
                with provider.lock:
                    require(not provider.errors and not provider.b_context.is_set()
                            and not any(key.startswith(("b-", "g-")) for key in provider.issued),
                            "cold predecessor performed B work or requested service access")
                    self.cold["model_requests"] = len(provider.requests)
                    provider.enabled = False
                self.host_loop_failure.require_alive()
                return self.cold
            require(self.phase == "restored" and process.pid != self.cold["first_pid"],
                    "cold acceptance did not launch a second actual gateway")
            p, _, result, _, member, _, _ = self.workflow_context
            _, after = await self.observe_shared(member)
            require_cold_g_transition(self.cold["before"], after)
            require(planning_lineage(directory, v.SDK.PrivateOriginalRef(**result["planning"]), p) == self.cold["lineage"],
                    "Restore changed the original helper or bound planning lineage")
            require(s.input_rows(directory, after["status"]["session_id"]) == self.cold["before_inputs"],
                    "Restore replayed or changed predecessor G inputs")
            parent_session = self.first_launch["parent"]["admission"]["session_id"]
            require(s.input_rows(directory, parent_session) == self.cold["parent_inputs"],
                    "Restore replayed or changed the original P input")
            # Cold Restore reattaches Core's retained scope. Meta republishes
            # the dispatcher before resume, without a new session/after_create.
            # Only the separate explicit respawn mode rejects its old scope.
            peer = shared_peer(directory, process.pid, self.resource_python, observed=after)
            self.cold.update(after=after, second_pid=process.pid, peer_after=peer,
                physical_snapshot=require_cold_transport(self.cold["peer_before"], peer),
                callbacks_after=require_cold_g_callbacks(self.customizer, self.cold, after["status"]["session_id"]),
                status="restored_awaiting_b")
            # The existing B-context gate invokes cancel_a only after B joins.
            # A's final departure must not seal the workflow before that join.
            report = await s.Scenario.run(self, private, provider, directory, events, process, base_url)
            require(self.cold["status"] == "fresh_b_work_observed" and self.roster.publications == 1
                    and self.roster.probe_publications == 0, "cold use repeated admission or omitted G dispatch")
            before = self.cold["before"]
            session = before["status"]["session_id"]
            lineage = self.cold["lineage"]
            header = lineage["bound_service_input"]["header"]
            require(header.get("authority_association") == lineage["final_association"]
                    and lineage["final_association"]["target"]["context"] == session
                    and sum(row["input_id"] == header["id"] for row in self.cold["before_inputs"]) == 1,
                    "G close selection lost original native input correlation")
            self.host_loop_failure.require_alive()
            self.cold["final_close"] = {}
            await wait_finalized_g_close(directory, before["canonical"]["record"]["workflow"]["workflow"],
                session, header["id"], deadline, self.cold["final_close"],
                failure_observer=self.host_loop_failure)
            self.host_loop_failure.require_alive()
            report["cold_restore"] = copy.deepcopy(self.cold)
            return report
        except BaseException as failure:
            provider.release_original_responses(abort=True)
            self.cold.update(status="failed", error=repr(failure),
                selected_host_failure=self.host_loop_failure.observation())
            raise
        finally:
            try:
                if enrollment is not None:
                    retired = await asyncio.to_thread(enrollment.retire, timeout=30)
                    require(retired.fenced and retired.reservations_drained and not retired.uncertain,
                            "cold predecessor login retirement unconfirmed")
                    self.cold["login_retirement"] = dataclasses.asdict(retired)
                if self.commission_attempt is not None:
                    await self.commission_attempt.join_listing()
            except BaseException as failure:
                self.cold.update(status="failed", cleanup_error=repr(failure))
                provider.release_original_responses(abort=True)
                raise
            finally:
                v.save_json(directory / ("shared-cold-" + self.phase + ".json"), self.cold)

    def verify_customizer(self):
        # This is the same real B context/entry/outcome oracle as warm G;
        # predecessor continuity was established by the actual process Restore.
        result = self.verify_fresh_b(self.cold, "restored_awaiting_b", "shared-cold-restored.json")
        result["cold_restore"] = result.pop("continuity")
        return result


def require_helperless_restore_pre(before, observed):
    """Cold retained Restore is idle until the existing governed input."""
    require(before["builds"] == [{"identity": C3_ID, "profile": C3_CLASS}]
            and len(before["build_contexts"]) == 1,
            "helperless prepared callback baseline is not one real build")
    for key in ("builds", "build_contexts", "created", "created_contexts", "calls", "model_requests"):
        require(observed[key] == before[key], "cold Restore changed pre-input " + key)
    require(not observed["callbacks"] and not observed["callback_overflow"],
            "cold Restore invoked a customizer or tool before the governed input")


def require_helperless_restore_wake(before, pre_input, after_input):
    """Compare only observations from the actual retained cold-wake interval."""
    require_helperless_restore_pre(before, pre_input)
    require(after_input["builds"] == before["builds"] * 2
            and after_input["build_contexts"] == before["build_contexts"] * 2
            and after_input["created"] == before["created"]
            and after_input["created_contexts"] == before["created_contexts"],
            "cold wake did not make exactly one unchanged build without after_create")
    require(after_input["calls"] == [{"nonce": "before", "handler_build": 1},
                                     {"nonce": "after", "handler_build": 2}],
            "cold wake did not dispatch the current handler-2 nonce")
    rows = after_input["callbacks"]
    require(not after_input["callback_overflow"]
            and [row["method"] for row in rows] == ["callback/agent_customizer/customize_build", "callback/call_tool"]
            and all(row["completed"] is True and "exception" not in row for row in rows),
            "cold wake callback interval is missing, extra, incomplete or failed")
    build, call = rows
    prior = [row for row in before["callbacks"] if row["method"] == "callback/agent_customizer/customize_build"]
    require(len(prior) == 1 and prior[0]["completed"] is True
            and build["identity"] == build["spec_identity"] == C3_ID
            and build["profile"] == C3_CLASS
            and isinstance(build["scope_id"], str) and build["scope_id"],
            "cold wake lacks the exact original member's actual build scope")
    def context(value):
        require(isinstance(value, dict) and set(value) == {"identity", "active_peers", "managed_edges"}
                and isinstance(value["active_peers"], list) and isinstance(value["managed_edges"], list),
                "cold wake build context is unavailable")
        return (value["identity"], value["active_peers"],
                {json.dumps(edge, sort_keys=True, separators=(",", ":")) for edge in value["managed_edges"]})
    require(context(build["context"]) == context(prior[0]["context"]),
            "cold wake changed its identity, active peers or managed-edge set")
    require(call["scope_id"] == build["scope_id"] and call["tool"] == C3_TOOL
            and call["nonce"] == "after"
            and call["handler_result"] == {"nonce": "after", "handler_build": 2},
            "cold wake tool call did not use this restored process's build scope")
    # Scope counters restart in a new process. No cross-process inequality.
    return {"scope_id": build["scope_id"], "new_builds": 1, "restored_after_create": 0,
            "handler_result": copy.deepcopy(call["handler_result"])}


class HelperlessScenario(Scenario):
    failure_member_id = C3_ID
    helperless_restore = True
    ordinary_customizer_only = True
    restore_after_shutdown = True
    phase = "prepared"
    name = "supplied-helperless-restore-success"
    scope = ("zero initial native members and no setup helper; actual held publication/adoption, "
             "one class Add and nonce callback, joined shutdown and ordinary process Restore of the "
             "same canonical admission/session with a fresh callback and second native input; "
             "no external service, account, workflow, shared-G recovery or schedule claim")

    def configure_owner(self, owner, source):
        c.ConsoleScenario.configure_owner(self, owner, source)
        native = owner["native_host"]
        self.ordinary_route = copy.deepcopy(native["routes"][0])
        self.ordinary_route["id"] = C3_ID
        self.ordinary_route["origin"]["member"].update(member=C3_ID, role=C3_CLASS)
        self.ordinary_route["logical_executor"] = v.installed_fixture_principal(C3_ID)
        for field in ("application_parent", "controller_parent"):
            self.ordinary_route.pop(field, None)
        native["members"], native["routes"] = [], []
        source.update(purpose=self.scope, later_original_route=copy.deepcopy(self.ordinary_route))

    def configure_plan(self, plan):
        app = plan["application"]
        app["roster"], app["members"] = [], []
        for field in ("helper", "helper_tools", "helper_profile"):
            app.pop(field)
        app["definition"]["text"] = ('[mob]\nid = "fixture"\n[profiles.' + C3_CLASS + ']\n'
            'external_addressable = true\nmodel = "fixture-original-model"\nprovider = "self_hosted"\n'
            'self_hosted_server_id = "local"\nruntime_mode = "turn_driven"\n'
            '[profiles.' + C3_CLASS + '.tools]\ncomms = true\n')
        declared = {"id": C3_CLASS, "profile": C3_CLASS, "factory": "acceptance-nonce-factory",
            "callback_account": {"kind": "declared_none"},
            "callbacks": [{"name": C3_TOOL, "description": C3_DESCRIPTION, "input_schema": copy.deepcopy(C3_SCHEMA)}],
            "grants": sorted([{"tool_name": name, "action": "invoke", "consequence": "r0" if name == C3_TOOL else "r2"}
                              for name in (C3_TOOL, "tool_catalog_search", "tool_catalog_load")], key=lambda row: row["tool_name"])}
        app["member_classes"] = [declared]
        app["tool_policy"]["members"] = []
        plan["owner"]["native_host"]["member_classes"] = [copy.deepcopy(declared)]
        plan["authorities"] = {}
        plan.pop("acquisition")
        self.plan = plan

    def configure_runtime(self, config, directory):
        c.ConsoleScenario.configure_runtime(self, config, directory)
        self.configure_error_observer(config)
        self.customizer = AcceptanceCustomizer("fixture-original-model")
        config.agent_customizer = self.customizer
        self.roster = DesiredRoster(directory, [], helperless=True)
        self.member_types = importlib.import_module("meerkat_toolkit.member_classes")
        self.member_mcp = self.member_types.MemberMcpApplication(self.roster.retain_empty_mcp, self.roster.lookup_empty_mcp)
        config.roster_provider = config.topology_provider = self.roster
        self.c3 = {"status": "not_started", "scope": self.scope}
        self.probe_attempt = None
        self.callback_observations = {}
        self.callback_dispatchers = []
        self.callback_lock = threading.Lock()

    def application_runtime_options(self):
        return {"member_mcp": self.member_mcp}

    def restore_runtime_options(self):
        # Shutdown permanently closes the prior SDK participant. Keep the exact
        # durable application rows; select a fresh participant over those rows.
        require(self.roster.selected() == [self.roster.probe_spec()] and self.roster.probe_publications == 1,
                "Restore lost the exact application listing")
        self.member_mcp = self.member_types.MemberMcpApplication(self.roster.retain_empty_mcp, self.roster.lookup_empty_mcp)
        return {"member_mcp": self.member_mcp}

    def observe_runtime(self, runtime):
        self.runtime = runtime
        phase = self.phase
        require(phase not in self.callback_observations, "helperless runtime phase was repeated")
        observation = {"rows": [], "overflow": False}
        self.callback_observations[phase] = observation
        dispatcher = runtime._dispatcher
        original = dispatcher.handle_callback

        async def observed(method, params):
            row = None
            if method in ("callback/agent_customizer/customize_build",
                          "callback/agent_customizer/after_create", "callback/call_tool"):
                # Observe only identifiers/context needed by this fixture. Forward
                # the same callback once; no replacement reply or extra await.
                value = params if isinstance(params, dict) else {}
                row = {"method": method, "completed": False}
                if method.endswith("/customize_build"):
                    context, spec = value.get("context"), value.get("spec")
                    context = context if isinstance(context, dict) else {}
                    spec = spec if isinstance(spec, dict) else {}
                    row.update(scope_id=value.get("scope_id"), identity=context.get("identity"),
                        spec_identity=spec.get("identity"), profile=spec.get("profile"),
                        context=copy.deepcopy({key: context.get(key) for key in
                            ("identity", "active_peers", "managed_edges")}))
                elif method == "callback/call_tool":
                    arguments = value.get("arguments")
                    row.update(scope_id=value.get("scope_id"), tool=value.get("tool"),
                        nonce=arguments.get("nonce") if isinstance(arguments, dict) else None)
                else:
                    row.update(identity=value.get("identity"), session_id=value.get("session_id"))
                with self.callback_lock:
                    if len(observation["rows"]) < 64:
                        observation["rows"].append(row)
                    else:
                        observation["overflow"] = True
            try:
                result = await original(method, params)
            except BaseException as error:
                if row is not None:
                    with self.callback_lock:
                        row["exception"] = type(error).__name__
                raise
            else:
                if row is not None:
                    with self.callback_lock:
                        row["completed"] = True
                        if method == "callback/call_tool":
                            content = result.get("content") if isinstance(result, dict) else None
                            row["handler_result"] = ({key: content.get(key) for key in ("nonce", "handler_build")}
                                if isinstance(content, dict) else None)
                return result

        # _bootstrap later registers this exact instance callback before start.
        dispatcher.handle_callback = observed
        self.callback_dispatchers.append((dispatcher, original, observed))

    def callback_snapshot(self, provider):
        with self.callback_lock:
            observation = copy.deepcopy(self.callback_observations[self.phase])
        with provider.lock:
            model_requests = len(provider.requests)
        return {"phase": self.phase, "builds": copy.deepcopy(self.customizer.builds),
            "build_contexts": copy.deepcopy(self.customizer.build_contexts),
            "created": copy.deepcopy(self.customizer.created),
            "created_contexts": copy.deepcopy(self.customizer.created_contexts),
            "calls": copy.deepcopy(self.customizer.calls), "model_requests": model_requests,
            "callbacks": observation["rows"], "callback_overflow": observation["overflow"]}

    def check_final_artifacts(self, directory, report):
        try:
            super().check_final_artifacts(directory, report)
            with self.callback_lock:
                final = copy.deepcopy(self.callback_observations)
            v.save_json(directory / "helperless-callbacks-final.json", final)
            report["helperless_callbacks_final"] = v.artifact_pin(directory / "helperless-callbacks-final.json")
            if report.get("status") == "passed":
                # This existing hook runs after native shutdown/readers/provider
                # cleanup. No late callback may escape the successful interval.
                require(final["restored"] == {"rows": self.c3["restore_after_input"]["callbacks"], "overflow": False},
                        "restored process emitted a callback outside the governed-input interval")
                require(self.customizer.created == self.prepared_callbacks["created"]
                        and self.customizer.created_contexts == self.prepared_callbacks["created_contexts"],
                        "restored process gained a late agent_customizer after_create")
        finally:
            for dispatcher, original, observed in self.callback_dispatchers:
                dispatcher.handle_callback = original

    def observe_completed_setup(self, setup, pin):
        v.require_helperless_completed(setup, self.plan["application"], Path(self.plan["output"]))
        self.completed_pin = copy.deepcopy(pin)

    async def run(self, private, provider, directory, events, process, base_url):
        require(self.runtime.host_control is private and "--private-workflow-launch" not in process.args,
                "helperless acceptance selected a different host or workflow launcher")
        require(self.runtime._transport._callback_handler is self.callback_dispatchers[-1][2],
                "helperless native callback observer was not registered on this runtime")
        deadline = time.monotonic() + 180
        report = {"scope": self.scope, "phase": self.phase, "status": "running", "pid": process.pid}
        enrollment = None
        try:
            self.host_loop_failure.require_alive()
            enrollment = private.begin_console_login(v.SDK.ConsoleLoginSelection(
                c.ISSUER, c.SUBJECT, "helperless-" + self.phase + "-" + uuid.uuid4().hex,
                int(time.time() + 240) * 1000), timeout=10)
            await asyncio.to_thread(enrollment.result)
            if self.phase == "prepared":
                require(await self.runtime.mob_handle().list_members() == []
                        and self.roster.selected() == [] and not self.customizer.builds and not self.customizer.created,
                        "zero-start Ready contained an initial member or callback")
                with provider.lock:
                    require(not provider.requests, "zero-start adoption invoked a model")
                    provider.enabled = True
                report["zero_start"] = {"native_members": [], "model_requests": 0, "callbacks": 0}
                await self.check_ordinary_rebuild(private, provider, directory, deadline)
                require(self.c3["status"] == "added_and_used_before_process_restore",
                        "helperless first process did not finish actual Add/use")
                self.first_pid = process.pid
                self.retained_listing = v.artifact_pin(self.roster.path)
                self.retained_mcp = v.artifact_pin(self.roster.probe_admitting)
                self.retained_inputs = copy.deepcopy(self.c3["before_use"]["inputs"])
                self.prepared_callbacks = self.callback_snapshot(provider)
                report["prepared_callbacks"] = copy.deepcopy(self.prepared_callbacks)
            else:
                require(self.phase == "restored" and process.pid != self.first_pid,
                        "Restore did not start a second native process")
                for pin in (self.retained_listing, self.retained_mcp, self.completed_pin):
                    v.exact_bytes(Path(pin["path"]), pin["sha256"])
                member = self.ordinary_route["origin"]["member"]
                status, after = await observe_ordinary(self, member)
                before = self.c3["before"]
                report["restore_observation"] = after
                require(status.identity == C3_ID and status.session_id == before["status"]["session_id"]
                        and status.state == "active",
                        "Restore did not rebuild the same ordinary canonical session")
                # Ordinary cold Restore reattaches Core's retained generation
                # and fence. Only explicit respawn selects a successor scope.
                require(status.agent_runtime_id == before["status"]["agent_runtime_id"]
                        and after["status"]["generation"] == before["status"]["generation"]
                        and isinstance(after["status"]["delivery_scope"], dict)
                        and after["status"]["delivery_scope"]
                        and after["status"]["delivery_scope"] == before["status"]["delivery_scope"],
                        "ordinary Restore changed its retained native delivery scope")
                require_ordinary_owner(before, after)
                require(after["canonical"]["record"] == before["canonical"]["record"],
                        "ordinary Restore changed its retained canonical member")
                self.c3["restored"] = after
                # Meta 0.9.6 cold Restore keeps recorded tools and performs no
                # build before the existing governed input wakes this member.
                self.c3["restore_pre_input"] = self.callback_snapshot(provider)
                report["restore_pre_input"] = copy.deepcopy(self.c3["restore_pre_input"])
                v.save_json(directory / "helperless-pre-input.json", self.c3["restore_pre_input"])
                require_helperless_restore_pre(self.prepared_callbacks, self.c3["restore_pre_input"])
                require(after["tools"]["tools_source"] == "recorded",
                        "cold Restore did not retain recorded tools before input")
                require(s.input_rows(directory, status.session_id) == self.retained_inputs,
                        "Restore modified earlier native input or began unsolicited work")
                try:
                    self.c3["after_use"] = await invoke_ordinary(self, private, directory, deadline, "after")
                finally:
                    self.c3["restore_after_input"] = self.callback_snapshot(provider)
                    report["restore_after_input"] = copy.deepcopy(self.c3["restore_after_input"])
                    v.save_json(directory / "helperless-after-input.json", self.c3["restore_after_input"])
                self.c3["restored_callbacks"] = require_helperless_restore_wake(self.prepared_callbacks,
                    self.c3["restore_pre_input"], self.c3["restore_after_input"])
                require_c3_history(provider.c3_histories["before"], provider.c3_histories["after"])
                rows = self.c3["after_use"]["inputs"]
                require(len(self.retained_inputs) == 1 and len(rows) == 2
                        and all(row in rows for row in self.retained_inputs)
                        and self.customizer.calls == [{"nonce": "before", "handler_build": 1},
                                                     {"nonce": "after", "handler_build": 2}]
                        and self.roster.probe_publications == 1 and self.roster.publications == 0,
                        "Restore replayed Add/work or failed its actual replacement callback")
                self.c3["status"] = "ordinary_process_restore_used"
            with provider.lock:
                require(not provider.errors and provider.issued and all(key.startswith("c3-") for key in provider.issued),
                        "helperless mode entered unrelated model work")
            self.host_loop_failure.require_alive()
            report.update(status="passed", ordinary_class=copy.deepcopy(self.c3))
        except BaseException as error:
            report.update(status="failed", error=repr(error),
                selected_host_failure=self.host_loop_failure.observation())
            raise
        finally:
            try:
                try:
                    if enrollment is not None:
                        retired = await asyncio.to_thread(enrollment.retire, timeout=30)
                        require(retired.fenced and retired.reservations_drained and not retired.uncertain,
                                "helperless login retirement unconfirmed")
                        report["login_retirement"] = dataclasses.asdict(retired)
                finally:
                    if self.probe_attempt is not None:
                        await self.probe_attempt.join_listing()
            except BaseException as error:
                report.update(status="failed", cleanup_error=repr(error))
                raise
            finally:
                v.save_json(directory / ("helperless-" + self.phase + ".json"), report)
        return report


def parse_args(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--toolkit-gateway", type=Path, required=True)
    parser.add_argument("--toolkit-gateway-sha256", required=True)
    parser.add_argument("--resources-sha256", required=True)
    parser.add_argument("--toolkit-python", type=Path, required=True)
    parser.add_argument("--meta-python", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument("--allow-public-hn-read", action="store_true")
    mode.add_argument("--both-selected-success", action="store_true",
                      help="two successful callers of one installed controlled-local arithmetic fixture, without HN")
    mode.add_argument("--ordinary-customizer-only", action="store_true",
                      help="only the existing ordinary class Add/respawn callback proof, with no workflow or HN read")
    mode.add_argument("--helperless-restore", action="store_true",
                      help="zero initial members, one actual class Add/use and ordinary process Restore/use")
    parser.add_argument("--respawn-shared-g", action="store_true",
                        help="with --allow-public-hn-read, respawn G once after A context completion and before B")
    parser.add_argument("--cold-shared-restore", action="store_true",
                        help="with --allow-public-hn-read, join the first gateway then restore G before fresh B")
    parser.add_argument("--withhold-p-completion", action="store_true",
                        help="with --cold-shared-restore, refuse P publication until the installed input finalizes")
    args = parser.parse_args(argv)
    if args.respawn_shared_g and not args.allow_public_hn_read:
        parser.error("--respawn-shared-g requires --allow-public-hn-read")
    if args.cold_shared_restore and (not args.allow_public_hn_read or args.respawn_shared_g):
        parser.error("--cold-shared-restore requires --allow-public-hn-read and excludes --respawn-shared-g")
    if args.withhold_p_completion and not args.cold_shared_restore:
        parser.error("--withhold-p-completion requires --cold-shared-restore")
    return args


def main():
    args = parse_args()
    for name in ("toolkit_gateway", "toolkit_python", "meta_python", "output"):
        path = getattr(args, name)
        require(path.is_absolute(), "all selected paths must be absolute")
        setattr(args, name, path.resolve())
    gateway = args.toolkit_gateway
    require(gateway.name == "toolkit_gateway" and gateway.parent.name == "bin" and os.access(gateway, os.X_OK)
            and v.digest(gateway) == args.toolkit_gateway_sha256, "exact supplied staged gateway required")
    prefix = gateway.parent.parent
    manifest = v.unique_json(v.exact_bytes(prefix / "libexec/meerkat-connect/resources.json", args.resources_sha256))
    require(not args.output.exists() and not args.output.is_relative_to(prefix), "fresh separate output required")
    args.output.mkdir(parents=True)
    sys.path[:0] = [str(args.toolkit_python), str(args.meta_python)]
    global COMMISSION
    for name, global_name, root in (("meerkat_toolkit.host_control", "SDK", args.toolkit_python),
        ("meerkat_toolkit.application_runtime", "APP_RUNTIME", args.toolkit_python),
        ("meerkat_meta.builder", "META_BUILDER", args.meta_python),
        ("meerkat_meta.identity_first_models", "META_IDENTITY", args.meta_python)):
        module = importlib.import_module(name)
        require(Path(module.__file__).resolve().is_relative_to(root), "SDK import escaped selected source")
        setattr(v, global_name, module)
    if not args.ordinary_customizer_only and not args.helperless_restore:
        COMMISSION = importlib.import_module("meerkat_toolkit.workflow_commission")
        require(Path(COMMISSION.__file__).resolve().is_relative_to(args.toolkit_python), "commission SDK escaped selected source")
    scenario_type = (BothSelectedScenario if args.both_selected_success else HelperlessScenario if args.helperless_restore
                     else ColdSharedScenario if args.cold_shared_restore else Scenario)
    scenario = scenario_type(v, c.loopback_origin())
    scenario.ordinary_customizer_only = args.ordinary_customizer_only or args.helperless_restore
    scenario.respawn_shared_g = args.respawn_shared_g
    if args.withhold_p_completion:
        scenario.withhold_p_completion = True
        scenario.name += "-publication-fault"
        scenario.scope = scenario.scope.replace("lost-reply completion publication, ", "") + (
            "; exact P completion write withheld until native installation finalizes, "
            "then repaired by the existing recovery owner; no lost installer reply claim")
    if args.respawn_shared_g or args.cold_shared_restore:
        scenario.resource_python = str((prefix / "libexec/meerkat-connect" / manifest["python"]).resolve(strict=True))
    if args.respawn_shared_g:
        scenario.name = "supplied-shared-g-respawn-success"
        scenario.scope += ("; opt-in real G same-session identity respawn after finalized A context, "
            "new SDK construction callbacks, native stale-scope refusal, retired transport refusal and fresh B setup/use; "
            "no direct retired callback invocation or selected-preparation replay claim")
    if args.ordinary_customizer_only:
        scenario.name = "supplied-ordinary-customizer-success"
        scenario.scope = ("supplied toolkit_gateway ordinary class Add and same-session respawn; actual SDK customize_build "
            "and after_create context, host-authored nonce tool before/after, exact native input audits and stale-scope refusal; "
            "no G commissioning, workflow, HN/provider integration, browser or cold recovery claim")
    candidate = {"kind": "supplied_toolkit_gateway", "binary": v.artifact_pin(gateway),
        "resources_sha256": args.resources_sha256, "claim": scenario.scope}
    result = asyncio.run(v._run_installed_product(args, candidate, prefix, manifest,
        corrupt=False, console=scenario))
    v.save_json(args.output / "RESULT.json", result)
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
