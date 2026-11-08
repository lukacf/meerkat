#!/usr/bin/env python3
"""Produce one real canonical G with pending native consent, then lose its owner.

No build, R5 call, replacement preparation or full recovery claim. The existing
installed harness retains its genuine failed result for deliberate exit78.
"""
from __future__ import annotations
import argparse
import asyncio
import copy
import importlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import sys
import time
import uuid
from http.client import HTTPConnection
from types import SimpleNamespace

SELECTOR = "application_installed::canonical_g_capture::published_g_pending_consent_survives_actual_owner_loss"
PARKED = "The actual G context is waiting for human consent before setup entry."
class CapturedCanonicalG(RuntimeError):
    pass

def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module

def wrapper(python, binary, report, origin):
    return "#!" + str(python) + "\n" + """import json, os, sys
binary, selector, report, origin = CONFIG
protocol = os.dup(1)
os.set_inheritable(protocol, True)
env = dict(os.environ)
env.update(CONNECT_CANONICAL_G_CHILD=json.dumps(sys.argv[1:]),
           CONNECT_CANONICAL_G_STDOUT=str(protocol), CONNECT_CANONICAL_G_REPORT=report,
           CONNECT_CANONICAL_G_ORIGIN=origin)
with open(os.devnull, 'wb') as output:
    os.dup2(output.fileno(), 1)
os.execve(binary, [binary, '--exact', selector, '--ignored', '--quiet', '--nocapture', '--test-threads=1'], env)
""".replace("CONFIG", repr((str(binary), SELECTOR, str(report), origin)))

def scenario_type(u):
    v, c, s, require = u.v, u.c, u.s, u.require
    class Provider(u.AcceptanceModel):
        def __init__(self, directory, events):
            super().__init__(directory, events)
            self.g_approval = None
            self.g_context = None
        def response(self, body):
            messages = body["messages"]
            texts = [row.get("content", "") for row in messages if row["role"] in ("system", "user")]
            if s.G_TEXT not in texts:
                return super().response(body)
            contexts = []
            for text in texts:
                try:
                    value = json.loads(text)
                except (ValueError, TypeError):
                    continue
                if isinstance(value, dict) and value.get("purpose") == "toolkit-workflow-inline-context-v1":
                    contexts.append(value)
            if not contexts:
                return self.done("Dedicated G awaits native waiter context.")
            require(len(contexts) == 1 and contexts[0]["recipient"]["original"]["route"] == "original-a",
                    "only the actual independent A context may be delivered to G")
            context = contexts[0]
            require(self.g_context is None or self.g_context == context, "G context changed")
            self.g_context = copy.deepcopy(context)
            results = {}
            for row in messages:
                if row["role"] != "tool":
                    continue
                key = row["tool_call_id"]
                value = v.unique_json(row["content"].encode())
                if key == "g-pending-consent-state":
                    require(value.get("error") == "review_unsatisfied"
                            and value.get("data", {}).get("kind") == "human_consent_required"
                            and c.nonempty(value["data"].get("approval_id")), "actual Core G R3 refusal required")
                    approval = value["data"]["approval_id"]
                    require(self.g_approval is None or self.g_approval == approval, "G approval changed")
                    self.g_approval = approval
                else:
                    require(not value.get("error"), "G catalog selection refused")
                results[key] = value
            if self.g_approval:
                return self.done(PARKED)
            offered = {row["function"]["name"] for row in body["tools"]}
            if "setup_state" not in offered:
                if "g-pending-search" not in results:
                    return self.call(body, "g-pending-search", "tool_catalog_search", {"query":"setup_state","limit":50})
                require(any(row["name"] == "setup_state" for row in results["g-pending-search"]["results"]),
                        "actual G catalog omitted setup_state")
                return self.call(body, "g-pending-load", "tool_catalog_load", {"names":["setup_state"]})
            return self.call(body, "g-pending-consent-state", "setup_state", {}, context)

    class Scenario(u.Scenario):
        restore_after_shutdown = True
        phase = "prepared"
        initial_children = ("a",)
        name = "canonical-g-pending-predecessor"
        scope = "one actual published G, scoped Pending/Parked consent and owner-issued presenter closure before process loss"
        def __init__(self, origin, observations):
            super().__init__(v, origin)
            self.observations, self.phases = observations, []
        def restore_runtime_options(self):
            return self.application_runtime_options()
        def observe_preparation(self, attempt, finalizer, gateway, process, observe):
            super().observe_preparation(attempt, finalizer, gateway, process, observe)
            self.process_observation = observe
        async def check_ordinary_rebuild(self, *args):
            # The original ordinary/respawn selectors remain unchanged. This
            # dedicated producer creates no second class/member or callback.
            return None
        def configure_plan(self, plan):
            super().configure_plan(plan)
            for classes in (plan["application"]["member_classes"], plan["owner"]["native_host"]["member_classes"]):
                selected = [row for row in classes if row["id"] == u.CLASS_ID]
                require(len(selected) == 1, "one actual G class declaration")
                grants = [row for row in selected[0]["grants"] if row["tool_name"] == "setup_state"]
                require(len(grants) == 1 and grants[0]["consequence"] == "r2", "exact initial setup_state policy")
                # Classification stays R2; the trusted installed operation policy selects actual R3.
        def configure_runtime(self, config, directory):
            super().configure_runtime(config, directory)
            with Path(config.access_config_path).open("a") as access:
                access.write('\n[[rules]]\nid = "canonical-g-consent"\nsubjects = ["' + c.EMAIL + '"]\n'
                    'actions = ["agent.view", "agent.send"]\nagents = ["planning-lead"]\n')
        def provider(self, directory, events):
            return Provider(directory, events)
        async def run(self, private, provider, directory, events, process, base_url):
            require(base_url.rstrip("/") == self.origin, "actual selected listener required")
            report = {"phase":self.phase,"status":"running"}
            self.phases.append(report)
            if self.phase == "prepared":
                require(not provider.requests and self.roster.publications == 0, "preparation must not commission G or execute input")
                report["status"] = "passed"
                return report
            require(self.phase == "restored", "one actual installed Restore required")
            deadline = time.monotonic() + 360
            enrollment = None
            async def read_approval(identifier):
                def read():
                    connection = HTTPConnection("127.0.0.1", self.port, timeout=10)
                    try:
                        request = {"approval_id":identifier}
                        connection.request("POST", "/host/toolkit/consent/read", body=v.canonical(request), headers={
                            "Authorization":"Bearer " + self.console_workflow_token,
                            "Origin":self.origin,"Content-Type":"application/json"})
                        response = connection.getresponse(); raw = response.read(2*1024*1024+1)
                        observation = {"path":"/host/toolkit/consent/read","request":request,
                                       "status":response.status,"response_bytes":len(raw)}
                        report.setdefault("http",[]).append(observation)
                        require(response.status == 200 and len(raw) <= 2*1024*1024,
                                f"actual authenticated consent read refused (HTTP {response.status})")
                        value = v.unique_json(raw)
                        observation["response"] = value
                        return value["approval"]
                    finally:
                        connection.close()
                return await asyncio.to_thread(read)
            async def capture(identifier):
                def request():
                    address = v.unique_json(v.exact_bytes(self.observations/"socket.json"))["path"]
                    require(Path(address).is_absolute(), "actual private owner socket")
                    connection = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM); connection.settimeout(75)
                    try:
                        connection.connect(address); connection.sendall(v.canonical({"approval_id":identifier})); connection.shutdown(socket.SHUT_WR)
                        raw = bytearray()
                        while True:
                            chunk=connection.recv(65536)
                            if not chunk: break
                            raw.extend(chunk); require(len(raw) <= 8*1024*1024, "bounded owner response")
                        return v.unique_json(bytes(raw))
                    finally:
                        connection.close()
                return await asyncio.to_thread(request)
            try:
                expires=int(time.time())+600; scope="canonical-g-"+uuid.uuid4().hex
                enrollment=private.begin_console_login(v.SDK.ConsoleLoginSelection(c.ISSUER,c.SUBJECT,scope,expires*1000),timeout=10)
                await asyncio.to_thread(enrollment.result)
                self.console_workflow_login,self.console_workflow_token=enrollment,c.signed_login(scope,expires)
                with provider.lock:
                    s.Scenario.begin_model_phase(self,provider); provider.deadline=deadline
                launch=await self.begin_workflow(private,provider,directory,events,process,deadline)
                require(len(launch["children"]) == 1 and launch["children"][0]["original"]["route"] == "original-a", "one actual independent caller")
                report["launch"]=launch
                child=launch["children"][0]; member=self.workflow_context[4]
                status,canonical=await self.observe_shared(member)
                record=canonical["canonical"]["record"]
                require(record["phase"]["kind"] == "active" and record.get("workflow_resource")
                        and self.roster.publications == 1, "actual published canonical G association required")
                self.process_observation()  # Retain the real transitive G descendants in the existing harness.
                workflow=record["workflow"]["workflow"]
                while time.monotonic() < deadline:
                    with provider.lock:
                        require(not provider.errors, "actual controlled model refused")
                        identifier=provider.g_approval
                    delivery=u.context_delivery(directory,child["source_key"],workflow,status.session_id)
                    rows=s.input_rows(directory,status.session_id)
                    if identifier and delivery and u.finalized_context(rows,delivery,PARKED):
                        break
                    await asyncio.sleep(.05)
                else: raise AssertionError("actual G context did not finish with pending consent")
                report["original_a_inputs"] = await s.wait_initial_a_completion(directory,child["admission"],deadline)
                require(u.finalized_context(report["original_a_inputs"], {"input":child["admission"]["input_id"],
                    "session":child["admission"]["session_id"]}, "The independently authenticated task is waiting for setup."),
                    "actual A waiting input must complete independently")
                approval=await read_approval(identifier)
                require(approval["status"] == "pending" and approval["consent"]["scope"]["session_id"] == status.session_id
                        and approval["consent"].get("custody") == {"state":"parked"}
                        and approval["consent"].get("consumed_at") is None and approval["consent"].get("withdrawal") is None,
                        "actual pending G scope required")
                # Join publication before the intentional process loss, even
                # though no new source or physical recovery is started here.
                await self.commission_attempt.join_listing()
                response=await capture(identifier)
                require(response.get("status") == "captured_predecessor", "actual G owner capture refused: " + repr(response))
                snapshot=response["capture"]
                require(snapshot["session_id"] == status.session_id and snapshot["origin_input_id"] == delivery["input"]
                        and snapshot["selected_approval"] == approval and snapshot["association"] == record["workflow_resource"]
                        and snapshot["admission"] == record and snapshot["pid"] == process.pid,
                        "one real G association/session/consent/native input must match")
                held=snapshot["native_run_start_hold"]
                require(held == {"operation":"hold_run_starts", "reason":"mob_stop", "session_id":status.session_id,
                    "current_run":None,"rechecked_current_run":None,"same_registration":True,
                    "mob_stop_invoked":False,"unregister_invoked":False,"approval_before_hold":approval},
                    "exact non-destructive native starts hold and unchanged pending row required")
                code=await asyncio.to_thread(process.wait,timeout=30)
                require(code == 78 and v.unique_json(v.exact_bytes(self.observations/"g-capture.json")) == snapshot,
                        "actual old process exit and durable capture required")
                report.update(status="captured_predecessor",actual_exit_code=code,native=response,
                    canonical=canonical,delivery=delivery,roster=v.artifact_pin(self.roster.path),
                    admitting=v.artifact_pin(self.roster.admitting),callbacks={"builds":self.customizer.builds,"created":self.customizer.created})
                raise CapturedCanonicalG("actual published G owner exited78 with pending consent")
            finally:
                try:
                    if self.commission_attempt is not None: await self.commission_attempt.join_listing()
                    if enrollment is not None and report.get("status") != "captured_predecessor":
                        retired=await asyncio.to_thread(enrollment.retire,timeout=10)
                        require(retired.fenced and retired.reservations_drained and not retired.uncertain, "actual login retirement must join")
                finally:
                    v.save_json(directory/"console-journey.json",{"scope":self.scope,"phases":self.phases})
        def check_final_artifacts(self,directory,result):
            require(result.get("status") == "failed" and result.get("error") == repr(CapturedCanonicalG("actual published G owner exited78 with pending consent"))
                    and len(self.phases) == 2 and self.phases[0]["status"] == "passed"
                    and self.phases[-1]["status"] == "captured_predecessor", "only exact intentional capture is accepted")
            termination=result.get("termination",{})
            require(termination.get("state") == "exited" and termination.get("exit_code") == 78
                    and termination.get("signal") is None and termination.get("escalation") == "none" and not result.get("forced_cleanup"),
                    "actual old process must be reaped without escalation")
            required={"process_observed","process_reaped","transport_process_released","public_reader_created","public_reader_joined",
                "private_reader_created","private_reader_joined","pending_responses_empty","observed_descendants_gone"}
            cleanup=result.get("cleanup_observation",{})
            require(set(cleanup) == required and all(value is True for value in cleanup.values())
                    and result.get("provider_joined") is True and not result.get("model_schedule_errors"), "complete transitive cleanup required")
            require(not (self.observations/"installed-return.json").exists(), "ordinary Rust disposal cannot substitute for crash")
            phase=self.phases[-1]
            require(v.unique_json(v.exact_bytes(self.observations/"native-command.json")) == phase["native"]
                    and v.unique_json(v.exact_bytes(self.observations/"g-capture.json")) == phase["native"]["capture"], "native retained evidence changed")
    return Scenario

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ("test-executable","resource-executable","toolkit-python","meta-python","output"):
        parser.add_argument("--"+name,required=True,type=Path)
    parser.add_argument("--selector",required=True)
    parser.add_argument("--resources-sha256",required=True)
    args=parser.parse_args()
    for name in ("test_executable","resource_executable","toolkit_python","meta_python","output"):
        path=getattr(args,name)
        if not path.is_absolute(): parser.error("explicit absolute paths required")
        setattr(args,name,path.resolve())
    if args.selector != SELECTOR or args.output.exists(): parser.error("exact selector and fresh output required")
    scripts=Path(__file__).resolve().parent.parent
    u=load("canonical_g_supplied",scripts/"verify_supplied_shared_workflow.py")
    v,c=u.v,u.c
    args.output.mkdir(mode=0o700,parents=True)
    report={"schema":1,"status":"running","cases":[],"scope":"one canonical G predecessor capture, not R5/full recovery"}
    try:
        sys.path[:0]=[str(args.toolkit_python),str(args.meta_python)]
        for name,field,root in (("meerkat_toolkit.host_control","SDK",args.toolkit_python),
            ("meerkat_toolkit.application_runtime","APP_RUNTIME",args.toolkit_python),
            ("meerkat_meta.builder","META_BUILDER",args.meta_python),
            ("meerkat_meta.identity_first_models","META_IDENTITY",args.meta_python)):
            module=importlib.import_module(name)
            v.require(Path(module.__file__).resolve().is_relative_to(root),"selected SDK module escaped its source")
            setattr(v,field,module)
        u.COMMISSION=importlib.import_module("meerkat_toolkit.workflow_commission")
        modules=[(u.COMMISSION,args.toolkit_python)]
        modules.extend((importlib.import_module(name),args.meta_python) for name in (
            "meerkat_meta","meerkat_meta.builder","meerkat_meta.runtime","meerkat_meta._transport",
            "meerkat_meta.agent_builder","meerkat_meta.identity_first_models","meerkat_meta.identity_first_providers","meerkat_meta.types"))
        modules.extend((importlib.import_module(name),args.toolkit_python) for name in (
            "meerkat_toolkit.host_control","meerkat_toolkit.application_runtime","meerkat_toolkit.member_classes"))
        for module,root in modules:
            path=Path(module.__file__).resolve();v.require(path.is_relative_to(root),"SDK import escaped explicit selection")
            report.setdefault("sdk_sources",[]).append(v.artifact_pin(path))
        v.require(args.resource_executable.parent.name == "bin","selected package layout")
        resource_root=args.resource_executable.parent.parent/"libexec/meerkat-connect"
        raw=v.exact_bytes(resource_root/"resources.json",args.resources_sha256,limit=32*1024*1024)
        manifest=v.unique_json(raw);v.require(manifest["kind"] == "meerkat_connect_resources","actual resource manifest")
        package=args.output/"native-test-package";(package/"bin").mkdir(mode=0o700,parents=True)
        shutil.copytree(resource_root,package/"libexec/meerkat-connect",symlinks=True)
        binary=package/"bin/connect-lib-test";shutil.copy2(args.test_executable,binary)
        v.require(v.digest(binary) == v.digest(args.test_executable),"actual emitted binary copy differs")
        v.require(v.exact_bytes(package/"libexec/meerkat-connect/resources.json") == raw,"resources changed")
        candidate={"kind":"emitted_native_test_executable","executable":v.artifact_pin(binary),
            "emitted":v.artifact_pin(args.test_executable),"resources_sha256":args.resources_sha256,
            "resource_origin":v.artifact_pin(resource_root/"resources.json")}
        report["candidate"]=candidate
        report["fixture_sources"]=[v.artifact_pin(path) for path in (Path(__file__).resolve(),
            scripts/"verify_supplied_shared_workflow.py",scripts/"verify_shared_driver.py",
            scripts/"verify_native_gateway_subprocess.py",scripts/"verify_installed_console.py")]
        observations=args.output/"g-owner";observations.mkdir(mode=0o700)
        origin=c.loopback_origin();gateway=package/"bin/toolkit_gateway"
        gateway.write_text(wrapper(Path(sys.executable).resolve(),binary,observations,origin));gateway.chmod(0o700)
        cases=args.output/"cases";cases.mkdir(mode=0o700)
        scenario=scenario_type(u)(origin,observations)
        result=asyncio.run(v._run_installed_product(SimpleNamespace(output=cases,toolkit_gateway=gateway),
            candidate,package,manifest,corrupt=False,console=scenario))
        actual=v.unique_json(v.exact_bytes(cases/scenario.name/"result.json"))
        v.require(actual == result,"actual failed installed record changed")
        scenario.check_final_artifacts(cases/scenario.name,actual)
        v.require("evidence_refusal" not in actual,"harness final evidence refused")
        report["cases"].append({"case":scenario.name,"status":"captured_predecessor",
            "failed_installed_result":v.artifact_pin(cases/scenario.name/"result.json"),
            "wrapper":v.artifact_pin(gateway),"capture":v.artifact_pin(observations/"g-capture.json"),
            "native_command":v.artifact_pin(observations/"native-command.json"),"actual_exit_code":78,
            "full_installed_success":False,"recovery_result":None})
        v.require(v.digest(binary) == candidate["emitted"]["sha256"] and v.exact_bytes(resource_root/"resources.json") == raw,
            "selected inputs changed")
        report["status"]="captured_predecessor"
    except BaseException as error:
        import traceback
        report.update(status="failed",error=repr(error),traceback=traceback.format_exc())
    finally:
        v.save_json(args.output/"result.json",report)
    return 0 if report["status"] == "captured_predecessor" else 1

if __name__ == "__main__":
    raise SystemExit(main())
