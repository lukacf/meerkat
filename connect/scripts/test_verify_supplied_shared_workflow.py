"""Offline acceptance-oracle controls; not native workflow execution."""
import asyncio
import copy
import contextlib
import dataclasses
import hashlib
import io
import json
import os
from pathlib import Path
import shlex
import socket
import sqlite3
import tempfile
import threading
import time
from types import SimpleNamespace
import unittest
from unittest.mock import AsyncMock, patch
import uuid
import verify_supplied_shared_workflow as w


@dataclasses.dataclass
class Reference:
    route: str
    original: str
    event_sha256: str


def request():
    return SimpleNamespace(admission_id=w.ADMISSION, identity=w.G_ID, instance={"class": w.CLASS_ID},
        parent=SimpleNamespace(original=Reference("planning-parent", "P", "a" * 64)))


def native_result():
    return {"state": "prepared", "admission_id": w.ADMISSION,
        "parent": dataclasses.asdict(request().parent.original),
        "native": {"state": "settled", "native": {"state": "result", "result": {
            "outcomes": {w.G_ID: {"restore": {"stage": "minted"}}}, "managed_edges": []}},
            "authorization": {"state": "succeeded"}, "cleanup": {"state": "succeeded"}}}


class CurrentCatalogPolicyControls(unittest.TestCase):
    """Real read-only SQLite oracle checks, not native admission or grant proof."""
    @staticmethod
    def policy(rows, revision=1, source=None):
        value = {"schema_version": 1, "provider_id": "fixture-provider", "policy_id": "fixture-policy",
                 "revision": revision, "policy_digest": "", "source": source or {
                     "source_id": "fixture", "source_digest": "sha256:" + "1" * 64},
                 "default_deny": True, "members": sorted(copy.deepcopy(rows), key=lambda row: row["member_identity"])}
        payload = {key: value[key] for key in value if key != "policy_digest"}
        value["policy_digest"] = w.v.installed_fixture_policy_digest(json.dumps(payload, separators=(",", ":")).encode())
        return json.dumps(value, separators=(",", ":")).encode() + b"\n"

    def fixture(self, directory):
        static = [{"member_identity": name, "grants": []}
                  for name in ("helper", "original-a", "original-b", "planning-parent")]
        classes, admissions, extra = [], [], []
        for name in ("ordinary", "planner"):
            declared = {"id": name + "-class", "profile": name + "-profile", "factory": name + "-factory",
                        "callbacks": [], "grants": [{"tool_name": name + "_read", "action": "invoke", "consequence": "r0"}]}
            selected = {"member": {"member": name, "mob_id": "fixture", "role": declared["profile"]},
                        "context_generation": 1, "logical_owner": w.v.installed_fixture_principal(name),
                        "audience": {"audience": "principal", "principal": w.c.human()}}
            destination = {"context": name + "-session", "context_generation": 1}
            classes.append(declared)
            admissions.append({"id": name + "-admission", "class": copy.deepcopy(declared),
                "instance": {"class": declared["id"], "member": selected},
                "destination": destination, "phase": {"kind": "active", **destination}})
            extra.append({"member_identity": name, "grants": copy.deepcopy(declared["grants"])})
        bootstrap, current = self.policy(static), self.policy(static + extra, 3,
            {"source_id": "toolkit-application-member-decision", "source_digest": "sha256:" + "4" * 64})
        slot = {"schema": "toolkit.application-service-tool-policy", "version": 1, "body": {
            "bootstrap": bootstrap.decode(), "current": current.decode(), "pending": None,
            "member_pending": None, "decision": None, "member_decision": None, "admissions": admissions}}
        native = {"realm": "mob.fixture", "member_classes": copy.deepcopy(classes),
                  "remote_operator": "operator", "grant_administrator": "administrator"}
        finalizer = SimpleNamespace(application_tool_policy=bootstrap, check_inputs=lambda: None,
            plan={"owner": {"deployment": "fixture-owner", "native_host": native},
                  "application": {"member_classes": classes}})
        key = "service-tool-policy-" + hashlib.sha256(w.v.canonical(
            [slot["schema"], "fixture-provider", "fixture-policy"])).hexdigest()
        path = directory / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
        path.parent.mkdir(parents=True)
        with sqlite3.connect(path) as db:
            db.execute("CREATE TABLE toolkit_records(realm TEXT,owner TEXT,id TEXT,revision INTEGER,digest TEXT,data TEXT)")
        fixture = SimpleNamespace(finalizer=finalizer, path=path, key=key, slot=slot, current=current)
        self.store(fixture, slot)
        return fixture

    @staticmethod
    def store(fixture, slot, *, digest=None, owner="fixture-owner"):
        raw = w.v.canonical(slot)
        with sqlite3.connect(fixture.path) as db:
            db.execute("DELETE FROM toolkit_records")
            db.execute("INSERT INTO toolkit_records VALUES(?,?,?,?,?,?)", ("mob.fixture", owner, fixture.key, 18,
                digest or hashlib.sha256(raw).hexdigest(), raw.decode()))

    def proposal(self, prior_raw, tag):
        prior = w.v.unique_json(prior_raw)
        rows = copy.deepcopy(prior["members"])
        member = next(row for row in rows if row["member_identity"] == "original-" + tag)
        member["grants"].extend({"tool_name": name, "action": "invoke", "consequence": "r2"}
                               for name in ("tool_catalog_load", "tool_catalog_search"))
        member["grants"].sort(key=lambda row: row["tool_name"])
        candidate = self.policy(rows, prior["revision"] + 1,
            {"source_id": "toolkit-application-service-decision", "source_digest": "sha256:" + "2" * 64})
        return {"provider_id": prior["provider_id"], "policy_id": prior["policy_id"], "member": "original-" + tag,
            "selection": {"tools": [], "catalog": "r2"}, "catalog_session": tag + "-session", "lifetime": "until_superseded",
            "prior": {"revision": prior["revision"], "digest": w.v.installed_fixture_policy_digest(prior_raw)},
            "candidate": {"revision": prior["revision"] + 1, "digest": w.v.installed_fixture_policy_digest(candidate)},
            "candidate_canonical_json": candidate.decode()}

    def test_dynamic_prior_and_joined_successor_keep_the_entire_policy_comparison(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root); fixture = self.fixture(directory)
            prior = w.s.current_catalog_policy(fixture.finalizer, directory, {})
            self.assertEqual(prior, fixture.current)
            # SharedDriver's default B selection and BOTH's first A use the same actual prior.
            for tag in ("a", "b"):
                proposal = self.proposal(prior, tag)
                w.s.reviewed_b_catalog_policy(proposal, "original_session", tag + "-session", prior, member="original-" + tag)
                with self.assertRaisesRegex(AssertionError, "original-" + tag):
                    w.s.reviewed_b_catalog_policy(proposal, "original_session", tag + "-session",
                        fixture.finalizer.application_tool_policy, member="original-" + tag)
            joined_a = self.proposal(prior, "a")["candidate_canonical_json"].encode()
            slot = copy.deepcopy(fixture.slot); slot["body"]["current"] = joined_a.decode()
            self.store(fixture, slot)
            captured = w.s.current_catalog_policy(fixture.finalizer, directory, {}, prior_policy=joined_a)
            self.assertEqual(captured, joined_a)
            # With no new class rows, a higher revision or altered provenance is not an independently reviewed prior.
            for changed in (self.policy(w.v.unique_json(joined_a)["members"], 5, w.v.unique_json(joined_a)["source"]),
                            self.policy(w.v.unique_json(joined_a)["members"], 4)):
                slot["body"]["current"] = changed.decode(); self.store(fixture, slot)
                with self.assertRaises(AssertionError):
                    w.s.current_catalog_policy(fixture.finalizer, directory, {}, prior_policy=joined_a)
            slot["body"]["current"] = joined_a.decode(); self.store(fixture, slot)
            proposal_b = self.proposal(captured, "b")
            w.s.reviewed_b_catalog_policy(proposal_b, "original_session", "b-session", captured)
            for label, mutate in (
                ("other-member", lambda value: value["members"][0]["grants"].append(
                    {"tool_name": "foreign", "action": "invoke", "consequence": "r2"})),
                ("dynamic-row", lambda value: value["members"].remove(
                    next(row for row in value["members"] if row["member_identity"] == "planner"))),
            ):
                bad = copy.deepcopy(proposal_b); value = w.v.unique_json(bad["candidate_canonical_json"].encode()); mutate(value)
                raw = self.policy(value["members"], value["revision"], value["source"])
                bad["candidate_canonical_json"] = raw.decode(); bad["candidate"]["digest"] = w.v.installed_fixture_policy_digest(raw)
                with self.subTest(label=label), self.assertRaisesRegex(AssertionError, "original-b catalog successor"):
                    w.s.reviewed_b_catalog_policy(bad, "original_session", "b-session", captured)

    def test_refused_snapshot_keeps_request_and_never_constructs_a_reservation(self):
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root); fixture = self.fixture(directory)
            proposal = {"tool_policy": self.proposal(fixture.current, "a")}
            request_ = SimpleNamespace(proposal=proposal, proposal_sha256=hashlib.sha256(w.v.canonical(proposal)).hexdigest())
            def change_current(slot, mutate):
                current = w.v.unique_json(slot["body"]["current"].encode()); mutate(current)
                slot["body"]["current"] = self.policy(current["members"], current["revision"], current["source"]).decode()
            changes = (
                ("bootstrap", lambda s: s["body"].update(bootstrap=self.policy([], 1).decode())),
                ("pending", lambda s: s["body"].update(pending={})),
                ("member-pending", lambda s: s["body"].update(member_pending={})),
                ("unknown-envelope", lambda s: s.update(unknown=True)),
                ("unknown-slot", lambda s: s["body"].update(unknown=True)),
                ("source-shape", lambda s: change_current(s, lambda p: p["source"].update(unknown=True))),
                ("source-digest", lambda s: change_current(s, lambda p: p["source"].update(source_digest="foreign"))),
                ("revision-drift", lambda s: s["body"].update(current=self.policy(
                    w.v.unique_json(s["body"]["current"].encode())["members"], 4,
                    w.v.unique_json(s["body"]["current"].encode())["source"]).decode())),
                ("source-drift", lambda s: s["body"].update(current=self.policy(
                    w.v.unique_json(s["body"]["current"].encode())["members"], 3).decode())),
                ("missing-admission", lambda s: s["body"]["admissions"].pop()),
                ("duplicate-admission", lambda s: s["body"]["admissions"].append(copy.deepcopy(s["body"]["admissions"][0]))),
                ("duplicate-id", lambda s: s["body"]["admissions"][1].update(id=s["body"]["admissions"][0]["id"])),
                ("class", lambda s: s["body"]["admissions"][0]["class"].update(factory="foreign")),
                ("unknown-class", lambda s: s["body"]["admissions"][0]["instance"].update({"class": "foreign"})),
                ("role", lambda s: s["body"]["admissions"][0]["instance"]["member"]["member"].update(role="foreign")),
                ("mob", lambda s: s["body"]["admissions"][0]["instance"]["member"]["member"].update(mob_id="foreign")),
                ("unqualified-owner", lambda s: s["body"]["admissions"][0]["instance"]["member"]["logical_owner"].update(
                    qualification={"kind": "unqualified"})),
                ("unqualified-audience", lambda s: s["body"]["admissions"][0]["instance"]["member"]["audience"]["principal"].update(
                    qualification={"kind": "unqualified"})),
                ("setup-capability", lambda s: s["body"]["admissions"][0]["instance"]["member"].update(setup_capability="foreign")),
                ("non-active", lambda s: s["body"]["admissions"][0]["phase"].update(kind="prepared")),
                ("destination", lambda s: s["body"]["admissions"][0]["destination"].update(context="foreign")),
                ("static-row", lambda s: change_current(s, lambda p: p["members"][0]["grants"].append(
                    {"tool_name": "foreign", "action": "invoke", "consequence": "r2"}))),
                ("extra-grant", lambda s: change_current(s, lambda p: next(r for r in p["members"] if r["member_identity"] == "planner")["grants"].clear())),
                ("missing-static", lambda s: change_current(s, lambda p: p["members"].pop(0))),
            )
            for label, mutate in changes:
                slot = copy.deepcopy(fixture.slot); mutate(slot); self.store(fixture, slot)
                evidence = {}
                with self.subTest(label=label), patch.object(w.s, "PublicReservation") as reservation:
                    with self.assertRaises(AssertionError):
                        prior = w.s.capture_public_review(fixture.finalizer, directory, request_, evidence)
                        w.s.PublicReservation(prior_policy=prior)
                    reservation.assert_not_called()
                    self.assertEqual(evidence["proposal"], proposal)
                    self.assertEqual(evidence["proposal_sha256"], request_.proposal_sha256)
                    self.assertEqual(w.v.unique_json(evidence["slot_json"].encode()), slot)
                    self.assertNotIn("validated", evidence)
            for label, options in (("store-digest", {"digest": "0" * 64}), ("wrong-owner", {"owner": "foreign"})):
                self.store(fixture, fixture.slot, **options)
                with self.subTest(label=label), self.assertRaises(AssertionError):
                    w.s.capture_public_review(fixture.finalizer, directory, request_, {})

    def test_repeated_reservation_check_uses_captured_prior_during_own_pending_transition(self):
        from meerkat_toolkit import host_control
        self.enterContext(patch.object(w.v, "SDK", host_control))
        with tempfile.TemporaryDirectory() as root:
            directory = Path(root).resolve(); fixture = self.fixture(directory); f = fixture.finalizer
            f.source_pin = {"sha256": "3" * 64}
            artifact = directory / "activation"; artifact.write_bytes(b"fixture")
            pin = {"path": str(artifact), "sha256": hashlib.sha256(b"fixture").hexdigest()}
            (directory / "current").symlink_to(artifact)
            f.selection = {"deployment_root": str(directory), **{key: pin for key in ("activation", "policy_manifest", "local_policy")}}
            child = {"original": {"route": "original-b", "original": "B"}, "admission": {"session_id": "b-session"}}
            proposal = {"original": child["original"], "operator": "operator", "application": "administrator",
                "represented_subject": w.c.human(), "source_policy_sha256": f.source_pin["sha256"],
                "instruction_sha256": hashlib.sha256(b"request-b").hexdigest(), "expires_in_ms": 180000,
                "tool_policy": self.proposal(fixture.current, "b"), "decision": {"route": "original-b", "choice": "approve",
                    "decision": "selected-decision", "catalog": "original_session", "permission": {"target": "original-b",
                        "connection": "installed-server", "resource": "installed-server", "operations": [w.s.OPERATION],
                        "lifetime": {"scope": {"kind": "task", "task": "selected-decision"}}},
                    "live_attachment": {"destination": {"context": "b-session", "logical_owner": w.v.installed_fixture_principal("original-b"),
                        "audience": {"audience": "principal", "principal": w.c.human()}}}}}
            request_ = SimpleNamespace(proposal=proposal, proposal_sha256=hashlib.sha256(w.v.canonical(proposal)).hexdigest())
            evidence = {}; prior = w.s.capture_public_review(f, directory, request_, evidence)
            scenario = SimpleNamespace(finalizer=f, request=lambda tag: "request-" + tag)
            provider = SimpleNamespace(results={"g-reconcile": {"observation": {"result": {
                "mcp_configuration": {"mcpServers": {"installed-server": {}}}}}}})
            reservation = w.s.PublicReservation(scenario, provider, request_, child,
                SimpleNamespace(poll=lambda: None), time.monotonic() + 30, prior_policy=prior)
            pending = copy.deepcopy(fixture.slot); pending["body"]["pending"] = {"own": "transition"}; self.store(fixture, pending)
            reservation.check_current()
            self.assertEqual(reservation.prior_policy, fixture.current)
            self.assertEqual(evidence["prior_canonical_json"].encode(), fixture.current)
            reservation.transferred = True; reservation.release()
            with self.assertRaises(w.v.SDK.HostReplyError):
                reservation.check_current()


class NativeObservationControls(unittest.TestCase):
    def test_prepared_label_or_marker_is_not_created_and_joined_proof(self):
        w.require_commissioned(native_result(), request())
        changes = (
            lambda v: v.update(state="created_without_planning"),
            lambda v: v.update(parent={**v["parent"], "original": "foreign"}),
            lambda v: v["native"].update(state="unsettled"),
            lambda v: v["native"]["native"]["result"]["outcomes"][w.G_ID]["restore"].update(stage="resumed"),
            lambda v: v["native"]["native"]["result"]["outcomes"].update(foreign={"restore": {"stage": "minted"}}),
            lambda v: v["native"].update(authorization={"state": "refused", "error": "denied"}),
            lambda v: v["native"].update(cleanup={"state": "refused", "error": "unavailable"}),
        )
        for change in changes:
            value = native_result(); change(value)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                w.require_commissioned(value, request())

    def test_canonical_source_locator_requires_exact_event_and_uniqueness(self):
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            path = directory / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
            path.parent.mkdir(parents=True)
            event = {"route": "original-a", "original": "A", "content_sha256": "c" * 64}
            original = Reference("original-a", "A", hashlib.sha256(w.v.canonical(event)).hexdigest())
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE toolkit_records(realm TEXT,owner TEXT,id TEXT,data TEXT)")
                db.execute("INSERT INTO toolkit_records VALUES(?,?,?,?)", ("mob.fixture", "native-owner", "native-key",
                    json.dumps({"schema": "toolkit.authenticated-source-event", "body": {"event": event}})))
            self.assertEqual(w.exact_source_key(directory, original),
                {"realm": "mob.fixture", "owner": "native-owner", "id": "native-key"})
            with self.assertRaises(AssertionError):
                w.exact_source_key(directory, dataclasses.replace(original, event_sha256="0" * 64))
            with sqlite3.connect(path) as db:
                db.execute("INSERT INTO toolkit_records SELECT realm,owner,'other-key',data FROM toolkit_records")
            with self.assertRaises(AssertionError):
                w.exact_source_key(directory, original)


class OriginalResponseGateControls(unittest.IsolatedAsyncioTestCase):
    def model(self):
        # Exercise the real response schedule without starting an HTTP server.
        with patch.object(w.s.Model, "__init__", return_value=None):
            model = w.AcceptanceModel(None, None)
        model.lock = threading.RLock()
        model.sequence, model.issued, model.results, model.call_inputs = 0, set(), {}, {}
        return model

    def selection(self, tag):
        original = Reference("original-" + tag, tag, "a" * 64)
        submission = SimpleNamespace(original=original, content=w.s.request(tag), transport_authentication="exact-auth")
        body = {"messages": [{"role": "user", "content": submission.content}],
                "tools": [{"function": {"name": "find_and_install"}}]}
        return submission, body

    async def test_first_actual_response_waits_for_single_exact_bound_replay_without_holding_model_lock(self):
        model = self.model()
        calls, responses, failures, workers, bodies = [], {}, [], [], {}
        admitted = {"state": "admitted", "deduplicated": False,
            "source_settlement": "fresh_acceptance_promoted", "turn_ticket": "native-ticket"}
        try:
            for tag in ("a", "b"):
                submission, body = self.selection(tag)
                bodies[tag] = body
                request = object()
                def respond(body=body, tag=tag):
                    try:
                        with model.lock:
                            responses[tag] = model.response(body)
                    except BaseException as error:
                        failures.append(error)
                def submit(*args, timeout):
                    self.assertEqual(args, (submission.original, submission.content, "exact-auth"))
                    self.assertEqual(timeout, 60)
                    calls.append("submit-" + tag)
                    worker = threading.Thread(target=respond)
                    workers.append(worker); worker.start()
                    return admitted
                def replay(actual, *, timeout):
                    self.assertIs(actual, request)
                    self.assertEqual(timeout, 30)
                    self.assertTrue(model.original_gates[tag]["entered"].is_set())
                    self.assertTrue(model.lock.acquire(timeout=1), "held response retained the provider lock")
                    try:
                        self.assertFalse(model.issued or responses)
                    finally:
                        model.lock.release()
                    calls.append("replay-" + tag)
                    return submission.original
                result = await w.submit_and_check_bound_child(SimpleNamespace(
                    submit_authenticated_original=submit, prepare_workflow_child=replay),
                    model, submission, request, time.monotonic() + 5)
                self.assertIs(result, admitted)
                self.assertTrue(model.original_gates[tag]["approved"])
                self.assertFalse(model.original_gates[tag]["released"].is_set() or model.issued or responses)
                if tag == "a":
                    with self.assertRaisesRegex(AssertionError, "selected original bound replays"):
                        model.release_original_responses()
                    self.assertFalse(any(gate["released"].is_set() for gate in model.original_gates.values()))
            model.release_original_responses()
        finally:
            model.release_original_responses(abort=True)
            for worker in workers:
                await asyncio.to_thread(worker.join, 1)
                self.assertFalse(worker.is_alive())
        self.assertEqual(calls, ["submit-a", "replay-a", "submit-b", "replay-b"])
        self.assertFalse(failures)
        self.assertEqual(model.issued, {"a-find", "b-find"})
        for tag, body in bodies.items():
            self.assertEqual(responses[tag]["delta"]["tool_calls"][0]["function"]["name"], "find_and_install")
            body["messages"].append({"role": "tool", "tool_call_id": tag + "-find", "content": "{}"})
            with model.lock:
                self.assertEqual(model.response(body)["finish_reason"], "stop")
        self.assertEqual(calls, ["submit-a", "replay-a", "submit-b", "replay-b"])

    async def test_changed_mismatch_or_missing_model_entry_never_retries_or_releases_a_tool_response(self):
        from meerkat_toolkit import host_control
        for failure in ("changed", "mismatch", "missing_entry", "changed_b_after_a_replay"):
            with self.subTest(failure=failure):
                model = self.model()
                target = "b" if failure == "changed_b_after_a_replay" else "a"
                submission, _ = self.selection(target)
                calls, workers, responses, failures = [], [], [], []
                changed = host_control.HostReplyError(host_control.HostReplyCode.OBSERVATION_CHANGED)
                def respond(body):
                    try:
                        with model.lock:
                            responses.append(model.response(body))
                    except BaseException as error:
                        failures.append(error)
                def submit(*args, **kwargs):
                    tag = args[0].route.removeprefix("original-")
                    calls.append("submit-" + tag)
                    if failure != "missing_entry":
                        worker = threading.Thread(target=respond, args=(self.selection(tag)[1],))
                        workers.append(worker); worker.start()
                    return {"state": "admitted", "deduplicated": False,
                        "source_settlement": "fresh_acceptance_promoted", "turn_ticket": "native-ticket"}
                def replay(original, **kwargs):
                    tag = original.route.removeprefix("original-")
                    self.assertTrue(model.original_gates[tag]["entered"].is_set())
                    self.assertFalse(model.issued or responses)
                    calls.append("replay-" + tag)
                    if failure == "changed_b_after_a_replay" and tag == "a":
                        return original
                    if failure in ("changed", "changed_b_after_a_replay"):
                        raise changed
                    return dataclasses.replace(submission.original, original="foreign")
                expected = host_control.HostReplyError if failure.startswith("changed") else AssertionError
                private = SimpleNamespace(submit_authenticated_original=submit, prepare_workflow_child=replay)
                try:
                    if target == "b":
                        first = self.selection("a")[0]
                        await w.submit_and_check_bound_child(private, model, first, first.original, time.monotonic() + 5)
                        self.assertTrue(model.original_gates["a"]["approved"])
                        self.assertFalse(model.original_gates["a"]["released"].is_set() or model.issued)
                    with self.assertRaises(expected) as refused:
                        await w.submit_and_check_bound_child(private, model,
                            submission, submission.original, time.monotonic() + (-1 if failure == "missing_entry" else 5))
                    if failure.startswith("changed"):
                        self.assertIs(refused.exception, changed)
                    self.assertTrue(all(gate["released"].is_set() and not gate["approved"]
                                        for gate in model.original_gates.values()))
                finally:
                    model.release_original_responses(abort=True)
                    for worker in workers:
                        await asyncio.to_thread(worker.join, 1)
                        self.assertFalse(worker.is_alive())
                expected_calls = ["submit-a", "replay-a"] if target == "b" else []
                expected_calls += ["submit-" + target]
                if failure != "missing_entry":
                    expected_calls.append("replay-" + target)
                self.assertEqual(calls, expected_calls)
                self.assertTrue(all(gate["released"].is_set() and not gate["approved"]
                                    for gate in model.original_gates.values()))
                self.assertFalse(responses or model.issued)
                self.assertEqual(len(failures), 0 if failure == "missing_entry" else 2 if target == "b" else 1)
                self.assertTrue(all(isinstance(error, AssertionError) for error in failures))
                # A late request after failed admission/replay cannot turn a
                # cleanup wake into either original's first find operation.
                with model.lock:
                    for tag in ("a", "b"):
                        with self.assertRaisesRegex(AssertionError, "without a successful bound replay"):
                            model.response(self.selection(tag)[1])
                self.assertFalse(model.issued)

    def test_parent_and_shared_context_requests_do_not_enter_an_original_gate(self):
        model = self.model()
        with model.lock:
            for marker in (w.s.P_TEXT, w.s.G_TEXT):
                response = model.response({"messages": [{"role": "user", "content": marker},
                    {"role": "user", "content": w.s.request("a")}]})
                self.assertEqual(response["finish_reason"], "stop")
        self.assertFalse(any(gate["entered"].is_set() or gate["released"].is_set()
                             for gate in model.original_gates.values()))

    def test_expired_approved_release_cannot_emit_an_original_tool_response(self):
        model = self.model()
        gate = model.original_gates["a"]
        gate.update(approved=True, deadline=time.monotonic() - 1)
        gate["entered"].set()
        gate["released"].set()
        with model.lock, self.assertRaisesRegex(AssertionError, "response deadline elapsed"):
            model.response(self.selection("a")[1])
        self.assertFalse(model.issued)


class ConsoleChildResponseGateControls(unittest.IsolatedAsyncioTestCase):
    async def test_supplied_console_permission_selects_only_both_children(self):
        from meerkat_meta import identity_first_models
        scenario = w.Scenario(w.v, "http://127.0.0.1:1234")
        scenario.plan = {"application": {
            "definition": {"text": '[profiles.' + w.C3_CLASS + ']\nmodel = "fixture-original-model"\n'
                '[profiles.' + w.CLASS_ID + ']\nmodel = "fixture-shared-model"\n'},
            "roster": [{"identity": name, "profile": "fixture"}
                for name in ("coordinator", "planning-parent", "original-a", "original-b")]}}
        with tempfile.TemporaryDirectory() as directory, patch.object(w.v, "META_IDENTITY", identity_first_models):
            config = SimpleNamespace()
            scenario.configure_runtime(config, Path(directory))
            access = w.tomllib.loads(Path(config.access_config_path).read_text())
        # Inspect the actual emitted policy; this is no native permission proof.
        self.assertEqual(access, {"enabled": True, "admins": ["unrelated-fixture-admin"], "rules": [
            {"id": "driver-caller-observation", "subjects": [w.c.EMAIL], "actions": ["agent.view"],
             "agents": ["planning-parent", "original-a", "original-b"]},
            {"id": "supplied-console-children", "subjects": [w.c.EMAIL], "actions": ["agent.send"],
             "agents": ["original-a", "original-b"]}]})

    async def exercise(self, failure=None):
        # Inert HTTP/native rows exercise the acceptance oracle and failure
        # cleanup, not the runtime's authentication or child admission owner.
        model = OriginalResponseGateControls.model(self)
        text = w.s.request("a")
        request = SimpleNamespace(route="original-a", send_key="selected-console-a")
        scenario = SimpleNamespace(port=1234, origin="http://127.0.0.1:1234",
            console_workflow_token="inert-controlled-token", console_workflow_login=object())
        accepted = {"status": "accepted", "identity": request.route, "session_id": "native-session",
            "interaction_id": "native-interaction", "input_frame_id": "native-frame"}
        event = {"route": request.route, "original": "actual-input", "content_sha256": "c" * 64}
        original = Reference(request.route, event["original"], hashlib.sha256(w.v.canonical(event)).hexdigest())
        association = {"exact": "native-association"}
        record = {"event": event, "console_child_input": "native_console", "workflow_child": {"selected": True},
            "final_association": association, "bound_service_input": {"header": {
                "id": original.original, "idempotency_key": "console:native-interaction:native-frame",
                "authority_association": association}}}
        rows = [{"input_id": original.original}]
        if failure == "stock_key": record["bound_service_input"]["header"]["idempotency_key"] = "console:foreign:frame"
        if failure == "association": record["bound_service_input"]["header"]["authority_association"] = {"foreign": True}
        if failure == "native_input": rows[0]["input_id"] = "other-input"
        if failure == "acceptance": accepted["identity"] = "foreign"
        calls, workers, responses, errors = [], [], [], []
        body = {"messages": [{"role": "user", "content": text}],
                "tools": [{"function": {"name": "find_and_install"}}]}
        def model_request():
            try:
                with model.lock: responses.append(model.response(body))
            except BaseException as error: errors.append(error)
        class Connection:
            def __init__(inner, host, port, timeout):
                self.assertEqual((host, port, timeout), ("127.0.0.1", scenario.port, 30))
            def request(inner, method, path, *, body, headers):
                self.assertEqual((method, path), ("POST", "/console/send"))
                self.assertEqual(json.loads(body), {"identity": request.route, "content": text,
                    "origin": "console", "origin_kind": "operator", "idempotency_key": request.send_key})
                self.assertEqual(headers["Authorization"], "Bearer " + scenario.console_workflow_token)
                calls.append("send")
                if failure != "missing_entry":
                    worker = threading.Thread(target=model_request)
                    workers.append(worker); worker.start()
            def getresponse(inner):
                if failure == "http": raise OSError("controlled HTTP read failure")
                if failure in ("forbidden", "foreign_error", "malformed_error", "duplicate_error", "oversized_error"):
                    raw = (b'{"error":"access_denied","error":"sensitive"}' if failure == "duplicate_error" else
                        b"not-json-sensitive-body" if failure == "malformed_error" else
                        b"x" * (1024 * 1024 + 1) if failure == "oversized_error" else
                        w.v.canonical({"error": "access_denied" if failure == "forbidden" else "sensitive-error",
                            "message": "private arbitrary detail", "token": scenario.console_workflow_token}))
                    return SimpleNamespace(status=403, read=lambda limit: raw)
                return SimpleNamespace(status=202, read=lambda limit: w.v.canonical(accepted))
            def close(inner): calls.append("http-closed")
        def observe(actual, login, *, timeout):
            self.assertIs(actual, request); self.assertIs(login, scenario.console_workflow_login)
            self.assertEqual(timeout, 30)
            self.assertTrue(model.original_gates["a"]["entered"].is_set())
            self.assertFalse(model.original_gates["a"]["approved"] or responses or model.issued)
            calls.append("observe")
            if failure == "locator_missing": return None
            if failure == "locator_changed" and calls.count("observe") == 2:
                return dataclasses.replace(original, original="changed-input")
            return original
        try:
            with tempfile.TemporaryDirectory() as directory:
                directory = Path(directory)
                path = directory / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
                path.parent.mkdir(parents=True)
                with sqlite3.connect(path) as db:
                    db.execute("CREATE TABLE toolkit_records(realm TEXT,owner TEXT,id TEXT,data TEXT)")
                    db.execute("INSERT INTO toolkit_records VALUES(?,?,?,?)", ("mob.fixture", "owner", "source",
                        json.dumps({"schema": "toolkit.authenticated-source-event", "body": record})))
                with patch.object(w.s, "HTTPConnection", Connection), patch.object(w.s, "input_rows", return_value=rows):
                    call = w.submit_console_child(scenario, SimpleNamespace(prepare_console_workflow_child=observe),
                        model, directory, request, text, time.monotonic() + (-1 if failure == "missing_entry" else 5))
                    if failure is None:
                        result = await call
                        self.assertEqual(result["admission"], {"session_id": "native-session", "input_id": original.original,
                            "console_acceptance": accepted, "native_inputs": rows})
                        self.assertEqual(result["original"], dataclasses.asdict(original))
                        self.assertTrue(model.original_gates["a"]["approved"])
                        self.assertFalse(model.original_gates["a"]["released"].is_set() or responses or model.issued)
                        model.release_original_responses(tags=("a",))
                    else:
                        with self.assertRaises(OSError if failure == "http" else AssertionError): await call
                        self.assertTrue(all(gate["released"].is_set() and not gate["approved"]
                                            for gate in model.original_gates.values()))
                    evidence = directory / "console-child-a-http.json"
                    if failure == "http":
                        self.assertFalse(evidence.exists())
                    else:
                        observation = json.loads(evidence.read_text())
                        self.assertEqual(set(observation), {"status", "response_bytes", "error_code"})
                        self.assertEqual(observation["status"], 403 if failure in (
                            "forbidden", "foreign_error", "malformed_error", "duplicate_error", "oversized_error") else 202)
                        self.assertEqual(observation["error_code"], "access_denied" if failure == "forbidden" else None)
                        self.assertGreater(observation["response_bytes"], 0)
                        self.assertNotIn(scenario.console_workflow_token, evidence.read_text())
                        self.assertNotIn("private arbitrary detail", evidence.read_text())
                        self.assertNotIn("sensitive", evidence.read_text())
                        if failure in ("forbidden", "foreign_error", "malformed_error", "duplicate_error", "oversized_error"):
                            self.assertNotIn("observe", calls)
        finally:
            model.release_original_responses(abort=True)
            for worker in workers:
                await asyncio.to_thread(worker.join, 1)
                self.assertFalse(worker.is_alive())
        self.assertEqual(calls.count("send"), 1)
        self.assertEqual(calls.count("http-closed"), 1)
        if failure is None:
            self.assertEqual(calls.count("observe"), 2)
            self.assertEqual(model.issued, {"a-find"}); self.assertFalse(errors)
        else:
            self.assertFalse(model.issued or responses)
            self.assertEqual(len(errors), int(failure != "missing_entry"))

    async def test_console_acceptance_requires_exact_source_native_input_and_stock_key_before_release(self):
        await self.exercise()

    async def test_console_http_observation_and_binding_failures_release_gates_without_retry(self):
        for failure in ("http", "forbidden", "foreign_error", "malformed_error", "duplicate_error", "oversized_error",
                        "acceptance", "missing_entry", "locator_missing", "locator_changed",
                        "stock_key", "association", "native_input"):
            with self.subTest(failure=failure): await self.exercise(failure)


class DurableApplicationPublicationControls(unittest.IsolatedAsyncioTestCase):
    async def test_child_refusal_requires_exact_control_code_and_never_accepts_success(self):
        from meerkat_toolkit import host_control
        self.enterContext(patch.object(w.v, "SDK", host_control))
        selected = object()
        calls = []
        for expected in (w.v.SDK.HostReplyCode.UNAVAILABLE, w.v.SDK.HostReplyCode.DENIED):
            for actual in w.v.SDK.HostReplyCode:
                def refuse(request, *, timeout):
                    calls.append((request, timeout))
                    raise w.v.SDK.HostReplyError(actual)
                with self.subTest(expected=expected, actual=actual):
                    private = SimpleNamespace(prepare_workflow_child=refuse)
                    if actual is expected:
                        self.assertEqual(await w.require_child_refusal(private, selected, expected), actual.value)
                    else:
                        with self.assertRaisesRegex(AssertionError, "child refusal expected"):
                            await w.require_child_refusal(private, selected, expected)
        self.assertTrue(all(request is selected and timeout == 30 for request, timeout in calls))
        with self.assertRaisesRegex(AssertionError, "was accepted"):
            await w.require_child_refusal(SimpleNamespace(prepare_workflow_child=lambda *a, **k: selected),
                                          selected, w.v.SDK.HostReplyCode.UNAVAILABLE)
        def disconnected(*args, **kwargs):
            raise EOFError("controlled transport loss")
        with self.assertRaises(EOFError):
            await w.require_child_refusal(SimpleNamespace(prepare_workflow_child=disconnected),
                                          selected, w.v.SDK.HostReplyCode.UNAVAILABLE)

    async def test_commission_refusal_is_retained_before_listing_join_and_acceptance(self):
        value = native_result()
        value.update(state="created_without_planning", error="denied")
        with tempfile.TemporaryDirectory() as directory:
            directory = Path(directory)
            path = directory / "supplied-commission-result.json"
            joined = []

            async def join_listing():
                self.assertEqual(json.loads(path.read_bytes()), value)
                joined.append(True)

            attempt = SimpleNamespace(result=lambda: value, join_listing=join_listing)
            result = await w.observe_commission(attempt, directory)
            self.assertIs(result, value)
            self.assertEqual(joined, [True])
            with self.assertRaises(AssertionError):
                w.require_commissioned(result, request())
            self.assertEqual(json.loads(path.read_bytes()), value)

            async def refused_join():
                self.assertEqual(json.loads(path.read_bytes()), value)
                raise RuntimeError("controlled publication join refusal")

            attempt.join_listing = refused_join
            with self.assertRaisesRegex(RuntimeError, "controlled publication join refusal"):
                await w.observe_commission(attempt, directory)
            self.assertEqual(json.loads(path.read_bytes()), value)

    async def test_real_customizer_observation_requires_delegated_after_create_and_preserves_draft(self):
        observer = w.s.ObservedCustomizer()
        context = SimpleNamespace(identity=w.G_ID)
        spec = SimpleNamespace(identity=w.G_ID, profile=w.CLASS_ID)
        draft = SimpleNamespace(external_tools=[], model="selected-model", labels={"kept": "exact"})
        before = copy.deepcopy(vars(draft))
        await observer.customize_build(context, spec, draft)
        self.assertEqual(vars(draft), before)
        with self.assertRaises(AssertionError):
            observer.require_created(w.G_ID)
        await observer.after_create(w.G_ID, "native-session", {})
        self.assertEqual(observer.require_created(w.G_ID)["created"],
                         [{"identity": w.G_ID, "session_id": "native-session"}])
        with self.assertRaises(AssertionError):
            await observer.customize_build(context, SimpleNamespace(identity="foreign"), draft)
        with self.assertRaises(AssertionError):
            await observer.customize_build(context, spec, SimpleNamespace(external_tools=["invented"]))

    async def test_admitting_is_not_listing_and_callback_is_exactly_once(self):
        with tempfile.TemporaryDirectory() as directory:
            initial = [{"identity": name, "profile": "initial"}
                       for name in ("helper", "planning-parent", "original-a", "original-b")]
            roster = w.DesiredRoster(Path(directory), initial)
            roster.prepare(request())
            self.assertEqual(roster.selected(), initial)
            self.assertEqual(roster.publications, 0)
            await roster.publish()
            self.assertEqual(roster.selected(), initial + [roster.spec()])
            self.assertEqual(roster.publications, 1)
            with self.assertRaises(AssertionError):
                await roster.publish()
            # Unknown later native settlement cannot erase this durable listing.
            self.assertEqual(json.loads(roster.path.read_bytes()), initial + [roster.spec()])

    async def test_changed_admitting_row_refuses_without_publishing(self):
        with tempfile.TemporaryDirectory() as directory:
            initial = [{"identity": str(index)} for index in range(4)]
            roster = w.DesiredRoster(Path(directory), initial)
            roster.prepare(request())
            changed = copy.deepcopy(roster.expected); changed["admission_id"] = "another-admission"
            w.atomic_json(roster.admitting, changed)
            with self.assertRaises(AssertionError):
                await roster.publish()
            self.assertEqual(roster.selected(), initial)
            self.assertEqual(roster.publications, 0)


class OrdinaryRebuildOracleControls(unittest.TestCase):
    def test_actual_fixture_plan_preserves_sorted_classes_and_zero_initial_instances(self):
        scenario = w.Scenario(w.v, "http://127.0.0.1:12345")
        with tempfile.TemporaryDirectory() as directory:
            plan, _, _ = w.v.installed_plan(Path(directory), [{"identity": "helper", "profile": "research"}],
                                           acquisition=True, console=scenario)
        app = plan["application"]
        self.assertEqual(app["member_classes"], plan["owner"]["native_host"]["member_classes"])
        self.assertEqual([row["id"] for row in app["member_classes"]], [w.C3_CLASS, w.CLASS_ID])
        self.assertEqual([row["identity"] for row in app["roster"]],
                         ["helper", "planning-parent", "original-a", "original-b"])
        self.assertEqual(app["member_classes"][0]["callbacks"],
            [{"name": w.C3_TOOL, "description": w.C3_DESCRIPTION, "input_schema": w.C3_SCHEMA}])
        self.assertEqual(app["member_classes"][1]["callbacks"], [])

    def observations(self):
        record = {"id": w.C3_ADMISSION, "class": {"id": w.C3_CLASS}, "instance": {"class": w.C3_CLASS},
                  "phase": {"kind": "active"}, "native_surface": [{"name": w.C3_TOOL}], "installed_surface": [{"name": w.C3_TOOL}],
                  "route": {"id": w.C3_ID, "controller_parent": [{"grant_id": "native-issued-parent"}]}}
        before = {"status": {"identity": w.C3_ID, "state": "active", "session_id": "session", "generation": 4,
                             "agent_runtime_id": "runtime-before"},
                  "tools": {"identity": w.C3_ID, "session_id": "session", "tools": [w.C3_TOOL]},
                  "canonical": {"record": record, "authorization": {"state": "succeeded"}}}
        after = copy.deepcopy(before)
        after["status"].update(generation=5, agent_runtime_id="runtime-after")
        result = {"identity": w.C3_ID, "session_id": "session", "generation": 5, "agent_runtime_id": "runtime-after",
                  "cleanup_warning": None, "live_respawn_warning": None}
        return before, after, result

    def test_callback_replay_rotation_missing_surface_and_changed_owner_are_not_rebuild_proof(self):
        w.require_c3_transition(*self.observations())
        changes = (
            lambda a, r: (a["status"].update(generation=4), r.update(generation=4)),
            lambda a, r: (a["status"].update(session_id="replaced"), r.update(session_id="replaced")),
            lambda a, r: a["tools"].update(tools=[]),
            lambda a, r: a["canonical"]["record"].update(id="second-add"),
            lambda a, r: a["canonical"]["record"].pop("route"),
            lambda a, r: a["canonical"]["record"]["route"].update(controller_parent=[]),
            lambda a, r: a["canonical"]["record"]["route"].update(controller_parent=[{"grant_id": "foreign-parent"}]),
            lambda a, r: a["canonical"]["record"].update(phase={"kind": "retired"}),
            lambda a, r: a["canonical"].update(authorization={"state": "refused", "error": "denied"}),
            lambda a, r: r.update(cleanup_warning="unjoined"),
        )
        for change in changes:
            before, after, result = self.observations()
            change(after, result)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                w.require_c3_transition(before, after, result)

    def test_preserved_history_requires_both_actual_tool_results_and_prior_completion(self):
        before = [{"role": "user", "content": w.c3_text("before")},
                  {"role": "tool", "tool_call_id": "c3-before-call", "content": "actual-before-result"}]
        after = before + [{"role": "assistant", "content": "Ordinary callback returned before"},
                          {"role": "tool", "tool_call_id": "c3-after-call", "content": "actual-after-result"}]
        w.require_c3_history(before, after)
        for changed in (after[1:], after[:-1], before + after[-1:], [*before[:-1], {**before[-1], "content": "changed"}, *after[2:]]):
            with self.subTest(changed=changed), self.assertRaises(AssertionError):
                w.require_c3_history(before, changed)

    def test_missing_after_create_can_never_mark_supplied_acceptance_passed(self):
        scenario = object.__new__(w.Scenario)
        scenario.c3 = {"status": "native_rebuild_observed_callback_pending", "after_create": {"state": "pending_meta_callback_fix"}}
        with self.assertRaisesRegex(AssertionError, "pending Meta callback fix"):
            scenario.verify_customizer()


def respawn_observations(identity=None, new_builds=1, *, late=False, session_id="same-session"):
    """Synthetic callback records for the real pure validator, not native evidence."""
    identity = identity or w.C3_ID
    context = {"model": "selected-model", "labels": {"fixture": "unchanged"}, "system_prompt": "selected prompt"}
    profile = w.C3_CLASS if identity == w.C3_ID else w.CLASS_ID
    def build(index):
        return {"kind": "build", "identity": identity, "profile": profile, "build": index,
                "context": copy.deepcopy(context)}
    created = {"kind": "created", "identity": identity, "session_id": session_id,
               "context": copy.deepcopy(context)}
    before = [build(1), copy.deepcopy(created)]
    returned = copy.deepcopy(before) + [build(index) for index in range(2, 2 + new_builds)]
    if not late:
        returned.append(copy.deepcopy(created))
    current = copy.deepcopy(returned) + ([copy.deepcopy(created)] if late else [])
    return before, returned, current


class RespawnCallbackWindowControls(unittest.IsolatedAsyncioTestCase):
    def test_one_or_two_in_window_builds_keep_one_same_session_notification(self):
        for identity in (w.C3_ID, w.G_ID):
            for count in (1, 2):
                for late in (False, True):
                    before, returned, current = respawn_observations(identity, count, late=late)
                    with self.subTest(identity=identity, count=count, late=late):
                        proof = w.require_respawn_callbacks(before, returned, current, identity, "same-session")
                        self.assertEqual(proof["new_builds"], count)
                        self.assertEqual(proof["latest_build"], count + 1)
                        self.assertEqual(proof["created"], [proof["created"][0]] * 2)
        # Meta may notify after its seat build but before the final wake build.
        before, returned, _ = respawn_observations(new_builds=2)
        returned[-2:] = [returned[-1], returned[-2]]
        self.assertEqual(w.require_respawn_callbacks(before, returned, returned, w.C3_ID,
                                                   "same-session")["latest_build"], 3)

    def test_initial_add_wrong_counts_extra_notifications_and_changed_context_are_refused(self):
        mutations = {
            "zero new builds": lambda b, r, c: (b, b + c[-1:], b + c[-1:]),
            "three new builds": lambda b, r, c: respawn_observations(new_builds=3),
            "extra notification": lambda b, r, c: (b, r, c + [copy.deepcopy(c[-1])]),
            "changed prefix": lambda b, r, c: (b, r[1:], c[1:]),
            "missing initial notification": lambda b, r, c: (b[:1], r, c),
            "foreign session": lambda b, r, c: (b, r, c[:-1] + [{**c[-1], "session_id": "foreign"}]),
            "changed context": lambda b, r, c: (b, r, c[:-1] + [{**c[-1], "context": {}}]),
            "foreign identity": lambda b, r, c: (b, r, c[:-1] + [{**c[-1], "identity": w.G_ID}]),
            "extra field": lambda b, r, c: (b, r, c[:-1] + [{**c[-1], "unrecorded": True}]),
        }
        for label, mutate in mutations.items():
            with self.subTest(label=label), self.assertRaises(AssertionError):
                w.require_respawn_callbacks(*mutate(*respawn_observations(late=True)), w.C3_ID, "same-session")

    async def test_real_wait_observer_accepts_late_notification_and_freezes_the_rpc_build_window(self):
        before, returned, current = respawn_observations(new_builds=2, late=True)
        customizer = w.AcceptanceCustomizer()
        for row in returned:
            customizer.record_respawn_event(row)
        waiting = threading.Event()
        class ObservedCondition(threading.Condition):
            def wait(self, timeout=None):
                waiting.set()
                return super().wait(timeout)
        customizer.created_changed = ObservedCondition()
        observed = asyncio.create_task(customizer.wait_respawn(before, returned, w.C3_ID,
                                                             "same-session", time.monotonic() + 2))
        self.assertTrue(await asyncio.to_thread(waiting.wait, 1))
        self.assertFalse(observed.done())
        customizer.record_respawn_event(current[-1])
        proof = await observed
        self.assertEqual(proof["latest_build"], 3)
        self.assertEqual(customizer.verify_respawn(proof), proof)
        customizer.record_respawn_event({**returned[-1], "build": 4})
        with self.assertRaisesRegex(AssertionError, "outside the selected respawn RPC window"):
            customizer.verify_respawn(proof)

    async def test_late_build_is_not_mistaken_for_notification_and_pending_keeps_original_deadline(self):
        before, returned, _ = respawn_observations(late=True)
        customizer = w.AcceptanceCustomizer()
        for row in returned:
            customizer.record_respawn_event(row)
        with self.assertRaisesRegex(AssertionError, "deadline elapsed"):
            await customizer.wait_respawn(before, returned, w.C3_ID, "same-session", time.monotonic())
        customizer.record_respawn_event({**returned[-1], "build": 3})
        with self.assertRaisesRegex(AssertionError, "outside the selected respawn RPC window"):
            await customizer.wait_respawn(before, returned, w.C3_ID, "same-session", time.monotonic() + 1)

    async def test_latest_actual_draft_handler_wins_and_prior_handler_is_refused(self):
        from meerkat_meta.identity_first_models import AgentBuildDraft, DurableAgentSpec
        from meerkat_meta.models import SessionCreatedContext
        for count in (1, 2):
            customizer = w.AcceptanceCustomizer("selected-model")
            drafts = []
            async def build():
                draft = AgentBuildDraft()
                await customizer.customize_build(SimpleNamespace(identity=w.C3_ID),
                    DurableAgentSpec(identity=w.C3_ID, profile=w.C3_CLASS), draft)
                drafts.append(draft)
            context = SessionCreatedContext.from_dict({"model": "selected-model", "labels": {}, "system_prompt": None})
            await build()
            await customizer.after_create(w.C3_ID, "same-session", context)
            before = customizer.respawn_snapshot(w.C3_ID)
            for _ in range(count):
                await build()
            returned = customizer.respawn_snapshot(w.C3_ID)
            await customizer.after_create(w.C3_ID, "same-session", context)
            proof = await customizer.wait_respawn(before, returned, w.C3_ID, "same-session", time.monotonic())
            latest = await drafts[-1].tool_handlers[w.C3_TOOL]({"nonce": "after"})
            w.require_c3_handler(latest, "after", proof["latest_build"])
            stale = await drafts[-2].tool_handlers[w.C3_TOOL]({"nonce": "after"})
            with self.assertRaisesRegex(AssertionError, "actual construction's handler"):
                w.require_c3_handler(stale, "after", proof["latest_build"])
            # The cold/helperless default remains the exact second handler.
            w.require_c3_handler({"nonce": "after", "handler_build": 2}, "after")
            with self.assertRaises(AssertionError):
                w.require_c3_handler({"nonce": "after", "handler_build": 3}, "after")


class OrdinaryCallbackAndPublicationControls(unittest.IsolatedAsyncioTestCase):
    async def test_actual_callback_notification_unblocks_bounded_observer_with_exact_context(self):
        from meerkat_meta.identity_first_models import AgentBuildDraft, DurableAgentSpec
        from meerkat_meta.models import SessionCreatedContext
        customizer = w.AcceptanceCustomizer("selected-profile-model")
        entered = threading.Event()
        class ObservedCondition(threading.Condition):
            def wait(self, timeout=None):
                entered.set()
                return super().wait(timeout)
        customizer.created_changed = ObservedCondition()
        for index in (1, 2):
            draft = AgentBuildDraft(labels={"fixture": "unchanged"}, system_prompt="actual selected prompt")
            await customizer.customize_build(SimpleNamespace(identity=w.C3_ID),
                DurableAgentSpec(identity=w.C3_ID, profile=w.C3_CLASS), draft)
            observed = asyncio.create_task(customizer.wait_created(index, "same-session", time.monotonic() + 2))
            self.assertTrue(await asyncio.to_thread(entered.wait, 1))
            self.assertFalse(observed.done(), "a completed build is not the callback observation")
            context = SessionCreatedContext.from_dict({"model": "selected-profile-model",
                "labels": {"fixture": "unchanged"}, "system_prompt": "actual selected prompt"})
            await customizer.after_create(w.C3_ID, "same-session", context)
            rows = await observed
            self.assertEqual(len(rows), index)
            self.assertEqual(rows[-1]["context"], dataclasses.asdict(context))
            self.assertEqual(draft.labels, {"fixture": "unchanged"})
            self.assertEqual(draft.system_prompt, "actual selected prompt")
            entered.clear()

    async def test_missing_flat_context_foreign_session_and_duplicate_callback_cannot_pass(self):
        from meerkat_meta.identity_first_models import AgentBuildDraft, DurableAgentSpec
        from meerkat_meta.models import SessionCreatedContext
        expected = {"model": "selected-model", "labels": {"fixture": "exact"}, "system_prompt": "selected prompt"}
        for change in ({}, {"model": ""}, {"labels": {}}, {"system_prompt": None},
                       {"session": "foreign"}, {"duplicate": True}, {"missing": True}):
            customizer = w.AcceptanceCustomizer("profile-fallback")
            await customizer.customize_build(SimpleNamespace(identity=w.C3_ID),
                DurableAgentSpec(identity=w.C3_ID, profile=w.C3_CLASS), AgentBuildDraft(**expected))
            if not change.get("missing"):
                value = {} if not change else {**expected, **{key: value for key, value in change.items() if key in expected}}
                context = SessionCreatedContext.from_dict(value)
                await customizer.after_create(w.C3_ID, change.get("session", "same-session"), context)
                if change.get("duplicate"):
                    await customizer.after_create(w.C3_ID, "same-session", context)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                await customizer.wait_created(1, "same-session", time.monotonic())

    async def test_stale_scope_uses_valid_sdk_delivery_identity_and_only_accepts_typed_refusal(self):
        from meerkat_meta.errors import RpcError, StaleScopeError
        from meerkat_meta.identity_first_models import DeliveryScope
        from meerkat_meta.runtime import MetaRuntime
        # Run the real SDK serializer with an offline RPC boundary. This is a
        # wire/oracle control; the native acceptance supplies the actual scope.
        runtime = object.__new__(MetaRuntime)
        scope = DeliveryScope(w.C3_ID, "old-runtime", 4, 7, {"session_id": "same-session"})
        correlations = []
        response = StaleScopeError("stale scope", mismatch="generation")
        async def rpc(method, params):
            self.assertEqual(method, "meta/dispatch")
            self.assertEqual(params["identity"], w.C3_ID)
            self.assertEqual(params["expected_scope"], scope.to_dict())
            delivery = params["dispatch_input"]
            correlation = delivery["correlation_id"]
            parsed = uuid.UUID(correlation)
            self.assertNotEqual(parsed.int, 0)
            self.assertEqual(str(parsed), correlation)
            self.assertNotIn(correlation, correlations)
            correlations.append(correlation)
            self.assertEqual(delivery["idempotency_key"], "acceptance-stale-delivery")
            if isinstance(response, Exception):
                raise response
            return response
        runtime._rpc = rpc
        self.assertEqual(await w.require_c3_stale_scope(runtime, scope),
                         {"state": "refused", "code": response.code, "mismatch": "generation"})
        response = RpcError(-32602, "malformed delivery identity")
        with self.assertRaises(RpcError) as caught:
            await w.require_c3_stale_scope(runtime, scope)
        self.assertIs(caught.exception, response)
        response = "unexpected accepted delivery"
        with self.assertRaisesRegex(AssertionError, "old ordinary delivery scope was not refused"):
            await w.require_c3_stale_scope(runtime, scope)

    async def test_real_sdk_draft_registers_only_declared_ordinary_tool_and_retains_new_handler(self):
        from meerkat_meta.identity_first_models import AgentBuildDraft, DurableAgentSpec
        customizer = w.AcceptanceCustomizer()
        g = AgentBuildDraft()
        before = copy.deepcopy(g.to_dict())
        await customizer.customize_build(SimpleNamespace(identity=w.G_ID), DurableAgentSpec(identity=w.G_ID, profile=w.CLASS_ID), g)
        self.assertEqual(g.to_dict(), before)
        for generation, phase in enumerate(("before", "after"), 1):
            draft = AgentBuildDraft()
            await customizer.customize_build(SimpleNamespace(identity=w.C3_ID),
                DurableAgentSpec(identity=w.C3_ID, profile=w.C3_CLASS), draft)
            self.assertEqual([tool.name for tool in draft.external_tools], [w.C3_TOOL])
            self.assertEqual(draft.external_tools[0].input_schema, w.C3_SCHEMA)
            self.assertEqual(await draft.tool_handlers[w.C3_TOOL]({"nonce": phase}),
                             {"nonce": phase, "handler_build": generation})
        with self.assertRaises(AssertionError):
            await draft.tool_handlers[w.C3_TOOL]({"nonce": "foreign"})
        self.assertEqual(customizer.calls, [{"nonce": "before", "handler_build": 1}, {"nonce": "after", "handler_build": 2}])

    async def test_same_roster_owner_keeps_ordinary_empty_mcp_row_separate_from_g_publication(self):
        from meerkat_toolkit.member_classes import MemberMcpAdmissionSelection, RetainedMemberMcpBindings
        with tempfile.TemporaryDirectory() as directory:
            initial = [{"identity": str(index)} for index in range(4)]
            roster = w.DesiredRoster(Path(directory), initial)
            roster.prepare_probe(SimpleNamespace(id=w.C3_ADMISSION))
            selection = MemberMcpAdmissionSelection(w.C3_ADMISSION, w.C3_ID, w.C3_CLASS)
            empty = RetainedMemberMcpBindings(selection, ())
            await roster.retain_empty_mcp(empty)
            self.assertEqual(await roster.lookup_empty_mcp(selection), empty)
            with self.assertRaises(AssertionError):
                await roster.retain_empty_mcp(RetainedMemberMcpBindings(selection, ("invented-binding",)))
            with self.assertRaises(AssertionError):
                await roster.lookup_empty_mcp(dataclasses.replace(selection, admission_id="foreign"))
            await roster.publish_probe()
            roster.prepare(request())
            await roster.publish()
            self.assertEqual(roster.selected(), initial + [roster.probe_spec(), roster.spec()])
            self.assertEqual((roster.probe_publications, roster.publications), (1, 1))
            with self.assertRaises(AssertionError):
                await roster.publish_probe()


class FocusedModeControls(unittest.IsolatedAsyncioTestCase):
    def test_cli_requires_one_explicit_scope_and_keeps_full_read_opt_in(self):
        common = ["--toolkit-gateway", "/fixture/bin/toolkit_gateway", "--toolkit-gateway-sha256", "a" * 64,
            "--resources-sha256", "b" * 64, "--toolkit-python", "/fixture/connect", "--meta-python", "/fixture/meta",
            "--output", "/fixture/output"]
        self.assertTrue(w.parse_args(common + ["--ordinary-customizer-only"]).ordinary_customizer_only)
        helperless = w.parse_args(common + ["--helperless-restore"])
        self.assertTrue(helperless.helperless_restore)
        self.assertFalse(helperless.allow_public_hn_read or helperless.ordinary_customizer_only or helperless.respawn_shared_g)
        both = w.parse_args(common + ["--both-selected-success"])
        self.assertTrue(both.both_selected_success)
        self.assertFalse(both.allow_public_hn_read or both.cold_shared_restore or both.withhold_p_completion)
        full = w.parse_args(common + ["--allow-public-hn-read"])
        self.assertTrue(full.allow_public_hn_read)
        self.assertFalse(full.ordinary_customizer_only)
        self.assertFalse(full.respawn_shared_g)
        self.assertTrue(w.parse_args(common + ["--allow-public-hn-read", "--respawn-shared-g"]).respawn_shared_g)
        self.assertTrue(w.parse_args(common + ["--allow-public-hn-read", "--cold-shared-restore"]).cold_shared_restore)
        self.assertFalse(w.parse_args(common + ["--allow-public-hn-read", "--cold-shared-restore"]).withhold_p_completion)
        self.assertTrue(w.parse_args(common + ["--allow-public-hn-read", "--cold-shared-restore",
            "--withhold-p-completion"]).withhold_p_completion)
        for flags in ([], ["--both-selected-success", "--allow-public-hn-read"],
                      ["--both-selected-success", "--cold-shared-restore"],
                      ["--both-selected-success", "--withhold-p-completion"],
                      ["--both-selected-success", "--respawn-shared-g"],
                      ["--both-selected-success", "--helperless-restore"],
                      ["--ordinary-customizer-only", "--allow-public-hn-read"],
                      ["--ordinary-customizer-only", "--respawn-shared-g"], ["--respawn-shared-g"],
                      ["--helperless-restore", "--allow-public-hn-read"],
                      ["--helperless-restore", "--ordinary-customizer-only"],
                      ["--helperless-restore", "--respawn-shared-g"], ["--cold-shared-restore"],
                      ["--helperless-restore", "--cold-shared-restore"],
                      ["--allow-public-hn-read", "--cold-shared-restore", "--respawn-shared-g"],
                      ["--allow-public-hn-read", "--withhold-p-completion"],
                      ["--helperless-restore", "--withhold-p-completion"]):
            with contextlib.redirect_stderr(io.StringIO()), self.assertRaises(SystemExit):
                w.parse_args(common + flags)

    def test_focused_model_cannot_fall_through_to_workflow_or_hn_schedule(self):
        model = object.__new__(w.AcceptanceModel)
        model.ordinary_only = True
        with self.assertRaisesRegex(AssertionError, "refused a workflow or public-read"):
            model.response({"messages": [{"role": "user", "content": w.s.request("b")}]})

    async def test_focused_run_excludes_full_workflow_and_joins_listing_and_login_on_failure(self):
        @dataclasses.dataclass
        class Retired:
            fenced: bool = True
            reservations_drained: bool = True
            uncertain: bool = False
        for fail in (False, True):
            with tempfile.TemporaryDirectory() as directory:
                directory = Path(directory)
                scenario = w.Scenario(w.v, "http://127.0.0.1:12345")
                scenario.ordinary_customizer_only = True
                retired = []
                enrollment = SimpleNamespace(result=lambda: None,
                    retire=lambda **kwargs: (retired.append(True), Retired())[1])
                private = SimpleNamespace(begin_console_login=lambda *args, **kwargs: enrollment)
                scenario.observe_runtime(SimpleNamespace(host_control=private))
                scenario.roster = SimpleNamespace(publications=0, admitting=directory / "not-created",
                    selected=lambda: [{"identity": "initial"}])
                scenario.probe_attempt = SimpleNamespace(join_listing=AsyncMock())
                async def ordinary(*args):
                    self.assertIs(args[0], private)
                    scenario.c3 = {"status": "native_rebuild_observed", "after_create": {"state": "observed"},
                        "after_create_contexts_before": ["one"], "after_create_contexts_after": ["one", "two"]}
                    if fail:
                        raise RuntimeError("controlled actual observation refusal")
                scenario.check_ordinary_rebuild = AsyncMock(side_effect=ordinary)
                scenario.begin_workflow = AsyncMock(side_effect=AssertionError("workflow must not run"))
                provider = SimpleNamespace(lock=threading.RLock(), requests=[], enabled=False, errors=[],
                    business_observation=None, issued={"c3-before-call", "c3-after-call"})
                sdk = SimpleNamespace(ConsoleLoginSelection=lambda *args: args)
                with patch.object(w.v, "SDK", sdk, create=True), patch.object(w.s.Scenario, "run",
                    new=AsyncMock(side_effect=AssertionError("full workflow must not run"))):
                    arguments = (private, provider, directory, None,
                        SimpleNamespace(args=["/fixture/bin/toolkit_gateway"]), scenario.origin)
                    if fail:
                        with self.assertRaisesRegex(RuntimeError, "controlled actual observation refusal"):
                            await scenario.run(*arguments)
                    else:
                        result = await scenario.run(*arguments)
                        self.assertEqual(result["status"], "passed")
                        self.assertFalse(result["workflow_commissioned"])
                        self.assertFalse(result["public_hn_read"])
                scenario.check_ordinary_rebuild.assert_awaited_once()
                scenario.begin_workflow.assert_not_awaited()
                scenario.probe_attempt.join_listing.assert_awaited_once()
                self.assertEqual(retired, [True])
                self.assertEqual(json.loads((directory / "console-journey.json").read_bytes())["status"],
                                 "failed" if fail else "passed")

    async def test_full_mode_still_delegates_to_original_workflow_and_listing_owner(self):
        scenario = w.Scenario(w.v, "http://127.0.0.1:12345")
        scenario.observe_runtime(None)
        scenario.run_ordinary_customizer = AsyncMock(side_effect=AssertionError("focused path must not run"))
        scenario.commission_attempt = SimpleNamespace(join_listing=AsyncMock())
        with patch.object(w.s.Scenario, "run", new=AsyncMock(return_value={"full": "original"})) as full:
            self.assertEqual(await scenario.run("unchanged"), {"full": "original"})
        full.assert_awaited_once_with("unchanged")
        scenario.run_ordinary_customizer.assert_not_awaited()
        scenario.commission_attempt.join_listing.assert_awaited_once()


class SharedRespawnControls(unittest.IsolatedAsyncioTestCase):
    def observations(self):
        before, after, result = OrdinaryRebuildOracleControls().observations()
        for row in (before, after):
            row["status"]["identity"] = row["tools"]["identity"] = w.G_ID
            row["tools"]["tools"] = list(w.s.SETUP)
            record = row["canonical"]["record"]
            record.update(id=w.ADMISSION, **{"class": {"id": w.CLASS_ID}},
                instance={"class": w.CLASS_ID, "owner": "exact-owner"},
                workflow={"workflow": {"key": "exact-workflow"}}, route={"id": w.G_ID},
                phase={"kind": "active", "context": "session"}, native_surface=list(w.s.SETUP))
        result["identity"] = w.G_ID
        return before, after, result

    def test_respawn_must_retain_exact_canonical_owner_and_session_and_rotate_native_runtime(self):
        w.require_g_transition(*self.observations())
        changes = (
            lambda a, r: a["canonical"]["record"]["instance"].update(owner="foreign"),
            lambda a, r: a["canonical"]["record"].update(workflow=None),
            lambda a, r: a["canonical"]["record"]["phase"].update(context="foreign"),
            lambda a, r: a["canonical"]["record"].update(id="another-add"),
            lambda a, r: (a["status"].update(agent_runtime_id="runtime-before"), r.update(agent_runtime_id="runtime-before")),
            lambda a, r: (a["status"].update(generation=6), r.update(generation=6)),
            lambda a, r: (a["status"].update(session_id="new-session"), r.update(session_id="new-session")),
            lambda a, r: a["tools"].update(tools=[]),
            lambda a, r: r.update(live_respawn_warning="unjoined"),
        )
        for change in changes:
            before, after, result = self.observations()
            change(after, result)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                w.require_g_transition(before, after, result)

    def test_canonical_delivery_selects_exact_source_and_finalized_g_input_not_original_a_answer(self):
        source, workflow = {"id": "A"}, {"key": "workflow"}
        delivery = {"membership": {"identity": {"source": source}, "workflow": workflow},
                    "session": "G-session", "input": "G-context"}
        effect = {"status": "Succeeded", "outcome": {"Succeeded": {"receipt": {
            "schema": "toolkit.workflow-context-input", "version": 1, "body": delivery}}}}
        row = {"input_id": "G-context", "terminal_completion": {
            "input_id": "G-context", "owner_input_id": "G-context", "completion_input_ids": ["G-context"],
            "phase": {"phase": "finalized", "finalization": "succeeded"},
            "outcome": {"completion_type": "completed", "session_id": "G-session", "text": "A received"}}}
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            path = root / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
            path.parent.mkdir(parents=True)
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE toolkit_effects(realm,owner,record_id,id,data)")
                def insert(value, ident):
                    db.execute("INSERT INTO toolkit_effects VALUES (?,?,?,?,?)",
                        ("mob.fixture", "owner", "record", ident, json.dumps(value)))
                    db.commit()
                foreign = copy.deepcopy(effect)
                foreign["outcome"]["Succeeded"]["receipt"]["body"]["membership"]["identity"]["source"] = {"id": "B"}
                insert(foreign, "foreign")
                self.assertIsNone(w.context_delivery(root, source, workflow, "G-session"))
                insert(effect, "exact")
                self.assertEqual(w.context_delivery(root, source, workflow, "G-session"), delivery)
                with self.assertRaisesRegex(AssertionError, "changed workflow"):
                    w.context_delivery(root, source, workflow, "another-session")
                insert(effect, "duplicate")
                with self.assertRaisesRegex(AssertionError, "ambiguous"):
                    w.context_delivery(root, source, workflow, "G-session")
        self.assertIsNone(w.finalized_context([{**row, "input_id": "original-A"}], delivery, "A received"))
        pending = copy.deepcopy(row); pending["terminal_completion"]["phase"] = {"phase": "pending"}
        self.assertIsNone(w.finalized_context([pending], delivery, "A received"))
        self.assertEqual(w.finalized_context([row], delivery, "A received"), row)
        for field, value in (("owner_input_id", "original-A"), ("completion_input_ids", ["G-context", "foreign"]),
                             ("outcome", {"completion_type": "failed"}),
                             ("phase", {"phase": "finalized", "finalization": "failed"})):
            changed = copy.deepcopy(row); changed["terminal_completion"][field] = value
            with self.subTest(field=field), self.assertRaises(AssertionError):
                w.finalized_context([changed], delivery, "A received")

    async def test_real_sdk_shared_stale_probe_requires_runtime_id_refusal_not_malformed_or_other_scope(self):
        from meerkat_meta.errors import RpcError, StaleScopeError
        from meerkat_meta.identity_first_models import DeliveryScope
        from meerkat_meta.runtime import MetaRuntime
        runtime = object.__new__(MetaRuntime)
        scope = DeliveryScope(w.G_ID, "old-runtime", 4, 7, {"session_id": "same-session"})
        response = StaleScopeError("stale scope", mismatch="runtime_id")
        async def rpc(method, params):
            self.assertEqual((method, params["identity"]), ("meta/dispatch", w.G_ID))
            self.assertEqual(params["expected_scope"], scope.to_dict())
            self.assertNotEqual(uuid.UUID(params["dispatch_input"]["correlation_id"]).int, 0)
            self.assertEqual(params["dispatch_input"]["idempotency_key"], "acceptance-stale-shared-g-delivery")
            if isinstance(response, Exception):
                raise response
            return response
        runtime._rpc = rpc
        self.assertEqual((await w.require_g_stale_scope(runtime, scope))["mismatch"], "runtime_id")
        response = StaleScopeError("wrong boundary", mismatch="generation")
        with self.assertRaisesRegex(AssertionError, "native runtime continuity"):
            await w.require_g_stale_scope(runtime, scope)
        response = RpcError(-32602, "malformed input")
        with self.assertRaises(RpcError):
            await w.require_g_stale_scope(runtime, scope)
        response = "accepted"
        with self.assertRaisesRegex(AssertionError, "was not refused"):
            await w.require_g_stale_scope(runtime, scope)

    async def test_g_callbacks_keep_drafts_unchanged_and_require_same_session_twice(self):
        from meerkat_meta.identity_first_models import AgentBuildDraft, DurableAgentSpec
        from meerkat_meta.models import SessionCreatedContext
        customizer = w.AcceptanceCustomizer(shared_profile_model="G-model")
        for count in (1, 2):
            draft = AgentBuildDraft(labels={"selected": "unchanged"})
            before = copy.deepcopy(draft.to_dict())
            await customizer.customize_build(SimpleNamespace(identity=w.G_ID),
                DurableAgentSpec(identity=w.G_ID, profile=w.CLASS_ID), draft)
            await customizer.after_create(w.G_ID, "same-session", SessionCreatedContext.from_dict(
                {"model": "G-model", "labels": {"selected": "unchanged"}, "system_prompt": None}))
            self.assertEqual(draft.to_dict(), before)
            self.assertEqual(len(await customizer.wait_shared_created(count, "same-session", time.monotonic())), count)
        with self.assertRaises(AssertionError):
            await customizer.wait_shared_created(2, "foreign-session", time.monotonic())

    async def test_real_respawn_entry_waits_for_finalized_a_context_and_never_retries(self):
        before, after, result = self.observations()
        with tempfile.TemporaryDirectory() as root:
            scenario = w.Scenario(w.v, "http://127.0.0.1:12345")
            ready, calls = [], []
            snapshots = iter((before, after))
            current = before
            async def status(identity):
                nonlocal current
                self.assertEqual(identity, w.G_ID)
                current = next(snapshots)
                return SimpleNamespace(session_id="session", delivery_scope=SimpleNamespace(session_id="session"),
                                       to_dict=lambda: current["status"])
            async def respawn(identity):
                self.assertEqual(identity, w.G_ID)
                self.assertEqual(ready, ["A finalized"])
                calls.append("respawn")
                for row in rpc_callbacks[len(initial_callbacks):]:
                    scenario.customizer.record_respawn_event(row)
                return result
            async def release_finalization(_):
                self.assertFalse(calls)
                ready.append("A finalized")
            runtime = SimpleNamespace(status=status, respawn=AsyncMock(side_effect=respawn),
                mob_handle=lambda: SimpleNamespace(identity_resolved_tools_detail=AsyncMock(return_value="tools")),
                member_classes=SimpleNamespace(observe=lambda *a, **kw: current["canonical"]))
            scenario.observe_runtime(runtime)
            scenario.resource_python = "/selected/python"
            scenario.precommission_pids = set()
            scenario.roster = SimpleNamespace(selected=lambda: ["exact-roster"], publications=1)
            initial_callbacks, rpc_callbacks, _ = respawn_observations(w.G_ID, new_builds=2, session_id="session")
            scenario.customizer = w.AcceptanceCustomizer()
            for row in initial_callbacks:
                scenario.customizer.record_respawn_event(row)
            scenario.customizer.wait_shared_created = AsyncMock(return_value=["initial callback"])
            provider = SimpleNamespace(lock=threading.Lock(), errors=[], b_context=threading.Event(), issued=set())
            delivery, rows = {"input": "A-context"}, [{"input_id": "A-context"}]
            with patch.object(w.dataclasses, "asdict", side_effect=lambda value: current["tools"]), \
                    patch.object(w, "context_delivery", return_value=delivery), \
                    patch.object(w, "finalized_context", side_effect=lambda *a: rows[0] if ready else None), \
                    patch.object(w.s, "input_rows", return_value=rows), \
                    patch.object(w.s, "wait_initial_a_completion", new=AsyncMock()), \
                    patch.object(w.asyncio, "sleep", side_effect=release_finalization), \
                    patch.object(w, "shared_peer", return_value={"owner_pid": 10, "bridge_pid": 11}), \
                    patch.object(w, "require_retired_transport", return_value={"state": "connection_refused"}), \
                    patch.object(w, "require_g_stale_scope", new=AsyncMock(return_value={"mismatch": "runtime_id"})):
                await scenario.check_shared_rebuild({"source_key": "A", "admission": {}}, "member", provider,
                    Path(root), SimpleNamespace(pid=7), time.monotonic() + 5)
            runtime.respawn.assert_awaited_once_with(w.G_ID)
            self.assertEqual(scenario.g_respawn["status"], "respawn_observed_awaiting_b")
            self.assertTrue(scenario.g_respawn["stale_scope_inputs_unchanged"])

    def test_success_gate_requires_fresh_b_contexts_own_successful_install_not_nonempty_refusal_audit(self):
        _, after, _ = self.observations()
        delivery = {"input": "B-context"}
        def audit(kind, **fields):
            return {"contributors": [{"input_id": "B-context"}], "observation": {
                "operation_id": "install-operation", "observation": {"kind": kind, **fields}}}
        records = [audit("prepared", target={"kind": "tool", "tool_name": "setup_install", "call_id": "g-install"}),
            audit("entry"), audit("outcome", outcome={"kind": "tool_dispatch_returned", "result_is_error": False,
                                                      "terminal_error": None, "asynchronous_operations": []})]
        mutations = [None, lambda rows: rows[-1]["authorization_audit"][0].update(contributors=[{"input_id": "foreign"}]),
            lambda rows: rows[-1].update(authorization_audit=[audit("refused")]),
            lambda rows: rows[-1]["authorization_audit"].append(audit("refused")),
            lambda rows: rows[-1]["authorization_audit"][2]["observation"]["observation"]["outcome"].update(result_is_error=True),
            lambda rows: rows.append({"input_id": "unexpected"}),
            lambda rows: rows.pop(0)]
        for mutate in mutations:
            scenario = w.Scenario(w.v, "http://127.0.0.1:12345")
            scenario.observe_runtime(None)
            scenario.respawn_shared_g = True
            c3_before, c3_returned, c3_current = respawn_observations()
            c3_proof = w.require_respawn_callbacks(c3_before, c3_returned, c3_current, w.C3_ID, "same-session")
            g_before, g_returned, g_current = respawn_observations(w.G_ID, new_builds=2, session_id="session")
            g_proof = w.require_respawn_callbacks(g_before, g_returned, g_current, w.G_ID, "session")
            scenario.c3 = {"status": "native_rebuild_observed", "after_create": {"state": "observed"},
                           "respawn_callbacks": c3_proof}
            predecessor = {"input_id": "A-context"}
            scenario.g_respawn = {"status": "respawn_observed_awaiting_b", "after": after,
                                  "before_inputs": [predecessor], "callbacks_after": g_proof["created"],
                                  "respawn_callbacks": g_proof}
            scenario.g_b_source, scenario.g_directory = "B", Path("/fixture")
            scenario.customizer = w.AcceptanceCustomizer()
            for row in c3_current + g_current:
                scenario.customizer.record_respawn_event(row)
            scenario.customizer.shared_created_contexts = copy.deepcopy(g_proof["created"])
            scenario.customizer.shared_build_contexts = [row["context"] for row in g_current if row["kind"] == "build"]
            scenario.customizer.builds = [{"identity": w.G_ID}] * 3
            rows = [copy.deepcopy(predecessor), {"input_id": "B-context", "authorization_audit": copy.deepcopy(records)}]
            if mutate:
                mutate(rows)
            with self.subTest(mutate=mutate), patch.object(w, "context_delivery", return_value=delivery), \
                    patch.object(w.s, "input_rows", return_value=rows), patch.object(w.v, "save_json"):
                if mutate:
                    with self.assertRaises(AssertionError):
                        scenario.verify_customizer()
                else:
                    self.assertEqual(scenario.verify_customizer()["shared_g_rebuild"]["status"], "fresh_b_work_observed")

    def test_owned_peer_lookup_rejects_ambiguity_and_old_socket_probe_never_connects_successor(self):
        with tempfile.TemporaryDirectory() as root, tempfile.TemporaryDirectory(prefix="connect-setup-", dir="/tmp") as sockets:
            root, sockets = Path(root).resolve(), Path(sockets).resolve()
            artifact = root / "setup-owned"
            config = artifact / "prepared/config/mcp.json"
            helper = artifact / "prepared/distribution/setup/local_session.py"
            config.parent.mkdir(parents=True); helper.parent.mkdir(parents=True)
            for path in (config, helper, helper.with_name("mcp_adapter.py"), helper.with_name("backend.py")):
                path.write_bytes(b"exact fixture bytes")
            digest = hashlib.sha256(b"exact fixture bytes").hexdigest()
            python = "/selected/python"
            args = ["-i", "PATH=/usr/bin:/bin", "PYTHONDONTWRITEBYTECODE=1", python, "-I", "-B", str(helper),
                "--configuration", str(config), "--configuration-sha256", digest, "--helper-sha256", digest,
                "--adapter-sha256", digest, "--backend-sha256", digest, "--state-root", str(artifact / "prepared/local-session")]
            (artifact / "prepare.stdout").write_text(json.dumps({"mcpServers": {"toolkit-setup": {"command": "/usr/bin/env", "args": args}}}))
            old, new = sockets / "0.sock", sockets / "1.sock"
            with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as old_listener, socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as new_listener:
                for listener, path in ((old_listener, old), (new_listener, new)):
                    listener.bind(str(path)); os.chmod(path, 0o600); listener.listen(1)
                commands = {10: args[3:] + ["--shared-peers", "1", "--shared-close-id", "a" * 64],
                            11: args[3:7] + ["--shared-peer", str(old)]}
                def ps(command, **kwargs):
                    self.assertEqual(command[:2], ["/bin/ps", "-p"])
                    return SimpleNamespace(returncode=0, stdout=shlex.join(commands[int(command[2])]) + "\n")
                def descendants(pid):
                    self.assertEqual(pid, 7)
                    return [{"pid": value} for value in commands]
                with patch.object(w.v, "child_snapshot", side_effect=descendants), \
                        patch.object(w.v.subprocess, "run", side_effect=ps), \
                        patch.object(w.v, "alive", return_value=False) as old_alive:
                    before = w.shared_peer(root, 7, python)
                    commands[12] = args[3:7] + ["--shared-peer", str(new)]
                    with self.assertRaisesRegex(AssertionError, "unique owned shared peer"):
                        w.shared_peer(root, 7, python)
                    del commands[11]
                    after = w.shared_peer(root, 7, python)
                    with self.assertRaisesRegex(AssertionError, "still accepts"):
                        w.require_retired_transport(before, after)
                    old_listener.close()
                    old_alive.return_value = True
                    with self.assertRaisesRegex(AssertionError, "retired shared peer process still exists"):
                        w.require_retired_transport(before, after)
                    old_alive.assert_called_with(11)
                    old_alive.return_value = False
                    self.assertEqual(w.require_retired_transport(before, after)["state"], "connection_refused")
                    new_listener.setblocking(False)
                    with self.assertRaises(BlockingIOError):
                        new_listener.accept()
                    commands[10][-3] = "2"
                    with self.assertRaisesRegex(AssertionError, "exact native close/restore selection"):
                        w.shared_peer(root, 7, python)




class ColdSharedRestoreControls(unittest.IsolatedAsyncioTestCase):
    async def test_final_close_requires_exact_workflow_and_original_g_finalized_receipt(self):
        workflow = {"key": {"realm": "mob.fixture", "owner": "workflow-owner", "id": "workflow-id"},
                    "terms_digest": "a" * 64}
        effect = {"key": {"record": workflow["key"], "id": "close"},
            "intent": {"id": "close", "kind": "setup_workflow_close", "request": {
                "schema": "meerkat_toolkit.setup_workflow.close", "version": 1,
                "body": {"workflow": workflow, "payload": workflow}}},
            "status": "Succeeded", "outcome": {"Succeeded": {"receipt": {
                "schema": "toolkit.setup-workflow-close", "version": 1,
                "body": {"state": "source_fenced_and_finalized", "session_id": "G-session", "input_id": "G-original"}}}}}
        changes = (
            ("foreign effect key", lambda e: e["key"].update(id="launch")),
            ("foreign workflow", lambda e: e["intent"]["request"]["body"].update(workflow={})),
            ("foreign intent", lambda e: e["intent"].update(kind="setup_workflow_launch")),
            ("foreign schema", lambda e: e["outcome"]["Succeeded"]["receipt"].update(schema="another-receipt")),
            ("foreign version", lambda e: e["outcome"]["Succeeded"]["receipt"].update(version=2)),
            ("foreign session", lambda e: e["outcome"]["Succeeded"]["receipt"]["body"].update(session_id="B-session")),
            ("context input", lambda e: e["outcome"]["Succeeded"]["receipt"]["body"].update(input_id="G-context")),
            ("not admitted", lambda e: e["outcome"]["Succeeded"]["receipt"].update(body={"state": "source_fenced_and_not_admitted"})),
            ("pending receipt", lambda e: e["outcome"]["Succeeded"]["receipt"].update(body={"state": "pending"})),
            ("failed", lambda e: e.update(status="Failed")),
        )
        failure_observer = w.s.SelectedHostLoopFailure(w.G_ID, "restored")
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            path = root / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
            path.parent.mkdir(parents=True)
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE toolkit_effects(realm,owner,record_id,id,data)")
                def store(value, ident="close"):
                    db.execute("DELETE FROM toolkit_effects")
                    if value is not None:
                        db.execute("INSERT INTO toolkit_effects VALUES(?,?,?,?,?)",
                            ("mob.fixture", "workflow-owner", "workflow-id", ident, json.dumps(value)))
                    db.commit()
                store(effect)
                observed = {}
                self.assertEqual(await w.wait_finalized_g_close(root, workflow, "G-session", "G-original",
                    time.monotonic() + 1, observed, failure_observer=failure_observer), effect)
                self.assertEqual(observed["effect"], effect)
                for name, change in changes:
                    changed = copy.deepcopy(effect); change(changed); store(changed)
                    observed = {}
                    with self.subTest(name=name), self.assertRaisesRegex(AssertionError, "original G close"):
                        await w.wait_finalized_g_close(root, workflow, "G-session", "G-original",
                            time.monotonic() + 1, observed, failure_observer=failure_observer)
                    self.assertEqual(observed["effect"], changed)
                # Missing/foreign-key rows and nonterminal effects cannot pass,
                # even when a stale successful receipt remains in their body.
                for status in (None, "foreign-key", "Pending", "InFlight", "Uncertain"):
                    changed = copy.deepcopy(effect)
                    if status not in (None, "foreign-key"):
                        changed["status"] = status
                    store(None if status is None else changed, "foreign-close" if status == "foreign-key" else "close")
                    observed = {}
                    with self.subTest(status=status), patch.object(w.time, "monotonic", side_effect=[0, 2]), \
                            patch.object(w.asyncio, "sleep", new_callable=AsyncMock), \
                            self.assertRaisesRegex(AssertionError, "did not finalize before shutdown"):
                        await w.wait_finalized_g_close(root, workflow, "G-session", "G-original", 1, observed, failure_observer=failure_observer)
                    self.assertEqual(observed["effect"], None if status in (None, "foreign-key") else changed)
                store({**effect, "status": "Uncertain"})
                async def complete_close(_):
                    store(effect)
                observed = {}
                with patch.object(w.asyncio, "sleep", side_effect=complete_close) as waited:
                    self.assertEqual(await w.wait_finalized_g_close(root, workflow, "G-session", "G-original",
                        time.monotonic() + 1, observed, failure_observer=failure_observer), effect)
                waited.assert_awaited_once_with(0.05)
                self.assertEqual(observed["effect"], effect)

    def test_cold_physical_oracle_requires_new_joined_owner_and_exact_predecessor_snapshot(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            snapshot = root / "prepared/local-session/shared-restore-claim.json"
            snapshot.parent.mkdir(parents=True)
            snapshot.write_bytes(b"exact owner-produced closed snapshot")
            self.assertFalse(snapshot.with_name("shared-close.json").exists())
            before = {"owner_pid": 11, "bridge_pid": 12, "artifact_directory": str(root), "close_id": "a" * 64,
                      "restore_sha256": None, "path": "/old/0.sock", "inode": [1, 2]}
            after = {"owner_pid": 21, "bridge_pid": 22, "artifact_directory": str(root), "close_id": "b" * 64,
                     "restore_sha256": hashlib.sha256(snapshot.read_bytes()).hexdigest(), "path": "/new/0.sock", "inode": [1, 3]}
            with patch.object(w.v, "alive", return_value=False):
                self.assertEqual(w.require_cold_transport(before, after)["sha256"], after["restore_sha256"])
                for change in (lambda row: row.update(owner_pid=11), lambda row: row.update(bridge_pid=12),
                    lambda row: row.update(close_id=before["close_id"]), lambda row: row.update(restore_sha256=None),
                    lambda row: row.update(artifact_directory="/another/artifact"), lambda row: row.update(inode=[1, 2])):
                    changed = copy.deepcopy(after); change(changed)
                    with self.subTest(change=change), self.assertRaises(AssertionError):
                        w.require_cold_transport(before, changed)
                snapshot.write_bytes(b"changed retained snapshot")
                with self.assertRaises(AssertionError):
                    w.require_cold_transport(before, after)
            with patch.object(w.v, "alive", return_value=True), self.assertRaises(AssertionError):
                w.require_cold_transport(before, after)

    def test_cold_identity_oracle_refuses_new_admission_session_helper_or_missing_mcp(self):
        before, after, _ = SharedRespawnControls().observations()
        before["status"]["delivery_scope"] = {"version": 1, "identity": w.G_ID,
            "agent_runtime_id": "runtime-before", "generation": 4, "lease_fencing_token": 1,
            "member": {"version": 2, "mob_incarnation": "same-mob",
                "runtime_id": {"identity": w.G_ID, "generation": 4}, "session_id": "session", "fence_token": 1}}
        after["status"] = copy.deepcopy(before["status"])
        for row in (before, after):
            row["canonical"]["record"]["destination"] = {"context": "session", "context_generation": 1}
            row["canonical"]["record"]["phase"]["context_generation"] = 1
        w.require_cold_g_transition(before, after)
        for change in (
            lambda row: row["canonical"]["record"].update(id="replacement"),
            lambda row: row["canonical"]["record"]["workflow"].update(workflow={"key": "replacement"}),
            lambda row: row["canonical"]["record"].update(destination=None),
            lambda row: row["canonical"]["record"]["destination"].update(context="replacement"),
            lambda row: row["canonical"]["record"]["destination"].update(context_generation=2),
            lambda row: row["canonical"]["record"]["phase"].update(context_generation=2),
            lambda row: row["canonical"]["record"].update(native_surface=[]),
            lambda row: row["status"].update(agent_runtime_id="runtime-after"),
            lambda row: row["status"].update(generation=5),
            lambda row: row["status"]["delivery_scope"]["member"].update(fence_token=2),
            lambda row: row["status"].update(delivery_scope=None),
            lambda row: row["status"].update(session_id="fresh-session"),
            lambda row: row["tools"].update(tools=[]),
            lambda row: row["canonical"].update(authorization={"state": "refused"}),
        ):
            changed = copy.deepcopy(after); change(changed)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                w.require_cold_g_transition(before, changed)

    def test_cold_callbacks_require_one_fresh_build_and_retained_creation(self):
        context = {"model": "selected-model", "labels": {"purpose": "setup"}, "system_prompt": "exact prompt"}
        created = [{"identity": w.G_ID, "session_id": "session", "context": copy.deepcopy(context)}]
        before = {"callbacks_before": copy.deepcopy(created), "build_contexts_before": [copy.deepcopy(context)]}
        customizer = SimpleNamespace(builds=[{"identity": w.G_ID, "profile": w.CLASS_ID}] * 2,
            shared_build_contexts=[copy.deepcopy(context), copy.deepcopy(context)],
            shared_created_contexts=copy.deepcopy(created))
        observed = w.require_cold_g_callbacks(customizer, before, "session")
        self.assertEqual(observed, created)
        observed[0]["context"]["labels"]["purpose"] = "changed-copy"
        self.assertEqual(customizer.shared_created_contexts, created)
        for change, message in (
            (lambda row: row.builds.pop(), "one unchanged callback build"),
            (lambda row: row.builds.append({"identity": w.G_ID, "profile": w.CLASS_ID}), "one unchanged callback build"),
            (lambda row: row.builds.__setitem__(1, {"identity": w.G_ID, "profile": "foreign"}), "one unchanged callback build"),
            (lambda row: row.shared_build_contexts.pop(), "one unchanged callback build"),
            (lambda row: row.shared_build_contexts[1].update(model="foreign"), "one unchanged callback build"),
            (lambda row: row.shared_created_contexts.append(copy.deepcopy(created[0])), "historical creation"),
            (lambda row: row.shared_created_contexts[0].update(session_id="foreign"), "historical creation"),
            (lambda row: row.shared_created_contexts[0]["context"].update(system_prompt="foreign"), "historical creation"),
        ):
            changed = copy.deepcopy(customizer); change(changed)
            with self.subTest(change=change), self.assertRaisesRegex(AssertionError, message):
                w.require_cold_g_callbacks(changed, before, "session")
        with self.assertRaisesRegex(AssertionError, "predecessor"):
            w.require_cold_g_callbacks(customizer, before, "foreign-session")

    def test_first_phase_release_does_not_admit_b_or_approve_its_bound_replay(self):
        model = OriginalResponseGateControls().model()
        model.original_gates["a"].update(approved=True, deadline=time.monotonic() + 30)
        model.release_original_responses(tags=("a",))
        self.assertTrue(model.original_gates["a"]["released"].is_set())
        self.assertFalse(model.original_gates["b"]["approved"] or model.original_gates["b"]["released"].is_set())
        with self.assertRaises(AssertionError):
            model.release_original_responses(tags=("b",))
        model.release_original_responses(abort=True)
        self.assertFalse(model.original_gates["b"]["approved"])
        self.assertTrue(model.original_gates["b"]["released"].is_set())

    def test_historical_planning_locator_requires_original_bound_mcp_provenance(self):
        with tempfile.TemporaryDirectory() as root:
            root = Path(root)
            path = root / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
            path.parent.mkdir(parents=True)
            event = {"route": w.G_ID, "original": "G", "content_sha256": "c" * 64}
            original = Reference(w.G_ID, "G", hashlib.sha256(w.v.canonical(event)).hexdigest())
            parent = Reference("planning-parent", "P", "b" * 64)
            body = {"event": event, "planning_source_evidence": {"role": "lead", "parent": dataclasses.asdict(parent)},
                "late_workflow_helper": {"admission": {"id": w.ADMISSION, "workflow": {
                    "parent": dataclasses.asdict(parent), "original": dataclasses.asdict(original)}},
                    "server": {"name": "toolkit-setup", "connector": None}},
                "bound_service_input": {"original": "retained-native-input"}, "final_association": {"input": "original-lineage"}}
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE toolkit_records(realm TEXT,owner TEXT,id TEXT,data TEXT)")
                def store(value):
                    db.execute("DELETE FROM toolkit_records")
                    db.execute("INSERT INTO toolkit_records VALUES(?,?,?,?)", ("mob.fixture", "owner", "G-source",
                        json.dumps({"schema": "toolkit.authenticated-source-event", "body": value})))
                    db.commit()
                store(body)
                observed = w.planning_lineage(root, original, parent)
                self.assertEqual(observed["late_workflow_helper"], body["late_workflow_helper"])
                for change in (lambda value: value.pop("late_workflow_helper"),
                    lambda value: value.update(bound_service_input=None),
                    lambda value: value.update(final_association=None),
                    lambda value: value["late_workflow_helper"]["server"].update(name="foreign"),
                    lambda value: value["late_workflow_helper"]["admission"]["workflow"].update(parent={}),
                    lambda value: value["planning_source_evidence"].update(role="waiter")):
                    changed = copy.deepcopy(body); change(changed); store(changed)
                    with self.subTest(change=change), self.assertRaises(AssertionError):
                        w.planning_lineage(root, original, parent)

    async def test_restored_phase_launches_only_fresh_b_and_never_recommissions(self):
        scenario = object.__new__(w.ColdSharedScenario)
        scenario.phase = "restored"
        scenario.cold = {"status": "restored_awaiting_b", "model_requests": 4}
        original_a = {"original": {"route": "original-a"}, "source_key": {"id": "a"}}
        scenario.first_launch = {"children": [original_a]}
        scenario.launch_children = AsyncMock(return_value={"children": [
            {"original": {"route": "original-b"}, "source_key": {"id": "b"}}]})
        provider = SimpleNamespace(requests=[None] * 4, errors=[], b_context=threading.Event(), issued={"a-find"}, enabled=False)
        scenario.begin_model_phase(provider)
        self.assertTrue(provider.enabled)
        with patch.object(w.Scenario, "begin_workflow", new_callable=AsyncMock) as commission:
            result = await scenario.begin_workflow("private", provider, Path("/case"), "events", "process", 30)
            commission.assert_not_awaited()
        self.assertEqual(scenario.launch_children.await_args.kwargs, {"tags": ("b",)})
        self.assertEqual([row["original"]["route"] for row in result["children"]], ["original-a", "original-b"])
        self.assertEqual(result["children"][0], original_a)
        for change in (lambda: provider.requests.append(None), lambda: provider.errors.append("native-refusal"),
                       lambda: provider.b_context.set()):
            change()
            with self.assertRaises(AssertionError):
                scenario.begin_model_phase(provider)
        scenario.cold["status"] = "failed"
        scenario.launch_children.reset_mock()
        with self.assertRaises(AssertionError):
            await scenario.begin_workflow("private", provider, Path("/case"), "events", "process", 30)
        scenario.launch_children.assert_not_awaited()

    async def test_predecessor_retains_selected_a_through_close_and_failure_releases_gates(self):
        @dataclasses.dataclass
        class Retired:
            fenced: bool = True
            reservations_drained: bool = True
            uncertain: bool = False
        @dataclasses.dataclass
        class Cancelled:
            cancelled: bool = True
        for refusal in (False, True):
            events = []
            scenario = object.__new__(w.ColdSharedScenario)
            scenario.phase = "prepared"
            scenario.host_loop_failure = w.s.SelectedHostLoopFailure(w.G_ID, scenario.phase)
            p, g = Reference("planning-parent", "P", "p" * 64), Reference(w.G_ID, "G", "g" * 64)
            parent_admission = {"session_id": "P-session", "input_id": "P-input"}
            child = {"original": dataclasses.asdict(Reference("original-a", "A", "a" * 64)),
                     "admission": {"session_id": "A-session", "input_id": "A-input"}, "source_key": {"id": "a"}}
            launch = {"children": [child], "parent": {"admission": parent_admission}}
            scenario.workflow_context = (p, parent_admission, {"planning": dataclasses.asdict(g)}, {}, {}, {}, [])
            scenario.commission_attempt = SimpleNamespace(join_listing=AsyncMock())
            scenario.resource_python = "/selected/python"
            scenario.roster = SimpleNamespace(path=Path("/roster"), admitting=Path("/admitting"))
            build_context = {"model": "fixture-G-model", "labels": {"selected": "original"}, "system_prompt": None}
            created_context = {"identity": w.G_ID, "session_id": "G-session", "context": build_context}
            scenario.customizer = SimpleNamespace(shared_build_contexts=[build_context],
                wait_shared_created=AsyncMock(return_value=[created_context]))
            provider = OriginalResponseGateControls().model()
            provider.requests, provider.errors, provider.enabled = [], [], False
            provider.b_context = threading.Event()
            provider.original_gates["a"].update(approved=True, deadline=time.monotonic() + 30)
            provider.release_original_responses(tags=("a",))
            class Login:
                def result(self): events.append("login")
                def retire(self, *, timeout): events.append("retire-login"); return Retired()
            def cancel(*args, **kwargs): events.append("cancel-a"); return Cancelled()
            private = SimpleNamespace(begin_console_login=lambda *a, **k: Login(), cancel_original_operation=cancel)
            async def finalized(*args):
                events.append("finalized-a-g")
                if refusal: raise AssertionError("native context not finalized")
                return SimpleNamespace(session_id="G-session", delivery_scope="old-scope"), {}, {}, []
            scenario.wait_selected_a = finalized
            sdk = SimpleNamespace(ConsoleLoginSelection=lambda *a: None, PrivateOriginalRef=Reference)
            with patch.object(w.v, "SDK", sdk), patch.object(w.Scenario, "begin_workflow", AsyncMock(return_value=launch)), \
                    patch.object(w.s, "input_rows", return_value=[{"input_id": "P-input"}]), \
                    patch.object(w, "finalized_context", return_value={"phase": "finalized"}), \
                    patch.object(w.s, "wait_departed", new_callable=AsyncMock) as departed, \
                    patch.object(w.c, "signed_login", return_value="private-test-token"), \
                    patch.object(w.s, "wait_initial_a_completion", AsyncMock(return_value=[{"input_id": "A-input"}])), \
                    patch.object(w, "planning_lineage", return_value={"exact": "lineage"}), \
                    patch.object(w, "shared_peer", return_value={"owner_pid": 11}), \
                    patch.object(w.v, "artifact_pin", side_effect=lambda p: {"path": str(p), "sha256": "a" * 64}), \
                    patch.object(w.v, "save_json"):
                if refusal:
                    with self.assertRaisesRegex(AssertionError, "not finalized"):
                        await scenario.run(private, provider, Path("/case"), None, SimpleNamespace(pid=10), "origin")
                    self.assertNotIn("cancel-a", events)
                    self.assertEqual(scenario.cold["status"], "failed")
                    self.assertFalse(provider.original_gates["b"]["approved"])
                    self.assertTrue(provider.original_gates["b"]["released"].is_set())
                else:
                    result = await scenario.run(private, provider, Path("/case"), None, SimpleNamespace(pid=10), "origin")
                    self.assertEqual(result["status"], "prepared_for_joined_close")
                    self.assertFalse(provider.enabled or provider.original_gates["b"]["released"].is_set())
                    self.assertNotIn("departure", result)
                    self.assertEqual(events, ["login", "finalized-a-g", "retire-login"])
                    self.assertEqual(result["callbacks_before"], [created_context])
                    self.assertEqual(result["build_contexts_before"], [build_context])
                    build_context["model"] = "changed-after-close"
                    self.assertEqual(result["build_contexts_before"][0]["model"], "fixture-G-model")
                departed.assert_not_awaited()
                scenario.commission_attempt.join_listing.assert_awaited_once()
                self.assertEqual(events[-1], "retire-login")


class ColdSelectedPreparationControls(unittest.IsolatedAsyncioTestCase):
    def observations(self):
        key = {"realm": "mob.fixture", "owner": "owner", "id": "source-a"}
        acquisition = {**key, "id": "acquisition-a"}
        identity = {"source": key, "acquisition": acquisition, "helper": {"identity": w.G_ID},
                    "leaving": False, "left": False, "delivered": None}
        member = {"key": {**key, "id": "P"}, "member": "a", "identity": identity}
        checked = {"validation_id": "a" * 32, "effect_sha256": "b" * 64}
        job = {"id": "c" * 32, "kind": "install", "family": "selected-node", "status": "succeeded",
               "result": {"receipt_sha256": "d" * 64}, "error": None}
        completion = {"controller": "e" * 64, "validation_id": checked["validation_id"], "job": job}
        record = {"group": {"retained": "actual fixture group"}, "selection": {"controller": "e" * 64,
            "arguments_sha256": "f" * 64, "arguments": {"profile": "node"}, "effect": {}, "plan": {}, "result": checked},
            "completion": completion, "members": {"a": copy.deepcopy(identity)}, "closed": False}
        original = dataclasses.asdict(Reference("original-a", "A", "1" * 64))
        child = {"source_key": key, "original": original, "admission": {"session_id": "A-session", "input_id": "A-input"}}
        selected = {"checked": checked, "job": job, "context": {"membership": {"identity": {"acquisition": acquisition}}}}
        before = {"member": member, "record": record, "operation_cancelled": False}
        after = copy.deepcopy(before)
        after["operation_cancelled"] = after["record"]["closed"] = True
        after["record"]["members"]["a"].update(leaving=True, left=True)
        return child, selected, before, after

    def test_completion_fault_refuses_only_selected_publication_and_never_writes_evidence(self):
        child, selected, before, _ = self.observations()
        before["member"]["key"]["id"] = "P'quoted"
        pending = copy.deepcopy(before["record"])
        pending["completion"] = None
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
            path.parent.mkdir(parents=True)
            source = {"schema": "toolkit.authenticated-source-event", "body": {
                "shared_preparations": [before["member"]], "operation_cancelled": False}}
            unfinished = json.dumps({"schema": "meerkat_toolkit.shared_preparation", "body": pending})
            finished = json.dumps({"schema": "meerkat_toolkit.shared_preparation", "body": before["record"]})
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE toolkit_records(realm TEXT,owner TEXT,id TEXT,data TEXT)")
                db.executemany("INSERT INTO toolkit_records VALUES(?,?,?,?)", [
                    (*child["source_key"].values(), json.dumps(source)),
                    (*before["member"]["key"].values(), unfinished),
                    ("mob.fixture", "owner", "other-P", unfinished)])
            w.selected_completion_fault(root, child)
            with sqlite3.connect(path) as db:
                with self.assertRaisesRegex(sqlite3.IntegrityError, "acceptance P completion unavailable"):
                    db.execute("UPDATE toolkit_records SET data=? WHERE id=?", (finished, "P'quoted"))
                self.assertEqual(db.execute("SELECT data FROM toolkit_records WHERE id=?", ("P'quoted",)).fetchone()[0], unfinished)
                self.assertEqual(db.execute("SELECT data FROM toolkit_records WHERE id='source-a'").fetchone()[0], json.dumps(source))
                db.execute("UPDATE toolkit_records SET data=? WHERE id='other-P'", (finished,))
            self.assertIsNone(w.selected_preparation_record(root, child, selected, allow_pending=True)["record"]["completion"])
            with self.assertRaises(AssertionError):
                w.selected_preparation_record(root, child, selected)
            w.selected_completion_fault(root, child, release=True)
            with sqlite3.connect(path) as db:
                db.execute("UPDATE toolkit_records SET data=? WHERE id=?", (finished, "P'quoted"))
            self.assertEqual(w.selected_preparation_record(root, child, selected), before)

    async def test_publication_fault_waits_for_native_completion_then_exact_p_and_cleans_up_on_failure(self):
        child, selected, completed, _ = self.observations()
        pending = copy.deepcopy(completed)
        pending["record"]["completion"] = None
        review_mismatches = ("foreign-call", "wrong-tool", "missing-call", "missing-tool")
        for mode in ("complete", "native-pending", "publication-unavailable", "cleanup-drop-error", *review_mismatches):
            with self.subTest(mode=mode):
                events = []
                scenario = object.__new__(w.ColdSharedScenario)
                scenario.host_loop_failure = w.s.SelectedHostLoopFailure(w.G_ID, "prepared")
                scenario.withhold_p_completion, scenario.cold = True, {}
                scenario.port, scenario.origin = 1, "http://fixture"
                old = SimpleNamespace(session_id="G-session")
                observed = {"canonical": {"record": {"workflow": {"workflow": {"id": "workflow"}}}}}
                scenario.observe_shared = AsyncMock(return_value=(old, observed))
                check_args = {"profile": "node"}
                provider = SimpleNamespace(lock=threading.RLock(), errors=[], b_context=threading.Event(),
                    issued={"a-selected-check"}, results={"a-selected-check": selected["checked"]},
                    call_inputs={"a-selected-check": {"name": "setup_selected_check", "arguments": check_args}},
                    selected_result=None)
                review = {"review_id": "review", "can_start": False, "prompt": {"approval_id": "approval",
                    "body": {"kind": "service_software_install", "check_tool": "setup_selected_check",
                        "call_id": "a-selected-install", "tool": "setup_selected_install",
                        "scope": "software_installation_only", "selection": check_args, "owner_proposal": selected["checked"]}}}
                if mode == "foreign-call": review["prompt"]["body"]["call_id"] = "b-selected-install"
                if mode == "wrong-tool": review["prompt"]["body"]["tool"] = "setup_install"
                if mode == "missing-call": del review["prompt"]["body"]["call_id"]
                if mode == "missing-tool": del review["prompt"]["body"]["tool"]
                class Connection:
                    def __init__(self, *args, **kwargs): pass
                    def request(self, method, *args, **kwargs):
                        self.method = method
                        if method == "POST":
                            self_test.assertEqual(events, ["block"])
                            events.append("approve")
                            provider.selected_result = selected
                            provider.results["a-selected-install"] = {"job": selected["job"]}
                            provider.call_inputs["a-selected-install"] = {"name": "setup_selected_install", "arguments": {}}
                    def getresponse(self):
                        return SimpleNamespace(status=200, read=lambda _: w.v.canonical({"review": review}))
                    def close(self): pass
                self_test = self
                def fault(*_, release=False):
                    events.append("release" if release else "block")
                    if release and mode == "cleanup-drop-error":
                        raise RuntimeError("completion fault cleanup failed")
                def preparation(*_, **kwargs):
                    self.assertTrue(kwargs["allow_pending"])
                    if mode in ("publication-unavailable", "cleanup-drop-error"):
                        raise RuntimeError("canonical completion unavailable")
                    return copy.deepcopy(completed if "release" in events else pending)
                async def pause(_):
                    if mode == "native-pending" and provider.selected_result is not None:
                        raise RuntimeError("native input remains pending")
                actual = {"input_id": "G-context", "authorization_audit": [{"contributors": [{"input_id": "G-context"}]}]}
                with patch.object(w.s, "HTTPConnection", Connection), patch.object(w.c, "approval_answer", return_value={}), \
                        patch.object(w, "selected_completion_fault", side_effect=fault), \
                        patch.object(w, "context_delivery", return_value={"input": "G-context"}), \
                        patch.object(w.s, "input_rows", return_value=[actual]), \
                        patch.object(w, "finalized_context", return_value=None if mode == "native-pending" else actual), \
                        patch.object(w.s, "public_operation_audit", return_value={"exact_install": True}), \
                        patch.object(w, "selected_preparation_record", side_effect=preparation) as read, \
                        patch.object(w.asyncio, "sleep", side_effect=pause):
                    run = scenario.wait_selected_a(child, "member", provider, Path("/case"),
                        SimpleNamespace(poll=lambda: None), time.monotonic() + 30, "token")
                    if mode in review_mismatches:
                        with self.assertRaisesRegex(AssertionError, "protected selected A review differs"):
                            await run
                        read.assert_not_called()
                        self.assertIsNone(provider.selected_result)
                        self.assertNotIn("selected_a", scenario.cold)
                    elif mode == "complete":
                        result = await run
                        self.assertEqual(result[0], old)
                        self.assertEqual(scenario.cold["selected_p_publication"]["pending"], pending)
                        self.assertEqual(scenario.cold["selected_p_publication"]["completed"], completed)
                    else:
                        reason = "native input remains pending" if mode == "native-pending" else "canonical completion unavailable"
                        with self.assertRaisesRegex(RuntimeError, reason) as failed: await run
                        self.assertNotIn("selected_a", scenario.cold)
                        if mode == "native-pending": read.assert_not_called()
                        if mode == "cleanup-drop-error":
                            self.assertEqual(scenario.cold["selected_p_publication_cleanup_error"], {
                                "type": "RuntimeError", "message": "completion fault cleanup failed"})
                            self.assertEqual(failed.exception.__notes__, [
                                "P publication fault cleanup failed: completion fault cleanup failed"])
                if mode in review_mismatches:
                    self.assertEqual([row["method"] for row in scenario.cold["selected_a_http"]], ["GET"])
                    self.assertEqual(events, [])
                else:
                    self.assertEqual(scenario.cold["selected_a_http"][1]["method"], "POST")
                    self.assertEqual(events, ["block", "approve", "release"])

    def test_exact_nonempty_p_correlation_is_required_before_cancellation_can_be_an_oracle(self):
        child, selected, before, _ = self.observations()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
            path.parent.mkdir(parents=True)
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE toolkit_records(realm TEXT,owner TEXT,id TEXT,data TEXT)")
                def store(source, record):
                    db.execute("DELETE FROM toolkit_records")
                    for key, schema, body in ((child["source_key"], "toolkit.authenticated-source-event", source),
                        (before["member"]["key"], "meerkat_toolkit.shared_preparation", record)):
                        db.execute("INSERT INTO toolkit_records VALUES(?,?,?,?)", (*key.values(), json.dumps({"schema": schema, "body": body})))
                    db.commit()
                source = {"shared_preparations": [before["member"]], "operation_cancelled": False}
                store(source, before["record"])
                self.assertEqual(w.selected_preparation_record(root, child, selected), before)
                for change in (lambda source, row: source.update(shared_preparations=[]),
                    lambda source, row: source["shared_preparations"][0]["identity"].update(source={"id": "B"}),
                    lambda source, row: row["selection"]["result"].update(validation_id="0" * 32),
                    lambda source, row: row.update(completion=None),
                    lambda source, row: row["completion"]["job"].update(status="unconfirmed")):
                    bad_source, bad_record = copy.deepcopy((source, before["record"]))
                    change(bad_source, bad_record); store(bad_source, bad_record)
                    with self.subTest(change=change), self.assertRaises(AssertionError):
                        w.selected_preparation_record(root, child, selected)

    async def test_publication_fault_accepts_the_controllers_omitted_optional_error(self):
        observations = list(self.observations())
        # Break the fixture's shared job object: the wire reply omits error,
        # while the independently retained canonical completion keeps null.
        observations[1] = copy.deepcopy(observations[1])
        observations[1]["job"].pop("error")
        with patch.object(self, "observations", return_value=tuple(observations)):
            await self.test_publication_fault_waits_for_native_completion_then_exact_p_and_cleans_up_on_failure()

    async def test_publication_fault_refuses_non_null_error_even_with_matching_tool_job(self):
        for error in ("operation_unconfirmed", "", False, 0, {}):
            observations = list(self.observations())
            observations[1] = copy.deepcopy(observations[1])
            observations[1]["job"]["error"] = error
            # Exercise the existing coroutine on an unrun TestCase so its
            # nested subTest does not absorb the rejection asserted here.
            exercise = ColdSelectedPreparationControls()
            with self.subTest(error=error), patch.object(exercise, "observations", return_value=tuple(observations)), \
                    self.assertRaisesRegex(AssertionError, "publication recovery requires one actual joined installation"):
                await exercise.test_publication_fault_waits_for_native_completion_then_exact_p_and_cleans_up_on_failure()

    async def test_restored_cleanup_requires_one_owner_reply_before_departure_and_never_retries_refusal(self):
        child, selected, before, after = self.observations()
        @dataclasses.dataclass
        class Reply:
            original: dict
            state: str = "operations_cancelled"
        for failure in (None, RuntimeError("native physical leave unavailable"), "operations_cancelled_cleanup_pending"):
            with self.subTest(failure=failure):
                scenario = object.__new__(w.ColdSharedScenario)
                scenario.phase = "restored"
                scenario.host_loop_failure = w.s.SelectedHostLoopFailure(w.G_ID, scenario.phase)
                scenario.first_launch = {"children": [child]}
                launch = {"children": [child, {"original": {"route": "original-b"}, "source_key": {"id": "source-b"}}]}
                scenario.cold = {"selected_a": {**selected, "preparation": before}}
                calls = []
                def cancel(original, *, timeout):
                    calls.append((original, timeout))
                    if isinstance(failure, Exception): raise failure
                    return Reply(child["original"], failure or "operations_cancelled")
                private = SimpleNamespace(cancel_original_operation=cancel)
                with patch.object(w.v, "SDK", SimpleNamespace(PrivateOriginalRef=Reference)), \
                        patch.object(w, "selected_preparation_record", side_effect=[before, after]), \
                        patch.object(w.s, "wait_departed", new_callable=AsyncMock) as departed, \
                        patch.object(w.s, "wait_initial_a_completion", AsyncMock(return_value=["exact-A-input"])):
                    if failure is None:
                        result = await scenario.cancel_a(private, Path("/case"), launch, 30)
                        self.assertEqual(result["a_before"], ["exact-A-input"])
                        self.assertEqual(scenario.cold["selected_p_cleanup"]["after"], after)
                        departed.assert_awaited_once_with(Path("/case"), child["source_key"], 30,
                            remaining_source=launch["children"][1]["source_key"],
                            failure_observer=scenario.host_loop_failure)
                    else:
                        with self.assertRaises((RuntimeError, AssertionError)):
                            await scenario.cancel_a(private, Path("/case"), launch, 30)
                        departed.assert_not_awaited()
                        self.assertNotIn("selected_p_cleanup", scenario.cold)
                self.assertEqual(calls, [(Reference(**child["original"]), 30)])
        reply = {"original": child["original"], "state": "operations_cancelled"}
        w.require_selected_p_leave(before, after, reply, child["original"])
        for change in (lambda row: row["record"]["completion"]["job"].update(id="0" * 32),
                       lambda row: row["record"]["members"]["a"].update(left=False),
                       lambda row: row["record"]["selection"]["result"].update(effect_sha256="0" * 64)):
            changed = copy.deepcopy(after); change(changed)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                w.require_selected_p_leave(before, changed, reply, child["original"])

    async def test_cold_run_joins_b_before_cancelling_a_and_keeps_effect_gate_closed_until_cleanup(self):
        child, selected, before, after = self.observations()
        b = {"original": {"route": "original-b"}, "source_key": {"id": "source-b"}}
        launch = {"children": [child, b]}
        @dataclasses.dataclass
        class Reply:
            original: dict
            state: str = "operations_cancelled"
        @dataclasses.dataclass
        class Retired:
            fenced: bool = True
            reservations_drained: bool = True
            uncertain: bool = False
        for refused in (False, True):
            with self.subTest(refused=refused):
                events, reports = [], {}
                scenario = object.__new__(w.ColdSharedScenario)
                scenario.phase, scenario.origin, scenario.port = "restored", "http://fixture", 1
                scenario.host_loop_failure = w.s.SelectedHostLoopFailure(w.G_ID, scenario.phase)
                scenario.first_launch = {"children": [child], "parent": {"admission": {"session_id": "P-session"}}}
                p, g = Reference("planning-parent", "P", "p" * 64), Reference(w.G_ID, "G", "g" * 64)
                scenario.workflow_context = (p, None, {"planning": dataclasses.asdict(g)}, None, "member", None, None)
                scenario.resource_python, scenario.runtime = "/python", object()
                scenario.commission_attempt = None
                scenario.cold = {"first_pid": 1, "before": {}, "before_inputs": [], "parent_inputs": [],
                    "lineage": {}, "peer_before": {}, "selected_a": {**selected, "preparation": before}}
                scenario.observe_shared = AsyncMock(return_value=(None, {"status": {"session_id": "G-session"}}))
                context = {"model": "selected-model", "labels": {}, "system_prompt": "exact prompt"}
                created = [{"identity": w.G_ID, "session_id": "G-session", "context": copy.deepcopy(context)}]
                scenario.cold.update(callbacks_before=copy.deepcopy(created), build_contexts_before=[copy.deepcopy(context)])
                scenario.customizer = SimpleNamespace(builds=[{"identity": w.G_ID, "profile": w.CLASS_ID}] * 2,
                    created=[], shared_build_contexts=[copy.deepcopy(context), copy.deepcopy(context)],
                    shared_created_contexts=copy.deepcopy(created),
                    wait_shared_created=AsyncMock(side_effect=AssertionError("cold Restore cannot create another session")))
                provider = SimpleNamespace(b_context=threading.Event(), release_b=threading.Event(), abort_b=threading.Event(),
                    lock=threading.RLock(), errors=[], results={}, issued=set(), business_observation=None,
                    release_original_responses=lambda **_: events.append("abort-original-responses"))
                scenario.begin_model_phase = lambda _: None
                async def begin(*_):
                    self.assertNotIn("cancel-a", events)
                    events.append("B-context")
                    provider.b_context.set()
                    return launch
                scenario.begin_workflow = begin
                def cancel(original, *, timeout):
                    self.assertTrue(provider.b_context.is_set())
                    self.assertFalse(provider.release_b.is_set())
                    self.assertEqual(original, Reference(**child["original"]))
                    self.assertEqual(timeout, 30)
                    events.append("cancel-a")
                    if refused:
                        raise RuntimeError("physical P leave refused")
                    return Reply(child["original"])
                async def departed(*_, **kwargs):
                    self.assertFalse(provider.release_b.is_set())
                    self.assertEqual(kwargs, {"remaining_source": b["source_key"],
                        "failure_observer": scenario.host_loop_failure})
                    events.append("P-leave-and-A-departure")
                    return {"sealed": False, "exact_active_B": b["source_key"]}
                class Login:
                    def result(self): return None
                    def retire(self, *, timeout): events.append("retire-login"); return Retired()
                private = SimpleNamespace(begin_console_login=lambda *a, **k: Login(), cancel_original_operation=cancel)
                responses = []
                def response():
                    responses.append(None)
                    if len(responses) > 1:
                        self.assertTrue(provider.release_b.is_set())
                        self.assertFalse(provider.abort_b.is_set())
                        events.append("B-released-after-cleanup")
                        raise RuntimeError("stop after verified effect gate")
                    return SimpleNamespace(status=200, read=lambda _: b'{"review":null}')
                connection = SimpleNamespace(request=lambda *a, **k: None, getresponse=response, close=lambda: None)
                sdk = SimpleNamespace(PrivateOriginalRef=Reference, ConsoleLoginSelection=lambda *a: None)
                with patch.object(w.v, "SDK", sdk), patch.object(w, "require_cold_g_transition"), \
                        patch.object(w, "planning_lineage", return_value={}), patch.object(w.s, "input_rows", return_value=[]), \
                        patch.object(w, "require_g_stale_scope", AsyncMock(side_effect=AssertionError("retained cold scope is current"))), \
                        patch.object(w, "shared_peer", return_value={}), patch.object(w, "require_cold_transport", return_value={}), \
                        patch.object(w, "selected_preparation_record", side_effect=[before, after]), \
                        patch.object(w.s, "wait_departed", side_effect=departed), \
                        patch.object(w.s, "wait_initial_a_completion", AsyncMock(return_value=["exact-A-input"])), \
                        patch.object(w.s, "HTTPConnection", return_value=connection), \
                        patch.object(w.c, "signed_login", return_value="fixture-token"), \
                        patch.object(w.v, "save_json", side_effect=lambda path, value: reports.update({path.name: copy.deepcopy(value)})):
                    with self.assertRaisesRegex(RuntimeError, "physical P leave refused" if refused else "stop after verified effect gate"):
                        await scenario.run(private, provider, Path("/case"), None,
                            SimpleNamespace(pid=2, poll=lambda: None), scenario.origin)
                scenario.customizer.wait_shared_created.assert_not_awaited()
                self.assertEqual(scenario.cold["callbacks_after"], created)
                self.assertEqual(events.count("cancel-a"), 1)
                self.assertEqual(events[:2], ["B-context", "cancel-a"])
                self.assertIn("retire-login", events)
                self.assertTrue(provider.abort_b.is_set() and provider.release_b.is_set())
                self.assertEqual(reports["console-journey.json"]["status"], "failed")
                self.assertEqual(reports["shared-cold-restored.json"]["status"], "failed")
                expected_error = "physical P leave refused" if refused else "stop after verified effect gate"
                self.assertIn(expected_error, reports["console-journey.json"]["error"])
                self.assertIn(expected_error, reports["shared-cold-restored.json"]["error"])
                if refused:
                    self.assertNotIn("P-leave-and-A-departure", events)
                    self.assertNotIn("B-released-after-cleanup", events)
                    self.assertNotIn("selected_p_cleanup", scenario.cold)
                else:
                    self.assertEqual(events[2:4], ["P-leave-and-A-departure", "B-released-after-cleanup"])
                    self.assertEqual(scenario.cold["selected_p_cleanup"]["after"], after)

    async def test_departure_readback_requires_exact_live_b_and_never_accepts_sealed_workflow(self):
        child, _, _, _ = self.observations()
        b = {**child["source_key"], "id": "source-b"}
        body = {"sealed": False, "waiters": {
            "A": {"identity": {"source": child["source_key"]}, "departed": True},
            "B": {"identity": {"source": b}, "departed": False}}}
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
            path.parent.mkdir(parents=True)
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE toolkit_records(realm TEXT,owner TEXT,id TEXT,data TEXT)")
                def store(value):
                    db.execute("DELETE FROM toolkit_records")
                    db.execute("INSERT INTO toolkit_records VALUES(?,?,?,?)", ("mob.fixture", "owner", "G",
                        json.dumps({"schema": "meerkat_toolkit.setup_workflow", "body": value})))
                    db.commit()
                store(body)
                observed = await w.s.wait_departed(root, child["source_key"], time.monotonic() + 5, remaining_source=b)
                self.assertEqual(observed, body)
                for change in (lambda row: row.update(sealed=True),
                               lambda row: row["waiters"]["B"].update(departed=True),
                               lambda row: row["waiters"]["B"]["identity"]["source"].update(id="foreign")):
                    changed = copy.deepcopy(body); change(changed); store(changed)
                    with self.subTest(change=change), self.assertRaisesRegex(AssertionError, "exact active B"):
                        await w.s.wait_departed(root, child["source_key"], time.monotonic() + 5, remaining_source=b)

    async def test_failure_cleanup_releases_held_b_response_without_issuing_setup_tools(self):
        model = OriginalResponseGateControls().model()
        model.b_context, model.release_b, model.abort_b = (threading.Event() for _ in range(3))
        context = {"purpose": "toolkit-workflow-inline-context-v1", "recipient": {"original": {"route": "original-b"}}}
        body = {"messages": [{"role": "user", "content": w.s.G_TEXT}, {"role": "user", "content": json.dumps(context)}],
                "tools": [{"function": {"name": name}} for name in w.s.SETUP]}
        failures = []
        def response():
            try:
                with model.lock:
                    model.response(body)
            except BaseException as error:
                failures.append(error)
        worker = threading.Thread(target=response)
        worker.start()
        try:
            self.assertTrue(await asyncio.to_thread(model.b_context.wait, 5))
            self.assertEqual(model.issued, set())
            model.abort_b.set()
            model.release_b.set()
            await asyncio.to_thread(worker.join, 5)
            self.assertFalse(worker.is_alive())
            self.assertEqual(len(failures), 1)
            self.assertRegex(str(failures[0]), "B model release was aborted")
            self.assertEqual(model.issued, set())
        finally:
            model.abort_b.set()
            model.release_b.set()
            await asyncio.to_thread(worker.join, 5)
            self.assertFalse(worker.is_alive())

    def test_no_build_fixture_stages_real_archive_without_toolchain_or_package_execution(self):
        import tarfile
        root = Path(w.__file__).parent / "fixtures/shared-selected-mcp"
        fixture = w.s.load("cold_selected_fixture_control", root / "setup_fixture.py")
        inspected = {"status": "accepted", "source_digest": fixture.sha(fixture.wire(fixture.package_document())),
                     "summary": {"package_content_digest": "f" * 64}}
        with tempfile.TemporaryDirectory() as directory:
            selected = fixture.stage_and_check_request(root, str(Path(directory).resolve()), inspected)
            self.assertEqual(set(selected), {"profile", "staged_id", "component", "package_json", "declaration_json"})
            declaration = json.loads(selected["declaration_json"])
            self.assertEqual(declaration["exports"][0]["authentication"], {"kind": "none"})
            self.assertEqual(set(declaration["components"][0]["source"]["dependencies"]["artifacts"]), {"package-json", "package-lock"})
            with tarfile.open(Path(directory) / fixture.COMPONENT / "artifact.tgz") as archive:
                self.assertEqual(set(archive.getnames()), {"package/" + name for name in fixture.PACKAGE_FILES})
                package = json.load(archive.extractfile("package/package.json"))
                self.assertNotIn("scripts", package)
                self.assertNotIn("dependencies", package)
                server = archive.extractfile("package/server.cjs").read()
                self.assertIn(b"const sum = args.left + args.right;", server)
                self.assertNotIn(b"addon.node", server)
            with self.assertRaises(FileExistsError):
                fixture.stage_and_check_request(root, str(Path(directory).resolve()), inspected)

    def test_selected_model_uses_current_a_membership_and_exact_check_for_one_install(self):
        model = OriginalResponseGateControls().model()
        context = {"purpose": "toolkit-workflow-inline-context-v1", "recipient": {"original": {"route": "original-a"}},
                   "membership": {"identity": {"acquisition": {"id": "actual-A-acquisition"}}}}
        body = {"messages": [{"role": "user", "content": w.s.G_TEXT},
                             {"role": "user", "content": json.dumps(context)}],
                "tools": [{"function": {"name": name}} for name in w.SELECTED_A_TOOLS]}
        with model.lock:
            self.assertEqual(model.response(body)["delta"]["content"], "A context received; no service effect requested for A.")
        model.selected_a = True
        model.selected_fixture = SimpleNamespace(COMPONENT="shared-selected-mcp")
        model.selected_request = {"profile": "node", "staged_id": "shared-selected-mcp", "component": "shared-selected-mcp",
                                  "package_json": "exact authored package", "declaration_json": "exact authored declaration"}
        model.issued.update(("a-selected-state", "a-selected-package"))
        body["messages"] += [{"role": "tool", "tool_call_id": "a-selected-state", "content": json.dumps({
            "selected_installation": {"profiles": ["node"], "staging_root": "/actual/staging"}})},
            {"role": "tool", "tool_call_id": "a-selected-package", "content": "{}"}]
        def call():
            with model.lock:
                value = model.response(body)
            row = value["delta"]["tool_calls"][0]
            return row["function"]["name"], json.loads(row["function"]["arguments"])
        self.assertEqual(call(), ("setup_selected_check", {**model.selected_request, "setup_acquisition": "actual-A-acquisition"}))
        checked = {"scope": "selected_software_inputs_only", "profile": "node", "component": "shared-selected-mcp",
            "mcp_name": "shared-selected-mcp", "validation_id": "a" * 32, "effect_sha256": "b" * 64,
            "declared_service": {"authentication": {"kind": "none"}, "operation_tools": {"add": "selected_add"}}}
        body["messages"].append({"role": "tool", "tool_call_id": "a-selected-check", "content": json.dumps(checked)})
        self.assertEqual(call(), ("setup_selected_install", {"validation_id": "a" * 32, "effect_sha256": "b" * 64,
                                                           "setup_acquisition": "actual-A-acquisition"}))
        self.assertNotIn("g-public-access", model.issued)
        with self.assertRaisesRegex(AssertionError, "duplicate scripted operation"):
            call()
        installed = {"scope": "retained_payload_and_launch_binding_only", "component": checked["component"],
            "effect_sha256": checked["effect_sha256"], "declared_service": checked["declared_service"],
            "receipt_sha256": "c" * 64, "mcp_configuration": {"selected": "actual retained launch"}}
        job = {"id": "d" * 32, "status": "succeeded", "result": {**installed,
            "scope": "observed_software_installation_only", "readback_scope": installed["scope"]}}
        body["messages"].append({"role": "tool", "tool_call_id": "a-selected-install", "content": json.dumps({"job": job})})
        self.assertEqual(call(), ("setup_reconcile", {"job_id": job["id"], "setup_acquisition": "actual-A-acquisition"}))
        observation = {"job_id": job["id"], "status": "matched", "result": installed}
        body["messages"].append({"role": "tool", "tool_call_id": "a-selected-reconcile", "content": json.dumps({"observation": observation})})
        with model.lock:
            self.assertEqual(model.response(body)["delta"]["content"], w.SELECTED_A_ANSWER)
        self.assertEqual(model.selected_result["job"], job)
        self.assertNotIn("installer_reply", job["result"])
        for change in (lambda row: row.update(scope="observed_software_installation_only"),
                       lambda row: row.update(receipt_sha256="e" * 64),
                       lambda row: row.update(mcp_configuration={"selected": "different launch"})):
            changed = copy.deepcopy(observation); change(changed["result"])
            body["messages"][-1]["content"] = json.dumps({"observation": changed})
            # Each negative supplies its first reconcile observation, so it
            # reaches the installer/readback check rather than replay drift.
            model.results.pop("a-selected-reconcile", None)
            with self.subTest(change=change), model.lock, self.assertRaisesRegex(AssertionError, "actual installer/readback"):
                model.response(body)


class HelperlessRestoreControls(unittest.IsolatedAsyncioTestCase):
    def observed_runtime(self, scenario, callback=None, **attributes):
        """Inert transport registration, using the real local observer wrapper."""
        class Transport:
            _callback_handler = None

            def set_callback_handler(self, handler):
                self._callback_handler = handler

        if callback is None:
            callback = AsyncMock(side_effect=AssertionError("this control must not dispatch callbacks"))
        if not hasattr(scenario, "callback_observations"):
            scenario.callback_observations = {}
            scenario.callback_dispatchers = []
            scenario.callback_lock = threading.Lock()
        dispatcher = SimpleNamespace(handle_callback=callback)
        runtime = SimpleNamespace(_dispatcher=dispatcher, _transport=Transport(), **attributes)
        scenario.observe_runtime(runtime)
        # Match _bootstrap's registration order without starting any SDK transport.
        runtime._transport.set_callback_handler(dispatcher.handle_callback)
        self.assertIs(scenario.callback_dispatchers[-1][1], callback)
        self.assertIs(runtime._transport._callback_handler, scenario.callback_dispatchers[-1][2])
        self.assertIs(runtime._transport._callback_handler, dispatcher.handle_callback)
        return runtime

    def plan(self, directory):
        scenario = w.HelperlessScenario(w.v, "http://127.0.0.1:12345")
        plan, plan_pin, source_pin = w.v.installed_plan(directory, [], acquisition=True, console=scenario)
        plan, plan_pin, source_pin = scenario.prepare_plan(directory, plan, plan_pin, source_pin)
        return scenario, plan, plan_pin, source_pin

    def test_selected_plan_is_empty_without_helper_or_authority_projection(self):
        with tempfile.TemporaryDirectory() as temp:
            _, plan, selected, source = self.plan(Path(temp).resolve())
            app = plan["application"]
            self.assertEqual(app["roster"], [])
            self.assertEqual(app["members"], [])
            self.assertEqual(app["tool_policy"]["members"], [])
            self.assertEqual(plan["owner"]["native_host"]["members"], [])
            self.assertEqual(plan["owner"]["native_host"]["routes"], [])
            self.assertEqual(plan["authorities"], {})
            self.assertNotIn("acquisition", plan)
            for key in ("helper", "helper_tools", "helper_profile"):
                self.assertNotIn(key, app)
            self.assertEqual([row["id"] for row in app["member_classes"]], [w.C3_CLASS])
            self.assertEqual(set(w.tomllib.loads(app["definition"]["text"])["profiles"]), {w.C3_CLASS})
            self.assertEqual(json.loads(w.v.exact_bytes(Path(selected["path"]), selected["sha256"])), plan)
            raw = w.v.exact_bytes(Path(source["path"]), source["sha256"])
            self.assertEqual(json.loads(raw)["roster"], [])
            self.assertEqual(app["tool_policy"]["source"]["source_digest"], w.v.installed_fixture_policy_digest(raw))

    async def test_ordinary_add_request_uses_actual_installed_principal_selection(self):
        from meerkat_meta import identity_first_models
        from meerkat_toolkit.member_classes import MemberClassAddRequest
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp).resolve()
            scenario, plan, _, _ = self.plan(directory)
            native = plan["owner"]["native_host"]
            before = copy.deepcopy(scenario.ordinary_route)
            scenario.ordinary_route["origin"]["requester"] = w.v.installed_fixture_principal("foreign-requester")
            scenario.ordinary_route["origin"]["ingress_actor"] = w.v.installed_fixture_principal("foreign-ingress")
            scenario.ordinary_route["source_authority"] = w.v.installed_fixture_principal("foreign-source")
            scenario.ordinary_route["represented_subject"] = None
            scenario.ordinary_route["authentication_domain"]["namespace"] = "foreign-authentication"
            scenario.ordinary_route["event_domain"]["namespace"] = "foreign-events"
            captured = []
            def begin(request, **_):
                captured.append(request.wire())
                raise RuntimeError("stop before native Add")
            self.observed_runtime(scenario, member_classes=SimpleNamespace(begin_add=begin))
            scenario.member_types = SimpleNamespace(MemberClassAddRequest=MemberClassAddRequest)
            scenario.roster = w.DesiredRoster(directory, [], helperless=True)
            scenario.customizer = w.AcceptanceCustomizer("selected-profile-model")
            scenario.c3 = {"status": "not_started", "scope": scenario.scope}
            with patch.object(w.v, "META_IDENTITY", identity_first_models), \
                 self.assertRaisesRegex(RuntimeError, "stop before native Add"):
                await scenario.check_ordinary_rebuild(None, None, directory, time.monotonic() + 60)
            self.assertEqual(len(captured), 1)
            request = captured[0]
            route, member = request["route"], request["instance"]["member"]
            executor = {"kind": "shared_agent", "id": w.C3_ID,
                        "qualification": native["ingress_domain"]["authority"]["qualification"]}
            self.assertEqual(route["logical_executor"], executor)
            self.assertEqual(member["logical_owner"], executor)
            self.assertEqual(route["represented_subject"], native["remote_operator"])
            self.assertEqual(route["origin"]["requester"], native["remote_operator"])
            self.assertEqual(route["origin"]["ingress_actor"], native["ingress_domain"]["authority"])
            self.assertEqual(route["source_authority"], native["ingress_domain"]["authority"])
            self.assertEqual(member["audience"], {"audience": "principal", "principal": native["remote_operator"]})
            self.assertEqual(route["authentication_domain"], native["authentication_domain"])
            self.assertEqual(route["event_domain"], native["event_domain"])
            self.assertEqual(native["authentication_domain"], before["authentication_domain"])
            self.assertEqual(native["event_domain"], before["event_domain"])
            self.assertNotIn("application_parent", route)
            self.assertNotIn("controller_parent", route)
            self.assertEqual(scenario.c3["status"], "failed")
            self.assertEqual(native["routes"], [], "this request control does not add a seed or prove native authorization")
            scenario.callback_dispatchers[-1][1].assert_not_awaited()

    async def test_helperless_finalizer_selects_schema2_and_never_invents_resource_handoff(self):
        @dataclasses.dataclass
        class Selected:
            path: str
            sha256: str
        @dataclasses.dataclass
        class Bootstrap:
            deployment_root: str
            activation: Selected
            policy_manifest: Selected
            local_policy: Selected
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp).resolve()
            _, plan, plan_pin, source_pin = self.plan(directory)
            output = Path(plan["output"]); output.mkdir()
            policy = w.v.new_artifact(output / "application-tool-policy.json", b'{"native_policy":"fixture"}\n')
            setup = {"schema": 4, "realm": plan["application"]["realm"],
                "definition_source": plan["application"]["definition"], "native_owner": plan["owner"],
                "native_members": [], "artifact_directory": str(output),
                "application_tool_policy": {"name": "application-tool-policy.json", "sha256": policy["sha256"]}}
            setup_pin = w.v.new_artifact(output / "application-setup.json", w.v.canonical(setup))
            manifest = w.v.new_artifact(output / "manifest.json", w.v.canonical({"schema": 1,
                "kind": "application_setup_preparation", "artifacts": [
                    {"path": Path(pin["path"]).name, "sha256": pin["sha256"]} for pin in (policy, setup_pin)]}))
            final_dir = directory / "finalization"; final_dir.mkdir()
            events = w.v.EventLog(directory)
            finalizer = w.v.InstalledFixtureFinalizer(Path("/fixture/bin/toolkit_gateway"), final_dir,
                plan, plan_pin, source_pin, {}, events, lambda: None)
            commands = []
            async def command(name, argv):
                # No subprocess/native behavior is mocked as acceptance here:
                # this control examines the finalizer's exact selected carrier.
                commands.append(name)
                if name == "prepare-policy":
                    carrier = json.loads((final_dir / "policy-inputs/native-setup.json").read_bytes())
                    self.assertEqual(carrier, {"schema": 2, "prepared_application_setup": {
                        "name": "application-setup.json", "sha256": setup_pin["sha256"]}})
                    root = Path(plan["deployment_root"])
                    bundle = root / "policy-bundle"; bundle.mkdir()
                    result = {"deployment_root": str(root),
                        "activation": w.v.new_artifact(root / "activation.json", b'{"activation":1}\n'),
                        "policy_manifest": w.v.new_artifact(bundle / "manifest.json", b'{"manifest":1}\n'),
                        "local_policy": w.v.new_artifact(bundle / "toolkit-local-policy.json", b'{"policy":1}\n')}
                    self.selection = result
                    return {"kind": "application_policy_prepared", "selection": result}
                self.assertEqual(name, "validate-policy")
                return {"kind": "application_policy_validated", "selection": self.selection}
            finalizer.command = command
            with patch.object(w.v, "SDK", SimpleNamespace(SelectedHostArtifact=Selected,
                              HostBootstrapSelection=Bootstrap), create=True):
                selected = await finalizer(Selected(**manifest))
            self.assertEqual(commands, ["prepare-policy", "validate-policy"])
            self.assertEqual(dataclasses.asdict(selected), finalizer.selection)
            self.assertNotIn("preparer_handoff", finalizer.evidence)
            self.assertNotIn("controller_authority", finalizer.evidence)
            self.assertEqual([row["event"] for row in events.rows], ["fixture_publish_entered", "fixture_published"])
            for change in (lambda x: x.update(helper={}), lambda x: x.update(resources={}),
                           lambda x: x.update(native_members=[{"member": "hidden-helper"}]),
                           lambda x: x.update(artifact_directory="/foreign/output"),
                           lambda x: x["native_owner"]["native_host"].update(member_classes=[])):
                changed = copy.deepcopy(setup); change(changed)
                with self.assertRaises(AssertionError):
                    w.v.require_helperless_completed(changed, plan["application"], output)

    async def test_zero_start_refuses_an_initial_native_member_before_add_and_joins_login(self):
        @dataclasses.dataclass
        class Retirement:
            fenced: bool = True
            reservations_drained: bool = True
            uncertain: bool = False
        with tempfile.TemporaryDirectory() as temp:
            directory = Path(temp).resolve()
            scenario = w.HelperlessScenario(w.v, "http://127.0.0.1:12345")
            retired = []
            enrollment = SimpleNamespace(result=lambda: None, retire=lambda **kw: (retired.append(True), Retirement())[1])
            private = SimpleNamespace(begin_console_login=lambda *a, **kw: enrollment)
            graph = SimpleNamespace(list_members=AsyncMock(return_value=[{"identity": "hidden-helper"}]))
            self.observed_runtime(scenario, host_control=private, mob_handle=lambda: graph)
            scenario.roster = SimpleNamespace(selected=lambda: [])
            scenario.customizer = SimpleNamespace(builds=[], created=[])
            scenario.probe_attempt = None
            scenario.check_ordinary_rebuild = AsyncMock()
            provider = SimpleNamespace(lock=threading.RLock(), requests=[], enabled=False)
            with patch.object(w.v, "SDK", SimpleNamespace(ConsoleLoginSelection=lambda *a: a), create=True):
                with self.assertRaisesRegex(AssertionError, "zero-start Ready"):
                    await scenario.run(private, provider, directory, None,
                        SimpleNamespace(pid=10, args=["/fixture/bin/toolkit_gateway"]), scenario.origin)
            scenario.check_ordinary_rebuild.assert_not_awaited()
            self.assertFalse(provider.enabled)
            self.assertEqual(retired, [True])
            self.assertEqual(json.loads((directory / "helperless-prepared.json").read_bytes())["status"], "failed")
            scenario.callback_dispatchers[-1][1].assert_not_awaited()

    async def test_restore_uses_existing_admission_and_refuses_changed_session_before_new_input(self):
        @dataclasses.dataclass
        class Retirement:
            fenced: bool = True
            reservations_drained: bool = True
            uncertain: bool = False
        identity_failures = {"session", "runtime", "scope", "owner"}
        for changed in (None, "session", "runtime", "scope", "owner", "missing_build", "extra_build",
                        "foreign_build", "draft", "creation", "nonce"):
            with self.subTest(changed=changed), tempfile.TemporaryDirectory() as temp:
                directory = Path(temp).resolve()
                scenario = w.HelperlessScenario(w.v, "http://127.0.0.1:12345")
                scenario.first_pid = 10
                scenario.ordinary_route = {"origin": {"member": {"member": w.C3_ID}}}
                pin = w.v.new_artifact(directory / "retained.json", b'{"retained":true}\n')
                scenario.retained_listing = scenario.retained_mcp = scenario.completed_pin = pin
                scenario.retained_inputs = [{"input_id": "before"}]
                before, _, _ = OrdinaryRebuildOracleControls().observations()
                before["tools"]["tools_source"] = "recorded"
                before["status"]["delivery_scope"] = {"version": 1, "identity": w.C3_ID,
                    "agent_runtime_id": "runtime-before", "generation": 4, "lease_fencing_token": 1,
                    "member": {"version": 2, "mob_incarnation": "same-mob",
                        "runtime_id": {"identity": w.C3_ID, "generation": 4},
                        "session_id": "session", "fence_token": 1}}
                after = copy.deepcopy(before)
                if changed == "session": after["status"]["session_id"] = "foreign"
                if changed == "runtime": after["status"]["agent_runtime_id"] = "runtime-after"
                if changed == "scope": after["status"]["delivery_scope"]["member"]["fence_token"] = 2
                if changed == "owner": after["canonical"]["record"]["route"] = {"id": "foreign"}
                build = {"identity": w.C3_ID, "profile": w.C3_CLASS}
                context = {"model": "fixture-original-model", "labels": {}, "system_prompt": None}
                created = {"identity": w.C3_ID, "session_id": "session"}
                created_context = {**created, "context": context}
                scenario.c3 = {"before": before, "customizer": {"build_contexts": [copy.deepcopy(context)]},
                    "after_create_before": [copy.deepcopy(created)],
                    "after_create_contexts_before": [copy.deepcopy(created_context)]}
                scenario.old_scope = object()
                scenario.roster = SimpleNamespace(probe_publications=1, publications=0)
                scenario.customizer = w.AcceptanceCustomizer("fixture-original-model")
                scenario.customizer.wait_created = AsyncMock(
                    side_effect=AssertionError("cold Restore must not await another creation"))
                scenario.probe_attempt = SimpleNamespace(join_listing=AsyncMock())
                scenario.check_ordinary_rebuild = AsyncMock(side_effect=AssertionError("Restore must not Add"))
                retired = []
                enrollment = SimpleNamespace(result=lambda: None, retire=lambda **kw: (retired.append(True), Retirement())[1])
                private = SimpleNamespace(begin_console_login=lambda *a, **kw: enrollment)
                before_history = [{"tool_call_id": "c3-before-call"}]
                provider = SimpleNamespace(lock=threading.RLock(), requests=[], errors=[],
                    issued={"c3-before-call", "c3-after-call"},
                    c3_histories={"before": before_history, "after": before_history + [
                        {"content": "Ordinary callback returned before"}, {"tool_call_id": "c3-after-call"}]})

                def runtime_fixture():
                    # No SDK/runtime execution. The dispatcher forwards the observed
                    # synthetic callbacks to the real local customizer and tool closure.
                    handlers, forwarded = {}, []

                    async def callback(method, params):
                        forwarded.append((method, params))
                        if method == "callback/agent_customizer/customize_build":
                            def register_tool(name, handler, **declaration):
                                self.assertEqual(name, w.C3_TOOL)
                                self.assertEqual(declaration, {"description": w.C3_DESCRIPTION,
                                                              "input_schema": w.C3_SCHEMA})
                                handlers[(params["scope_id"], name)] = handler
                            draft = SimpleNamespace(**copy.deepcopy(params["draft"]), external_tools=[],
                                                    register_tool=register_tool)
                            await scenario.customizer.customize_build(SimpleNamespace(**params["context"]),
                                SimpleNamespace(**params["spec"]), draft)
                            return None
                        if method == "callback/agent_customizer/after_create":
                            return await scenario.customizer.after_create(params["identity"], params["session_id"],
                                SimpleNamespace(**params["context"]))
                        self.assertEqual(method, "callback/call_tool")
                        return {"content": await handlers[(params["scope_id"], params["tool"])](params["arguments"])}

                    runtime = self.observed_runtime(scenario, callback, host_control=private)

                    async def dispatch(method, params):
                        count = len(forwarded)
                        self.assertIs(runtime._transport._callback_handler, runtime._dispatcher.handle_callback)
                        result = await runtime._transport._callback_handler(method, params)
                        self.assertEqual(len(forwarded), count + 1)
                        self.assertEqual(forwarded[-1][0], method)
                        self.assertIs(forwarded[-1][1], params)
                        return result
                    return dispatch, handlers, forwarded

                build_params = {"scope_id": "fixture-scope", "context": {"identity": w.C3_ID,
                    "active_peers": [], "managed_edges": []}, "spec": build, "draft": context}
                create_params = {**created, "context": context}
                call_params = {"scope_id": "fixture-scope", "tool": w.C3_TOOL, "arguments": {"nonce": "before"}}
                dispatch, prepared_handlers, prepared_forwarded = runtime_fixture()
                await dispatch("callback/agent_customizer/customize_build", copy.deepcopy(build_params))
                await dispatch("callback/agent_customizer/after_create", copy.deepcopy(create_params))
                self.assertEqual(await dispatch("callback/call_tool", copy.deepcopy(call_params)),
                                 {"content": {"nonce": "before", "handler_build": 1}})
                scenario.prepared_callbacks = scenario.callback_snapshot(provider)
                self.assertEqual(scenario.prepared_callbacks["builds"], [build])
                self.assertEqual(scenario.prepared_callbacks["created"], [created])
                self.assertEqual(scenario.prepared_callbacks["calls"], [{"nonce": "before", "handler_build": 1}])
                self.assertEqual(len(prepared_forwarded), 3)
                scenario.phase = "restored"
                dispatch, restored_handlers, restored_forwarded = runtime_fixture()

                async def governed_input(selected, control, path, deadline, nonce):
                    self.assertIs(selected, scenario)
                    self.assertIs(control, private)
                    self.assertEqual((path, nonce), (directory, "after"))
                    self.assertEqual(restored_forwarded, [])
                    self.assertEqual(scenario.c3["restore_pre_input"]["callbacks"], [])
                    # Faults are injected only in the same governed-input interval
                    # as the valid lazy build. No callback is staged before input.
                    if changed != "missing_build":
                        params = copy.deepcopy(build_params)
                        if changed == "draft": params["draft"]["model"] = "foreign"
                        await dispatch("callback/agent_customizer/customize_build", params)
                        if changed == "extra_build":
                            await dispatch("callback/agent_customizer/customize_build", copy.deepcopy(build_params))
                        if changed == "foreign_build":
                            scenario.customizer.builds[-1]["profile"] = "foreign"
                        if changed == "creation":
                            await dispatch("callback/agent_customizer/after_create", copy.deepcopy(create_params))
                        if changed == "nonce":
                            # Dispatch the actual earlier closure, not a fabricated
                            # reply: the unchanged oracle must reject handler 1.
                            restored_handlers[("fixture-scope", w.C3_TOOL)] = prepared_handlers[("fixture-scope", w.C3_TOOL)]
                        params = copy.deepcopy(call_params); params["arguments"]["nonce"] = "after"
                        response = await dispatch("callback/call_tool", params)
                        self.assertEqual(response, {"content": {"nonce": "after",
                            "handler_build": 1 if changed == "nonce" else 3 if changed == "extra_build" else 2}})
                    return {"inputs": scenario.retained_inputs + [{"input_id": "after"}]}

                status = SimpleNamespace(**after["status"])
                with patch.object(w.v, "SDK", SimpleNamespace(ConsoleLoginSelection=lambda *a: a), create=True), \
                     patch.object(w, "observe_ordinary", new=AsyncMock(return_value=(status, after))), \
                     patch.object(w.s, "input_rows", return_value=scenario.retained_inputs), \
                     patch.object(w, "require_c3_stale_scope", new=AsyncMock(return_value={"mismatch": "runtime_id"})) as stale, \
                     patch.object(w, "invoke_ordinary", new=AsyncMock(side_effect=governed_input)) as invoke:
                    args = (private, provider, directory, None,
                            SimpleNamespace(pid=11, args=["/fixture/bin/toolkit_gateway"]), scenario.origin)
                    if changed is not None:
                        expected = {"session": "same ordinary canonical session", "runtime": "retained native delivery scope",
                                    "scope": "retained native delivery scope", "owner": "exact native issued controller route",
                                    "missing_build": "exactly one unchanged build without after_create",
                                    "extra_build": "exactly one unchanged build without after_create",
                                    "foreign_build": "exactly one unchanged build without after_create",
                                    "draft": "exactly one unchanged build without after_create",
                                    "creation": "exactly one unchanged build without after_create",
                                    "nonce": "current handler-2 nonce"}[changed]
                        with self.assertRaisesRegex(AssertionError, expected):
                            await scenario.run(*args)
                        if changed in identity_failures:
                            invoke.assert_not_awaited()
                            self.assertEqual(restored_forwarded, [])
                        else:
                            invoke.assert_awaited_once_with(scenario, private, directory, unittest.mock.ANY, "after")
                    else:
                        result = await scenario.run(*args)
                        self.assertEqual(result["ordinary_class"]["status"], "ordinary_process_restore_used")
                        invoke.assert_awaited_once_with(scenario, private, directory, unittest.mock.ANY, "after")
                        self.assertEqual(result["restore_observation"], before)
                        self.assertEqual([method for method, _ in restored_forwarded],
                            ["callback/agent_customizer/customize_build", "callback/call_tool"])
                        self.assertEqual(result["ordinary_class"]["restored_callbacks"]["handler_result"],
                            {"nonce": "after", "handler_build": 2})
                    stale.assert_not_awaited()
                    scenario.customizer.wait_created.assert_not_awaited()
                scenario.check_ordinary_rebuild.assert_not_awaited()
                scenario.probe_attempt.join_listing.assert_awaited_once()
                self.assertEqual(retired, [True])


class BothSelectedExchangeRetirementControls(unittest.IsolatedAsyncioTestCase):
    """Exercise the real verifier loop and cleanup with offline service replies."""

    async def exercise(self, failure=None):
        class AfterBothExchanges(Exception):
            pass

        order, requests, reservations = [], {}, {}
        provider = SimpleNamespace(lock=threading.Lock(), errors=[], results={}, call_inputs={},
            selected_results={}, local_results={}, issued={"a-selected-access"}, helper_finished=set(),
            finish_gates={"a": threading.Event()}, finish_abort=threading.Event(), finish_release=threading.Event(),
            release_original_responses=lambda **kw: order.append(("original-cleanup", kw)))
        provider.finish_gates["a"].set()
        joined = {tag: {"outcome": "succeeded", "custody": "joined", "source": {"original": tag},
            "tool_policy": {"native": "accepted", "retained_as_current": True}} for tag in ("a", "b")}
        for tag in ("a", "b"):
            reservations[tag] = SimpleNamespace(released=False, transferred=False)
            calls = []

            def result(tag=tag):
                order.append(("joined", tag))
                reservations[tag].released = True
                return copy.deepcopy(joined[tag])

            def retire(*, timeout, tag=tag, calls=calls):
                self.assertEqual(timeout, 30)
                calls.append(True)
                order.append(("retire", tag, len(calls)))
                if tag == "a" and len(calls) == 1:
                    if failure == "refusal":
                        raise RuntimeError("retirement refused")
                    if failure == "mismatch":
                        return {**copy.deepcopy(joined[tag]), "source": {"original": "other"}}
                    if failure == "unreleased":
                        reservations[tag].released = False
                else:
                    reservations[tag].released = True
                return copy.deepcopy(joined[tag])

            attempt = SimpleNamespace(submit=lambda **kw: None, result=result, retire=retire)
            requests[tag] = SimpleNamespace(attempt=attempt, tool_policy_sha256=tag * 64,
                proposal={"tool_policy": {"candidate_canonical_json": "policy-" + tag}},
                authorize_tool_policy=lambda *args, attempt=attempt: attempt)
        login = dataclasses.make_dataclass("LoginRetired", ["fenced", "reservations_drained", "uncertain"])(True, True, False)
        enrollment = SimpleNamespace(result=lambda: None,
            retire=lambda **kw: (order.append(("login-cleanup",)), login)[1])
        next_tags = iter(("a", "b"))
        private = SimpleNamespace(begin_console_login=lambda *args, **kw: enrollment,
            next_service_parent_administration=lambda: requests[next(next_tags)])
        scenario = w.BothSelectedScenario.__new__(w.BothSelectedScenario)
        scenario.host_loop_failure = w.s.SelectedHostLoopFailure(w.G_ID, "live")
        scenario.finalizer = SimpleNamespace(application_tool_policy=b"bootstrap")
        scenario.port, scenario.origin = 1, "http://offline.invalid"
        scenario.begin_model_phase = lambda provider: None
        scenario.begin_workflow = AsyncMock(return_value={"children": [{"tag": "a"}]})
        scenario.workflow_context = (None, None, None, {"record": {"workflow": {"workflow": "G"}}}, object())
        scenario.observe_shared = AsyncMock(return_value=(SimpleNamespace(session_id="same-G"), None))

        async def launch(*args, tags):
            self.assertEqual(tags, ("b",))
            self.assertIn(("retire", "a", 1), order)
            self.assertTrue(reservations["a"].released)
            self.assertFalse(provider.finish_release.is_set())
            order.append(("launch", "b"))
            provider.issued.add("b-selected-access")
            return {"children": [{"tag": "b"}]}

        scenario.launch_children = AsyncMock(side_effect=launch)

        async def observe(call, *args, failure_observer):
            self.assertIs(failure_observer, scenario.host_loop_failure)
            failure_observer.require_alive()
            return call()

        async def pause(*args):
            if ("joined", "b") in order:
                self.assertIn(("retire", "b", 1), order)
                raise AfterBothExchanges()

        connection = SimpleNamespace(request=lambda *a, **kw: None, close=lambda: None,
            getresponse=lambda: SimpleNamespace(status=200, read=lambda limit: b'{"review":null}'))
        with tempfile.TemporaryDirectory() as temporary, contextlib.ExitStack() as stack:
            directory = Path(temporary)
            stack.enter_context(patch.object(w.v, "SDK", SimpleNamespace(ConsoleLoginSelection=lambda *a: None,
                StandingRuleDecision=lambda reservation: reservation)))
            stack.enter_context(patch.object(w.c, "signed_login", return_value="offline"))
            stack.enter_context(patch.object(w.s, "HTTPConnection", return_value=connection))
            stack.enter_context(patch.object(w.v, "observe_installed_service_reply", side_effect=observe))
            stack.enter_context(patch.object(w.s, "capture_public_review", side_effect=lambda *a, **kw: kw["prior_policy"]))
            stack.enter_context(patch.object(w.s, "PublicReservation", side_effect=lambda *a, tag, **kw: reservations[tag]))
            stack.enter_context(patch.object(w.asyncio, "sleep", side_effect=pause))
            expected = RuntimeError if failure == "refusal" else AssertionError if failure else AfterBothExchanges
            with self.assertRaises(expected):
                await scenario.run_shared(private, provider, directory, [], SimpleNamespace(poll=lambda: None), "unused")
            report = json.loads((directory / "console-journey.json").read_text())
        self.assertEqual(order[-1], ("login-cleanup",))
        self.assertIn(("original-cleanup", {"abort": True}), order)
        self.assertTrue(provider.finish_abort.is_set() and provider.finish_release.is_set())
        self.assertEqual(report["service_retirement"], {tag: joined[tag] for tag in report["public_joins"]})
        self.assertNotIn("cleanup_errors", report)
        return order, report, scenario.launch_children

    async def test_each_joined_exchange_retires_before_next_caller_and_cleanup_is_idempotent(self):
        order, report, launch = await self.exercise()
        launch.assert_awaited_once()
        self.assertLess(order.index(("joined", "a")), order.index(("retire", "a", 1)))
        self.assertLess(order.index(("retire", "a", 1)), order.index(("launch", "b")))
        self.assertLess(order.index(("joined", "b")), order.index(("retire", "b", 1)))
        self.assertEqual(report["public_retirements"], report["public_joins"])
        self.assertIn(("retire", "a", 2), order)
        self.assertIn(("retire", "b", 2), order)

    async def test_retirement_mismatch_refusal_or_unreleased_reservation_stops_b_and_keeps_cleanup(self):
        for failure in ("mismatch", "refusal", "unreleased"):
            with self.subTest(failure=failure):
                order, report, launch = await self.exercise(failure)
                launch.assert_not_awaited()
                self.assertNotIn(("joined", "b"), order)
                self.assertIn(("retire", "a", 2), order)
                self.assertEqual(set(report["public_joins"]), {"a"})
                if failure == "mismatch":
                    self.assertNotEqual(report["public_retirements"]["a"], report["service_retirement"]["a"])


class BothSelectedSuccessControls(unittest.TestCase):
    def observations(self):
        child, selected, preparation, _ = ColdSelectedPreparationControls().observations()
        children, selections, preparations, joins, proposals = {}, {}, {}, {}, {}
        for tag in ("a", "b"):
            children[tag], selections[tag], preparations[tag] = copy.deepcopy((child, selected, preparation))
            children[tag]["source_key"]["id"] = "source-" + tag
            children[tag]["original"].update(route="original-" + tag, original=tag)
            children[tag]["admission"].update(session_id=tag + "-session", input_id=tag + "-input")
            identity = {**copy.deepcopy(preparation["member"]["identity"]), "source": children[tag]["source_key"],
                        "acquisition": {**child["source_key"], "id": "acquisition-" + tag}}
            preparations[tag]["member"].update(member=tag, identity=identity)
            selections[tag]["context"]["membership"]["identity"]["acquisition"] = identity["acquisition"]
            selections[tag]["readback"] = {"status": "matched", "job_id": selected["job"]["id"], "result": copy.deepcopy(selected["job"]["result"])}
            selections[tag]["software_review"] = {"approval_id": "software-approval-" + tag}
            proposals[tag] = {"original": children[tag]["original"], "decision": {"decision": "decision-" + tag},
                              "issuance_id": "parent-" + tag}
            joins[tag] = {"outcome": "succeeded", "custody": "joined", "parent_issued": True,
                "parent_issuance_id": "parent-" + tag, "child": {"state": "grant_issued", "grant": {
                    "route": "original-" + tag, "decision": "decision-" + tag, "decision_sha256": tag * 64}},
                "source": {"route": "original-" + tag, "original": "fresh-" + tag}}
        members = {tag: preparations[tag]["member"]["identity"] for tag in ("a", "b")}
        for row in preparations.values(): row["record"]["members"] = copy.deepcopy(members)
        workflow = {"sealed": False, "waiters": {tag: {"identity": {"source": children[tag]["source_key"]},
            "departed": False} for tag in ("a", "b")}}
        return children, selections, preparations, joins, proposals, workflow

    def test_shared_job_requires_two_current_members_and_independent_correlated_grants(self):
        values = self.observations()
        w.require_both_selected_custody(*values)
        changes = (
            lambda c, s, p, j, q, f: s["b"]["job"].update(id="0" * 32),
            lambda c, s, p, j, q, f: s["b"]["readback"]["result"].update(receipt_sha256="0" * 64),
            lambda c, s, p, j, q, f: s["b"]["software_review"].update(approval_id=s["a"]["software_review"]["approval_id"]),
            lambda c, s, p, j, q, f: p["b"]["member"]["key"].update(id="other-P"),
            lambda c, s, p, j, q, f: p["a"]["record"]["members"]["a"].update(left=True),
            lambda c, s, p, j, q, f: f["waiters"]["a"].update(departed=True),
            lambda c, s, p, j, q, f: f.update(sealed=True),
            lambda c, s, p, j, q, f: j["b"]["child"]["grant"].update(decision=j["a"]["child"]["grant"]["decision"]),
            lambda c, s, p, j, q, f: j["b"].update(custody="pending"),
            lambda c, s, p, j, q, f: q["b"]["decision"].update(decision="foreign"),
            lambda c, s, p, j, q, f: p["b"].update(operation_cancelled=True),
        )
        for change in changes:
            changed = copy.deepcopy(values); change(*changed)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                w.require_both_selected_custody(*changed)

    def test_two_member_read_is_exact_and_does_not_weaken_single_member_cold_oracle(self):
        children, selected, preparations, joins, proposals, workflow = self.observations()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
            path.parent.mkdir(parents=True)
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE toolkit_records(realm TEXT,owner TEXT,id TEXT,data TEXT)")
                rows = [(children[tag]["source_key"], "toolkit.authenticated-source-event", {
                    "shared_preparations": [preparations[tag]["member"]], "operation_cancelled": False}) for tag in ("a", "b")]
                rows.append((preparations["a"]["member"]["key"], "meerkat_toolkit.shared_preparation", preparations["a"]["record"]))
                for key, schema, body in rows:
                    db.execute("INSERT INTO toolkit_records VALUES(?,?,?,?)", (*key.values(), json.dumps({"schema": schema, "body": body})))
            actual = {tag: w.selected_preparation_record(root, children[tag], selected[tag], expected_members=2) for tag in ("a", "b")}
            self.assertEqual(actual, preparations)
            w.require_both_selected_custody(children, selected, actual, joins, proposals, workflow)
            with self.assertRaises(AssertionError):
                w.selected_preparation_record(root, children["a"], selected["a"])

    def model(self):
        base = OriginalResponseGateControls().model()
        model = object.__new__(w.BothSelectedModel)
        model.__dict__.update(base.__dict__)
        model.selected_a = True
        model.selected_fixture = SimpleNamespace(COMPONENT="shared-selected-mcp")
        model.selected_request = {"profile": "node", "staged_id": "shared-selected-mcp"}
        model.selected_results, model.local_results, model.helper_finished = {}, {}, set()
        model.finish_gates = {tag: threading.Event() for tag in ("a", "b")}
        model.finish_release, model.finish_abort = threading.Event(), threading.Event()
        return model

    def test_each_g_context_uses_its_own_acquisition_for_same_selected_install_and_service(self):
        model = self.model()
        checked = {"scope": "selected_software_inputs_only", "profile": "node", "component": "shared-selected-mcp",
            "mcp_name": "shared-selected-mcp", "validation_id": "a" * 32, "effect_sha256": "b" * 64,
            "declared_service": {"authentication": {"kind": "none"}, "operation_tools": {"add": w.SELECTED_OPERATION}}}
        installed = {"scope": "retained_payload_and_launch_binding_only", "component": checked["component"],
            "effect_sha256": checked["effect_sha256"], "declared_service": checked["declared_service"], "receipt_sha256": "c" * 64}
        job = {"id": "d" * 32, "kind": "install", "status": "succeeded", "result": {**installed,
            "scope": "observed_software_installation_only", "readback_scope": installed["scope"]}}
        observation = {"job_id": job["id"], "status": "matched", "result": installed}
        for tag in ("a", "b"):
            context = {"purpose": "toolkit-workflow-inline-context-v1", "recipient": {"original": {"route": "original-" + tag}},
                "membership": {"identity": {"acquisition": {"id": "acquisition-" + tag}}}}
            body = {"messages": [{"role": "user", "content": w.s.G_TEXT}, {"role": "user", "content": json.dumps(context)}],
                "tools": [{"function": {"name": name}} for name in (*w.SELECTED_A_TOOLS, "setup_jobs", "toolkit_request_public_service")]}
            for suffix, value in (("state", {"selected_installation": {"profiles": ["node"]},
                "software_review_observation": {"scope": "latest_retained_software_attempt_only", "review": None}}), ("package", {})):
                key = f"{tag}-selected-{suffix}"; model.issued.add(key)
                body["messages"].append({"role": "tool", "tool_call_id": key, "content": json.dumps(value)})
            for suffix, name, value in (("check", "setup_selected_check", checked), ("install", "setup_selected_install", {"job": job}),
                ("reconcile", "setup_reconcile", {"observation": observation}),
                ("reviewed-state", "setup_state", {"software_review_observation": {
                    "scope": "latest_retained_software_attempt_only", "review": {
                        "approval_id": "software-approval-" + tag, "call_id": f"{tag}-selected-install",
                        "tool": "setup_selected_install", "validation_id": checked["validation_id"],
                        "retained_install_job_id": job["id"], "approval_status": "approved", "decision": "approve",
                        "dispatch_custody": "settled", "effect_outcome": "committed", "dispatch_error_kind": None}}}),
                ("jobs", "setup_jobs", {"jobs": [{"id": job["id"], "kind": "install", "status": "succeeded"}], "next_before": None}),
                ("access", "toolkit_request_public_service", {"grant_issued": True, "fresh_instruction_retained": True, "ready": False})):
                with model.lock: reply = model.response(body)
                call = reply["delta"]["tool_calls"][0]
                self.assertEqual(call["id"], f"{tag}-selected-{suffix}")
                self.assertEqual(call["function"]["name"], name)
                arguments = json.loads(call["function"]["arguments"])
                self.assertEqual(arguments["setup_acquisition"], "acquisition-" + tag)
                if suffix == "access":
                    self.assertEqual(arguments["agent"], "original-" + tag)
                    self.assertEqual(arguments["operations"], [w.SELECTED_OPERATION])
                body["messages"].append({"role": "tool", "tool_call_id": call["id"], "content": json.dumps(value)})
            with model.lock: self.assertEqual(model.response(body)["delta"]["content"], model.helper_answer(tag))
        self.assertEqual(model.helper_finished, {"a", "b"})
        self.assertEqual(model.selected_results["a"]["job"], model.selected_results["b"]["job"])
        self.assertNotEqual(model.selected_results["a"]["software_review"]["approval_id"],
                            model.selected_results["b"]["software_review"]["approval_id"])
        self.assertFalse(model.finish_release.is_set())

    def test_real_result_precedes_final_response_gate_and_abort_never_produces_success(self):
        for abort in (False, True):
            model, replies, failures, workers = self.model(), {}, {}, []
            try:
                for tag in ("a", "b"):
                    model.original_gates[tag]["deadline"] = time.monotonic() + 5
                    results = {f"{tag}-local-search": {"results": [{"name": w.SELECTED_OPERATION}]},
                        f"{tag}-local-load": {}, f"{tag}-local-add": {"sum": sum(w.SELECTED_ARGUMENTS[tag].values())}}
                    def respond(tag=tag, results=results):
                        try:
                            with model.lock: replies[tag] = model.original_continuation({}, results, tag, 2)
                        except BaseException as error: failures[tag] = error
                    worker = threading.Thread(target=respond); workers.append(worker); worker.start()
                self.assertTrue(all(gate.wait(2) for gate in model.finish_gates.values()))
                self.assertEqual(replies, {})
                self.assertEqual(set(model.local_results), {"a", "b"})
                if abort: model.finish_abort.set()
                model.finish_release.set()
            finally:
                model.finish_abort.set() if abort else None
                model.finish_release.set()
                for worker in workers: worker.join(5)
            self.assertFalse(any(worker.is_alive() for worker in workers))
            if abort:
                self.assertEqual(replies, {})
                self.assertEqual(set(failures), {"a", "b"})
                self.assertTrue(all("aborted" in str(error) for error in failures.values()))
            else:
                self.assertEqual(failures, {})
                self.assertEqual({tag: row["delta"]["content"] for tag, row in replies.items()},
                                 {tag: model.local_results[tag]["answer"] for tag in ("a", "b")})
        bad = self.model()
        with bad.lock, self.assertRaisesRegex(AssertionError, "actual arithmetic result differs"):
            bad.original_continuation({}, {"a-local-search": {"results": [{"name": w.SELECTED_OPERATION}]},
                "a-local-load": {}, "a-local-add": {"sum": 999}}, "a", 2)
        self.assertFalse(bad.finish_gates["a"].is_set())




class ParentPreparationOrderingControls(unittest.IsolatedAsyncioTestCase):
    async def test_actual_commission_schedule_prepares_exact_parent_before_its_native_input(self):
        from meerkat_toolkit import host_control, workflow_commission
        @dataclasses.dataclass
        class Work:
            id: str = "parent-work"
            revision: int = 1
            realm_id: str = "realm"
            namespace: str = "work"
            owner: object = None
            labels: object = None
        for mode in ("prepared", "refused", "wrong parent"):
            with self.subTest(mode=mode), tempfile.TemporaryDirectory() as temporary:
                directory, order = Path(temporary), []
                scenario = w.Scenario.__new__(w.Scenario)
                scenario.check_ordinary_rebuild = AsyncMock()
                scenario.respawn_shared_g = False
                parent = host_control.PrivateOriginalRef("planning-parent", "P", "b" * 64)
                evidence = host_control.PrivateSourceEvidence("event", "1", "a" * 64)
                def event(_, route, text, contributors=()):
                    return host_control.PrivateAuthenticatedEvent(route, "P" if route == "planning-parent" else "G",
                        "occurrence", evidence, evidence, tuple(contributors), hashlib.sha256(text.encode()).hexdigest())
                def prepare(selected, **kwargs):
                    order.append("prepare")
                    self.assertEqual(selected.parent.original, parent)
                    self.assertEqual(selected.event.contributors, (parent.original,))
                    self.assertEqual((kwargs["parent_root_issuance_id"], kwargs["parent_leaf_issuance_id"]),
                                     ("supplied-p-root", "supplied-p-leaf"))
                    if mode == "refused": raise RuntimeError("preparation refused")
                    return parent if mode == "prepared" else dataclasses.replace(parent, original="foreign")
                def submit(original, text, authentication, **kwargs):
                    self.assertEqual(order, ["prepare"])
                    self.assertEqual((original, text, authentication), (parent, w.s.P_TEXT, evidence))
                    order.append("submit")
                    return {"state": "admitted", "deduplicated": False,
                            "source_settlement": "fresh_acceptance_promoted", "turn_ticket": "actual-owner-snapshot"}
                def commission(selected, **kwargs):
                    self.assertEqual(order, ["prepare", "submit", "roster"])
                    order.append("commission")
                    raise RuntimeError("stop at existing native commission boundary")
                private = SimpleNamespace(retain_authenticated_event=lambda *a, **k: SimpleNamespace(original=parent),
                    prepare_workflow_parent=prepare, submit_authenticated_original=submit,
                    begin_workflow_commission=commission)
                graph = SimpleNamespace(workgraph_snapshot=AsyncMock(return_value=SimpleNamespace(
                    realm_id="realm", namespace="work", all_namespaces=False)),
                    workgraph_create=AsyncMock(return_value=Work()),
                    workgraph_claim=AsyncMock(side_effect=lambda *args, **kw: Work(owner=args[2], labels=[w.CYCLE])))
                scenario.runtime = SimpleNamespace(host_control=private, mob_handle=lambda: graph)
                scenario.roster = SimpleNamespace(prepare=lambda selected: order.append("roster"), publish=lambda *a: None)
                scenario.plan = {"owner": {"native_host": {"realm": "realm", "routes": [{"id": "planning-parent",
                    "origin": {"member": {"mob_id": "mob"}, "requester": w.v.installed_fixture_principal("operator")}}]}}}
                with patch.object(w.v, "SDK", host_control), patch.object(w, "COMMISSION", workflow_commission, create=True), \
                        patch.object(w, "authenticated_event", side_effect=event):
                    expected = "native commission boundary" if mode == "prepared" else (
                        "preparation refused" if mode == "refused" else "different actual P")
                    with self.assertRaisesRegex((RuntimeError, AssertionError), expected):
                        await scenario.begin_workflow(private, None, directory, SimpleNamespace(add=lambda *a, **k: None),
                            SimpleNamespace(args=["/selected/bin/toolkit_gateway", "--private-workflow-launch"]),
                            time.monotonic() + 30)
                self.assertEqual(order, ["prepare", "submit", "roster", "commission"] if mode == "prepared" else ["prepare"])


class PublicOperationAuditControls(unittest.TestCase):
    """Native audit-schema fixtures exercise the oracle, not native dispatch."""
    OPERATION_ID = "00000000-0000-4000-8000-000000000003"

    @staticmethod
    def audit(call_id="a-selected-access"):
        def principal(name, kind="service_account"):
            return {"kind": kind, "id": name, "qualification": {
                "kind": "qualified", "trust_domain_id": "fixture"}}
        def lineage(grant):
            return {"root_authority": principal("operator"), "authority_namespace": "fixture",
                "authority_generation": 1, "authority_incarnation": "00000000-0000-4000-8000-000000000001",
                "grant_id": grant, "issued_revision": 20}
        tool = "toolkit_request_public_service"
        input_id = "00000000-0000-4000-8000-000000000006"
        first = {"contributors": [{"input_id": input_id, "requester": principal("human", "human"),
            "logical_executor": principal("planning-lead"), "represented_subject": principal("human", "human")}],
            "observation": {"operation_id": PublicOperationAuditControls.OPERATION_ID, "execution_scope": {
                "kind": "runtime_input", "owner_session_id": "00000000-0000-4000-8000-000000000004",
                "runtime_epoch_id": "00000000-0000-4000-8000-000000000005",
                "submitted_input_id": input_id, "canonical_input_id": input_id},
                "run_id": "00000000-0000-4000-8000-000000000007", "context_revision": None,
                "observation": {"kind": "prepared",
                    "target": {"kind": "tool", "call_id": call_id, "tool_name": tool,
                        "arguments_digest": [7] * 32, "owners": [
                            {"authority_key": "root-dispatcher", "owner_key": tool}]},
                    "policy": {"publication_sequence": 248, "observed_at_ms": 1792949645241,
                        "operation_authorities": [{"kind": "targeted_grant_lineages",
                            "base": [lineage("G-root")], "targets": [{"target": {
                                "kind": "registered_tool_bundle_operation",
                                "bundle": "meerkat_toolkit.setup_accounts", "operation": tool},
                                "lineage": [lineage("G-public")]}]}], "controller_lineages": [],
                        "publication_instance": "00000000-0000-4000-8000-000000000002"}}}}
        def observation(value):
            row = copy.deepcopy(first)
            row["observation"]["observation"] = value
            return row
        second = copy.deepcopy(first)
        second["observation"]["observation"]["policy"].update(
            publication_sequence=262, observed_at_ms=1792949645931)
        outcome = {"kind": "outcome", "outcome": {"kind": "tool_dispatch_returned",
            "asynchronous_operations": [], "result_is_error": False, "terminal_error": None}}
        # Same seven-kind sequence as both retained installed service operations.
        return [first, *(observation({"kind": "entry", "boundary": boundary}) for boundary in (
            "tool_dispatch", "tool_execution_policy", "tool_effect")), second,
            observation(outcome), observation(copy.deepcopy(outcome))]

    def test_single_and_refreshed_preparations_preserve_every_observation(self):
        for caller in ("a", "b"):
            for mode in ("single", "refreshed", "same_publication", "wall_clock_backwards"):
                with self.subTest(caller=caller, mode=mode):
                    call_id = f"{caller}-selected-access"
                    rows = self.audit(call_id)
                    if mode == "single": rows.pop(4)
                    elif mode == "same_publication":
                        rows[4]["observation"]["observation"]["policy"]["publication_sequence"] = 248
                    elif mode == "wall_clock_backwards":
                        rows[4]["observation"]["observation"]["policy"]["observed_at_ms"] = 1792949645240
                    before = copy.deepcopy(rows)
                    result = w.s.public_operation_audit(rows, "toolkit_request_public_service", call_id)
                    self.assertEqual(result, {"operation_id": self.OPERATION_ID,
                        "observations": [row["observation"]["observation"] for row in before]})
                    self.assertEqual(rows, before)

    def test_changed_identity_target_contributors_or_authority_refuses(self):
        event = ("observation", "observation")
        mutations = [
            (("observation", "operation_id"), "another-operation"),
            (("observation", "run_id"), "another-run"),
            (("observation", "context_revision"), "another-context"),
            (("observation", "execution_scope", "runtime_epoch_id"), "another-epoch"),
            (("observation", "execution_scope", "canonical_input_id"), "another-input"),
            (("contributors", 0, "input_id"), "another-input"),
            (("contributors", 0, "requester", "id"), "another-human"),
            ((*event, "target", "kind"), "model"),
            ((*event, "target", "call_id"), "another-call"),
            ((*event, "target", "tool_name"), "another-tool"),
            ((*event, "target", "arguments_digest", 0), 8),
            ((*event, "target", "owners", 0, "owner_key"), "another-owner"),
            ((*event, "policy", "operation_authorities", 0, "base", 0, "grant_id"), "another-grant"),
            ((*event, "policy", "operation_authorities", 0, "base", 0, "authority_generation"), True),
            ((*event, "policy", "operation_authorities", 0, "targets", 0, "lineage", 0,
                "root_authority", "id"), "another-authority"),
            ((*event, "policy", "controller_lineages"), [[{"grant_id": "another-controller"}]]),
            ((*event, "policy", "publication_instance"), "00000000-0000-4000-8000-000000000003"),
        ]
        for path, value in mutations:
            with self.subTest(path=path):
                rows = self.audit()
                changed = rows[4]
                for key in path[:-1]: changed = changed[key]
                changed[path[-1]] = value
                before = copy.deepcopy(rows)
                with self.assertRaisesRegex(AssertionError, "a-selected-access"):
                    w.s.public_operation_audit(rows, "toolkit_request_public_service", "a-selected-access")
                self.assertEqual(rows, before)

    def test_entry_and_outcome_cannot_borrow_an_operation_id_from_another_binding(self):
        mutations = [(("observation", "run_id"), "another-run"),
            (("observation", "execution_scope", "runtime_epoch_id"), "another-epoch"),
            (("observation", "context_revision"), "another-context"),
            (("contributors", 0, "input_id"), "another-input"),
            (("contributors", 0, "logical_executor", "id"), "another-executor")]
        for index in (1, 5):
            for path, value in mutations:
                with self.subTest(row=index, path=path):
                    rows = self.audit()
                    changed = rows[index]
                    for key in path[:-1]: changed = changed[key]
                    changed[path[-1]] = value
                    before = copy.deepcopy(rows)
                    with self.assertRaisesRegex(AssertionError, "operation audit binding or contributors changed"):
                        w.s.public_operation_audit(rows, "toolkit_request_public_service", "a-selected-access")
                    self.assertEqual(rows, before)

    def test_invalid_policy_observation_or_regressing_publication_refuses(self):
        cases = [(field, value) for field in ("publication_sequence", "observed_at_ms")
            for value in (None, True, "262", -1, 2**64)]
        cases += [("publication_sequence", 247), ("publication_instance", None),
                  ("publication_instance", ""), ("publication_instance", 262)]
        for field, value in cases:
            with self.subTest(field=field, value=value):
                rows = self.audit()
                rows[4]["observation"]["observation"]["policy"][field] = value
                with self.assertRaises(AssertionError):
                    w.s.public_operation_audit(rows, "toolkit_request_public_service", "a-selected-access")
        for mode in ("missing_policy", "missing_instance", "single_missing_instance"):
            with self.subTest(mode=mode):
                rows = self.audit()
                if mode == "single_missing_instance": rows.pop(4)
                prepared = rows[0]["observation"]["observation"]
                if mode == "missing_policy": prepared.pop("policy")
                else: prepared["policy"].pop("publication_instance")
                with self.assertRaises(AssertionError):
                    w.s.public_operation_audit(rows, "toolkit_request_public_service", "a-selected-access")
        rows = self.audit()
        third = copy.deepcopy(rows[4])
        third["observation"]["observation"]["policy"]["publication_sequence"] = 260
        rows.insert(5, third)
        with self.assertRaisesRegex(AssertionError, "publication sequence regressed"):
            w.s.public_operation_audit(rows, "toolkit_request_public_service", "a-selected-access")

    def test_missing_preparation_entry_success_or_any_refusal_still_fails(self):
        for mode in ("missing_preparation", "missing_entry", "missing_outcome", "wrong_operation_outcomes",
                     "error_result", "terminal_error", "asynchronous_result", "refused", "authorization_unavailable"):
            with self.subTest(mode=mode):
                rows = self.audit("b-selected-access")
                if mode.startswith("missing_"):
                    kind = {"missing_preparation": "prepared", "missing_entry": "entry",
                            "missing_outcome": "outcome"}[mode]
                    rows = [row for row in rows if row["observation"]["observation"]["kind"] != kind]
                elif mode in ("refused", "authorization_unavailable"):
                    failure = copy.deepcopy(rows[0])
                    event = failure["observation"]["observation"]
                    failure["observation"]["observation"] = {"kind": mode, "target": event["target"]}
                    if mode == "refused": failure["observation"]["observation"].update(reason="denied")
                    rows.append(failure)
                else:
                    for row in rows[-2:]:
                        if mode == "wrong_operation_outcomes": row["observation"]["operation_id"] = "another-operation"
                        else:
                            outcome = row["observation"]["observation"]["outcome"]
                            if mode == "error_result": outcome["result_is_error"] = True
                            elif mode == "terminal_error": outcome["terminal_error"] = "refused"
                            else: outcome["asynchronous_operations"] = ["pending-operation"]
                with self.assertRaisesRegex(AssertionError, "b-selected-access"):
                    w.s.public_operation_audit(rows, "toolkit_request_public_service", "b-selected-access")


class SelectedPreparationJobShapeControls(unittest.TestCase):
    """Real read-only oracle over synthetic SQLite rows, not native acceptance."""

    @contextlib.contextmanager
    def retained(self):
        children, selected, preparations, *_ = BothSelectedSuccessControls().observations()
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            path = root / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
            path.parent.mkdir(parents=True)
            with sqlite3.connect(path) as db:
                db.execute("CREATE TABLE toolkit_records(realm TEXT,owner TEXT,id TEXT,data TEXT)")
                def store(record):
                    db.execute("DELETE FROM toolkit_records")
                    rows = [(children[tag]["source_key"], "toolkit.authenticated-source-event", {
                        "shared_preparations": [preparations[tag]["member"]], "operation_cancelled": False})
                        for tag in ("a", "b")]
                    rows.append((preparations["a"]["member"]["key"], "meerkat_toolkit.shared_preparation", record))
                    for key, schema, body in rows:
                        db.execute("INSERT INTO toolkit_records VALUES(?,?,?,?)",
                            (*(key[name] for name in ("realm", "owner", "id")), json.dumps({"schema": schema, "body": body})))
                    db.commit()
                store(preparations["a"]["record"])
                yield root, children, selected, preparations, store

    def test_absent_and_explicit_null_tool_error_match_the_same_canonical_completion(self):
        with self.retained() as (root, children, selected, preparations, _):
            for tag in ("a", "b"):
                for absent in (True, False):
                    observed = copy.deepcopy(selected[tag])
                    if absent:
                        observed["job"].pop("error")
                    original = copy.deepcopy(observed)
                    with self.subTest(tag=tag, absent=absent):
                        actual = w.selected_preparation_record(root, children[tag], observed, expected_members=2)
                        self.assertEqual(actual, preparations[tag])
                        self.assertEqual(observed, original)
            with self.assertRaises(AssertionError):
                w.selected_preparation_record(root, children["a"], selected["a"])

    def test_mandatory_tool_fields_and_exact_canonical_job_shape_remain_required(self):
        with self.retained() as (root, children, selected, preparations, store):
            for field in ("id", "kind", "family", "status", "result"):
                bad = copy.deepcopy(selected["a"])
                bad["job"].pop(field)
                with self.subTest(missing_tool_field=field), self.assertRaises(KeyError):
                    w.selected_preparation_record(root, children["a"], bad, expected_members=2)
            for field in ("id", "kind", "family", "status", "result", "error", "extra"):
                record = copy.deepcopy(preparations["a"]["record"])
                if field == "extra":
                    record["completion"]["job"][field] = None
                else:
                    record["completion"]["job"].pop(field)
                store(record)
                with self.subTest(canonical_shape=field), self.assertRaises(AssertionError):
                    w.selected_preparation_record(root, children["a"], selected["a"], expected_members=2)

    def test_mismatches_non_success_and_non_null_errors_stay_refused(self):
        with self.retained() as (root, children, selected, preparations, store):
            for field, value in (("id", "0" * 32), ("kind", "export"), ("family", "selected-python"),
                                 ("status", "unconfirmed"), ("result", {"receipt_sha256": "0" * 64})):
                bad = copy.deepcopy(selected["a"])
                bad["job"][field] = value
                with self.subTest(mismatch=field), self.assertRaises(AssertionError):
                    w.selected_preparation_record(root, children["a"], bad, expected_members=2)
            for field in ("controller", "validation_id"):
                record = copy.deepcopy(preparations["a"]["record"])
                record["completion"][field] = "0" * len(record["completion"][field])
                store(record)
                with self.subTest(canonical_identity=field), self.assertRaises(AssertionError):
                    w.selected_preparation_record(root, children["a"], selected["a"], expected_members=2)
            for field, value in (("status", "running"), ("status", "unconfirmed"),
                                 ("error", "operation_unconfirmed"), ("error", ""),
                                 ("error", False), ("error", 0), ("error", {})):
                bad = copy.deepcopy(selected["a"])
                record = copy.deepcopy(preparations["a"]["record"])
                bad["job"][field] = value
                record["completion"]["job"][field] = value
                store(record)
                with self.subTest(matching_but_refused=(field, value)), self.assertRaises(AssertionError):
                    w.selected_preparation_record(root, children["a"], bad, expected_members=2)


if __name__ == "__main__":
    unittest.main()
