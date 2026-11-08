#!/usr/bin/env python3
"""Capture a real routable predecessor after Admitted + Requeue, before Resume.

Builds nothing. The existing harness retains its actual failed installed result
for the intentional process exit77. A separate capture result requires that
exact exit plus reader/provider/descendant cleanup; it is not an installed PASS.
"""
from __future__ import annotations
import argparse
import asyncio
import importlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import sys
import time
import traceback
import uuid
from http.client import HTTPConnection
from types import SimpleNamespace
from urllib.parse import urlencode

SELECTOR = "application_installed::consent_transfer_tests::cold_capture::routable_consent_predecessor_exits_after_actual_admitted_requeue"
REQUEUE_SELECTOR = SELECTOR

class CapturedPredecessor(RuntimeError):
    pass

REQUEST = "Find a public Hacker News reader for this consent transfer control."
REPLACEMENT = "A separate same-session input must not spend the first input's consent."
PARKED = "The exact native request is waiting for human consent."
COMPLETE = "The exact registered consent notice authorized the retained Find."


def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


def wrapper(python, binary, report, origin, selector=SELECTOR, requeue=False):
    return "#!" + str(python) + "\n" + """import json, os, sys
binary, selector, report, origin, requeue = CONFIG
protocol = os.dup(1)
os.set_inheritable(protocol, True)
env = dict(os.environ)
env.update(CONNECT_CONSENT_TRANSFER_CHILD=json.dumps(sys.argv[1:]),
           CONNECT_CONSENT_TRANSFER_STDOUT=str(protocol),
           CONNECT_CONSENT_TRANSFER_REPORT=report,
           CONNECT_CONSENT_TRANSFER_ORIGIN=origin)
if requeue:
    env['CONNECT_CONSENT_REQUEUE'] = 'selected'
    env['CONNECT_CONSENT_CAPTURE'] = 'selected'
else:
    env.pop('CONNECT_CONSENT_REQUEUE', None)
with open(os.devnull, 'wb') as harness_output:
    os.dup2(harness_output.fileno(), 1)
os.execve(binary, [binary, '--exact', selector, '--ignored', '--quiet', '--nocapture', '--test-threads=1'], env)
""".replace("CONFIG", repr((str(binary), selector, str(report), origin, requeue)))


def scenario_type(v, c):
    require = v.require

    class Scenario(c.ConsoleScenario):
        restore_after_shutdown = True
        scope = "real installed request, actual Admitted/Requeue and process exit; no R5 settlement or physical closure"

        def __init__(self, origin, replacement, observations, requeue=False):
            super().__init__(v, origin)
            self.phase, self.phases = "prepared", []
            self.replacement, self.observations, self.requeue = replacement, observations, requeue
            self.name = "r5-routable-predecessor"
            if requeue:
                self.scope = type(self).scope

        def configure_plan(self, plan):
            super().configure_plan(plan)
            grants = next(row for row in plan["application"]["tool_policy"]["members"]
                          if row["member_identity"] == "original")["grants"]
            selected = [row for row in grants if row["tool_name"] == "find_and_install"]
            require(len(selected) == 1 and selected[0]["consequence"] == "r2", "exact existing Find classification")
            # R2 remains metadata; the trusted installed operation owner selects actual R3.

        def provider(self, directory, events):
            class Provider(v.InstalledAcquisitionProvider):
                def __init__(self, directory, events):
                    super().__init__(directory, events)
                    self.approvals = {}
                    self.resumed_boundary = False

                def response(self, body):
                    lane = body["model"]
                    require(lane in ("fixture-original-model", "fixture-helper-model"), "unexpected model lane")
                    if lane == "fixture-helper-model":
                        # The real Find may start its helper. No service effect is
                        # requested by this delivery control.
                        return {"delta": {"content": "No service installation was requested by this control."}, "finish_reason": "stop"}
                    messages = body["messages"]
                    tools = {row["function"]["name"] for row in body["tools"]}
                    results = [row for row in messages if row["role"] == "tool"]
                    calls = {"consent-original", "consent-replacement"}
                    for row in results:
                        if row["tool_call_id"] not in calls:
                            continue
                        value = v.unique_json(row["content"].encode())
                        require(value.get("error") == "review_unsatisfied"
                                and value.get("data", {}).get("kind") == "human_consent_required"
                                and c.nonempty(value["data"].get("approval_id")), "exact native R3 refusal required")
                        old = self.approvals.setdefault(row["tool_call_id"], value["data"]["approval_id"])
                        require(old == value["data"]["approval_id"], "approval ID changed")
                    # Provider text only drives the response. The acceptance
                    # oracle below requires the real typed persisted notice.
                    feedback = "The human-approved find_and_install call consent-original ran after its approval."
                    if any(feedback in str(row.get("content", "")) for row in messages if row["role"] in ("system", "user")):
                        self.finished.set()
                        return {"delta": {"content": COMPLETE}, "finish_reason": "stop"}
                    users = [row.get("content") for row in messages if row["role"] == "user"]
                    require(REQUEST in users, "original input missing from native history")
                    call_id = "consent-replacement" if REPLACEMENT in users else "consent-original"
                    if call_id not in self.approvals:
                        require(call_id not in self.issued and "find_and_install" in tools,
                                "one actual Find attempt per native input")
                        self.issued.add(call_id)
                        return {"delta": {"tool_calls": [{"index": 0, "id": call_id, "type": "function",
                            "function": {"name": "find_and_install", "arguments": json.dumps({
                                "request": REQUEST, "user_context": [], "caller_observations": []})}}]},
                            "finish_reason": "tool_calls"}
                    # Only the actual delivered notice can cause this later run.
                    # The harmless catalog call advances a genuine native boundary
                    # if Core has not already spent its transferred candidate.
                    identifier = self.approvals["consent-original"]
                    notice = any(("Consent for approval " + identifier + " was approved.") in str(row.get("content", "")) for row in messages
                                 if row["role"] in ("system", "user"))
                    if notice and not self.resumed_boundary:
                        require("tool_catalog_search" in tools, "resumed native catalog surface missing")
                        self.resumed_boundary = True
                        return {"delta": {"tool_calls": [{"index": 0, "id": "consent-notice-boundary", "type": "function",
                            "function": {"name": "tool_catalog_search", "arguments": '{"query":"find","limit":1}'}}]},
                            "finish_reason": "tool_calls"}
                    return {"delta": {"content": PARKED}, "finish_reason": "stop"}
            return Provider(directory, events)

        async def run(self, private, provider, directory, events, process, base_url):
            require(base_url.rstrip("/") == self.origin, "actual listener differs")
            report = {"phase": self.phase, "status": "running", "http": [], "frames": []}
            self.phases.append(report)
            if self.phase == "prepared":
                require(not provider.requests, "preparation must not execute a model turn")
                report["status"] = "passed"
                return report
            deadline = time.monotonic() + 180
            expires = int(time.time()) + 240
            scope = "consent-transfer-" + uuid.uuid4().hex
            token = c.signed_login(scope, expires)
            enrollment = None

            async def http(method, path, body=None):
                def request():
                    require(process.poll() is None and time.monotonic() < deadline, "live bounded gateway required")
                    connection = HTTPConnection("127.0.0.1", self.port, timeout=min(10, max(.1, deadline-time.monotonic())))
                    try:
                        headers = {"Authorization": "Bearer " + token}
                        if body is not None:
                            headers.update({"Origin": self.origin, "Content-Type": "application/json"})
                        connection.request(method, path, body=None if body is None else v.canonical(body), headers=headers)
                        response = connection.getresponse()
                        raw = response.read(2*1024*1024+1)
                        observation = {"method":method,"path":path,"request":body,
                                       "status":response.status,"response_bytes":len(raw)}
                        report["http"].append(observation)
                        require(len(raw) <= 2*1024*1024, "bounded response")
                        require(response.status == 200,
                                f"actual Console request refused: {path} (HTTP {response.status})")
                        value = v.unique_json(raw)
                        observation["response"] = value
                        return value
                    finally:
                        connection.close()
                return await asyncio.to_thread(request)

            async def frames():
                page = await http("GET", "/console/timeline?" + urlencode({"identity":"original","limit":200}))
                require(page.get("available", True) and isinstance(page.get("frames"), list), "native timeline unavailable")
                result = [c.frame_record(row) for row in page["frames"]]
                report["frames"] = result
                return result

            async def row(identifier):
                return (await http("POST", "/host/toolkit/consent/read", {"approval_id":identifier}))["approval"]

            async def lifecycle(command, approval):
                def request():
                    address = v.unique_json(v.exact_bytes(self.observations / "requeue-socket.json"))["path"]
                    require(isinstance(address, str) and Path(address).is_absolute(), "actual private owner socket required")
                    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
                    connection.settimeout(min(70, max(.1, deadline-time.monotonic())))
                    try:
                        connection.connect(address)
                        request = {"command":command,"approval_id":approval["approval_id"]}
                        connection.sendall(v.canonical(request))
                        connection.shutdown(socket.SHUT_WR)
                        raw = bytearray()
                        while True:
                            chunk = connection.recv(65536)
                            if not chunk:
                                break
                            raw.extend(chunk)
                            require(len(raw) <= 8*1024*1024, "bounded native owner observation")
                        result = v.unique_json(bytes(raw))
                        report.setdefault("native_lifecycle", []).append(result)
                        require(result.get("status") == "passed" and result.get("request") == request,
                                "real native lifecycle operation refused: " + command)
                        require(result["native"]["session_id"] == approval["consent"]["scope"]["session_id"],
                                "native owner observed another session")
                        return result["native"]
                    finally:
                        connection.close()
                return await asyncio.to_thread(request)

            async def parked(identifier, accepted, text):
                while time.monotonic() < deadline:
                    with provider.lock:
                        require(not provider.errors, "controlled provider refused actual execution")
                        approval = provider.approvals.get(identifier)
                    observed = await frames()
                    starts = [f for f in observed if f.get("kind") == "run_started"
                              and f.get("session_id") == accepted["session_id"] and c.input_text(f) == text]
                    require(len(starts) <= 1, "duplicate native input run")
                    if approval and starts:
                        run = c.run_id(starts[0])
                        done = [f for f in observed if f.get("kind") == "interaction_complete" and f["payload"].get("source_event_type") == "run_completed" and c.run_id(f) == run]
                        if done:
                            require(len(done) == 1 and done[0].get("status") == "completed"
                                    and done[0]["payload"].get("result") == PARKED, "exact completed parked answer")
                            actual = await row(approval)
                            require(actual["status"] == "pending" and actual["consent"].get("custody") == {"state":"parked"}
                                    and actual["consent"].get("consumed_at") is None
                                    and actual["consent"].get("withdrawal") is None
                                    and actual["consent"]["scope"]["session_id"] == accepted["session_id"],
                                    "actual issuer did not park unspent consent after native completion")
                            return actual, run
                    await asyncio.sleep(.05)
                raise AssertionError("actual input did not finish with parked consent")

            try:
                enrollment = private.begin_console_login(v.SDK.ConsoleLoginSelection(
                    issuer=c.ISSUER,subject=c.SUBJECT,login_scope_id=scope,expires_at_ms=expires*1000),timeout=10)
                await asyncio.to_thread(enrollment.result)
                report["enrollment"] = {"id":enrollment.enrollment_id,"scope":scope}
                with provider.lock:
                    provider.enabled = True
                    provider.deadline = deadline
                accepted = await http("POST", "/console/send", {"identity":"original","content":REQUEST,"origin":"console","origin_kind":"operator","idempotency_key":"consent-original-"+uuid.uuid4().hex})
                require(accepted.get("identity") == "original" and c.nonempty(accepted.get("session_id")), "exact native original acceptance")
                report["accepted"] = accepted
                original, original_run = await parked("consent-original", accepted, REQUEST)
                report["parked_original"] = original
                replacement = replacement_run = None
                if self.replacement:
                    second = await http("POST", "/console/send", {"identity":"original","content":REPLACEMENT,"origin":"console","origin_kind":"operator","idempotency_key":"consent-replacement-"+uuid.uuid4().hex})
                    require(second["session_id"] == accepted["session_id"] and second["interaction_id"] != accepted["interaction_id"],
                            "replacement must be a distinct actual input in the same native session")
                    replacement, replacement_run = await parked("consent-replacement", second, REPLACEMENT)
                    require(replacement["approval_id"] != original["approval_id"]
                            and replacement["consent"]["origin"] != original["consent"]["origin"]
                            and replacement["proposed_action"]["body"] == original["proposed_action"]["body"], "same arguments cannot substitute native origin")
                    report["parked_replacement"] = replacement
                if self.requeue:
                    report["native_hold"] = await lifecycle("hold", original)
                    require(report["native_hold"]["approval"] == original, "hold did not preserve exact pending row")
                requeued = False
                decision = await http("POST", "/host/toolkit/consent/decide", {
                    "approval_id":original["approval_id"],"decision":"approve","reason":"exact retained Find"})
                require(decision["approval"]["status"] == "approved" and decision["approval"]["consent"].get("consumed_at") is None,
                        "decision must not itself execute")
                report["decision"] = decision
                while time.monotonic() < deadline:
                    actual = await row(original["approval_id"])
                    deliveries = [d for d in actual["consent"].get("deliveries", []) if d["transition"] == "approved"]
                    require(len(deliveries) == 1, "one approved notice obligation")
                    observed = await frames()
                    state = deliveries[0]["state"]
                    if self.requeue and not requeued and state["status"] == "admitted":
                        require(actual["consent"].get("consumed_at") is None
                                and actual["consent"].get("withdrawal") is None
                                and actual["consent"].get("custody") == {"state":"parked"},
                                "held run starts must keep the actual notice unspent")
                        closed = await lifecycle("requeue", original)
                        require(closed["input_id"] == state["input_id"]
                                and closed["approval"] == actual, "Requeue must affect exactly the admitted notice input")
                        snapshot = await lifecycle("capture", original)
                        require(snapshot["input_id"] == state["input_id"]
                                and snapshot["selected_approval"] == actual
                                and snapshot["session_id"] == accepted["session_id"]
                                and snapshot["registration_absent"] is True
                                and snapshot["executor_absent"] is True
                                and snapshot["native_input_source"] == "DurableStore"
                                and snapshot["native_input_recheck_source"] == "DurableStore",
                                "capture must preserve the real Admitted/Requeue selection")
                        code = await asyncio.to_thread(process.wait, timeout=30)
                        require(code == 77 and snapshot["pid"] == process.pid,
                                "actual selected process exit77 required")
                        require(v.unique_json(v.exact_bytes(self.observations / "cold-capture.json")) == snapshot,
                                "durable capture differs from actual socket response")
                        report.update(status="captured_predecessor", native_requeue=closed,
                                      native_capture=snapshot, actual_exit_code=code)
                        # Preserve the ordinary harness's failure report. Its
                        # finally block still joins the actual SDK/process owners.
                        raise CapturedPredecessor("actual admitted/requeued owner exited77 before Resume")
                    await asyncio.sleep(.05)
                raise AssertionError("actual admitted notice was not captured before deadline")
            finally:
                try:
                    if enrollment is not None and report.get("status") != "captured_predecessor":
                        retired = await asyncio.to_thread(enrollment.retire,timeout=10)
                        require(retired.fenced and retired.reservations_drained and not retired.uncertain, "real login retirement must join")
                finally:
                    v.save_json(directory / "console-journey.json", {"scope":self.scope,"phases":self.phases})

        def check_final_artifacts(self, directory, result):
            require(result.get("status") == "failed" and len(self.phases) == 2
                    and self.phases[0].get("phase") == "prepared" and self.phases[0].get("status") == "passed"
                    and self.phases[-1].get("status") == "captured_predecessor"
                    and result.get("error") == repr(CapturedPredecessor("actual admitted/requeued owner exited77 before Resume")),
                    "only the selected intentional process exit is capture evidence")
            termination = result.get("termination", {})
            require(termination.get("state") == "exited" and termination.get("exit_code") == 77
                    and termination.get("signal") is None and termination.get("escalation") == "none"
                    and not result.get("forced_cleanup"), "actual child exit must be reaped without escalation")
            cleanup = result.get("cleanup_observation", {})
            required_cleanup = {"process_observed", "process_reaped", "transport_process_released",
                                "public_reader_created", "public_reader_joined", "private_reader_created",
                                "private_reader_joined", "pending_responses_empty", "observed_descendants_gone"}
            require(set(cleanup) == required_cleanup and all(value is True for value in cleanup.values())
                    and result.get("provider_joined") is True and not result.get("model_schedule_errors"),
                    "all original process/readers/descendants/provider joins remain mandatory")
            require(not (self.observations / "installed-return.json").exists(),
                    "intentional crash must not be substituted with Rust owner disposal")
            phase = self.phases[-1]
            require([row["request"]["command"] for row in phase["native_lifecycle"]] == ["hold","requeue","capture"],
                    "actual capture must occur before any Resume")
            for index, observed in enumerate(phase["native_lifecycle"], 1):
                require(v.unique_json(v.exact_bytes(self.observations / ("requeue-"+str(index)+".json"))) == observed,
                        "retained native lifecycle observation differs")
            require(v.unique_json(v.exact_bytes(self.observations / "cold-capture.json")) == phase["native_capture"],
                    "actual durable capture changed")
            # No mutation of result.status: the ordinary installed run failed.

    return Scenario


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ("test-executable", "resource-executable", "toolkit-python", "meta-python", "output"):
        parser.add_argument("--" + name, required=True, type=Path)
    parser.add_argument("--selector", required=True)
    parser.add_argument("--resources-sha256", required=True)
    args = parser.parse_args()
    for name in ("test_executable", "resource_executable", "toolkit_python", "meta_python", "output"):
        path = getattr(args, name)
        if not path.is_absolute():
            parser.error("explicit absolute paths required")
        setattr(args, name, path.resolve())
    if args.selector != SELECTOR or args.output.exists():
        parser.error("exact ignored selector and fresh output required")
    scripts = Path(__file__).resolve().parent.parent
    v = load("consent_native_verifier", scripts / "verify_native_gateway_subprocess.py")
    c = load("consent_console_fixture", scripts / "verify_installed_console.py")
    args.output.mkdir(mode=0o700, parents=True)
    report = {"schema":1,"status":"running","cases":[],
              "scope":"routable predecessor capture only; original failed installed result and cleanup retained"}
    try:
        sys.path[0:0] = [str(args.toolkit_python), str(args.meta_python)]
        v.SDK = importlib.import_module("meerkat_toolkit.host_control")
        v.APP_RUNTIME = importlib.import_module("meerkat_toolkit.application_runtime")
        v.META_BUILDER = importlib.import_module("meerkat_meta.builder")
        v.META_IDENTITY = importlib.import_module("meerkat_meta.identity_first_models")
        selected = [(v.SDK,args.toolkit_python),(v.APP_RUNTIME,args.toolkit_python)]
        selected.extend((importlib.import_module(name),args.meta_python) for name in (
            "meerkat_meta","meerkat_meta.builder","meerkat_meta.runtime","meerkat_meta._transport",
            "meerkat_meta.agent_builder","meerkat_meta.identity_first_models","meerkat_meta.identity_first_providers","meerkat_meta.types"))
        for module, root in selected:
            path = Path(module.__file__).resolve()
            v.require(path.is_relative_to(root), "SDK import escaped explicit selection")
            report.setdefault("sdk_sources",[]).append(v.artifact_pin(path))
        v.require(args.resource_executable.parent.name == "bin", "selected resource package layout")
        resource_root = args.resource_executable.parent.parent / "libexec/meerkat-connect"
        raw = v.exact_bytes(resource_root / "resources.json", args.resources_sha256, limit=32*1024*1024)
        manifest = v.unique_json(raw)
        v.require(manifest["kind"] == "meerkat_connect_resources", "actual resources manifest")
        package = args.output / "native-test-package"
        (package / "bin").mkdir(mode=0o700, parents=True)
        shutil.copytree(resource_root, package / "libexec/meerkat-connect", symlinks=True)
        binary = package / "bin/connect-lib-test"
        shutil.copy2(args.test_executable,binary)
        v.require(v.digest(binary) == v.digest(args.test_executable), "emitted test copy changed")
        v.require(v.exact_bytes(package / "libexec/meerkat-connect/resources.json") == raw, "resources changed")
        candidate = {"kind":"emitted_native_test_executable","executable":v.artifact_pin(binary),
                     "emitted":v.artifact_pin(args.test_executable),"resources_sha256":args.resources_sha256,
                     "resource_origin":v.artifact_pin(resource_root / "resources.json")}
        report["candidate"] = candidate
        report["fixture_sources"] = [v.artifact_pin(path) for path in (Path(__file__).resolve(),
            scripts / "verify_native_gateway_subprocess.py",scripts / "verify_installed_console.py")]
        cases = args.output / "cases"
        cases.mkdir(mode=0o700)
        Scenario = scenario_type(v,c)
        for replacement in (False,):
            observations = args.output / ("replacement-owner" if replacement else "transfer-owner")
            observations.mkdir(mode=0o700)
            origin = c.loopback_origin()
            gateway = package / "bin" / ("consent-replacement-gateway" if replacement else "consent-transfer-gateway")
            gateway.write_text(wrapper(Path(sys.executable).resolve(),binary,observations,origin,args.selector,True))
            gateway.chmod(0o700)
            scenario = Scenario(origin,replacement,observations,True)
            result = asyncio.run(v._run_installed_product(SimpleNamespace(output=cases,toolkit_gateway=gateway),
                candidate,package,manifest,corrupt=False,console=scenario))
            # Re-read the unchanged retained raw report, then independently
            # validate the expected crash. Never promote its status to passed.
            raw_result = v.unique_json(v.exact_bytes(cases/scenario.name/"result.json"))
            v.require(raw_result == result, "actual harness result changed")
            scenario.check_final_artifacts(cases/scenario.name, raw_result)
            v.require("evidence_refusal" not in raw_result, "harness final evidence check refused")
            report["cases"].append({"case":scenario.name,"status":"captured_predecessor",
                "failed_installed_result":v.artifact_pin(cases/scenario.name/"result.json"),
                "wrapper":v.artifact_pin(gateway),"capture":v.artifact_pin(observations/"cold-capture.json"),
                "native_records":[v.artifact_pin(observations/("requeue-"+str(i)+".json")) for i in (1,2,3)],
                "actual_exit_code":77,"full_installed_success":False,"recovery_result":None})
        v.require(v.digest(binary) == candidate["emitted"]["sha256"] and
                  v.exact_bytes(resource_root/"resources.json") == raw,"selected inputs changed")
        report["status"] = "captured_predecessor"
    except BaseException as error:
        report.update(status="failed",error=repr(error),traceback=traceback.format_exc())
    finally:
        v.save_json(args.output/"result.json",report)
    return 0 if report["status"] == "captured_predecessor" else 1


if __name__ == "__main__":
    raise SystemExit(main())
