"""An unchanged runtime refresh earns fresh evidence and cannot author repairs."""

from copy import deepcopy
import pytest

from launchkit.implementation import read_receipt
from launchkit.implementation_worker import implement_and_review, runtime_refresh_source
from launchkit.templates.storage_setup import write_private
import launchkit.implementation_worker as worker

from test_implementation_worker import (builder_check_fixture, completed_review,
    publish_checked_normalization, require_sandbox, team)


def bind_refresh(tools):
    predecessor = {"build_id": "accepted-predecessor", "source_digest": "a" * 64}
    with tools.workspace._locked():
        state = tools.workspace._state()
        framework = state["framework"]
        framework.update(basis="runtime-refresh", reason="explicit-runtime-refresh", previous=predecessor)
        framework.pop("signature")
        framework["signature"] = tools.workspace._signed(framework)
        tools.workspace._save_state(state)
    tools.input.update(preserve_design=True, refresh_framework=True, request="",
        runtime_refresh_source_digest=tools.workspace.digest(), runtime_refresh_predecessor=predecessor,
        framework=tools.workspace.framework_provenance(), previous_implementation={
            "build_id": predecessor["build_id"], "copied_source_digest": predecessor["source_digest"],
            "status": "completed", "summary": "Accepted authored feature",
            "unresolved": ["Live credentials and external services remain untested."]})
    write_private(tools.directory / "input.json", tools.input)
    return tools.workspace.digest()


def fake_fresh_check(tools, monkeypatch):
    calls = []

    async def check(args):
        calls.append("fresh-check")
        builder_check_fixture(tools)
        tools.publication = None
        return tools.workspace.verification()

    monkeypatch.setattr(tools, "check", check)
    return calls


@pytest.mark.asyncio
async def test_runtime_refresh_runs_fresh_checks_then_only_native_acceptance_and_keeps_live_limits(team, monkeypatch):
    require_sandbox()
    tools, reviewer = team
    tools.workspace.write_file("application/feature.py", "def normalize(value):\n    return value.strip().lower()\n")
    digest = bind_refresh(tools)
    checked = fake_fresh_check(tools, monkeypatch)
    calls = []
    protected = '''import unittest
from application.feature import normalize
class RequestedOutcome(unittest.TestCase):
    def test_mixed_case_spaces(self):
        self.assertEqual(normalize("  HeLLo  "), "hello")
'''

    async def invoke(identity, content, key, admitted):
        assert checked == ["fresh-check"]
        assert identity == "acceptance:main"
        assert tools.publication["checked_digest"] == digest
        assert tools.publication["unresolved"] == ["Live credentials and external services remain untested."]
        calls.append((identity, key))
        admitted("native-" + key)
        assert (await reviewer.write_test({"path": "test_normalization.py", "content": protected}))["ok"]
        execution = await reviewer.check({})
        assert execution["status"] == "pass", execution
        await publish_checked_normalization(reviewer, execution)
        return completed_review("native-" + key)

    accepted = await implement_and_review(tools.directory, tools, reviewer, invoke)
    assert calls == [("acceptance:main", "acceptance-1")]
    assert accepted == reviewer.acceptance.verification() and accepted["status"] == "pass"
    assert tools.workspace.digest() == digest
    assert len(reviewer.acceptance.history()) == 1
    assert not read_receipt(tools.directory).get("repair_round")


@pytest.mark.asyncio
async def test_runtime_refresh_independent_counterexample_is_retained_without_author_repair(team, monkeypatch):
    tools, reviewer = team
    digest = bind_refresh(tools)
    fake_fresh_check(tools, monkeypatch)
    calls = []

    async def invoke(identity, content, key, admitted):
        calls.append((identity, key))
        assert identity == "acceptance:main"
        admitted("native-" + key)
        execution = await reviewer.check({})
        assert execution["tests_run"] == 0
        result = await reviewer.publish({"summary": "Requested behavior needs attention.",
            "requirements": [{"id": "normalize-input", "status": "unresolved", "source_paths": [],
                "test_ids": [], "evidence": "The requested behavior has no executable evidence.",
                "remaining_dependencies": ["Review the existing application behavior."]}],
            "counterexamples": ["Mixed-case input remains unchanged."], "remaining_dependencies": []})
        assert result["ok"]
        return completed_review("native-" + key)

    with pytest.raises(RuntimeError, match="automatic repair budget"):
        await implement_and_review(tools.directory, tools, reviewer, invoke)
    assert calls == [("acceptance:main", "acceptance-1")]
    assert tools.workspace.digest() == digest
    assert len(reviewer.acceptance.history()) == 1
    receipt = read_receipt(tools.directory)
    assert not receipt["ready_to_package"]
    assert receipt["semantic_acceptance"]["status"] == "needs_repair"
    assert receipt["semantic_acceptance"]["counterexamples"] == ["Mixed-case input remains unchanged."]
    assert not receipt.get("repair_round")


@pytest.mark.parametrize("fresh_result", [
    {"ok": False, "error": "Preparation unavailable"},
    {"status": "fail", "ready_to_package": False, "tests_run": 1},
    {"status": "pass", "ready_to_package": True, "tests_run": 0},
    {"status": "pass", "ready_to_package": True, "tests_run": True},
    {"status": "pass", "ready_to_package": True, "tests_run": 1, "digest": "b" * 64},
])
@pytest.mark.asyncio
async def test_runtime_refresh_failed_fresh_check_cannot_reuse_old_pass_or_dispatch(team, monkeypatch, fresh_result):
    tools, reviewer = team
    bind_refresh(tools)
    builder_check_fixture(tools)
    old_publication = deepcopy(tools.publication)

    async def failed(args):
        return fresh_result

    async def forbidden(*args, **kwargs):
        pytest.fail("An unsuccessful fresh check must not admit a native turn.")

    monkeypatch.setattr(tools, "check", failed)
    with pytest.raises(RuntimeError, match="fresh passing"):
        await implement_and_review(tools.directory, tools, reviewer, forbidden)
    assert tools.publication == old_publication
    assert reviewer.acceptance.history() == []


@pytest.mark.parametrize("when", ["before-check", "during-check", "after-review"])
@pytest.mark.asyncio
async def test_runtime_refresh_source_change_never_earns_completion(team, monkeypatch, when):
    tools, reviewer = team
    digest = bind_refresh(tools)
    checked = fake_fresh_check(tools, monkeypatch)
    calls = []
    if when == "before-check":
        tools.workspace.write_file("application/changed.py", "CHANGED = True\n")
    elif when == "during-check":
        async def changed_check(args):
            tools.workspace.write_file("application/changed.py", "CHANGED = True\n")
            builder_check_fixture(tools)
            return tools.workspace.verification()
        monkeypatch.setattr(tools, "check", changed_check)

    async def review(directory, builder, acceptance, invoke, *, max_reviews, timing):
        assert max_reviews == 1 and checked == ["fresh-check"]
        calls.append("review")
        tools.workspace.write_file("application/changed.py", "CHANGED = True\n")
        return {"status": "pass"}

    monkeypatch.setattr(worker, "review_and_repair", review)
    with pytest.raises(RuntimeError, match="source changed"):
        await implement_and_review(tools.directory, tools, reviewer, None)
    assert calls == (["review"] if when == "after-review" else [])
    assert tools.workspace.digest() != digest
    assert not read_receipt(tools.directory)["ready_to_package"]


@pytest.mark.parametrize("change", ["feedback", "predecessor", "framework", "typed-framework", "digest", "mode"])
def test_runtime_refresh_requires_exact_host_frozen_binding_before_dispatch(team, change):
    tools, _ = team
    bind_refresh(tools)
    if change == "feedback":
        tools.input["request"] = "Change the application"
    elif change == "predecessor":
        tools.input["runtime_refresh_predecessor"]["build_id"] = "foreign-build"
    elif change == "framework":
        tools.input["framework"]["origin"] = {"untrusted": "foreign framework"}
    elif change == "typed-framework":
        tools.input["framework"]["version"] = True
    elif change == "digest":
        tools.input["runtime_refresh_source_digest"] = "b" * 64
    else:
        tools.input["preserve_design"] = 1
    with pytest.raises((RuntimeError, ValueError)):
        runtime_refresh_source(tools)


@pytest.mark.asyncio
async def test_ordinary_build_still_authors_and_uses_existing_repair_budget(team, monkeypatch):
    tools, reviewer = team
    tools.input.update(request="Keep the user feedback.", previous_implementation={"status": "failed"})
    calls = []

    async def invoke(identity, content, key):
        assert identity == "implementation:main" and key == "build"
        assert "Keep the user feedback." in content and "Continue the saved authored source" in content
        calls.append("author")
        return completed_review("native-builder")

    async def publication(directory, builder, completed, admission, *, builder_phase):
        assert completed == completed_review("native-builder") and builder_phase == "initial_build"
        calls.append("publication")

    async def review(directory, builder, acceptance, admission, *, timing):
        # No max_reviews override is passed to ordinary authoring.
        calls.append("ordinary-review-and-repair")
        return {"status": "pass"}

    monkeypatch.setattr(worker, "ensure_builder_publication", publication)
    monkeypatch.setattr(worker, "review_and_repair", review)
    assert await implement_and_review(tools.directory, tools, reviewer, invoke) == {"status": "pass"}
    assert calls == ["author", "publication", "ordinary-review-and-repair"]
