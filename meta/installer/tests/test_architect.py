import copy
import asyncio
import json
import sys
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest

from launchkit.architect import ArchitectService, setup_runtime_root, setup_settings_root, summarize_changes
from launchkit.blueprint import starter_blueprint
from launchkit.projects import fingerprint


@pytest.fixture
def architect(tmp_path, monkeypatch):
    monkeypatch.setitem(sys.modules, 'launchkit.compiler', SimpleNamespace(package_preview=lambda value: {'checks': [], 'summary': {}}))
    service=ArchitectService(tmp_path)
    service.set_context(starter_blueprint().model_dump(mode='json'))
    return service


@pytest.mark.asyncio
async def test_proposal_is_validated_and_does_not_mutate_current_design(architect):
    before=copy.deepcopy(architect.context)
    after=copy.deepcopy(before)
    after['project']['name']='Research home'
    result=await architect.propose({'blueprint':after,'summary':'A revised project name'})
    assert result['valid'] is True
    assert 'publication_token' not in result
    assert architect.context==before
    assert architect.proposal['base_hash']==fingerprint(before)
    assert architect.proposal['blueprint']==after
    assert architect.proposal['changes']==['Project updated']
    assert (architect.root/'proposal.json').stat().st_mode & 0o077==0


@pytest.mark.asyncio
async def test_invalid_or_invented_connection_cannot_be_proposed(architect):
    after=copy.deepcopy(architect.context)
    after['topology']['entrypoint']='unknown:main'
    result=await architect.propose({'blueprint':after})
    assert result['valid'] is False
    assert architect.proposal is None
    after=copy.deepcopy(architect.context)
    after['connections']=[{'id':'guessed','name':'Guessed MCP','transport':'stdio','command':'npx','args':['-y','invented-server']}]
    result=await architect.propose({'blueprint':after})
    assert result['valid'] is False
    assert 'not retrieved' in result['issues']
    assert architect.proposal is None


@pytest.mark.asyncio
async def test_registry_evidence_allows_config_but_not_invented_launch_arguments(architect):
    config={'id':'search','name':'Search MCP','transport':'stdio','command':'npx','args':['-y','verified-package@1.0.0'], 'env':{},'headers':{},'url':''}
    architect.discovered['search']=config
    after=copy.deepcopy(architect.context)
    after['connections']=[copy.deepcopy(config)]
    assert (await architect.propose({'blueprint':after}))['valid']
    after['connections'][0]['args'].append('--new-invented-flag')
    assert not (await architect.propose({'blueprint':after}))['valid']


def test_changes_describe_only_changed_domains():
    assert summarize_changes({'flows':[]}, {'flows':[], 'target':'gcp'})==['Deployment target updated']


@pytest.mark.asyncio
async def test_instruction_only_proposal_reports_responsibilities_without_claiming_new_grants(architect):
    before = copy.deepcopy(architect.context)
    after = copy.deepcopy(before)
    after['agent_classes'][0]['instructions'] += ' Preserve the revised audience context in every answer.'
    result = await architect.propose({'blueprint': after, 'summary': 'Keep the current team and update its responsibilities.'})
    assert result['valid']
    assert architect.proposal['changes'] == ['Agent responsibilities updated']
    assert architect.proposal['blueprint']['agent_classes'][0]['tools'] == before['agent_classes'][0]['tools']
    assert architect.context == before


def test_member_instruction_summary_preserves_membership_and_grants():
    before = starter_blueprint().model_dump(mode='json')
    before['agent_classes'][0]['members'] = [{'key': 'main', 'name': 'Continuing owner',
                                            'instructions': 'Keep the continuing context and return useful evidence.'}]
    after = copy.deepcopy(before)
    after['agent_classes'][0]['members'][0]['instructions'] += ' Explain corrections explicitly.'
    assert summarize_changes(before, after) == ['Agent responsibilities updated']
    assert after['agent_classes'][0]['members'][0]['key'] == before['agent_classes'][0]['members'][0]['key']


@pytest.mark.parametrize('change, expected', [
    ('model', 'Agent models updated'),
    ('membership', 'Team membership updated'),
    ('grants', 'Tool grants updated'),
    ('read_only', 'Read-only tool policy updated'),
    ('roles', 'Agent roles updated'),
    ('topology', 'Agent communication updated'),
    ('flow', 'Workflows updated'),
])
def test_change_summary_distinguishes_actual_configuration_changes(change, expected):
    before = starter_blueprint().model_dump(mode='json')
    after = copy.deepcopy(before)
    role = after['agent_classes'][0]
    if change == 'model':
        role['model'] = 'gpt-6-sol'
    elif change == 'membership':
        role['instances'] += 1
    elif change == 'grants':
        role['tools']['shell'] = True
    elif change == 'read_only':
        role['tools']['read_only'] = True
    elif change == 'roles':
        added = copy.deepcopy(role)
        added['id'] = 'additional_role'
        after['agent_classes'].append(added)
    elif change == 'topology':
        after['topology']['mode'] = 'mesh'
    else:
        after['flows'][0]['steps'][0]['prompt'] += ' State what the evidence does not establish.'
    assert summarize_changes(before, after) == [expected]
    assert summarize_changes(after, after) == []


@pytest.mark.asyncio
@pytest.mark.parametrize('design', ['shared_tools_distinct_expertise', 'known_context_population'])
async def test_guided_design_keeps_expertise_and_population_choices_loose(tmp_path, design):
    service = ArchitectService(tmp_path)
    candidate = starter_blueprint().model_dump(mode='json')
    owner = copy.deepcopy(candidate['agent_classes'][0])
    candidate['agent_classes'] = [owner]
    candidate['flows'] = []
    candidate['topology'] = {'mode': 'manual', 'entrypoint': 'lead:main', 'edges': []}
    if design == 'shared_tools_distinct_expertise':
        specialties = [
            ('conditions', 'Conditions expert',
             'Interpret observation quality, uncertainty and changing conditions; return the evidence and limits to the owner.'),
            ('methods', 'Methods expert',
             'Apply subject-matter constraints and compare suitable methods; return the reasoning and original evidence to the owner.'),
        ]
        for identity, name, instructions in specialties:
            specialist = copy.deepcopy(owner)
            specialist.update(id=identity, name=name, description=instructions,
                              instructions=instructions, instances=1, members=[])
            specialist['tools'].update(workspace=True, memory=False)
            candidate['agent_classes'].append(specialist)
            candidate['topology']['edges'].append({'a': 'lead:main', 'b': f'{identity}:main'})
        expected_count = 3
    else:
        context_owner = copy.deepcopy(owner)
        context_owner.update(id='context', name='Project owners', instances=500,
            description='Each supplied project has its own decisions and continuing history.',
            instructions='Own only the supplied project context; preserve its decisions, evidence and corrections.')
        context_owner['members'] = [
            {'key': f'project_{index:03d}', 'name': f'Project {index:03d}',
             'instructions': f'Retain the independent evidence, decisions and corrections for supplied project {index:03d}.'}
            for index in range(500)
        ]
        candidate['agent_classes'].append(context_owner)
        expected_count = 501
    before = copy.deepcopy(candidate)
    service.set_context(candidate)
    service.guide_context = {'action': 'answer'}
    await service.read_design_reference({})
    result = await service.propose({'blueprint': candidate, 'summary': 'A work-derived test design.'})
    assert result['valid'], result
    published = service.proposal['blueprint']
    assert sum(role['instances'] for role in published['agent_classes']) == expected_count
    assert published == candidate == before
    assert service.runtime is None
    assert not result['design_findings']
    if design == 'shared_tools_distinct_expertise':
        first, second = published['agent_classes'][1:]
        assert first['tools'] == second['tools'] and first['tools']['memory'] is False
        assert first['instructions'] != second['instructions']
    else:
        assert published['agent_classes'][1]['members'][499]['key'] == 'project_499'


def test_fresh_meta_runtime_preserves_existing_three_role_store_and_guide_history(tmp_path):
    old = tmp_path / 'architect-runtime' / '.mobkit' / 'state'
    old.mkdir(parents=True)
    (old / 'event_log.sqlite3').write_bytes(b'preserved native history')
    (tmp_path / 'guide.sqlite3').write_bytes(b'preserved guide history')
    runtime = setup_runtime_root(tmp_path, version='0.9.3')
    assert runtime == tmp_path / 'architect-runtime-meta-v1'
    assert not runtime.exists()
    assert (old / 'event_log.sqlite3').read_bytes() == b'preserved native history'
    assert (tmp_path / 'guide.sqlite3').read_bytes() == b'preserved guide history'


@pytest.mark.parametrize(('version', 'directory'), [
    ('0.9.0', 'architect-runtime-meta-v1'), ('0.9.2+meta', 'architect-runtime-meta-v1'),
    ('0.9.3', 'architect-runtime-meta-v1'), ('0.9.4', 'architect-runtime-meta-v2'),
    ('0.9.5', 'architect-runtime-meta-v2'), ('0.10.0', 'architect-runtime-meta-v2')])
def test_native_storage_release_starts_a_fresh_setup_generation(tmp_path, version, directory):
    # Meta 0.9.4 refuses state an earlier release wrote until it is migrated
    # offline, so the setup team moves to a fresh generation, never in place.
    old = tmp_path / 'architect-runtime-meta-v1' / '.mobkit' / 'state'
    old.mkdir(parents=True)
    (old / 'event_log.sqlite3').write_bytes(b'preserved native history')
    assert setup_runtime_root(tmp_path, version=version) == tmp_path / directory
    assert (old / 'event_log.sqlite3').read_bytes() == b'preserved native history'
    assert [path.name for path in tmp_path.iterdir()] == ['architect-runtime-meta-v1']


def test_setup_generation_follows_the_selected_guide_release(tmp_path, monkeypatch):
    from launchkit import guide_sdk
    assert setup_runtime_root(tmp_path) == setup_runtime_root(tmp_path, version=guide_sdk.SELECTED_VERSION)
    monkeypatch.setattr(guide_sdk, 'SELECTED_VERSION', '0.9.4')
    assert setup_runtime_root(tmp_path) == tmp_path / 'architect-runtime-meta-v2'
    for invalid in ('', 'latest', '0.9'):
        with pytest.raises(ValueError):
            setup_runtime_root(tmp_path, version=invalid)


def test_fresh_setup_generation_carries_saved_tuning_until_its_own_composition(tmp_path):
    from launchkit.provider_settings import canonical_setup_settings
    from test_provider_settings import native_spec
    thresholds = {'architect': 150000, 'connectors': None, 'reviewer': 65000, 'visualizer': 200000}
    def mutate(value):
        for name, threshold in thresholds.items():
            value['definition']['profiles'][name]['auto_compact_threshold'] = threshold
    previous = tmp_path / 'architect-runtime-meta-v1'
    fresh = tmp_path / 'architect-runtime-meta-v2'
    assert setup_settings_root(tmp_path, version='0.9.4') == fresh
    database = native_spec(previous, mutate=mutate)
    before = database.read_bytes()
    assert setup_settings_root(tmp_path, version='0.9.3') == previous
    assert setup_settings_root(tmp_path, version='0.9.4') == previous
    assert canonical_setup_settings(setup_settings_root(tmp_path, version='0.9.4')) == {
        'provider': 'anthropic', 'auto_compact_thresholds': thresholds}
    # A generation created but not yet composed still borrows the settings.
    (fresh / '.mobkit' / 'state').mkdir(parents=True)
    assert setup_settings_root(tmp_path, version='0.9.4') == previous
    (fresh / '.mobkit' / 'state').rmdir()
    # Once the fresh generation holds its own definition, that one binds.
    native_spec(fresh, provider='openai')
    assert setup_settings_root(tmp_path, version='0.9.4') == fresh
    assert canonical_setup_settings(fresh)['provider'] == 'openai'
    assert database.read_bytes() == before


@pytest.mark.asyncio
async def test_pending_native_interaction_prevents_replacing_context(architect, monkeypatch):
    from launchkit.architect import ArchitectUnavailable
    architect.runtime = SimpleNamespace(rust_http_base_url='http://127.0.0.1:1')
    architect.pending_interaction = {'cursor': 1, 'interaction_id': 'native-in-flight'}
    before = copy.deepcopy(architect.context)
    after = copy.deepcopy(before)
    after['project']['name'] = 'New context'
    async def unchanged():
        return architect.status()
    monkeypatch.setattr(architect, 'refresh_status', unchanged)
    with pytest.raises(ArchitectUnavailable, match='previous request'):
        await architect.message('Another design request', after)
    assert architect.context == before


@pytest.mark.asyncio
async def test_status_does_not_race_active_message_timeline(architect):
    architect.runtime = SimpleNamespace(rust_http_base_url='http://127.0.0.1:1')
    architect.pending_interaction = {'cursor': 1, 'interaction_id': 'native-in-flight'}
    async with architect.message_lock:
        status = await architect.refresh_status()
    assert status['busy']
    assert architect.pending_interaction['interaction_id'] == 'native-in-flight'


@pytest.mark.asyncio
async def test_failed_conversation_index_does_not_abandon_native_admission(architect, monkeypatch):
    import launchkit.architect as module
    from launchkit.guide_models import default_question
    requests = []
    publication = None
    class Response:
        def __init__(self, payload):
            self.payload = payload
        def raise_for_status(self):
            pass
        def json(self):
            return self.payload
    async def native_request(client, method, url, **kwargs):
        nonlocal publication
        requests.append(method)
        if method == 'post':
            publication = await architect.publish_guide_step({'message': 'Completed advice',
                'question': default_question('autonomy'), 'brief': brief})
            return Response({'interaction_id': 'admitted-turn', 'cursor': 'console:7'})
        return Response({'frames': [
            # The execution fact carries no body; the result event carries the
            # tool's output.
            {'kind': 'tool_execution_completed', 'identity': 'architect:main',
             'interaction_id': 'admitted-turn', 'source': {'kind': 'console_event'},
             'payload': {'name': 'publish_guide_step', 'is_error': False, 'duration_ms': 3}},
            {'kind': 'tool_result_received', 'identity': 'architect:main',
             'interaction_id': 'admitted-turn', 'source': {'kind': 'console_event'},
             'payload': {'name': 'publish_guide_step', 'is_error': False, 'result': json.dumps(publication)}},
            {'kind': 'interaction_complete', 'identity': 'architect:main',
             'interaction_id': 'admitted-turn', 'source': {'kind': 'console_event'},
             'payload': {'result': 'Completed advice', 'source_event_type': 'run_completed'}}]})
    async def start():
        pass
    async def failed_index(context, accepted):
        raise OSError('Index temporarily unavailable')
    architect.runtime = SimpleNamespace(rust_http_base_url='http://native.test', agent=lambda identity: None)
    architect.on_interaction_admitted = failed_index
    monkeypatch.setattr(architect, 'start', start)
    monkeypatch.setattr(module, '_native_request', native_request)
    brief = {'facts': {'outcome': {'summary': 'Compare source documents with cited findings',
        'status': 'confirmed', 'source_message_ids': ['user-1']}}, 'requirements': []}
    context = {'guide_id': 'our-setup', 'action': 'answer', 'current_stage': 'purpose',
        'evidence_messages': [{'id': 'user-1', 'role': 'user', 'text': 'Compare source documents'}], 'brief': {}}
    result = await architect.message('A setup turn', architect.context, guide_context=context)
    assert result['message'] == 'Completed advice'
    assert requests == ['post', 'get']
    assert architect.pending_interaction is None
    assert architect.message_publications is None
    assert architect.state == 'ready'


@pytest.mark.asyncio
async def test_message_stops_on_its_exact_native_failure(architect, monkeypatch):
    from launchkit.architect import ArchitectUnavailable
    architect.runtime = SimpleNamespace(rust_http_base_url='http://offline.invalid')
    monkeypatch.setattr(architect, 'start', AsyncMock())
    async def request(client, method, url, **kwargs):
        body = {'interaction_id': 'failed-turn', 'cursor': 'console:1'} if method == 'post' else {
            'frames': [{'kind': 'interaction_failed', 'identity': 'architect:main',
                'interaction_id': 'failed-turn', 'source': {'kind': 'console_event'}, 'payload': {}}]}
        return SimpleNamespace(raise_for_status=lambda: None, json=lambda: body)
    monkeypatch.setattr('launchkit.architect._native_request', request)
    with pytest.raises(ArchitectUnavailable, match='could not complete'):
        await asyncio.wait_for(architect.message('One request', architect.context), timeout=2)
    assert architect.pending_interaction is None and architect.message_publications is None


@pytest.mark.asyncio
async def test_native_guide_tool_validates_evidence_and_allows_adaptive_topics(architect):
    from launchkit.guide_models import default_question
    architect.guide_context = {'action': 'answer', 'current_stage': 'purpose',
        'evidence_messages': [{'id': 'user-1', 'role': 'user', 'text': 'Compare source documents'}], 'brief': {}}
    brief = {'facts': {'outcome': {'summary': 'Compare source documents with cited findings',
        'status': 'confirmed', 'source_message_ids': ['user-1']}}, 'requirements': []}
    result = await architect.publish_guide_step({'message': 'A useful next choice', 'question': default_question('autonomy'), 'brief': brief})
    assert result['valid']
    result = await architect.publish_guide_step({'message': 'An updated plan', 'question': default_question('connections'), 'brief': brief, 'design_changed': True})
    assert not result['valid'] and 'changed plan' in result['issue']
    await architect.read_design_reference({})
    assert (await architect.propose({'blueprint': architect.context, 'summary': 'An initial useful team'}))['valid']
    assert (await architect.publish_guide_step({'message': 'An updated plan', 'question': default_question('connections'), 'brief': brief, 'design_changed': True}))['valid']
    assert not (await architect.publish_guide_step({'message': 'Ready', 'question': None, 'brief': brief}))['valid']
    brief['facts']['outcome']['source_message_ids'] = ['invented']
    result = await architect.publish_guide_step({'message': 'A question', 'question': default_question('purpose'), 'brief': brief})
    assert not result['valid']
    assert 'evidence' in result['issue'].lower() or 'missing' in result['issue'].lower()


@pytest.mark.asyncio
async def test_native_guide_tool_publishes_unchanged_operational_status_without_final_review(architect):
    from launchkit.guide_models import default_question, review_readiness
    brief = {'facts': {'outcome': {'summary': 'Compare source documents with cited findings',
        'status': 'confirmed', 'source_message_ids': ['user-1']}}, 'requirements': [
            {'id': 'custom_ui', 'outcome': 'Use a custom UI.', 'disposition': 'open',
             'source_message_ids': ['user-1']}]}
    architect.guide_context = {'action': 'chat', 'current_stage': 'purpose', 'current_status': 'question',
        'current_question': default_question('purpose'), 'saved_plan_available': True,
        'evidence_messages': [{'id': 'user-1', 'role': 'user', 'text': 'Compare documents in a custom UI.'}],
        'brief': copy.deepcopy(brief)}
    before_context, before_plan = copy.deepcopy(architect.guide_context), copy.deepcopy(architect.context)
    publication = {'message': 'The source is checked; the custom UI still needs evidence.',
                   'question': None, 'brief': copy.deepcopy(brief), 'design_changed': False}
    before_publication = copy.deepcopy(publication)
    result = await architect.publish_guide_step(publication)
    assert result == {'valid': True, 'stage': 'purpose', 'status': 'shown_in_guided_setup'}
    assert architect.guide_result['question'] is None and not architect.guide_result['design_changed']
    assert not review_readiness(architect.guide_result['brief'])['ready']
    assert architect.guide_context == before_context and architect.context == before_plan
    assert publication == before_publication and architect.proposal is None
    previous_publication = copy.deepcopy(architect.guide_result)
    publication['brief']['requirements'][0]['disposition'] = 'supported'
    invalid = await architect.publish_guide_step(publication)
    assert invalid['valid'] is False
    assert architect.guide_result == previous_publication


@pytest.mark.asyncio
async def test_native_guide_tool_explicit_refinement_requires_current_proposal_and_keeps_discovery_stage(architect):
    architect.guide_context = {'action': 'chat', 'current_stage': 'autonomy',
        'brief': {}, 'evidence_messages': [{'id': 'request', 'role': 'user', 'text': 'Compare two source documents.'}]}
    publication = {'publication_mode': 'refinement', 'question': None, 'message': 'A first draft is saved.',
        'brief': {'facts': {'outcome': {'summary': 'Compare two source documents with cited findings.',
            'status': 'confirmed', 'source_message_ids': ['request']}}, 'requirements': []}}
    assert (await architect.publish_guide_step(publication))['valid'] is False
    await architect.read_design_reference({})
    assert (await architect.propose({'blueprint': architect.context, 'summary': 'A first draft.'}))['valid']
    result = await architect.publish_guide_step(publication)
    assert result == {'valid': True, 'stage': 'autonomy', 'status': 'shown_in_guided_setup'}
    assert architect.guide_result['publication_mode'] == 'refinement'
    saved = copy.deepcopy(architect.guide_result)
    architect.proposal = None
    assert (await architect.publish_guide_step(publication))['valid'] is False
    assert architect.guide_result == saved


def native_catalog(model='native-test-model', provider='openai'):
    from meerkat_mobkit.types import CatalogEntry, ModelsCatalogResult, ProviderDefaults
    entry = CatalogEntry(id=model, display_name='Actual catalog entry', provider=provider,
                         tier='balanced', context_window=64000, max_output_tokens=8000,
                         vision=True, image_tool_results=False)
    return ModelsCatalogResult(models=[entry], provider_defaults=[
        ProviderDefaults(provider=provider, default_model_id=model, models=[entry])])


@pytest.mark.asyncio
async def test_model_choices_use_exact_sdk_catalog_once_per_runtime_and_return_detached_context(architect):
    handle = SimpleNamespace(models_catalog=AsyncMock(return_value=native_catalog()))
    first_runtime = SimpleNamespace(mob_handle=lambda: handle)
    architect.runtime = first_runtime
    reference = await architect.read_design_reference({'topic': 'capabilities'})
    choices = reference['model_choices']
    assert choices['status'] == 'observed'
    assert choices['source'] == 'native_runtime_curated_catalog'
    assert choices['method'] == 'runtime.mob_handle().models_catalog()'
    assert choices['native_version'] == '0.8.47'
    assert choices['core_version'] == '0.8.51'
    assert choices['models'][0]['id'] == 'native-test-model'
    assert choices['models'][0]['vision'] is True
    assert choices['provider_defaults'][0]['default_model_id'] == 'native-test-model'
    choices['models'][0]['id'] = 'caller-mutation'
    inspected = await architect.inspect({'include_schema': False, 'include_model_choices': True})
    assert inspected['model_choices']['models'][0]['id'] == 'native-test-model'
    assert inspected['model_choices']['runtime_instance'] == choices['runtime_instance']
    handle.models_catalog.assert_awaited_once_with()
    second_handle = SimpleNamespace(models_catalog=AsyncMock(return_value=native_catalog('second-model', 'anthropic')))
    architect.runtime = SimpleNamespace(mob_handle=lambda: second_handle)
    fresh = (await architect.read_design_reference({}))['model_choices']
    assert fresh['models'][0]['id'] == 'second-model'
    assert fresh['runtime_instance'] != choices['runtime_instance']
    second_handle.models_catalog.assert_awaited_once_with()


@pytest.mark.asyncio
async def test_fresh_guided_inspection_requires_complete_reference_before_proposal(architect, monkeypatch):
    start = AsyncMock(side_effect=AssertionError('Reference inspection must not start native work'))
    monkeypatch.setattr(architect, 'start', start)
    architect.guide_context = {'guide_id': 'current-guide', 'action': 'chat', 'brief': {}}
    inspected = await architect.inspect({})
    assert inspected['design_reference_loaded'] is False
    assert 'topic:all before proposing' in inspected['design_reference']
    assert 'focused topic alone does not satisfy' in inspected['design_reference']
    focused = await architect.read_design_reference({'topic': 'foundations'})
    assert set(focused['sections']) == {'foundations'}
    assert (await architect.inspect({}))['design_reference_loaded'] is False
    blocked = await architect.propose({'blueprint': architect.context})
    assert not blocked['valid'] and 'complete read_design_reference' in blocked['issues']
    complete = await architect.read_design_reference({'topic': 'all'})
    assert set(complete['sections']) == {'foundations', 'patterns', 'capabilities', 'comms'}
    assert (await architect.propose({'blueprint': architect.context}))['valid']
    assert architect.runtime is None
    start.assert_not_awaited()


@pytest.mark.asyncio
async def test_loaded_reference_guidance_reuses_context_but_allows_lost_context_reread(architect, monkeypatch):
    start = AsyncMock(side_effect=AssertionError('Reference reread must not start native work'))
    monkeypatch.setattr(architect, 'start', start)
    architect.guide_context = {'guide_id': 'current-guide', 'action': 'chat', 'brief': {}}
    fresh = await architect.inspect({})
    complete = await architect.read_design_reference({'topic': 'all'})
    loaded = await architect.inspect({})
    assert loaded['design_reference_loaded'] is True
    assert 'Reuse it when available' in loaded['design_reference']
    assert 'focused topic for a concrete unanswered contract question' in loaded['design_reference']
    assert 'topic:all again if the reference context was lost' in loaded['design_reference']
    changed_guidance = {'design_reference', 'design_reference_loaded'}
    assert {key: value for key, value in fresh.items() if key not in changed_guidance} == {
        key: value for key, value in loaded.items() if key not in changed_guidance}
    architect.context['project']['name'] = 'Current revised design'
    current = await architect.inspect({})
    assert current['blueprint']['project']['name'] == 'Current revised design'
    before = copy.deepcopy((architect.context, architect.guide_context, architect.proposal))
    reread = await architect.read_design_reference({'topic': 'all'})
    assert reread == complete
    assert (architect.context, architect.guide_context, architect.proposal) == before
    assert architect.reference_read is True and architect.runtime is None
    start.assert_not_awaited()


@pytest.mark.asyncio
async def test_offline_model_inspection_does_not_start_runtime_or_invent_catalog(architect, monkeypatch):
    start = AsyncMock(side_effect=AssertionError('Offline inspection must not start native work'))
    monkeypatch.setattr(architect, 'start', start)
    reference = await architect.read_design_reference({})
    inspected = await architect.inspect({'include_schema': False, 'include_model_choices': True})
    for choices in (reference['model_choices'], inspected['model_choices']):
        assert choices['status'] == 'unknown'
        assert choices['issue'] == 'setup_runtime_not_started'
        assert choices['models'] == choices['provider_defaults'] == []
        assert 'runtime_instance' not in choices
        assert choices['native_version'] == '0.8.47' and choices['core_version'] == '0.8.51'
        assert choices['supplemental_models'] == []
    assert architect.runtime is None
    start.assert_not_awaited()


@pytest.mark.asyncio
async def test_catalog_read_error_is_cached_as_unknown_without_exposing_private_diagnostics(architect):
    handle = SimpleNamespace(models_catalog=AsyncMock(side_effect=RuntimeError('private-provider-token-do-not-echo')))
    architect.runtime = SimpleNamespace(mob_handle=lambda: handle)
    first = (await architect.read_design_reference({}))['model_choices']
    second = (await architect.inspect({'include_schema': False, 'include_model_choices': True}))['model_choices']
    assert first == second
    assert first['status'] == 'unknown' and first['issue'] == 'native_catalog_unavailable'
    assert first['models'] == []
    assert 'private-provider-token' not in str(first)
    assert 'does not verify' in first['availability_scope']
    handle.models_catalog.assert_awaited_once_with()


def test_old_compiler_supplement_is_explicitly_release_bound_and_not_native_catalog():
    import tomllib
    from launchkit.native_models import model_catalog_context, model_registry_lines, supplemental_model_metadata
    older = {'native_version': '0.8.42', 'core_version': None}
    supplement = supplemental_model_metadata(**older)
    assert [(row['provider'], row['id']) for row in supplement] == [('openai', 'gpt-6.1-sol')]
    assert supplement[0]['source'] == 'compiler_explicit_model_registry'
    emitted = tomllib.loads('\n'.join(model_registry_lines([('openai', 'gpt-6.1-sol')], **older)))
    assert emitted['models']['gpt-6.1-sol'] == supplement[0]['metadata']
    assert model_registry_lines([('openai', 'gpt-4.1-mini')]) == []
    context = model_catalog_context(native_catalog())
    assert context['models'][0]['id'] == 'native-test-model'
    assert all(row['id'] != 'gpt-6.1-sol' for row in context['models'])
    assert 'Unlisted pairs are unknown' in context['selection_guidance']
    assert 'not an exhaustive effective custom registry' in context['availability_scope']
    supplement[0]['metadata']['context_window'] = 1
    assert supplemental_model_metadata(**older)[0]['metadata']['context_window'] == 1050000
    assert supplemental_model_metadata() == []
    assert model_registry_lines([('openai', 'gpt-6.1-sol')]) == []


@pytest.mark.asyncio
async def test_malformed_sdk_catalog_cannot_be_promoted_to_observed_choices(architect):
    malformed = native_catalog(model=123)
    handle = SimpleNamespace(models_catalog=AsyncMock(return_value=malformed))
    architect.runtime = SimpleNamespace(mob_handle=lambda: handle)
    result = (await architect.inspect({'include_schema': False, 'include_model_choices': True}))['model_choices']
    assert result['status'] == 'unknown' and result['models'] == []
    assert result['issue'] == 'native_catalog_unavailable'


@pytest.mark.asyncio
async def test_runtime_replacement_during_catalog_read_cannot_publish_the_old_snapshot(architect):
    entered, release = asyncio.Event(), asyncio.Event()
    async def delayed():
        entered.set()
        await release.wait()
        return native_catalog('old-model')
    architect.runtime = SimpleNamespace(mob_handle=lambda: SimpleNamespace(models_catalog=delayed))
    read = asyncio.create_task(architect.model_choices())
    await entered.wait()
    new_handle = SimpleNamespace(models_catalog=AsyncMock(return_value=native_catalog('new-model')))
    architect.runtime = SimpleNamespace(mob_handle=lambda: new_handle)
    release.set()
    old = await read
    assert old['status'] == 'unknown' and old['models'] == []
    assert old['issue'] == 'setup_runtime_changed_during_catalog_read'
    fresh = await architect.model_choices()
    assert fresh['status'] == 'observed' and fresh['models'][0]['id'] == 'new-model'
    new_handle.models_catalog.assert_awaited_once_with()


@pytest.mark.asyncio
async def test_authoring_expands_before_compiler_checks_and_only_normal_blueprint_is_saved(architect, monkeypatch):
    import json
    checked = []
    monkeypatch.setitem(sys.modules, 'launchkit.compiler', SimpleNamespace(
        package_preview=lambda value: checked.append(value.model_dump(mode='json')) or {'checks': [], 'summary': {}}))
    raw = copy.deepcopy(architect.context)
    raw['agent_classes'][1]['instances'] = 3
    raw['topology'] = {'mode': 'manual', 'entrypoint': 'lead:main', 'edges': []}
    raw['flows'] = []
    authoring = {'member_sequences': [{'id': 'contexts', 'class_id': 'research', 'start': 8, 'count': 3,
        'key_template': 'c{n:03d}', 'name_template': 'Context {n:03d}',
        'instructions_template': 'Retain only the evidence and decisions of {name}.'}],
        'peer_edge_groups': [{'a': {'sequence': 'contexts'}, 'b': {'identities': ['review:main']}}]}
    before = copy.deepcopy((architect.context, raw, authoring))
    result = await architect.propose({'blueprint': raw, 'authoring': authoring, 'summary': 'Known context owners'})
    assert result['valid']
    assert len(checked) == 1
    candidate = architect.proposal['blueprint']
    assert checked[0] == candidate
    assert [m['name'] for m in candidate['agent_classes'][1]['members']] == ['Context 008', 'Context 009', 'Context 010']
    assert candidate['topology']['edges'] == [{'a': f'research:c{n:03d}', 'b': 'review:main'} for n in range(8, 11)]
    saved = json.loads((architect.root / 'proposal.json').read_text())
    assert saved == architect.proposal and 'authoring' not in saved
    assert (architect.context, raw, authoring) == before
    assert saved['base_hash'] == fingerprint(before[0])


@pytest.mark.asyncio
async def test_invalid_authoring_returns_findings_without_compiling_or_replacing_a_proposal(architect, monkeypatch):
    await architect.propose({'blueprint': architect.context, 'summary': 'Existing raw proposal'})
    before = copy.deepcopy(architect.proposal)
    saved = (architect.root / 'proposal.json').read_bytes()
    monkeypatch.setitem(sys.modules, 'launchkit.compiler', SimpleNamespace(
        package_preview=lambda value: pytest.fail('Invalid authoring cannot reach compiler checks')))
    args = {'blueprint': architect.context, 'authoring': {'peer_edge_groups': [
        {'a': {'identities': ['missing:main']}, 'b': {'identities': ['lead:main']}}]}}
    for callback in (architect.validate, architect.propose):
        result = await callback(args)
        assert not result['valid']
    assert architect.proposal == before and (architect.root / 'proposal.json').read_bytes() == saved


@pytest.mark.asyncio
async def test_authoring_cannot_bypass_existing_connection_or_mcp_evidence_checks(architect):
    raw = copy.deepcopy(architect.context)
    raw['connections'] = [{'id': 'guessed', 'name': 'Guessed MCP', 'transport': 'stdio',
                           'command': 'npx', 'args': ['invented-package']}]
    result = await architect.propose({'blueprint': raw, 'authoring': {}})
    assert not result['valid'] and 'not retrieved' in result['issues']
    assert architect.proposal is None
    config = {'id': 'documented', 'name': 'Documented MCP', 'transport': 'stdio',
              'command': 'known-command', 'args': []}
    architect.discovered['documented'] = config
    architect.guide_context = {'action': 'answer'}
    architect.reference_read = True
    raw = copy.deepcopy(architect.context)
    raw['connections'] = [config]
    raw['agent_classes'][0]['tools']['mcp'] = [{'connection_id': 'documented', 'tool_names': ['invented_tool']}]
    result = await architect.propose({'blueprint': raw, 'authoring': {}})
    assert not result['valid'] and 'publisher-backed exact tool names' in result['issues']
    assert architect.proposal is None


@pytest.mark.asyncio
async def test_offline_inspection_exposes_authoring_contract_without_starting_native_runtime(architect, monkeypatch):
    from launchkit.planning_limits import MAX_INSTANCES_PER_CLASS
    start = AsyncMock(side_effect=AssertionError('Schema inspection cannot start native work'))
    monkeypatch.setattr(architect, 'start', start)
    inspected = await architect.inspect({'include_schema': True})
    schema = inspected['authoring_schema']
    assert set(schema['properties']) == {'member_sequences', 'peer_edge_groups'}
    assert schema['$defs']['MemberSequence']['properties']['count']['maximum'] == MAX_INSTANCES_PER_CLASS
    assert (await architect.inspect({'include_schema': False}))['authoring_schema'] is None
    start.assert_not_called()


@pytest.mark.asyncio
async def test_default_inspection_preserves_complete_current_context_with_opt_in_schemas(architect):
    from launchkit.blueprint import Blueprint
    from launchkit.blueprint_authoring import authoring_schema
    from launchkit.guide_models import GuidePublication, default_question
    from launchkit.guided_edits import guided_edit_schema
    user = {'id': 'current-user', 'role': 'user', 'text': 'Keep my manually chosen responsibilities.',
            'action': 'manual', 'stage': 'purpose'}
    brief = {'facts': {'outcome': {'summary': 'Compare supplied documents with cited evidence.',
        'status': 'confirmed', 'source_message_ids': [user['id']]}}, 'requirements': []}
    manual = [{'changed_paths': ['/agent_classes/0/instructions'],
        'rationale': user['text'], 'message_id': user['id']}]
    actions = {'build_available': False, 'unavailable_reason': 'Save this current design first.'}
    architect.guide_context = {'guide_id': 'current-guide', 'guide_version': 5, 'action': 'manual',
        'current_stage': 'purpose', 'current_question': default_question('purpose'),
        'brief': brief, 'evidence_messages': [user], 'current_user_message': user,
        'manual_edits': manual, 'edit_base_token': 'current-checked-edit-token',
        'implementation_actions': actions}
    architect.guide_edit_turn = {'context': architect.guide_context}
    handle = SimpleNamespace(models_catalog=AsyncMock(return_value=native_catalog()))
    architect.runtime = SimpleNamespace(mob_handle=lambda: handle)
    before = copy.deepcopy((architect.context, architect.guide_context))
    slim = await architect.inspect({})
    full = await architect.inspect({'include_schema': True})
    expected_schemas = {'schema': Blueprint.model_json_schema(), 'authoring_schema': authoring_schema(),
        'guided_edit_schema': guided_edit_schema(), 'guide_publication_schema': GuidePublication.model_json_schema()}
    for key, schema in expected_schemas.items():
        assert slim[key] is None and full[key] == schema
    assert {key: value for key, value in full.items() if key not in expected_schemas} == {
        key: value for key, value in slim.items() if key not in expected_schemas}
    assert slim['blueprint'] == before[0]
    assert slim['guided_setup']['brief'] == brief
    assert slim['guided_setup']['manual_edits'] == manual
    assert slim['guided_setup']['implementation_actions'] == actions
    assert slim['guided_setup']['edit_base_token'] == 'current-checked-edit-token'
    assert slim['guided_setup']['evidence_messages'][0]['text'] == user['text']
    assert slim['model_choices'] is None
    handle.models_catalog.assert_not_awaited()
    assert (architect.context, architect.guide_context) == before
