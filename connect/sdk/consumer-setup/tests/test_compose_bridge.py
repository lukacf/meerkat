"""Real local subprocess transport tests with inert pinned backend owners.

These fixtures test setup composition, not Docker or canonical package validity.
"""
import copy
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import threading
import time
import unittest
from unittest.mock import patch


ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("compose_setup_adapter", ROOT / "mcp_adapter.py")
adapter = importlib.util.module_from_spec(spec)
spec.loader.exec_module(adapter)
backend = adapter.backend

COMMON = '''
import hashlib,json,sys,time
from pathlib import Path
ROOT = Path(ROOT_LITERAL)
def enc(x): return (json.dumps(x,sort_keys=True,separators=(",",":"),ensure_ascii=False)+"\\n").encode()
def sha(x): return hashlib.sha256(x).hexdigest()
def control(): return json.loads((ROOT/"control.json").read_text())
def log(action):
    with (ROOT/"calls.jsonl").open("a") as f:f.write(json.dumps({"action":action,"pid":__import__("os").getpid()})+"\\n")
def plan(raw):
    req=json.loads(raw); meta=json.loads(req["manifest_json"])
    if meta.get("reject"): raise ValueError("fixture metadata refusal")
    component={"service":"mcp","lifetime":"request","image":"sha256:"+"a"*64,"image_id":"sha256:"+"a"*64,"platform":"linux/amd64","dependencies":[],"deployment_dependencies":[],"readiness":"running","stop_timeout_ms":1000,"command":["PRIVATE_COMMAND"],"environment":{"PRIVATE_ENV":"value"},"unknown":{"contents":"PRIVATE_FUTURE_FIELD"}}
    contents={"compose.json":"PRIVATE_COMPOSE_JSON","inputs/configuration":req["configuration"],"inputs/policy":req["policy"],**{"inputs/files/"+k:v for k,v in req["files"].items()}}
    p={"request_sha256":sha(raw),"package_sha256":sha(req["package_json"].encode()),"manifest_sha256":sha(req["manifest_json"].encode()),"manifest_source_sha256":sha(req["manifest_json"].encode()),"configuration_sha256":sha(req["configuration"].encode()),"policy_sha256":sha(req["policy"].encode()),"profile_sha256":req["profile_sha256"],"host_owner":"fixture","project":"fixture-project","components":{"mcp":component},"files":{k:{"contents":v,"sha256":sha(v.encode()),"mode":292} for k,v in contents.items()},"initialization_order":["mcp"],"shutdown_order":["mcp"],"deployment_order":[],"request_components":["mcp"],"compose_json":"PRIVATE_COMPOSE_JSON","engine":{"private":"PRIVATE_ENGINE"},"padding":"x"*control().get("padding",0)}
    if control().get("namespaced_deployment"):
        p["components"]["vendor:worker"]={**component,"lifetime":"deployment"}
        p["initialization_order"]=["vendor:worker","mcp"];p["shutdown_order"]=["mcp","vendor:worker"];p["deployment_order"]=["vendor:worker"]
    if control().get("migration_metadata"):
        p["state_migration"]={"apply":{"executable":"PRIVATE_MIGRATION_APPLY","arguments":[]},"readback":{"executable":"PRIVATE_MIGRATION_READBACK","arguments":[]}}
    return {"schema_version":1,"status":"prepared","plan_sha256":sha(enc(p)),"plan":p}
def summary(reply):
    p=reply["plan"]
    s={k:p[k] for k in ("request_sha256","package_sha256","manifest_sha256","manifest_source_sha256","profile_sha256","configuration_sha256","policy_sha256","host_owner","project","initialization_order","shutdown_order","deployment_order","request_components")}
    s["components"]={name:{k:item[k] for k in ("service","lifetime","image","image_id","platform","dependencies","deployment_dependencies","readiness","stop_timeout_ms")} for name,item in p["components"].items()}
    s["files"]={name:{"sha256":item["sha256"],"mode":item["mode"]} for name,item in p["files"].items()}
    return {"schema_version":1,"status":"prepared_summary","plan_sha256":reply["plan_sha256"],"summary":s}
'''

PLANNER = '''
log("planner")
try: result=plan(sys.stdin.buffer.read())
except Exception: sys.stderr.write("fixture_refusal\\n"); sys.exit(1)
sys.stdout.buffer.write(enc(result))
'''

RUNTIME = '''
def arg(name): return sys.argv[sys.argv.index(name)+1]
graph=Path(arg("--workspace")); action=sys.argv[sys.argv.index("--planner-sha256")+2]
if action=="selection":
    prepared=json.loads((graph/"prepared.json").read_text()); state=json.loads((graph/"fixture-state.json").read_text())
    proposal=json.loads((graph/"proposal.json").read_text()) if "--transition-sha256" in sys.argv else None
    if proposal and proposal["transition_sha256"]!=arg("--transition-sha256"): sys.exit(1)
    result={"schema_version":1,"status":"selected_plan","selected":summary(prepared),"state_namespaces":control().get("state_namespaces",["data"]),"selected_from_transition_sha256":state.get("selected_from"),"pending_transition":state.get("pending"),"pending_operation":state.get("pending_operation",False),"proposal":None if proposal is None else {**proposal,"status":"prepared_successor_summary","predecessor":summary(proposal["predecessor"]),"successor":summary(proposal["successor"])}}
    sys.stdout.buffer.write(enc(result));sys.exit(0)
log(action)
cfg=control()
time.sleep(cfg.get("delay",0))
if cfg.get("nonzero"): sys.stderr.write("PRIVATE_FIXTURE_DETAIL\\n"); sys.exit(2)
if "exit_code" in cfg:
    sys.stdout.write(cfg.get("stdout",""));sys.stdout.flush()
    sys.stderr.write(cfg.get("stderr",""));sys.stderr.flush()
    if cfg.get("signal"): __import__("os").kill(__import__("os").getpid(),15)
    sys.exit(cfg["exit_code"])
if action=="prepare":
    raw=sys.stdin.buffer.read(); result=plan(raw); graph.mkdir()
    (graph/"request.json").write_bytes(raw); (graph/"prepared.json").write_bytes(enc(result))
    (graph/"fixture-state.json").write_bytes(enc({"running":False,"generation":0}))
elif action=="prepare-successor":
    raw=sys.stdin.buffer.read(); prior=json.loads((graph/"prepared.json").read_text())
    if prior["plan_sha256"]!=arg("--predecessor-plan-sha256"):sys.exit(1)
    result={"schema_version":cfg.get("successor_schema",1),"status":"prepared_successor","transition_sha256":sha((graph/"request.json").read_bytes()+raw),"predecessor":prior,"successor":plan(raw)}
    (graph/"proposal.json").write_bytes(enc(result))
else:
    prepared=json.loads((graph/"prepared.json").read_text())
    state=json.loads((graph/"fixture-state.json").read_text())
    def obs():
        x={"schema_version":1,"plan_sha256":prepared["plan_sha256"],"containers":{},"volumes":{},"network":None,"available":{"db":state["running"]},"pending_operation":state.get("pending_operation",False),"generation":state["generation"]}
        x["observation_sha256"]=sha(enc(x)); return x
    def observed():
        if not state.get("pending"):return obs()
        x={"status":"successor_pending","transition_sha256":state["pending"]["transition_sha256"],"phase":state["pending"]["phase"],"observation":obs(),"custody_sha256":sha(enc(state))}
        x["observation_sha256"]=sha(enc(x));return x
    if action in ("resume","stop","remove","adopt-successor","reconcile-successor") and arg("--observation-sha256")!=observed()["observation_sha256"]:sys.exit(3)
    if action in ("adopt-successor","reconcile-successor"):
        proposal=json.loads((graph/"proposal.json").read_text())
        if proposal["transition_sha256"]!=arg("--transition-sha256"):sys.exit(1)
        phase=cfg.get("pending_phase","activate") if cfg.get("pending") else None
        prepared=proposal["predecessor" if phase in ("retire","migrate") else "successor"]
        state["selected_from"]=None if phase in ("retire","migrate") else proposal["transition_sha256"]
        state["pending"]={"transition_sha256":proposal["transition_sha256"],"phase":phase,"predecessor_plan_sha256":proposal["predecessor"]["plan_sha256"],"successor_plan_sha256":proposal["successor"]["plan_sha256"]} if phase else None
        (graph/"prepared.json").write_bytes(enc(prepared))
    if action!="observe":
        state["running"]=action in ("start","resume","adopt-successor","reconcile-successor");state["generation"]+=1
        (graph/"fixture-state.json").write_bytes(enc(state));log("effect:"+action)
    observation=observed()
    if cfg.get("invalid_observation"): observation["pending_operation"]="false"
    result=observation if action=="observe" else {"status":"unconfirmed" if cfg.get("unconfirmed") or state.get("pending") else "observed","plan_sha256":prepared["plan_sha256"],"observation":None if state.get("pending") else observation}
    if action in ("adopt-successor","reconcile-successor"):result["transition_sha256"]=proposal["transition_sha256"]
if cfg.get("wrong_plan"): result["plan_sha256"]="f"*64
sys.stdout.buffer.write(enc(result))
'''



class ComposeFixture:
    def setUp(self):
        self.tmp = tempfile.TemporaryDirectory()
        self.root = Path(self.tmp.name).resolve()
        self.staging = self.root / "staging"
        self.staging.mkdir()
        self.graph = self.root / "graph"
        self.sequence = 0
        self.controllers = []
        self.configure()
        self.python = Path(sys.executable).resolve()
        for name, source in (("planner", PLANNER), ("runtime.py", RUNTIME)):
            path = self.root / name
            path.write_text("#!" + str(self.python) + "\n" + COMMON.replace("ROOT_LITERAL", repr(str(self.root))) + source)
            path.chmod(0o700)
        self.profile_path = self.root / "backend-profile.json"
        self.profile_path.write_bytes(backend.encoded({"volumes": {"data": {}}}))
        self.host = {"schema_version": 1, "scope": "execute_agent_selected_local_container_plan",
            "staging_root": str(self.staging), "graph_workspace": str(self.graph), "mode": "new",
            "expected_plan_sha256": None, "operation_timeout_seconds": 10,
            "allowed_actions": ["prepare", "start", "observe", "resume", "stop", "remove"],
            "delete_state_namespaces": ["data"]}
        for key, path in (("python", self.python), ("runtime", self.root / "runtime.py"),
                          ("planner", self.root / "planner"), ("backend_profile", self.profile_path)):
            self.host[key] = {"path": str(path), "sha256": backend.digest(path.read_bytes())}

    def tearDown(self):
        for controller in self.controllers:
            controller.close_admission()
            controller.join()
        self.tmp.cleanup()

    def configure(self, **values):
        (self.root / "control.json").write_text(json.dumps(values))

    def calls(self):
        path = self.root / "calls.jsonl"
        return [json.loads(line)["action"] for line in path.read_text().splitlines()] if path.exists() else []

    def controller(self, **kwargs):
        self.sequence += 1
        host_path = self.root / ("host-%s.json" % self.sequence)
        host_path.write_bytes(backend.encoded(self.host))
        owner = backend.Controller(None, None, self.root / ("session-%s" % self.sequence), (),
            str(self.python), None, None, compose_profile=host_path,
            compose_profile_sha256=backend.digest(host_path.read_bytes()), **kwargs)
        self.controllers.append(owner)
        return owner

    def stage(self, **manifest):
        path = self.staging / "selection"
        path.mkdir(exist_ok=True)
        raw = backend.encoded({"schema_version": 1, "package_json": "{}", "manifest_json": json.dumps(manifest),
                               "configuration": "exact configuration\n", "policy": "{}\n", "files": {"note": "exact file\n"}})
        (path / "inputs.json").write_bytes(raw)
        return {"staged_id": "selection", "inputs_sha256": backend.digest(raw)}

    def action(self, owner, action, request_id=None, **args):
        if action != "observe":
            args["request_id"] = request_id or os.urandom(16).hex()
        return owner.dispatch("/api/compose_" + action, args)

    def job(self, owner, action, **args):
        result = self.action(owner, action, **args)
        owner.join()
        return owner.job_detail({"job_id": result["job"]["id"]})["job"]

    def prepared(self, owner):
        job = self.job(owner, "prepare", **self.stage())
        self.assertEqual(job["status"], "succeeded", job)
        return job["result"]["backend"]["plan_sha256"]

    def successor_host(self, plan, transition=None):
        """Host-selected immutable target; the fixture owns no lifecycle policy."""
        target = self.root / "target-inputs.json"
        if not target.exists():
            target.write_bytes(backend.encoded({"schema_version": 1, "package_json": "{}",
                "manifest_json": '{"version":"B"}', "configuration": "PRIVATE_SUCCESSOR_CONFIG",
                "policy": "PRIVATE_SUCCESSOR_POLICY", "files": {"note": "PRIVATE_SUCCESSOR_FILE"}}))
        profile = self.root / "target-profile.json"
        if not profile.exists():
            profile.write_bytes(backend.encoded({"volumes": {"data": {}}, "fixture_version": "B"}))
        self.host.update(mode="reopen", expected_plan_sha256=plan,
            allowed_actions=["observe", "start", "stop", "resume", "remove", "prepare_successor",
                             "adopt_successor", "reconcile_successor", "rebind"],
            successor={"inputs": {"path": str(target), "sha256": backend.digest(target.read_bytes())},
                "backend_profile": {"path": str(profile), "sha256": backend.digest(profile.read_bytes())},
                "predecessor_plan_sha256": plan, "transition_sha256": transition})

    def prepared_successor(self, owner, plan):
        result = self.job(owner, "prepare_successor", predecessor_plan_sha256=plan,
            inputs_sha256=owner.state()["compose"]["successor"]["inputs_sha256"])
        self.assertEqual(result["status"], "succeeded", result)
        return result["result"]["backend"]


class ComposeBridgeTests(ComposeFixture, unittest.TestCase):
    def test_fresh_successor_host_admits_only_explicit_canonical_dormant_namespaces(self):
        initial = self.controller(); plan = self.prepared(initial)
        self.successor_host(plan)
        target_profile = Path(self.host["successor"]["backend_profile"]["path"])
        target_profile.write_bytes(backend.encoded({"volumes": {}, "fixture_version": "B"}))
        self.host["successor"]["backend_profile"]["sha256"] = backend.digest(target_profile.read_bytes())
        transition_host = self.controller()
        proposal = self.prepared_successor(transition_host, plan)
        observed = self.action(transition_host, "observe")["backend"]
        adopted = self.job(transition_host, "adopt_successor", predecessor_plan_sha256=plan,
            transition_sha256=proposal["transition_sha256"], observation_sha256=observed["observation_sha256"])
        self.assertEqual(adopted["status"], "succeeded", adopted)
        self.host.update(expected_plan_sha256=proposal["successor"]["plan_sha256"],
            backend_profile=self.host.pop("successor")["backend_profile"],
            allowed_actions=["observe", "stop", "remove"], delete_state_namespaces=["vendor:data"])
        self.configure(state_namespaces=["vendor:data"])
        granted = self.controller()
        self.assertEqual(granted.compose.profile["volumes"], {})
        self.assertEqual(granted.compose.selection_state["state_namespaces"], ["vendor:data"])
        self.assertEqual(granted.state()["compose"]["delete_state_namespaces"], ["vendor:data"])
        self.host["delete_state_namespaces"] = []
        absent = self.controller()
        before = self.calls()
        with self.assertRaisesRegex(backend.SetupError, "invalid_compose_arguments"):
            self.action(absent, "remove", plan_sha256=proposal["successor"]["plan_sha256"],
                observation_sha256="f" * 64, delete_state=["vendor:data"])
        self.assertEqual(self.calls(), before)
        self.host["delete_state_namespaces"] = ["foreign"]
        with self.assertRaisesRegex(backend.SetupError, "invalid_compose_profile"):
            self.controller()
        self.assertEqual(self.calls(), before)

    def test_new_host_deletion_grant_still_requires_an_active_profile_namespace(self):
        self.host["delete_state_namespaces"] = ["foreign"]
        with self.assertRaisesRegex(backend.SetupError, "invalid_compose_profile"):
            self.controller()
        self.assertEqual(self.calls(), [])
        self.assertFalse(self.graph.exists())

    def test_canonical_namespaced_deployment_identifier_survives_selection(self):
        self.configure(namespaced_deployment=True)
        owner = self.controller(); plan = self.prepared(owner)
        self.assertEqual(owner.compose.plan["summary"]["deployment_order"], ["vendor:worker"])
        self.assertEqual(self.action(owner, "observe")["runtime_bindings"][0]["component"], "mcp")
        self.host.update(mode="reopen", expected_plan_sha256=plan)
        self.host["allowed_actions"].remove("prepare")
        reopened = self.controller()
        self.assertEqual(reopened.compose.plan, owner.compose.plan)

    def test_successor_requires_explicit_rebind_and_retains_job_replay(self):
        initial = self.controller(); plan = self.prepared(initial)
        self.successor_host(plan); owner = self.controller()
        proposal = self.prepared_successor(owner, plan)
        target, transition = proposal["successor"]["plan_sha256"], proposal["transition_sha256"]
        observed = self.action(owner, "observe")["backend"]
        adopted = self.job(owner, "adopt_successor", predecessor_plan_sha256=plan,
            transition_sha256=transition, observation_sha256=observed["observation_sha256"])
        self.assertEqual(adopted["status"], "succeeded", adopted)
        self.assertEqual(adopted["result"]["runtime_bindings"], [])
        state = owner.state()["compose"]
        self.assertTrue(state["rebind_required"])
        self.assertEqual(state["plan_sha256"], plan)
        self.assertEqual(state["selected_plan_sha256"], target)
        before = self.calls()
        rebound = self.job(owner, "rebind", request_id="a" * 32, plan_sha256=target, transition_sha256=transition)
        self.assertEqual(rebound["status"], "succeeded", rebound)
        self.assertEqual(self.calls(), before)
        self.assertEqual(rebound["result"]["runtime_bindings"][0]["plan_sha256"], target)
        state = owner.state()["compose"]
        self.assertFalse(state["rebind_required"])
        self.assertEqual(state["backend_profile_sha256"], self.host["successor"]["backend_profile"]["sha256"])
        self.assertEqual(self.action(owner, "rebind", request_id="a" * 32, plan_sha256=target,
            transition_sha256=transition)["job"], rebound)
        self.assertEqual(self.calls(), before)
        with self.assertRaisesRegex(backend.SetupError, "compose_request_changed"):
            self.action(owner, "rebind", request_id="a" * 32, plan_sha256=plan, transition_sha256=transition)
        for private in ("PRIVATE_SUCCESSOR", "PRIVATE_COMMAND", "PRIVATE_ENV", "PRIVATE_FUTURE", "PRIVATE_COMPOSE"):
            self.assertNotIn(private, json.dumps([proposal, adopted, rebound, state]))
        self.assertEqual(self.job(owner, "start", plan_sha256=target)["status"], "succeeded")

    def test_pending_reopen_requires_exact_host_transition_and_never_rebinds(self):
        initial = self.controller(); plan = self.prepared(initial)
        self.successor_host(plan); owner = self.controller()
        proposal = self.prepared_successor(owner, plan)
        transition = proposal["transition_sha256"]
        observed = self.action(owner, "observe")["backend"]
        self.configure(pending=True)
        job = self.job(owner, "adopt_successor", predecessor_plan_sha256=plan,
            transition_sha256=transition, observation_sha256=observed["observation_sha256"])
        self.assertEqual(job["status"], "unconfirmed", job)
        self.assertEqual(job["result"]["runtime_bindings"], [])
        with self.assertRaises(backend.SetupError): self.controller()
        self.successor_host(plan, transition); reopened = self.controller()
        pending = self.action(reopened, "observe")
        self.assertEqual(pending["backend"]["status"], "successor_pending")
        self.assertEqual(pending["runtime_bindings"], [])
        self.configure()
        settled = self.job(reopened, "reconcile_successor", transition_sha256=transition,
            observation_sha256=pending["backend"]["observation_sha256"])
        self.assertEqual(settled["status"], "succeeded", settled)
        self.assertEqual(settled["result"]["runtime_bindings"], [])
        target = proposal["successor"]["plan_sha256"]
        self.assertEqual(self.job(reopened, "rebind", plan_sha256=target, transition_sha256=transition)["status"], "succeeded")

    def test_successor_pins_are_required_and_changed_input_is_not_an_agent_selection(self):
        self.host["allowed_actions"].append("prepare_successor")
        with self.assertRaisesRegex(backend.SetupError, "invalid_compose_profile"): self.controller()
        self.host["allowed_actions"].remove("prepare_successor")
        initial = self.controller(); plan = self.prepared(initial)
        self.successor_host(plan); owner = self.controller()
        Path(self.host["successor"]["inputs"]["path"]).write_text("changed")
        before = self.calls()
        result = self.job(owner, "prepare_successor", predecessor_plan_sha256=plan,
            inputs_sha256=self.host["successor"]["inputs"]["sha256"])
        self.assertEqual(result["error"], "compose_host_changed")
        self.assertEqual(self.calls(), before)

    def test_migration_projection_keeps_a_selected_until_explicit_reconciliation(self):
        initial = self.controller(); plan = self.prepared(initial)
        self.successor_host(plan); owner = self.controller()
        self.configure(successor_schema=2, migration_metadata=True)
        proposal = self.prepared_successor(owner, plan)
        self.assertEqual(proposal["schema_version"], 2)
        self.assertIsNone(owner.state()["compose"]["pending_transition"])
        target, transition = proposal["successor"]["plan_sha256"], proposal["transition_sha256"]
        observed = self.action(owner, "observe")["backend"]
        self.configure(pending=True, pending_phase="migrate")
        job = self.job(owner, "adopt_successor", request_id="b" * 32,
            predecessor_plan_sha256=plan, transition_sha256=transition,
            observation_sha256=observed["observation_sha256"])
        self.assertEqual(job["status"], "unconfirmed", job)
        self.assertEqual(job["result"]["backend"]["plan_sha256"], plan)
        self.assertEqual(job["result"]["runtime_bindings"], [])
        state = owner.state()["compose"]
        self.assertEqual(state["selected_plan_sha256"], plan)
        self.assertFalse(state["rebind_required"])
        before = self.calls()
        self.assertEqual(self.action(owner, "adopt_successor", request_id="b" * 32,
            predecessor_plan_sha256=plan, transition_sha256=transition,
            observation_sha256=observed["observation_sha256"])["job"], job)
        self.assertEqual(self.calls(), before)
        with self.assertRaises(backend.SetupError): self.controller()  # Null X does not admit pending migration.
        self.successor_host(plan, transition); reopened = self.controller()
        pending = self.action(reopened, "observe")
        self.assertEqual(pending["backend"]["phase"], "migrate")
        self.assertEqual(pending["backend"]["observation"]["plan_sha256"], plan)
        self.assertEqual(pending["runtime_bindings"], [])
        before = self.calls()
        rejected = self.job(reopened, "rebind", plan_sha256=target, transition_sha256=transition)
        self.assertEqual(rejected["error"], "compose_rebind_unavailable")
        self.assertEqual(self.calls(), before)
        # A rejected operation keeps the existing outer fence. A fresh admitted
        # session can observe and explicitly reconcile; replay never repairs it.
        resumed = self.controller()
        fresh = self.action(resumed, "observe")["backend"]
        self.configure()
        settled = self.job(resumed, "reconcile_successor", transition_sha256=transition,
            observation_sha256=fresh["observation_sha256"])
        self.assertEqual(settled["status"], "succeeded", settled)
        self.assertEqual(settled["result"]["runtime_bindings"], [])
        rebound = self.job(resumed, "rebind", plan_sha256=target, transition_sha256=transition)
        self.assertEqual(rebound["status"], "succeeded", rebound)
        self.assertEqual(rebound["result"]["runtime_bindings"][0]["plan_sha256"], target)
        self.assertNotIn("PRIVATE_MIGRATION", json.dumps([proposal, job, state, pending, settled, rebound]))
        self.assertEqual(self.calls().count("effect:adopt-successor"), 1)
        self.assertEqual(self.calls().count("effect:reconcile-successor"), 1)

    def test_migration_projection_rejects_wrong_selected_plan_and_unsupported_envelopes(self):
        initial = self.controller(); plan = self.prepared(initial)
        self.successor_host(plan); owner = self.controller()
        self.configure(successor_schema=2)
        proposal = self.prepared_successor(owner, plan)
        observed = self.action(owner, "observe")["backend"]
        self.configure(pending=True, pending_phase="migrate")
        self.job(owner, "adopt_successor", predecessor_plan_sha256=plan,
            transition_sha256=proposal["transition_sha256"], observation_sha256=observed["observation_sha256"])
        self.successor_host(plan, proposal["transition_sha256"])
        paths = [self.graph / name for name in ("prepared.json", "proposal.json", "fixture-state.json")]
        originals = [path.read_bytes() for path in paths]
        for mutation in ("selected_b", "legacy_migrate", "future_schema", "boolean_schema", "private_field", "wrong_transition"):
            with self.subTest(mutation=mutation):
                values = [json.loads(raw) for raw in originals]
                if mutation == "selected_b":
                    values[0] = values[1]["successor"]
                    values[2]["selected_from"] = proposal["transition_sha256"]
                elif mutation == "wrong_transition":
                    values[2]["pending"]["transition_sha256"] = "f" * 64
                elif mutation == "private_field": values[1]["receipt"] = "PRIVATE_RECEIPT"
                else: values[1]["schema_version"] = {"legacy_migrate": 1, "future_schema": 3, "boolean_schema": True}[mutation]
                for path, value in zip(paths, values): path.write_bytes(backend.encoded(value))
                before = self.calls()
                with self.assertRaises(backend.SetupError): self.controller()
                self.assertEqual(self.calls(), before)
        for path, raw in zip(paths, originals): path.write_bytes(raw)

    def test_prepare_start_stop_restart_and_reopen_preserve_owner_and_replay(self):
        owner = self.controller()
        plan = self.prepared(owner)
        prepared = owner.job
        self.assertEqual(prepared["kind"], "compose")
        self.assertFalse(prepared["stop_supported"])
        self.assertEqual(self.calls(), ["planner", "prepare"])
        binding = prepared["result"]["runtime_bindings"][0]
        self.assertEqual(binding["scope"], "compose_backend_stdio")
        self.assertEqual(binding["command"], str(self.python))
        self.assertEqual(binding["args"][-2:], ["--component", "mcp"])
        self.assertEqual(binding["env"], {})
        ident = "1" * 32
        start = self.job(owner, "start", request_id=ident, plan_sha256=plan)
        observed = self.action(owner, "observe")["backend"]
        self.assertTrue(observed["available"]["db"])
        stopped = self.job(owner, "stop", plan_sha256=plan, observation_sha256=observed["observation_sha256"])
        self.assertEqual(stopped["status"], "succeeded")
        before = self.calls()
        self.assertEqual(self.action(owner, "start", request_id=ident, plan_sha256=plan)["job"], start)
        self.assertEqual(before, self.calls())
        restart = self.job(owner, "start", plan_sha256=plan)
        self.assertNotEqual(start["id"], restart["id"])
        self.host.update(mode="reopen", expected_plan_sha256=plan)
        self.host["allowed_actions"].remove("prepare")
        other = self.controller()
        self.assertEqual(other.jobs, {})
        self.assertEqual(other.compose.plan, owner.compose.plan)
        self.assertTrue(self.action(other, "observe")["backend"]["available"]["db"])
        self.assertEqual(owner.compose.graph, other.compose.graph)

    def test_invalid_stage_is_prevalidated_without_poisoning_graph(self):
        owner = self.controller()
        job = self.job(owner, "prepare", **self.stage(reject=True))
        self.assertEqual(job["status"], "unconfirmed")
        self.assertEqual(job["error"], "compose_plan_rejected")
        self.assertFalse(self.graph.exists())
        self.assertEqual(self.calls(), ["planner"])
        self.prepared(owner)

    def test_mutation_refusal_before_runtime_and_no_staging_dependency_after_prepare(self):
        owner = self.controller()
        plan = self.prepared(owner)
        (self.staging / "selection/inputs.json").unlink()
        self.assertEqual(self.job(owner, "start", plan_sha256=plan)["status"], "succeeded")
        before = self.calls()
        (owner.workspace / "compose/runtime.py").write_text("changed")
        failed = self.job(owner, "stop", plan_sha256=plan, observation_sha256="a" * 64)
        self.assertEqual(failed["status"], "unconfirmed")
        self.assertEqual(self.calls(), before)

    def test_unconfirmed_owner_result_retained_without_readiness_promotion(self):
        owner = self.controller(); plan = self.prepared(owner)
        self.configure(unconfirmed=True)
        job = self.job(owner, "start", plan_sha256=plan)
        self.assertEqual(job["status"], "unconfirmed")
        self.assertEqual(job["result"]["backend"]["status"], "unconfirmed")
        self.assertFalse(owner.compose.execution_uncertain)
        self.assertIn("observation_sha256", self.action(owner, "observe")["backend"])

    def test_request_conflict_parallel_replay_and_stop_refusal(self):
        owner = self.controller(); plan = self.prepared(owner)
        self.configure(delay=.15)
        args = {"request_id": "2" * 32, "plan_sha256": plan}
        replies = []
        threads = [threading.Thread(target=lambda: replies.append(self.action(owner, "start", **args))) for _ in range(8)]
        for thread in threads: thread.start()
        for thread in threads: thread.join()
        self.assertEqual(len({r["job"]["id"] for r in replies}), 1)
        with self.assertRaisesRegex(backend.SetupError, "job_stop_not_supported"):
            owner.stop({"job_id": replies[0]["job"]["id"]})
        with self.assertRaisesRegex(backend.SetupError, "compose_request_changed"):
            self.action(owner, "start", request_id=args["request_id"], plan_sha256="f" * 64)
        owner.join()
        self.assertEqual(self.calls().count("effect:start"), 1)
        summary = owner.job_summaries({"before": None})["jobs"][0]
        self.assertEqual(summary["action"], "start")
        self.assertFalse(summary["stop_supported"])

    def test_explicit_scope_actions_and_delete_ceiling(self):
        self.host["scope"] = backend.SELECTED_SCOPE
        with self.assertRaisesRegex(backend.SetupError, "invalid_compose_profile"): self.controller()
        self.host["scope"] = "execute_agent_selected_local_container_plan"
        self.host["allowed_actions"] = ["prepare", "observe"]
        owner = self.controller(); plan = self.prepared(owner)
        before = self.calls()
        with self.assertRaisesRegex(backend.SetupError, "compose_action_not_allowed"):
            self.action(owner, "start", plan_sha256=plan)
        with self.assertRaisesRegex(backend.SetupError, "invalid_compose_arguments"):
            self.action(owner, "remove", plan_sha256=plan, observation_sha256="f" * 64, delete_state=["foreign"])
        self.assertEqual(before, self.calls())

    def test_timeout_and_bad_reply_require_new_host_no_automatic_retry(self):
        owner = self.controller(); plan = self.prepared(owner)
        self.configure(nonzero=True)
        job = self.job(owner, "start", request_id="3" * 32, plan_sha256=plan)
        self.assertEqual(job["status"], "unconfirmed")
        self.assertNotIn("PRIVATE_FIXTURE_DETAIL", json.dumps(job))
        self.assertTrue(owner.compose.execution_uncertain)
        self.assertEqual(self.action(owner, "start", request_id="3" * 32, plan_sha256=plan)["job"], job)
        before = self.calls()
        with self.assertRaisesRegex(backend.SetupError, "compose_host_reopen_required"):
            self.action(owner, "observe")
        self.assertEqual(before, self.calls())

    def test_actual_outer_timeout_preserves_graph_and_blocks_session(self):
        self.host["operation_timeout_seconds"] = 1
        owner = self.controller(); plan = self.prepared(owner)
        self.configure(delay=3)
        started = time.monotonic()
        job = self.job(owner, "start", plan_sha256=plan)
        self.assertLess(time.monotonic() - started, 2.5)
        self.assertEqual(job["status"], "unconfirmed")
        self.assertTrue(owner.compose.execution_uncertain)
        self.assertTrue((self.graph / "prepared.json").is_file())
        self.assertNotIn("effect:start", self.calls())

    def test_malformed_owner_observation_cannot_be_projected_as_success(self):
        owner = self.controller(); plan = self.prepared(owner)
        self.configure(invalid_observation=True)
        job = self.job(owner, "start", plan_sha256=plan)
        self.assertEqual(job["status"], "unconfirmed")
        self.assertNotIn("result", job)
        self.assertTrue(owner.compose.execution_uncertain)

    def test_reopen_rejects_changed_profile_before_any_engine_observation(self):
        owner = self.controller(); plan = self.prepared(owner)
        self.host.update(mode="reopen", expected_plan_sha256=plan)
        self.host["allowed_actions"] = ["observe"]
        self.profile_path.write_text('{"volumes":{"data":{}},"changed":true}')
        self.host["backend_profile"]["sha256"] = backend.digest(self.profile_path.read_bytes())
        before = self.calls()
        with self.assertRaisesRegex(backend.SetupError, "compose_profile_changed"):
            self.controller()
        self.assertEqual(self.calls(), before)

    def test_symlink_input_refused_and_attempt_budget_bounded(self):
        owner = self.controller(); args = self.stage()
        path = self.staging / "selection/inputs.json"
        original = self.root / "outside.json"; path.rename(original); path.symlink_to(original)
        self.assertEqual(self.job(owner, "prepare", **args)["status"], "unconfirmed")
        self.assertEqual(self.calls(), [])
        owner.compose.intents.update({("%032x" % i): ({}, "none") for i in range(128)})
        with self.assertRaisesRegex(backend.SetupError, "compose_attempt_limit"):
            self.action(owner, "prepare", **args)

    def test_thread_start_failure_replays_one_unconfirmed_job(self):
        owner = self.controller(); args = {**self.stage(), "request_id": "8" * 32}
        with patch.object(backend.threading.Thread, "start", side_effect=RuntimeError("fixture")):
            failed = self.action(owner, "prepare", **args)["job"]
        self.assertEqual(failed["status"], "unconfirmed")
        self.assertEqual(self.action(owner, "prepare", **args)["job"], failed)
        self.assertFalse(owner.operation_lock.locked())
        self.assertEqual(self.calls(), [])

    def test_large_owner_plan_uses_opt_in_capture_limit(self):
        self.configure(padding=2 * 1024 * 1024 + 100)
        owner = self.controller(); self.prepared(owner)
        self.assertGreater(len((self.graph / "prepared.json").read_bytes()), backend.MAX_JSON)
        self.assertLess(len(json.dumps(owner.job["result"])), 10000)

    def test_prepared_projection_is_closed_and_retained_plan_stays_exact(self):
        owner = self.controller(); plan = self.prepared(owner)
        retained = (self.graph / "prepared.json").read_bytes()
        self.assertEqual(json.loads(retained)["plan_sha256"], owner.compose.plan["plan_sha256"])
        self.assertEqual(owner.compose.plan["status"], "prepared_summary")
        self.assertNotIn("plan", owner.compose.plan)
        public = owner.job["result"]["backend"]
        self.assertEqual(set(public), {"schema_version", "status", "plan_sha256", "summary"})
        self.assertEqual(public["plan_sha256"], plan)
        summary = public["summary"]
        self.assertEqual(set(summary), {"request_sha256", "package_sha256", "manifest_sha256",
            "manifest_source_sha256", "profile_sha256", "configuration_sha256", "policy_sha256",
            "host_owner", "project", "initialization_order", "shutdown_order", "deployment_order",
            "request_components", "components", "files"})
        self.assertEqual(set(summary["components"]["mcp"]), {"service", "lifetime", "image", "image_id",
            "platform", "dependencies", "deployment_dependencies", "readiness", "stop_timeout_ms"})
        for name, item in summary["files"].items():
            self.assertEqual(set(item), {"sha256", "mode"})
            self.assertEqual(item["sha256"], owner.compose.plan["summary"]["files"][name]["sha256"])
        for response in (owner.state(), owner.job_detail({"job_id": owner.job["id"]}),
                         self.action(owner, "prepare", request_id=owner.job["request_id"], **self.stage())):
            raw = json.dumps(response)
            for forbidden in ("PRIVATE_COMMAND", "PRIVATE_ENV", "PRIVATE_FUTURE_FIELD", "PRIVATE_COMPOSE_JSON",
                              "PRIVATE_ENGINE", "exact configuration", "exact file"):
                self.assertNotIn(forbidden, raw)
        self.assertEqual((self.graph / "prepared.json").read_bytes(), retained)

    def test_runtime_diagnostic_exact_bytes_only_and_replay_keeps_uncertainty(self):
        original = self.controller(); plan = self.prepared(original)
        self.host.update(mode="reopen", expected_plan_sha256=plan)
        self.host["allowed_actions"].remove("prepare")
        cases = [
            ({"exit_code": 1, "stderr": "engine_observation_unavailable\n"}, "engine_observation_unavailable"),
            ({"exit_code": 1, "stderr": "stale_observation\n"}, "stale_observation"),
            ({"exit_code": 1, "stderr": "PRIVATE_PATH /private/example\n"}, None),
            ({"exit_code": 1, "stderr": "engine_changed"}, None),
            ({"exit_code": 1, "stderr": " engine_changed\n"}, None),
            ({"exit_code": 1, "stderr": "engine_changed\r\n"}, None),
            ({"exit_code": 1, "stderr": "engine_changed\nPRIVATE_TRAILER"}, None),
            ({"exit_code": 1, "stderr": "engine_changed\n", "stdout": "{}"}, None),
            ({"exit_code": 2, "stderr": "engine_changed\n"}, None),
            ({"exit_code": 0, "stderr": "engine_changed\n"}, None),
            ({"exit_code": 1, "stderr": "engine_changed\n", "signal": True}, None),
            ({"exit_code": 1, "stderr": "engine_changed\n" + "x" * 200}, None),
        ]
        for config, expected in cases:
            with self.subTest(config=config):
                self.configure(**config)
                owner = self.controller()
                job = self.job(owner, "start", request_id="d" * 32, plan_sha256=plan)
                self.assertEqual(job["status"], "unconfirmed")
                self.assertEqual(job.get("runtime_error"), expected)
                self.assertTrue(owner.compose.execution_uncertain)
                self.assertNotIn("PRIVATE", json.dumps(job))
                count = len(self.calls())
                self.assertEqual(self.action(owner, "start", request_id="d" * 32, plan_sha256=plan)["job"], job)
                with self.assertRaisesRegex(backend.SetupError, "compose_host_reopen_required"):
                    self.action(owner, "observe")
                self.assertEqual(len(self.calls()), count)

    def test_changed_runtime_pin_suppresses_diagnostic_attribution(self):
        owner = self.controller(); plan = self.prepared(owner)
        self.configure(exit_code=1, stderr="engine_changed\n")
        original = owner.runner.run_compose
        def changed(*args, **kwargs):
            result = original(*args, **kwargs)
            (owner.workspace / "compose/runtime.py").write_text("changed")
            return result
        with patch.object(owner.runner, "run_compose", side_effect=changed):
            job = self.job(owner, "start", plan_sha256=plan)
        self.assertEqual(job["error"], "compose_host_changed")
        self.assertNotIn("runtime_error", job)
        self.assertTrue(owner.compose.execution_uncertain)


class RunnerTests(unittest.TestCase):
    def test_diagnostic_wrapper_preserves_override_and_trailing_stderr(self):
        with tempfile.TemporaryDirectory() as temp:
            runner = backend.Runner(Path(temp))
            code, output, diagnostic = runner.run_compose_diagnostic([sys.executable, "-I", "-c",
                "import os,time;os.close(1);time.sleep(.05);os.write(2,b'engine_changed\\n');raise SystemExit(1)"], timeout=5)
            self.assertEqual((code, output, diagnostic), (1, b"", b"engine_changed\n"))
            class Override(backend.Runner):
                count = 0
                def run(self, argv, *, capture=False, inspection_timeout=None):
                    self.count += 1
                    self._diagnostic_context.stderr.extend(b"engine_changed\n")
                    return 1, b""
            custom = Override(Path(temp))
            self.assertEqual(custom.run_compose_diagnostic(["not-executed"], timeout=5),
                             (1, b"", b"engine_changed\n"))
            self.assertEqual(custom.count, 1)
            self.assertFalse(hasattr(custom._diagnostic_context, "stderr"))
            with patch.object(custom, "run", side_effect=ValueError("fixture")):
                with self.assertRaises(ValueError):
                    custom.run_compose_diagnostic(["not-executed"], timeout=5)
            self.assertFalse(hasattr(custom._diagnostic_context, "stderr"))
            custom._diagnostic_context.stderr = bytearray(b"outer")
            with self.assertRaisesRegex(backend.SetupError, "compose_runner_unavailable"):
                custom.run_compose_diagnostic(["not-executed"], timeout=5)
            self.assertEqual(custom._diagnostic_context.stderr, b"outer")

    def test_regular_file_stdin_bounds_and_legacy_override(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp).resolve(); source = root / "input"; source.write_bytes(b"exact")
            runner = backend.Runner(root)
            code, output = runner.run_compose([sys.executable, "-I", "-c", "import sys;sys.stdout.buffer.write(sys.stdin.buffer.read())"],
                input_file=source, input_sha256=backend.digest(b"exact"), timeout=5)
            self.assertEqual((code, output), (0, b"exact"))
            with self.assertRaisesRegex(backend.SetupError, "compose_inputs_changed"):
                runner.run_compose([sys.executable], input_file=source, input_sha256="0" * 64, timeout=5)
            with self.assertRaises(backend.SetupError):
                runner.run_compose([sys.executable, "-I", "-c", "print('x'*100)"], output_limit=10, timeout=5)
            class Override(backend.Runner):
                def run(self, argv, *, capture=False, inspection_timeout=None):
                    self.called = (argv, capture, inspection_timeout); return 0, b"override"
            custom = Override(root)
            self.assertEqual(custom.run_compose(["inert"], timeout=5), (0, b"override"))
            self.assertEqual(custom.called, (["inert"], True, 5))


if __name__ == "__main__": unittest.main()
