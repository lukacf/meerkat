"""The unavailable native preparation adapter never admits new builder work."""
import copy

import pytest

from launchkit.guide import GuideService, implementation_action_context
from launchkit.guide_store import GuideStore
from launchkit.implementation import ImplementationPrerequisiteNotAdmitted, ImplementationService
from test_implementation import service  # Existing inert-worker fixture, no native execution.


@pytest.mark.parametrize("saved_build", [False, True])
async def test_required_checks_block_first_build_and_stopped_resume_without_changing_source(service, saved_build):
    if saved_build:
        original = await service.submit(service.guide.identity, 3)
        await service.shutdown()
        assert (await service.status(service.guide.identity))["status"] == "interrupted"
    before = {p.relative_to(service.root): p.read_bytes() for p in service.root.rglob('*') if p.is_file()}
    service._require_service_checks = True
    # Old accepted wording, a forged projection and even an empty intent list
    # cannot stand in for native resource/operation observations.
    service.guide.state.update(service_preparation={"status": "ready"})
    service.guide.state["brief"] = {"facts": {}, "requirements": [], "service_requirements": []}
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
    guide = GuideService(GuideStore(tmp_path), object())
    state = guide.store.create({'version':1, 'brief':{'facts':{},'requirements':[]},
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
        'brief': {'facts':{},'requirements':[]}}
    impl=ImplementationService('/unused',None,require_service_checks=True)
    state['service_preparation']=impl.service_preparation(state)
    # Legacy brief avoids unrelated interview decisions, as for old saved work.
    state['brief']={}
    actions=implementation_action_context(state, {'lifecycle':{'build_gate_clear':True,
        'availability':'available','content_current':True,'status':'interrupted','resume_available':True}},
        {'available':True}, True, availability={'available':True})
    assert actions['build_available'] is False and actions['resume_available'] is False
    assert 'Required service access has not been verified' in actions['unavailable_reason']


def test_production_server_binds_required_checks_to_the_same_guide_and_builder():
    from launchkit import server
    state={'id':'guide','version':1,'brief':{}}
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
    monkeypatch.setattr(server, 'application_host', None)
    monkeypatch.setattr(server, 'implementation', service)
    with pytest.raises(HTTPException) as failure:
        await server.build_implementation(service.guide.identity,
            server.ImplementationRequest(version=3, previous_build_id=selected))
    assert failure.value.status_code == 503
    assert 'Build and Resume are blocked' in failure.value.detail
    assert not service.tasks and not service.processes
