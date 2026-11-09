"""Synthetic native orchestration only; no provider or application execution."""
import asyncio
from copy import deepcopy
from pathlib import Path

import pytest

from launchkit.acceptance import SemanticAcceptance, _json, _sha
from launchkit.acceptance_reviewer import AcceptanceTools
from launchkit.blueprint import starter_blueprint
from launchkit.implementation import read_receipt
from launchkit.implementation_worker import BuildTools, MissingBuilderPublication, review_and_repair
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.templates.storage_setup import write_private
from launchkit import implementation_worker as worker


@pytest.fixture
def team(tmp_path):
    assert Path(worker.__file__).resolve() == Path(__file__).resolve().parents[1] / "launchkit/implementation_worker.py"
    frozen = {"blueprint": starter_blueprint().model_dump(mode="json"),
              "brief": {"requirements": [{"id": "requested-behavior", "outcome": "Deliver the requested behavior"}]}}
    write_private(tmp_path / "input.json", frozen)
    write_private(tmp_path / "receipt.json", {"status": "running", "progress": [], "ready_to_package": False})
    workspace = ImplementationWorkspace(tmp_path / "workspace", frozen["blueprint"])
    workspace.seed()
    workspace.write_file("application/__init__.py", "def requested_behavior():\n    return None\n")
    tools = BuildTools(tmp_path)
    acceptance = SemanticAcceptance(tmp_path / "acceptance", tools.workspace, frozen)
    tools.acceptance = acceptance
    return tools, AcceptanceTools(tmp_path, acceptance, tools.reference)


def completion(key):
    interaction = "synthetic-" + key
    return {"interaction_id": interaction, "native_completion": {
        "kind": "interaction_complete", "interaction_id": interaction, "cursor": "synthetic-cursor"}}


def checked_publication(tools):
    # Authentic host check binding with substituted execution, never app code.
    with tools.workspace._locked():
        state = tools.workspace._state()
        checked = {"status": "pass", "tests_run": 1, "ready_to_package": True,
                   "digest": tools.workspace._digest(tools.workspace._files(state)), "checks": []}
        checked["signature"] = tools.workspace._signed(checked)
        state["last_check"] = checked
        tools.workspace._save_state(state)
    tools.publication = {"summary": "Synthetic host execution only", "unresolved": [],
                         "checked_digest": checked["digest"], "receipt": checked}


def reviewer_publication(reviewer, *, failed=True):
    acceptance = reviewer.acceptance
    if not acceptance.context()["acceptance_tests"]:
        acceptance.write_test("test_requested_behavior.py", "import unittest\nfrom application import requested_behavior\nclass RequestedBehavior(unittest.TestCase):\n    def test_requested_behavior(self):\n        self.assertEqual(requested_behavior(), 'requested')\n")
    # Substitute execution only; publication, completion sealing and current
    # authenticated verification use the real shipping acceptance authority.
    with acceptance._locked():
        state = acceptance._state()
        row = acceptance._round(state)
        test_id = "test_requested_behavior.py::RequestedBehavior.test_requested_behavior"
        execution = {"id": "synthetic-execution", "source_digest": row["source_digest"],
                     "test_digest": _sha(_json(acceptance._tests(row))), "status": "pass",
                     "tests_run": 1, "test_results": [{"id": test_id, "file": "test_requested_behavior.py", "status": "pass"}],
                     "detail": "Synthetic orchestration fixture; application not executed."}
        execution["signature"] = acceptance._signed(execution)
        row["execution"] = execution
        acceptance._save(state)
    reviewer.publication = acceptance.publish({"summary": "Synthetic independent findings",
        "requirements": [{"id": "requested-behavior", "status": "tested", "source_paths": ["application/__init__.py"],
            "test_ids": [test_id], "evidence": "Synthetic host execution binding only.", "remaining_dependencies": []}],
        "counterexamples": ["Requested behavior remains missing."] if failed else [], "remaining_dependencies": []})


@pytest.mark.asyncio
async def test_unchanged_checked_repair_retains_exact_failed_review_and_protected_tests(team):
    tools, reviewer = team
    checked_publication(tools)
    calls, before = [], {}
    async def invoke(identity, prompt, key, admitted):
        calls.append((identity, key))
        if admitted:
            admitted(completion(key)["interaction_id"])
        if identity == "acceptance:main":
            reviewer_publication(reviewer)
        else:
            before["assessment"] = reviewer.acceptance.verification()
            before["tests"] = reviewer.acceptance.context()["acceptance_tests"]
            checked_publication(tools)
        return completion(key)
    with pytest.raises(RuntimeError, match="made no source change"):
        await review_and_repair(tools.directory, tools, reviewer, invoke)
    saved = read_receipt(tools.directory)
    assert calls == [("acceptance:main", "acceptance-1"), ("implementation:main", "repair-1")]
    assert len(reviewer.acceptance.history()) == 1
    assert saved["semantic_acceptance"] == before["assessment"] == reviewer.acceptance.verification()
    assert reviewer.acceptance.context()["acceptance_tests"] == before["tests"]
    assert saved["status"] == "failed" and saved["phase"] == "automatic_repair" and saved["ready_to_package"] is False
    halt = saved["repair_halt"]
    assert halt["reason"] == "unchanged_source_after_automatic_repair"
    assert halt["source_digest"] == tools.workspace.digest() == before["assessment"]["source_digest"]
    assert halt["native_completion"] == completion("repair-1")["native_completion"]
    assert halt["repair_native_completion"] == completion("repair-1")["native_completion"]
    assert saved["semantic_acceptance"]["status"] == "needs_repair"


@pytest.mark.asyncio
async def test_publication_recovery_then_unchanged_halt_binds_both_normal_completions(team):
    tools, reviewer = team
    checked_publication(tools)
    calls = []
    async def invoke(identity, prompt, key, admitted):
        calls.append((identity, key))
        if admitted:
            admitted(completion(key)["interaction_id"])
        if identity == "acceptance:main":
            reviewer_publication(reviewer)
        elif key == "builder-publication-recovery-1":
            checked_publication(tools)
        return completion(key)
    with pytest.raises(RuntimeError, match="made no source change"):
        await review_and_repair(tools.directory, tools, reviewer, invoke)
    assert calls == [("acceptance:main", "acceptance-1"), ("implementation:main", "repair-1"),
                     ("implementation:main", "builder-publication-recovery-1")]
    saved = read_receipt(tools.directory)
    assert saved["repair_halt"]["repair_native_completion"] == completion("repair-1")["native_completion"]
    assert saved["repair_halt"]["native_completion"] == completion("builder-publication-recovery-1")["native_completion"]
    assert saved["semantic_acceptance"] == reviewer.acceptance.verification()
    assert saved["semantic_acceptance"]["status"] == "needs_repair" and saved["ready_to_package"] is False
    assert len(reviewer.acceptance.history()) == 1


@pytest.mark.asyncio
async def test_changed_checked_repair_still_requires_second_review(team):
    tools, reviewer = team
    checked_publication(tools)
    calls = []
    async def invoke(identity, prompt, key, admitted):
        calls.append((identity, key))
        if admitted:
            admitted(completion(key)["interaction_id"])
        if identity == "acceptance:main":
            reviewer_publication(reviewer, failed=key == "acceptance-1")
        else:
            tools.workspace.write_file("application/__init__.py", "def requested_behavior():\n    return 'requested'\n")
            checked_publication(tools)
        return completion(key)
    result = await review_and_repair(tools.directory, tools, reviewer, invoke)
    assert calls == [("acceptance:main", "acceptance-1"), ("implementation:main", "repair-1"), ("acceptance:main", "acceptance-2")]
    assert result["status"] == "pass" and result == reviewer.acceptance.verification()
    assert reviewer.acceptance.history()[0]["source_digest"] != result["source_digest"]
    assert "repair_halt" not in read_receipt(tools.directory)


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["unavailable", "prose", "source_digest", "blueprint_digest", "brief_digest",
    "binding_digest", "policy", "boolean-policy", "missing-review-round", "not-failed"])
async def test_unknown_or_malformed_current_binding_never_skips_fresh_review(team, monkeypatch, change):
    tools, reviewer = team
    checked_publication(tools)
    calls, mutate_next = [], [False]
    actual_verification = reviewer.acceptance.verification
    def verification():
        current = actual_verification()
        if not mutate_next[0]:
            return current
        mutate_next[0] = False
        if change == "unavailable":
            return None
        if change == "prose":
            return "The source is the same; please stop."
        current = deepcopy(current)
        if change.endswith("digest"):
            current[change] = "f" * 64
        elif change == "policy":
            current["acceptance_policy_version"] = 2
        elif change == "boolean-policy":
            current["acceptance_policy_version"] = True
        elif change == "missing-review-round":
            current.pop("review_round")
        else:
            current["status"] = "pass"
        return current
    monkeypatch.setattr(reviewer.acceptance, "verification", verification)
    async def invoke(identity, prompt, key, admitted):
        calls.append((identity, key))
        if admitted:
            admitted(completion(key)["interaction_id"])
        if identity == "acceptance:main":
            reviewer_publication(reviewer)
        else:
            checked_publication(tools)
            mutate_next[0] = True
        return completion(key)
    with pytest.raises(RuntimeError, match="automatic repair budget"):
        await review_and_repair(tools.directory, tools, reviewer, invoke, max_reviews=2)
    assert calls == [("acceptance:main", "acceptance-1"), ("implementation:main", "repair-1"), ("acceptance:main", "acceptance-2")]
    saved = read_receipt(tools.directory)
    assert "repair_halt" not in saved and saved["ready_to_package"] is False
    assert saved["semantic_acceptance"]["status"] == "needs_repair"


@pytest.mark.asyncio
@pytest.mark.parametrize("problem", ["failed-completion", "malformed-completion", "missing-publication", "transport", "cancel"])
async def test_completion_publication_and_cancellation_gates_precede_halt(team, problem):
    tools, reviewer = team
    checked_publication(tools)
    calls = []
    async def invoke(identity, prompt, key, admitted):
        calls.append((identity, key))
        if admitted:
            admitted(completion(key)["interaction_id"])
        if identity == "acceptance:main":
            reviewer_publication(reviewer)
        elif problem == "transport":
            raise RuntimeError("Synthetic provider transport failure")
        elif problem == "cancel":
            raise asyncio.CancelledError
        elif problem != "missing-publication":
            checked_publication(tools)
        result = completion(key)
        if identity == "implementation:main" and problem == "failed-completion":
            result["native_completion"]["status"] = "failed"
        if identity == "implementation:main" and problem == "malformed-completion":
            result["native_completion"]["cursor"] = None
        return result
    error = asyncio.CancelledError if problem == "cancel" else MissingBuilderPublication if problem == "missing-publication" else RuntimeError
    with pytest.raises(error):
        await review_and_repair(tools.directory, tools, reviewer, invoke)
    saved = read_receipt(tools.directory)
    assert "repair_halt" not in saved and saved["ready_to_package"] is False
    assert saved["semantic_acceptance"]["status"] == "needs_repair"
    assert len(reviewer.acceptance.history()) == 1
    assert len(calls) == (3 if problem == "missing-publication" else 2)
