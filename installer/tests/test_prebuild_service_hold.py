"""Only unresolved required service operations prevent new builder work."""
import copy
from types import SimpleNamespace

import pytest

from launchkit.guide import GuideService, implementation_action_context
from launchkit.guide_store import GuideStore
from launchkit.implementation import ImplementationPrerequisiteNotAdmitted, ImplementationService
from test_implementation import service  # Existing inert-worker fixture, no native execution.


def required_brief():
    return {"facts": {}, "requirements": [{"id": "conversation", "disposition": "open"}],
        "service_requirements": [{"id": "slack-incoming", "requirement_id": "conversation",
            "service": "Slack", "capability": "incoming_interaction",
            "operation": "Receive a self-DM and reply in that conversation",
            "resource": "My self-DM", "source_message_ids": ["request"]}]}


@pytest.mark.parametrize("saved_build", [False, True])
async def test_required_checks_block_first_build_and_stopped_resume_without_changing_source(service, saved_build):
    if saved_build:
        original = await service.submit(service.guide.identity, 3)
        await service.shutdown()
        assert (await service.status(service.guide.identity))["status"] == "interrupted"
    before = {p.relative_to(service.root): p.read_bytes() for p in service.root.rglob('*') if p.is_file()}
    service._require_service_checks = True
    # A forged ready projection cannot replace checks for a required operation.
    service.guide.state.update(service_preparation={"status": "ready"})
    service.guide.state["brief"] = required_brief()
    with pytest.raises(ImplementationPrerequisiteNotAdmitted, match="Build and Resume are blocked"):
        await service.submit(service.guide.identity, 3,
            expected_selected_build_id=original["build_id"] if saved_build else None)
    assert not service.tasks and not service.processes
    assert {p.relative_to(service.root): p.read_bytes() for p in service.root.rglob('*') if p.is_file()} == before


async def test_existing_inflight_admission_still_observes_the_same_owned_worker(service):
    try:
        original = await service.submit(service.guide.identity, 3)
        original_task = service.tasks[original['build_id']]
        service._require_service_checks = True
        observed = await service.submit(service.guide.identity, 3)
        assert observed['build_id'] == original['build_id']
        assert service.tasks == {original['build_id']: original_task}
    finally:
        await service.shutdown()


async def test_projection_is_read_only_and_neither_saved_nor_model_supplied(tmp_path):
    guide = GuideService(GuideStore(tmp_path), SimpleNamespace())
    state = guide.store.create({'version':1, 'brief': required_brief(),
        'service_preparation': {'status':'ready'}})
    original = copy.deepcopy(state)
    impl = ImplementationService(tmp_path, guide, require_service_checks=True)
    guide.service_preparation_provider = impl.service_preparation
    observed = await guide._with_implementation_evidence(state)
    assert observed['service_preparation']['status'] == 'unavailable'
    assert guide.store.get(state['id']) == original
    assert not impl.root.exists()
    saved = await guide._save(observed, observed['version'])
    assert saved['service_preparation']['status'] == 'unavailable'
    assert 'service_preparation' not in guide.store.get(state['id'])


def test_guide_cannot_offer_first_build_or_resume_under_the_same_hold():
    state={'id':'guide','version':1,'proposal':{'anything':'saved'},
        'brief': required_brief()}
    impl=ImplementationService('/unused',None,require_service_checks=True)
    state['service_preparation']=impl.service_preparation(state)
    # Legacy brief avoids unrelated interview decisions, as for old saved work.
    state['brief']={}
    actions=implementation_action_context(state, {'lifecycle':{'build_gate_clear':True,
        'availability':'available','content_current':True,'status':'interrupted','resume_available':True}},
        {'available':True}, True, availability={'available':True})
    assert actions['build_available'] is False and actions['resume_available'] is False
    assert 'required operations below have not been verified' in actions['unavailable_reason']


def test_production_server_binds_required_checks_to_the_same_guide_and_builder():
    from launchkit import server
    state={'id':'guide','version':1,'brief': required_brief()}
    assert server.guide.service_preparation_provider == server.implementation.service_preparation
    assert server.implementation.service_preparation(state)['status'] == 'unavailable'


@pytest.mark.parametrize('saved_build', [False, True])
async def test_actual_post_route_cannot_bypass_required_service_checks(service, monkeypatch, saved_build):
    from fastapi import HTTPException
    from launchkit import server
    selected = None
    if saved_build:
        selected = (await service.submit(service.guide.identity, 3))['build_id']
        await service.shutdown()
    service._require_service_checks = True
    service.guide.state['brief'] = required_brief()
    monkeypatch.setattr(server, 'application_host', None)
    monkeypatch.setattr(server, 'implementation', service)
    with pytest.raises(HTTPException) as failure:
        await server.build_implementation(service.guide.identity,
            server.ImplementationRequest(version=3, previous_build_id=selected))
    assert failure.value.status_code == 503
    assert 'Build and Resume are blocked' in failure.value.detail
    assert not service.tasks and not service.processes


@pytest.mark.parametrize("saved_build", [False, True])
async def test_application_without_service_requirements_can_build_and_resume(service, saved_build):
    selected = None
    if saved_build:
        selected = (await service.submit(service.guide.identity, 3))["build_id"]
        await service.shutdown()
    service._require_service_checks = True
    service.guide.state["brief"] = {"mission": "Compare local schedules", "service_requirements": []}
    assert service.service_preparation(service.guide.state) is None
    try:
        admitted = await service.submit(service.guide.identity, 3, expected_selected_build_id=selected)
        assert admitted["build_id"] in service.tasks
    finally:
        await service.shutdown()


def test_only_required_operations_are_reported_and_incoming_cannot_use_outbound_intent():
    impl = ImplementationService("/unused", None, require_service_checks=True)
    brief = required_brief()
    brief["requirements"].append({"id": "old-service", "disposition": "deferred"})
    brief["service_requirements"].append({**brief["service_requirements"][0],
        "id": "deferred", "requirement_id": "old-service"})
    brief["service_requirements"][0]["capability"] = "read"
    brief["incoming_data"] = [{"id": "self-dm", "requirement_id": "conversation",
        "name": "Slack", "purpose": "Reply to incoming questions", "scope": "My self-DM",
        "source_message_ids": ["request"]}]
    state = {"id": "guide", "version": 1, "brief": brief}
    original = copy.deepcopy(state)
    rows = impl.service_preparation(state)["required_operations"]
    assert [r["id"] for r in rows] == ["slack-incoming", "incoming:self-dm"]
    assert rows[1]["capability"] == "unresolved"
    assert rows[1]["reason_code"] == "source_operation_not_selected"
    assert state == original


def test_polled_collection_is_not_mislabeled_as_incoming_conversation():
    impl = ImplementationService("/unused", None, require_service_checks=True)
    state = {"id": "guide", "version": 1, "brief": {"incoming_data": [{
        "id": "ghe", "requirement_id": "repository-monitor", "name": "GHE repository changes",
        "purpose": "Poll repository changes", "scope": "The two requested repositories",
        "source_message_ids": ["request"]}]}}
    observation = impl.service_preparation(state)
    assert observation["reason_code"] == "service_inventory_missing"
    assert observation["required_operations"][0]["capability"] == "unresolved"


def test_incoming_operation_does_not_hide_sources_sharing_its_feature():
    impl = ImplementationService("/unused", None, require_service_checks=True)
    brief = required_brief()
    brief["requirements"].append({"id": "later", "disposition": "deferred"})
    brief["incoming_data"] = [
        {"id": "self-dm", "requirement_id": "conversation", "name": "Slack",
         "purpose": "Receive self-DM questions", "scope": "My self-DM",
         "source_message_ids": ["slack-request"]},
        {"id": "repositories", "requirement_id": "conversation", "name": "GHE",
         "purpose": "Poll repository changes", "scope": "Both requested repositories",
         "source_message_ids": ["ghe-request"]},
        {"id": "later-source", "requirement_id": "later", "name": "Later",
         "purpose": "Read later", "scope": "Deferred", "source_message_ids": ["later-request"]},
    ]
    state = {"id": "guide", "version": 1, "brief": brief}
    original = copy.deepcopy(state)
    observation = impl.service_preparation(state)
    assert observation["status"] == "unavailable"
    rows = observation["required_operations"]
    assert [row["id"] for row in rows] == [
        "slack-incoming", "incoming:self-dm", "incoming:repositories"]
    assert rows[1]["source_message_ids"] == ["slack-request"]
    assert rows[2]["resource"] == "Both requested repositories"
    assert rows[2]["source_message_ids"] == ["ghe-request"]
    assert all(row["capability"] == "unresolved" for row in rows[1:])
    assert state == original


def test_legacy_selected_connections_need_inventory_and_workspace_is_not_a_service():
    impl = ImplementationService("/unused", None, require_service_checks=True)
    state = {"id": "guide", "version": 1, "brief": {}, "proposal": {"connections": ["workspace"]}}
    observation = impl.service_preparation(state)
    assert observation["reason_code"] == "service_inventory_missing"
    assert observation["required_operations"] == []
    state["brief"]["service_requirements"] = []
    assert impl.service_preparation(state) is None
    state["proposal"]["connections"] = [{"id": "ghe", "name": "GHE", "tool_evidence": {"tool_names": ["read"]}}]
    row = impl.service_preparation(state)["required_operations"][0]
    assert row["service"] == "GHE" and row["capability"] == "unresolved"
    assert row["reason_code"] == "required_operations_not_selected"
    assert "tool_evidence" not in row


@pytest.mark.parametrize("saved_build", [False, True])
async def test_legacy_zero_connections_never_bypasses_inventory_on_build_or_resume(service, saved_build):
    selected = None
    if saved_build:
        selected = (await service.submit(service.guide.identity, 3))["build_id"]
        await service.shutdown()
    service._require_service_checks = True
    service.guide.state["brief"] = {"mission": "GHE and Slack are required"}
    service.guide.state["proposal"]["connections"] = []
    before = {p.relative_to(service.root): p.read_bytes() for p in service.root.rglob('*') if p.is_file()}
    with pytest.raises(ImplementationPrerequisiteNotAdmitted, match="recorded service inventory"):
        await service.submit(service.guide.identity, 3, expected_selected_build_id=selected)
    assert not service.tasks and not service.processes
    assert {p.relative_to(service.root): p.read_bytes() for p in service.root.rglob('*') if p.is_file()} == before
