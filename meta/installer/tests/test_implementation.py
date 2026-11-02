import asyncio
from copy import deepcopy
import hashlib
import io
import json
import os
from pathlib import Path
import stat
import threading
import time
import uuid
import zipfile

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.implementation import ImplementationService, read_receipt, update_receipt
from launchkit.implementation_worker import BuildTools
from launchkit.implementation_worker import interaction_completed
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.generator import PackageFile
from launchkit.projects import DraftConflict, fingerprint

from launchkit.implementation_workspace import _sandbox_executable as actual_sandbox_executable


class Guide:
    def __init__(self):
        self.identity = str(uuid.uuid4())
        self.state = {"id": self.identity, "version": 3,
                      "proposal": starter_blueprint().model_dump(mode="json"),
                      "brief": {"mission": "Compare a requested schedule with available slots"},
                      "messages": [{"role": "user", "text": "Preserve the required approval."}]}

    async def get(self, identity):
        if identity != self.identity:
            raise LookupError("Unknown setup")
        return deepcopy(self.state)


class Service(ImplementationService):
    async def _run(self, directory):
        await asyncio.Event().wait()

    def _task_finished(self, directory, task):
        # This caller fixture never spawns a process. Even tests that remove
        # its task map before cancelling must record its positive fake stop.
        if task.cancelled():
            update_receipt(directory, cleanup_pending=False, cleanup_in_progress=False,
                           worker_stop_confirmed=True)
        super()._task_finished(directory, task)


@pytest.fixture
def service(tmp_path, monkeypatch):
    # Service._run is an inert caller fixture; no generated code executes here.
    sandbox = actual_sandbox_executable()
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: sandbox or "/fictional/sandbox")
    monkeypatch.setattr("launchkit.planner.planner_capability", lambda: {"available": True, "provider": "openai"})
    return Service(tmp_path, Guide())


@pytest.mark.parametrize("mask", [0o002, 0o022])
async def test_build_reservation_keeps_guide_history_private(service, monkeypatch, mask):
    mkdir = Path.mkdir
    parents = (service.root, service.root / service.guide.identity)
    def create(path, *args, **kwargs):
        if path not in parents:
            return mkdir(path, *args, **kwargs)
        previous = os.umask(mask)
        try:
            return mkdir(path, *args, **kwargs)
        finally:
            os.umask(previous)
    # Apply the host mask only during synchronous history-parent creation;
    # restore it before any await, worker or source preparation can run.
    monkeypatch.setattr(Path, "mkdir", create)
    try:
        admitted = await service.submit(service.guide.identity, 3)
        directory = service._directory(service.guide.identity, admitted["build_id"])
        assert all(stat.S_IMODE(path.stat().st_mode) == 0o700
            for path in (service.root, directory.parent, directory))
    finally:
        await service.shutdown()


@pytest.mark.parametrize("mode", [0o750, 0o770])
async def test_build_reservation_preserves_existing_history_modes_and_recovery_guards(service, mode):
    from launchkit.connect_installer_caller import CustodyError
    from launchkit.implementation_selection import candidates

    parent = service.root / service.guide.identity
    parent.mkdir(parents=True)
    service.root.chmod(mode)
    parent.chmod(mode)
    try:
        admitted = await service.submit(service.guide.identity, 3)
        directory = service._directory(service.guide.identity, admitted["build_id"])
        assert stat.S_IMODE(service.root.stat().st_mode) == mode
        assert stat.S_IMODE(parent.stat().st_mode) == mode
        assert stat.S_IMODE(directory.stat().st_mode) == 0o700
        if mode == 0o770:
            with pytest.raises(CustodyError):
                candidates(service.root, service.guide.identity)
    finally:
        await service.shutdown()


def decision_brief():
    from launchkit.guide_models import REVIEW_TOPICS
    return {"facts": {topic: {"summary": f"The user supplied {topic} context.",
        "status": "confirmed", "source_message_ids": ["request"]} for topic in REVIEW_TOPICS},
        "requirements": []}


@pytest.mark.parametrize("prerequisite", ["sandbox", "planner", "checker"])
async def test_prerequisite_refusal_is_owner_typed_before_build_publication(service, monkeypatch, prerequisite):
    from launchkit.architect import ArchitectUnavailable
    from launchkit.implementation import ImplementationPrerequisiteNotAdmitted

    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable",
        lambda: None if prerequisite == "sandbox" else "/fictional/sandbox")
    monkeypatch.setattr("launchkit.planner.planner_capability", lambda:
        {"available": False, "reason": "Prepare the synthetic provider."} if prerequisite == "planner"
        else {"available": True, "provider": "openai"})
    service.guide.state["brief"] = decision_brief()

    class Checker:
        async def status(self):
            return {"status": "preparation-required"}

    if prerequisite == "checker":
        service.checker_setup = Checker()
    with pytest.raises(ArchitectUnavailable) as caught:
        await service.submit(service.guide.identity, 3, "Build the proposed application")
    assert isinstance(caught.value, ImplementationPrerequisiteNotAdmitted)
    assert not service.root.exists()
    assert not service.tasks and not service.processes


@pytest.mark.parametrize("unresolved", ["authority", "outcome", "feature", "first_version"])
async def test_canonical_decisions_refuse_before_new_build_work(service, monkeypatch, unresolved):
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: "/fictional/sandbox")
    brief = decision_brief()
    if unresolved == "authority":
        brief["facts"]["authority"]["status"] = "unresolved"
        expected = "Resolve authority"
    elif unresolved == "outcome":
        brief["facts"]["outcome"]["status"] = "proposed"
        expected = "Understand a concrete first job"
    else:
        brief["requirements"] = [{"id": "staff_interface", "outcome": "Staff use a separate interface.",
            "disposition": "open" if unresolved == "feature" else "deferred",
            "source_message_ids": ["request"],
            "decision_message_ids": [] if unresolved == "feature" else ["later-decision"]}]
        expected = "Agree how to handle" if unresolved == "feature" else "Confirm the useful first version"
    service.guide.state.update(brief=brief, readiness={"ready": True, "decision_blockers": []})

    class UnexpectedChecker:
        async def status(self):
            pytest.fail("A decision-blocked build must not prepare checker admission.")

    service.checker_setup = UnexpectedChecker()
    with pytest.raises(DraftConflict, match=expected):
        await service.submit(service.guide.identity, 3, "Build the proposed application")
    assert not service.root.exists()
    assert not service.tasks and not service.processes


async def test_proposed_defaults_and_implementation_work_admit_without_claiming_completion(service, monkeypatch):
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: "/fictional/sandbox")
    brief = decision_brief()
    brief["facts"]["destination"].update(status="proposed", source_message_ids=[])
    brief["requirements"] = [
        {"id": "staff_interface", "outcome": "Staff use a separate interface.",
         "extension": "Implement the staff interface.", "disposition": "open", "source_message_ids": ["request"]},
        {"id": "calendar", "outcome": "Review schedule conflicts.", "disposition": "handoff",
         "source_message_ids": ["request"], "decision_message_ids": ["later-decision"]},
    ]
    service.guide.state.update(brief=brief, readiness={"ready": False, "decision_blockers": ["Obsolete cached decision."]})
    try:
        admitted = await service.submit(service.guide.identity, 3, "Implement these outcomes")
        assert set(service.tasks) == {admitted["build_id"]}
        assert admitted["status"] == "queued" and not admitted["ready_to_package"]
        directory = service._directory(service.guide.identity)
        assert json.loads((directory / "input.json").read_text())["brief"] == brief
    finally:
        await service.shutdown()


@pytest.mark.parametrize("brief", [None, {}])
async def test_legacy_pre_brief_proposals_retain_admission_semantics(service, monkeypatch, brief):
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: "/fictional/sandbox")
    if brief is None:
        service.guide.state.pop("brief")
    else:
        service.guide.state["brief"] = brief
    try:
        admitted = await service.submit(service.guide.identity, 3)
        assert set(service.tasks) == {admitted["build_id"]}
        assert not admitted["ready_to_package"]
    finally:
        await service.shutdown()


def workspace(service, status):
    root = service._directory(status["guide_id"], status["build_id"])
    return ImplementationWorkspace(root / "workspace", service.guide.state["proposal"])


def host_receipt(project, status="pass", tests=1):
    # Service tests simulate the host checker. Real execution and signing are
    # separately exercised by test_implementation_workspace.py.
    with project._locked():
        state = project._state()
        receipt = {"status": status, "tests_run": tests,
                   "ready_to_package": status == "pass" and tests > 0,
                   "digest": project._digest(project._files(state)),
                   "checks": [{"name": "authored tests", "status": status, "detail": "Fixture host result"}]}
        receipt["signature"] = project._signed(receipt)
        state["last_check"] = receipt
        project._save_state(state)
        return receipt


async def complete(service, status, result="pass", tests=1):
    task = service.tasks.pop(status["build_id"])
    task.cancel()
    await asyncio.gather(task, return_exceptions=True)
    project = workspace(service, status)
    project.write_file("application/__init__.py", "def build(context):\n    return object()\n")
    receipt = host_receipt(project, result, tests)
    update_receipt(service._directory(status["guide_id"], status["build_id"]),
                   status="completed", summary="Implemented scheduling logic", receipt=receipt,
                   checks=receipt["checks"], unresolved=["Live account authentication remains to be verified."],
                   cleanup_pending=False, cleanup_in_progress=False, worker_stop_confirmed=True)
    if result == "pass" and tests > 0:
        from launchkit.acceptance import SemanticAcceptance
        directory = service._directory(status["guide_id"], status["build_id"])
        frozen = json.loads((directory / "input.json").read_text())
        acceptance = SemanticAcceptance(directory / "acceptance", project, frozen)
        acceptance.begin_round()
        acceptance.bind_run("fixture-native-review")
        acceptance.write_test("test_entrypoint.py", "import unittest\nfrom application import build\n"
            "class Entry(unittest.TestCase):\n    def test_construction(self):\n        self.assertIsNotNone(build(None))\n")
        execution = await asyncio.to_thread(acceptance.check)
        assert execution["status"] == "pass", execution
        acceptance.publish({"summary": "Service fixture independently checked construction.",
            "requirements": [{"id": "requested-outcome", "status": "tested",
                "source_paths": ["application/__init__.py"],
                "test_ids": [row["id"] for row in execution["test_results"]],
                "evidence": "The construction test ran in the protected host harness.",
                "remaining_dependencies": ["External account authentication is not tested."]}],
            "counterexamples": [], "remaining_dependencies": ["Live authentication"]})
        acceptance.complete_run("fixture-native-review", {"kind": "interaction_complete",
            "interaction_id": "fixture-native-review", "cursor": "console:99"})
    return project


async def test_admission_freezes_evidence_and_deduplicates_active_and_completed_requests(service):
    first = await service.submit(service.guide.identity, 3)
    directory = service._directory(service.guide.identity, first["build_id"])
    evidence = json.loads((directory / "input.json").read_text())
    assert evidence["messages"][0]["text"] == "Preserve the required approval."
    assert evidence["brief"]["mission"].startswith("Compare")
    assert (await service.submit(service.guide.identity, 3))["build_id"] == first["build_id"]
    with pytest.raises(DraftConflict, match="already building"):
        await service.submit(service.guide.identity, 3, "Add a custom UI")
    await complete(service, first)
    assert (await service.submit(service.guide.identity, 3))["build_id"] == first["build_id"]
    await service.shutdown()


async def test_selected_build_fence_precedes_active_and_completed_idempotent_returns(service):
    with pytest.raises(DraftConflict, match="selected application build changed"):
        await service.submit(service.guide.identity, 3, expected_selected_build_id=str(uuid.uuid4()))
    assert not service.tasks and not list(service.root.glob("*/*/receipt.json"))
    first = await service.submit(service.guide.identity, 3, expected_selected_build_id=None)
    directory = service._directory(service.guide.identity)
    before = (directory / "receipt.json").read_bytes()
    for expected in (None, str(uuid.uuid4())):
        with pytest.raises(DraftConflict, match="selected application build changed"):
            await service.submit(service.guide.identity, 3, expected_selected_build_id=expected)
        assert (directory / "receipt.json").read_bytes() == before
    assert (await service.submit(service.guide.identity, 3,
        expected_selected_build_id=first["build_id"]))["build_id"] == first["build_id"]
    await complete(service, first)
    with pytest.raises(DraftConflict, match="selected application build changed"):
        await service.submit(service.guide.identity, 3, expected_selected_build_id=None)
    assert (await service.submit(service.guide.identity, 3,
        expected_selected_build_id=first["build_id"]))["build_id"] == first["build_id"]
    await service.shutdown()


async def test_pending_cleanup_keeps_checked_source_and_acceptance_facts_but_blocks_readiness(service):
    first = await service.submit(service.guide.identity, 3)
    await complete(service, first)
    accepted = await service.status(service.guide.identity)
    assert accepted["ready_to_package"] and accepted["acceptance_current"]
    directory = service._directory(service.guide.identity)
    update_receipt(directory, cleanup_pending=True, cleanup_in_progress=False,
                   worker_stop_confirmed=False)
    blocked = await service.status(service.guide.identity)
    assert blocked["status"] == "completed" and not blocked["ready_to_package"]
    for key in ("checks", "source_verified", "source_digest", "acceptance_trusted", "acceptance_current", "acceptance"):
        assert blocked[key] == accepted[key]
    assert blocked["cleanup_custody_lost"]
    with pytest.raises(ValueError, match="needs passing"):
        await service.package(service.guide.identity, first["build_id"])
    await service.shutdown()


async def test_normal_selection_fence_can_resume_dirty_failed_source_without_trial_authority(service):
    first = await service.submit(service.guide.identity, 3, expected_selected_build_id=None)
    task = service.tasks.pop(first["build_id"])
    task.cancel()
    await asyncio.gather(task, return_exceptions=True)
    directory = service._directory(service.guide.identity)
    update_receipt(directory, status="failed", ready_to_package=False)
    project = workspace(service, first)
    source = "def build(context):\n    return 'dirty failed source'\n"
    project.write_file("application/__init__.py", source)
    second = await service.submit(service.guide.identity, 3, "Repair this source",
                                  expected_selected_build_id=first["build_id"])
    assert second["build_id"] != first["build_id"]
    assert workspace(service, second).read_file("application/__init__.py") == source
    assert not second["ready_to_package"] and not second["acceptance_trusted"]
    await service.shutdown()


async def test_host_timings_are_bound_to_the_build_and_never_grant_readiness(service):
    from launchkit.implementation_timing import ImplementationTiming
    admitted = await service.submit(service.guide.identity, 3)
    directory = service._directory(service.guide.identity, admitted["build_id"])
    assert admitted["timing"]["build_id"] == admitted["build_id"]
    assert admitted["timing"]["phases"][0]["phase"] == "preparation"
    ImplementationTiming(directory).finish("completed")
    measured = await service.status(service.guide.identity)
    assert measured["timing"]["finished"] is True
    assert measured["status"] == "queued" and measured["source_verified"] is False
    assert measured["acceptance_trusted"] is False and measured["ready_to_package"] is False
    service.guide.state["version"] += 1
    service.guide.state["proposal"]["project"]["name"] = "Changed system scope"
    stale = await service.status(service.guide.identity)
    assert stale["current"] is False and stale["ready_to_package"] is False
    (directory / "timing.json").unlink()
    assert (await service.status(service.guide.identity))["timing"] is None
    await service.shutdown()


async def test_feedback_carries_authored_source_and_never_reuses_previous_checks(service):
    first = await service.submit(service.guide.identity, 3)
    await complete(service, first)
    second = await service.submit(service.guide.identity, 3, "Add a second requested outcome")
    assert second["build_id"] != first["build_id"]
    assert workspace(service, second).read_file("application/__init__.py").startswith("def build")
    assert workspace(service, second).verification() is None
    assert not second["ready_to_package"]
    assert read_receipt(service._directory(service.guide.identity, first["build_id"]))["status"] == "completed"
    await service.shutdown()


async def test_legacy_read_only_false_keeps_accepted_scope_and_same_spec_tests(service):
    from launchkit.acceptance import SemanticAcceptance
    from launchkit.blueprint import Blueprint
    legacy = deepcopy(service.guide.state["proposal"])
    assert all("read_only" not in item["tools"] for item in legacy["agent_classes"])
    first = await service.submit(service.guide.identity, 3, "Keep the requested useful job.")
    old = await complete(service, first)
    previous = service._directory(service.guide.identity, first["build_id"])
    input_bytes = (previous / "input.json").read_bytes()
    accepted_before = await service.status(service.guide.identity)
    assert accepted_before["current"] and accepted_before["acceptance_current"] and accepted_before["ready_to_package"]
    explicit_false = deepcopy(legacy)
    for item in explicit_false["agent_classes"]:
        item["tools"]["read_only"] = False
    service.guide.state["proposal"] = Blueprint.model_validate(explicit_false).model_dump(mode="json")
    assert service.guide.state["proposal"] == legacy
    accepted_after = await service.status(service.guide.identity)
    assert accepted_after["current"] and accepted_after["acceptance_trusted"] and accepted_after["acceptance_current"]
    assert accepted_after["ready_to_package"] and accepted_after["acceptance"] == accepted_before["acceptance"]
    first_frozen = json.loads(input_bytes)
    acceptance = SemanticAcceptance(previous / "acceptance", old, first_frozen)
    old_tests = {name: acceptance.read_test(name) for name in acceptance.context()["acceptance_tests"]}
    second = await service.submit(service.guide.identity, 3, "Explain the saved outcome.")
    current = service._directory(service.guide.identity, second["build_id"])
    second_frozen = json.loads((current / "input.json").read_text())
    retained = SemanticAcceptance(current / "acceptance", workspace(service, second), second_frozen)
    assert {name: retained.read_test(name) for name in retained.context()["acceptance_tests"]} == old_tests
    assert second_frozen["framework"]["basis"] == "accepted-baseline"
    assert (previous / "input.json").read_bytes() == input_bytes
    assert retained.verification() is None and not second["ready_to_package"]
    await service.shutdown()


def newer_framework(monkeypatch, extra_paths=()):
    import launchkit.implementation_workspace as module
    original = module.generate_package
    changed = {"app.py", "connectors.py", "remote_operations.py", "setup.py", *extra_paths}
    monkeypatch.setattr(module, "generate_package", lambda blueprint: [
        PackageFile(item.path, item.content + "\n# Refreshed compiler source.\n", item.mode)
        if item.path in changed else item for item in original(blueprint)])


async def test_installed_framework_continuation_keeps_newer_application_and_fresh_authority(service, monkeypatch):
    from launchkit.acceptance import SemanticAcceptance
    from launchkit.deployment_updates import compatibility
    first = await service.submit(service.guide.identity, 3, "Keep the original requested outcome.")
    installed = await complete(service, first)
    installed_digest = installed.digest()
    installed_files = {item.path: (item.content.encode(), item.mode)
                       for item in installed.export_files(require_verified=False)}
    installed_directory = service._directory(service.guide.identity, first["build_id"])
    original_receipt = (installed_directory / "receipt.json").read_bytes()
    original_input = (installed_directory / "input.json").read_bytes()
    service.guide.state["version"] = 4
    service.guide.state["brief"]["mission"] = "Show the current useful result independently of bounded history."
    fresh_brief = deepcopy(service.guide.state["brief"])
    newer_framework(monkeypatch, {"requirements.txt"})
    newer = await service.submit(service.guide.identity, 4, "Put the useful result first.", refresh_framework=True)
    candidate = workspace(service, newer)
    candidate.write_file("application/result_view.py", "TITLE = 'Useful result first'\n")
    candidate.write_file("tests/test_result_view.py", "import unittest\nfrom application.result_view import TITLE\n"
        "class ResultView(unittest.TestCase):\n    def test_current_title(self):\n        self.assertEqual(TITLE, 'Useful result first')\n")
    await complete(service, newer)
    candidate_digest = candidate.digest()
    candidate_directory = service._directory(service.guide.identity)
    candidate_input = json.loads((candidate_directory / "input.json").read_text())
    accepted = SemanticAcceptance(candidate_directory / "acceptance", candidate, candidate_input)
    retained_tests = {name: accepted.read_test(name) for name in accepted.context()["acceptance_tests"]}
    prior_files = {item.path: (item.content.encode(), item.mode)
                   for item in candidate.export_files(require_verified=False)}
    assert compatibility(installed_files, prior_files, {})[1]

    continued = await service.submit(service.guide.identity, 4, "Keep the useful UI and installed framework.",
        expected_previous_build_id=newer["build_id"], expected_source_digest=candidate_digest,
        expected_framework_build_id=first["build_id"], expected_framework_source_digest=installed_digest)
    combined = workspace(service, continued)
    combined_files = {item.path: (item.content.encode(), item.mode)
                     for item in combined.export_files(require_verified=False)}
    for name in ("app.py", "requirements.txt", "connectors.py", "remote_operations.py", "setup.py"):
        assert combined_files[name] == installed_files[name]
    assert combined_files["application/result_view.py"] == prior_files["application/result_view.py"]
    assert combined_files["tests/test_result_view.py"] == prior_files["tests/test_result_view.py"]
    assert compatibility(installed_files, combined_files, {})[1] == []
    frozen = json.loads((service._directory(service.guide.identity) / "input.json").read_text())
    assert frozen["brief"] == fresh_brief and frozen["brief"] != json.loads(original_input)["brief"]
    assert frozen["guide_version"] == 4
    assert frozen["previous_implementation"]["build_id"] == newer["build_id"]
    assert frozen["previous_implementation"]["copied_source_digest"] == candidate_digest
    assert frozen["implementation_requests"] == ["Keep the original requested outcome.",
        "Put the useful result first.", "Keep the useful UI and installed framework."]
    assert frozen["framework"]["previous"] == {"build_id": first["build_id"], "source_digest": installed_digest}
    assert frozen["framework"]["origin"] == frozen["framework"]["previous"]
    assert "requirements.txt" in frozen["framework"]["changed_contract_paths"]
    independent = SemanticAcceptance(service._directory(service.guide.identity) / "acceptance", combined, frozen)
    assert {name: independent.read_test(name) for name in independent.context()["acceptance_tests"]} == retained_tests
    assert combined.verification() is None and independent.verification() is None
    assert combined._key_path.read_bytes() not in (installed._key_path.read_bytes(), candidate._key_path.read_bytes())
    assert not continued["source_verified"] and not continued["acceptance_trusted"] and not continued["ready_to_package"]
    assert (installed_directory / "receipt.json").read_bytes() == original_receipt
    assert (installed_directory / "input.json").read_bytes() == original_input
    assert installed.digest() == installed_digest and candidate.digest() == candidate_digest
    await complete(service, continued)
    verified = independent.verification()
    assert verified["source_digest"] == combined.digest()
    assert verified["brief_digest"] == fingerprint(fresh_brief)
    await service.shutdown()


@pytest.mark.parametrize("corruption", ["digest", "source", "check", "acceptance", "scope", "blueprint", "policy", "foreign"])
async def test_installed_framework_requires_exact_current_host_check_and_independent_acceptance(service, corruption):
    first = await service.submit(service.guide.identity, 3, "Keep the original outcome.")
    installed = await complete(service, first)
    digest = installed.digest()
    directory = service._directory(service.guide.identity)
    if corruption == "blueprint":
        service.guide.state["proposal"]["project"]["description"] += " Different compiler input."
    current = await service.submit(service.guide.identity, 3, "Improve the current application.")
    await complete(service, current)
    before = set(service.root.glob("*/*"))
    build_id = first["build_id"]
    if corruption == "digest":
        digest = "0" * 64
    elif corruption == "source":
        installed.write_file("application/changed.py", "VALUE = 1\n")
    elif corruption == "check":
        with installed._locked():
            state = installed._state()
            state["last_check"]["signature"] = "0" * 64
            installed._save_state(state)
    elif corruption == "acceptance":
        path = directory / "acceptance" / "acceptance-state.json"
        saved = json.loads(path.read_text())
        saved["rounds"][-1]["publication"]["signature"] = "0" * 64
        path.write_text(json.dumps(saved))
    elif corruption == "scope":
        path = directory / "input.json"
        saved = json.loads(path.read_text())
        saved["brief"]["mission"] = "An unrelated scope."
        path.write_text(json.dumps(saved))
    elif corruption == "policy":
        update_receipt(directory, acceptance_policy_version=0)
    elif corruption == "foreign":
        build_id = str(uuid.uuid4())
    with pytest.raises(DraftConflict, match="framework baseline"):
        await service.submit(service.guide.identity, 3, "Continue the installed framework.",
            expected_framework_build_id=build_id, expected_framework_source_digest=digest)
    assert service.selected_build_id(service.guide.identity) == current["build_id"]
    assert set(service.root.glob("*/*")) == before and not service.tasks
    await service.shutdown()


async def test_installed_framework_change_during_preparation_refuses_worker_publication(service, monkeypatch):
    first = await service.submit(service.guide.identity, 3, "Keep the original outcome.")
    installed = await complete(service, first)
    digest = installed.digest()
    current = await service.submit(service.guide.identity, 3, "Improve the UI.")
    await complete(service, current)
    seed = ImplementationWorkspace.seed
    def changed_after_seed(project, *args, **kwargs):
        result = seed(project, *args, **kwargs)
        installed.write_file("application/late_change.py", "VALUE = 'changed after snapshot'\n")
        return result
    monkeypatch.setattr(ImplementationWorkspace, "seed", changed_after_seed)
    with pytest.raises(DraftConflict, match="framework baseline"):
        await service.submit(service.guide.identity, 3, "Keep the installed framework.",
            expected_framework_build_id=first["build_id"], expected_framework_source_digest=digest)
    assert service.selected_build_id(service.guide.identity) == current["build_id"]
    assert len(list(service.root.glob("*/*/receipt.json"))) == 2 and not service.tasks
    await service.shutdown()


@pytest.mark.parametrize("legacy", [False, True])
@pytest.mark.parametrize("dependency_drift", [False, True])
async def test_same_spec_feedback_preserves_accepted_host_and_cumulative_requests(service, monkeypatch, legacy, dependency_drift):
    from launchkit.deployment_updates import compatibility
    first = await service.submit(service.guide.identity, 3, "Preserve authenticated approval.")
    old = await complete(service, first)
    if legacy:
        # Already accepted workspaces predate framework provenance. Their
        # authentic current check and independent acceptance select the origin.
        with old._locked():
            stored = old._state()
            stored.pop("framework")
            old._save_state(stored)
    previous_files = {item.path: (item.content.encode(), item.mode)
                      for item in old.export_files(require_verified=False)}
    previous_input = (service._directory(service.guide.identity) / "input.json").read_bytes()
    old_key = old._key_path.read_bytes()
    old_digest = old.digest()
    newer_framework(monkeypatch, {"requirements.txt"} if dependency_drift else ())
    second = await service.submit(service.guide.identity, 3, "Explain the real saved status clearly.")
    new = workspace(service, second)
    current_files = {item.path: (item.content.encode(), item.mode)
                     for item in new.export_files(require_verified=False)}
    assert current_files == previous_files
    assert compatibility(previous_files, current_files, {"execution_mode": "interactive"}) == ([], [])
    assert new._key_path.read_bytes() != old_key
    assert new.verification() is None and not second["ready_to_package"]
    directory = service._directory(service.guide.identity)
    frozen = json.loads((directory / "input.json").read_text())
    assert frozen["framework"] == second["framework"] == new.framework_provenance()
    assert frozen["framework"]["basis"] == "accepted-baseline"
    assert frozen["framework"]["origin"] == {"build_id": first["build_id"], "source_digest": old_digest}
    assert frozen["framework"]["previous"] == frozen["framework"]["origin"]
    assert frozen["framework"]["retained_template_paths"] == sorted([
        "app.py", "connectors.py", "remote_operations.py", "setup.py", *(["requirements.txt"] if dependency_drift else [])])
    assert frozen["framework"]["changed_contract_paths"] == (["requirements.txt"] if dependency_drift else [])
    assert frozen["implementation_requests"] == ["Preserve authenticated approval.",
                                                  "Explain the real saved status clearly."]
    assert (service._directory(service.guide.identity, first["build_id"]) / "input.json").read_bytes() == previous_input
    assert old.digest() == old_digest
    # After fresh source verification/independent review, an ordinary authored
    # change remains eligible for the unchanged source-only compatibility gate.
    await complete(service, second)
    measured = await service.status(service.guide.identity)
    assert measured["framework"] == frozen["framework"]
    payload, _ = await service.package(service.guide.identity, second["build_id"])
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        report = json.loads(archive.read("implementation-report.json"))
        assert report["framework"] == measured["framework"]
        assert report["verification"]["digest"] == new.digest()
        assert report["verification"]["signature"] != read_receipt(
            service._directory(service.guide.identity, first["build_id"]))["receipt"]["signature"]
        for line in archive.read("SHA256SUMS").decode().splitlines():
            sha, path = line.split("  ", 1)
            assert hashlib.sha256(archive.read(path)).hexdigest() == sha
    new.write_file("application/status_help.txt", "Saved means the actual record was read.\n")
    changed_files = {item.path: (item.content.encode(), item.mode)
                     for item in new.export_files(require_verified=False)}
    changes, blockers = compatibility(previous_files, changed_files, {"execution_mode": "interactive"})
    assert [row["path"] for row in changes] == ["SHA256SUMS", "application/status_help.txt"]
    assert blockers == []
    await service.shutdown()


async def test_failed_continuation_keeps_signed_origin_but_not_previous_verification(service, monkeypatch):
    first = await service.submit(service.guide.identity, 3, "Preserve the original required outcome.")
    old = await complete(service, first)
    newer_framework(monkeypatch)
    second = await service.submit(service.guide.identity, 3, "Keep duplicate decisions explicit.")
    pending = service.tasks.pop(second["build_id"])
    pending.cancel()
    await asyncio.gather(pending, return_exceptions=True)
    failed = workspace(service, second)
    failed.write_file("application/partial.py", "VALUE = 'retained partial repair'\n")
    directory = service._directory(service.guide.identity)
    update_receipt(directory, status="failed", detail="Native review transport stopped.", ready_to_package=False)
    failed_source = failed.digest()
    assert failed.verification() is None
    second_input = (directory / "input.json").read_bytes()
    third = await service.submit(service.guide.identity, 3, "")
    repaired = workspace(service, third)
    assert repaired.read_file("app.py") == old.read_file("app.py")
    assert repaired.read_file("application/partial.py") == failed.read_file("application/partial.py")
    provenance = repaired.framework_provenance()
    assert provenance["origin"] == failed.framework_provenance()["origin"]
    assert provenance["previous"] == {"build_id": second["build_id"], "source_digest": failed_source}
    assert repaired.verification() is None and not third["acceptance_trusted"] and not third["ready_to_package"]
    frozen = json.loads((service._directory(service.guide.identity) / "input.json").read_text())
    assert frozen["implementation_requests"] == ["Preserve the original required outcome.",
                                                  "Keep duplicate decisions explicit."]
    assert frozen["request"] == "Keep duplicate decisions explicit."
    assert (directory / "input.json").read_bytes() == second_input
    await service.shutdown()


@pytest.mark.parametrize("edited_after_check", [False, True])
async def test_blank_failed_resume_gives_builder_bound_redacted_diagnostics_without_old_check_authority(
        service, monkeypatch, edited_after_check):
    monkeypatch.setenv("OPENAI_API_KEY", "private-provider-canary")
    original_request = "Keep approval review separate from scheduling."
    first = await service.submit(service.guide.identity, 3, original_request)
    task = service.tasks.pop(first["build_id"])
    task.cancel()
    await asyncio.gather(task, return_exceptions=True)
    old = workspace(service, first)
    old.write_file("application/approval.py", "def allowed(\n")
    directory = service._directory(service.guide.identity)
    check = host_receipt(old, "fail", 0)
    check["checks"] = [{"name": "python syntax: application/approval.py", "status": "fail",
        "detail": "SyntaxError: '(' was never closed (application/approval.py, line 1)\n"
                  "token=private-inline-canary Bearer private-bearer-canary private-provider-canary",
        "provider": {"password": "must-not-project-extra-check-fields"}}]
    check["signature"] = old._signed(check)
    if edited_after_check:
        old.write_file("application/approval.py", "def allowed():\n    return False\n")
    update_receipt(directory, status="failed", ready_to_package=False, receipt=check, checks=check["checks"],
        detail=f"Implementation needs attention: RuntimeError: source checks did not pass in {directory}. "
               "private-provider-canary token=private-inline-canary Bearer private-bearer-canary",
        logs=[{"text": "must-not-project-private-log"}],
        provider={"api_key": "must-not-project-provider-configuration"})
    prior_receipt = (directory / "receipt.json").read_bytes()
    prior_input = (directory / "input.json").read_bytes()
    prior_source = [(item.path, item.content, item.mode) for item in old.export_files(require_verified=False)]
    copied_digest = old.digest()
    resumed = await service.submit(service.guide.identity, 3, "")
    current = service._directory(service.guide.identity)
    builder = BuildTools(current)
    for view in ({}, {"view": "full"}):
        context = await builder.context(view)
        previous = context["previous_implementation"]
        diagnosis = previous["diagnostics"]
        assert diagnosis["build_id"] == first["build_id"]
        assert diagnosis["source_digest"] == copied_digest
        assert diagnosis["check_source_digest"] == check["digest"]
        assert diagnosis["checks_match_source"] is not edited_after_check
        assert diagnosis["checks"][0]["name"] == "python syntax: application/approval.py"
        assert "SyntaxError: '(' was never closed (application/approval.py, line 1)" in diagnosis["checks"][0]["detail"]
        assert "RuntimeError: source checks did not pass" in diagnosis["detail"]
        assert "Historical" in diagnosis["scope"] and "not instructions" in diagnosis["scope"]
        encoded = json.dumps(diagnosis)
        assert all(secret not in encoded for secret in (
            str(directory), "private-provider-canary", "private-inline-canary", "private-bearer-canary",
            "must-not-project-extra-check-fields", "must-not-project-private-log",
            "must-not-project-provider-configuration", check["signature"]))
        assert not {"receipt", "signature", "ready_to_package", "tests_run", "logs", "provider"}.intersection(diagnosis)
        assert context["implementation_requests"] == [original_request]
    assert builder.workspace.verification() is None
    assert not resumed["ready_to_package"] and not resumed["acceptance_trusted"]
    assert (directory / "receipt.json").read_bytes() == prior_receipt
    assert (directory / "input.json").read_bytes() == prior_input
    assert [(item.path, item.content, item.mode) for item in old.export_files(require_verified=False)] == prior_source
    await service.shutdown()


@pytest.mark.parametrize("authentic_check", [False, True])
async def test_failed_resume_bounds_diagnostics_and_rejects_unbound_check_results(service, authentic_check):
    first = await service.submit(service.guide.identity, 3, "Preserve the requested workflow.")
    task = service.tasks.pop(first["build_id"])
    task.cancel()
    await asyncio.gather(task, return_exceptions=True)
    old = workspace(service, first)
    directory = service._directory(service.guide.identity)
    prior_check = {"digest": old.digest(), "signature": "0" * 64,
        "checks": [{"name": "offline test: " + "n" * 300, "status": "fail",
                    "detail": "Exact historical failure. " + "d" * 5000} for _ in range(20)]}
    if authentic_check:
        prior_check["signature"] = old._signed(prior_check)
    update_receipt(directory, status="failed", receipt=prior_check, checks=prior_check["checks"], detail="Saved error. " + "x" * 5000)
    await service.submit(service.guide.identity, 3, "")
    builder = BuildTools(service._directory(service.guide.identity))
    diagnosis = (await builder.context({}))["previous_implementation"]["diagnostics"]
    assert len(diagnosis["detail"]) == 1500 and diagnosis["detail_truncated"]
    if authentic_check:
        assert len(diagnosis["checks"]) == 8 and diagnosis["checks_truncated"]
        assert all(len(row["name"]) == 180 and len(row["detail"]) == 1200 and row["truncated"]
                   for row in diagnosis["checks"])
        assert diagnosis["check_source_digest"] == prior_check["digest"]
        assert diagnosis["checks_match_source"] is True
        assert len(json.dumps(diagnosis)) < 14000
    else:
        assert diagnosis["checks"] == [] and diagnosis["check_source_digest"] is None
        assert diagnosis["checks_match_source"] is False
    assert builder.workspace.verification() is None
    await service.shutdown()


@pytest.mark.parametrize("change", ["brief", "blueprint"])
async def test_changed_contract_does_not_hide_host_migration_incompatibility(service, monkeypatch, change):
    from launchkit.deployment_updates import compatibility
    first = await service.submit(service.guide.identity, 3)
    old = await complete(service, first)
    previous_files = {item.path: (item.content.encode(), item.mode)
                      for item in old.export_files(require_verified=False)}
    newer_framework(monkeypatch)
    if change == "brief":
        service.guide.state["brief"]["mission"] += " Require a changed deployment behavior."
    elif change == "blueprint":
        service.guide.state["proposal"]["project"]["name"] = "Different native deployment"
    second = await service.submit(service.guide.identity, 3, "Implement the changed requested contract.")
    new = workspace(service, second)
    assert new.read_file("app.py") != old.read_file("app.py")
    provenance = new.framework_provenance()
    assert provenance["basis"] == "current-compiler" and provenance["origin"] is None
    assert provenance["reason"] == "changed-specification"
    assert provenance["changed_contract_paths"] == []
    current_files = {item.path: (item.content.encode(), item.mode)
                     for item in new.export_files(require_verified=False)}
    _, blockers = compatibility(previous_files, current_files, {"execution_mode": "interactive"})
    assert any("app.py" in item and "separate verified upgrade path" in item for item in blockers)
    assert new.verification() is None and not second["ready_to_package"]
    await service.shutdown()


async def test_trial_repair_rechecks_source_inside_build_admission_and_rejects_later_work(service):
    first = await service.submit(service.guide.identity, 3)
    await complete(service, first)
    checked = await service.status(service.guide.identity)
    for expected in ({"expected_previous_build_id": str(uuid.uuid4()),
                      "expected_source_digest": checked["source_digest"]},
                     {"expected_previous_build_id": first["build_id"],
                      "expected_source_digest": "b" * 64}):
        with pytest.raises(DraftConflict, match="changed since this trial"):
            await service.submit(service.guide.identity, 3, "Fix the trial", **expected)
    second = await service.submit(service.guide.identity, 3, "Fix the trial",
        expected_previous_build_id=first["build_id"], expected_source_digest=checked["source_digest"])
    assert second["build_id"] != first["build_id"]
    assert workspace(service, second).read_file("application/__init__.py").startswith("def build")
    assert not second["ready_to_package"]
    with pytest.raises(DraftConflict, match="changed since this trial"):
        await service.submit(service.guide.identity, 3, "Fix the trial",
            expected_previous_build_id=first["build_id"], expected_source_digest=checked["source_digest"])
    await service.shutdown()


# A progress gate for real threaded preparation, not a speed claim. Under a 15%
# CPU quota the preparation before the paused step took up to 6.7 s, so a 5 s
# wait failed although it was only slow. A stuck or lost wake-up still fails,
# with its recorded cause, in reach_preparation.
PREPARATION_BARRIER_SECONDS = 60


class PreparationBarrier(asyncio.Event):
    """The paused step's wake-up plus the timeline that explains a miss."""

    def __init__(self, method):
        super().__init__()
        self.method = method
        self.entered = self.returned = None


def pause_build_preparation(monkeypatch, method):
    """Hold real threaded preparation before admission, permitting a competing edit."""
    loop = asyncio.get_running_loop()
    reached, release = PreparationBarrier(method), threading.Event()
    original = getattr(ImplementationWorkspace, method)

    def paused(project, *args, **kwargs):
        reached.entered = time.monotonic()
        result = original(project, *args, **kwargs)
        reached.returned = time.monotonic()
        loop.call_soon_threadsafe(reached.set)
        if not release.wait(10):
            raise TimeoutError("The test did not release build preparation.")
        return result

    monkeypatch.setattr(ImplementationWorkspace, method, paused)
    return reached, release


async def reach_preparation(reached, admission):
    """Wait for the paused preparation step, or fail with why it was not reached."""
    started = time.monotonic()
    waiter = asyncio.ensure_future(reached.wait())
    try:
        await asyncio.wait({waiter, admission}, timeout=PREPARATION_BARRIER_SECONDS,
                           return_when=asyncio.FIRST_COMPLETED)
    finally:
        waiter.cancel()
    if reached.is_set():
        assert reached.returned is not None, "The barrier woke without its paused step returning."
        return

    def at(moment):
        return "never" if moment is None else f"{moment - started:+.1f}s"

    if reached.returned is not None:
        pytest.fail(f"{reached.method} returned at {at(reached.returned)}, but its wake-up never reached the "
                    "event loop (lost wake-up).")
    if admission.done():
        cause = "cancelled" if admission.cancelled() else repr(admission.exception() or admission.result())
        pytest.fail(f"Build admission ended before preparation reached {reached.method}: {cause}")
    pytest.fail(f"Preparation did not reach {reached.method} within {PREPARATION_BARRIER_SECONDS}s "
                f"(entered {at(reached.entered)}, returned {at(reached.returned)}).")


async def test_selected_build_changed_during_preparation_does_not_publish_worker(service, monkeypatch):
    from launchkit.templates.storage_setup import write_private
    first = await service.submit(service.guide.identity, 3)
    await complete(service, first, result="fail")
    second = await service.submit(service.guide.identity, 3, "A separate repair")
    await complete(service, second, result="fail")
    parent = service._directory(service.guide.identity).parent
    write_private(parent / "latest.json", {"build_id": first["build_id"]})
    reached, release = pause_build_preparation(monkeypatch, "list_files")
    admission = asyncio.create_task(service.submit(service.guide.identity, 3, "Repair the displayed source",
                                   expected_selected_build_id=first["build_id"]))
    try:
        await reach_preparation(reached, admission)
        write_private(parent / "latest.json", {"build_id": second["build_id"]})
        release.set()
        with pytest.raises(DraftConflict, match="application changed while preparing"):
            await admission
        assert service.selected_build_id(service.guide.identity) == second["build_id"]
        assert not service.tasks
        assert len(list(service.root.glob("*/*/receipt.json"))) == 2
    finally:
        release.set()
        await asyncio.gather(admission, return_exceptions=True)
        await service.shutdown()


@pytest.mark.parametrize("change", ["version", "proposal", "brief", "messages", "operation"])
async def test_build_rechecks_setup_after_threaded_preparation_before_publishing_worker(service, monkeypatch, change):
    reached, release = pause_build_preparation(monkeypatch, "seed")
    admission = asyncio.create_task(service.submit(service.guide.identity, 3))
    try:
        await reach_preparation(reached, admission)
        if change == "version":
            service.guide.state["version"] += 1
        elif change == "proposal":
            service.guide.state["proposal"]["project"]["name"] = "A different requested system"
        elif change == "brief":
            service.guide.state["brief"]["mission"] = "A changed outcome"
        elif change == "messages":
            service.guide.state["messages"].append({"role": "user", "text": "Require an independent operator."})
        else:
            service.guide.state["operation"] = {"status": "running"}
        release.set()
        with pytest.raises(DraftConflict, match="changed while preparing"):
            await admission
        assert not service.tasks
        assert not (service.root / service.guide.identity / "latest.json").exists()
        assert not list(service.root.glob("*/*/receipt.json"))
    finally:
        release.set()
        await asyncio.gather(admission, return_exceptions=True)
        await service.shutdown()


async def test_refused_first_preparation_allows_current_setup_retry_without_losing_saved_work(service, monkeypatch):
    from launchkit.implementation import NoApplicationBuild, BuildSelectionUnavailable
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: "/fictional/sandbox")
    reached, release = pause_build_preparation(monkeypatch, "seed")
    admission = asyncio.create_task(service.submit(service.guide.identity, 3, "Prepare my first application"))
    try:
        await reach_preparation(reached, admission)
        service.guide.state["version"] = 4
        service.guide.state["messages"].append({"role": "user", "text": "Include the revised household approval."})
        release.set()
        with pytest.raises(DraftConflict, match="changed while preparing"):
            await admission
        assert not service.tasks and not service.processes
        with pytest.raises(NoApplicationBuild):
            await service.status(service.guide.identity)
        current = await service.submit(service.guide.identity, 4, "Build with my revised requirement")
        assert set(service.tasks) == {current["build_id"]}
        assert current["guide_version"] == 4 and current["status"] == "queued"
        frozen = json.loads((service._directory(service.guide.identity) / "input.json").read_text())
        assert frozen["messages"] == service.guide.state["messages"]
        assert frozen["request"] == "Build with my revised requirement"
        assert len(list(service.root.glob("*/*/receipt.json"))) == 1
        await service.shutdown()
        # A genuinely unselected prior build still requires recovery, rather
        # than becoming permission to silently replace its saved work.
        selected = service._directory(service.guide.identity)
        retained = {str(p.relative_to(selected)): p.read_bytes() for p in selected.rglob("*") if p.is_file()}
        (selected.parent / "latest.json").unlink()
        with pytest.raises(BuildSelectionUnavailable, match="without a selected build"):
            await service.submit(service.guide.identity, 4, "Do not replace the saved application")
        assert retained == {str(p.relative_to(selected)): p.read_bytes() for p in selected.rglob("*") if p.is_file()}
        assert not service.tasks and not service.processes
    finally:
        release.set()
        await asyncio.gather(admission, return_exceptions=True)
        await service.shutdown()


async def test_failed_first_workspace_preparation_retains_bytes_and_permits_explicit_retry(service, monkeypatch):
    from launchkit.implementation import NoApplicationBuild
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: "/fictional/sandbox")
    original = ImplementationWorkspace.seed
    failed = False

    def unavailable(project, *args, **kwargs):
        nonlocal failed
        result = original(project, *args, **kwargs)
        if not failed:
            failed = True
            raise OSError("Synthetic first preparation write failed")
        return result

    monkeypatch.setattr(ImplementationWorkspace, "seed", unavailable)
    with pytest.raises(OSError, match="first preparation write failed"):
        await service.submit(service.guide.identity, 3)
    assert not service.tasks and not service.processes
    retained = service.root.parent / "unadmitted-builds" / service.guide.identity
    assert len(list(retained.glob("*/workspace/project/app.py"))) == 1
    retained_bytes = {str(p.relative_to(retained)): p.read_bytes() for p in retained.rglob("*") if p.is_file()}
    with pytest.raises(NoApplicationBuild):
        await service.status(service.guide.identity)
    try:
        current = await service.submit(service.guide.identity, 3, "Retry the refused first preparation")
        assert set(service.tasks) == {current["build_id"]}
        assert current["status"] == "queued"
        assert len(list(service.root.glob("*/*/receipt.json"))) == 1
        assert retained_bytes == {str(p.relative_to(retained)): p.read_bytes() for p in retained.rglob("*") if p.is_file()}
    finally:
        await service.shutdown()


async def test_lost_status_after_worker_publication_never_archives_admitted_build(service, monkeypatch):
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: "/fictional/sandbox")

    async def unavailable(*args, **kwargs):
        raise OSError("Synthetic observation failed after publication")

    monkeypatch.setattr(service, "status", unavailable)
    try:
        with pytest.raises(OSError, match="observation failed after publication"):
            await service.submit(service.guide.identity, 3)
        selected = service._directory(service.guide.identity)
        assert set(service.tasks) == {selected.name}
        assert read_receipt(selected)["status"] == "queued"
        assert (selected / "input.json").is_file()
        assert not (service.root.parent / "unadmitted-builds").exists()
    finally:
        await service.shutdown()


@pytest.mark.parametrize("repair,change", [(False, "source"), (True, "source"), (True, "verification")])
async def test_continued_build_rechecks_copied_source_and_trial_proof_at_final_admission(service, monkeypatch, repair, change):
    first = await service.submit(service.guide.identity, 3)
    old = await complete(service, first)
    checked = await service.status(service.guide.identity)
    previous = service._directory(service.guide.identity)
    # list_files runs after source copying and retained-test preparation. The
    # old status reads its private inventory instead, so the barrier is the new
    # build's last awaited preparation step.
    reached, release = pause_build_preparation(monkeypatch, "list_files")
    expected = {"expected_previous_build_id": first["build_id"],
                "expected_source_digest": checked["source_digest"]} if repair else {}
    admission = asyncio.create_task(service.submit(service.guide.identity, 3, "Improve from the trial", **expected))
    try:
        await reach_preparation(reached, admission)
        if change == "source":
            old.write_file("application/__init__.py", "def build(context):\n    return 'later source'\n")
        else:
            host_receipt(old, "fail", 1)
        release.set()
        with pytest.raises(DraftConflict, match="application changed while preparing"):
            await admission
        assert service._directory(service.guide.identity) == previous
        assert not service.tasks
        assert len(list(service.root.glob("*/*/receipt.json"))) == 1
    finally:
        release.set()
        await asyncio.gather(admission, return_exceptions=True)
        await service.shutdown()


async def test_failed_browser_retry_retains_exact_request_source_and_protected_tests_without_receipts(service):
    from launchkit.acceptance import SemanticAcceptance
    original_request = "Keep approval revocation responsive while an adapter request is pending."
    first = await service.submit(service.guide.identity, 3, original_request)
    old = await complete(service, first)
    previous = service._directory(service.guide.identity, first["build_id"])
    previous_frozen = json.loads((previous / "input.json").read_text())
    old.write_file("application/approval.py", "def allowed(revoked):\n    return not revoked\n")
    old_acceptance = SemanticAcceptance(previous / "acceptance", old, previous_frozen)
    # The failed reviewer had begun its second round and authored another real
    # regression case, but transport failure prevented publication/completion.
    old_acceptance.begin_round()
    old_acceptance.bind_run("failed-native-review")
    test_source = ("import unittest\nfrom application.approval import allowed\n"
                   "class Approval(unittest.TestCase):\n"
                   "    def test_revoked_approval_is_rejected(self):\n"
                   "        self.assertFalse(allowed(True))\n")
    old_acceptance.write_test("test_revocation.py", test_source)
    update_receipt(previous, status="failed", phase="independent_review", ready_to_package=False,
                   detail="Native reviewer stream ended without Done event.")
    (previous / "timeline.jsonl").write_text('{"kind":"interaction_failed","cursor":"console:81"}\n')
    before_input = (previous / "input.json").read_bytes()
    before_review = (previous / "acceptance/acceptance-state.json").read_bytes()
    # This is the existing browser Retry implementation request with its cleared
    # composer. It admits a new worker without rewriting the failed build.
    second = await service.submit(service.guide.identity, 3, "")
    assert second["build_id"] != first["build_id"]
    current = service._directory(service.guide.identity, second["build_id"])
    frozen = json.loads((current / "input.json").read_text())
    assert frozen["request"] == original_request
    assert frozen["implementation_requests"] == [original_request]
    assert frozen["brief"] == previous_frozen["brief"]
    assert frozen["messages"] == previous_frozen["messages"]
    assert frozen["blueprint"] == previous_frozen["blueprint"]
    assert second["request"] == original_request
    assert (await service.submit(service.guide.identity, 3, ""))["build_id"] == second["build_id"]
    new = workspace(service, second)
    assert new.read_file("application/approval.py") == old.read_file("application/approval.py")
    assert new.verification() is None
    accepted = SemanticAcceptance(current / "acceptance", new, frozen)
    assert accepted.read_test("test_revocation.py") == test_source
    assert set(accepted.context()["acceptance_tests"]) == {"test_entrypoint.py", "test_revocation.py"}
    assert accepted.verification() is None
    assert accepted.history()[0]["reviewer_interaction_id"] is None
    assert accepted.history()[0]["publication"] is None
    assert accepted.history()[0]["execution"] is None
    assert (current / "acceptance/acceptance-key").read_bytes() != (previous / "acceptance/acceptance-key").read_bytes()
    assert not (new.project / "acceptance-tests").exists()
    assert not second["ready_to_package"]
    with pytest.raises(ValueError, match="issued by the host"):
        new.write_file("acceptance-tests/test_revocation.py", "raise AssertionError\n")
    builder = BuildTools(current)
    builder.acceptance = accepted
    feedback = await builder.feedback({})
    assert feedback["assessment"]["summary"] == "Service fixture independently checked construction."
    assert (await builder.feedback({"path": "test_revocation.py"}))["content"] == test_source
    assert (previous / "input.json").read_bytes() == before_input
    assert (previous / "acceptance/acceptance-state.json").read_bytes() == before_review
    assert '"interaction_failed"' in (previous / "timeline.jsonl").read_text()
    await service.shutdown()


async def test_new_feedback_retains_earlier_implementation_requests_as_original_evidence(service):
    first = await service.submit(service.guide.identity, 3, "Preserve shared approval across interfaces.")
    await complete(service, first)
    second = await service.submit(service.guide.identity, 3, "Add duplicate request protection.")
    frozen = json.loads((service._directory(service.guide.identity, second["build_id"]) / "input.json").read_text())
    assert frozen["request"] == "Add duplicate request protection."
    assert frozen["implementation_requests"] == ["Preserve shared approval across interfaces.",
                                                   "Add duplicate request protection."]
    assert frozen["previous_implementation"]["request"] == "Preserve shared approval across interfaces."
    await service.shutdown()


async def test_first_partial_review_retry_exposes_protected_tests_without_inventing_assessment(service):
    from launchkit.acceptance import SemanticAcceptance
    if not actual_sandbox_executable():
        pytest.skip("Independent checks have no unsandboxed execution fallback.")
    original_request = "Revoked approval must reject the next action."
    first = await service.submit(service.guide.identity, 3, original_request)
    pending = service.tasks.pop(first["build_id"])
    pending.cancel()
    await asyncio.gather(pending, return_exceptions=True)
    previous = service._directory(service.guide.identity, first["build_id"])
    old = workspace(service, first)
    old.write_file("application/__init__.py", "def build(context):\n    return object()\n")
    old.write_file("application/approval.py", "def allowed(revoked):\n    return True\n")
    previous_frozen = json.loads((previous / "input.json").read_text())
    original_acceptance = SemanticAcceptance(previous / "acceptance", old, previous_frozen)
    original_acceptance.begin_round()
    original_acceptance.bind_run("native-first-review-failed")
    test_source = ("import unittest\nfrom application.approval import allowed\n"
                   "class Approval(unittest.TestCase):\n"
                   "    def test_revoked_approval_is_rejected(self):\n"
                   "        self.assertFalse(allowed(True))\n")
    original_acceptance.write_test("test_revocation.py", test_source)
    assert original_acceptance.history()[0]["publication"] is None
    update_receipt(previous, status="failed", phase="independent_review", ready_to_package=False,
                   detail="The first native reviewer stream ended before publishing findings.")
    previous_state = (previous / "acceptance/acceptance-state.json").read_bytes()
    second = await service.submit(service.guide.identity, 3, "")
    current = service._directory(service.guide.identity, second["build_id"])
    builder = BuildTools(current)
    assert builder.independent_feedback is None
    builder.acceptance = SemanticAcceptance(current / "acceptance", builder.workspace, builder.input)
    feedback = await builder.feedback({})
    assert feedback["assessment"] is None
    assert feedback["assessment_status"] == "not_published"
    assert feedback["tests"] == ["test_revocation.py"]
    assert "no independent assessment" in feedback["detail"]
    read = await builder.feedback({"path": "test_revocation.py"})
    assert read == {"path": "test_revocation.py", "content": test_source}
    assert not (await builder.feedback({"path": "../acceptance-key"}))["ok"]
    assert not (await builder.write({"path": "acceptance-tests/test_revocation.py",
                                    "content": "raise AssertionError\n"}))["ok"]
    # The retained reviewer case still reproduces the real source defect in the
    # fresh host sandbox. Reading it neither edits it nor establishes acceptance.
    execution = await asyncio.to_thread(builder.acceptance.check)
    assert execution["status"] == "fail", execution
    assert execution["test_results"][0]["id"] == "test_revocation.py::Approval.test_revoked_approval_is_rejected"
    assert execution["test_results"][0]["status"] == "fail"
    assert builder.acceptance.read_test("test_revocation.py") == test_source
    assert builder.acceptance.verification() is None
    assert not (await service.status(service.guide.identity))["ready_to_package"]
    assert (previous / "acceptance/acceptance-state.json").read_bytes() == previous_state
    await service.shutdown()


async def test_revised_design_retains_app_source_with_fresh_native_host_and_no_old_checks(service):
    first = await service.submit(service.guide.identity, 3)
    old = await complete(service, first)
    old_deployment = old.read_file("deployment.json")
    old_blueprint = deepcopy(service.guide.state["proposal"])
    service.guide.state["version"] = 4
    service.guide.state["proposal"]["project"]["name"] = "Revised schedule application"
    second = await service.submit(service.guide.identity, 4, "Adapt to the revised design")
    new = workspace(service, second)
    assert new.read_file("application/__init__.py") == old.read_file("application/__init__.py")
    assert new.read_file("deployment.json") != old_deployment
    assert "Revised schedule application" in new.read_file("deployment.json")
    assert new.verification() is None and not second["ready_to_package"]
    frozen = json.loads((service._directory(service.guide.identity) / "input.json").read_text())
    assert frozen["blueprint"] == service.guide.state["proposal"]
    assert frozen["previous_implementation"]["blueprint"] == old_blueprint
    assert frozen["previous_implementation"]["build_id"] == first["build_id"]
    assert frozen["previous_implementation"]["retained_acceptance_tests"] == []
    assert not (service._directory(service.guide.identity, second["build_id"]) / "acceptance").exists()
    assert old.read_file("deployment.json") == old_deployment
    await service.shutdown()


@pytest.mark.parametrize("result,tests", [("fail", 2), ("unavailable", 2), ("pass", 0)])
async def test_completed_label_cannot_package_without_current_passing_executable_checks(service, result, tests):
    status = await service.submit(service.guide.identity, 3)
    await complete(service, status, result, tests)
    assert not (await service.status(service.guide.identity))["ready_to_package"]
    with pytest.raises(ValueError, match="passing executable checks"):
        await service.package(service.guide.identity)


async def test_export_contains_exact_checked_source_live_gaps_and_complete_hash_manifest(service):
    status = await service.submit(service.guide.identity, 3)
    project = await complete(service, status)
    payload, slug = await service.package(service.guide.identity, status["build_id"])
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        assert archive.read("application/__init__.py").decode() == project.read_file("application/__init__.py")
        report = json.loads(archive.read("implementation-report.json"))
        assert report["build_id"] == status["build_id"]
        assert report["verification"]["digest"] == project.digest()
        assert report["unresolved"] == ["Live account authentication remains to be verified."]
        lines = archive.read("SHA256SUMS").decode().splitlines()
        assert len(lines) == len(archive.namelist()) - 1
        for line in lines:
            digest, name = line.split("  ", 1)
            assert hashlib.sha256(archive.read(name)).hexdigest() == digest
        assert archive.getinfo("start.sh").external_attr >> 16 & 0o111
        assert not any("verification-key" in name or "native/" in name for name in archive.namelist())
    project.write_file("application/__init__.py", "def build(context):\n    return None\n")
    with pytest.raises(ValueError, match="passing executable checks"):
        await service.package(service.guide.identity)


async def test_legacy_accepted_export_omits_framework_and_preserves_original_manifest(service):
    admitted = await service.submit(service.guide.identity, 3)
    directory = service._directory(service.guide.identity, admitted["build_id"])
    project = workspace(service, admitted)
    # Reproduce the metadata of an already accepted pre-provenance build without
    # changing any compiled source, native configuration or verification key.
    for path in (directory / "input.json", directory / "receipt.json"):
        value = json.loads(path.read_text())
        value.pop("framework")
        path.write_text(json.dumps(value))
    with project._locked():
        stored = project._state()
        stored.pop("framework")
        project._save_state(stored)
    key = project._key_path.read_bytes()
    baseline = project.digest()
    await complete(service, admitted)
    checked = await service.status(service.guide.identity)
    assert checked["ready_to_package"] and "framework" not in checked
    assert project._key_path.read_bytes() == key
    frozen = json.loads((directory / "input.json").read_text())
    # This is the report contract before framework preservation was introduced.
    # Legacy exports must retain it byte-for-byte instead of adding framework:{}.
    from launchkit.projects import fingerprint
    expected = {name: checked[name] for name in
                ("build_id", "guide_version", "blueprint_hash", "summary", "checks", "unresolved")}
    expected["brief_hash"] = fingerprint(frozen.get("brief") or {})
    expected["verification"] = project.verification()
    expected["acceptance"] = checked["acceptance"]
    expected["acceptance_scope"] = "offline"
    payload, _ = await service.package(service.guide.identity, admitted["build_id"])
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        assert archive.read("implementation-report.json") == (json.dumps(expected, indent=2) + "\n").encode()
        for line in archive.read("SHA256SUMS").decode().splitlines():
            sha, path = line.split("  ", 1)
            assert hashlib.sha256(archive.read(path)).hexdigest() == sha
    again, _ = await service.package(service.guide.identity, admitted["build_id"])
    assert again == payload
    assert project.digest() != baseline  # Only the fixture's authored application was added.
    assert "framework" not in json.loads((directory / "receipt.json").read_text())
    await service.shutdown()


async def test_design_revision_and_build_identity_fence_inspection_and_download(service):
    first = await service.submit(service.guide.identity, 3)
    await complete(service, first)
    service.guide.state["version"] = 4
    service.guide.state["brief"]["mission"] = "A changed requested application outcome"
    assert not (await service.status(service.guide.identity))["current"]
    with pytest.raises(DraftConflict, match="guide changed"):
        await service.package(service.guide.identity, first["build_id"])
    second = await service.submit(service.guide.identity, 4, "Continue current design")
    for call in (service.file(service.guide.identity, "app.py", first["build_id"]),
                 service.timeline(service.guide.identity, build_id=first["build_id"]),
                 service.package(service.guide.identity, first["build_id"])):
        with pytest.raises(DraftConflict, match="newer application build"):
            await call
    assert second["guide_version"] == 4
    await service.shutdown()


async def test_changed_blueprint_same_version_is_stale(service):
    status = await service.submit(service.guide.identity, 3)
    await complete(service, status)
    service.guide.state["proposal"]["project"]["name"] += " changed"
    assert not (await service.status(service.guide.identity))["current"]
    with pytest.raises(DraftConflict):
        await service.package(service.guide.identity)


async def test_package_rechecks_revision_after_export_yields(service, monkeypatch):
    status = await service.submit(service.guide.identity, 3)
    await complete(service, status)
    original = ImplementationWorkspace.export_files
    def exported(project, **options):
        files = original(project, **options)
        service.guide.state["version"] += 1
        return files
    monkeypatch.setattr(ImplementationWorkspace, "export_files", exported)
    with pytest.raises(DraftConflict, match="changed while preparing"):
        await service.package(service.guide.identity)


async def test_detached_package_preserves_verified_export_and_both_status_checks_without_guide_reentry(service, monkeypatch):
    status = await service.submit(service.guide.identity, 3)
    await complete(service, status)
    state = deepcopy(service.guide.state)
    original_status = service.status
    calls = []
    async def checked(identity, *, state=None):
        calls.append(state)
        return await original_status(identity, state=state)
    async def forbidden(identity):
        raise AssertionError("Detached package checks must not reread the guide")
    monkeypatch.setattr(service, "status", checked)
    monkeypatch.setattr(service.guide, "get", forbidden)
    payload, slug = await service.package(service.guide.identity, status["build_id"], state=state)
    assert calls == [state, state] and slug == state["proposal"]["project"]["slug"]
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        report = json.loads(archive.read("implementation-report.json"))
        assert report["build_id"] == status["build_id"]
        assert report["verification"]["digest"] == (await original_status(service.guide.identity, state=state))["source_digest"]
    harmless = {**state, "version": 4}
    continued, _ = await service.package(service.guide.identity, status["build_id"], state=harmless)
    assert continued == payload
    checked = await original_status(service.guide.identity, state=harmless)
    assert checked["current"] and checked["guide_version"] == 3 and checked["current_guide_version"] == 4
    with pytest.raises(ValueError, match="matching guide"):
        await service.package(service.guide.identity, status["build_id"], state={**state, "id": "another"})


async def test_receipt_and_native_frames_survive_restart_with_honest_interruption(service):
    status = await service.submit(service.guide.identity, 3)
    directory = service._directory(service.guide.identity, status["build_id"])
    frames = [{"kind": "tool_call", "cursor": "console:8", "payload": {"name": "check_implementation"}},
              {"kind": "reasoning_summary", "cursor": "console:12", "payload": {"text": "Provider supplied summary"}}]
    (directory / "timeline.jsonl").write_text("".join(json.dumps(frame) + "\n" for frame in frames) + '{"unfinished":')
    resumed = Service(service.root.parent, service.guide)
    assert (await resumed.status(service.guide.identity))["status"] == "interrupted"
    assert (await resumed.timeline(service.guide.identity))["frames"] == frames
    assert (await resumed.timeline(service.guide.identity, after="console:10"))["frames"] == frames[1:]
    with pytest.raises(ValueError, match="cursor"):
        await resumed.timeline(service.guide.identity, after="wrong")
    await service.shutdown()


async def test_shutdown_before_task_starts_is_durable_and_retryable(service):
    status = await service.submit(service.guide.identity, 3)
    await service.shutdown()
    assert read_receipt(service._directory(service.guide.identity))["status"] == "interrupted"
    retried = await service.submit(service.guide.identity, 3)
    assert retried["build_id"] != status["build_id"]
    await service.shutdown()


async def test_guide_operation_and_stale_versions_reject_build_before_worker_admission(service):
    class Operations:
        async def read(self, guide, identity):
            return {**await guide.get(identity), "operation": {"status": "running"}}
    service.operations = Operations()
    with pytest.raises(DraftConflict, match="current revision"):
        await service.submit(service.guide.identity, 3)
    with pytest.raises(DraftConflict, match="changed"):
        await service.submit(service.guide.identity, 2)
    assert not service.tasks


async def test_unsupported_check_host_refuses_paid_work_before_worker_admission(service, monkeypatch):
    from launchkit.architect import ArchitectUnavailable
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: None)
    with pytest.raises(ArchitectUnavailable, match="isolated app checks") as refused:
        await service.submit(service.guide.identity, 3)
    assert "generated code was not executed" in str(refused.value)
    assert not service.tasks and not service.processes and not service.root.exists()


async def test_build_tools_cannot_publish_unchecked_modified_or_testless_sources(service):
    status = await service.submit(service.guide.identity, 3)
    directory = service._directory(service.guide.identity)
    tools = BuildTools(directory)
    args = {"summary": "Built application", "unresolved": ["External auth needed"]}
    assert not (await tools.publish(args))["ok"]
    assert not (await tools.write({"path": "app.py", "content": "replacement"}))["ok"]
    assert (await tools.write({"path": "application/__init__.py", "content": "def build(context):\n    return object()\n"}))["ok"]
    host_receipt(tools.workspace)
    assert (await tools.publish(args))["ok"]
    assert tools.publication["checked_digest"] == tools.workspace.digest()
    assert (await tools.write({"path": "application/domain.py", "content": "VALUE = 1\n"}))["ok"]
    assert tools.publication is None and not (await tools.publish(args))["ok"]
    assert (await tools.reference({"name": "application"}))["content"]
    assert "async" in (await tools.reference({"name": "runtime", "limit": 250}))["content"]
    assert "dispatch_text_and_wait" in (await tools.reference({"name": "messaging", "limit": 250}))["content"]
    assert "run_flow" in (await tools.reference({"name": "flows", "limit": 250}))["content"]
    assert "host_runnables" in (await tools.reference({"name": "schedule", "limit": 250}))["content"]
    await service.shutdown()


async def test_active_status_does_not_wait_on_generated_test_workspace_lock(service, monkeypatch):
    status = await service.submit(service.guide.identity, 3)
    def blocked(*args):
        raise AssertionError("Polling cannot acquire the executable check's workspace lock.")
    monkeypatch.setattr(ImplementationWorkspace, "list_files", blocked)
    monkeypatch.setattr(ImplementationWorkspace, "verification", blocked)
    receipt = await asyncio.wait_for(service.status(service.guide.identity), .2)
    assert receipt["status"] == "queued" and receipt["files"] == status["files"]
    await service.shutdown()


@pytest.mark.parametrize("kind", ["interaction_failed", "run_failed", "message_delivery_failed", "interaction_canceled"])
def test_native_failures_end_build_instead_of_waiting_for_success_or_timeout(kind):
    with pytest.raises(RuntimeError, match=kind):
        interaction_completed([{"kind": "interaction_complete", "interaction_id": "ours"},
                               {"kind": kind, "interaction_id": "ours", "payload": {"error": "Known failure"}}], "ours")
    assert not interaction_completed([{"kind": kind, "interaction_id": "another"}], "ours")
    assert interaction_completed([{"kind": "interaction_complete", "interaction_id": "ours"}], "ours")


async def test_host_eof_cancels_an_awaited_native_admission_and_preserves_retry_receipt(service, monkeypatch):
    import os
    from types import SimpleNamespace
    from launchkit import implementation_worker as worker
    status = await service.submit(service.guide.identity, 3)
    directory = service._directory(service.guide.identity)
    read_fd, write_fd = os.pipe()
    waiting, canceled = asyncio.Event(), asyncio.Event()
    async def pending_admission(root, stop, *, timing=None):
        waiting.set()
        try:
            await asyncio.Event().wait()
        finally:
            assert stop.is_set()
            canceled.set()
    monkeypatch.setattr(worker, "execute", pending_admission)
    monkeypatch.setattr(worker.sys, "stdin", SimpleNamespace(fileno=lambda: read_fd))
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    task = asyncio.create_task(worker.run(directory, owned_stdin=True))
    try:
        await asyncio.wait_for(waiting.wait(), 5)
        os.close(write_fd)
        write_fd = None
        await asyncio.wait_for(task, 1)
        assert canceled.is_set()
        receipt = read_receipt(directory)
        assert receipt["status"] == "interrupted" and not receipt["ready_to_package"]
        assert receipt["native_console_url"] is None
    finally:
        if write_fd is not None:
            os.close(write_fd)
        os.close(read_fd)
        await service.shutdown()


def test_implementation_api_validates_requests_and_forwards_exact_build_scope(monkeypatch):
    from fastapi.testclient import TestClient
    from launchkit import server
    calls = []
    class Endpoint:
        async def submit(self, identity, version, request, *, expected_selected_build_id, refresh_framework):
            calls.append((identity, version, request, expected_selected_build_id, refresh_framework))
            return {"build_id": "build", "status": "queued"}
        async def file(self, identity, path, build_id):
            calls.append((identity, path, build_id))
            return {"path": path, "content": "source"}
        async def timeline(self, identity, after, limit, build_id):
            calls.append((identity, after, limit, build_id))
            return {"frames": []}
        async def package(self, identity, build_id):
            calls.append((identity, build_id))
            return b"archive", "sample"
    monkeypatch.setattr(server, "implementation", Endpoint())
    client = TestClient(server.app)
    assert client.post("/api/guide/setup/implementation", json={"version": 3, "request": "Add UI"}).status_code == 202
    assert calls[-1] == ("setup", 3, "Add UI", None, False)
    assert client.post("/api/guide/setup/implementation", json={
        "version": 3, "request": "Continue this source", "previous_build_id": "displayed-build"}).status_code == 202
    assert calls[-1] == ("setup", 3, "Continue this source", "displayed-build", False)
    for value in ({"version": True}, {"version": "3"}, {"version": 0}, {"version": 3, "request": "x" * 6001}, {"version": 3, "shell": "unsafe"}):
        assert client.post("/api/guide/setup/implementation", json=value).status_code == 422
    assert client.get("/api/guide/setup/implementation/files", params={"path": "application/a.py", "build_id": "exact"}).json()["content"] == "source"
    assert calls[-1] == ("setup", "application/a.py", "exact")
    assert client.get("/api/guide/setup/implementation/timeline", params={"after": "console:4", "limit": 20, "build_id": "exact"}).status_code == 200
    assert calls[-1] == ("setup", "console:4", 20, "exact")
    assert client.get("/api/guide/setup/implementation/timeline?limit=0").status_code == 422
    response = client.get("/api/guide/setup/implementation/package?build_id=exact")
    assert calls[-1] == ("setup", "exact") and response.content == b"archive"
    assert 'sample-application.zip' in response.headers["content-disposition"]


async def test_harmless_interview_turn_retains_exact_build_package_and_original_acceptance(service):
    first = await service.submit(service.guide.identity, 3)
    await complete(service, first)
    original = await service.status(service.guide.identity)
    directory = service._directory(service.guide.identity)
    frozen_bytes = (directory / "input.json").read_bytes()
    original_package, _ = await service.package(service.guide.identity, first["build_id"])
    service.guide.state["version"] = 4
    service.guide.state["messages"].append({"role": "user", "text": "What has been checked so far?"})
    current = await service.status(service.guide.identity)
    assert current["current"] and current["acceptance_current"] and current["ready_to_package"]
    assert current["guide_version"] == 3 and current["current_guide_version"] == 4
    assert current["acceptance"] == original["acceptance"]
    assert (await service.submit(service.guide.identity, 4))["build_id"] == first["build_id"]
    assert (directory / "input.json").read_bytes() == frozen_bytes
    payload, _ = await service.package(service.guide.identity, first["build_id"])
    assert payload == original_package
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        report = json.loads(archive.read("implementation-report.json"))
    assert report["guide_version"] == 3
    assert report["brief_hash"] == original["acceptance"]["brief_digest"]
    assert report["acceptance"] == original["acceptance"]
    # An explicit new implementation request is not a harmless status turn.
    second = await service.submit(service.guide.identity, 4, "Add a separate custom interface")
    assert second["build_id"] != first["build_id"] and not second["ready_to_package"]
    frozen = json.loads((service._directory(service.guide.identity) / "input.json").read_text())
    assert frozen["implementation_requests"] == ["Add a separate custom interface"]
    await service.shutdown()


@pytest.mark.parametrize("changed", ["brief", "proposal"])
async def test_changed_scope_during_acceptance_snapshot_cannot_remain_current(service, monkeypatch, changed):
    first = await service.submit(service.guide.identity, 3)
    await complete(service, first)
    from launchkit.acceptance import SemanticAcceptance
    original = SemanticAcceptance.verification
    def inspect(acceptance):
        result = original(acceptance)
        if changed == "brief":
            service.guide.state["brief"]["mission"] = "New requested recurrence and custom UI"
        else:
            service.guide.state["proposal"]["agent_classes"][0]["model"] = "gpt-6-sol"
        service.guide.state["version"] += 1
        return result
    monkeypatch.setattr(SemanticAcceptance, "verification", inspect)
    result = await service.status(service.guide.identity)
    assert not result["current"] and not result["acceptance_current"] and not result["ready_to_package"]
    assert result["current_guide_version"] is None
    await service.shutdown()


@pytest.mark.parametrize("legacy_flag", [False, True])
async def test_explicit_framework_refresh_retains_application_and_tests_without_old_grants(service, monkeypatch, legacy_flag):
    from launchkit.acceptance import SemanticAcceptance
    from launchkit.deployment_updates import compatibility
    first = await service.submit(service.guide.identity, 3, "Keep the original requested outcome.")
    old = await complete(service, first)
    directory = service._directory(service.guide.identity)
    if legacy_flag:
        for filename in ("input.json", "receipt.json"):
            data = json.loads((directory / filename).read_text())
            data.pop("refresh_framework")
            (directory / filename).write_text(json.dumps(data))
    old_files = {item.path: (item.content, item.mode) for item in old.export_files(require_verified=False)}
    old_input = (directory / "input.json").read_bytes()
    old_receipt = (directory / "receipt.json").read_bytes()
    old_digest = old.digest()
    old_key = old._key_path.read_bytes()
    frozen = json.loads(old_input)
    accepted = SemanticAcceptance(directory / "acceptance", old, frozen)
    old_tests = {name: accepted.read_test(name) for name in accepted.context()["acceptance_tests"]}
    (directory / "runtime-settings.json").write_text('{"synthetic_private_setting":"do not transfer"}')
    (directory / "native-state").mkdir()
    (directory / "native-state" / "history.txt").write_text("Original owned native history")
    newer_framework(monkeypatch, {"requirements.txt"})
    refreshed = await service.submit(service.guide.identity, 3, "Keep the original requested outcome.",
        refresh_framework=True, expected_selected_build_id=first["build_id"])
    assert refreshed["build_id"] != first["build_id"]
    new_directory = service._directory(service.guide.identity)
    new = workspace(service, refreshed)
    current_files = {item.path: (item.content, item.mode) for item in new.export_files(require_verified=False)}
    for name in ("app.py", "connectors.py", "remote_operations.py", "setup.py", "requirements.txt"):
        assert current_files[name][0] == old_files[name][0] + "\n# Refreshed compiler source.\n"
    assert current_files["application/__init__.py"] == old_files["application/__init__.py"]
    assert new._key_path.read_bytes() != old_key
    assert new.verification() is None
    assert refreshed["refresh_framework"] is True
    assert not refreshed["source_verified"] and not refreshed["acceptance_trusted"] and not refreshed["ready_to_package"]
    new_input = json.loads((new_directory / "input.json").read_text())
    assert new_input["refresh_framework"] is True
    assert new_input["framework"] == refreshed["framework"] == new.framework_provenance()
    assert new_input["framework"]["basis"] == "current-compiler"
    assert new_input["framework"]["reason"] == "explicit-framework-refresh"
    assert new_input["framework"]["origin"] is None and new_input["framework"]["previous"] is None
    assert new_input["framework"]["baseline_digest"] == new_input["framework"]["current_compiler_baseline_digest"]
    assert new_input["previous_implementation"]["build_id"] == first["build_id"]
    assert new_input["previous_implementation"]["diagnostics"]["source_digest"] == old_digest
    assert new_input["implementation_requests"] == ["Keep the original requested outcome."]
    retained = SemanticAcceptance(new_directory / "acceptance", new, new_input)
    assert {name: retained.read_test(name) for name in retained.context()["acceptance_tests"]} == old_tests
    assert retained.verification() is None
    assert retained.history()[0]["execution"] is None
    assert retained.history()[0]["publication"] is None
    assert retained.history()[0]["native_completion"] is None
    assert not (new_directory / "runtime-settings.json").exists()
    assert not (new_directory / "native-state").exists()
    assert (directory / "input.json").read_bytes() == old_input
    assert (directory / "receipt.json").read_bytes() == old_receipt
    assert old.digest() == old_digest
    _, blockers = compatibility(
        {name: (content.encode(), mode) for name, (content, mode) in old_files.items()},
        {name: (content.encode(), mode) for name, (content, mode) in current_files.items()},
        {"execution_mode": "interactive"})
    assert any("app.py" in blocker and "separate verified upgrade path" in blocker for blocker in blockers)
    await service.shutdown()


async def test_refresh_flag_differentiates_active_and_completed_admission(service, monkeypatch):
    first = await service.submit(service.guide.identity, 3, "Keep the requested behavior.")
    with pytest.raises(DraftConflict, match="already building"):
        await service.submit(service.guide.identity, 3, "Keep the requested behavior.", refresh_framework=True)
    assert (await service.submit(service.guide.identity, 3, "Keep the requested behavior."))["build_id"] == first["build_id"]
    await complete(service, first)
    newer_framework(monkeypatch)
    second = await service.submit(service.guide.identity, 3, "Keep the requested behavior.", refresh_framework=True)
    assert second["build_id"] != first["build_id"]
    assert (await service.submit(service.guide.identity, 3, "Keep the requested behavior.", refresh_framework=True))["build_id"] == second["build_id"]
    with pytest.raises(DraftConflict, match="already building"):
        await service.submit(service.guide.identity, 3, "Keep the requested behavior.")
    await complete(service, second)
    assert (await service.submit(service.guide.identity, 3, "Keep the requested behavior.", refresh_framework=True))["build_id"] == second["build_id"]
    third = await service.submit(service.guide.identity, 3, "Keep the requested behavior.")
    assert third["build_id"] != second["build_id"]
    assert third["refresh_framework"] is False
    assert workspace(service, third).read_file("app.py") == workspace(service, second).read_file("app.py")
    assert third["framework"]["basis"] == "accepted-baseline"
    await service.shutdown()


@pytest.mark.parametrize("value", [None, 0, 1, "false", "true", {}, []])
async def test_framework_refresh_service_rejects_non_boolean_before_admission(service, value):
    with pytest.raises(ValueError, match="boolean value"):
        await service.submit(service.guide.identity, 3, refresh_framework=value)
    assert not service.tasks and not list(service.root.glob("*/*/receipt.json"))
    await service.shutdown()


@pytest.mark.parametrize("change", ["version", "selection", "source", "trial-proof"])
async def test_framework_refresh_keeps_guide_selection_source_and_trial_admission_fences(service, monkeypatch, change):
    from launchkit.templates.storage_setup import write_private
    first = await service.submit(service.guide.identity, 3)
    old = await complete(service, first)
    directory = service._directory(service.guide.identity)
    checked = await service.status(service.guide.identity)
    other = None
    if change == "selection":
        other = await service.submit(service.guide.identity, 3, "An already saved alternate build.")
        await complete(service, other)
        write_private(directory.parent / "latest.json", {"build_id": first["build_id"]})
    reached, release = pause_build_preparation(monkeypatch, "list_files")
    options = {"refresh_framework": True, "expected_selected_build_id": first["build_id"]}
    if change == "trial-proof":
        options.update(expected_previous_build_id=first["build_id"], expected_source_digest=checked["source_digest"])
    admission = asyncio.create_task(service.submit(service.guide.identity, 3, "Refresh this framework", **options))
    try:
        await reach_preparation(reached, admission)
        if change == "version":
            service.guide.state["version"] += 1
        elif change == "selection":
            write_private(directory.parent / "latest.json", {"build_id": other["build_id"]})
        elif change == "source":
            old.write_file("application/later.py", "LATER = True\n")
        else:
            host_receipt(old, "fail", 1)
        release.set()
        with pytest.raises(DraftConflict, match="changed"):
            await admission
        assert not service.tasks
        assert len(list(service.root.glob("*/*/receipt.json"))) == (2 if other else 1)
    finally:
        release.set()
        await asyncio.gather(admission, return_exceptions=True)
        await service.shutdown()


@pytest.mark.parametrize("cleanup", ["cleanup_pending", "cleanup_in_progress"])
async def test_framework_refresh_does_not_bypass_prior_owned_cleanup(service, cleanup):
    first = await service.submit(service.guide.identity, 3)
    await complete(service, first)
    directory = service._directory(service.guide.identity)
    update_receipt(directory, **{cleanup: True})
    before = (directory / "receipt.json").read_bytes()
    with pytest.raises(DraftConflict, match="cleanup"):
        await service.submit(service.guide.identity, 3, refresh_framework=True,
            expected_selected_build_id=first["build_id"])
    assert not service.tasks
    assert len(list(service.root.glob("*/*/receipt.json"))) == 1
    assert (directory / "receipt.json").read_bytes() == before
    await service.shutdown()


async def test_organization_reaches_builder_and_reviewer_from_saved_guide_only(service, monkeypatch):
    from launchkit.acceptance import SemanticAcceptance
    from launchkit.implementation_workspace import WorkspaceError
    organization = {"schema_version": 1, "id": "workplace", "version": "1.0",
        "label": "Example workplace", "description": "", "starting_examples": [],
        "guide_context": "Use the selected private identity package; keep its missing contract explicit."}
    service.guide.state["organization"] = deepcopy(organization)
    monkeypatch.setattr("launchkit.organization.load_organization_plugin",
        lambda *a, **kw: pytest.fail("Build must not reload the installed plugin"))
    try:
        admitted = await service.submit(service.guide.identity, 3, "Build this workplace tool")
        directory = service._directory(service.guide.identity)
        frozen = json.loads((directory / "input.json").read_text())
        assert admitted["current"] and admitted["organization_hash"] == fingerprint(organization)
        assert frozen["organization"] == organization
        tools = BuildTools(directory)
        assert (await tools.context({"view": "full"}))["organization"] == organization
        projected = await tools.context({})
        projected["organization"]["guide_context"] = "Changed projection"
        assert tools.input["organization"] == organization
        reviewer = SemanticAcceptance(directory / "acceptance", tools.workspace, frozen)
        reviewer.begin_round()
        assert reviewer.context()["organization"] == organization
        reviewer.context()["organization"]["guide_context"] = "Changed reviewer projection"
        assert reviewer.context()["organization"] == organization
        changed = deepcopy(frozen)
        changed["organization"]["guide_context"] = "Different identity constraints"
        with pytest.raises(WorkspaceError):
            SemanticAcceptance(directory / "acceptance", tools.workspace, changed)
        service.guide.state["organization"]["version"] = "2.0"
        assert not (await service.status(service.guide.identity))["current"]
        assert tools.input["organization"] == organization
        assert json.loads((directory / "input.json").read_text())["organization"] == organization
    finally:
        await service.shutdown()


async def test_organization_change_during_preparation_refuses_new_admission(service, monkeypatch):
    service.guide.state["organization"] = {"schema_version": 1, "id": "workplace", "version": "1.0",
        "label": "Example workplace", "description": "", "starting_examples": [], "guide_context": "Original context"}
    reached, release = pause_build_preparation(monkeypatch, "seed")
    admission = asyncio.create_task(service.submit(service.guide.identity, 3, "Prepare the application"))
    try:
        await reach_preparation(reached, admission)
        service.guide.state["organization"]["guide_context"] = "Different identity constraints"
        release.set()
        with pytest.raises(DraftConflict, match="changed while preparing"):
            await admission
        assert not service.tasks and not service.processes
        assert not (service.root / service.guide.identity / "latest.json").exists()
    finally:
        release.set()
        await asyncio.gather(admission, return_exceptions=True)
        await service.shutdown()


async def test_organization_bound_application_keeps_framework_export_and_recovery(service):
    service.guide.state["organization"] = {"schema_version": 1, "id": "workplace", "version": "1.0",
        "label": "Example workplace", "description": "", "starting_examples": [], "guide_context": "Workplace identity context"}
    try:
        first = await service.submit(service.guide.identity, 3, "Keep workplace identity")
        project = await complete(service, first)
        status = await service.status(service.guide.identity)
        assert status["current"] and status["ready_to_package"]
        baseline = service._accepted_framework(service.guide.identity, first["build_id"], project.digest(),
            service.guide.state, service.guide.state["proposal"])
        assert baseline.previous_build_id == first["build_id"]
        selection_path = service._directory(service.guide.identity).parent / "latest.json"
        selection_bytes = selection_path.read_bytes()
        selection_path.unlink()
        candidates = await service.saved_selection(service.guide.identity)
        assert [row["build_id"] for row in candidates["candidates"]] == [first["build_id"]]
        selection_path.write_bytes(selection_bytes)
        data, _ = await service.package(service.guide.identity, first["build_id"])
        with zipfile.ZipFile(io.BytesIO(data)) as package:
            report = json.loads(package.read("implementation-report.json"))
        assert report["organization_hash"] == fingerprint(service.guide.state["organization"])
        assert (await service.submit(service.guide.identity, 3, "Keep workplace identity"))["build_id"] == first["build_id"]
    finally:
        await service.shutdown()


async def test_package_hashing_and_compression_leave_the_event_loop_free(service, monkeypatch):
    status = await service.submit(service.guide.identity, 3)
    await complete(service, status)
    original = zipfile.ZipFile.writestr
    def slowed(archive, info, *args, **kwargs):
        if getattr(info, "filename", info) == "SHA256SUMS":
            time.sleep(1.0)  # A large package on a small host.
        return original(archive, info, *args, **kwargs)
    monkeypatch.setattr(zipfile.ZipFile, "writestr", slowed)
    stalls, done = [], asyncio.Event()
    async def other_request():
        while not done.is_set():
            started = time.monotonic()
            await asyncio.sleep(0.01)
            stalls.append(time.monotonic() - started)
    probe = asyncio.create_task(other_request())
    await asyncio.sleep(0)  # The other request is waiting on the loop.
    try:
        payload, _ = await service.package(service.guide.identity, status["build_id"])
    finally:
        done.set()
        await probe
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        assert "SHA256SUMS" in archive.namelist()
    assert stalls and max(stalls) < 0.5


def held_workspace_lock(service, status, seconds):
    """Hold this build's workspace lock as a long writer would, then release it."""
    import fcntl
    import os
    path = workspace(service, status).root / "workspace.lock"
    descriptor = os.open(path, os.O_RDWR)
    fcntl.flock(descriptor, fcntl.LOCK_EX)
    release = threading.Timer(seconds, os.close, (descriptor,))
    release.start()
    return release


@pytest.mark.parametrize("reader", ["status", "selected-runtime"])
async def test_server_reads_refuse_a_busy_workspace_within_their_bound(service, monkeypatch, reader):
    # A browser poll or an install check must not wait without bound for a
    # workspace lock: abandoned polls would queue up in the thread pool.
    from launchkit import implementation as implementation_module
    status = await service.submit(service.guide.identity, 3)
    await complete(service, status)
    status = await service.status(service.guide.identity)
    monkeypatch.setattr(implementation_module, "WORKSPACE_READ_LOCK_TIMEOUT", 0.3, raising=False)
    release = held_workspace_lock(service, status, 3.0)
    started = time.monotonic()
    try:
        with pytest.raises(DraftConflict, match="busy"):
            if reader == "status":
                await service.status(service.guide.identity)
            else:
                await asyncio.to_thread(service.selected_runtime_target, service.guide.identity,
                    build_id=status["build_id"], source_digest=status["source_digest"])
        assert time.monotonic() - started < 2.0
    finally:
        release.join()
    # The same read succeeds once the lock is free again.
    assert (await service.status(service.guide.identity))["build_id"] == status["build_id"]
