"""Actual compiler, publication and saved-guide contracts with offline admission."""
import asyncio
import copy
import json
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest

from launchkit.architect import ArchitectService
from launchkit.blueprint import Blueprint, starter_blueprint
from launchkit.compiler import package_preview
from launchkit.guide import GuideService, compact_guidance_context
from launchkit.guide_models import DesignBrief, REVIEW_TOPICS, conversation_message, default_question, review_readiness
from launchkit.guide_store import GuideStore
from launchkit.guided_edits import guided_document, guided_edit_token
from launchkit.projects import DraftConflict
from launchkit.server import GuideResponse


def complete_brief():
    return {'facts': {topic: {'summary': 'Compare supplied documents with cited evidence.' if topic == 'outcome'
        else f'Explicit public fixture decision for {topic}.', 'status': 'confirmed',
        'source_message_ids': ['original-request']} for topic in REVIEW_TOPICS}, 'requirements': []}


def incoming_brief(recipient_kind):
    value = complete_brief()
    value['requirements'] = [{'id': 'updates', 'outcome': 'Notice changes to the selected project documents.',
        'disposition': 'open', 'extension': 'Resolve observation capabilities and native delivery.',
        'source_message_ids': ['original-request']}]
    value['incoming_data'] = [{'id': 'documents', 'requirement_id': 'updates', 'name': 'Document changes',
        'scope': 'Selected project documents', 'purpose': 'Keep the comparison current.',
        'history': 'bounded_history', 'history_window': 'The current project week',
        'routes': [{'recipient': 'review' if recipient_kind == 'class' else 'lead:main',
                    'recipient_kind': recipient_kind, 'delivery': 'direct'}],
        'source_message_ids': ['original-request']}]
    return DesignBrief.model_validate(value).model_dump(mode='json')


def context(blueprint, **changes):
    value = {'guide_id': 'fixture-guide', 'guide_version': 4, 'action': 'chat',
        'current_stage': 'autonomy', 'current_status': 'question', 'current_question': default_question('autonomy'),
        'saved_plan_available': True, 'manual_plan_submitted': False, 'brief': complete_brief(),
        'evidence_messages': [{'id': 'original-request', 'role': 'user', 'text': 'Compare supplied documents.'},
                              {'id': 'current-request', 'role': 'user', 'text': 'Rename my system.'}],
        'current_user_message': {'id': 'current-request', 'role': 'user', 'text': 'Rename my system.'},
        'manual_edits': [], 'reopened_topics': []}
    value.update(changes)
    value['edit_base_token'] = guided_edit_token(value, blueprint)
    return value


def activate(service, value):
    service.guide_context = value
    service.guide_edit_turn = {'context': value, 'token': value['edit_base_token'],
        'blueprint': copy.deepcopy(service.context), 'document': guided_document(service.context, value['brief'])}


def edit_args(service, edits=None, **changes):
    value = {'base_token': service.guide_context['edit_base_token'], 'edits': edits or [],
        'publication_mode': 'refinement', 'question': None, 'message': 'The revised draft is ready to explore.',
        'summary': 'A localized revision.'}
    value.update(changes)
    return value


def rename(name='Revised document team'):
    return [{'op': 'replace', 'path': '/blueprint/project/name', 'value': name}]


def initial_context(blueprint, *, intent_only=False):
    message = {'id': 'first-request', 'role': 'user',
        'text': 'Compare the documents I provide, cite differences and unresolved claims.',
        'intent_only': intent_only}
    return context(blueprint, guide_version=1, current_stage='purpose', current_question=default_question('purpose'),
        saved_plan_available=False, brief={}, evidence_messages=[message], current_user_message=message)


def initial_edits(message_id):
    draft = starter_blueprint().model_dump(mode='json')
    draft['project'] = {'name': 'Document comparison', 'slug': 'document-comparison',
        'description': 'Compare supplied documents with cited differences and unresolved claims.'}
    draft['agent_classes'][0].update(name='Comparison lead',
        description='Keeps the requested comparison and its output context.',
        instructions='Own the document comparison request. Coordinate evidence checks and explain cited differences and unresolved claims.')
    draft['agent_classes'][1].update(name='Document analyst',
        description='Maintains independent source analysis for the comparison.',
        instructions='Analyze the supplied documents independently. Identify and cite differences, retaining exact source evidence and uncertainty.')
    draft['agent_classes'][2].update(name='Evidence reviewer',
        description='Challenges unsupported comparison claims.',
        instructions='Review each comparison claim against its cited document evidence. Mark unsupported or unresolved claims explicitly.')
    draft['flows'][0].update(name='Compare supplied documents',
        description='Independent document analysis, evidence review and a cited comparison.')
    brief = {'facts': {
        'outcome': {'summary': 'Compare supplied documents with cited differences and unresolved claims.',
            'status': 'confirmed', 'source_message_ids': [message_id]},
        'audience': {'summary': 'The audience and sharing scope are still unknown.',
            'status': 'unresolved', 'source_message_ids': []}}, 'requirements': []}
    return [{'op': 'replace', 'path': '/blueprint', 'value': draft},
            {'op': 'replace', 'path': '/brief', 'value': brief}]


def initial_question():
    return {'id': 'comparison_audience', 'stage': 'purpose', 'title': 'Who will use these comparisons?',
        'description': 'This determines how each person keeps request context and sees the results.',
        'multiple': False, 'options': [
            {'id': 'individual', 'label': 'Just me', 'description': 'A single person uses the comparison.'},
            {'id': 'shared', 'label': 'Several people', 'description': 'Several people use or share the comparisons.'}],
        'allow_text': True}


@pytest.fixture
def author(tmp_path):
    service = ArchitectService(tmp_path / 'author')
    service.set_context(starter_blueprint().model_dump(mode='json'))
    service.reference_read = True
    activate(service, context(service.context))
    return service


@pytest.mark.asyncio
async def test_edit_matches_actual_full_compiler_files_and_visualization_source(author, tmp_path):
    original = copy.deepcopy(author.context)
    args = edit_args(author, rename())
    untouched = copy.deepcopy(args)
    edited = await author.publish_guide_edit(args)
    assert edited['valid'], edited
    full = ArchitectService(tmp_path / 'full')
    full.set_context(original)
    full.guide_context = copy.deepcopy(author.guide_context)
    full.reference_read = True
    candidate = copy.deepcopy(original)
    candidate['project']['name'] = 'Revised document team'
    checked = await full.propose({'blueprint': candidate, 'summary': args['summary']})
    assert checked['valid']
    publication = {key: value for key, value in args.items() if key not in {'base_token', 'edits'}}
    publication.update(brief=author.guide_context['brief'], design_changed=True)
    assert (await full.publish_guide_step(publication))['valid']
    assert author.proposal['blueprint'] == full.proposal['blueprint'] == candidate
    assert author.guide_result == full.guide_result
    assert edited['checks'] == checked['checks'] and edited['design_findings'] == checked['design_findings']
    assert package_preview(author.proposal['blueprint']) == package_preview(full.proposal['blueprint'])
    from launchkit.visualizer import VisualizerService
    graph = {'nodes': [{'id': identity, 'kind': 'agent'} for identity in Blueprint.model_validate(original).identities()],
             'edges': [], 'phases': []}
    before = author.visualizer.request_binding(original, graph)
    after = author.visualizer.request_binding(author.proposal['blueprint'], graph)
    assert after == full.visualizer.request_binding(full.proposal['blueprint'], graph)
    assert before['native_request_key'] != after['native_request_key']
    assert isinstance(author.visualizer, VisualizerService)
    assert author.context == original and args == untouched


@pytest.mark.asyncio
@pytest.mark.parametrize('fault', ['compiler', 'connection', 'mcp', 'evidence', 'requirement',
                                  'deferral', 'outcome', 'readiness', 'reference'])
async def test_rejected_full_checks_leave_the_prior_pair_and_cache_unchanged(author, fault):
    if fault == 'requirement':
        author.guide_context['brief']['requirements'] = [{'id': 'cited_output', 'outcome': 'Retain source citations.',
            'disposition': 'supported', 'source_message_ids': ['original-request']}]
    if fault == 'mcp':
        connection = {'id': 'known', 'name': 'Known MCP', 'transport': 'stdio', 'command': 'known-public-package', 'args': []}
        author.context['connections'] = [connection]
    activate(author, context(author.context, brief=copy.deepcopy(author.guide_context['brief'])))
    assert (await author.publish_guide_edit(edit_args(author, rename('First checked draft'))))['valid']
    before = copy.deepcopy((author.proposal, author.guide_result))
    saved = (author.root / 'proposal.json').read_bytes()
    edits = rename('Rejected replacement')
    overrides = {}
    if fault == 'compiler':
        edits.append({'op': 'replace', 'path': '/blueprint/topology/entrypoint', 'value': 'missing:main'})
    elif fault == 'connection':
        edits.append({'op': 'add', 'path': '/blueprint/connections/-', 'value':
            {'id': 'invented', 'name': 'Invented MCP', 'transport': 'stdio', 'command': 'made-up-package', 'args': []}})
    elif fault == 'mcp':
        edits.append({'op': 'add', 'path': '/blueprint/agent_classes/0/tools/mcp/-',
            'value': {'connection_id': 'known', 'tool_names': ['invented_write'], 'usage': 'write'}})
    elif fault == 'evidence':
        edits.append({'op': 'replace', 'path': '/brief/facts/outcome/source_message_ids', 'value': ['invented']})
    elif fault == 'requirement':
        edits.append({'op': 'remove', 'path': '/brief/requirements/0'})
    elif fault == 'deferral':
        edits.append({'op': 'add', 'path': '/brief/requirements/-', 'value': {'id': 'outside_action',
            'outcome': 'Write an outside action.', 'disposition': 'deferred',
            'source_message_ids': ['original-request'], 'decision_message_ids': ['original-request']}})
    elif fault == 'outcome':
        edits.append({'op': 'replace', 'path': '/brief/facts/outcome/status', 'value': 'unresolved'})
    elif fault == 'readiness':
        edits.append({'op': 'replace', 'path': '/brief/facts/audience/status', 'value': 'unresolved'})
        overrides['publication_mode'] = 'advance'
    else:
        author.reference_read = False
    result = await author.publish_guide_edit(edit_args(author, edits, **overrides))
    assert not result['valid'], result
    assert (author.proposal, author.guide_result) == before
    assert (author.root / 'proposal.json').read_bytes() == saved


@pytest.mark.asyncio
@pytest.mark.parametrize('initial', [False, True])
@pytest.mark.parametrize('source_change', ['token', 'version', 'guide', 'message', 'manual', 'base', 'replaced_context', 'ended'])
async def test_stale_or_inactive_edit_cannot_change_any_staged_source(author, source_change, initial):
    if initial:
        activate(author, context(author.context, saved_plan_available=False))
    args = edit_args(author, rename())
    assert (await author.publish_guide_edit(args))['valid']
    before = copy.deepcopy((author.proposal, author.guide_result))
    saved = (author.root / 'proposal.json').read_bytes()
    if source_change == 'token':
        args['base_token'] = '0' * 64
    elif source_change == 'version':
        author.guide_context['guide_version'] += 1
    elif source_change == 'guide':
        author.guide_context['guide_id'] = 'other-guide'
    elif source_change == 'message':
        author.guide_context['current_user_message']['id'] = 'later-message'
    elif source_change == 'manual':
        author.guide_context['manual_plan_submitted'] = True
    elif source_change == 'base':
        author.context['project']['name'] = 'Other source'
    elif source_change == 'replaced_context':
        author.guide_context = copy.deepcopy(author.guide_context)
    else:
        author.guide_edit_turn = None
    assert not (await author.publish_guide_edit(args))['valid']
    assert (author.proposal, author.guide_result) == before
    assert (author.root / 'proposal.json').read_bytes() == saved


@pytest.mark.asyncio
async def test_source_change_during_compiler_check_is_fenced_before_writes(author, monkeypatch):
    check = author._check_candidate
    def changed(args):
        result = check(args)
        author.guide_context['guide_version'] += 1
        return result
    monkeypatch.setattr(author, '_check_candidate', changed)
    assert not (await author.publish_guide_edit(edit_args(author, rename())))['valid']
    assert author.proposal is None and author.guide_result is None and not author.root.exists()


@pytest.mark.asyncio
async def test_repeat_calls_use_immutable_turn_base_and_cannot_erase_a_fresh_pair(author):
    assert (await author.publish_guide_edit(edit_args(author, rename('First draft'))))['valid']
    before = copy.deepcopy((author.proposal, author.guide_result))
    saved = (author.root / 'proposal.json').read_bytes()
    result = await author.publish_guide_edit(edit_args(author, [], publication_mode='advance'))
    assert not result['valid'] and 'erase' in result['issues']
    assert (author.proposal, author.guide_result) == before and (author.root / 'proposal.json').read_bytes() == saved
    changed = [{'op': 'replace', 'path': '/blueprint/project/description', 'value': 'A cumulative replacement draft.'}]
    assert (await author.publish_guide_edit(edit_args(author, changed)))['valid']
    assert author.proposal['blueprint']['project']['name'] == author.context['project']['name']
    assert author.proposal['blueprint']['project']['description'] == 'A cumulative replacement draft.'


@pytest.mark.asyncio
async def test_valid_full_replacement_invalidates_publication_but_invalid_full_keeps_pair(author):
    assert (await author.publish_guide_edit(edit_args(author, rename())))['valid']
    before = copy.deepcopy((author.proposal, author.guide_result))
    invalid = copy.deepcopy(author.context)
    invalid['topology']['entrypoint'] = 'unknown:main'
    assert not (await author.propose({'blueprint': invalid}))['valid']
    assert (author.proposal, author.guide_result) == before
    replacement = copy.deepcopy(author.context)
    replacement['project']['name'] = 'Full custom replacement'
    assert (await author.propose({'blueprint': replacement}))['valid']
    assert author.guide_result is None
    publication = {key: value for key, value in edit_args(author).items() if key not in {'base_token', 'edits'}}
    publication.update(brief=author.guide_context['brief'], design_changed=True)
    assert (await author.publish_guide_step(publication))['valid']
    assert author.proposal['blueprint'] == replacement


@pytest.mark.asyncio
async def test_unchanged_operational_guidance_skips_compiler_without_claiming_readiness(author, monkeypatch):
    brief = copy.deepcopy(author.guide_context['brief'])
    brief['facts']['audience']['status'] = 'unresolved'
    activate(author, context(author.context, brief=brief))
    monkeypatch.setattr(author, '_check_candidate', lambda args: pytest.fail('Unchanged operational guidance is not a fresh design'))
    result = await author.publish_guide_edit(edit_args(author, [], publication_mode='advance'))
    assert result['valid'] and result['proposal_id'] is None
    assert author.proposal is None and not author.root.exists()
    assert not review_readiness(author.guide_result['brief'])['ready']
    assert author.guide_result['brief'] == brief and author.guide_result['design_changed'] is False


@pytest.mark.asyncio
@pytest.mark.parametrize('initial', [False, True])
async def test_model_authority_overrides_are_rejected_on_initial_and_saved_turns(author, initial):
    activate(author, context(author.context, saved_plan_available=not initial))
    for field, value in [('brief', {}), ('design_changed', False), ('authoring', {}),
                         ('saved_plan_available', True), ('manual_plan_submitted', True),
                         ('guide_id', 'other'), ('guide_version', 100), ('evidence_messages', [])]:
        assert not (await author.publish_guide_edit(edit_args(author, rename(), **{field: value})))['valid']
    assert author.proposal is None and author.guide_result is None and not author.root.exists()


@pytest.mark.asyncio
async def test_grounded_initial_atomic_draft_preserves_original_source_and_provisional_readiness(author):
    activate(author, initial_context(author.context))
    original = copy.deepcopy((author.context, author.guide_context, author.guide_edit_turn['document']))
    args = edit_args(author, initial_edits('first-request'), publication_mode='advance', question=initial_question())
    untouched = copy.deepcopy(args)
    result = await author.publish_guide_edit(args)
    assert result['valid'], result
    assert result['status'] == 'staged_for_current_guided_turn' and result['checks']
    assert author.proposal['blueprint'] == Blueprint.model_validate(args['edits'][0]['value']).model_dump(mode='json')
    assert author.guide_result['brief'] == args['edits'][1]['value']
    assert author.guide_result['design_changed'] is True and author.guide_result['question'] == initial_question()
    assert not review_readiness(author.guide_result['brief'])['ready']
    assert (author.root / 'proposal.json').exists()
    assert (author.context, author.guide_context, author.guide_edit_turn['document']) == original and args == untouched


@pytest.mark.asyncio
@pytest.mark.parametrize('fault', ['compiler', 'missing_evidence', 'foreign_evidence', 'assistant_evidence',
                                  'unconfirmed_outcome', 'category', 'connection', 'mcp', 'reference'])
async def test_initial_atomic_draft_rejection_never_stages_half_of_the_publication(author, fault):
    value = initial_context(author.context, intent_only=fault == 'category')
    if fault == 'assistant_evidence':
        value['evidence_messages'].append({'id': 'assistant-opening', 'role': 'assistant', 'text': 'An opening suggestion.'})
        value['edit_base_token'] = guided_edit_token(value, author.context)
    activate(author, value)
    edits = initial_edits('first-request')
    draft, brief = edits[0]['value'], edits[1]['value']
    if fault == 'compiler':
        draft['topology']['entrypoint'] = 'missing:main'
    elif fault.endswith('evidence'):
        brief['facts']['outcome']['source_message_ids'] = ([] if fault == 'missing_evidence' else
            ['assistant-opening'] if fault == 'assistant_evidence' else ['foreign-request'])
    elif fault == 'unconfirmed_outcome':
        brief['facts']['outcome']['status'] = 'proposed'
    elif fault == 'connection':
        draft['connections'].append({'id': 'invented', 'name': 'Invented connection',
            'transport': 'stdio', 'command': 'made-up-package', 'args': []})
    elif fault == 'mcp':
        connection = {'id': 'known', 'name': 'Known MCP', 'transport': 'stdio', 'command': 'known-public-package', 'args': []}
        author.context['connections'] = [connection]
        activate(author, initial_context(author.context))
        draft['connections'] = [connection]
        draft['agent_classes'][0]['tools']['mcp'] = [{'connection_id': 'known', 'tool_names': ['invented_write'], 'usage': 'write'}]
    elif fault == 'reference':
        author.reference_read = False
    before = copy.deepcopy((author.context, author.guide_context))
    result = await author.publish_guide_edit(edit_args(author, edits, publication_mode='advance', question=initial_question()))
    assert not result['valid'], result
    assert author.proposal is None and author.guide_result is None and not author.root.exists()
    assert (author.context, author.guide_context) == before


@pytest.mark.asyncio
async def test_cache_write_failure_does_not_stage_either_half(author, monkeypatch):
    assert (await author.publish_guide_edit(edit_args(author, rename('Before failure'))))['valid']
    before = copy.deepcopy((author.proposal, author.guide_result))
    saved = (author.root / 'proposal.json').read_bytes()
    def fail(*args):
        raise OSError('Synthetic offline write failure')
    monkeypatch.setattr('launchkit.templates.storage_setup.write_private', fail)
    with pytest.raises(OSError):
        await author.publish_guide_edit(edit_args(author, rename('After failure')))
    assert (author.proposal, author.guide_result) == before and (author.root / 'proposal.json').read_bytes() == saved


def offline_admission(monkeypatch, author, callback):
    """Replace only native transport/start; keep actual message and callbacks."""
    author.runtime = SimpleNamespace(rust_http_base_url='http://offline.invalid', agent=lambda identity: None)
    monkeypatch.setattr(author, 'start', AsyncMock())
    class Response:
        def __init__(self, body):
            self.body = body
        def raise_for_status(self):
            pass
        def json(self):
            return self.body
    tool_frames = []
    async def request(client, method, url, **kwargs):
        if method == 'post':
            assert author.guide_edit_turn and author.message_lock.locked()
            results = await callback()
            results = results if isinstance(results, list) else [('publish_guide_edit', results)]
            for name, result in results:
                tool_frames.append({'kind': 'tool_result_received', 'identity': 'architect:main',
                    'interaction_id': 'offline-admission', 'source': {'kind': 'console_event'},
                    'cursor': f'offline:{len(tool_frames) + 2}',
                    'payload': {'name': name, 'is_error': False, 'result': json.dumps(result)}})
            return Response({'interaction_id': 'offline-admission', 'cursor': 'offline:1'})
        return Response({'frames': [*tool_frames, {'kind': 'interaction_complete', 'identity': 'architect:main',
            'interaction_id': 'offline-admission', 'source': {'kind': 'console_event'},
            'cursor': f'offline:{len(tool_frames) + 2}',
            'payload': {'result': 'Offline terminal projection.', 'source_event_type': 'run_completed'}}]})
    monkeypatch.setattr('launchkit.architect._native_request', request)


@pytest.mark.asyncio
@pytest.mark.parametrize('recipient_kind', ['identity', 'class'])
@pytest.mark.parametrize('next_question', [False, True])
async def test_unchanged_incoming_guide_reply_saves_without_reproposing_team(
        tmp_path, monkeypatch, recipient_kind, next_question):
    monkeypatch.setattr('launchkit.guide.planner_capability', lambda: {'available': True})
    author = ArchitectService(tmp_path / 'author')
    author.reference_read = True
    service = GuideService(GuideStore(tmp_path / 'guide'), author)
    state = await service.create()
    blueprint = starter_blueprint().model_dump(mode='json')
    if recipient_kind == 'class':
        next(item for item in blueprint['agent_classes'] if item['id'] == 'review')['instances'] = 0
        blueprint['flows'] = []
    state.update(proposal=blueprint, brief=incoming_brief(recipient_kind), stage='autonomy',
        question=default_question('autonomy'))
    state['messages'].append({'id': 'original-request', 'role': 'user',
        'text': 'Notice changes to the project documents.', 'action': 'chat', 'stage': 'purpose'})
    state = service.store.save(state, state['version'])
    original = copy.deepcopy(state)
    question = default_question('connections') if next_question else None
    seen = {}

    def unexpected_compile(args):
        pytest.fail('An unchanged source plan must not compile a replacement team')

    monkeypatch.setattr(author, '_check_candidate', unexpected_compile)

    async def callback():
        seen['ack'] = await author.publish_guide_edit(edit_args(author, [],
            publication_mode='advance', question=question, message='Collection still needs implementation.'))
        assert seen['ack']['valid'], seen['ack']
        return seen['ack']

    offline_admission(monkeypatch, author, callback)
    result = await service.respond(state['id'], GuideResponse(version=state['version'],
        action='chat', text='Explain the next setup step.'))
    saved = GuideStore(tmp_path / 'guide').get(state['id'])
    assert result['version'] == saved['version'] == original['version'] + 1
    assert saved['brief'] == original['brief']
    assert saved['proposal'] == original['proposal']
    assert saved['messages'][:-2] == original['messages']
    assert saved['messages'][-1]['text'] == 'Collection still needs implementation.'
    assert saved['question'] == (question if next_question else original['question'])
    assert not review_readiness(saved['brief'])['ready']
    assert seen['ack']['proposal_id'] is None
    assert author.proposal is None and not (author.root / 'proposal.json').exists()


@pytest.mark.asyncio
@pytest.mark.parametrize('recipient_kind', ['identity', 'class'])
@pytest.mark.parametrize('fault', ['no_saved_team', 'missing_recipient', 'foreign_evidence'])
async def test_unchanged_incoming_edit_still_checks_team_and_evidence(author, recipient_kind, fault):
    brief = incoming_brief(recipient_kind)
    if fault == 'missing_recipient':
        brief['incoming_data'][0]['routes'][0]['recipient'] = 'missing' if recipient_kind == 'class' else 'missing:main'
    elif fault == 'foreign_evidence':
        brief['incoming_data'][0]['source_message_ids'] = ['foreign-request']
    activate(author, context(author.context, brief=brief, saved_plan_available=fault != 'no_saved_team'))
    before = copy.deepcopy((author.context, author.guide_context, author.guide_edit_turn))
    result = await author.publish_guide_edit(edit_args(author, [],
        publication_mode='advance', question=default_question('connections')))
    assert result['valid'] is False
    expected = ('Propose the team' if fault == 'no_saved_team' else 'non-user evidence'
                if fault == 'foreign_evidence' else 'existing class' if recipient_kind == 'class' else 'existing identity')
    assert expected in result['issues']
    assert (author.context, author.guide_context, author.guide_edit_turn) == before
    assert author.proposal is None and author.guide_result is None and not author.root.exists()


@pytest.mark.asyncio
@pytest.mark.parametrize('turn_kind', ['advance', 'refinement', 'compiler', 'evidence', 'stale_token', 'authority'])
async def test_actual_first_guided_turn_uses_one_atomic_publication_and_saves_only_after_completion(tmp_path, monkeypatch, turn_kind):
    monkeypatch.setattr('launchkit.guide.planner_capability', lambda: {'available': True, 'provider': 'openai', 'reason': 'Offline fixture'})
    author = ArchitectService(tmp_path / 'author')
    author.reference_read = True
    service = GuideService(GuideStore(tmp_path / 'guide'), author)
    state = await service.create()
    original = copy.deepcopy(state)
    seen = {}
    monkeypatch.setattr(author, 'propose', AsyncMock(side_effect=AssertionError('The initial atomic path does not need a separate proposal tool')))
    monkeypatch.setattr(author, 'publish_guide_step', AsyncMock(side_effect=AssertionError('The initial atomic path does not need a separate publication tool')))

    async def callback():
        current = author.guide_context
        assert current['saved_plan_available'] is False and current['manual_plan_submitted'] is False
        assert current['guide_version'] == original['version'] and current['brief'] == original['brief']
        assert author.context['project']['description'] == 'Technical scaffolding, not a proposed user design.'
        edits = initial_edits(current['current_user_message']['id'])
        if turn_kind == 'compiler':
            edits[0]['value']['topology']['entrypoint'] = 'missing:main'
        elif turn_kind == 'evidence':
            edits[1]['value']['facts']['outcome']['source_message_ids'] = ['invented-source']
        args = edit_args(author, edits, publication_mode='refinement' if turn_kind == 'refinement' else 'advance',
            question=None if turn_kind == 'refinement' else initial_question(),
            message='Here is a first document comparison team. Who should it serve?', summary='An evidence-grounded first proposal.')
        if turn_kind == 'stale_token':
            args['base_token'] = '0' * 64
        elif turn_kind == 'authority':
            args['guide_version'] = original['version'] + 1
        seen.update(context=copy.deepcopy(current), draft=copy.deepcopy(edits[0]['value']))
        seen['ack'] = await author.publish_guide_edit(args)
        assert service.store.get(state['id']) == original, 'Atomic publication stages the pair, not a saved guide version'
        return seen['ack']

    offline_admission(monkeypatch, author, callback)
    request = GuideResponse(version=state['version'], action='chat',
        text='Compare the documents I provide, cite differences and unresolved claims.')
    if turn_kind in {'advance', 'refinement'}:
        saved = await service.respond(state['id'], request)
        assert seen['ack']['valid'], seen['ack']
        assert saved['version'] == original['version'] + 1
        assert saved['proposal'] == Blueprint.model_validate(seen['draft']).model_dump(mode='json')
        assert saved['blueprint'] is None and saved['source'] == 'agent'
        assert saved['status'] == ('refining' if turn_kind == 'refinement' else 'question')
        assert saved['question'] == (None if turn_kind == 'refinement' else initial_question())
        assert not saved['readiness']['ready'] and saved['brief']['facts']['audience']['status'] == 'unresolved'
        assert saved['messages'][:-2] == original['messages']
        assert saved['messages'][-2]['id'] == seen['context']['current_user_message']['id']
        assert saved['brief']['facts']['outcome']['source_message_ids'] == [saved['messages'][-2]['id']]
        assert saved['messages'][-1]['text'] == 'Here is a first document comparison team. Who should it serve?'
        with pytest.raises(ValueError):
            await service.apply(saved['id'], saved['version'])
    else:
        from launchkit.guide_models import GuidePublicationMissing
        with pytest.raises(GuidePublicationMissing):
            await service.respond(state['id'], request)
        assert not seen['ack']['valid'] and service.store.get(state['id']) == original
        assert 'publication_token' not in seen['ack']
        assert author.proposal is None and author.guide_result is None and not author.root.exists()
    assert author.guide_edit_turn is None
    author.propose.assert_not_called()
    author.publish_guide_step.assert_not_called()


@pytest.mark.asyncio
@pytest.mark.parametrize('exit_kind', ['terminal', 'start_failure', 'cancelled_start', 'cancelled_admission'])
async def test_actual_message_revokes_edit_authority_on_every_exit(author, monkeypatch, exit_kind):
    args = edit_args(author, rename())
    async def edit():
        ack = await author.publish_guide_edit(args)
        assert ack['valid']
        if exit_kind == 'cancelled_admission':
            raise asyncio.CancelledError
        return ack
    offline_admission(monkeypatch, author, edit)
    if exit_kind in {'start_failure', 'cancelled_start'}:
        monkeypatch.setattr(author, 'start', AsyncMock(side_effect=RuntimeError('Offline start failed')
            if exit_kind == 'start_failure' else asyncio.CancelledError))
    if exit_kind == 'terminal':
        result = await author.message('Offline guided turn', author.context, guide_context=author.guide_context)
        assert result['proposal']['project']['name'] == 'Revised document team'
    else:
        with pytest.raises(RuntimeError if exit_kind == 'start_failure' else asyncio.CancelledError):
            await author.message('Offline guided turn', author.context, guide_context=author.guide_context)
    assert author.guide_edit_turn is None
    assert author.guide_context is not None, 'Visualization retains its context independently'
    assert not (await author.publish_guide_edit(args))['valid']


@pytest.mark.asyncio
@pytest.mark.parametrize('late_kind', ['null_peer', 'other_interaction', 'after_terminal', 'missing_output'])
async def test_message_owns_publication_despite_later_peer_staging(author, monkeypatch, late_kind):
    import launchkit.architect as module
    async def publish():
        return await author.publish_guide_edit(edit_args(author, rename('The requested design'),
            message='The requested design is shown.'))
    offline_admission(monkeypatch, author, publish)
    original_request = module._native_request
    inspector = AsyncMock(side_effect=AssertionError('Session-wide output cannot identify this turn'))
    author.runtime.agent = lambda identity: SimpleNamespace(inspect=inspector)
    async def request(client, method, url, **kwargs):
        response = await original_request(client, method, url, **kwargs)
        if method == 'post':
            return response
        body = copy.deepcopy(response.json())
        late = await author.publish_guide_edit(edit_args(author, rename('A later peer design'),
            message='A later peer answer.'))
        assert late['valid']
        foreign = {'kind': 'tool_result_received', 'identity': 'architect:main',
            'interaction_id': 'offline-admission' if late_kind == 'after_terminal' else
                'other-interaction' if late_kind == 'other_interaction' else None,
            'source': {'kind': 'console_event'},
            'payload': {'name': 'publish_guide_edit', 'is_error': False, 'result': json.dumps(late)}}
        intermediate = {'kind': 'interaction_complete', 'identity': 'architect:main',
            'interaction_id': 'offline-admission', 'source': {'kind': 'session_history'},
            'payload': {'result': 'Still using tools.', 'message': {'stop_reason': 'tool_use'}}}
        wrong_terminal = {**intermediate, 'source': {'kind': 'console_event'},
            'payload': {'result': 'Still using tools.', 'source_event_type': 'turn_completed'}}
        owned, terminal = body['frames']
        if late_kind == 'missing_output':
            terminal['payload'].pop('result')
        body['frames'] = [intermediate, wrong_terminal, owned, terminal, foreign] if late_kind == 'after_terminal' else [
            intermediate, wrong_terminal, owned, foreign, terminal]
        return type(response)(body)
    monkeypatch.setattr(module, '_native_request', request)
    result = await author.message('Rename the design', author.context, guide_context=author.guide_context)
    assert result['proposal']['project']['name'] == 'The requested design'
    assert result['guide']['message'] == 'The requested design is shown.'
    assert author.proposal['blueprint']['project']['name'] == 'A later peer design'
    assert author.guide_result['message'] == 'A later peer answer.'
    if late_kind == 'missing_output':
        assert result['message'] == 'The requested design is shown.'
    inspector.assert_not_called()
    assert author.message_publications is None


@pytest.mark.asyncio
@pytest.mark.parametrize('missing_join', ['history', 'other_interaction', 'unknown_token', 'failed_result'])
async def test_guided_message_cannot_use_unjoined_mutable_publication(author, monkeypatch, missing_join):
    import launchkit.architect as module
    from launchkit.guide_models import GuidePublicationMissing
    async def publish():
        return await author.publish_guide_edit(edit_args(author, rename()))
    offline_admission(monkeypatch, author, publish)
    original_request = module._native_request
    async def request(client, method, url, **kwargs):
        response = await original_request(client, method, url, **kwargs)
        if method == 'post':
            return response
        body = copy.deepcopy(response.json())
        frame = body['frames'][0]
        if missing_join == 'history':
            frame['source']['kind'] = 'session_history'
        elif missing_join == 'other_interaction':
            frame['interaction_id'] = 'other-interaction'
        elif missing_join == 'unknown_token':
            result = json.loads(frame['payload']['result'])
            result['publication_token'] = 'not-host-issued'
            frame['payload']['result'] = json.dumps(result)
        else:
            frame['payload']['is_error'] = True
        return type(response)(body)
    monkeypatch.setattr(module, '_native_request', request)
    with pytest.raises(GuidePublicationMissing):
        await author.message('Rename the design', author.context, guide_context=author.guide_context)
    assert author.guide_result is not None, 'A staged value alone is not a turn-owned publication'
    assert author.pending_interaction is None and author.message_publications is None


@pytest.mark.asyncio
@pytest.mark.parametrize('turn_kind', ['localized', 'brief', 'operational', 'manual', 'cas_conflict', 'evidence_failure', 'mixed_full'])
async def test_actual_guided_save_keeps_cas_manual_scope_history_and_refinement(tmp_path, monkeypatch, turn_kind):
    monkeypatch.setattr('launchkit.guide.planner_capability', lambda: {'available': True, 'provider': 'openai', 'reason': 'Offline fixture'})
    author = ArchitectService(tmp_path / 'author')
    author.reference_read = True
    service = GuideService(GuideStore(tmp_path / 'guide'), author)
    state = await service.create()
    blueprint = starter_blueprint().model_dump(mode='json')
    state.update(proposal=blueprint, brief=complete_brief(), stage='autonomy', question=default_question('autonomy'),
        fact_history=[{'topic': 'inputs', 'fact': {'summary': 'An earlier source choice.', 'status': 'proposed',
            'source_message_ids': []}, 'obsolete': True, 'reason': 'revised', 'message_id': 'original-request'}])
    state['messages'].append({'id': 'original-request', 'role': 'user', 'text': 'Compare supplied documents.', 'action': 'chat', 'stage': 'purpose'})
    state = service.store.save(state, state['version'])
    original = copy.deepcopy(state)
    seen = {}
    async def callback():
        seen['context'] = copy.deepcopy(author.guide_context)
        seen['base'] = copy.deepcopy(author.context)
        edits = [] if turn_kind in {'manual', 'operational'} else rename()
        if turn_kind == 'brief':
            edits = [{'op': 'replace', 'path': '/brief/facts/audience', 'value': {
                'summary': 'Five colleagues use the comparison.', 'status': 'confirmed',
                'source_message_ids': [author.guide_context['current_user_message']['id']]}}]
        if turn_kind == 'evidence_failure':
            edits.append({'op': 'replace', 'path': '/brief/facts/outcome/source_message_ids', 'value': ['invented']})
        ack = await author.publish_guide_edit(edit_args(author, edits,
            publication_mode='advance' if turn_kind == 'operational' else 'refinement'))
        seen['ack'] = ack
        if turn_kind == 'mixed_full':
            replacement = copy.deepcopy(author.context)
            replacement['project']['name'] = 'A later full custom design'
            full_ack = await author.propose({'blueprint': replacement})
            assert full_ack['valid']
            return [('publish_guide_edit', ack), ('propose_blueprint', full_ack)]
        if turn_kind == 'cas_conflict':
            rival = service.store.get(state['id'])
            rival['summary'] = 'A newer independent saved tab.'
            seen['rival'] = service.store.save(rival, rival['version'])
        return ack
    offline_admission(monkeypatch, author, callback)
    manual = copy.deepcopy(blueprint)
    manual['agent_classes'][0]['instructions'] = 'Exact manual remit.\n  Keep this context.'
    request = GuideResponse(version=state['version'], action='manual' if turn_kind == 'manual' else 'chat',
        text='Review the exact manual remit.' if turn_kind == 'manual' else 'Five colleagues will use this comparison.'
            if turn_kind == 'brief' else 'Rename the system or explain the next step.',
        blueprint=manual if turn_kind == 'manual' else None)
    if turn_kind == 'cas_conflict':
        with pytest.raises(DraftConflict):
            await service.respond(state['id'], request)
        assert service.store.get(state['id']) == seen['rival']
    elif turn_kind in {'evidence_failure', 'mixed_full'}:
        from launchkit.guide_models import GuidePublicationMissing
        with pytest.raises(GuidePublicationMissing):
            await service.respond(state['id'], request)
        assert service.store.get(state['id']) == original
        if turn_kind == 'evidence_failure':
            assert author.proposal is None and not author.root.exists()
        else:
            assert author.proposal['blueprint']['project']['name'] == 'A later full custom design'
            assert author.guide_result is None, 'A terminal full replacement cannot reuse the previous edit publication'
    else:
        saved = await service.respond(state['id'], request)
        assert saved['version'] == original['version'] + 1
        assert saved['messages'][:-2] == original['messages']
        expected_brief = copy.deepcopy(original['brief'])
        if turn_kind == 'brief':
            expected_brief['facts']['audience'] = {'summary': 'Five colleagues use the comparison.', 'status': 'confirmed',
                'source_message_ids': [seen['context']['current_user_message']['id']]}
            assert saved['fact_history'][:-1] == original['fact_history']
            assert saved['fact_history'][-1]['fact'] == original['brief']['facts']['audience']
        else:
            assert saved['fact_history'] == original['fact_history']
        assert saved['brief'] == expected_brief
        assert saved['status'] == ('question' if turn_kind == 'operational' else 'refining')
        assert saved['stage'] == original['stage']
        expected = manual if turn_kind == 'manual' else copy.deepcopy(blueprint)
        if turn_kind == 'localized':
            expected['project']['name'] = 'Revised document team'
        assert saved['proposal'] == expected
        if turn_kind == 'manual':
            assert saved['manual_edits'][-1]['changed_paths'][0]['after'] == manual['agent_classes'][0]['instructions']
            assert seen['base'] == manual
        else:
            assert saved['manual_edits'] == original['manual_edits']
        assert seen['ack']['valid']
    current = seen['context']
    assert current['guide_version'] == original['version']
    assert current['edit_base_token'] == guided_edit_token(current, seen['base'])
    assert compact_guidance_context(current)['edit_base_token'] == current['edit_base_token']
    assert author.guide_edit_turn is None


@pytest.mark.asyncio
async def test_visualization_feedback_full_and_atomic_tools_keep_sidecar_outside_design(author):
    from launchkit.guide_models import GuidePublication, publication_visualization_feedback
    from launchkit.guided_edits import guided_edit_schema
    text = 'Keep this circular composition open to a custom arrangement.'
    document = copy.deepcopy(author.guide_edit_turn['document'])
    previous = {'text': 'Preserve the existing broad circular composition.', 'source_message_id': 'original-request'}
    author.guide_context['visualization_feedback'] = copy.deepcopy(previous)
    for update in ('omitted', None, '', text):
        changes = {} if update == 'omitted' else {'visualization_feedback': update}
        expected = previous if update in ('omitted', None) else None if update == '' else {
            'text': text, 'source_message_id': 'current-request'}
        full = {'question': None, 'message': 'This affects only the proposal animation.',
            'brief': copy.deepcopy(author.guide_context['brief']), **changes}
        assert (await author.publish_guide_step(full))['valid']
        assert publication_visualization_feedback(author.guide_result, author.guide_context) == expected
        args = edit_args(author, [], publication_mode='advance', **changes)
        result = await author.publish_guide_edit(args)
        assert result['valid'], result
        assert author.proposal is None
        assert publication_visualization_feedback(author.guide_result, author.guide_context) == expected
        assert author.guide_edit_turn['document'] == document
    assert guided_edit_schema()['properties']['visualization_feedback'] == GuidePublication.model_json_schema()['properties']['visualization_feedback']
    assert 'visualization_feedback' not in document['blueprint'] and 'visualization_feedback' not in document['brief']
