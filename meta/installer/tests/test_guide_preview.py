"""Checked staged preview ownership, without provider or native admissions."""
import asyncio
import copy
from types import SimpleNamespace

import httpx
import pytest

from launchkit import server
from launchkit.architect import ArchitectService
from launchkit.guide import GuideService
from launchkit.guide_operations import GuideOperations
from launchkit.guide_preview import staged_guide_preview
from launchkit.guide_store import GuideStore
from launchkit.projects import fingerprint
from launchkit.server import GuideResponse
from test_guided_edit_publication import edit_args, initial_edits, initial_question, offline_admission


def fixture():
    state = {"id": "owned-guide", "version": 3, "brief": {"facts": {}, "requirements": []},
        "readiness": {"context_ready": False}, "question": {"title": "Saved question"},
        "operation": {"status": "running", "version": 3, "request_id": "a" * 64}}
    context = {"guide_id": state["id"], "guide_version": state["version"], "operation_request_id": "a" * 64}
    proposal = {"blueprint": {"project": {"name": "Checked next draft"}}}
    publication = {"brief": {"facts": {"outcome": {"summary": "A checked next outcome", "status": "confirmed",
        "source_message_ids": ["current-answer"]}}, "requirements": []}}
    return state, {"context": context, "proposal": proposal, "publication": publication, "active": True}


def test_checked_pair_is_detached_and_never_promotes_saved_truth():
    state, arguments = fixture()
    original = copy.deepcopy((state, arguments))
    projected = staged_guide_preview(state, **arguments)
    assert projected == {"guide_id": state["id"], "guide_version": 3, "request_id": "a" * 64,
        "blueprint": arguments["proposal"]["blueprint"], "brief": arguments["publication"]["brief"]}
    projected["blueprint"]["project"]["name"] = "Caller mutation"
    projected["brief"]["facts"]["outcome"]["source_message_ids"].append("caller-added")
    assert (state, arguments) == original
    assert "readiness" not in projected and "question" not in projected


@pytest.mark.parametrize("fault", ["inactive", "unknown_activity", "foreign_guide", "foreign_context_version",
    "boolean_context_version", "changed_state_version", "changed_operation_version", "foreign_request",
    "missing_request", "malformed_request", "completed", "failed", "interrupted", "array_publication",
    "missing_proposal", "missing_brief", "missing_blueprint", "array_brief", "array_blueprint"])
def test_incomplete_or_stale_stage_cannot_be_projected(fault):
    state, arguments = fixture()
    if fault == "inactive": arguments["active"] = False
    elif fault == "unknown_activity": arguments["active"] = None
    elif fault == "foreign_guide": arguments["context"]["guide_id"] = "other-guide"
    elif fault == "foreign_context_version": arguments["context"]["guide_version"] += 1
    elif fault == "boolean_context_version": arguments["context"]["guide_version"] = True
    elif fault == "changed_state_version": state["version"] += 1
    elif fault == "changed_operation_version": state["operation"]["version"] += 1
    elif fault == "foreign_request": arguments["context"]["operation_request_id"] = "b" * 64
    elif fault == "missing_request": arguments["context"].pop("operation_request_id")
    elif fault == "malformed_request": state["operation"]["request_id"] = "unbound"
    elif fault in {"completed", "failed", "interrupted"}: state["operation"]["status"] = fault
    elif fault == "array_publication": arguments["publication"] = []
    elif fault == "missing_proposal": arguments["proposal"] = None
    elif fault == "missing_brief": arguments["publication"].pop("brief")
    elif fault == "missing_blueprint": arguments["proposal"].pop("blueprint")
    elif fault == "array_brief": arguments["publication"]["brief"] = []
    elif fault == "array_blueprint": arguments["proposal"]["blueprint"] = []
    original = copy.deepcopy((state, arguments))
    assert staged_guide_preview(state, **arguments) is None
    assert (state, arguments) == original


def test_validated_topology_before_publication_has_no_borrowed_brief_or_saved_authority():
    state, arguments = fixture()
    state["brief"]["facts"]["authority"] = {"summary": "Saved earlier permission context"}
    arguments["context"]["brief"] = copy.deepcopy(state["brief"])
    arguments["publication"] = None
    original = copy.deepcopy((state, arguments))
    projected = staged_guide_preview(state, **arguments)
    assert projected == {"guide_id": state["id"], "guide_version": 3, "request_id": "a" * 64,
        "blueprint": arguments["proposal"]["blueprint"], "brief": None, "context_pending": True}
    assert "readiness" not in projected and "question" not in projected
    projected["blueprint"]["project"]["name"] = "Caller mutation"
    assert (state, arguments) == original


@pytest.mark.asyncio
@pytest.mark.parametrize("finish", ["completed", "failed"])
async def test_actual_checked_atomic_tool_preview_and_commit_or_failure_are_one_bound_pair(tmp_path, monkeypatch, finish):
    monkeypatch.setattr("launchkit.guide.planner_capability", lambda: {"available": True, "provider": "openai", "reason": "Offline fixture"})
    author = ArchitectService(tmp_path / "author")
    author.reference_read = True
    service = GuideService(GuideStore(tmp_path / "guide"), author)
    operations = GuideOperations(wait_seconds=0.01)
    initial = await service.create()
    saved_original = copy.deepcopy(service.store.get(initial["id"]))
    staged, release = asyncio.Event(), asyncio.Event()
    seen = {}

    async def callback():
        context = author.guide_context
        seen["request_id"] = context["operation_request_id"]
        args = edit_args(author, initial_edits(context["current_user_message"]["id"]),
            publication_mode="advance", question=initial_question(),
            message="Here is a first document comparison team. Who should it serve?", summary="A checked evolving draft.", visualization_feedback="Use circular groups and show one evidence handoff.")
        acknowledgement = await author.publish_guide_edit(args)
        assert acknowledgement["valid"] and acknowledgement["publication_token"]
        seen["pair"] = copy.deepcopy((author.proposal["blueprint"], author.guide_result["brief"]))
        staged.set()
        await release.wait()
        if finish == "failed": raise RuntimeError("Synthetic offline response failure")
        return acknowledgement

    offline_admission(monkeypatch, author, callback)
    monkeypatch.setattr(server, "guide", service)
    monkeypatch.setattr(server, "guide_operations", operations)
    monkeypatch.setattr(server, "architect", author)
    request = GuideResponse(version=initial["version"], action="chat",
        text="Compare the documents I provide, cite differences and unresolved claims.")
    task = None
    try:
        pending, deferred = await operations.submit(service, initial["id"], request)
        assert deferred and pending["operation"]["status"] == "running"
        task = operations.tasks[initial["id"]]
        await asyncio.wait_for(staged.wait(), 30)
        assert seen["request_id"] == fingerprint({"guide_id": initial["id"], **request.model_dump(mode="json")})
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=server.app), base_url="http://127.0.0.1") as client:
            read = await client.get(f"/api/guide/{initial['id']}")
            assert read.status_code == 200
            current = read.json()
            preview = current["live_design"]
            assert preview["request_id"] == current["operation"]["request_id"] == seen["request_id"]
            assert (preview["blueprint"], preview["brief"]) == seen["pair"]
            expected_feedback = {"text": "Use circular groups and show one evidence handoff.",
                "source_message_id": author.guide_context["current_user_message"]["id"]}
            assert preview["visualization_feedback"] == expected_feedback
            assert current["brief"] == initial["brief"] and current["readiness"] == initial["readiness"]
            assert current["question"] == initial["question"] and "live_proposal" not in current
            assert service.store.get(initial["id"]) == saved_original
            preview["blueprint"]["project"]["name"] = "Caller mutation"
            preview["brief"]["facts"]["outcome"]["summary"] = "Caller mutation"
            repeated = (await client.get(f"/api/guide/{initial['id']}")).json()
            assert (repeated["live_design"]["blueprint"], repeated["live_design"]["brief"]) == seen["pair"]
            release.set()
            if finish == "completed": await task
            else:
                with pytest.raises(RuntimeError): await task
            terminal = (await client.get(f"/api/guide/{initial['id']}")).json()
            assert "live_design" not in terminal and terminal["operation"]["status"] == finish
            if finish == "completed":
                assert (terminal["proposal"], terminal["brief"]) == seen["pair"]
                assert terminal["version"] == initial["version"] + 1
                assert terminal["visualization_feedback"] == expected_feedback
            else:
                assert service.store.get(initial["id"]) == saved_original
    finally:
        release.set()
        if task and not task.done():
            await asyncio.gather(task, return_exceptions=True)


@pytest.mark.asyncio
@pytest.mark.parametrize("finish", ["completed", "failed"])
async def test_actual_full_authoring_previews_topology_before_brief_then_checked_pair(tmp_path, monkeypatch, finish):
    monkeypatch.setattr("launchkit.guide.planner_capability", lambda: {"available": True, "provider": "openai", "reason": "Offline fixture"})
    author = ArchitectService(tmp_path / "author")
    author.reference_read = True
    service = GuideService(GuideStore(tmp_path / "guide"), author)
    operations = GuideOperations(wait_seconds=0.01)
    initial = await service.create()
    saved_original = copy.deepcopy(service.store.get(initial["id"]))
    topology_staged, publish, pair_staged, finish_turn = (asyncio.Event() for _ in range(4))
    seen = {}

    async def callback():
        context = author.guide_context
        edits = initial_edits(context["current_user_message"]["id"])
        seen["pair"] = copy.deepcopy((edits[0]["value"], edits[1]["value"]))
        acknowledgement = await author.propose({"blueprint": edits[0]["value"], "summary": "Checked early topology."})
        assert acknowledgement["valid"] and author.guide_result is None
        topology_staged.set()
        await publish.wait()
        acknowledgement = await author.publish_guide_step({"publication_mode": "advance", "question": initial_question(),
            "message": "Here is a document comparison team. Who should it serve?", "brief": edits[1]["value"],
            "design_changed": True, "summary": "The current checked design and understanding."})
        assert acknowledgement["valid"] and acknowledgement["publication_token"]
        pair_staged.set()
        await finish_turn.wait()
        if finish == "failed":
            raise RuntimeError("Synthetic offline response failure")
        return acknowledgement

    offline_admission(monkeypatch, author, callback)
    monkeypatch.setattr(server, "guide", service)
    monkeypatch.setattr(server, "guide_operations", operations)
    monkeypatch.setattr(server, "architect", author)
    request = GuideResponse(version=initial["version"], action="chat",
        text="Compare supplied documents and retain cited differences and uncertainty.")
    task = None
    try:
        pending, deferred = await operations.submit(service, initial["id"], request)
        assert deferred and pending["operation"]["status"] == "running"
        task = operations.tasks[initial["id"]]
        await asyncio.wait_for(topology_staged.wait(), 30)
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=server.app), base_url="http://127.0.0.1") as client:
            early = (await client.get(f"/api/guide/{initial['id']}")).json()
            assert early["live_design"]["blueprint"] == seen["pair"][0]
            assert early["live_design"]["brief"] is None and early["live_design"]["context_pending"] is True
            assert early["live_design"]["request_id"] == pending["operation"]["request_id"]
            for key in ("brief", "question", "readiness", "implementation_actions"):
                assert early.get(key) == initial.get(key)
            assert service.store.get(initial["id"]) == saved_original
            publish.set()
            await asyncio.wait_for(pair_staged.wait(), 30)
            paired = (await client.get(f"/api/guide/{initial['id']}")).json()
            assert (paired["live_design"]["blueprint"], paired["live_design"]["brief"]) == seen["pair"]
            assert "context_pending" not in paired["live_design"]
            assert service.store.get(initial["id"]) == saved_original
            finish_turn.set()
            if finish == "completed":
                await task
            else:
                with pytest.raises(RuntimeError):
                    await task
            terminal = (await client.get(f"/api/guide/{initial['id']}")).json()
            assert "live_design" not in terminal and terminal["operation"]["status"] == finish
            if finish == "completed":
                assert (terminal["proposal"], terminal["brief"]) == seen["pair"]
            else:
                assert service.store.get(initial["id"]) == saved_original
    finally:
        publish.set()
        finish_turn.set()
        if task and not task.done():
            await asyncio.gather(task, return_exceptions=True)


@pytest.mark.asyncio
@pytest.mark.parametrize("fault", ["foreign_request", "foreign_version", "inactive", "malformed_publication"])
@pytest.mark.parametrize("context_pending", [False, True])
async def test_http_read_fences_stale_same_guide_context_and_preserves_saved_fields(monkeypatch, fault, context_pending):
    state, arguments = fixture()
    if context_pending:
        arguments["publication"] = None
    if fault == "foreign_request": arguments["context"]["operation_request_id"] = "b" * 64
    elif fault == "foreign_version": arguments["context"]["guide_version"] += 1
    elif fault == "inactive": arguments["active"] = False
    else: arguments["publication"] = {}
    original = copy.deepcopy(state)
    input_surface = {"file_uploads": True, "image_analysis": False}
    async def read(_service, _identity): return copy.deepcopy(state)
    monkeypatch.setattr(server, "guide", SimpleNamespace(input_surface=lambda: input_surface))
    monkeypatch.setattr(server, "guide_operations", SimpleNamespace(read=read))
    monkeypatch.setattr(server, "architect", SimpleNamespace(guide_context=arguments["context"],
        proposal=arguments["proposal"], guide_result=arguments["publication"],
        message_lock=SimpleNamespace(locked=lambda: arguments["active"])))
    async with httpx.AsyncClient(transport=httpx.ASGITransport(app=server.app), base_url="http://127.0.0.1") as client:
        response = await client.get("/api/guide/owned-guide")
        assert response.status_code == 200 and response.json() == {**original, "input_surface": input_surface}
    assert state == original


@pytest.mark.parametrize('update', ['omitted', None, '', '  Give the coordinator more space.\nKeep circles.  '])
def test_visualization_feedback_paired_preview_uses_checked_update_and_host_source(update):
    state, arguments = fixture()
    existing = {'text': 'Earlier checked preference.', 'source_message_id': 'earlier-user'}
    state['visualization_feedback'] = copy.deepcopy(existing)
    arguments['context'].update(visualization_feedback=copy.deepcopy(existing),
        current_user_message={'id': 'actual-current-user', 'role': 'user'})
    if update != 'omitted':
        arguments['publication']['visualization_feedback'] = update
    original = copy.deepcopy((state, arguments))
    projected = staged_guide_preview(state, **arguments)
    expected = existing if update in ('omitted', None) else None if update == '' else {
        'text': update, 'source_message_id': 'actual-current-user'}
    assert projected['visualization_feedback'] == expected
    if expected:
        projected['visualization_feedback']['text'] = 'Caller mutation'
    assert (state, arguments) == original
    assert 'visualization_feedback' not in projected['blueprint']
    assert 'visualization_feedback' not in projected['brief']


def test_visualization_feedback_stale_or_unpaired_preview_never_borrows_saved_feedback():
    state, arguments = fixture()
    existing = {'text': 'Saved feedback cannot join a topology half.', 'source_message_id': 'earlier-user'}
    state['visualization_feedback'] = copy.deepcopy(existing)
    arguments['context'].update(visualization_feedback=copy.deepcopy(existing),
        current_user_message={'id': 'actual-current-user', 'role': 'user'})
    arguments['publication'] = None
    projected = staged_guide_preview(state, **arguments)
    assert projected['context_pending'] is True and 'visualization_feedback' not in projected
    arguments['publication'] = {'brief': copy.deepcopy(state['brief']), 'visualization_feedback': 'Current checked feedback.'}
    arguments['context']['operation_request_id'] = 'b' * 64
    assert staged_guide_preview(state, **arguments) is None
