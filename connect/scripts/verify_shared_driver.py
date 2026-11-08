#!/usr/bin/env python3
"""Opt-in production Driver embedding acceptance, not stock Console workflow UI.

Build nothing. Supply the acceptance example staged in bin/ with its exact full
resource closure, and exact SDK sources. Reuse the installed verifier's real
SDK startup, finalizer and joined native/process cleanup. Model choices are
controlled; packaged install, protected software review, native per-caller
authority, one public Hacker News read and fresh B completion are real.
"""
from __future__ import annotations
import argparse
import asyncio
import copy
from contextlib import contextmanager
import dataclasses
import hashlib
from http.client import HTTPConnection
import importlib
import importlib.util
import inspect
import json
import os
from pathlib import Path
import sqlite3
import sys
import threading
import time
import uuid


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


v = load("shared_driver_verifier", Path(__file__).with_name("verify_native_gateway_subprocess.py"))
c = load("shared_driver_console", Path(__file__).with_name("verify_installed_console.py"))
require = v.require
P_TEXT = "Driver host planning parent, without either waiter request"
G_TEXT = "Driver host planning lead, awaiting independently authorized context delivery"
OPERATION = "get_top_hackernews_stories"
# Explicit application ceiling matching CheckedInstallation's existing peer.
# The controlled model invokes only SETUP below. The owner validates this list
# against the actual retained peer before issuing independent G authority.
CHECKED_SETUP = (
    "setup_state", "setup_jobs", "setup_job", "setup_catalog", "setup_catalog_entry",
    "setup_catalog_route", "setup_prepare", "setup_retry_prepare", "setup_help", "setup_check",
    "setup_stop", "setup_reconcile", "setup_recovery_manifest", "setup_guide",
    "setup_inspect_package", "setup_inspect_schema", "setup_acquire_python", "setup_search_node",
    "setup_acquire_node", "setup_acquire_node_prebuilt", "setup_inspect_acquired", "setup_recipes",
    "setup_recipe", "setup_remember_remote", "setup_export_recipe", "setup_install",
    "setup_selected_install", "setup_selected_check")
SETUP = ("setup_prepare", "setup_help", "setup_check", "setup_install", "setup_job", "setup_reconcile")


def request(tag):
    return f"driver-shared-{tag}: read one public Hacker News story using the packaged integration"


class ObservedCustomizer:
    """Explicit SDK callback, preserving the actual draft and creating no tools."""
    def __init__(self):
        self.builds = []
        self.created = []

    async def customize_build(self, context, spec, draft):
        require(context.identity == spec.identity, "SDK customizer identity differs from actual spec")
        require(not draft.external_tools, "acceptance selected no application callback tools")
        self.builds.append({"identity": context.identity, "profile": spec.profile})

    async def after_create(self, identity, session_id, context):
        require(any(row["identity"] == identity for row in self.builds)
                and isinstance(session_id, str) and session_id, "after_create has no actual build or session")
        self.created.append({"identity": identity, "session_id": session_id})

    def require_created(self, identity):
        builds = [row for row in self.builds if row["identity"] == identity]
        created = [row for row in self.created if row["identity"] == identity]
        require(builds and len(created) == 1, "actual G customize_build and delegated after_create required")
        return {"builds": copy.deepcopy(builds), "created": copy.deepcopy(created),
                "draft_changed": False, "application_tools_added": False}


class Model(v.InstalledAcquisitionProvider):
    """Scripted decisions over actual native messages and tool observations."""
    request = staticmethod(request)
    def __init__(self, directory, events):
        self.b_context = threading.Event()
        self.release_b = threading.Event()
        self.abort_b = threading.Event()
        self.call_inputs = {}
        self.sequence = 0
        super().__init__(directory, events)

    def call(self, body, key, name, arguments, context=None):
        offered = {row["function"]["name"] for row in body.get("tools", [])}
        require(name in offered, "actual native surface omitted " + name)
        if name in SETUP or name in ("setup_state", "setup_jobs", "setup_inspect_package", "setup_selected_check", "setup_selected_install"):
            require(context is not None, "setup operation has no actual delivered membership")
            arguments = {**arguments, "setup_acquisition": context["membership"]["identity"]["acquisition"]["id"]}
        require(key not in self.issued, "duplicate scripted operation " + key)
        self.issued.add(key)
        self.call_inputs[key] = {"name": name, "arguments": arguments}
        return {"delta": {"tool_calls": [{"index": 0, "id": key, "type": "function",
            "function": {"name": name, "arguments": json.dumps(arguments)}}]}, "finish_reason": "tool_calls"}

    @staticmethod
    def done(text):
        return {"delta": {"content": text}, "finish_reason": "stop"}

    def original_a_context(self, body, results, context):
        return self.done("A context received; no service effect requested for A.")

    def original_context(self, body, results, context):
        if context["recipient"]["original"]["route"] == "original-a":
            return self.original_a_context(body, results, context)
        return None

    def response(self, body):
        self.sequence += 1
        seq = self.sequence
        texts = [m["content"] for m in body["messages"] if m["role"] == "user" and isinstance(m.get("content"), str)]
        results = {}
        for message in body["messages"]:
            key = message.get("tool_call_id")
            if message["role"] != "tool" or key not in self.issued:
                continue
            raw = message["content"]
            require(isinstance(raw, str), "native tool observation must retain text")
            if key == "b-public-read":
                value = {"story": v.installed_public_story(raw), "actual_tool_result": raw}
            else:
                value = v.unique_json(raw.encode())
                require(not value.get("error"), "actual tool refused " + key + ": " + repr(value))
            require(key not in self.results or self.results[key] == value, "actual result changed")
            self.results[key] = value
            results[key] = value
        if P_TEXT in texts:
            return self.done("Planning parent is ready.")
        if G_TEXT in texts:
            contexts = []
            for text in texts:
                try:
                    value = json.loads(text)
                except ValueError:
                    continue
                if isinstance(value, dict) and value.get("purpose") == "toolkit-workflow-inline-context-v1":
                    contexts.append(value)
            if not contexts:
                return self.done("Dedicated G awaits native waiter context.")
            context = contexts[-1]
            route = context["recipient"]["original"]["route"]
            require(route in ("original-a", "original-b"), "unexpected delivered caller")
            response = self.original_context(body, results, context)
            if response is not None:
                return response
            self.b_context.set()
            # This is a model scheduling gate, not native permission/readiness.
            # The parent releases it only after canonical A departure.
            # Handler owns this test-only lock. Let unrelated native A/B
            # inference finish while this one model response is deliberately held.
            self.lock.release()
            try:
                require(self.release_b.wait(60.0), "A cancellation did not settle before B model continued")
            finally:
                self.lock.acquire()
            require(not self.abort_b.is_set(), "B model release was aborted before setup effects")
            names = {row["function"]["name"] for row in body.get("tools", [])}
            if not set(SETUP).issubset(names):
                searches = [value for key, value in results.items() if key.startswith("g-search-")]
                if searches and set(SETUP).issubset({row["name"] for row in searches[-1]["results"]}):
                    return self.call(body, f"g-load-{seq}", "tool_catalog_load", {"names": list(SETUP)})
                return self.call(body, f"g-search-{seq}", "tool_catalog_search", {"query": "setup", "limit": 50})
            selected = {"family": "agno", "profile": "hackernews", "operations": [OPERATION], "values": {"timeout": 7}}
            if "g-prepare" not in results:
                return self.call(body, "g-prepare", "setup_prepare", {"family": "agno"}, context)
            for key, label in (("g-prepare", "prepare"), ("g-install", "install")):
                if key in results:
                    job = results[key]["job"]["id"]
                    rows = [value for value in results.values() if value.get("job", {}).get("id") == job]
                    latest = next((row for row in rows if row["job"]["status"] == "succeeded"), results[key])
                    require(latest["job"]["status"] in ("running", "succeeded"), "actual setup worker failed")
                    if latest["job"]["status"] == "running":
                        return self.call(body, f"g-{label}-poll-{seq}", "setup_job", {"job_id": job}, context)
            if "g-help" not in results:
                return self.call(body, "g-help", "setup_help", {"family": "agno", "profile": "hackernews"}, context)
            require(results["g-help"]["authentication"] == {"kind": "none"}, "selected package is not public")
            require(any(row["id"] == OPERATION and row["tool"] == OPERATION for row in results["g-help"]["operations"]), "raw operation missing")
            if "g-check" not in results:
                return self.call(body, "g-check", "setup_check", selected, context)
            checked = results["g-check"]
            require(checked["scope"] == "configuration_only" and checked["credential_environment_references"] == [], "check crossed public scope")
            if "g-install" not in results:
                return self.call(body, "g-install", "setup_install", {**selected,
                    "validation_id": checked["validation_id"], "configuration_sha256": checked["configuration_sha256"]}, context)
            job = results["g-install"]["job"]["id"]
            if "g-reconcile" not in results:
                return self.call(body, "g-reconcile", "setup_reconcile", {"job_id": job}, context)
            require(results["g-reconcile"]["observation"]["status"] == "matched", "install not reconciled")
            if "g-public-access" not in results:
                return self.call(body, "g-public-access", "toolkit_request_public_service", {
                    "job_id": job, "agent": "original-b", "operations": [OPERATION], "expires_in_ms": 180000,
                    "setup_acquisition": context["membership"]["identity"]["acquisition"]["id"],
                    "tool_policy": {"tools": [], "catalog": "r2"}})
            value = results["g-public-access"]
            require(value["grant_issued"] is True and value["fresh_instruction_retained"] is True and value["ready"] is False,
                    "helper response overstated actual service readiness")
            self.finished.set()
            return self.done("B's reviewed public instruction is retained; actual use remains B's task.")
        tags = [tag for tag in ("a", "b") if any(self.request(tag) in text for text in texts)]
        require(len(tags) == 1, "model received no unique independent caller")
        tag = tags[0]
        key = tag + "-find"
        if key not in results:
            return self.call(body, key, "find_and_install", {"request": self.request(tag)})
        count = sum(self.request(tag) in text for text in texts)
        if count == 1:
            return self.done("The independently authenticated task is waiting for setup.")
        return self.original_continuation(body, results, tag, count)

    def original_continuation(self, body, results, tag, count):
        require(tag == "b" and count == 2, "cancelled A or duplicate fresh B input reached the model")
        if "b-search" not in results:
            return self.call(body, "b-search", "tool_catalog_search", {"query": OPERATION, "limit": 50})
        require(any(row["name"] == OPERATION for row in results["b-search"]["results"]), "B cannot discover its reviewed operation")
        if "b-load" not in results:
            return self.call(body, "b-load", "tool_catalog_load", {"names": [OPERATION]})
        if "b-public-read" not in results:
            return self.call(body, "b-public-read", OPERATION, {"num_stories": 1})
        story = results["b-public-read"]["story"]
        answer = f"Hacker News story {story['id']}: {story['title']}"
        require(self.business_observation is None, "duplicate model answer")
        self.business_observation = {**results["b-public-read"], "answer": answer}
        self.business_response_prepared.set()
        return self.done(answer)


class SelectedHostLoopFailure:
    """One fixed typed failure for one selected member and one runtime phase."""

    def __init__(self, member_id, phase, previous=None):
        require(member_id in ("planning-lead", "acceptance-ordinary-member")
                and phase in ("live", "prepared", "restored"), "unknown acceptance error selection")
        self.member_id, self.phase, self.previous = member_id, phase, previous
        self.failed = threading.Event()
        self._conditions_lock = threading.Lock()
        self._conditions = {}

    async def __call__(self, event):
        # Meta dispatches ErrorEvent here. Retain no error payload or inferred
        # durability/lock cause, and forward the identical event exactly once.
        if event.category == "host_loop_crash" and event.context.get("member_id") == self.member_id:
            self.failed.set()
            with self._conditions_lock:
                conditions = tuple(self._conditions.values())
            # Never hold the registry lock while acquiring a waiter Condition.
            # Its predicate checks the Event under that same Condition, so an
            # event before registration or before parking cannot be lost.
            for condition in conditions:
                with condition:
                    condition.notify_all()
        if self.previous is not None:
            result = self.previous(event)
            if inspect.isawaitable(result):
                await result

    @contextmanager
    def wake_condition(self, condition):
        # Registration belongs to the actual worker and this observer phase.
        # No cancellation, replacement waiter or callback replay is introduced.
        token = object()
        with self._conditions_lock:
            self._conditions[token] = condition
        try:
            yield
        finally:
            with self._conditions_lock:
                del self._conditions[token]

    def observation(self):
        if self.failed.is_set():
            return {"category": "host_loop_crash", "member_id": self.member_id, "phase": self.phase}
        return None

    def require_alive(self):
        require(not self.failed.is_set(), "selected acceptance member reported typed host_loop_crash")


class Scenario(c.ConsoleScenario):
    failure_member_id = "planning-lead"

    def __init__(self, verifier, origin):
        super().__init__(verifier, origin)
        self.host_loop_failure = SelectedHostLoopFailure(self.failure_member_id, getattr(self, "phase", "live"))

    def configure_error_observer(self, config):
        previous = getattr(config, "error_callback", None)
        if isinstance(previous, SelectedHostLoopFailure):
            require(previous is self.host_loop_failure, "unexpected acceptance error observer")
            previous = previous.previous
        # Restore receives its own copied config before connect. The old
        # runtime keeps its old callback, so late cleanup cannot poison it.
        self.host_loop_failure = SelectedHostLoopFailure(
            self.failure_member_id, getattr(self, "phase", "live"), previous)
        config.error_callback = self.host_loop_failure

    request = staticmethod(request)
    name = "shared-driver-public-success"
    scope = ("actual ApplicationDriver::run_with_workflows backend embedding; four initial members, zero initial G, "
             "late held G, two independent preselected callers, cancelled A, protected B software install, "
             "one native public HN operation and finalized fresh B answer; controlled model and explicit SDK pass-through customizer; "
             "not stock Console workflow composition, live login or cold recovery acceptance")

    def configure_owner(self, owner, source):
        super().configure_owner(owner, source)
        native = owner["native_host"]
        template_member = copy.deepcopy(native["members"][1])
        template_route = copy.deepcopy(native["routes"][0])
        native["members"] = native["members"][:1]
        native["routes"] = []
        for name, role in (("planning-parent", "parent"), ("original-a", "original"), ("original-b", "original")):
            selected = copy.deepcopy(template_member)
            selected["member"].update(member=name, role=role)
            selected["logical_owner"] = v.installed_fixture_principal(name)
            native["members"].append(selected)
            route = copy.deepcopy(template_route)
            route.update(id=name, logical_executor=selected["logical_owner"])
            route["origin"]["member"] = selected["member"]
            native["routes"].append(route)
        native["service_parent_administration"] = {"kind": "application_policy_tasks", "routes": ["original-b"]}
        source["purpose"] = self.scope
        source["acceptance_selection"] = {"parent": "planning-parent", "children": ["original-a", "original-b"],
            "late_helper": "planning-lead", "requests": {tag: request(tag) for tag in ("a", "b")},
            "public_operation": OPERATION, "arguments": {"num_stories": 1}}

    def configure_plan(self, plan):
        plan["acquisition"]["console_setup"] = {"origin": self.origin}
        plan["acquisition"]["standing_policy"] = {"installation": "human_review", "access": "provision_task_parent",
                                                   "tool_policy": "until_superseded"}
        app = plan["application"]
        specs = [copy.deepcopy(app["roster"][0])]
        for name, role in (("planning-parent", "parent"), ("original-a", "original"), ("original-b", "original")):
            specs.append({"identity": name, "profile": role, "runtime_mode_override": "turn_driven"})
        app["roster"] = specs
        app["definition"]["text"] += '''
[profiles.parent]
external_addressable = true
model = "fixture-original-model"
provider = "self_hosted"
self_hosted_server_id = "local"
runtime_mode = "turn_driven"
[profiles.parent.tools]
comms = true
[profiles.shared-planner]
model = "fixture-helper-model"
provider = "self_hosted"
self_hosted_server_id = "local"
runtime_mode = "turn_driven"
[profiles.shared-planner.tools]
comms = true
'''
        grants = sorted([{"tool_name": name, "action": "invoke", "consequence": "r2"}
            for name in (*CHECKED_SETUP, "tool_catalog_search", "tool_catalog_load", "toolkit_request_public_service")],
            key=lambda row: row["tool_name"])
        declared = {"id": "shared-planner", "purpose": "shared_workflow", "profile": "shared-planner",
                    "factory": "driver-shared-factory", "callbacks": [], "grants": grants}
        app["member_classes"] = [declared]
        plan["owner"]["native_host"]["member_classes"] = [copy.deepcopy(declared)]
        app["tool_policy"]["members"] = [app["tool_policy"]["members"][0],
            {"member_identity": "planning-parent", "grants": []},
            *[{"member_identity": f"original-{tag}", "grants": [
                {"tool_name": "find_and_install", "action": "invoke", "consequence": "r2"},
                {"tool_name": OPERATION, "action": "invoke", "consequence": "r2"}]} for tag in ("a", "b")]]
        app["tool_policy"]["members"].sort(key=lambda row: row["member_identity"])
        plan["authorities"]["controllers"] = [plan["authorities"]["controllers"][0], *[
            {"target": {"kind": "original_route", "route": name}, "issuance_id": name + "-controller"}
            for name in ("planning-parent", "original-a", "original-b")]]
        plan["authorities"]["agent_setup"] = []
        plan["acquisition"]["proposal_ceiling"]["actions"] = [
            {"action": action, "resource": name} for name in ("original-a", "original-b")
            for action in ("find_and_install", "delegate")]
        plan["acquisition"]["limits"]["setup_timeout_ms"] = 540000
        self.plan = plan

    def prepare_plan(self, directory, plan, plan_pin, source_pin):
        # The source artifact is the selected application input, not a frozen
        # reference to the default two-member roster generated before this hook.
        source = v.unique_json(Path(source_pin["path"]).read_bytes())
        source["roster"] = plan["application"]["roster"]
        source["definition"] = plan["application"]["definition"]
        source["member_classes"] = plan["application"]["member_classes"]
        updated = v.new_artifact(directory / "shared-driver-policy.json", v.canonical(source))
        plan["application"]["tool_policy"]["source"]["source_digest"] = v.installed_fixture_policy_digest(v.canonical(source))
        plan_pin = v.new_artifact(directory / "shared-driver-plan.json", v.canonical(plan))
        return plan, plan_pin, updated

    def configure_runtime(self, config, directory):
        super().configure_runtime(config, directory)
        self.configure_error_observer(config)
        # Meta's host hook may wrap an existing customizer, never add one.
        # This explicit application callback observes real construction and
        # leaves the draft untouched; native registration observes all tools.
        self.customizer = ObservedCustomizer()
        config.agent_customizer = self.customizer
        selected = [v.META_IDENTITY.DurableAgentSpec.from_dict(row) for row in self.plan["application"]["roster"]]
        class Providers:
            async def roster(self, context):
                require(context.mob_definition is not None, "actual native roster context missing")
                return selected
            async def compute_edges(self, target_identities, context):
                return []
        provider = Providers()
        config.roster_provider, config.topology_provider = provider, provider
        Path(config.access_config_path).write_text('enabled = true\nadmins = ["unrelated-fixture-admin"]\n\n[[rules]]\n'
            'id = "driver-caller-observation"\nsubjects = ["' + c.EMAIL + '"]\nactions = ["agent.view"]\n'
            'agents = ["planning-parent", "original-a", "original-b"]\n')

    def provider(self, directory, events):
        return Model(directory, events)

    def observe_preparation(self, attempt, finalizer, gateway, process, observe):
        self.finalizer = finalizer

    async def begin_workflow(self, private, provider, directory, events, process, deadline):
        # Existing embedding fixture owns its launch task. Supplied-host cases
        # override only this operation, keeping the same model/review/cleanup.
        (directory / "begin-workflow").write_bytes(b"start once\n")
        return None

    def verify_customizer(self):
        return self.customizer.require_created("planning-lead")

    def begin_model_phase(self, provider):
        require(not provider.requests, "model began before actual SDK Ready")
        provider.enabled = True

    async def cancel_a(self, private, directory, launch, deadline):
        a = next(row for row in launch["children"] if row["original"]["route"] == "original-a")
        b = next(row for row in launch["children"] if row["original"]["route"] == "original-b")
        cancelled = await asyncio.to_thread(private.cancel_original_operation,
            v.SDK.PrivateOriginalRef(**a["original"]), timeout=30)
        await wait_departed(directory, a["source_key"], deadline, remaining_source=b["source_key"], failure_observer=self.host_loop_failure)
        return {"cancel_a": dataclasses.asdict(cancelled),
                "a_before": await wait_initial_a_completion(directory, a["admission"], deadline,
                    failure_observer=self.host_loop_failure)}

    async def run(self, private, provider, directory, events, process, base_url):
        require(base_url.rstrip("/") == self.origin, "unexpected native HTTP owner")
        deadline = time.monotonic() + 540
        report = {"scope": self.scope, "status": "running", "http": [], "software_decisions": []}
        enrollment = service_request = reservation = None
        token = None
        error = None
        service_observation = None
        def http_call(method, path, body=None):
            require(time.monotonic() < deadline and process.poll() is None, "Driver acceptance owner/deadline ended")
            connection = HTTPConnection("127.0.0.1", self.port, timeout=10)
            try:
                connection.request(method, path, body=None if body is None else v.canonical(body), headers={
                    "Authorization": "Bearer " + token, "Accept": "application/json", "Origin": self.origin,
                    "Content-Type": "application/json"})
                response = connection.getresponse()
                raw = response.read(4 * 1024 * 1024 + 1)
                require(len(raw) <= 4 * 1024 * 1024, "bounded actual HTTP response exceeded")
                value = v.unique_json(raw)
                report["http"].append({"method": method, "path": path, "request": body, "status": response.status, "response": value})
                if method == "GET" and path == c.PANEL + "/data" and response.status == 503:
                    require(value == {"error": "Protected setup is unavailable"}, "unknown unavailable result")
                    return None
                require(response.status in (200, 202), "native protected handler refused")
                return value
            finally:
                connection.close()
        async def http(*args):
            return await asyncio.to_thread(http_call, *args)
        try:
            expires = int(time.time() + 650)
            scope = "shared-driver-" + uuid.uuid4().hex
            enrollment = private.begin_console_login(v.SDK.ConsoleLoginSelection(c.ISSUER, c.SUBJECT, scope, expires * 1000), timeout=10)
            await asyncio.to_thread(enrollment.result)
            token = c.signed_login(scope, expires)
            self.console_workflow_login, self.console_workflow_token = enrollment, token
            initial = await http("GET", c.PANEL + "/data")
            require(initial is not None and initial["review"] is None, "fresh selected app already has review")
            with provider.lock:
                self.begin_model_phase(provider)
            launch = await self.begin_workflow(private, provider, directory, events, process, deadline)
            if launch is not None:
                report["launch"] = launch
            cancelled = False
            answered = set()
            while time.monotonic() < deadline:
                self.host_loop_failure.require_alive()
                # Do not take the model lock while its B-context scheduling gate
                # is held; this thread must be able to cancel A and release it.
                path = directory / "driver-launch-result.json"
                if launch is None and path.exists():
                    value = v.unique_json(path.read_bytes())
                    require(value["status"] == "submitted", "real Driver commissioning failed: " + repr(value))
                    launch = value["launch"]
                    require(launch["initial_members"] == 4 and launch["initial_g_members"] == 0 and launch["native_add"] == "created",
                            "acceptance did not construct a genuinely late G")
                    report["launch"] = launch
                if launch is not None and provider.b_context.is_set() and not cancelled:
                    report.update(await self.cancel_a(private, directory, launch, deadline))
                    require("g-install" not in provider.issued, "install preceded canonical A departure")
                    cancelled = True
                    provider.release_b.set()
                require(not provider.errors, "controlled model failed before B release: " + repr(provider.errors))
                if cancelled:
                    with provider.lock:
                        require(not provider.errors, "controlled model rejected native result: " + repr(provider.errors))
                        results = copy.deepcopy(provider.results)
                        business = copy.deepcopy(provider.business_observation)
                        issued = set(provider.issued)
                    data = await http("GET", c.PANEL + "/data")
                    review = data["review"] if data else None
                    if review and review["prompt"] is not None:
                        key = (review["review_id"], review["prompt"].get("approval_id"))
                        if key not in answered:
                            require(cancelled and not answered, "unexpected extra protected installation")
                            answer = c.software_review(review, results["g-check"], OPERATION)
                            await http("POST", c.PANEL + "/answer", answer)
                            report["software_decisions"].append({"review": review, "answer": answer})
                            answered.add(key)
                    if "g-public-access" in issued and service_request is None:
                        service_request = await v.observe_installed_service_reply(private.next_service_parent_administration,
                            provider, process, deadline, failure_observer=self.host_loop_failure)
                        b = next(row for row in launch["children"] if row["original"]["route"] == "original-b")
                        report["public_proposal"] = copy.deepcopy(service_request.proposal)
                        review = report["public_policy_review"] = {}
                        prior = capture_public_review(self.finalizer, directory, service_request, review)
                        reservation = PublicReservation(self, provider, service_request, b, process, deadline, prior_policy=prior)
                        attempt = service_request.authorize_tool_policy(v.SDK.StandingRuleDecision(reservation),
                            service_request.tool_policy_sha256)
                        reservation.transferred = True
                        report["public_submit"] = await asyncio.to_thread(attempt.submit, timeout=30)
                        service_observation = await v.observe_installed_service_reply(attempt.result, provider, process, deadline,
                            failure_observer=self.host_loop_failure)
                        require(service_observation["outcome"] == "succeeded" and service_observation["custody"] == "joined"
                                and reservation.released, "actual B authority/source did not join")
                        report["public_proposal"] = service_request.proposal
                        report["public_join"] = service_observation
                    if business is not None and service_observation is not None and provider.finished.is_set():
                        b = next(row for row in launch["children"] if row["original"]["route"] == "original-b")
                        fresh = v.SDK.PrivateOriginalRef(**service_observation["source"])
                        observed = await asyncio.to_thread(private.observe_service_input_receipt, fresh, timeout=30)
                        if observed["state"] == "admitted" and observed["receipt"] is not None:
                            receipt = observed["receipt"]["report"]
                            if receipt["status"] == "finalized":
                                require(observed["session_id"] == b["admission"]["session_id"]
                                        and observed["input_id"] != b["admission"]["input_id"]
                                        and receipt["owner_input_id"] == observed["input_id"]
                                        and receipt["recipient_input_ids"] == [observed["input_id"]]
                                        and receipt["terminal"] == {"outcome_type": "consumed"}
                                        and receipt["outcome"]["completion_type"] == "completed"
                                        and receipt["outcome"]["text"] == business["answer"]
                                        and receipt["outcome"]["tool_calls"] == 3,
                                        "B answer is not its own exact finalized fresh native result")
                                a = next(row for row in launch["children"] if row["original"]["route"] == "original-a")
                                after = input_rows(directory, a["admission"]["session_id"])
                                require(len(after) == 1 and after[0]["input_id"] == report["a_before"][0]["input_id"]
                                        and after[0]["terminal_completion"] == report["a_before"][0]["terminal_completion"],
                                        "cancelled A received a result or new native input")
                                b_rows = input_rows(directory, observed["session_id"])
                                require(len(b_rows) == 2, "B did not have exactly original plus fresh continuation")
                                fresh_row = next(row for row in b_rows if row["input_id"] == observed["input_id"])
                                audit = fresh_row["authorization_audit"]
                                require(audit and all([item["input_id"] for item in row["contributors"]] == [observed["input_id"]]
                                        and row["observation"]["run_id"] == receipt["run_id"] for row in audit),
                                        "B native authorization audit has foreign contributors")
                                public_audit = public_operation_audit(audit)
                                report["public_operation_audit"] = public_audit
                                report["g_customizer"] = self.verify_customizer()
                                require(len(answered) == 1 and cancelled, "missing protected review or A departure")
                                self.host_loop_failure.require_alive()
                                report.update(status="passed", native_b=observed, business=business,
                                    b_canonical_inputs=b_rows, a_after=after, effects=workflow_rows(directory))
                                break
                self.host_loop_failure.require_alive()
                await asyncio.sleep(0.1)
            self.host_loop_failure.require_alive()
            require(report["status"] == "passed", "Driver acceptance timed out without native proof")
        except BaseException as failure:
            error = failure
            report.update(status="failed", error=repr(failure),
                selected_host_failure=self.host_loop_failure.observation())
            provider.abort_b.set()
            raise
        finally:
            provider.release_b.set()
            report["customizer_observations"] = {"builds": copy.deepcopy(self.customizer.builds),
                                                "created": copy.deepcopy(self.customizer.created)}
            try:
                if service_request is not None:
                    owned = service_request.attempt or service_request
                    report["service_retirement"] = await asyncio.to_thread(owned.retire, timeout=30)
                if enrollment is not None:
                    retired = await asyncio.to_thread(enrollment.retire, timeout=30)
                    require(retired.fenced and retired.reservations_drained and not retired.uncertain, "login retirement unconfirmed")
                    report["login_retirement"] = dataclasses.asdict(retired)
            except BaseException as cleanup:
                report.update(status="failed", cleanup_error=repr(cleanup))
                if error is None:
                    raise
            finally:
                v.save_json(directory / "console-journey.json", report)
        return report



def current_catalog_policy(finalizer, directory, evidence, *, prior_policy=None):
    """Capture one independent canonical prior before reserving a service successor.

    This fixture expects settled Active class admissions. It does not recover
    policy transitions or confer member authority. Native CAS owns later races.
    """
    finalizer.check_inputs()
    bootstrap_raw = finalizer.application_tool_policy
    # The caller supplies a successor only after full review and a joined native acceptance.
    baseline_raw = bootstrap_raw if prior_policy is None else prior_policy
    def policy(raw):
        value = v.unique_json(raw)
        keys = ("schema_version", "provider_id", "policy_id", "revision", "policy_digest", "source", "default_deny", "members")
        require(list(value) == list(keys) and type(value["schema_version"]) is int
                and value["schema_version"] == 1 and value["default_deny"] is True
                and type(value["revision"]) is int and value["revision"] > 0
                and all(isinstance(value[key], str) and value[key] for key in ("provider_id", "policy_id")),
                "canonical catalog policy contract changed")
        source = value["source"]
        require(set(source) == {"source_id", "source_digest"} and isinstance(source["source_id"], str) and source["source_id"]
                and isinstance(source["source_digest"], str) and len(source["source_digest"]) == 71
                and source["source_digest"].startswith("sha256:")
                and all(ch in "0123456789abcdef" for ch in source["source_digest"][7:]),
                "canonical catalog source provenance changed")
        payload = {key: value[key] for key in keys if key != "policy_digest"}
        require(value["policy_digest"] == v.installed_fixture_policy_digest(
                    json.dumps(payload, separators=(",", ":"), ensure_ascii=False).encode())
                and raw == json.dumps(value, separators=(",", ":"), ensure_ascii=False).encode() + b"\n",
                "canonical catalog policy bytes or digest changed")
        for row in value["members"]:
            require(set(row) == {"member_identity", "grants"} and isinstance(row["member_identity"], str)
                    and row["member_identity"] and isinstance(row["grants"], list)
                    and all(set(grant) == {"tool_name", "action", "consequence"}
                            and isinstance(grant["tool_name"], str) and grant["tool_name"]
                            and grant["action"] == "invoke" and grant["consequence"] in ("r0", "r1", "r2", "r3")
                            for grant in row["grants"]), "canonical catalog member or grant contract changed")
            tools = [grant["tool_name"] for grant in row["grants"]]
            require(tools == sorted(set(tools)), "canonical catalog grants repeated or unsorted")
        names = [row["member_identity"] for row in value["members"]]
        require(names == sorted(set(names)), "canonical catalog member identities repeated or unsorted")
        return value
    bootstrap, baseline = policy(bootstrap_raw), policy(baseline_raw)
    native = finalizer.plan["owner"]["native_host"]
    realm, owner = native["realm"], finalizer.plan["owner"]["deployment"]
    schema = "toolkit.application-service-tool-policy"
    key = "service-tool-policy-" + hashlib.sha256(v.canonical(
        [schema, bootstrap["provider_id"], bootstrap["policy_id"]])).hexdigest()
    path = (directory / "deployment/shared/data/toolkit-native" / hashlib.sha256(realm.encode()).hexdigest()
            / "originals.sqlite3").resolve(strict=True)
    evidence.update(store_path=str(path), key={"realm": realm, "owner": owner, "id": key})
    with sqlite3.connect(path.as_uri() + "?mode=ro", uri=True) as db:
        rows = db.execute("SELECT revision,digest,data FROM toolkit_records WHERE realm=? AND owner=? AND id=? LIMIT 2",
                          (realm, owner, key)).fetchall()
    require(len(rows) == 1, "exact canonical catalog slot missing or repeated")
    revision, digest, raw = rows[0]
    evidence.update(store_revision=revision, store_digest=digest, slot_json=raw)
    require(len(raw.encode()) <= 2 * 1024 * 1024, "canonical catalog slot exceeded fixture bound")
    record = v.unique_json(raw.encode())
    require(set(record) == {"schema", "version", "body"} and record["schema"] == schema
            and type(record["version"]) is int and record["version"] == 1
            and hashlib.sha256(v.canonical(record)).hexdigest() == digest,
            "canonical catalog slot schema or store digest changed")
    slot = record["body"]
    require({"bootstrap", "current", "decision", "pending"} <= slot.keys()
            and slot.keys() <= {"bootstrap", "current", "decision", "pending", "admissions", "member_decision", "member_pending"}
            and slot["bootstrap"].encode() == bootstrap_raw and slot.get("pending") is None
            and slot.get("member_pending") is None, "catalog prior has foreign bootstrap or pending transition")
    current_raw = slot["current"].encode()
    evidence.update(prior_canonical_json=slot["current"], prior_sha256=hashlib.sha256(current_raw).hexdigest())
    current = policy(current_raw)
    require(all(row[key] == bootstrap[key] for row in (baseline, current) for key in ("provider_id", "policy_id"))
            and current["revision"] >= baseline["revision"] >= bootstrap["revision"]
            and (current["revision"] != baseline["revision"] or current_raw == baseline_raw),
            "catalog prior crossed reviewed policy identity or revision")
    static = {row["member_identity"] for row in bootstrap["members"]}
    baseline_rows = {row["member_identity"]: row for row in baseline["members"]}
    require(static <= baseline_rows.keys(), "reviewed catalog baseline lost a static member")
    expected = {name: baseline_rows[name] for name in static}
    classes = native["member_classes"]
    require(classes == finalizer.plan["application"]["member_classes"]
            and len({row["id"] for row in classes}) == len(classes), "selected member classes changed or repeated")
    declared = {row["id"]: row for row in classes}
    def qualified(principal):
        return (isinstance(principal, dict) and set(principal) == {"id", "kind", "qualification"}
                and all(isinstance(principal[key], str) and principal[key].strip() == principal[key] and principal[key]
                        for key in ("id", "kind"))
                and isinstance(principal["qualification"], dict)
                and set(principal["qualification"]) == {"kind", "trust_domain_id"}
                and principal["qualification"]["kind"] == "qualified"
                and isinstance(principal["qualification"]["trust_domain_id"], str)
                and bool(principal["qualification"]["trust_domain_id"].strip()))
    seen = set()
    for admission in slot.get("admissions", []):
        selected, phase, destination = admission["instance"]["member"], admission["phase"], admission.get("destination")
        member = selected["member"]
        name, class_id = member["member"], admission["instance"]["class"]
        audience = selected["audience"]
        require(isinstance(admission["id"], str) and admission["id"] and admission["id"] not in seen
                and name not in expected and class_id in declared
                and set(admission["instance"]) == {"class", "member"} and set(member) == {"member", "mob_id", "role"}
                and selected.get("setup_capability") is None and qualified(selected["logical_owner"])
                and set(audience) == {"audience", "principal"} and audience["audience"] == "principal"
                and qualified(audience["principal"])
                and admission["class"] == declared[class_id] and member["role"] == declared[class_id]["profile"]
                and realm == "mob." + member["mob_id"] and isinstance(name, str) and name and name.strip() == name
                and type(selected["context_generation"]) is int and selected["context_generation"] > 0
                and set(phase) == {"kind", "context", "context_generation"}
                and phase["kind"] == "active" and type(phase["context_generation"]) is int
                and phase["context_generation"] >= selected["context_generation"]
                and isinstance(phase["context"], str) and phase["context"]
                and destination == {"context": phase["context"], "context_generation": phase["context_generation"]},
                "catalog prior has unselected, duplicate or non-active class admission")
        seen.add(admission["id"])
        expected[name] = {"member_identity": name, "grants": declared[class_id]["grants"]}
    require(current["members"] == [expected[name] for name in sorted(expected)]
            and all(name in expected and expected[name] == row for name, row in baseline_rows.items()),
            "catalog prior changed a reviewed row or exact admitted class materialization")
    added = expected.keys() - baseline_rows.keys()
    require(current["revision"] == baseline["revision"] + len(added)
            and (current_raw == baseline_raw if not added else
                 current["source"]["source_id"] == "toolkit-application-member-decision"),
            "catalog prior has unreviewed same-row revision or provenance drift")
    evidence["validated"] = True
    return current_raw


def capture_public_review(finalizer, directory, request_, evidence, *, prior_policy=None):
    # Retain the actual request and exact SQL snapshot even if review refuses.
    evidence.update(proposal=copy.deepcopy(request_.proposal), proposal_sha256=request_.proposal_sha256)
    require(hashlib.sha256(v.canonical(evidence["proposal"])).hexdigest() == evidence["proposal_sha256"],
            "public review proposal digest changed")
    return current_catalog_policy(finalizer, directory, evidence, prior_policy=prior_policy)


def reviewed_b_catalog_policy(proposal, catalog, session_id, prior_raw, *, member="original-b"):
    """Review the complete native successor, changing only the selected caller catalog grants."""
    prior = v.unique_json(prior_raw)
    candidate_raw = proposal["candidate_canonical_json"].encode()
    candidate = v.unique_json(candidate_raw)
    require(set(proposal) == {"provider_id", "policy_id", "member", "selection", "catalog_session",
                "prior", "candidate", "candidate_canonical_json", "lifetime"}
            and proposal["provider_id"] == prior["provider_id"] and proposal["policy_id"] == prior["policy_id"]
            and proposal["member"] == member and proposal["selection"] == {"tools": [], "catalog": "r2"}
            and proposal["catalog_session"] == session_id and catalog == "original_session"
            and proposal["lifetime"] == "until_superseded"
            and proposal["prior"] == {"revision": prior["revision"], "digest": v.installed_fixture_policy_digest(prior_raw)}
            and proposal["candidate"] == {"revision": prior["revision"] + 1,
                "digest": v.installed_fixture_policy_digest(candidate_raw)}, f"{member} catalog review crossed exact native selection")
    expected = copy.deepcopy(prior)
    expected["revision"] += 1
    members = [row for row in expected["members"] if row["member_identity"] == member]
    require(len(members) == 1 and not any(row["tool_name"] in ("tool_catalog_search", "tool_catalog_load")
            for row in members[0]["grants"]), f"{member} already had catalog permission before explicit successor")
    members[0]["grants"].extend({"tool_name": name, "action": "invoke", "consequence": "r2"}
        for name in ("tool_catalog_search", "tool_catalog_load"))
    members[0]["grants"].sort(key=lambda row: row["tool_name"])
    source = candidate["source"]
    require(set(source) == {"source_id", "source_digest"}
            and source["source_id"] == "toolkit-application-service-decision"
            and isinstance(source["source_digest"], str) and len(source["source_digest"]) == 71
            and source["source_digest"].startswith("sha256:")
            and all(ch in "0123456789abcdef" for ch in source["source_digest"][7:]), f"{member} candidate source is not native decision provenance")
    expected["source"] = source
    payload = {key: expected[key] for key in ("schema_version", "provider_id", "policy_id", "revision", "source", "default_deny", "members")}
    expected["policy_digest"] = v.installed_fixture_policy_digest(json.dumps(payload, separators=(",", ":"), ensure_ascii=False).encode())
    require(candidate == expected and candidate_raw == json.dumps(expected, separators=(",", ":"), ensure_ascii=False).encode() + b"\n",
            f"{member} catalog successor changed another member, permission or canonical byte")


class PublicReservation:
    """One explicit finite application rule, held until the real SDK joins it."""
    def __init__(self, scenario, provider, request_, child, process, deadline, *, tag="b",
                 operation=OPERATION, readback_key="g-reconcile", prior_policy):
        self.scenario, self.provider, self.request, self.child = scenario, provider, request_, child
        self.process, self.deadline = process, deadline
        require(tag in ("a", "b"), "unknown reviewed caller")
        self.tag, self.operation, self.readback_key = tag, operation, readback_key
        self.prior_policy = prior_policy  # Immutable independently checked snapshot; native CAS owns publication.
        self.exclusion = threading.Lock()
        require(self.exclusion.acquire(False), "finite reservation already held")
        self.released = self.transferred = False
        self.proposal_sha256 = request_.proposal_sha256
        self.authorization_ref = "driver-public-" + tag + "-" + self.proposal_sha256
        self.provenance = {"owner": "explicit Driver acceptance rule", "source": scenario.finalizer.source_pin,
            "scope": f"one {tag} {operation} operation and exact complete catalog successor"}
        self.check_current()

    def check_current(self):
        try:
            require(self.exclusion.locked() and not self.released and self.process.poll() is None
                    and time.monotonic() < self.deadline, "finite reservation no longer current")
            f, proposal = self.scenario.finalizer, self.request.proposal
            f.check_inputs()
            require(hashlib.sha256(v.canonical(proposal)).hexdigest() == self.proposal_sha256,
                    "actual proposal bytes changed")
            for key in ("activation", "policy_manifest", "local_policy"):
                v.exact_bytes(Path(f.selection[key]["path"]), f.selection[key]["sha256"])
            require((Path(f.selection["deployment_root"]) / "current").resolve() == Path(f.selection["activation"]["path"]), "selected app changed")
            decision = proposal["decision"]
            native = f.plan["owner"]["native_host"]
            require(proposal["original"] == self.child["original"] and proposal["operator"] == native["remote_operator"]
                    and proposal["application"] == native["grant_administrator"] and proposal["represented_subject"] == c.human()
                    and proposal["source_policy_sha256"] == f.source_pin["sha256"]
                    and proposal["instruction_sha256"] == hashlib.sha256(self.scenario.request(self.tag).encode()).hexdigest()
                    and proposal["expires_in_ms"] == 180000, f"original-{self.tag} proposal crossed source/operator/application")
            installed = self.provider.results[self.readback_key]["observation"]["result"]
            servers = installed["mcp_configuration"]["mcpServers"]
            require(len(servers) == 1, "expected one installed HN service")
            server = next(iter(servers))
            permission = decision["permission"]
            destination = decision["live_attachment"]["destination"]
            require(decision["route"] == "original-" + self.tag and decision["choice"] == "approve"
                    and permission["target"] == "original-" + self.tag and permission["connection"] == server
                    and permission["resource"] == server and permission["operations"] == [self.operation]
                    and permission["lifetime"]["scope"] == {"kind": "task", "task": decision["decision"]}
                    and destination["context"] == self.child["admission"]["session_id"]
                    and destination["logical_owner"] == v.installed_fixture_principal("original-" + self.tag)
                    and destination["audience"] == {"audience": "principal", "principal": c.human()},
                    f"original-{self.tag} permission expanded target, operation, task or audience")
            reviewed_b_catalog_policy(proposal["tool_policy"], decision["catalog"],
                self.child["admission"]["session_id"], self.prior_policy, member="original-" + self.tag)
        except BaseException as failure:
            raise v.SDK.HostReplyError(v.SDK.HostReplyCode.DENIED) from failure

    def release(self):
        require(self.transferred and not self.released and self.exclusion.locked(), "reservation release repeated or premature")
        self.released = True
        self.exclusion.release()


def public_operation_audit(records, tool_name=OPERATION, call_id="b-public-read"):
    label = f"{tool_name}/{call_id}"
    prepared = [row for row in records if row["observation"]["observation"]["kind"] == "prepared"
        and row["observation"]["observation"]["target"].get("kind") == "tool"
        and row["observation"]["observation"]["target"].get("tool_name") == tool_name
        and row["observation"]["observation"]["target"].get("call_id") == call_id]
    require(prepared and len({row["observation"]["operation_id"] for row in prepared}) == 1,
        f"{label}: exact tool preparation audit missing or has multiple operation IDs")
    operation = prepared[0]["observation"]["operation_id"]
    require(isinstance(operation, str) and operation, f"{label}: missing operation identity")
    selected = [row for row in records if row["observation"]["operation_id"] == operation]
    envelope = copy.deepcopy(prepared[0])
    envelope["observation"].pop("observation")
    envelope = v.canonical(envelope)
    baseline, previous_sequence = None, None
    # Core current() can prepare the same binding again after publication changes.
    # Validate all its preparations, including a target that no longer matches
    # the initial call filter. Keep every observation and all authority fields.
    for row in selected:
        comparable = copy.deepcopy(row)
        event = comparable["observation"].pop("observation")
        require(v.canonical(comparable) == envelope,
            f"{label}: operation audit binding or contributors changed")
        if event["kind"] != "prepared":
            continue
        comparable["observation"]["observation"] = event
        policy = event.get("policy")
        require(isinstance(policy, dict), f"{label}: missing preparation policy observation")
        sequence, observed_at = policy.pop("publication_sequence", None), policy.pop("observed_at_ms", None)
        require(all(type(value) is int and 0 <= value < 2**64 for value in (sequence, observed_at)),
            f"{label}: invalid preparation policy observation counters")
        instance = policy.get("publication_instance")
        require(isinstance(instance, str) and instance, f"{label}: missing publication instance")
        comparable = v.canonical(comparable)
        require(baseline is None or comparable == baseline,
            f"{label}: preparation changed operation, target, contributors or authority")
        require(previous_sequence is None or sequence >= previous_sequence,
            f"{label}: preparation publication sequence regressed")
        # observed_at_ms is host wall time, not the clock's monotonic Instant.
        baseline, previous_sequence = comparable, sequence
    entered = [row["observation"]["observation"] for row in selected]
    require(any(row["kind"] == "entry" for row in entered)
        and any(row["kind"] == "outcome" and row["outcome"] == {
            "kind": "tool_dispatch_returned", "asynchronous_operations": [],
            "result_is_error": False, "terminal_error": None} for row in entered)
        and not any(row["kind"] in ("refused", "authorization_unavailable") for row in entered),
        f"{label}: tool did not reach its own successful native dispatch outcome")
    return {"operation_id": operation, "observations": entered}


async def wait_initial_a_completion(directory, admitted, deadline, *, failure_observer=None):
    # Cancellation concerns the pending setup task. The original's waiting
    # answer is allowed to finish; it cannot later receive B's service result.
    while time.monotonic() < deadline:
        if failure_observer is not None:
            failure_observer.require_alive()
        rows = input_rows(directory, admitted["session_id"])
        if failure_observer is not None:
            failure_observer.require_alive()
        require(len(rows) == 1 and rows[0]["input_id"] == admitted["input_id"], "A gained another native input")
        if rows[0].get("terminal_outcome") is not None and rows[0].get("terminal_completion") is not None:
            return rows
        await asyncio.sleep(0.05)
    if failure_observer is not None:
        failure_observer.require_alive()
    raise AssertionError("A original waiting input did not actually complete")



def input_rows(directory, session):
    path = directory / "state/runtime.sqlite"
    with sqlite3.connect(path.resolve(strict=True).as_uri() + "?mode=ro", uri=True) as db:
        values = db.execute("SELECT state_json FROM runtime_input_states WHERE runtime_id=? ORDER BY input_id",
                            ("rt:session:" + session,)).fetchall()
    return [json.loads(value[0]) for value in values]


def workflow_rows(directory):
    path = directory / "deployment/shared/data/toolkit-native" / hashlib.sha256(b"mob.fixture").hexdigest() / "originals.sqlite3"
    with sqlite3.connect(path.resolve(strict=True).as_uri() + "?mode=ro", uri=True) as db:
        rows = db.execute("SELECT owner,id,data FROM toolkit_records WHERE realm=? ORDER BY owner,id LIMIT 257", ("mob.fixture",)).fetchall()
    require(len(rows) < 257, "bounded canonical evidence exceeded")
    return [{"owner": owner, "id": ident, "data": json.loads(raw)} for owner, ident, raw in rows
            if json.loads(raw)["schema"] in ("meerkat_toolkit.setup_workflow", "meerkat_toolkit.acquisition")]


async def wait_departed(directory, source_key, deadline, *, remaining_source, failure_observer=None):
    require(source_key != remaining_source, "A and B must have independent canonical sources")
    while time.monotonic() < deadline:
        if failure_observer is not None:
            failure_observer.require_alive()
        workflows = [row["data"]["body"] for row in workflow_rows(directory) if row["data"]["schema"] == "meerkat_toolkit.setup_workflow"]
        if failure_observer is not None:
            failure_observer.require_alive()
        if len(workflows) == 1 and len(workflows[0]["waiters"]) == 2:
            rows = list(workflows[0]["waiters"].values())
            # Source key ID is an opaque host derivation. Read exact retained
            # source correlation below, not a route-name substring heuristic.
            sources = [row for row in rows if row["identity"]["source"] == source_key]
            if len(sources) == 1 and sources[0]["departed"] is True:
                remaining = [row for row in rows if row["identity"]["source"] == remaining_source]
                require(len(remaining) == 1 and remaining[0]["departed"] is False
                        and workflows[0]["sealed"] is False,
                        "A departure must preserve exact active B in an open workflow")
                return workflows[0]
        await asyncio.sleep(0.05)
    if failure_observer is not None:
        failure_observer.require_alive()
    raise AssertionError("actual canonical A departure not observed")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--acceptance-host", required=True, type=Path)
    parser.add_argument("--acceptance-host-sha256", required=True)
    parser.add_argument("--resources-sha256", required=True)
    parser.add_argument("--toolkit-python", required=True, type=Path)
    parser.add_argument("--meta-python", required=True, type=Path)
    parser.add_argument("--output", required=True, type=Path)
    parser.add_argument("--allow-public-hn-read", action="store_true", required=True)
    args = parser.parse_args()
    for name in ("acceptance_host", "toolkit_python", "meta_python", "output"):
        path = getattr(args, name)
        require(path.is_absolute(), "all selected paths must be absolute")
        setattr(args, name, path.resolve())
    require(args.acceptance_host.parent.name == "bin" and os.access(args.acceptance_host, os.X_OK)
            and v.digest(args.acceptance_host) == args.acceptance_host_sha256, "exact staged acceptance example required")
    prefix = args.acceptance_host.parent.parent
    resources = prefix / "libexec/meerkat-connect/resources.json"
    manifest = v.unique_json(v.exact_bytes(resources, args.resources_sha256))
    require(not args.output.exists() and not args.output.is_relative_to(prefix), "fresh separate evidence output required")
    args.output.mkdir(parents=True)
    sys.path[:0] = [str(args.toolkit_python), str(args.meta_python)]
    for name, global_name, root in (("meerkat_toolkit.host_control", "SDK", args.toolkit_python),
        ("meerkat_toolkit.application_runtime", "APP_RUNTIME", args.toolkit_python),
        ("meerkat_meta.builder", "META_BUILDER", args.meta_python),
        ("meerkat_meta.identity_first_models", "META_IDENTITY", args.meta_python)):
        module = importlib.import_module(name)
        require(Path(module.__file__).resolve().is_relative_to(root), "SDK import escaped selected source")
        setattr(v, global_name, module)
    candidate = {"kind": "acceptance_example", "example": "shared_driver_acceptance", "binary": v.artifact_pin(args.acceptance_host),
        "resources_sha256": args.resources_sha256, "claim": "same production Driver embedding, not stock gateway CLI"}
    args.toolkit_gateway = args.acceptance_host
    result = asyncio.run(v._run_installed_product(args, candidate, prefix, manifest,
        corrupt=False, console=Scenario(v, c.loopback_origin())))
    v.save_json(args.output / "RESULT.json", result)
    return 0 if result["status"] == "passed" else 1


if __name__ == "__main__":
    raise SystemExit(main())
