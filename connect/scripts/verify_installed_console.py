"""Installed Console HTTP acceptance with a controlled issuer and model.

Loaded explicitly by verify_native_gateway_subprocess.py under python -I.
Automatic cases do not claim browser or deployed login acceptance. The opt-in
Browser case retains the selected shipping login front and operator actions.
No account, private B-input receipt or service-event qualification is claimed. Native verifies the fixture JWT and owns admission,
review, attachment, execution and shutdown. Importing this module does no I/O.
"""
from __future__ import annotations

import asyncio
import base64
import dataclasses
import hashlib
import hmac
from http.client import HTTPConnection
import json
from pathlib import Path
import socket
import time
import uuid
from urllib.parse import urlencode, urlsplit

# Native permits HS256 only for development issuer and JWKS hosts.
ISSUER = "https://installed-console-fixture.localhost"
SUBJECT = "installed-console-human"
EMAIL = "human@installed-console-fixture.invalid"
AUDIENCE = "installed-console-acceptance"
# Deliberately public fixture key, never an account credential or production key.
KEY = b"installed-console-controlled-issuer-only-20261012"
PANEL = "/host/toolkit/setup"


def require(value, message):
    if not value:
        raise AssertionError(message)


def nonempty(value):
    return isinstance(value, str) and bool(value.strip())


def human():
    return {"kind": "human", "id": SUBJECT, "qualification": {
        "kind": "qualified", "trust_domain_id": "oidc-issuer:" + ISSUER}}


def loopback_origin():
    # Select before preparing the pinned policy. A competing bind is a failure,
    # not permission to rewrite that selection or retry on a different listener.
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return "http://127.0.0.1:" + str(probe.getsockname()[1])


def signed_login(scope, expires):
    def part(value):
        raw = json.dumps(value, separators=(",", ":"), sort_keys=True).encode()
        return base64.urlsafe_b64encode(raw).rstrip(b"=")
    header = part({"alg": "HS256", "typ": "JWT"})
    claims = part({"iss": ISSUER, "aud": AUDIENCE, "sub": SUBJECT,
        "email": EMAIL, "email_verified": True, "sid": scope, "exp": expires})
    body = header + b"." + claims
    return (body + b"." + base64.urlsafe_b64encode(
        hmac.new(KEY, body, hashlib.sha256).digest()).rstrip(b"=")).decode()


def approval_answer(review):
    require(type(review.get("review_id")) is int and review["review_id"] > 0,
            "actual review id missing")
    prompt = review["prompt"]
    require(prompt["kind"] == "approval" and nonempty(prompt["approval_id"])
            and sorted(prompt["allowed_decisions"]) == ["approve", "deny"],
            "actual explicit human approval prompt missing")
    return {"review_id": review["review_id"], "approval_id": prompt["approval_id"],
            "decision": "approve"}


def software_review(review, checked, operation, *, selection=None):
    answer = approval_answer(review)
    body = review["prompt"]["body"]
    if selection is None:
        selection = {"family": "agno", "profile": "hackernews",
                     "operations": [operation], "values": {"timeout": 7}}
    require(review["can_start"] is False and body["kind"] == "service_software_install"
            and body["check_tool"] == "setup_check" and body["scope"] == "software_installation_only"
            and body["owner_proposal"] == checked
            and body["selection"] == selection,
            "human software review differs from the actual checked package")
    return answer


def original_work_reference(value, source_authority):
    # Native OriginalWorkRef is source-owned work, not a run or session ID.
    return (isinstance(value, dict) and set(value) == {"authority", "work"}
            and value["authority"] == source_authority and nonempty(value["work"]))


def reviewed_original_catalog(request, decision, accepted, prior_raw, verifier, *, expected_selection=None):
    """Join the reviewed whole successor to the actual native bootstrap bytes."""
    wrapper = request["member_tool_policy"]
    proposal = wrapper["proposal"]
    require(set(wrapper) == {"proposal", "approval_scope", "lifetime", "catalog_visibility", "native_service_scope"}
            and wrapper["approval_scope"] == "the complete successor policy, including preserved permissions for all listed members"
            and wrapper["lifetime"] == "until superseded; this member policy remains after the service task ends"
            and wrapper["catalog_visibility"] == "when selected, eligible tool metadata in the entire original conversation, not only this integration"
            and wrapper["native_service_scope"] == "the separate native service and catalog grants still expire with this task",
            "human catalog review omitted its complete policy or visibility scope")
    verifier.reviewed_original_catalog_policy(proposal, decision["catalog"],
        accepted["session_id"], prior_raw, expected_selection=expected_selection)


def public_review(review, accepted, results, operation, original_owner, route, task_scope, source_authority,
                  *, prior_tool_policy=None, verifier=None, principal=None, expected_tool_policy=None):
    answer = approval_answer(review)
    principal = human() if principal is None else principal
    body = review["prompt"]["body"]
    request, decision = body["access_request"], body["service_access"]
    context = request["service_instruction"]
    permission = decision["permission"]
    destination = decision["live_attachment"]["destination"]
    installed = results["reconciled-install"]["observation"]["result"]
    servers = installed["mcp_configuration"]["mcpServers"]
    require(len(servers) == 1, "expected the actual single installed public service")
    server = next(iter(servers))
    fresh = context["fresh_original"]
    require(nonempty(fresh) and context["business_task"] == {**task_scope,
                "id": results["original-request"]["operation"]}
            and original_work_reference(context["prior_original"], source_authority),
            "actual original-task references missing")
    selection = {"operations": [operation], "agent": "original", "expires_in_ms": None}
    if prior_tool_policy is not None:
        selection["tool_policy"] = ({"tools": [], "catalog": "r2"}
                                    if expected_tool_policy is None else expected_tool_policy)
        reviewed_original_catalog(request, decision, accepted, prior_tool_policy, verifier,
                                  expected_selection=expected_tool_policy)
    require(request["software"]["job_id"] == results["installed-job"]["job"]["id"]
            and request["software"]["family"] == "agno" and request["software"]["profile"] == "hackernews"
            and request["software"]["server_name"] == server
            and request["phase"] == "installed_public_service_access"
            and request["authentication"] == {"kind": "none"}
            and request["application_selection"] == selection
            and decision["route"] == route and decision["decision"] == fresh
            and decision["choice"] == "approve"
            and permission["target"] == "original" and permission["connection"] == server
            and permission["resource"] == server and permission["operations"] == [operation]
            and permission["lifetime"]["scope"] == {"kind": "task", "task": fresh}
            and type(permission["lifetime"]["expires_at_ms"]) is int
            and destination["context"] == accepted["session_id"]
            and destination["logical_owner"] == original_owner
            and destination["audience"] == {"audience": "principal", "principal": principal}
            and context["live_attachment"] == decision["live_attachment"]
            and context["operator_and_requester"] == principal
            and context["represented_subject"] == principal,
            "human public review crossed the actual task, service, operation or audience")
    require(nonempty(context["proposed_fresh_instruction"]), "reviewed fresh instruction missing")
    return answer, context


def frame_record(value):
    """Unwrap native frames; never use UI's permissive correlation fallback."""
    require(isinstance(value, dict), "timeline frame is not an object")
    for _ in range(3):
        if isinstance(value.get("type"), str) and "frame" in value:
            value = value["frame"]
        elif value.get("kind") == "frame_updated":
            value = value["payload"]["frame"]
        else:
            break
        require(isinstance(value, dict), "invalid nested native frame")
    require(nonempty(value.get("id")) and isinstance(value.get("payload"), dict),
            "native frame identity/payload missing")
    return value


def run_id(frame):
    require(nonempty(frame.get("source_event_id")) and frame["source_event_id"] == frame["id"],
            "native frame does not retain its original source event id")
    value = frame.get("run_id")
    lineage = frame["payload"].get("identity")
    require(nonempty(value) and isinstance(lineage, dict) and lineage.get("run_id") == value,
            "native run lineage missing or contradictory")
    return value


def input_text(frame):
    value = frame["payload"].get("input", {})
    if value.get("kind") != "content":
        return None
    content = value.get("content")
    if isinstance(content, str):
        return content
    if (isinstance(content, list) and len(content) == 1
            and content[0].get("type") == "text" and isinstance(content[0].get("text"), str)):
        return content[0]["text"]
    return None


def native_original_pair(frames, live, accepted, original_request, fresh_request, *, fresh_context=None):
    """Join the accepted Send anchor to at most two ordered native run starts."""
    # The accepted Send owns its reservation anchor. Each native RunStarted
    # owns its input interaction lineage, which can differ from that reservation.
    # Preserve an absent native interaction ID rather than inventing one.
    positions = {f["id"]: i for i, f in enumerate(frames)}
    require(len(positions) == len(frames), "duplicate native source frame")
    anchors = [f for f in frames if f["id"] == accepted["input_frame_id"]]
    if not anchors:
        return None
    anchor = anchors[0]
    require(anchor.get("kind") == "user_input" and anchor.get("source", {}).get("kind") == "send"
            and anchor.get("identity") == accepted["identity"]
            and anchor.get("session_id") == accepted["session_id"]
            and anchor.get("interaction_id") == accepted["interaction_id"]
            and anchor.get("status") in ("accepted", "delivered")
            and anchor["payload"].get("content") == original_request,
            "accepted Console input anchor differs from its exact request or owner")
    sends = [f for f in frames if f.get("kind") == "user_input"
             and f.get("source", {}).get("kind") == "send"
             and f.get("identity") == accepted["identity"]
             and f.get("session_id") == accepted["session_id"]]
    require(len(sends) == 1, "another Console send entered the fresh fixture session")
    starts = [f for f in live if f.get("kind") == "run_started"
              and f.get("session_id") == accepted["session_id"]]
    require(len(starts) <= 2, "duplicate native original or fresh run")
    runs = []
    interactions = {}
    for index, frame in enumerate(starts):
        run = run_id(frame)
        runs.append(run)
        interaction = frame["payload"]["identity"].get("interaction_id")
        require(interaction is None or nonempty(interaction), "invalid native run interaction")
        interactions[run] = interaction
        expected = original_request if index == 0 else fresh_request
        if index == 1 and fresh_context is not None:
            # The caller binds this exact context from the checked human review
            # before submitting approval. Never infer it from observed input.
            require(nonempty(fresh_context), "reviewed fresh host context is missing")
            matches_input = frame["payload"].get("input") == {"kind": "content", "content": [
                {"type": "text", "text": fresh_context}, {"type": "text", "text": expected}]}
        else:
            matches_input = input_text(frame) == expected
        require(frame["payload"].get("source_event_type") == "run_started"
                and frame["payload"].get("session_id") == accepted["session_id"]
                and matches_input
                and positions[anchor["id"]] < positions[frame["id"]],
                "native original/fresh input differs from the accepted session or request")
    require(len(set(runs)) == len(runs), "fresh instruction reused the original native run")
    for frame in live:
        if frame.get("run_id") in runs:
            run_id(frame)
            require(frame.get("interaction_id") == frame["payload"]["identity"].get("interaction_id")
                    and frame.get("interaction_id") == interactions[frame["run_id"]],
                    "native run contradicts its RunStarted interaction lineage")
    if len(starts) < 2:
        return None
    a, b = starts[:1], starts[1:]
    a_run, b_run = runs
    return a, b, a_run, b_run, positions


def completed_journey(frames, accepted, context, business, verifier, *, fresh_context=None):
    """Return only a complete native A -> reviewed fresh B -> real result join.

    Native tool events inherit active-run lineage but have no session field.
    Their exact run is joined to session-bearing native RunStarted/RunCompleted.
    Missing observations return None; contradictory selected facts are failures.
    """
    live = [f for f in frames if f.get("source", {}).get("kind") == "console_event"
            and f.get("identity") == accepted["identity"]]
    for frame in live:
        if (frame.get("kind") in ("interaction_failed", "run_failed")
                and (frame.get("session_id") == accepted["session_id"]
                     or frame.get("interaction_id") == accepted["interaction_id"])):
            raise AssertionError("the actual original Console interaction failed")
    if context is None or business is None:
        return None
    pair = native_original_pair(frames, live, accepted, verifier.ACQUISITION_REQUEST,
                                context["proposed_fresh_instruction"], fresh_context=fresh_context)
    if pair is None:
        return None
    a, b, a_run, b_run, positions = pair

    def complete(run, text):
        candidates = [f for f in live if f.get("kind") == "interaction_complete"
            and f["payload"].get("source_event_type") == "run_completed"
            and f.get("run_id") == run]
        require(len(candidates) <= 1, "duplicate native completed run")
        if not candidates:
            return None
        frame = candidates[0]
        require(run_id(frame) == run and frame.get("session_id") == accepted["session_id"]
            and frame["payload"].get("session_id") == accepted["session_id"]
            and frame.get("status") == "completed" and frame["payload"]["result"] == text
            and frame["payload"].get("extraction_required", False) is False
            and nonempty(frame["payload"].get("assistant_message_id")),
            "native completed run lacks exact session, committed answer or final status")
        return frame
    a_done = complete(a_run, verifier.ORIGINAL_A_FINISH)
    b_done = complete(b_run, business["answer"])
    selected = [f for f in live if f.get("run_id") == b_run
                and f["payload"].get("id") == verifier.ORIGINAL_STORY_CALL]
    calls = [f for f in selected if f.get("kind") == "tool_call_requested"]
    results = [f for f in selected if f.get("kind") == "tool_result_received"]
    require(len(calls) <= 1 and len(results) <= 1, "public operation was replayed")
    if not a_done or not b_done or not calls or not results:
        return None
    for frame in calls + results:
        require(run_id(frame) == b_run and frame["payload"]["name"] == verifier.ACQUISITION_OPERATION,
                "native public tool result belongs to a different run or operation")
    require(calls[0]["payload"]["args"] == {"num_stories": 1}
            and results[0]["payload"]["is_error"] is False, "public operation did not succeed")
    blocks = results[0]["payload"]["content"]
    require(isinstance(blocks, list) and len(blocks) == 2
            and isinstance(blocks[0], dict) and set(blocks[0]) == {"type", "text"}
            and blocks[0]["type"] == "text" and isinstance(blocks[0]["text"], str)
            and isinstance(blocks[1], dict) and set(blocks[1]) == {"type", "data"}
            and blocks[1]["type"] == "structured",
            "expected actual Agno text and structured native result blocks")
    # Match Core's ContentBlock text projection without discarding structured data.
    projected = blocks[0]["text"] + "\n" + verifier.canonical(blocks[1]["data"]).decode()
    require(projected == business["actual_tool_result"]
            and verifier.installed_public_story(projected) == business["story"],
            "native result differs from the original's real public answer")
    catalog = business.get("original_catalog")
    if catalog is not None:
        previous = positions[b[0]["id"]]
        for prefix, tool, args in (("search", "tool_catalog_search", {"query": verifier.ACQUISITION_OPERATION, "limit": 50}),
                                  ("load", "tool_catalog_load", {"names": [verifier.ACQUISITION_OPERATION]})):
            identifier = "original-catalog-" + prefix
            selected = [f for f in live if f.get("run_id") == b_run and f["payload"].get("id") == identifier]
            invoked = [f for f in selected if f.get("kind") == "tool_call_requested"]
            returned = [f for f in selected if f.get("kind") == "tool_result_received"]
            require(len(invoked) <= 1 and len(returned) <= 1, "original catalog operation was replayed")
            if not invoked or not returned:
                return None
            require(all(run_id(f) == b_run and f["payload"]["name"] == tool for f in invoked + returned)
                    and invoked[0]["payload"]["args"] == args and returned[0]["payload"]["is_error"] is False,
                    "native original catalog operation differs from its selected call")
            content = returned[0]["payload"]["content"]
            require(isinstance(content, list) and content and all(block.get("type") == "text"
                    and isinstance(block.get("text"), str) for block in content), "catalog result lacks text blocks")
            require(verifier.unique_json("\n".join(block["text"] for block in content).encode())
                    == catalog[prefix + "_result"], "native catalog receipt differs from the original's result")
            require(previous < positions[invoked[0]["id"]] < positions[returned[0]["id"]]
                    < positions[calls[0]["id"]], "original catalog did not precede the exact business call")
            previous = positions[returned[0]["id"]]
    require(positions[a[0]["id"]] < positions[a_done["id"]] < positions[b[0]["id"]]
            < positions[calls[0]["id"]] < positions[results[0]["id"]] < positions[b_done["id"]],
            "native original/fresh/tool/answer order differs")
    return {"identity": accepted["identity"], "session_id": accepted["session_id"],
        "initial_interaction_id": accepted["interaction_id"], "a_run_id": a_run, "b_run_id": b_run,
        "a_completion_frame": a_done["id"], "b_completion_frame": b_done["id"],
        "tool_call_frame": calls[0]["id"], "tool_result_frame": results[0]["id"],
        "assistant_message_id": b_done["payload"]["assistant_message_id"],
        "answer": business["answer"], "story": business["story"],
        "private_b_input_receipt": "not_requested_or_claimed"}


class ConsoleScenario:
    name = "installed-console-success"
    scope = ("actual installed native Console HTTP and mounted setup handlers; controlled issuer/model; "
             "human decisions and acquired public operation with same-session native completed answer; "
             "no browser, deployed login, live account or service-event acceptance claim")

    def __init__(self, verifier, origin):
        parsed = urlsplit(origin)
        require(parsed.scheme == "http" and parsed.hostname == "127.0.0.1" and parsed.port
                and origin == "http://127.0.0.1:" + str(parsed.port), "fixed fixture origin required")
        self.v, self.origin, self.port = verifier, origin, parsed.port

    def human(self):
        return human()

    def wire_value(self, value):
        return value

    def configure_owner(self, owner, source):
        native = owner["native_host"]
        # SetupScope::key is the existing canonical acquisition store owner.
        self.task_scope = {"realm": native["realm"], "owner": "toolkit/acquisition/" + owner["deployment"]}
        original = next(row for row in native["members"] if row["member"]["member"] == "original")
        original["audience"] = {"audience": "principal", "principal": self.human()}
        route = native["routes"][0]
        require(route["id"] == self.v.ACQUISITION_ROUTE and len(native["routes"]) == 1,
                "expected exact existing original route")
        self.source_authority = route["source_authority"]
        route["origin"] = {"kind": "local_console", "member": original["member"],
            "requester": self.human(), "ingress_actor": route["source_authority"]}
        route["represented_subject"] = self.human()
        owner["components"], owner["channels"] = [], []
        native.pop("service_parent_administration")
        # Preserve the configured native issuer; the verified human reviews.
        source.pop("finite_service_rule")
        source["purpose"] = "controlled-issuer installed native Console human acquisition"
        source["console_fixture"] = {"issuer": ISSUER, "subject": SUBJECT,
            "audience": AUDIENCE, "principal": self.human(), "origin": self.origin}

    def configure_plan(self, plan):
        plan["acquisition"]["console_setup"] = {"origin": self.origin}
        plan["acquisition"].pop("standing_policy")

    def native_admin_subjects(self):
        return ["unrelated-fixture-admin"]

    def report_value(self, value):
        return value

    def check_response(self, value):
        pass

    def check_final_artifacts(self, directory, report):
        pass

    def original_request(self):
        return self.v.ACQUISITION_REQUEST

    def software_answer(self, review, checked):
        return software_review(review, checked, self.v.ACQUISITION_OPERATION)

    def checked_access_review(self, review, accepted, results):
        answer, context = public_review(review, accepted, results, self.v.ACQUISITION_OPERATION,
            self.v.installed_fixture_principal("original-agent"), self.v.ACQUISITION_ROUTE,
            self.task_scope, self.source_authority,
            prior_tool_policy=self.application_tool_policy, verifier=self.v, principal=self.human(),
            expected_tool_policy={"tools": [{"name": self.v.ACQUISITION_OPERATION, "consequence": "r2"}],
                                  "catalog": "r2"})
        require(context["proposed_fresh_instruction"] == self.v.ACQUISITION_REQUEST,
                "human review changed the controlled original request")
        return answer, context

    def bind_access_context(self, review, accepted, results, provider):
        # Bind only from the independently checked public review, before its
        # answer can enter native. This is an oracle expectation, not authority.
        _, context = self.checked_access_review(review, accepted, results)
        expected = self.v.installed_approved_service_context({
            "decision": review["prompt"]["body"]["service_access"],
            "acquisition": context["business_task"]})
        encoded = self.v.canonical(expected).decode()
        with provider.lock:
            require(provider.expected_service_context in (None, expected)
                    and getattr(self, "_approved_service_context", None) in (None, encoded),
                    "approved service context changed after selection")
            provider.expected_service_context = expected
            self._approved_service_context = encoded

    async def access_answer(self, http, review, accepted, results, report):
        answer, context = self.checked_access_review(review, accepted, results)
        return answer, context, PANEL + "/answer", "public_access"

    def completed(self, frames, accepted, context, business):
        if business is not None:
            require(business.get("original_catalog", {}).get("operation") == self.v.ACQUISITION_OPERATION,
                    "original public completion lacks its own catalog receipts")
        fresh_context = getattr(self, "_approved_service_context", None)
        require(context is None or fresh_context is not None,
                "approved service context was not selected before submission")
        return completed_journey(frames, accepted, context, business, self.v,
                                 fresh_context=fresh_context)

    def finish_report(self, report, results, provider, completion):
        require(len(report["decisions"]) == 2 and len(report["starts"]) == 1
                and results.get("public-task-access") == provider.public_reply(),
                "native public response or explicit human reviews missing")
        report.update(status="passed", completion=completion,
                      public_reply=results["public-task-access"])

    def configure_runtime(self, config, directory):
        config.console_read_only, config.console_require_app_auth = False, True
        config.http_listen, config.http_public_base_url = "127.0.0.1:" + str(self.port), self.origin
        config.auth_config = {"provider": "jwt", "shared_secret": KEY.decode(),
            "issuer": ISSUER, "audience": AUDIENCE, "email_allowlist": [EMAIL]}
        access = directory / "console-access.toml"
        access.write_text('enabled = true\nadmins = ' + json.dumps(self.native_admin_subjects()) + '\n\n[[rules]]\n'
            'id = "controlled-console-original"\nsubjects = ["' + EMAIL + '"]\n'
            'actions = ["agent.view", "agent.send"]\nagents = ["original"]\n')
        panel = directory / "console.toml"
        panel.write_text('[[sidebar.buttons]]\nid = "connect-setup"\nlabel = "Connect setup"\npanel = "' + PANEL + '"\n')
        config.access_config_path, config.console_config_path = str(access), str(panel)

    def provider(self, directory, events):
        class HumanProvider(self.v.InstalledAcquisitionProvider):
            original_catalog = True
            allow_fresh_helper_search = True

            def access_arguments(self, job_id, raw):
                return {**super().access_arguments(job_id, raw),
                        "tool_policy": {"tools": [{"name": name, "consequence": "r2"} for name in raw],
                                        "catalog": "r2"}}

            def public_reply(self):
                return {"state": "reviewed", "access_reviewed": True, "grant_issued": True,
                        "fresh_instruction_retained": True, "ready": False}
        return HumanProvider(directory, events)

    async def run(self, private, provider, directory, events, process, base_url):
        require(base_url.rstrip("/") == self.origin, "native listener differs from pinned Console origin")
        v = self.v
        report = {"scope": self.scope, "status": "running", "http": [], "timeline": [],
                  "decisions": [], "starts": [], "login_retirement": "not_observed"}
        frames, by_id, context = [], {}, None
        expires = int(time.time() + v.ACQUISITION_LIMIT + 60)
        scope = "installed-console-" + uuid.uuid4().hex
        enrollment = None
        deadline = time.monotonic() + v.ACQUISITION_LIMIT
        original_error = None
        token = None

        def request(method, path, body=None, *, bearer=True, origin=None, expected=200, text=False, status_poll=False):
            if status_poll:
                require(method == "GET" and path == PANEL + "/data" and body is None and expected == 200 and not text,
                        "pending allowance is only for the existing status poll")
            require(time.monotonic() < deadline and process.poll() is None,
                    "actual Console owner or acquisition deadline ended")
            connection = HTTPConnection("127.0.0.1", self.port,
                timeout=min(10.0, max(0.1, deadline - time.monotonic())))
            headers = {"Accept": "text/html" if text else "application/json"}
            if bearer:
                headers["Authorization"] = "Bearer " + token
            if body is not None:
                headers.update({"Origin": self.origin if origin is None else origin,
                                "Content-Type": "application/json"})
            try:
                connection.request(method, path, body=None if body is None else v.canonical(body), headers=headers)
                response = connection.getresponse()
                raw = response.read(4 * 1024 * 1024 + 1)
                require(len(raw) <= 4 * 1024 * 1024, "Console response exceeds fixture bound")
                value = raw.decode() if text else v.unique_json(raw)
                # Never retain the bearer token or Authorization header.
                report["http"].append({"method": method, "path": path, "request": self.report_value(body),
                    "status": response.status, "response": self.report_value(value)})
                self.check_response(value)
                if status_poll and response.status == 503:
                    require(value == {"error": "Protected setup is unavailable"}, "malformed unavailable status response")
                    return None
                require(response.status == expected, "actual " + path + " returned " + str(response.status))
                return value
            finally:
                connection.close()

        async def http(*args, **kwargs):
            return await asyncio.to_thread(request, *args, **kwargs)

        try:
            enrollment = private.begin_console_login(v.SDK.ConsoleLoginSelection(
                issuer=ISSUER, subject=SUBJECT, login_scope_id=scope, expires_at_ms=expires * 1000), timeout=10.0)
            await asyncio.to_thread(enrollment.result)
            report["enrollment"] = {"id": enrollment.enrollment_id, "acknowledged": True,
                "issuer": ISSUER, "subject": SUBJECT, "login_scope_id": scope,
                "meaning": "parent lifetime only; native request verification authenticates the human"}
            token = signed_login(scope, expires)
            await http("GET", PANEL + "/data", bearer=False, expected=401)
            experience = await http("GET", "/console/experience")
            buttons = experience["console_config"]["sidebar"]["buttons"]
            require(sum(button.get("panel") == PANEL for button in buttons) == 1,
                    "native Console did not project the actual docked setup panel")
            await http("GET", PANEL, text=True)
            await http("GET", PANEL + "/panel.js", text=True)
            initial = await http("GET", PANEL + "/data")
            require(initial["review"] is None, "fresh Console already has a setup review")
            with provider.lock:
                require(not provider.requests and not provider.errors, "model work preceded human original")
                provider.enabled = True
            accepted = await http("POST", "/console/send", {"identity": "original",
                "content": self.original_request(), "origin": "console", "origin_kind": "operator",
                "idempotency_key": "installed-console-" + uuid.uuid4().hex})
            require(accepted["status"] == "accepted" and accepted["identity"] == "original"
                    and all(nonempty(accepted.get(key)) for key in ("session_id", "interaction_id", "input_frame_id")),
                    "Console did not return the actual original interaction/session")
            report["accepted"] = accepted
            cursor = None
            answered = set()
            while time.monotonic() < deadline:
                with provider.lock:
                    require(not provider.errors, "controlled model rejected actual native request")
                    results = dict(provider.results)
                    business = provider.business_observation
                page = await http("GET", "/console/timeline?" + urlencode({"identity": "original",
                    "mode": "since", "limit": 400, **({"after": cursor} if cursor else {})}))
                require(page.get("available", True) is True and isinstance(page.get("frames"), list),
                        "native Console timeline unavailable")
                report["timeline"].append(page)
                for raw in page["frames"]:
                    frame = frame_record(raw)
                    previous = by_id.get(frame["id"])
                    if previous is None:
                        require(len(frames) < 4000, "native timeline exceeds fixture bound")
                        by_id[frame["id"]] = len(frames)
                        frames.append(frame)
                    else:
                        old = frames[previous]
                        require(all(old.get(key) == frame.get(key) for key in
                            ("identity", "session_id", "run_id", "kind", "source", "source_event_id")),
                            "native updated frame changed its owner")
                        frames[previous] = frame
                if page["frames"]:
                    cursor = page.get("next_cursor") or page.get("latest_cursor")
                    require(nonempty(cursor), "native page omitted continuation cursor")
                completion = self.completed(frames, accepted, context, business)
                if completion is not None and provider.finished.is_set():
                    self.finish_report(report, results, provider, completion)
                    break
                data = await http("GET", PANEL + "/data", status_poll=True)
                if data is None:
                    # Only the next existing bounded observation is eligible.
                    # Never replay Send, Start or an approval after a failed POST.
                    await asyncio.sleep(0.1)
                    continue
                review = data["review"]
                if review is not None:
                    if review["can_start"]:
                        require(len(report["decisions"]) == 1 and not report["starts"]
                                and review["prompt"] is None, "unexpected public setup start")
                        await http("POST", PANEL + "/start", {"review_id": review["review_id"]}, expected=202)
                        report["starts"].append(review["review_id"])
                    elif review["prompt"] is not None:
                        prompt = review["prompt"]
                        key = (review["review_id"], prompt.get("approval_id"))
                        if key not in answered:
                            if not report["decisions"]:
                                answer = self.software_answer(review, results["checked-install"])
                                answer_path = PANEL + "/answer"
                                kind = "software"
                            else:
                                require(len(report["decisions"]) == 1 and report["starts"] == [review["review_id"]],
                                        "unexpected additional human review")
                                answer, context, answer_path, kind = await self.access_answer(
                                    http, review, accepted, results, report)
                                if kind == "public_access":
                                    self.bind_access_context(review, accepted, results, provider)
                            if not report["decisions"]:
                                await http("POST", PANEL + "/answer", answer,
                                    origin="https://wrong-origin.invalid", expected=403)
                            await http("POST", answer_path, answer, expected=202)
                            report["decisions"].append({"kind": kind, "prompt": self.report_value(review),
                                                       "answer": self.report_value(answer),
                                                       "observation": "forwarded; outcome checked separately"})
                            answered.add(key)
                await asyncio.sleep(0.1)
            require(report["status"] == "passed", "same-original native Console completion was not observed")
        except BaseException as error:
            original_error = error
            report.update(status="failed", error=self.report_value(repr(error)))
            raise
        finally:
            if enrollment is not None:
                try:
                    retired = await asyncio.to_thread(enrollment.retire, timeout=10.0)
                    report["login_retirement"] = dataclasses.asdict(retired)
                    require(retired.fenced and retired.reservations_drained and not retired.uncertain,
                            "parent Console login retirement is unconfirmed")
                except BaseException as error:
                    report.update(status="failed", retirement_error=self.report_value(repr(error)))
                    if original_error is None:
                        v.save_json(directory / "console-journey.json", report)
                        raise
            self.check_response(report)
            v.save_json(directory / "console-journey.json", report)
        events.add("installed_console_completed", **report["completion"])
        return report


class BrowserConsoleScenario(ConsoleScenario):
    """Operator drives the shipping front; this fixture observes public facts.

    No observer request submits, approves, starts or reads with borrowed login
    credentials. A Browser refresh supplies the final durable timeline page.
    """
    name = "installed-console-browser-success"
    scope = ("installed public Console through the selected shipping Forge login front; "
             "operator Browser Send, approvals and Start; actual native same-session completion "
             "and joined shutdown required; screenshots/rendering retained separately; "
             "controlled model, no credential entry or live account claim")

    def __init__(self, verifier, origin, source_path, source_sha256):
        super().__init__(verifier, origin)
        # The execution owner supplies a pinned shipping-source receipt. This
        # does not freeze acceptance to an older front missing a shipped route.
        raw = verifier.exact_bytes(source_path, expected=source_sha256)
        self.source_receipt = verifier.bytes_pin(source_path, raw)
        selected = verifier.unique_json(raw)
        rows = selected["forge_files"]
        names = ("console_configuration", "console_auth", "native_console_login")
        self.forge_sources = []
        selected_paths, selected_bytes = {}, {}
        for name in names:
            matches = [row for row in rows if row["repository_path"] ==
                       "installer/launchkit/templates_v2/" + name + ".py"]
            require(len(matches) == 1, "selected Forge module is not unique")
            row = matches[0]
            path = Path(row["path"])
            require(path.is_absolute() and not path.is_symlink(), "selected Forge module path")
            data = verifier.exact_bytes(path, expected=row["sha256"])
            require(len(data) == row["bytes"], "selected Forge module length")
            selected_paths[name], selected_bytes[name] = path, data
            self.forge_sources.append(verifier.bytes_pin(path, data))
        require(len({path.parent for path in selected_paths.values()}) == 1,
                "shipping Forge modules do not share one package")
        # Explicit package loading preserves ConsoleConnectLogin class identity
        # across the unchanged front and its unchanged native enrollment owner.
        import importlib.util
        import sys
        import types
        package_name = "connect_selected_browser_forge"
        require(package_name not in sys.modules, "Forge source package already loaded")
        package = types.ModuleType(package_name)
        package.__path__ = [str(selected_paths["console_auth"].parent)]
        sys.modules[package_name] = package
        modules = {}
        for name in names:
            qualified = package_name + "." + name
            spec = importlib.util.spec_from_file_location(qualified, selected_paths[name])
            module = importlib.util.module_from_spec(spec)
            sys.modules[qualified] = module
            exec(compile(selected_bytes[name], str(selected_paths[name]), "exec"), module.__dict__)
            require(Path(module.__file__).resolve() == selected_paths[name].resolve(),
                    "Forge module import escaped the selected file")
            modules[name] = module
        self.auth, self.login_module = modules["console_auth"], modules["native_console_login"]
        require(PANEL + "/data" in self.auth.TOOLKIT_SETUP_PATHS
                and all(PANEL + suffix in self.auth.TOOLKIT_SETUP_POST_LIMITS
                        for suffix in ("/data", "/answer", "/start")),
                "selected shipping front lacks a required public panel route")
        self.front = None
        self.password = None

    def human(self):
        return {"kind": "human", "id": EMAIL, "qualification": {
            "kind": "qualified", "trust_domain_id": "oidc-issuer:" + self.auth.ISSUER}}

    def configure_owner(self, owner, source):
        super().configure_owner(owner, source)
        source["purpose"] = "shipping Forge browser login and installed native Console acquisition"
        source["console_fixture"] = {"issuer": self.auth.ISSUER, "subject": EMAIL,
            "audience": self.auth.AUDIENCE, "principal": self.human(), "origin": self.origin}

    def configure_runtime(self, config, directory):
        super().configure_runtime(config, directory)
        root = directory / "forge-console"
        root.mkdir(mode=0o700)
        # Reuse the deliberately public fixture value for this test-only login.
        # The unchanged shipping front still creates its own random signing key.
        self.password = KEY.decode("ascii")
        self.front = self.auth.ConsoleFront(root, self.port, EMAIL, self.password, runtime_namespace="meta")
        config.http_listen = "127.0.0.1:0"
        config.http_public_base_url = self.origin
        config.auth_config = self.front.native_auth

    def wire_value(self, value):
        if isinstance(value, str):
            for secret in (self.front.key if self.front else None,):
                if secret:
                    value = value.replace(secret, "[PRIVATE_FORGE_VALUE]")
            return value
        if isinstance(value, list):
            return [self.wire_value(item) for item in value]
        if isinstance(value, dict):
            return {self.wire_value(key): self.wire_value(item) for key, item in value.items()}
        return value

    def check_final_artifacts(self, directory, report):
        self.v.exact_bytes(Path(self.source_receipt["path"]), expected=self.source_receipt["sha256"])
        for row in self.forge_sources:
            require(len(self.v.exact_bytes(Path(row["path"]), expected=row["sha256"])) == row["bytes"],
                    "selected shipping Forge source changed")
        require(self.wire_value(report) == report, "private Forge value reached report")
        for name in ("events.json", "model-requests.json", "console-journey.json", "gateway.stderr"):
            path = directory / name
            if path.exists():
                raw = self.v.exact_bytes(path, minimum=0)
                require(all(not secret or secret.encode() not in raw
                            for secret in (self.front.key if self.front else None,)),
                        "private Forge value reached retained evidence")

    @staticmethod
    def cleanup_category(error):
        # Only closed labels reach evidence. Exception text, task names and
        # login identifiers may contain private values.
        if isinstance(error, asyncio.CancelledError):
            return "cancelled"
        if isinstance(error, TimeoutError):
            return "timeout"
        if getattr(error, "code", None) == "cleanup_unconfirmed":
            return "native_login_cleanup_unconfirmed"
        for message, category in (
            ("Console subscription cleanup is unconfirmed.", "subscription_cleanup_unconfirmed"),
            ("Console upstream cleanup is unconfirmed.", "upstream_cleanup_unconfirmed"),
            ("Console access retirement is unconfirmed.", "access_retirement_unconfirmed"),
            ("Console access cleanup is unconfirmed.", "access_cleanup_unconfirmed"),
            ("Console login cleanup is unconfirmed.", "login_cleanup_unconfirmed"),
        ):
            if isinstance(error, RuntimeError) and error.args == (message,):
                return category
        if isinstance(error, AssertionError):
            return "assertion_failed"
        return "other_error"

    @classmethod
    def cleanup_children(cls, front, front_logins, subscriptions):
        # Observe the original retained tasks only. Never await, cancel, retry
        # or replace them. Counts keep the output bounded by fixed categories.
        rows = {}
        def observe(role, task):
            if task is None:
                return
            if not task.done():
                category = "pending"
            elif task.cancelled():
                category = "cancelled"
            else:
                error = task.exception()
                category = "succeeded" if error is None else cls.cleanup_category(error)
            counts = rows.setdefault(role, {})
            counts[category] = counts.get(category, 0) + 1
        observe("front_close", getattr(front, "_close_task", None))
        observe("server", getattr(front, "task", None))
        for login in front_logins:
            for role in ("retirement_task", "connect_close", "controller_review_close",
                         "console_login_close", "controller_review_start"):
                observe(role, getattr(login, role, None))
        for subscription in subscriptions:
            observe("subscription", subscription.task)
            observe("subscription_close", subscription.close_task)
        return rows

    async def run(self, private, provider, directory, events, process, base_url):
        v, front = self.v, self.front
        require(front is not None and base_url.rstrip("/") != self.origin,
                "shipping front and native listener must be distinct")
        report = {"scope": self.scope, "status": "running", "http": [], "front_routes": [],
                  "timeline": [], "decisions": [], "starts": [], "login_retirement": [],
                  "forge_source_receipt": self.source_receipt, "forge_sources": self.forge_sources}
        observations, inputs, logins = [], [], []
        frames, by_id, by_sequence, reviews = [], {}, {}, {}
        accepted = context = None
        status_pending = False
        observer_errors = []
        deadline = time.monotonic() + v.ACQUISITION_LIMIT
        release = directory / "browser-release.json"
        original_error = None

        def login_owner(login):
            owner = self.login_module.NativeConsoleLogin(private, login, issuer=self.auth.ISSUER,
                is_current=lambda: process.poll() is None and time.monotonic() < deadline)
            logins.append(owner)
            return owner

        def admitted(event):
            nonlocal accepted
            event = dataclasses.asdict(event)
            require(not inputs and event["channel"] == "host-authenticated-console"
                    and event["principal"] == EMAIL and event["identity"] == "original"
                    and event["content"] == self.original_request(),
                    "Browser Send changed the selected public input or human")
            inputs.append(event)
            accepted = {key: event[key] for key in
                        ("identity", "interaction_id", "session_id", "input_frame_id",
                         "cursor", "conversation_id")}
            report["accepted"] = accepted

        async def observed_response(response):
            # Observe bounded, public fixture JSON only. Never retain headers,
            # login bodies, cookies, credential routes or an SSE body.
            request = response.request
            path, method = request.url.path, request.method
            body = None
            if path not in (PANEL + "/data", PANEL + "/answer", PANEL + "/start", "/console/rpc"):
                return
            try:
                if method == "POST":
                    require(len(request.content) <= 65536, "public Browser request bound")
                    body = v.unique_json(request.content)
                if path == "/console/rpc":
                    if body.get("method") != "meta/console/query_timeline":
                        return
                    if body.get("params", {}).get("identity") != "original":
                        return
                raw = await response.aread()
                require(len(raw) <= 4 * 1024 * 1024, "public Browser response bound")
                value = v.unique_json(raw)
                require(len(observations) < 2000, "public Browser observation bound")
                observations.append({"method": method, "path": path, "request": body,
                                     "status": response.status_code, "response": value})
                if path == PANEL + "/data" and response.status_code == 200:
                    review = value["review"]
                    prompt = review.get("prompt") if review is not None else None
                    if (prompt is not None and prompt.get("body", {}).get("access_request", {})
                            .get("phase") == "installed_public_service_access"):
                        require(accepted is not None, "access review preceded accepted Browser Send")
                        with provider.lock:
                            results = dict(provider.results)
                        # httpx awaits this hook before the front releases the
                        # review to the browser. An answer-response hook is too
                        # late: B can run before that response is observed.
                        self.bind_access_context(review, accepted, results, provider)
            except Exception as error:
                observer_errors.append({"method": method, "path": path, "error_type": type(error).__name__})
                raise

        async def consume_observations(results):
            nonlocal frames, context, status_pending
            while observations:
                observation = observations.pop(0)
                report["http"].append(observation)
                path, method, body, value = (observation[key] for key in ("path", "method", "request", "response"))
                if path == PANEL + "/data" and method in ("GET", "POST") and observation["status"] == 503:
                    require(value == {"error": "Protected setup is unavailable"},
                            "malformed unavailable Browser status response")
                    status_pending = True
                    continue
                require(observation["status"] == (202 if path in (PANEL + "/answer", PANEL + "/start") else 200),
                        "observed public Browser request failed")
                if path == PANEL + "/data":
                    review = value["review"]
                    if review is not None:
                        key = (review["review_id"], review["prompt"].get("approval_id") if review["prompt"] else None)
                        reviews[key] = review
                    status_pending = False
                elif path == PANEL + "/answer":
                    require(accepted is not None, "approval preceded accepted Browser Send")
                    review = reviews[(body["review_id"], body["approval_id"])]
                    if not report["decisions"]:
                        expected, kind = self.software_answer(review, results["checked-install"]), "software"
                    else:
                        require(len(report["decisions"]) == 1 and report["starts"] == [review["review_id"]],
                                "unexpected Browser approval sequence")
                        expected, context, _, kind = await self.access_answer(None, review, accepted, results, report)
                    require(body == expected, "Browser approval differs from actual reviewed proposal")
                    report["decisions"].append({"kind": kind, "prompt": review, "answer": body,
                                               "observation": "forwarded; outcome checked separately"})
                elif path == PANEL + "/start":
                    review = reviews[(body["review_id"], None)]
                    require(len(report["decisions"]) == 1 and not report["starts"]
                            and review["can_start"] is True and review["prompt"] is None
                            and body == {"review_id": review["review_id"]}, "unexpected Browser Start")
                    report["starts"].append(review["review_id"])
                else:
                    require("error" not in value and value["id"] == body["id"], "native timeline RPC refused")
                    page = value["result"]
                    require(page.get("available", True) is True and isinstance(page.get("frames"), list),
                            "Browser native timeline unavailable")
                    report["timeline"].append(page)
                    for raw in page["frames"]:
                        frame = frame_record(raw)
                        cursor = frame.get("cursor")
                        require(isinstance(cursor, str) and cursor.startswith("console:")
                                and 1 <= len(cursor[8:]) <= 20 and cursor[8:].isascii()
                                and cursor[8:].isdigit(), "Browser native cursor lacks its sequence")
                        sequence = int(cursor[8:])
                        require(sequence <= 2**64 - 1 and cursor == f"console:{sequence}"
                                and by_sequence.get(sequence, frame["id"]) == frame["id"],
                                "Browser native cursor sequence is contradictory")
                        previous = by_id.get(frame["id"])
                        if previous is None:
                            require(len(by_id) < 4000, "Browser native timeline exceeds fixture bound")
                        else:
                            require(all(previous.get(key) == frame.get(key) for key in
                                ("cursor", "identity", "session_id", "run_id", "kind", "source", "source_event_id")),
                                "Browser updated timeline frame changed its owner")
                        by_id[frame["id"]] = frame
                        by_sequence[sequence] = frame["id"]
            # Selected native ConsoleCursor::from_seq uses console:<u64>;
            # Recent and Since windows use that numeric store order. Older
            # pages may arrive later, so HTTP completion never orders A/B.
            frames = [by_id[identifier] for _, identifier in sorted(by_sequence.items())]

        original_application = front.application
        def observed_application(native_url):
            application = original_application(native_url)
            front.client.event_hooks["response"].append(observed_response)
            async def observed(scope, receive, send):
                async def sent(message):
                    if scope["type"] == "http" and message["type"] == "http.response.start":
                        if len(report["front_routes"]) < 2000:
                            report["front_routes"].append({"method": scope["method"],
                                "path": scope["path"], "status": message["status"]})
                        else:
                            observer_errors.append({"error_type": "front_route_bound"})
                    await send(message)
                await application(scope, receive, sent)
            return observed
        front.application = observed_application
        front.console_login_factory, front.on_input = login_owner, admitted
        try:
            with provider.lock:
                require(not provider.requests and not provider.errors, "model work preceded Browser Send")
                provider.enabled = True
            await front.start(base_url)
            v.save_json(directory / "browser-ready.json", {"login_url": self.origin + "/login",
                "console_url": self.origin + "/console", "panel_path": PANEL,
                "fixture_login": {"principal": EMAIL, "password": self.password,
                    "scope": "deliberately public test-only value, never an account or deployment password"},
                "identity": "original",
                "original_request": self.original_request(), "release_file": str(release),
                "deadline_unix_seconds": time.time() + max(0, deadline - time.monotonic()),
                "instructions": "Use the real login form, Send once, open Connect setup, approve software, Start once, approve task access. After the final answer, refresh Console history without sending again. Retain Browser evidence, then write {\"finish\":true} to release_file."})
            while time.monotonic() < deadline:
                require(process.poll() is None and not front.task.done(), "Browser owner exited during observation")
                require(not observer_errors and not front.failures, "shipping front or public observation failed")
                require(len(inputs) <= 1, "Browser admitted an additional original input")
                with provider.lock:
                    require(not provider.errors, "controlled model rejected actual Browser native request")
                    results, business = dict(provider.results), provider.business_observation
                await consume_observations(results)
                completion = self.completed(frames, accepted, context, business) if accepted else None
                if completion is not None and provider.finished.is_set() and not status_pending:
                    require(any(row == {"method": "GET", "path": "/login", "status": 200}
                                for row in report["front_routes"])
                            and sum(row == {"method": "POST", "path": "/login", "status": 303}
                                    for row in report["front_routes"]) == 1 and len(logins) == 1,
                            "actual shipping Browser login was not observed once")
                    answers = [row for row in report["front_routes"] if row["path"] == PANEL + "/answer"]
                    starts = [row for row in report["front_routes"] if row["path"] == PANEL + "/start"]
                    require(answers == [{"method": "POST", "path": PANEL + "/answer", "status": 202}] * 2
                            and starts == [{"method": "POST", "path": PANEL + "/start", "status": 202}]
                            and any(row == {"method": "GET", "path": PANEL, "status": 200}
                                    for row in report["front_routes"])
                            and any(row == {"method": "POST", "path": PANEL + "/data", "status": 200}
                                    for row in report["front_routes"]),
                            "shipping front did not return the selected panel, polling and action responses")
                    if report["status"] != "passed":
                        self.finish_report(report, results, provider, completion)
                        v.save_json(directory / "browser-completion.json", completion)
                    # This file only releases observation/cleanup after the
                    # native result; it cannot submit or approve anything.
                    if release.exists():
                        released = v.unique_json(v.exact_bytes(release))
                        require(set(released) == {"finish"} and released["finish"] is True,
                                "invalid Browser observation release")
                        report["operator_observation_released"] = True
                        break
                await asyncio.sleep(0.1)
            require(report.get("operator_observation_released") is True,
                    "bounded Browser observation did not complete and release")
        except BaseException as error:
            original_error = error
            report.update(status="failed", error_type=type(error).__name__, error=self.wire_value(str(error)))
            raise
        finally:
            report["observation_errors"] = observer_errors
            cleanup_phase = "front_close"
            # Retain references before successful cleanup clears owner lists.
            front_logins = tuple(getattr(front, "_logins", {}).values())
            subscriptions = tuple(item for login in front_logins for item in login.subscriptions)
            try:
                front_error = login_error = None
                try:
                    await front.close()
                except BaseException as error:
                    front_error = error
                # front.close already owns these retirements. Observe every
                # retained result even if its aggregate close failed.
                for owner in logins:
                    try:
                        retired = await owner.close()  # The same idempotent, joined owner result.
                        if retired is not None:
                            report["login_retirement"].append(dataclasses.asdict(retired))
                        require(retired is not None and retired.fenced and retired.reservations_drained
                                and not retired.uncertain, "shipping login retirement unconfirmed")
                    except BaseException as error:
                        if login_error is None:
                            login_error = error
                        counts = report.setdefault("login_retirement_failures", {})
                        category = self.cleanup_category(error)
                        counts[category] = counts.get(category, 0) + 1
                report["cleanup_children"] = self.cleanup_children(front, front_logins, subscriptions)
                if front_error is not None:
                    raise front_error
                cleanup_phase = "login_retirement"
                if login_error is not None:
                    raise login_error
                report["front_joined"] = True
                cleanup_phase = "post_close_observation"
                if original_error is None:
                    require(not observer_errors and not front.failures and len(inputs) == 1
                            and len(logins) == 1, "late Browser observation, login or input changed the result")
                    with provider.lock:
                        require(not provider.errors, "late controlled model failure")
                        results, business = dict(provider.results), provider.business_observation
                    await consume_observations(results)
                    require(not status_pending, "Browser status remained unavailable after completion")
                    require(self.completed(frames, accepted, context, business) == report["completion"],
                            "late Browser timeline observation changed completion")
                    for suffix, count in (("/answer", 2), ("/start", 1)):
                        returned = [row for row in report["front_routes"] if row["path"] == PANEL + suffix]
                        require(returned == [{"method": "POST", "path": PANEL + suffix, "status": 202}] * count,
                                "late Browser action changed the result")
            except BaseException as error:
                report.update(status="failed", front_joined=report.get("front_joined", False),
                              cleanup_failure_phase=cleanup_phase, cleanup_error_type=type(error).__name__,
                              cleanup_failure_category=self.cleanup_category(error))
                if original_error is None:
                    v.save_json(directory / "console-journey.json", self.wire_value(report))
                    raise
            v.save_json(directory / "console-journey.json", self.wire_value(report))
        events.add("installed_console_browser_completed", **report["completion"])
        return report


# A distinct setup-only fixture. No value appears in this source, the model
# request, the checked package configuration or the native preparation plan.
TRELLO_SLOTS = ("TOOLKIT_TRELLO_FIXTURE_KEY", "TOOLKIT_TRELLO_FIXTURE_TOKEN")
TRELLO_OPERATION = "list_boards"
TRELLO_SELECTION = {"family": "agno", "profile": "trello", "operations": [TRELLO_OPERATION],
    "values": {"timeout": 7, "api_key_env": TRELLO_SLOTS[0], "token_env": TRELLO_SLOTS[1]}}
TRELLO_REQUEST = (
    "Install the checked Agno Trello profile with only list_boards and timeout 7. "
    "Use TOOLKIT_TRELLO_FIXTURE_KEY and TOOLKIT_TRELLO_FIXTURE_TOKEN as the two declared credential references. "
    "Request original-task access through toolkit_request_service_account with declared_secret_slots. "
    "The human supplies both controlled credentials only in the protected setup panel. "
    "Save the reviewed configuration and credentials, attach the service to this same original task, "
    "then report the actual setup result. Do not call Trello or claim account access was tested."
)
TRELLO_FINISH = ("The reviewed Trello setup is attached to this original task. "
    "No Trello operation was called and no provider account was verified.")


def completed_credential_setup(frames, accepted, context, business, verifier, *, fresh_context=None):
    """Join native setup-only A/fresh B runs without a provider operation."""
    live = [frame for frame in frames if frame.get("source", {}).get("kind") == "console_event"
            and frame.get("identity") == accepted["identity"]]
    for frame in live:
        if (frame.get("kind") in ("interaction_failed", "run_failed")
                and (frame.get("session_id") == accepted["session_id"]
                     or frame.get("interaction_id") == accepted["interaction_id"])):
            raise AssertionError("the actual credential setup original interaction failed")
        require(not (frame.get("kind") in ("tool_call_requested", "tool_result_received")
                     and frame["payload"].get("name") == TRELLO_OPERATION),
                "setup-only acceptance entered a vendor operation")
    if context is None or business is None:
        return None
    pair = native_original_pair(frames, live, accepted, TRELLO_REQUEST,
                                context["proposed_fresh_instruction"], fresh_context=fresh_context)
    if pair is None:
        return None
    first, fresh, a_run, b_run, positions = pair
    require(not any(frame.get("run_id") == b_run
                    and frame.get("kind") in ("tool_call_requested", "tool_result_received")
                    for frame in live), "setup-only continuation made a tool call")

    def completed(run, answer):
        selected = [frame for frame in live if frame.get("kind") == "interaction_complete"
            and frame["payload"].get("source_event_type") == "run_completed"
            and frame.get("run_id") == run]
        require(len(selected) <= 1, "duplicate native setup completion")
        if not selected:
            return None
        frame = selected[0]
        require(run_id(frame) == run and frame.get("session_id") == accepted["session_id"]
                and frame["payload"].get("session_id") == accepted["session_id"]
                and frame.get("status") == "completed" and frame["payload"]["result"] == answer
                and frame["payload"].get("extraction_required", False) is False
                and nonempty(frame["payload"].get("assistant_message_id")),
                "native setup completion lacks exact session, committed answer or final status")
        return frame

    a_done, b_done = completed(a_run, verifier.ORIGINAL_A_FINISH), completed(b_run, TRELLO_FINISH)
    if a_done is None or b_done is None:
        return None
    require(business["answer"] == TRELLO_FINISH and business["attached_operation"] == TRELLO_OPERATION,
            "setup completion differs from actual original model observation")
    require(positions[first[0]["id"]] < positions[a_done["id"]] < positions[fresh[0]["id"]]
            < positions[b_done["id"]], "native setup original/fresh/answer order differs")
    return {"identity": accepted["identity"], "session_id": accepted["session_id"],
        "initial_interaction_id": accepted["interaction_id"], "a_run_id": a_run, "b_run_id": b_run,
        "a_completion_frame": a_done["id"], "b_completion_frame": b_done["id"],
        "assistant_message_id": b_done["payload"]["assistant_message_id"], "answer": TRELLO_FINISH,
        "attached_operation": TRELLO_OPERATION, "vendor_operation": "not_requested_or_observed",
        "account_verification": "not_requested_or_claimed", "private_b_input_receipt": "not_requested_or_claimed"}


class TrelloConsoleScenario(ConsoleScenario):
    """Setup-only canary scenario requiring the authored N1 native outcomes.

    It shares the real Console/login/cleanup flow and never calls a vendor
    operation. The exact native owner and consumer must be selected together.
    """
    name = "installed-console-credentials-success"
    scope = ("controlled two-credential Agno Trello setup through installed native Console; "
             "native persistence, attachment and same-original completion required; "
             "no vendor operation, account verification or service readiness claim")

    def __init__(self, verifier, origin):
        super().__init__(verifier, origin)
        self._credential_values = {slot: "controlled-canary-" + uuid.uuid4().hex
                                   for slot in TRELLO_SLOTS}
        self._persistence_proposal = None
        self._credential_service_context = None

    def original_request(self):
        return TRELLO_REQUEST

    def completed(self, frames, accepted, context, business):
        require(context is None or self._credential_service_context is not None,
                "credential service context was not selected before submission")
        return completed_credential_setup(frames, accepted, context, business, self.v,
            fresh_context=(self.v.canonical(self._credential_service_context).decode()
                           if self._credential_service_context is not None else None))

    def report_value(self, value):
        if isinstance(value, str):
            for secret in self._credential_values.values():
                value = value.replace(secret, "[REDACTED]")
            return value
        if isinstance(value, list):
            return [self.report_value(item) for item in value]
        if isinstance(value, dict):
            return {self.report_value(key): self.report_value(item) for key, item in value.items()}
        return value

    def check_response(self, value):
        require(self.report_value(value) == value, "controlled credential escaped protected input")

    def check_final_artifacts(self, directory, report):
        self.check_response(report)
        # events.json includes the retained SDK protocol observations. Scan
        # evidence only; native credential storage intentionally holds values.
        for name in ("model-requests.json", "events.json", "console-journey.json",
                     "gateway.stderr", "original-business-observation.json"):
            path = directory / name
            if path.exists():
                raw = self.v.exact_bytes(path, minimum=0)
                require(all(secret.encode() not in raw for secret in self._credential_values.values()),
                        "controlled credential escaped retained evidence")

    def native_admin_subjects(self):
        # Native Console resolves its verified auth.email into AccessView.
        # This is the actual host access configuration, not a Toolkit grant.
        return [EMAIL]

    def configure_owner(self, owner, source):
        super().configure_owner(owner, source)
        native = owner["native_host"]
        # Exact trusted fixture selections, never taken from a returned prompt.
        self._review_owner = {"operator": native["remote_operator"],
            "application": native["grant_administrator"], "realm": native["realm"],
            "route": self.v.ACQUISITION_ROUTE, "purpose": "operations",
            "represented_subject": human()}
        source["console_fixture"]["credential_setup"] = {
            "selection": TRELLO_SELECTION, "request": TRELLO_REQUEST,
            "authentication": {"kind": "declared_secret_slots"},
            "native_admin_subject": EMAIL, "vendor_operation": "not_requested",
        }

    def configure_plan(self, plan):
        super().configure_plan(plan)
        members = plan["application"]["tool_policy"]["members"]
        helper = next(row for row in members if row["member_identity"] == "helper")
        public = [row for row in helper["grants"] if row["tool_name"] == "toolkit_request_public_service"]
        require(len(public) == 1, "expected one existing helper public-request classification")
        public[0]["tool_name"] = "toolkit_request_service_account"
        original = next(row for row in members if row["member_identity"] == "original")
        operation = [row for row in original["grants"] if row["tool_name"] == self.v.ACQUISITION_OPERATION]
        require(len(operation) == 1, "expected one existing original operation classification")
        operation[0]["tool_name"] = TRELLO_OPERATION
        report = plan["authorities"]["helper_report"]
        require(report.pop("public_issuance_id") == "helper-public-request-parent"
                and "account_issuance_id" not in report, "unexpected existing helper account authority")
        report["account_issuance_id"] = "helper-account-request-parent"
        setup = plan["authorities"]["agent_setup"]
        require(len(setup) == 1, "expected exact original setup authority")
        targets = setup[0]["helper_targets"]
        public = [row for row in targets if row.get("target") == {
            "kind": "registered_tool_bundle_operation", "bundle": "meerkat_toolkit.setup_accounts",
            "operation": "toolkit_request_public_service"}]
        require(len(public) == 1, "expected one actual public request target")
        public[0]["target"]["operation"] = "toolkit_request_service_account"
        public[0]["issuance_id"] = "original-helper-account-ceiling"

    def software_answer(self, review, checked):
        return software_review(review, checked, TRELLO_OPERATION, selection=TRELLO_SELECTION)

    def credential_answer(self, review, accepted, results):
        answer = approval_answer(review)
        self.check_response(review)
        require(review["can_start"] is False, "credential review is not ready for an answer")
        body = review["prompt"]["body"]
        require(all(body.get(key) == value for key, value in self._review_owner.items()),
                "credential review changed its actual application, route or represented human")
        request, decision = body["access_request"], body["service_access"]
        context = request["service_instruction"]
        checked = results["checked-install"]
        job = results["installed-job"]["job"]
        observation = results["reconciled-install"]["observation"]
        installed = observation["result"]
        servers = installed["mcp_configuration"]["mcpServers"]
        require(job["status"] == "succeeded" and observation["status"] == "matched"
                and observation["job_id"] == job["id"] and len(servers) == 1
                and installed["family"] == "agno" and installed["profile"] == "trello",
                "credential review lacks its actual reconciled Trello installation")
        server = next(iter(servers))
        launch = servers[server]
        require(isinstance(launch, dict) and set(launch) <= {"command", "args", "env", "secret_env"}
                and nonempty(launch.get("command")) and Path(launch["command"]).is_absolute()
                and isinstance(launch.get("args", []), list)
                and all(isinstance(argument, str) for argument in launch.get("args", []))
                and launch.get("env", {}) == {} and launch.get("secret_env", []) == [],
                "reconciled launch is not the exact ordinary credential-free stdio input")
        # Match CheckedInstalls.secret_launch_configuration and the selected
        # native serde shape, using the real checked publisher launch/slots.
        expected_configuration = {"name": server, "command": launch["command"],
            "args": launch.get("args", []), "env": {}, "secret_env": sorted(TRELLO_SLOTS),
            "access": {"allowed_operations": [TRELLO_OPERATION]}}
        proposal = context["configuration_persistence"]
        defining_realm = proposal["defining_realm"]
        expected_generation = proposal["expected_configuration_generation"]
        require(nonempty(defining_realm) and type(expected_generation) is int
                and 0 <= expected_generation < 2**64,
                "native-owned proposal omitted its defining realm or observed generation")
        # The driver obtains these owner facts before recording approval.
        # This case correlates that proposal with actual native outcomes;
        # separate owner controls establish how the handle selects them.
        require(request["phase"] == "installed_credential_service_access"
                and request["authentication"] == {"kind": "declared_secret_slots"}
                and request["operation_basis"] == {"kind": "package_published_metadata"}
                and request["application_selection"] == {"operations": [TRELLO_OPERATION],
                    "agent": "original", "expires_in_ms": None}
                and request["installation"] == expected_configuration
                and request["software"]["job_id"] == job["id"]
                and request["software"]["family"] == "agno"
                and request["software"]["profile"] == "trello"
                and request["software"]["server_name"] == server
                and request["software"]["validation_id"] == checked["validation_id"]
                and request["software"]["receipt_sha256"] == installed["receipt_sha256"]
                and request["software"]["configuration_sha256"] == installed["configuration_sha256"]
                and installed["configuration_sha256"] == checked["configuration_sha256"],
                "credential review changed the checked package, configuration or requested operation")
        require(context["configuration_persistence"] == {
                    "kind": "requested_defining_realm_persistence",
                    "defining_realm": defining_realm,
                    "expected_configuration_generation": expected_generation,
                    "configuration": expected_configuration,
                    "credential_slots": sorted(TRELLO_SLOTS),
                    "requested_effects": "persist this exact configuration, then privately store the declared credentials",
                    "authority": "requires current authenticated native realm administration at each mutation entry",
                    "partial_effects": "configuration or credentials can remain after cancellation or later failure"},
                "recorded persistence proposal differs from the actual owner and complete reviewed effects")
        fresh = context["fresh_original"]
        permission = decision["permission"]
        destination = decision["live_attachment"]["destination"]
        require(nonempty(fresh) and original_work_reference(context["prior_original"], self.source_authority)
                and context["business_task"] == {**self.task_scope,
                    "id": results["original-request"]["operation"]}
                and context["proposed_fresh_instruction"] == TRELLO_REQUEST
                and context["operator_and_requester"] == human()
                and context["represented_subject"] == human()
                and context["live_attachment"] == decision["live_attachment"]
                and decision["route"] == self.v.ACQUISITION_ROUTE
                and decision["decision"] == fresh and decision["choice"] == "approve"
                and decision["connection_revision"] == hashlib.sha256(
                    self.v.canonical(expected_configuration)).hexdigest()
                and body["connector_selection"] == {"server": server, "connector": None}
                and permission["target"] == "original" and permission["connection"] == server
                and permission["resource"] == server and permission["operations"] == [TRELLO_OPERATION]
                and permission["lifetime"]["scope"] == {"kind": "task", "task": fresh}
                and type(permission["lifetime"]["expires_at_ms"]) is int
                and permission["lifetime"]["expires_at_ms"] > int(time.time() * 1000)
                and accepted["identity"] == "original"
                and destination["context"] == accepted["session_id"]
                and destination["logical_owner"] == self.v.installed_fixture_principal("original-agent")
                and destination["audience"] == {"audience": "principal", "principal": human()},
                "credential review crossed the actual original, task, native configuration or audience")
        projection = review.get("credential_input")
        require(isinstance(projection, dict) and set(projection) == {"kind", "slots"}
                and projection["kind"] == "declared_secret_slots"
                and projection["slots"] == [
                    {"slot": TRELLO_SLOTS[0], "label": "config.api_key_env"},
                    {"slot": TRELLO_SLOTS[1], "label": "config.token_env"}],
                "protected review does not name exactly the checked two library slots")
        require(self._persistence_proposal is None, "credential persistence review was repeated")
        self._persistence_proposal = {"defining_realm": defining_realm, "server_name": server,
            "reviewed_configuration_generation": expected_generation}
        # The credential-specific checks above bind configuration, slots,
        # original, audience and lifetime before any protected submission.
        # Project only the same nonsecret facts used by native attachment.
        self._credential_service_context = self.v.installed_approved_service_context({
            "decision": decision, "acquisition": context["business_task"]})
        # This body goes only to POST /host/toolkit/setup/credentials. The
        # shared HTTP reporter redacts both canaries before retaining requests.
        return {**answer, "credentials": [
            {"slot": slot, "value": self._credential_values[slot]} for slot in TRELLO_SLOTS]}, context

    async def access_answer(self, http, review, accepted, results, report):
        answer, context = self.credential_answer(review, accepted, results)
        report["credential_refusals"] = await self.reject_invalid_credentials(http, review, answer)
        return answer, context, PANEL + "/credentials", "credential_access"

    def check_persistence_result(self, result):
        self.check_response(result)
        require(self._persistence_proposal is not None and isinstance(result, dict)
                and set(result) == {"state", "access_reviewed", "persistence", "grant_issued",
                    "fresh_instruction_retained", "ready", "task_publication"}
                and result["state"] == "reviewed" and result["access_reviewed"] is True
                and result["task_publication"] == "retained" and result["grant_issued"] is True
                and result["fresh_instruction_retained"] is True and result["ready"] is False,
                "actual credential request did not report known persistence and reviewed publication")
        persistence = result["persistence"]
        installed = persistence["install"]
        require(set(installed) == {"state", "generation"} and installed["state"] == "installed"
                and type(installed["generation"]) is int and 0 <= installed["generation"] < 2**64,
                "native configuration install did not return an actual generation")
        require(persistence == {**self._persistence_proposal, "install": installed,
                    "secrets": [{"slot": slot, "state": "stored"} for slot in TRELLO_SLOTS]},
                "native persistence differs from the reviewed owner or has incomplete credential writes")

    def finish_report(self, report, results, provider, completion):
        self.check_persistence_result(results["credential-task-access"])
        require([item["kind"] for item in report["decisions"]] == ["software", "credential_access"]
                and len(report["starts"]) == 1 and len(report.get("credential_refusals", [])) == 6
                and provider.business_observation is not None,
                "credential setup lacks its actual decisions, refusal interactions or original observation")
        report.update(status="passed", completion=completion,
                      credential_reply=results["credential-task-access"])

    async def reject_invalid_credentials(self, http, review, answer):
        # Run only after credential_answer validates this exact live review.
        # No valid decision is sent here; each refusal must leave the same
        # immutable approval and names-only form available for its real answer.
        credentials = answer["credentials"]
        invalid = [
            ("foreign_origin", answer, {"origin": "https://wrong-origin.invalid", "expected": 403}),
            ("malformed_envelope", ["not-a-credential-answer"], {"expected": 409}),
            ("missing_slot", {**answer, "credentials": credentials[:-1]}, {"expected": 409}),
            ("extra_slot", {**answer, "credentials": credentials + [
                {"slot": "UNDECLARED_FIXTURE_SLOT", "value": credentials[0]["value"]}]}, {"expected": 409}),
            ("duplicate_slot", {**answer, "credentials": [credentials[0], credentials[0]]}, {"expected": 409}),
            ("foreign_approval", {**answer, "approval_id": "not-the-retained-approval"}, {"expected": 409}),
        ]
        observations = []
        for case, body, options in invalid:
            response = await http("POST", PANEL + "/credentials", body, **options)
            self.check_response(response)
            current = await http("GET", PANEL + "/data")
            self.check_response(current)
            require(current["review"] == review,
                    "rejected credential response consumed or replaced the actual review")
            observations.append({"case": case, "expected_status": options["expected"],
                                 "same_review_retained": True})
        return observations

    def provider(self, directory, events):
        scenario = self

        class TrelloProvider(self.v.InstalledAcquisitionProvider):
            request_text = TRELLO_REQUEST
            operation = TRELLO_OPERATION
            profile = "trello"
            selection_values = TRELLO_SELECTION["values"]
            access_tool = "toolkit_request_service_account"
            access_call_id = "credential-task-access"

            def check_model_request(self, body):
                # Runs before retaining the model request or dispatching it.
                scenario.check_response(body)
                require(not any(call.get("function", {}).get("name") == TRELLO_OPERATION
                    for message in body["messages"] for call in message.get("tool_calls", [])),
                    "setup-only model history contains a vendor operation")

            def error_observation(self, error):
                return scenario.report_value(repr(error))

            def checked_configuration(self, checked):
                require(checked["scope"] == "configuration_only"
                        and checked["credential_environment_references"] == [
                            {"field_path": ["config", "api_key_env"], "environment_variable": TRELLO_SLOTS[0]},
                            {"field_path": ["config", "token_env"], "environment_variable": TRELLO_SLOTS[1]}],
                        "actual package check differs from the two declared Trello references")

            def check_authentication_metadata(self, metadata):
                # This publisher declares no-auth only for its public profiles.
                # The model explicitly chooses the closed declared-slot branch.
                require("authentication" not in metadata, "Trello publisher unexpectedly declares no-auth")

            def access_arguments(self, job_id, raw):
                return {**super().access_arguments(job_id, raw),
                        "authentication": {"kind": "declared_secret_slots"}}

            def check_access_result(self, result):
                scenario.check_persistence_result(result)

            def helper_finish(self):
                return ("The reviewed configuration and both declared credentials were stored by the native owner. "
                    "The task grant and fresh instruction were retained. No vendor operation or account verification was performed.")

            def response(self, body):
                if body["model"] == "fixture-original-model":
                    messages = body["messages"]
                    originals = [index for index, row in enumerate(messages)
                        if row["role"] == "user" and row.get("content") == self.request_text]
                    require(len(originals) in (1, 2), "expected exact setup original and at most one fresh input")
                    require(len(originals) == 2 or not any(row["role"] == "assistant"
                            and row.get("content") == scenario.v.ORIGINAL_A_FINISH for row in messages),
                            "credential continuation lost its original or fresh input")
                    if len(originals) == 2:
                        first, fresh = originals
                        require(scenario._persistence_proposal is not None
                                and "original-request" in self.issued and "original-request" in self.results
                                and self.results["original-request"].get("worker") == {"state": "input_admitted",
                                    "deduplicated": False, "source_settlement": "fresh_acceptance_promoted"}
                                and any(row["role"] == "assistant" and row.get("content") == scenario.v.ORIGINAL_A_FINISH
                                    for row in messages[first + 1:fresh]),
                                "credential fresh input lacks the actual original setup and completed history")
                        require(scenario._credential_service_context is not None,
                                "credential continuation lacks its reviewed service context")
                        scenario.v.original_catalog_history(messages, self.request_text, TRELLO_OPERATION, fresh,
                            service_context=scenario._credential_service_context)
                        tool_results = [row for row in messages if row["role"] == "tool"]
                        require(len(tool_results) == 1 and tool_results[0]["tool_call_id"] == "original-request"
                                and scenario.v.unique_json(tool_results[0]["content"].encode())
                                    == self.results["original-request"],
                                "setup-only continuation changed or added original tool effects")
                        offered = [row for row in body["tools"] if row["function"]["name"] == TRELLO_OPERATION]
                        deferred = [row for row in messages[fresh + 1:] if row["role"] == "user"
                            and scenario.v.original_tool_notice(row.get("content"), TRELLO_OPERATION) == "added"]
                        # Core publishes deferred metadata before loading a tool inline.
                        # This setup-only observation grants no invocation authority;
                        # reviewed native persistence and grant results remain separate.
                        require((len(offered), len(deferred)) in ((1, 0), (0, 1)),
                                "actual original lacks unambiguous checked Trello operation metadata")
                        require(self.business_observation is None, "setup-only original answer was replayed")
                        self.business_observation = {"answer": TRELLO_FINISH, "attached_operation": TRELLO_OPERATION,
                            "operation_metadata_visibility": "inline" if offered else "deferred",
                            "a_input_history_index": first, "b_input_history_index": fresh,
                            "model_request_index": len(self.requests) - 1,
                            "model_request_sha256": hashlib.sha256(scenario.v.canonical(body)).hexdigest(),
                            "vendor_operation": "not_requested", "native_b_terminal": "not_observed"}
                        self.events.add("original_credential_setup_answer_prepared", **self.business_observation)
                        self.business_response_prepared.set()
                        return {"delta": {"content": TRELLO_FINISH}, "finish_reason": "stop"}
                return super().response(body)

        return TrelloProvider(directory, events)
