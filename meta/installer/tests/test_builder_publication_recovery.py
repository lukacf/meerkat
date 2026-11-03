"""Offline orchestration fixtures, never native or authored application execution."""
import asyncio
from types import SimpleNamespace

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.artifact_binding import proposal_hash
from launchkit.implementation import read_receipt, update_receipt
from launchkit.implementation_evidence import project_implementation_evidence
from launchkit import implementation_worker as worker
from launchkit.implementation_worker import (BuildTools, MissingBuilderPublication,
    ensure_builder_publication, review_and_repair)
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.templates.storage_setup import write_private


@pytest.fixture
def tools(tmp_path):
    frozen = {"blueprint": starter_blueprint().model_dump(mode="json"), "brief": {"requirements": []}}
    write_private(tmp_path / "input.json", frozen)
    write_private(tmp_path / "receipt.json", {"status": "running", "progress": [], "ready_to_package": False})
    workspace = ImplementationWorkspace(tmp_path / "workspace", frozen["blueprint"])
    workspace.seed()
    workspace.write_file("application/__init__.py", "def build(context):\n    return object()\n")
    return BuildTools(tmp_path)


def check_fixture(tools):
    """A signed host check fixture; no generated source or sandbox is executed."""
    with tools.workspace._locked():
        state = tools.workspace._state()
        receipt = {"status": "pass", "tests_run": 1, "ready_to_package": True,
                   "digest": tools.workspace._digest(tools.workspace._files(state)), "checks": []}
        receipt["signature"] = tools.workspace._signed(receipt)
        state["last_check"] = receipt
        tools.workspace._save_state(state)
    return receipt


async def publish_fixture(tools):
    check_fixture(tools)
    result = await tools.publish({"summary": "Offline host check fixture only", "unresolved": []})
    assert result["ok"], result
    return tools.publication


def complete(identity="prior-builder", cursor="completion-cursor"):
    return {"interaction_id": identity, "native_completion": {
        "kind": "interaction_complete", "interaction_id": identity, "cursor": cursor}}


@pytest.mark.asyncio
async def test_current_checked_publication_has_no_extra_turn(tools):
    expected = await publish_fixture(tools)
    async def invoke(*args):
        pytest.fail("A current checked publication does not need another builder turn.")
    assert await ensure_builder_publication(tools.directory, tools, complete(), invoke,
                                            builder_phase="initial_build") == expected
    assert "builder_publication_retries" not in read_receipt(tools.directory)


@pytest.mark.asyncio
@pytest.mark.parametrize("phase", ["initial_build", "automatic_repair"])
async def test_missing_publication_requests_one_turn_and_retains_failure_source(tools, phase):
    before = tools.workspace.digest()
    calls = []
    async def invoke(identity, content, key, admitted):
        receipt = read_receipt(tools.directory)
        assert receipt["builder_publication_retries"] == 1
        assert receipt["builder_publication_failures"][0]["source_digest"] == before
        assert receipt["ready_to_package"] is False
        calls.append((identity, content, key, admitted))
        assert "prior-builder" in content and before in content
        assert "read_acceptance_feedback" in content and "check_implementation" in content
        assert "publish_implementation" in content and "do not relax" in content
        await publish_fixture(tools)
        return complete("recovery-builder")
    result = await ensure_builder_publication(tools.directory, tools, complete(), invoke, builder_phase=phase)
    assert result["checked_digest"] == before
    assert len(calls) == 1 and calls[0][0] == "implementation:main" and calls[0][3] is None
    receipt = read_receipt(tools.directory)
    assert receipt["builder_publication_retries"] == 1 and receipt["ready_to_package"] is False
    assert receipt["builder_publication_failures"][0]["interaction_id"] == "prior-builder"
    assert receipt["builder_publication_failures"][0]["builder_phase"] == phase
    assert receipt["builder_publication_failures"][0]["reason"] == "completed_without_current_publication"
    assert receipt["builder_publication_recovery_completion"]["interaction_id"] == "recovery-builder"
    assert not receipt.get("semantic_acceptance")


@pytest.mark.asyncio
async def test_stale_check_and_publication_never_pass_after_recovery_prose(tools):
    await publish_fixture(tools)
    old = tools.workspace.digest()
    tools.workspace.write_file("application/change.py", "CHANGED = True\n")
    current = tools.workspace.digest()
    calls = []
    async def invoke(*args):
        calls.append(args)
        rejected = await tools.publish({"summary": "Unproven recovery prose", "unresolved": []})
        assert rejected["ok"] is False
        return complete("recovery-without-current-check")
    with pytest.raises(MissingBuilderPublication):
        await ensure_builder_publication(tools.directory, tools, complete(), invoke, builder_phase="initial_build")
    assert old != current and len(calls) == 1
    receipt = read_receipt(tools.directory)
    assert receipt["builder_publication_failures"][0]["source_digest"] == current
    assert receipt["ready_to_package"] is False and not receipt.get("semantic_acceptance")


@pytest.mark.asyncio
async def test_initial_recovery_spends_budget_for_later_repair_and_reconstruction(tools):
    calls = []
    async def invoke(*args):
        calls.append(args)
        await publish_fixture(tools)
        return complete("first-recovery")
    await ensure_builder_publication(tools.directory, tools, complete(), invoke, builder_phase="initial_build")
    tools = BuildTools(tools.directory)
    with pytest.raises(MissingBuilderPublication):
        await ensure_builder_publication(tools.directory, tools, complete("later-repair"), invoke,
                                         builder_phase="automatic_repair")
    receipt = read_receipt(tools.directory)
    assert len(calls) == 1 and receipt["builder_publication_retries"] == 1
    assert [row["retry_requested"] for row in receipt["builder_publication_failures"]] == [True, False]
    assert receipt["builder_publication_failures"][-1]["interaction_id"] == "later-repair"


@pytest.mark.asyncio
@pytest.mark.parametrize("change", ["missing", "wrong-kind", "wrong-id", "empty-cursor",
    "numeric-cursor", "failed", "error", "invalid-id"])
async def test_invalid_or_failed_completion_cannot_admit_recovery(tools, change):
    completed = complete()
    if change == "missing":
        completed = None
    elif change == "invalid-id":
        completed["interaction_id"] = {}
    else:
        key, value = {"wrong-kind": ("kind", "run_completed"), "wrong-id": ("interaction_id", "foreign"),
            "empty-cursor": ("cursor", ""), "numeric-cursor": ("cursor", 5),
            "failed": ("status", "failed"), "error": ("is_error", True)}[change]
        completed["native_completion"][key] = value
    async def invoke(*args):
        pytest.fail("Invalid completion must not request an extra turn.")
    with pytest.raises(RuntimeError, match="matching normal native completion"):
        await ensure_builder_publication(tools.directory, tools, completed, invoke, builder_phase="initial_build")
    assert "builder_publication_retries" not in read_receipt(tools.directory)


@pytest.mark.asyncio
@pytest.mark.parametrize("error", [RuntimeError("Synthetic transport failure"), asyncio.CancelledError()])
async def test_transport_failure_and_cancellation_propagate_without_another_turn(tools, error):
    calls = []
    async def invoke(*args):
        calls.append(args)
        raise error
    with pytest.raises(type(error)) as caught:
        await ensure_builder_publication(tools.directory, tools, complete(), invoke, builder_phase="initial_build")
    assert caught.value is error and len(calls) == 1
    async def no_second(*args):
        pytest.fail("An uncertain extra turn consumes the whole-build budget.")
    with pytest.raises(MissingBuilderPublication):
        await ensure_builder_publication(tools.directory, BuildTools(tools.directory), complete("later"),
                                         no_second, builder_phase="automatic_repair")


@pytest.mark.asyncio
async def test_recovery_requires_its_own_matching_completion_even_if_it_publishes(tools):
    async def invoke(*args):
        await publish_fixture(tools)
        result = complete("recovery")
        result["native_completion"]["interaction_id"] = "foreign"
        return result
    with pytest.raises(RuntimeError, match="matching normal native completion"):
        await ensure_builder_publication(tools.directory, tools, complete(), invoke, builder_phase="initial_build")
    assert read_receipt(tools.directory)["ready_to_package"] is False


@pytest.mark.asyncio
async def test_unrelated_source_error_is_not_a_missing_publication_retry(tools, monkeypatch):
    def unavailable():
        raise RuntimeError("Synthetic source custody unavailable")
    monkeypatch.setattr(tools.workspace, "verification", unavailable)
    async def invoke(*args):
        pytest.fail("Source custody failure is not a publication recovery admission.")
    with pytest.raises(RuntimeError, match="source custody unavailable"):
        await ensure_builder_publication(tools.directory, tools, complete(), invoke, builder_phase="initial_build")


@pytest.mark.asyncio
async def test_automatic_repair_missing_publication_recovers_before_fresh_review(tools):
    await publish_fixture(tools)
    rounds, calls, seals = [], [], []
    reviewer = SimpleNamespace(publication=None)
    def begin_round():
        row = {"number": len(rounds) + 1, "source_digest": tools.workspace.digest()}
        rounds.append(row)
        return row
    def verification():
        return {"status": "needs_repair" if len(rounds) == 1 else "pass",
            "ready_to_package": len(rounds) == 2, "summary": "Offline reviewer fixture",
            "counterexamples": ["Synthetic behavioral defect"], "requirements": [], "execution": {}}
    reviewer.acceptance = SimpleNamespace(begin_round=begin_round, bind_run=lambda identity: None,
        complete_run=lambda identity, evidence: seals.append((identity, evidence)), verification=verification)
    async def invoke(identity, prompt, key, admitted):
        calls.append((identity, key))
        if identity == "acceptance:main":
            assert tools.publication and tools.publication["checked_digest"] == tools.workspace.digest()
            reviewer.publication = {"scope": "offline reviewer fixture"}
        elif key == "repair-1":
            assert tools.independent_feedback["status"] == "needs_repair"
            tools.workspace.write_file("application/change.py", "REPAIRED = True\n")
        else:
            assert key == "builder-publication-recovery-1"
            assert tools.independent_feedback["status"] == "needs_repair"
            await publish_fixture(tools)
        return complete("native-" + key)
    result = await review_and_repair(tools.directory, tools, reviewer, invoke)
    assert result["status"] == "pass" and len(seals) == 2
    assert calls == [("acceptance:main", "acceptance-1"), ("implementation:main", "repair-1"),
        ("implementation:main", "builder-publication-recovery-1"), ("acceptance:main", "acceptance-2")]
    assert rounds[0]["source_digest"] != rounds[1]["source_digest"]
    assert read_receipt(tools.directory)["builder_publication_failures"][0]["builder_phase"] == "automatic_repair"


@pytest.mark.asyncio
@pytest.mark.parametrize("builder_phase,expected_phase", [
    ("initial_build", "implementation"), ("automatic_repair", "automatic_repair")])
async def test_recovery_keeps_current_host_lifecycle_and_cleaned_continuation(tools, builder_phase, expected_phase):
    """Actual host projection of a labelled offline lifecycle snapshot."""
    state = {"id": "synthetic-guide", "version": 1,
             "proposal": tools.input["blueprint"], "brief": tools.input["brief"]}
    update_receipt(tools.directory, guide_id=state["id"], build_id="synthetic-build",
        guide_version=1, current_guide_version=1, current=True,
        blueprint_hash=proposal_hash(state["proposal"]), source_digest=tools.workspace.digest(),
        source_verified=False, acceptance_trusted=False, acceptance_current=False,
        stop_requested=False, cleanup_pending=False, cleanup_in_progress=False,
        cleanup_custody_lost=False, worker_stop_confirmed=False)
    calls = []
    async def invoke(*args):
        calls.append(args)
        projected = project_implementation_evidence(state, read_receipt(tools.directory))
        lifecycle = projected["lifecycle"]
        assert lifecycle["availability"] == "available"
        assert lifecycle["phase"] == expected_phase and lifecycle["content_current"] is True
        assert lifecycle["stop_available"] is True and lifecycle["resume_available"] is False
        assert lifecycle["application_trial_gate_clear"] is False
        return complete("completed-without-publication")
    with pytest.raises(MissingBuilderPublication):
        await ensure_builder_publication(tools.directory, tools, complete(), invoke,
                                         builder_phase=builder_phase)
    assert len(calls) == 1
    # This is a synthetic terminal host snapshot, not a process-cleanup claim.
    update_receipt(tools.directory, status="failed", worker_stop_confirmed=True)
    projected = project_implementation_evidence(state, read_receipt(tools.directory))
    lifecycle = projected["lifecycle"]
    assert lifecycle["availability"] == "available" and lifecycle["phase"] == expected_phase
    assert lifecycle["build_gate_clear"] is True and lifecycle["resume_available"] is True
    assert lifecycle["application_trial_gate_clear"] is False
    assert projected["acceptance_current"] is False


@pytest.mark.asyncio
async def test_known_stop_before_native_turn_admits_no_post(tools, monkeypatch):
    stop = asyncio.Event()
    stop.set()
    requests = []
    async def unexpected_request(*args, **kwargs):
        requests.append((args, kwargs))
        pytest.fail("A known stop must be fenced before native admission.")
    monkeypatch.setattr(worker, "_native_request", unexpected_request)
    with pytest.raises(asyncio.CancelledError):
        await worker.native_turn(None, "http://offline.invalid", tools.directory, stop,
                                 "implementation:main", "Synthetic task", "synthetic-key")
    assert requests == []
    assert not read_receipt(tools.directory).get("native_turns")
