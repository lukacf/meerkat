import asyncio
from copy import deepcopy
import json
from pathlib import Path

import pytest
from jsonschema import Draft202012Validator
from meerkat_mobkit.models import SessionBuildOptions

from launchkit.acceptance import SemanticAcceptance, _execution_result, tracked_requirements
from launchkit.acceptance_reviewer import ACCEPTANCE_PROMPT, AcceptanceTools
import launchkit.acceptance as acceptance_module
from launchkit.blueprint import starter_blueprint
from launchkit.implementation_workspace import ImplementationWorkspace, WorkspaceError, _json, _sha
from launchkit.templates.storage_setup import write_private


@pytest.fixture
def environment(tmp_path):
    blueprint = starter_blueprint().model_dump(mode="json")
    frozen = {"blueprint": blueprint, "brief": {"requirements": [
        {"id": "exclusive-operation", "outcome": "Only one approved session can control the same shared resource.", "disposition": "open"},
        {"id": "responsive-revocation", "outcome": "Revocation remains responsive during pending external I/O.", "disposition": "open"}]},
        "messages": [{"role": "user", "text": "Keep approval meaningful and prevent overlapping resource operation."}]}
    workspace = ImplementationWorkspace(tmp_path / "workspace", blueprint)
    workspace.seed()
    workspace.write_file("application/__init__.py", "")
    review = SemanticAcceptance(tmp_path / "acceptance", workspace, frozen)
    review.begin_round()
    review.bind_run("native-admitted-review-1")
    return workspace, review, frozen


def require_sandbox():
    if not acceptance_module._sandbox_executable():
        pytest.skip("Protected acceptance execution has no unsandboxed fallback.")


def test_protected_dom_execution_keeps_existing_signed_publication_and_native_seal(environment):
    require_sandbox()
    workspace, review, _ = environment
    review.write_test("test_dom.dom.cjs", "module.exports=({test,createDOM,assert})=>test('actual script event',()=>{const dom=createDOM('<button>change</button><output>old</output><script>document.querySelector(\"button\").onclick=()=>document.querySelector(\"output\").textContent=\"new\";</script>');dom.window.document.querySelector('button').click();assert.equal(dom.window.document.querySelector('output').textContent,'new');});")
    checked = review.check()
    assert checked["status"] == "pass" and checked["tests_run"] == 1, checked
    assert checked["test_results"][0]["id"] == "test_dom.dom.cjs::actual script event"
    assert "not browser form encoding" in checked["detail"]
    provisional = review.publish(report(review,checked))
    assert not provisional["ready_to_package"] and review.verification() is None
    sealed = complete(review)
    assert sealed["ready_to_package"] and sealed["source_digest"] == workspace.digest()
    assert sealed == review.verification()
    assert {item.path for item in review.export_files()} == {"acceptance-report.json","acceptance-tests/test_dom.dom.cjs"}
    workspace.write_file("application/change.py", "CHANGED = True\n")
    assert review.verification() is None


def test_protected_dom_and_python_mixed_suite_failure_or_test_edit_cannot_reuse_execution(environment):
    require_sandbox()
    _, review, _ = environment
    simple_tests(review)
    review.write_test("test_dom.dom.cjs", "module.exports=({test,assert})=>test('failure',()=>assert.equal(1,2));")
    failed = review.check()
    assert failed["status"] == "fail" and failed["tests_run"] == 2, failed
    assert review.publish(report(review,failed))["ready_to_package"] is False
    assert complete(review)["ready_to_package"] is False
    # A new protected round requires fresh execution and publication, not the
    # preceding mixed-suite results or a manually declared passing test count.
    review.begin_round(); review.bind_run("native-admitted-review-2")
    review.write_test("test_dom.dom.cjs", "module.exports=({test})=>test('pass',()=>{});")
    checked = review.check()
    assert checked["status"] == "pass", checked
    review.write_test("test_dom.dom.cjs", "module.exports=({test,assert})=>test('changed',()=>assert.fail('new failure'));")
    with pytest.raises(WorkspaceError,match="host execution|Run"):
        review.publish(report(review,checked))


@pytest.mark.parametrize("content", ["module.exports=()=>{};", "module.exports=({test})=>test('pending',()=>{}, {skip:true});", "process.exit(0);"])
def test_protected_dom_no_executed_tests_cannot_gain_acceptance(environment, content):
    require_sandbox()
    _, review, _ = environment
    review.write_test("test_dom.dom.cjs",content)
    checked=review.check()
    assert checked["status"] == "fail", checked
    assert not review.publish(report(review,checked,status="unresolved"))["ready_to_package"]
    assert not complete(review)["ready_to_package"]


def simple_tests(review):
    review.write_test("test_contract.py", '''import unittest
class Contract(unittest.TestCase):
    def test_request_remains_exact(self):
        self.assertNotEqual("approved", "revoked")
''')


def host_execution(review, *, status="pass", tests=1):
    # Receipt binding tests substitute host execution only. Real protected
    # behavioral execution is covered by the generated-source regression below.
    with review._locked():
        state = review._state()
        row = review._round(state)
        value = {"id": "host-execution", "source_digest": row["source_digest"],
                 "test_digest": _sha(_json(review._tests(row))), "status": status,
                 "tests_run": tests, "test_results": [
                     {"id": "test_contract.py::Contract.test_request_remains_exact", "file": "test_contract.py", "status": status}]
                     if tests else [], "detail": "Host fixture only"}
        value["signature"] = review._signed(value)
        row["execution"] = value
        review._save(state)
    return value


def report(review, execution, *, counterexamples=(), status="tested"):
    ids = [row["id"] for row in execution["test_results"] if row["status"] == "pass"]
    return {"summary": "Independent offline assessment; no live transport proof.",
            "requirements": [{"id": item["id"], "status": status,
                 "source_paths": ["application/__init__.py"], "test_ids": ids if status == "tested" else [],
                 "evidence": "Observed the specifically stated offline behavior; live adapters remain unverified.",
                 "remaining_dependencies": ["Configure and verify actual live adapters on the destination."]}
                for item in review.requirements],
            "counterexamples": list(counterexamples), "remaining_dependencies": ["Live adapter verification."]}


def complete(review):
    identity = review.history()[-1]["reviewer_interaction_id"]
    return review.complete_run(identity, {"kind": "interaction_complete", "interaction_id": identity, "cursor": "native:42"})


def publication_tool(review, directory):
    directory.mkdir(mode=0o700)
    write_private(directory / "receipt.json", {"status": "running", "progress": []})
    tools = AcceptanceTools(directory, review, lambda args: None)
    draft = SessionBuildOptions()
    tools.register(draft)
    wire = next(row for row in draft.to_dict()["tools"] if row["name"] == "publish_acceptance")
    return tools, draft.tool_handlers["publish_acceptance"], wire["input_schema"]


def test_registered_publication_schema_describes_actual_nested_evidence(environment, tmp_path):
    _, review, _ = environment
    _, _, schema = publication_tool(review, tmp_path / "tool")
    Draft202012Validator.check_schema(schema)
    validator = Draft202012Validator(schema)
    nested = schema["properties"]["requirements"]["items"]
    assert nested["required"] == ["id", "status", "evidence"]
    assert nested["additionalProperties"] is False
    assert "explanation" not in nested["properties"]
    assert "not an explanation field" in ACCEPTANCE_PROMPT
    for status in ("proposal", "source", "tested", "unresolved"):
        item = {"id": review.requirements[0]["id"], "status": status,
                "evidence": "Exact offline observation; live transport remains unverified.",
                "source_paths": None, "test_ids": None, "remaining_dependencies": None}
        value = {"summary": "Offline review", "requirements": [item],
                 "counterexamples": [], "remaining_dependencies": []}
        assert not list(validator.iter_errors(value))
        for field in ("id", "status", "evidence"):
            changed = deepcopy(value)
            changed["requirements"][0].pop(field)
            assert list(validator.iter_errors(changed))
        changed = deepcopy(value)
        changed["requirements"][0]["explanation"] = changed["requirements"][0].pop("evidence")
        assert list(validator.iter_errors(changed))
    value["requirements"][0]["status"] = "connected"
    assert list(validator.iter_errors(value))


@pytest.mark.asyncio
async def test_registered_publication_callback_keeps_host_evidence_authority(environment, tmp_path):
    workspace, review, _ = environment
    simple_tests(review)
    # This fixture exercises the real signed publication boundary only;
    # actual protected execution is covered by the sandbox regressions below.
    execution = host_execution(review)
    tools, publish, schema = publication_tool(review, tmp_path / "tool")
    validator = Draft202012Validator(schema)
    valid = report(review, execution)
    for item in valid["requirements"]:
        item["source_paths"] = None
        item["remaining_dependencies"] = None
    original = deepcopy(valid)
    for field in ("missing", "explanation", "null"):
        value = deepcopy(valid)
        for item in value["requirements"]:
            evidence = item.pop("evidence")
            if field == "explanation":
                item["explanation"] = evidence
            elif field == "null":
                item["evidence"] = None
        assert list(validator.iter_errors(value))
        result = await publish(value)
        assert result["ok"] is False and "actual evidence" in result["error"]
        assert tools.publication is None and review.history()[-1].get("publication") is None
    for status, empty_field, error in (("tested", "test_ids", "passing host-executed test IDs"),
                                      ("source", "source_paths", "actual source path"),
                                      ("unresolved", "remaining_dependencies", "remaining dependencies")):
        value = deepcopy(valid)
        for item in value["requirements"]:
            item["status"] = status
            item[empty_field] = None
        assert not list(validator.iter_errors(value))
        result = await publish(value)
        assert result["ok"] is False and error in result["error"]
        assert review.verification() is None
    assert not list(validator.iter_errors(valid))
    result = await publish(valid)
    assert result["ok"] and result["pending_native_completion"]
    assert valid == original
    provisional = result["semantic_acceptance"]
    assert provisional["source_digest"] == workspace.digest()
    assert not provisional["ready_to_package"] and review.verification() is None
    for item in provisional["requirements"]:
        assert item["source_paths"] == [] and item["remaining_dependencies"] == []
        assert item["test_ids"] == [execution["test_results"][0]["id"]]
        assert item["evidence"] == valid["requirements"][0]["evidence"]
    sealed = complete(review)
    assert sealed["ready_to_package"] and review.verification() == sealed


def test_source_test_brief_and_signature_bound_evidence(environment):
    workspace, review, frozen = environment
    simple_tests(review)
    execution = host_execution(review)
    provisional = review.publish(report(review, execution))
    assert not provisional["ready_to_package"] and review.verification() is None
    receipt = complete(review)
    assert receipt["status"] == "pass" and receipt["ready_to_package"]
    assert receipt["source_digest"] == workspace.digest()
    assert receipt["reviewer_identity"] == "acceptance:main"
    assert receipt["reviewer_interaction_id"] == "native-admitted-review-1"
    assert {row["id"] for row in receipt["requirements"]} == {"exclusive-operation", "responsive-revocation"}
    assert "Not live-ready" in receipt["scope"]
    assert SemanticAcceptance(review.root, workspace, frozen).verification() == receipt
    assert {item.path for item in review.export_files()} == {"acceptance-report.json", "acceptance-tests/test_contract.py"}
    with pytest.raises(WorkspaceError, match="sealed"):
        review.write_test("test_contract.py", "pass\n")
    with review._locked():
        state = review._state()
        state["rounds"][-1]["publication"]["tests_run"] = 900
        review._save(state)
    assert review.verification() is None


def test_source_only_requested_outcomes_cannot_package_after_native_completion(environment):
    _, review, _ = environment
    simple_tests(review)
    assessment = report(review, host_execution(review), status="source")
    provisional = review.publish(assessment)
    assert provisional["status"] == "incomplete" and not provisional["ready_to_package"]
    sealed = complete(review)
    assert sealed["status"] == "incomplete" and not sealed["ready_to_package"]
    assert sealed["native_completion"]["kind"] == "interaction_complete"
    assert review.verification() == sealed
    for item in sealed["requirements"]:
        assert item["status"] == "source" and item["source_paths"] == ["application/__init__.py"]
        assert item["remaining_dependencies"] == assessment["requirements"][0]["remaining_dependencies"]
    with pytest.raises(WorkspaceError, match="Packaging requires"):
        review.export_files()


@pytest.mark.parametrize("disposition", ["deferred", "cancelled"])
def test_inactive_tested_outcome_cannot_cover_active_source_only_scope(environment, tmp_path, disposition):
    workspace, _, frozen = environment
    frozen = deepcopy(frozen)
    frozen["brief"]["requirements"][0]["disposition"] = disposition
    review = SemanticAcceptance(tmp_path / "scoped-acceptance", workspace, frozen)
    review.begin_round(); review.bind_run("native-scoped-review")
    simple_tests(review)
    execution = host_execution(review)
    assessment = report(review, execution, status="source")
    assessment["requirements"][0].update(status="tested", test_ids=[execution["test_results"][0]["id"]])
    assert review.publish(assessment)["status"] == "incomplete"
    sealed = complete(review)
    assert sealed["requirements"][0]["disposition"] == disposition
    assert sealed["requirements"][1]["disposition"] == "open"
    assert not sealed["ready_to_package"] and review.verification() == sealed
    with pytest.raises(WorkspaceError, match="Packaging requires"):
        review.export_files()


def test_active_tested_outcome_with_source_live_dependencies_preserves_policy_one_receipt(environment):
    workspace, review, frozen = environment
    simple_tests(review)
    execution = host_execution(review)
    assessment = report(review, execution, status="source")
    assessment["requirements"][0].update(status="tested", test_ids=[execution["test_results"][0]["id"]])
    assert review.publish(assessment)["status"] == "pass"
    sealed = complete(review)
    assert sealed["acceptance_policy_version"] == 1 and sealed["ready_to_package"]
    assert sealed["requirements"][1]["status"] == "source"
    assert sealed["requirements"][1]["remaining_dependencies"]
    before = (review.root / "acceptance-state.json").read_bytes()
    assert SemanticAcceptance(review.root, workspace, frozen).verification() == sealed
    assert {item.path for item in review.export_files()} == {"acceptance-report.json", "acceptance-tests/test_contract.py"}
    assert (review.root / "acceptance-state.json").read_bytes() == before


@pytest.mark.parametrize("execution_status,tests,counterexamples,expected", [
    ("pass", 1, (), "pass"),
    ("pass", 1, ("The checked offline operation violates its declared constraint.",), "needs_repair"),
    ("fail", 1, (), "needs_repair"),
    ("pass", 0, (), "incomplete"),
])
def test_all_inactive_scope_keeps_existing_execution_and_failure_semantics(environment, tmp_path,
        execution_status, tests, counterexamples, expected):
    workspace, _, frozen = environment
    frozen = deepcopy(frozen)
    for item, disposition in zip(frozen["brief"]["requirements"], ("deferred", "cancelled")):
        item["disposition"] = disposition
    review = SemanticAcceptance(tmp_path / "inactive-acceptance", workspace, frozen)
    review.begin_round(); review.bind_run("native-inactive-review")
    simple_tests(review)
    execution = host_execution(review, status=execution_status, tests=tests)
    assert review.publish(report(review, execution, status="unresolved", counterexamples=counterexamples))["status"] == expected
    sealed = complete(review)
    assert sealed["ready_to_package"] is (expected == "pass")
    assert review.verification() == sealed
    if expected == "pass":
        assert review.export_files()
    else:
        with pytest.raises(WorkspaceError, match="Packaging requires"):
            review.export_files()


@pytest.mark.parametrize("execution_status,counterexamples", [
    ("fail", ()), ("pass", ("A checked requested operation still fails.",)),
])
def test_active_source_only_coverage_does_not_hide_failures(environment, execution_status, counterexamples):
    _, review, _ = environment
    simple_tests(review)
    execution = host_execution(review, status=execution_status)
    assert review.publish(report(review, execution, status="source", counterexamples=counterexamples))["status"] == "needs_repair"
    assert not complete(review)["ready_to_package"]


@pytest.mark.parametrize("inactive_disposition", [None, "deferred", "cancelled"])
def test_prior_signed_zero_active_coverage_pass_cannot_supply_cached_readiness(environment, tmp_path, inactive_disposition):
    workspace, _, frozen = environment
    frozen = deepcopy(frozen)
    if inactive_disposition:
        frozen["brief"]["requirements"][0]["disposition"] = inactive_disposition
    review = SemanticAcceptance(tmp_path / "prior-acceptance", workspace, frozen)
    review.begin_round(); review.bind_run("native-prior-review")
    simple_tests(review)
    execution = host_execution(review)
    assessment = report(review, execution, status="source")
    if inactive_disposition:
        assessment["requirements"][0].update(status="tested", test_ids=[execution["test_results"][0]["id"]])
    assert review.publish(assessment)["status"] == "incomplete"
    complete(review)
    # Model the earlier policy-1 host-signed pass in this receipt fixture only.
    # No actual retained build or native operation is changed or replayed.
    with review._locked():
        state = review._state()
        prior = state["rounds"][-1]["publication"]
        prior.update(status="pass", ready_to_package=True)
        prior["signature"] = review._signed(prior)
        review._save(state)
    before = (review.root / "acceptance-state.json").read_bytes()
    assert prior["acceptance_policy_version"] == 1
    assert SemanticAcceptance(review.root, workspace, frozen).verification() is None
    with pytest.raises(WorkspaceError, match="Packaging requires"):
        review.export_files()
    assert (review.root / "acceptance-state.json").read_bytes() == before


def test_prior_provisional_source_only_pass_cannot_become_ready_at_completion(environment):
    _, review, _ = environment
    simple_tests(review)
    review.publish(report(review, host_execution(review), status="source"))
    with review._locked():
        state = review._state()
        prior = state["rounds"][-1]["publication"]
        prior["status"] = "pass"
        prior["signature"] = review._signed(prior)
        review._save(state)
    sealed = complete(review)
    assert sealed["status"] == "pass" and not sealed["ready_to_package"]
    assert sealed["native_completion"]["kind"] == "interaction_complete"
    assert review.verification() is None
    with pytest.raises(WorkspaceError, match="Packaging requires"):
        review.export_files()


def test_changed_source_or_tests_cannot_reuse_acceptance(environment):
    workspace, review, frozen = environment
    simple_tests(review)
    execution = host_execution(review)
    review.publish(report(review, execution))
    complete(review)
    workspace.write_file("application/change.py", "NEW_BEHAVIOR = True\n")
    assert review.verification() is None
    with pytest.raises(WorkspaceError):
        review.export_files()
    review.begin_round()
    review.bind_run("native-admitted-review-2")
    assert review.read_test("test_contract.py").startswith("import unittest")
    assert review.history()[0]["publication"]["source_digest"] != workspace.digest()
    execution = host_execution(review)
    review.write_test("test_contract.py", "import unittest\nclass More(unittest.TestCase):\n    def test_value(self): self.assertEqual(1, 1)\n")
    with pytest.raises(WorkspaceError, match="Run host acceptance"):
        review.publish(report(review, execution))
    changed = deepcopy(frozen)
    changed["brief"]["requirements"][0]["outcome"] = "A different outcome"
    with pytest.raises(WorkspaceError, match="different frozen brief"):
        SemanticAcceptance(review.root, workspace, changed)


def test_cumulative_implementation_requests_are_reviewed_and_bound(environment, tmp_path):
    workspace, _, frozen = environment
    frozen = deepcopy(frozen)
    frozen.update(request="Improve duplicate rejection", implementation_requests=[
        "Keep the stored collection on restart", "Improve duplicate rejection"])
    review = SemanticAcceptance(tmp_path / "feedback-acceptance", workspace, frozen)
    review.begin_round()
    assert review.context()["implementation_requests"] == frozen["implementation_requests"]
    changed = deepcopy(frozen)
    changed["implementation_requests"][0] = "Discard the collection on restart"
    with pytest.raises(WorkspaceError, match="different frozen brief"):
        SemanticAcceptance(review.root, workspace, changed)


def test_a_single_latest_request_preserves_its_existing_binding(environment):
    workspace, review, frozen = environment
    same = deepcopy(frozen)
    same["implementation_requests"] = [same.get("request", "")]
    assert SemanticAcceptance(review.root, workspace, same).binding_digest == review.binding_digest


def test_builder_cannot_write_protected_tests_reports_or_host_storage(environment):
    workspace, review, _ = environment
    for path in ("../acceptance/round-1/tests/test_contract.py", "acceptance-report.json",
                 "Acceptance-Report.json", "acceptance-tests/test_contract.py", "Acceptance-Tests/x.txt"):
        with pytest.raises(WorkspaceError):
            workspace.write_file(path, "forged completion")
    with pytest.raises(WorkspaceError, match="outside"):
        SemanticAcceptance(workspace.project / "acceptance", workspace, review.frozen)
    simple_tests(review)
    assert review.read_test("test_contract.py").startswith("import unittest")
    with pytest.raises(WorkspaceError):
        review.write_test("../outside.py", "pass")
    with pytest.raises(WorkspaceError):
        review.write_test("application/source.py", "pass")


def test_testless_failed_unresolved_or_fake_ids_cannot_claim_acceptance(environment):
    _, review, _ = environment
    empty = review.check()
    assert empty["status"] == "fail" and empty["tests_run"] == 0
    value = report(review, empty, status="source")
    assert review.publish(value)["status"] == "needs_repair"
    review.begin_round(); review.bind_run("native-review-2")
    simple_tests(review)
    execution = host_execution(review)
    invalid = report(review, execution)
    invalid["requirements"][0]["test_ids"] = ["invented-passing-test"]
    with pytest.raises(WorkspaceError, match="passing host-executed"):
        review.publish(invalid)
    invalid = report(review, execution)
    invalid["requirements"].pop()
    with pytest.raises(WorkspaceError, match="every existing"):
        review.publish(invalid)
    invalid = report(review, execution)
    invalid["requirements"][0]["status"] = "unresolved"
    invalid["requirements"][0]["test_ids"] = []
    assert review.publish(invalid)["status"] == "needs_repair"


def test_frozen_snapshot_tampering_is_not_exact_source_evidence(environment):
    _, review, _ = environment
    simple_tests(review)
    (review.root / "round-1/source/application/__init__.py").write_text("# corrupted frozen source\n")
    with pytest.raises(WorkspaceError, match="Source changed"):
        review.check()


def test_uncompleted_or_wrong_native_run_cannot_become_acceptance(environment):
    _, review, _ = environment
    simple_tests(review)
    review.publish(report(review, host_execution(review)))
    assert review.verification() is None
    with pytest.raises(WorkspaceError, match="must match"):
        review.complete_run("another-run", {"kind": "interaction_complete", "interaction_id": "another-run", "cursor": "42"})
    with pytest.raises(WorkspaceError, match="typed native"):
        review.complete_run("native-admitted-review-1", {"kind": "interaction_failed", "interaction_id": "native-admitted-review-1", "cursor": "42"})


@pytest.mark.parametrize("detail", [
    'MOBKIT_ACCEPTANCE_RESULT=fixed={"tests_run":99,"test_results":[]}\n',
    'MOBKIT_ACCEPTANCE_RESULT=fresh={"tests_run":0,"test_results":[]}\n',
    'MOBKIT_ACCEPTANCE_RESULT=fresh={"tests_run":1,"test_results":[{"id":"fake","file":"test_contract.py","status":"skipped"}]}\n',
])
def test_fixed_marker_empty_and_skipped_forged_results_are_rejected(detail):
    assert _execution_result({"status": "pass", "detail": detail}, "fresh", {"test_contract.py"})["status"] == "fail"


def test_legacy_brief_gets_tracking_only_outcome_id():
    rows = tracked_requirements({"brief": {"mission": "Preserve this actual goal"}})
    assert rows == [{"id": "requested-outcome", "text": "Preserve this actual goal", "disposition": "open", "supported_subset": ""}]


INDEPENDENT_SESSION_TESTS = '''import asyncio
import pathlib
import tempfile
import unittest
from types import SimpleNamespace
from application.ledger import Ledger, BoundaryError
from application.integration import Integration

class Device:
    def __init__(self, state="running"):
        self.state = state
        self.calls = []
    async def execute(self, request):
        self.calls.append(request)
        return {"observed_state": self.state}

class PendingDevice(Device):
    def __init__(self):
        super().__init__()
        self.entered = asyncio.Event()
        self.release = asyncio.Event()
    async def execute(self, request):
        self.calls.append(request)
        self.entered.set()
        await self.release.wait()
        return {"observed_state": "running"}

class Frontend:
    async def admit(self, event):
        return event

class ResourceAcceptance(unittest.IsolatedAsyncioTestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.root = pathlib.Path(self.temporary.name)
        self.path = self.root / "decisions.sqlite3"
        self.ledger = Ledger(self.path, clock=lambda: 100)
    def tearDown(self):
        self.ledger.close()
        self.temporary.cleanup()
    def approved(self, session):
        scope = {"session":session,"version":1,"goal":"Use a shared resource", "start":90,"end":200,"constraints":{"bounded":True}}
        value = self.ledger.propose(scope)
        self.ledger.decide(session, value["digest"], "approved", "human", "console")
        return {"session":session,"digest":value["digest"],"key":session + "-start","kind":"start","scope":scope,"target":"within-scope","until":150}
    async def verify_restart_ownership(self, state):
        first = self.approved("first")
        second = self.approved("second")
        await self.ledger.act(first, Device(state))
        self.ledger.close()
        self.ledger = Ledger(self.path, clock=lambda:100)
        device = Device()
        with self.assertRaises(BoundaryError, msg="A second approved session must not take ownership of the same active resource"):
            await self.ledger.act(second, device)
        self.assertEqual(device.calls, [])
    async def test_running_owner_survives_restart(self):
        await self.verify_restart_ownership("running")
    async def test_paused_owner_survives_restart(self):
        await self.verify_restart_ownership("paused")
    async def test_pending_owner_blocks_other_connection(self):
        first = self.approved("first")
        second = self.approved("second")
        pending = PendingDevice()
        task = asyncio.create_task(self.ledger.act(first, pending))
        await asyncio.wait_for(pending.entered.wait(), 30)
        other = Ledger(self.path, clock=lambda:100)
        try:
            device = Device()
            with self.assertRaises(BoundaryError, msg="Reserve the resource before pending external I/O"):
                await other.act(second, device)
            self.assertEqual(device.calls, [])
        finally:
            pending.release.set()
            await task
            other.close()
    async def test_revocation_is_admitted_during_pending_io(self):
        request = self.approved("first")
        integration = Integration(SimpleNamespace(root=self.root, runtime=None))
        integration.ledger = self.ledger
        integration.frontend = Frontend()
        integration.device = PendingDevice()
        action = asyncio.create_task(integration.action(request))
        await asyncio.wait_for(integration.device.entered.wait(), 30)
        decision = {"kind":"decision","session":"first","digest":request["digest"],"decision":"revoked","actor":"human","channel":"console"}
        try:
            value = await asyncio.wait_for(integration.admitted_event(decision), 0.2)
            self.assertEqual(value["decision"], "revoked")
        finally:
            integration.device.release.set()
            await action
        self.assertEqual(self.ledger.status("first")["decision"], "revoked", "A late device receipt must not restore approval")
        next_request = {**request,"key":"later", "kind":"continue"}
        with self.assertRaises(BoundaryError):
            await integration.action(next_request)
'''


@pytest.mark.parametrize("variant,expected", [("original", "fail"), ("repaired", "pass")])
def test_protected_tests_catch_actual_generated_overlap_and_blocked_revocation(tmp_path, variant, expected):
    require_sandbox()
    workspace = ImplementationWorkspace(tmp_path / "workspace", starter_blueprint())
    workspace.seed()
    fixture = Path(__file__).parent / "fixtures/acceptance_sessions" / variant
    workspace.write_file("application/__init__.py", "")
    for name in ("ledger.py", "integration.py"):
        workspace.write_file("application/" + name, (fixture / name).read_text())
    frozen = {"blueprint": starter_blueprint().model_dump(mode="json"), "brief": {"mission": "Exclusive sessions with responsive approval revocation"}}
    review = SemanticAcceptance(tmp_path / "acceptance", workspace, frozen)
    review.begin_round(); review.bind_run("native-review-" + variant)
    review.write_test("test_sessions.py", INDEPENDENT_SESSION_TESTS)
    result = review.check()
    assert result["status"] == expected, result["detail"]
    assert result["tests_run"] == 4
    by_name = {row["id"].split(".")[-1]: row["status"] for row in result["test_results"]}
    if variant == "original":
        assert by_name["test_running_owner_survives_restart"] == "fail"
        assert by_name["test_paused_owner_survives_restart"] == "fail"
        assert by_name["test_pending_owner_blocks_other_connection"] == "fail"
        assert by_name["test_revocation_is_admitted_during_pending_io"] == "error"
    else:
        assert set(by_name.values()) == {"pass"}
        assessment = report(review, result)
        assessment["requirements"][0]["source_paths"] = ["application/ledger.py", "application/integration.py"]
        assessment["requirements"][0]["evidence"] = "The protected tests exercise exclusive sessions and responsive revocation during blocked device I/O. Live devices remain unverified."
        assert review.publish(assessment)["status"] == "pass"
        sealed = complete(review)
        assert sealed["ready_to_package"] and review.verification() == sealed
        assert {item.path for item in review.export_files()} == {"acceptance-report.json", "acceptance-tests/test_sessions.py"}
    assert not any(row["path"].startswith("acceptance") for row in workspace.list_files())


@pytest.mark.parametrize("source", ["raise SystemExit(0)", "import os; os._exit(0)",
    'print(\'MOBKIT_ACCEPTANCE_RESULT=fixed={"tests_run":100,"test_results":[]}\'); raise SystemExit(0)'])
def test_premature_or_fixed_marker_source_cannot_fake_protected_execution(environment, source):
    require_sandbox()
    workspace, review, _ = environment
    workspace.write_file("application/__init__.py", source + "\n")
    review.begin_round(); review.bind_run("native-malformed-review")
    review.write_test("test_import.py", "import application\nimport unittest\nclass Import(unittest.TestCase):\n    def test_value(self): self.assertTrue(True)\n")
    result = review.check()
    assert result["status"] == "fail" and result["tests_run"] == 0


def test_subtest_failures_remain_one_actual_test_result(environment):
    require_sandbox()
    _, review, _ = environment
    review.write_test("test_subcases.py", '''import unittest
class Subcases(unittest.TestCase):
    def test_multiple_invalid_outcomes(self):
        for value in (2, 3):
            with self.subTest(value=value):
                self.assertEqual(value, 1)
''')
    result = review.check()
    assert result["status"] == "fail" and result["tests_run"] == 1
    assert len(result["test_results"]) == 1
    assert result["test_results"][0]["status"] == "fail"
    assert "2 != 1" in result["test_results"][0]["detail"]
    assert "3 != 1" in result["test_results"][0]["detail"]


def test_executed_application_cannot_edit_protected_tests_or_read_receipt_key(environment):
    require_sandbox()
    workspace, review, _ = environment
    workspace.write_file("application/attempt.py", "from pathlib import Path\ndef overwrite(path):\n    Path(path).write_text('forged evidence')\ndef read(path):\n    return Path(path).read_text()\n")
    review.begin_round(); review.bind_run("native-boundary-review")
    review.write_test("test_storage_boundary.py", '''import unittest
from application.attempt import overwrite, read
class Isolation(unittest.TestCase):
    def test_application_cannot_overwrite_protected_test(self):
        with self.assertRaises(OSError):
            overwrite(__file__)
    def test_application_cannot_read_host_signing_key(self):
        with self.assertRaises(OSError):
            read(''' + repr(str(review.key_path)) + ''')
''')
    before = review.read_test("test_storage_boundary.py")
    result = review.check()
    assert result["status"] == "pass", result["detail"]
    assert result["tests_run"] == 2
    assert review.read_test("test_storage_boundary.py") == before
