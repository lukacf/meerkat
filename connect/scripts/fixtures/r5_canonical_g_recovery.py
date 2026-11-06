#!/usr/bin/env python3
"""Reopen ONE captured canonical G through the actual installed R5 path.

The predecessor must come from the corrected G producer using this same emitted
binary. No second plan, commission, old/A producer or reconstructed native row.
This is an explicit destructive-to-that-fixture recovery, never an automatic retry.
"""
from __future__ import annotations
import argparse
import asyncio
import base64
import copy
import dataclasses
import importlib
import importlib.util
import json
import os
from pathlib import Path
import shutil
import socket
import sys
import time
import tomllib
import traceback
import uuid
from urllib.parse import urlsplit

SELECTOR = "application_installed::canonical_g_recovery::same_published_g_settles_reopens_and_accepts_fresh_work"
ANSWER = "The recovered same G read its fresh setup job list for the new B context."
CLEANUP = {"process_observed", "process_reaped", "transport_process_released", "public_reader_created",
    "public_reader_joined", "private_reader_created", "private_reader_joined", "pending_responses_empty", "observed_descendants_gone"}
def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec); sys.modules[name] = module; spec.loader.exec_module(module)
    return module

def wrapper(python, binary, report, capture, origin):
    return "#!" + str(python) + "\n" + """import json, os, sys
binary, selector, report, capture, origin = CONFIG
protocol = os.dup(1); os.set_inheritable(protocol, True)
env = dict(os.environ)
env.update(CONNECT_CANONICAL_G_RECOVERY_CHILD=json.dumps(sys.argv[1:]),
    CONNECT_CANONICAL_G_RECOVERY_STDOUT=str(protocol), CONNECT_CANONICAL_G_RECOVERY_REPORT=report,
    CONNECT_CANONICAL_G_RECOVERY_CAPTURE=capture, CONNECT_CANONICAL_G_RECOVERY_ORIGIN=origin)
with open(os.devnull, 'wb') as output: os.dup2(output.fileno(), 1)
os.execve(binary, [binary, '--exact', selector, '--ignored', '--quiet', '--nocapture', '--test-threads=1'], env)
""".replace("CONFIG", repr((str(binary), SELECTOR, str(report), str(capture), origin)))

def checked_pin(v, pin):
    raw = v.exact_bytes(Path(pin["path"]), pin["sha256"], limit=64*1024*1024)
    expected = pin.get("bytes", pin.get("size"))
    v.require(expected is None or expected == len(raw), "retained pin size differs")
    return raw

def predecessor(v, args):
    root = args.predecessor
    report = v.unique_json(v.exact_bytes(root/"result.json"))
    v.require(report["status"] == "captured_predecessor" and len(report["cases"]) == 1,
        "actual successful G producer parent required")
    case = report["cases"][0]
    v.require(case["case"] == "canonical-g-pending-predecessor" and case["actual_exit_code"] == 78
        and case["full_installed_success"] is False and case["recovery_result"] is None,
        "old producer must remain an intentional failed installed run")
    capture = v.unique_json(checked_pin(v, case["capture"]))
    failed = v.unique_json(checked_pin(v, case["failed_installed_result"]))
    v.require(Path(case["capture"]["path"]) == root/"g-owner/g-capture.json"
        and failed["status"] == "failed" and failed["error"] ==
            "CapturedCanonicalG('actual published G owner exited78 with pending consent')"
        and failed["termination"]["exit_code"] == 78 and failed["termination"]["escalation"] == "none"
        and failed["termination"]["signal"] is None and failed["provider_joined"] is True
        and not failed["forced_cleanup"] and set(failed["cleanup_observation"]) == CLEANUP
        and all(value is True for value in failed["cleanup_observation"].values()) and not failed["model_schedule_errors"],
        "actual old owner and transitive cleanup are prerequisites")
    native = v.unique_json(checked_pin(v, case["native_command"]))
    v.require(native == {"status":"captured_predecessor", "capture":capture}, "native producer record differs")
    readback = v.unique_json(v.exact_bytes(root/"g-owner-loss-readback.json"))
    v.require(readback["scope"]=="same_G_issuer_presenter_and_exclusive_physical_no_entry"
        and readback["association"]==capture["association"] and readback["session_id"]==capture["session_id"]
        and readback["issuer"]==capture["issuer"] and readback["approvals"]==capture["approvals"]
        and readback["presenter_closed"]==capture["presenter_closed"] and readback["physical"]==capture["physical"]
        and readback["native_R5_result"] is None and readback["fresh_recovery_preparation"] is None,
        "same original parent exclusion/census required")
    # Keep its complete original shape; the typed reader below independently
    # reacquires the issuer and EX physical token instead of trusting flags.
    evidence = [v.artifact_pin(root/"result.json"), v.artifact_pin(root/"g-owner-loss-readback.json"),
        case["capture"], case["native_command"], case["failed_installed_result"]]
    v.require(capture["phase"] == "canonical_g_pending_parked_before_exit"
        and capture["association_document"]["session"] == capture["session_id"]
        and capture["association_document"]["closed"] is None
        and capture["admission"]["workflow_resource"] == capture["association"]
        and capture["native_input_source"] == "LiveRuntime" and len(capture["native_inputs"]) == 2
        and capture["selected_approval"] == capture["approvals"][0] and len(capture["approvals"]) == 1
        and capture["native_settlement"] is None and capture["new_preparation"] is None,
        "one exact canonical G predecessor required")
    v.require(v.unique_json(checked_pin(v, capture["association"])) == capture["association_document"],
        "canonical association bytes differ")
    checked_pin(v, capture["presenter_closed"])
    v.require(v.digest(args.test_executable) == report["candidate"]["emitted"]["sha256"]
        == report["candidate"]["executable"]["sha256"] and args.resources_sha256 == report["candidate"]["resources_sha256"],
        "consumer must use the same actual emitted G producer binary and resources")
    retained_binary=Path(report["candidate"]["executable"]["path"])
    v.require(retained_binary.is_file() and not retained_binary.is_symlink()
        and retained_binary.stat().st_size==report["candidate"]["executable"]["bytes"]
        and v.digest(retained_binary)==report["candidate"]["executable"]["sha256"],"retained producer binary changed")
    directory = Path(case["failed_installed_result"]["path"]).parent
    journey = v.unique_json(checked_pin(v, failed["console_evidence"]))
    v.require(len(journey["phases"]) == 2 and journey["phases"][-1]["status"] == "captured_predecessor",
        "actual old G commissioning phase missing")
    phase = journey["phases"][-1]
    v.require(phase["native"]["capture"] == capture and phase["canonical"]["canonical"]["record"] == capture["admission"],
        "old SDK and actual owner selections differ")
    for pin in (phase["roster"], phase["admitting"], failed["preparation_plan"], failed["model_configuration"],
        failed["finalization"]["completed_setup"], failed["console_evidence"]):
        checked_pin(v,pin); evidence.append(pin)
    # The new run may mutate native stores, but never these immutable originals.
    evidence.extend([capture["association"],capture["presenter_closed"]])
    return report, failed, capture, phase, directory, evidence, readback

def types(u, model_port):
    v,s,c,require=u.v,u.s,u.c,u.require
    class Provider(u.AcceptanceModel):
        listen_port=model_port
        def __init__(self,directory,events):
            super().__init__(directory,events); self.context=None; self.read_result=None
        def response(self,body):
            texts=[m.get("content","") for m in body["messages"] if m["role"] in ("system","user")]
            if s.G_TEXT not in texts: return super().response(body)
            contexts=[]
            for text in texts:
                try: value=json.loads(text)
                except (ValueError,TypeError): continue
                if isinstance(value,dict) and value.get("purpose")=="toolkit-workflow-inline-context-v1": contexts.append(value)
            require(contexts and contexts[-1]["recipient"]["original"]["route"]=="original-b",
                "only a fresh B context may cause work after Ready")
            context=contexts[-1]
            require(self.context is None or self.context==context,"fresh G context changed")
            self.context=copy.deepcopy(context)
            results={}
            for row in body["messages"]:
                if row["role"]=="tool" and row.get("tool_call_id") in self.issued:
                    value=v.unique_json(row["content"].encode());require(not value.get("error"),"actual fresh G tool refused")
                    results[row["tool_call_id"]]=value
            if "g-recovered-jobs" in results:
                self.read_result=results["g-recovered-jobs"]
                return self.done(ANSWER)
            offered={t["function"]["name"] for t in body["tools"]}
            if "setup_jobs" not in offered:
                if "g-recovered-search" not in results:
                    return self.call(body,"g-recovered-search","tool_catalog_search",{"query":"setup_jobs","limit":50})
                require(any(row["name"]=="setup_jobs" for row in results["g-recovered-search"]["results"]),"fresh setup surface absent")
                return self.call(body,"g-recovered-load","tool_catalog_load",{"names":["setup_jobs"]})
            return self.call(body,"g-recovered-jobs","setup_jobs",{},context)
    class Roster:
        def __init__(self,plan,phase):
            self.initial=copy.deepcopy(plan["application"]["roster"]);self.publications=1
            self.path=Path(phase["roster"]["path"]);self.admitting=Path(phase["admitting"]["path"])
            self.pins=(phase["roster"],phase["admitting"])
        def selected(self):
            for pin in self.pins: checked_pin(v,pin)
            rows=v.unique_json(checked_pin(v,self.pins[0]))
            require(rows==self.initial+[u.DesiredRoster.spec()],"same actual application G listing required")
            return rows
        async def roster(self,context):
            require(context.mob_definition is not None,"actual native roster context missing")
            return [v.META_IDENTITY.DurableAgentSpec.from_dict(row) for row in self.selected()]
        async def compute_edges(self,identities,context): return []
        async def retain_empty_mcp(self,value): raise AssertionError("same-G reader cannot publish another ordinary member")
        async def lookup_empty_mcp(self,value): raise AssertionError("same-G has no ordinary member binding to restore")
    class Scenario(u.Scenario):
        phase="same-g-recovery"
        scope="same_canonical_g_r5_and_fresh_read"
        def configure_existing(self,config,plan,phase,source):
            self.plan=plan;self.native_evidence_directory=source
            self.roster=Roster(plan,phase);self.roster.selected()
            profiles=tomllib.loads(plan["application"]["definition"]["text"])["profiles"]
            self.customizer=u.AcceptanceCustomizer(profiles[u.C3_CLASS]["model"],profiles[u.CLASS_ID]["model"])
            self.member_types=importlib.import_module("meerkat_toolkit.member_classes")
            self.member_mcp=self.member_types.MemberMcpApplication(self.roster.retain_empty_mcp,self.roster.lookup_empty_mcp)
            config.agent_customizer=self.customizer;config.roster_provider=config.topology_provider=self.roster
            config.workgraph_enabled=True;config.console_read_only=False;config.console_require_app_auth=True
            config.http_listen="127.0.0.1:"+str(self.port);config.http_public_base_url=self.origin
            config.auth_config={"provider":"jwt","shared_secret":c.KEY.decode(),"issuer":c.ISSUER,"audience":c.AUDIENCE,"email_allowlist":[c.EMAIL]}
            config.access_config_path=str(source/"console-access.toml");config.console_config_path=str(source/"console.toml")
            launch=phase["launch"];record=phase["canonical"]["canonical"]["record"]
            parents=[row for row in launch["graph_receipts"] if row.get("owner",{}).get("key")=={"kind":"agent","id":"supplied-parent-owner"}]
            require(len(parents)==1,"one actual retained P work claim required")
            parent=parents[0];parent_work={"realm_id":parent["realm_id"],"namespace":parent["namespace"],"item_id":parent["id"]}
            self.workflow_context=(v.SDK.PrivateOriginalRef(**launch["parent"]["original"]),launch["parent"]["admission"],
                launch["commission"],launch["canonical_g"],record["instance"]["member"]["member"],parent_work,launch["graph_receipts"])
    return Provider,Scenario

async def native_observation(v,directory,phase):
    def read():
        address=v.unique_json(v.exact_bytes(directory/"socket.json"))["path"]
        sock=socket.socket(socket.AF_UNIX,socket.SOCK_STREAM);sock.settimeout(75)
        try:
            sock.connect(address);sock.sendall(v.canonical({"phase":phase}));sock.shutdown(socket.SHUT_WR);raw=bytearray()
            while True:
                chunk=sock.recv(65536)
                if not chunk:break
                raw.extend(chunk);v.require(len(raw)<=8*1024*1024,"bounded native G observation")
            result=v.unique_json(bytes(raw));v.require(result.get("status")=="observed" and result["phase"]==phase,"native G observation refused")
            v.require(result["value"]==v.unique_json(v.exact_bytes(directory/(phase+".json"))),"native retained output differs")
            return result["value"]
        finally:sock.close()
    return await asyncio.to_thread(read)

async def run(u,args,old,failed,capture,phase,source,evidence,readback,package,binary):
    v,c,s,require=u.v,u.c,u.s,u.require
    import ast
    # Read the pinned original launcher literal, never execute or reinterpret it.
    original_wrapper=ast.parse(checked_pin(v,old["cases"][0]["wrapper"]).decode())
    assignments=[n for n in original_wrapper.body if isinstance(n,ast.Assign) and any(isinstance(t,ast.Tuple)
        and [e.id for e in t.elts if isinstance(e,ast.Name)]==["binary","selector","report","origin"] for t in n.targets)]
    require(len(assignments)==1,"original G launcher identity unavailable")
    original_binary,original_selector,original_owner,origin=ast.literal_eval(assignments[0].value)
    require(Path(original_binary)==Path(old["candidate"]["executable"]["path"])
        and original_selector=="application_installed::canonical_g_capture::published_g_pending_consent_survives_actual_owner_loss"
        and Path(original_owner)==args.predecessor/"g-owner","wrong predecessor launcher")
    config_path=Path(failed["model_configuration"]["path"])
    model_config=tomllib.loads(checked_pin(v,failed["model_configuration"]).decode())
    model_url=urlsplit(model_config["self_hosted"]["servers"]["local"]["base_url"])
    require(model_url.scheme=="http" and model_url.hostname=="127.0.0.1" and model_url.port and model_url.path=="/v1",
        "exact original controlled model endpoint required")
    plan=v.unique_json(checked_pin(v,failed["preparation_plan"]))
    require(Path(plan["application"]["realm"]["state_root"]).resolve()==(source/"state").resolve(),
        "same canonical native store root required")
    completed=failed["finalization"]["completed_setup"];completed_raw=checked_pin(v,completed)
    selected=v.SDK.HostBootstrapSelection(failed["application_selection"]["deployment_root"],
        **{key:v.SDK.SelectedHostArtifact(**failed["application_selection"][key]) for key in ("activation","policy_manifest","local_policy")})
    # Select the actual SDK type used by successful Restore, with no plan finalizer.
    # HostBootstrapSelection is validated by the installed private startup again.
    Provider,Scenario=types(u,model_url.port)
    scenario=Scenario(v,origin);provider=None;runtime=transport=process=private=public_reader=private_reader=None
    resource_root=package/"libexec/meerkat-connect"
    resource_manifest=v.unique_json(v.exact_bytes(resource_root/"resources.json",args.resources_sha256,limit=32*1024*1024))
    resource_python=(resource_root/resource_manifest["python"]).resolve(strict=True)
    require(resource_python.is_relative_to(resource_root.resolve()),"resource interpreter escaped selected package")
    events=v.EventLog(args.output);observed_children={};outgoing=[];responses=[];settlements=[]
    report={"schema":1,"status":"running","scope":"same_canonical_g_r5_and_fresh_read",
        "expected_owner_held":args.expect_owner_held,"same_predecessor":evidence,
        "predecessor_readback":readback,"source_directory":str(source),"forced_cleanup":False,
        "new_application_preparation":False,"new_workflow_commission":False}
    owner_output=args.output/"g-reader";owner_output.mkdir(mode=0o700)
    gateway=package/"bin/toolkit_gateway"
    gateway.write_text(wrapper(Path(sys.executable).resolve(),binary,owner_output,args.predecessor/"g-owner/g-capture.json",origin))
    gateway.chmod(0o700)
    environment=v.process_env(source)
    empty=args.output/"empty-path";empty.mkdir(mode=0o700);environment["PATH"]=str(empty)
    stderr=args.output/"gateway.stderr";environment["META_GATEWAY_STDERR_FILE"]=str(stderr)
    cwd=Path.cwd();enrollment=None
    def observe():
        require(process is not None and process.poll() is None,"same actual reader process is not running")
        rows=[{**r,"process_stat":v.descendant_process_stat(r["pid"])} for r in v.child_snapshot(process.pid)]
        for row in rows:observed_children[row["pid"]]=row
        report.setdefault("descendant_snapshots",[]).append({"main_pid":process.pid,"monotonic_ns":time.monotonic_ns(),"children":rows})
        report["observed_children"]=list(observed_children.values())
    class ObservedRestore(v.SDK.ToolkitMobKitRuntime):
        def _make_gateway_transport(self):
            nonlocal transport,process,private,public_reader,private_reader
            transport=super()._make_gateway_transport();transport._env=dict(environment)
            write,send,deliver,start=transport._write_line,transport._send_sync_running,transport._deliver_init_event,transport.start
            def observed_write(value,**kwargs):
                outgoing.append(copy.deepcopy(value));events.add("sdk_wire_out",message=value);return write(value,**kwargs)
            def observed_send(request,*,timeout=None,**kwargs):
                value=send(request,timeout=timeout,**kwargs);responses.append(copy.deepcopy(value));events.add("sdk_wire_response",message=value);return value
            def observed_deliver(method,params,**kwargs):
                settlements.append({"method":method,"params":copy.deepcopy(params)});events.add("sdk_init_event",method=method,params=params)
                return deliver(method,params,**kwargs)
            def observed_start():
                nonlocal process,private,public_reader,private_reader
                # Ordinary Restore bootstrap can synchronously close and clear
                # host_control. Observe its already-created real owner before
                # forwarding that call, not after failure erased the slot.
                bootstrap=v.SDK.PrivateHostClient.bootstrap
                def observed_bootstrap(client,selection,**kwargs):
                    nonlocal process,private,public_reader,private_reader
                    require(client is transport.host_control and private is None,
                        "one actual private bootstrap owner required")
                    process,private=transport._process,client
                    public_reader,private_reader=transport._reader_thread,client._reader
                    require(process is not None and public_reader is not None and private_reader is not None,
                        "actual process and both readers must precede bootstrap")
                    events.add("same_g_gateway_started",pid=process.pid,argv=process.args)
                    return bootstrap(client,selection,**kwargs)
                v.SDK.PrivateHostClient.bootstrap=observed_bootstrap
                try:return start()
                finally:
                    v.SDK.PrivateHostClient.bootstrap=bootstrap
                    # Keep any partial actual owner for failed-path cleanup.
                    if process is None:process=transport._process
                    if public_reader is None:public_reader=transport._reader_thread
            transport._write_line,transport._send_sync_running=observed_write,observed_send
            transport._deliver_init_event,transport.start=observed_deliver,observed_start
            return transport
    try:
        provider=Provider(args.output,events)
        require(provider.port==model_url.port,"provider endpoint changed from the actual stored model selection")
        config=v.META_BUILDER.MetaBuilderConfig(gateway_bin=str(gateway),init_deadline=v.INSTALLED_LIMIT,
            gateway_init_timeout=v.INSTALLED_LIMIT,persistent_state=plan["application"]["realm"]["state_root"],
            meerkat_config_path=str(config_path),prepared_application_bundle=completed_raw,prepared_application_digest=completed["sha256"])
        scenario.configure_existing(config,plan,phase,source)
        report["unchanged_runtime_configuration"]=[v.artifact_pin(config_path),v.artifact_pin(Path(config.access_config_path)),v.artifact_pin(Path(config.console_config_path))]
        runtime=ObservedRestore(config,selected,**scenario.application_runtime_options());scenario.observe_runtime(runtime)
        os.chdir(source)
        connect_error=None
        try:await runtime.connect()
        except BaseException as failure:
            connect_error=failure;report["connect_error"]={"type":type(failure).__name__,"message":str(failure)}
        if args.expect_owner_held:
            require(connect_error is not None and not any(x["params"].get("outcome")=="ready" for x in settlements),
                "held original issuer must refuse before native Ready")
            refusal=v.unique_json(v.exact_bytes(owner_output/"issuer-refusal.json"))
            require(refusal=={"category":"owner_held","phase":"before_runtime_observer","identity":capture["identity"],"issuer":capture["issuer"]},
                "failure did not discriminate actual held issuer")
            require(not provider.requests and not any((owner_output/(name+".json")).exists() for name in
                ("before-native","native-repeat","one-shot","ready","after-work")),"negative entered native recovery or effects")
            report["issuer_refusal"]=v.artifact_pin(owner_output/"issuer-refusal.json")
        else:
            if connect_error is not None:raise connect_error
            require(process is not None and process.pid!=capture["pid"] and runtime.is_running,"fresh reader process required")
            initialized=[x for x in outgoing if x.get("method")=="meta/init"]
            require(len(initialized)==1 and "mob_config" not in initialized[0]["params"]
                and "application_tool_policies" not in initialized[0]["params"]
                and initialized[0]["params"].get("prepared_application")=={
                    "bundle_base64":base64.b64encode(completed_raw).decode("ascii"),"digest":completed["sha256"]},
                "Restore must authenticate the same native completed application carrier")
            ready=[x for x in settlements if x["method"]=="meta/init_settled" and x["params"].get("outcome")=="ready"
                and x["params"].get("init_id")==initialized[0]["params"]["init_id"]]
            require(len(ready)==1 and runtime.rust_http_base_url==origin,"actual correlated same-origin Ready required")
            observe();before=await native_observation(v,owner_output,"ready")
            native_repeat=v.unique_json(v.exact_bytes(owner_output/"native-repeat.json"))
            require(native_repeat["repeat_convergence_count"]==0 and native_repeat["repeat_owner_loss"]=="not_owed"
                and native_repeat["durable_rows_closed"]==2 and before["aggregate_value"]["native"]["census"]==native_repeat["complete_census"]
                and before["aggregate_value"]["consent_records"]==native_repeat["consent_records"]
                and before["aggregate_value"]["physical"]==readback["physical_census"],"full same-G native/consent/physical join required")
            require(before["session"]==capture["session_id"] and before["document"]["predecessor"]==capture["association"]
                and before["document"]["physical"]!=capture["physical"]
                and before["document"]["approval_source"]!=capture["association_document"]["approval_source"],"actual fresh G preparation/publication missing")
            before_rows=s.input_rows(source,capture["session_id"])
            require(len(before_rows)==2,"Ready must not execute recovered model work")
            with provider.lock:
                require(not provider.requests,"model entered before explicit fresh work");provider.enabled=True;provider.deadline=time.monotonic()+360
            scope="same-g-reader-"+uuid.uuid4().hex;expires=int(time.time())+600
            enrollment=private.begin_console_login(v.SDK.ConsoleLoginSelection(c.ISSUER,c.SUBJECT,scope,expires*1000),timeout=10)
            await asyncio.to_thread(enrollment.result);scenario.console_workflow_login=enrollment;scenario.console_workflow_token=c.signed_login(scope,expires)
            deadline=time.monotonic()+360
            launch=await scenario.launch_children(private,provider,args.output,events,process,deadline,tags=("b",))
            require(len(launch["children"])==1 and len(launch["negative_controls"])==5,"actual child plus all five owner refusals required")
            child=launch["children"][0];workflow=capture["admission"]["workflow"]["workflow"]
            while time.monotonic()<deadline:
                with provider.lock:require(not provider.errors,"actual controlled model refused")
                delivery=u.context_delivery(source,child["source_key"],workflow,capture["session_id"])
                rows=s.input_rows(source,capture["session_id"])
                if delivery and u.finalized_context(rows,delivery,ANSWER):break
                await asyncio.sleep(.05)
            else:raise AssertionError("fresh authenticated B context did not complete on the same restored G")
            actual=u.finalized_context(rows,delivery,ANSWER)
            require(len(rows)==3 and all(row in rows for row in before_rows)
                and delivery["input"] not in {r["input_id"] for r in before_rows},"old G inputs changed or fresh context reused predecessor")
            audit=actual["authorization_audit"]
            require(audit and all([x["input_id"] for x in row["contributors"]]==[delivery["input"]] for row in audit),
                "fresh operation borrowed predecessor authority")
            operation=s.public_operation_audit(audit,"setup_jobs","g-recovered-jobs")
            require(provider.read_result is not None and provider.context["membership"]==delivery["membership"],"actual read result/context missing")
            after=await native_observation(v,owner_output,"after-work");observe()
            require(after["association"]==before["association"] and after["document"]==before["document"]
                and after["issuer_rows"]==before["issuer_rows"] and len(after["inputs"])==3,"fresh work replaced G or changed old consent history")
            report.update(native_ready=ready[0],before=before,after=after,fresh_child=launch,delivery=delivery,
                fresh_input=actual,native_operation=operation,read_result=provider.read_result,
                fresh_physical=u.shared_peer(source,process.pid,str(resource_python),
                    physical=after["document"]["physical"]))
            require(report["fresh_physical"]["lifetime"]["resources_sha256"]==args.resources_sha256,
                "actual fresh physical owner uses different selected resources")
        report["main_gateway_pid"]=process.pid
        report["status"]="observed_awaiting_cleanup"
    except BaseException as failure:
        report.update(status="failed",error=repr(failure),traceback=traceback.format_exc())
    finally:
        if provider is not None:provider.release_original_responses(abort=True)
        if enrollment is not None:
            try:
                retired=await asyncio.to_thread(enrollment.retire,timeout=30)
                require(retired.fenced and retired.reservations_drained and not retired.uncertain,"real login retirement did not join")
                report["login_retirement"]=dataclasses.asdict(retired)
            except BaseException as failure:report.update(status="failed",login_cleanup_error=repr(failure))
        if runtime is not None:
            try:await runtime.shutdown()
            except BaseException as failure:
                report["shutdown_error"]=repr(failure)
                if not args.expect_owner_held:report["status"]="failed"
        if transport is not None:
            term=transport.last_termination
            if term is not None:report["termination"]=dataclasses.asdict(term)
            cleanup={"process_observed":process is not None,"process_reaped":process is not None and process.poll() is not None,
                "transport_process_released":transport._process is None,"public_reader_created":public_reader is not None,
                "public_reader_joined":public_reader is not None and not public_reader.is_alive(),
                "private_reader_created":private_reader is not None,"private_reader_joined":private_reader is not None and not private_reader.is_alive(),
                "pending_responses_empty":not transport._pending and not transport._results,"observed_descendants_gone":False}
            report["cleanup_observation"]=cleanup
            try:
                v.require_descendants_gone(report,observed_children);cleanup["observed_descendants_gone"]=True
                require(set(cleanup)==CLEANUP and all(value is True for value in cleanup.values()),"all actual native/SDK/descendant owners must join")
                require(term is not None and term.state=="exited" and term.signal is None and term.escalation=="none"
                    and term.exit_code==(1 if args.expect_owner_held else 0),"actual child terminal exit/cleanup differs")
                if not args.expect_owner_held:
                    shutdown=[x for x in outgoing if x.get("method")=="meta/shutdown"]
                    require(len(shutdown)==1,"one actual same-owner shutdown request required")
                    reply=[x for x in responses if str(x.get("id"))==str(shutdown[0]["id"])]
                    require(len(reply)==1 and "error" not in reply[0] and all(reply[0].get("result",{}).get(k) is True
                        for k in ("shutdown","runtime_cleanup_completed","host_cleanup_completed")),"native cleanup acknowledgement missing")
                    report["shutdown_response"]=reply[0]
                with socket.socket() as probe:
                    probe.settimeout(.25);require(probe.connect_ex(("127.0.0.1",scenario.port))!=0,"HTTP listener survived native join")
                returned=v.unique_json(v.exact_bytes(owner_output/"installed-return.json"))
                require(returned["returned"] is True and returned["runtime_observation_released"] is True
                    and returned["observer_error"] is None and ((returned["error"] is not None) if args.expect_owner_held else returned["error"] is None),
                    "installed owner/observer return differs")
                report["installed_return"]=v.artifact_pin(owner_output/"installed-return.json")
            except BaseException as failure:report.update(status="failed",cleanup_error=repr(failure))
        else:report.update(status="failed",cleanup_error="actual transport absent")
        if provider is not None:
            try:provider.close(drain=True);report["provider_joined"]=True
            except BaseException as failure:report.update(status="failed",provider_joined=False,provider_cleanup_error=repr(failure))
            report["model_requests"]=v.artifact_pin(args.output/"model-requests.json")
            report["model_schedule_errors"]=provider.errors
            if provider.errors:report["status"]="failed"
        os.chdir(cwd);events.flush();report["events"]=v.artifact_pin(args.output/"events.json")
        if stderr.exists():report["stderr"]=v.artifact_pin(stderr)
        try:
            for pin in evidence:checked_pin(v,pin)
            for pin in report.get("unchanged_runtime_configuration",[]):checked_pin(v,pin)
            require(v.digest(binary)==old["candidate"]["emitted"]["sha256"],"selected emitted bytes changed")
        except BaseException as failure:report.update(status="failed",immutable_input_error=repr(failure))
        if report["status"]=="observed_awaiting_cleanup":
            report["native_records"]=[v.artifact_pin(owner_output/(name+".json")) for name in
                (("issuer-refusal","installed-return") if args.expect_owner_held else
                 ("before-native","native-repeat","one-shot","ready","after-work","installed-return"))]
            report["status"]="passed"
        v.save_json(args.output/"result.json",report)
    return report

def main():
    parser=argparse.ArgumentParser(description=__doc__)
    for name in ("test-executable","resource-executable","toolkit-python","meta-python","predecessor","output"):
        parser.add_argument("--"+name,required=True,type=Path)
    parser.add_argument("--selector",required=True);parser.add_argument("--resources-sha256",required=True)
    parser.add_argument("--expect-owner-held",action="store_true");args=parser.parse_args()
    for name in ("test_executable","resource_executable","toolkit_python","meta_python","predecessor","output"):
        path=getattr(args,name)
        if not path.is_absolute():parser.error("absolute selected paths required")
        setattr(args,name,path.resolve())
    if args.selector!=SELECTOR or args.output.exists():parser.error("exact selected reader and fresh output required")
    scripts=Path(__file__).resolve().parent.parent;u=load("same_g_supplied",scripts/"verify_supplied_shared_workflow.py");v=u.v
    args.output.mkdir(mode=0o700,parents=True)
    report={"status":"failed","scope":"same_canonical_g_r5_and_fresh_read"}
    try:
        sys.path[:0]=[str(args.toolkit_python),str(args.meta_python)]
        sources=[]
        for name,field,root in (("meerkat_toolkit.host_control","SDK",args.toolkit_python),
            ("meerkat_toolkit.application_runtime","APP_RUNTIME",args.toolkit_python),
            ("meerkat_meta.builder","META_BUILDER",args.meta_python),
            ("meerkat_meta.identity_first_models","META_IDENTITY",args.meta_python)):
            module=importlib.import_module(name);path=Path(module.__file__).resolve()
            v.require(path.is_relative_to(root),"SDK import escaped selection");setattr(v,field,module);sources.append(v.artifact_pin(path))
        u.COMMISSION=importlib.import_module("meerkat_toolkit.workflow_commission")
        modules=[(u.COMMISSION,args.toolkit_python)]
        modules.extend((importlib.import_module(name),args.meta_python) for name in (
            "meerkat_meta","meerkat_meta.runtime","meerkat_meta._transport","meerkat_meta.agent_builder",
            "meerkat_meta.identity_first_providers","meerkat_meta.types"))
        modules.extend((importlib.import_module(name),args.toolkit_python) for name in ("meerkat_toolkit.member_classes",))
        for module,root in modules:
            path=Path(module.__file__).resolve();v.require(path.is_relative_to(root),"SDK import escaped selection");sources.append(v.artifact_pin(path))
        old,failed,capture,phase,source,evidence,readback=predecessor(v,args)
        # Validate every actual SDK source recorded by this producer. No import
        # result from a different release/checkout can qualify the reader.
        for pin in old["sdk_sources"]:checked_pin(v,pin)
        old_modules={pin["path"]:pin["sha256"] for pin in old["sdk_sources"]}
        v.require(all(old_modules.get(pin["path"])==pin["sha256"] for pin in sources),
            "actual reader imports differ from the selected producer SDK files")
        for pin in old["fixture_sources"]:checked_pin(v,pin)
        resource_root=args.resource_executable.parent.parent/"libexec/meerkat-connect"
        raw=v.exact_bytes(resource_root/"resources.json",args.resources_sha256,limit=32*1024*1024)
        package=args.output/"native-test-package";(package/"bin").mkdir(mode=0o700,parents=True)
        shutil.copytree(resource_root,package/"libexec/meerkat-connect",symlinks=True)
        binary=package/"bin/connect-lib-test";shutil.copy2(args.test_executable,binary)
        v.require(v.digest(binary)==old["candidate"]["emitted"]["sha256"],"selected emitted copy differs")
        archive=args.output/"predecessor-evidence";archive.mkdir(mode=0o700)
        for number,pin in enumerate(evidence):
            (archive/(str(number)+".json")).write_bytes(checked_pin(v,pin))
        v.save_json(archive/"INDEX.json",evidence)
        report=asyncio.run(run(u,args,old,failed,capture,phase,source,evidence,readback,package,binary))
        report["sdk_sources"]=sources;report["producer"]=v.artifact_pin(args.predecessor/"result.json")
        report["reader_source"]=v.artifact_pin(Path(__file__).resolve());report["binary"]=v.artifact_pin(binary)
        v.require(v.exact_bytes(resource_root/"resources.json")==raw,"resource identity changed")
    except BaseException as failure:report.update(status="failed",error=repr(failure),traceback=traceback.format_exc())
    finally:v.save_json(args.output/"result.json",report)
    return 0 if report["status"]=="passed" else 1
if __name__=="__main__":raise SystemExit(main())
