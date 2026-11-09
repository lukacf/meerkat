import asyncio
import copy
import json

import pytest
from fastapi.testclient import TestClient

from launchkit import server
from launchkit.guide import GUIDE_PROMPT, GuideService, compact_guidance_context, implementation_action_context
from launchkit.guide_models import (REVIEW_TOPICS, TOPIC_GROUPS, GuideQuestion, default_question,
                                   review_readiness, validate_guide_publication)
from launchkit.guide_store import GuideStore


def fact(summary, source, status='confirmed'):
    return {'summary': summary, 'status': status, 'source_message_ids': [source] if source else []}


def complete_brief(source):
    summaries = {
        'outcome': 'Compare two heat-pump proposals and cite their source text.',
        'audience': 'One person uses the comparison.', 'interfaces': 'MobKit console.',
        'ownership': 'One assistant owns the comparison and follow-up questions.',
        'inputs': 'Two UTF-8 proposal files placed in the workspace.',
        'workflow': 'The user launches each comparison manually.',
        'knowledge': 'Identity memory and internal notes; no knowledge service.',
        'authority': 'Internal notes may be written; no outside actions.',
        'destination': 'Local Mac.',
    }
    return {'facts': {topic: fact(summary, source) for topic, summary in summaries.items()}, 'requirements': []}


def test_core_context_can_be_ready_while_application_requirements_still_block_review():
    brief = complete_brief('request')
    brief['requirements'] = [{
        'id': 'staff_app', 'outcome': 'Staff use a separate authenticated web app.',
        'extension': 'Build an authenticated staff interface.', 'disposition': 'open',
        'source_message_ids': ['request'],
    }]
    readiness = review_readiness(brief)
    assert readiness['context_ready'] is True
    assert readiness['ready'] is False
    assert readiness['blockers'] == ['Implementation still required: Staff use a separate authenticated web app.']
    assert set(readiness['confirmed_topics']) == set(REVIEW_TOPICS)


@pytest.mark.parametrize('topic,status', [('outcome', 'proposed'), ('audience', 'unresolved'),
                                        ('destination', None)])
def test_missing_core_context_does_not_offer_a_ready_context(topic, status):
    brief = complete_brief('request')
    if status is None:
        del brief['facts'][topic]
    else:
        brief['facts'][topic]['status'] = status
    readiness = review_readiness(brief)
    assert readiness['context_ready'] is False and readiness['ready'] is False
    assert len(readiness['blockers']) == 1


def test_explicit_proposed_defaults_keep_the_existing_context_readiness_semantics():
    brief = complete_brief('request')
    brief['facts']['destination'] = fact('A local Mac is the proposed destination.', None, 'proposed')
    readiness = review_readiness(brief)
    assert readiness['context_ready'] is True and readiness['ready'] is True
    assert readiness['blockers'] == [] and readiness['proposed_topics'] == ['destination']


@pytest.mark.parametrize('kind,available', [('unresolved_authority', False), ('unplanned_feature', False),
                                          ('proposed_default', True), ('implementation_work', True)])
def test_build_action_uses_canonical_decisions_instead_of_cached_readiness(kind, available):
    from launchkit.blueprint import starter_blueprint
    brief = complete_brief('request')
    if kind == 'unresolved_authority':
        brief['facts']['authority']['status'] = 'unresolved'
    elif kind == 'proposed_default':
        brief['facts']['destination'] = fact('Propose a local Mac.', None, 'proposed')
    else:
        brief['requirements'] = [{'id': 'staff_interface', 'outcome': 'Staff use a separate interface.',
            'disposition': 'open', 'source_message_ids': ['request'],
            'extension': 'Implement the staff interface.' if kind == 'implementation_work' else ''}]
    state = {'proposal': starter_blueprint().model_dump(mode='json'), 'brief': brief,
             'readiness': {'ready': not available, 'decision_blockers': [] if not available else ['Stale decision.']}}
    actions = implementation_action_context(state, {'lifecycle': {'availability': 'absent', 'build_gate_clear': True}},
                                            {'available': True}, True)
    assert actions['build_available'] is available
    assert actions['decision_blockers'] == review_readiness(brief)['decision_blockers']
    assert bool(actions['unavailable_reason']) is not available
    if not available:
        assert all(issue in actions['unavailable_reason'] for issue in actions['decision_blockers'])


class SetupAgent:
    """Contract fixture. Behavioral assertions target persistence and evidence rules."""
    def __init__(self):
        self.calls, self.fail, self.reply = [], False, None

    async def message(self, prompt, blueprint, *, guide_context):
        self.calls.append({'prompt': prompt, 'blueprint': copy.deepcopy(blueprint), 'guide_context': copy.deepcopy(guide_context)})
        if self.fail:
            raise RuntimeError('Provider failed with private details')
        if self.reply:
            return self.reply(guide_context, copy.deepcopy(blueprint))
        brief, current = copy.deepcopy(guide_context['brief']), guide_context['current_user_message']
        if current['intent_only']:
            question = default_question('purpose')
            question.update(id='first_job', title='What is one job you want to hand over?',
                            description='A concrete example is enough.', options=[])
            proposal = None
        elif not brief['facts'].get('outcome'):
            brief['facts']['outcome'] = fact(current['text'], current['id'])
            question, proposal = default_question('connections'), blueprint
        elif guide_context['action'] == 'manual':
            question, proposal = default_question('connections'), blueprint
        else:
            question, proposal = default_question(guide_context['current_stage']), None
        return {'guide': {'message': 'Here is the next useful decision.', 'summary': '', 'warnings': [],
                          'question': question, 'brief': brief, 'design_changed': proposal is not None},
                'proposal': proposal, 'console_url': 'http://127.0.0.1:60123/console'}


@pytest.fixture
def guide_client(tmp_path, monkeypatch):
    from launchkit import guide as module
    agent = SetupAgent()
    monkeypatch.setattr(module, 'planner_capability', lambda: {'available': True, 'provider': 'openai', 'reason': 'Ready'})
    service = GuideService(GuideStore(tmp_path), agent)
    from launchkit.implementation import NoApplicationBuild
    def confirmed_absence(_state):
        raise NoApplicationBuild('No application build')
    service.implementation_evidence_provider = confirmed_absence
    monkeypatch.setattr(server, 'guide', service)
    # These tests assert saved interview semantics, not a 50 ms disk deadline.
    # Dedicated GuideOperations tests cover durable 202 admission/reconnection.
    from launchkit.guide_operations import GuideOperations
    monkeypatch.setattr(server, 'guide_operations', GuideOperations(wait_seconds=2))
    return TestClient(server.app), agent, service


def start(client):
    response = client.post('/api/guide/start', json={})
    assert response.status_code == 200, response.text
    return response.json()


def test_setup_creation_replay_preserves_one_saved_interview(guide_client):
    client, agent, service = guide_client
    request_id = 'cd20e4ba-618c-42e2-b40a-07d1628f1a53'
    created = client.post('/api/guide/start', json={'start_request_id': request_id})
    assert created.status_code == 200
    first = created.json()
    replay = client.post('/api/guide/start', json={'start_request_id': request_id})
    assert replay.status_code == 200
    assert replay.json() == first
    assert len(service.store.list()) == 1 and agent.calls == []
    # A later retry reads the saved conversation rather than its original opening.
    saved = service.store.save({**service.store.get(first['id']), 'summary': 'Keep my initial idea'}, 1)
    assert client.post('/api/guide/start', json={'start_request_id': request_id}).json()['summary'] == saved['summary']
    other = client.post('/api/guide/start', json={'start_request_id': '36d4f452-fc5d-4177-a3e9-3b162f439ccd'})
    assert other.status_code == 200 and other.json()['id'] != first['id']
    assert len(service.store.list()) == 2 and agent.calls == []


@pytest.mark.parametrize('value', ['not-an-id', '', 17, True, {'id': 'wrong'}, 'A' * 36])
def test_invalid_setup_creation_request_creates_no_interview(guide_client, value):
    client, agent, service = guide_client
    response = client.post('/api/guide/start', json={'start_request_id': value})
    assert response.status_code == 422
    assert service.store.list() == [] and agent.calls == []


def respond(client, state, **value):
    return client.post(f"/api/guide/{state['id']}/respond", json={'version': state['version'], **value})


def answer(client, state, text='Compare two heat-pump proposals with source references', **value):
    response = respond(client, state, text=text, **value)
    assert response.status_code == 200, response.text
    return response.json()


def review_reply(context, blueprint):
    blueprint['project']['name'] = 'Heat-pump comparison'
    return {'guide': {'question': None, 'message': 'Your comparison assistant is ready to review.',
                      'brief': complete_brief(context['current_user_message']['id']),
                      'design_changed': True, 'summary': 'One comparison assistant.', 'warnings': []},
            'proposal': blueprint}


def inspected_guide_turn(call, tmp_path):
    from launchkit.architect import ArchitectService
    architect = ArchitectService(tmp_path / 'inspection')
    architect.set_context(call['blueprint'])
    architect.guide_context = call['guide_context']
    architect.guide_edit_turn = {'token': call['guide_context']['edit_base_token']}
    return asyncio.run(architect.inspect({'include_schema': False}))


@pytest.mark.parametrize('reference', ['/my/local/report.pdf', 'https://example.test/report.png'])
def test_setup_input_surface_reaches_inspection_without_inventing_received_files(guide_client, tmp_path, reference):
    client, agent, service = guide_client
    request = f'Compare my evidence. The reference is {reference}.'
    state = answer(client, start(client), text=request)
    call = agent.calls[-1]
    context = call['guide_context']
    inspected = inspected_guide_turn(call, tmp_path)['guided_setup']
    expected = copy.deepcopy(context['input_surface'])
    assert inspected['input_surface'] == state['input_surface'] == expected
    assert expected['text'] is True and expected['offered_choices'] is True
    assert expected['file_uploads'] is True and expected['image_attachments'] is True
    assert expected['image_analysis'] is None
    assert expected['max_text_characters'] == server.GuideResponse.model_json_schema()['properties']['text']['maxLength']
    assert inspected['current_user_message']['text'] == request
    assert inspected['evidence_messages'][-1]['text'] == request
    assert not {'attachments', 'files', 'content', 'blob_id'} & inspected['current_user_message'].keys()
    assert not {'attachments', 'files', 'content', 'blob_id'} & inspected['evidence_messages'][-1].keys()
    inspected['input_surface']['file_uploads'] = False
    assert context['input_surface'] == expected
    assert 'input_surface' not in service.store.get(state['id'])


@pytest.mark.parametrize('image_analysis', [False, True])
def test_checked_native_image_capability_reaches_http_and_current_inspection(guide_client, tmp_path, image_analysis):
    from launchkit.guide_attachments import input_surface
    client, agent, service = guide_client
    current = {**input_surface(), 'image_analysis': image_analysis,
               'image_analysis_detail': 'Checked native model capability.'}
    agent.input_surface = lambda: current
    assert client.get('/api/guide/new').json()['input_surface'] == current
    state = start(client)
    assert state['input_surface'] == current
    assert client.get(f'/api/guide/{state["id"]}').json()['input_surface'] == current
    state = answer(client, state)
    call = agent.calls[-1]
    assert state['input_surface'] == current
    assert inspected_guide_turn(call, tmp_path)['guided_setup']['input_surface'] == current
    assert current['file_uploads'] is True and current['image_attachments'] is True
    call['guide_context']['input_surface']['image_analysis'] = None
    assert service.input_surface()['image_analysis'] is image_analysis
    assert 'input_surface' not in service.store.get(state['id'])


def test_setup_input_limits_do_not_remove_future_application_upload_requirement(guide_client, tmp_path):
    client, agent, service = guide_client
    def future_upload(context, blueprint):
        result = review_reply(context, blueprint)
        source = context['current_user_message']['id']
        result['guide']['question'] = default_question('connections')
        result['guide']['brief']['facts']['interfaces'] = fact('A custom application with file and image uploads.', source)
        result['guide']['brief']['requirements'] = [{
            'id': 'evidence_upload', 'outcome': 'People upload files and images in the application.',
            'extension': 'Build application upload controls.', 'disposition': 'open',
            'source_message_ids': [source]}]
        return result
    agent.reply = future_upload
    state = answer(client, start(client), text='The application must accept file and image uploads.')
    future_brief = copy.deepcopy(state['brief'])
    agent.reply = None
    state = answer(client, state, action='chat', text='What evidence can I provide in this setup chat?')
    assert state['brief'] == future_brief
    before = copy.deepcopy(service.store.get(state['id']))
    call = agent.calls[-1]
    inspected = inspected_guide_turn(call, tmp_path)['guided_setup']
    assert inspected['input_surface']['max_attachments'] == 4
    assert inspected['input_surface']['max_file_bytes'] == 8 * 1024 * 1024
    assert inspected['brief']['requirements'] == state['brief']['requirements']
    assert inspected['brief']['requirements'][0]['disposition'] == 'open'
    assert inspected['brief']['facts']['interfaces'] == state['brief']['facts']['interfaces']
    assert call['guide_context']['current_user_message']['text'] == 'What evidence can I provide in this setup chat?'
    assert any(row['text'] == 'The application must accept file and image uploads.'
               for row in inspected['evidence_messages'])
    assert service.store.get(state['id']) == before


@pytest.mark.parametrize('combine', [False, True])
def test_native_turn_sends_current_words_once_and_inspection_keeps_choice_evidence(guide_client, tmp_path, combine):
    from launchkit.guide_timeline import legacy_binding
    client, agent, service = guide_client
    state = answer(client, start(client))
    source = next(row['id'] for row in state['messages'] if row['role'] == 'user')
    state['brief'] = complete_brief(source)
    state['question'] = {
        'id': 'context_choice', 'stage': 'purpose', 'multiple': not combine,
        'title': 'Which support contexts should be retained?',
        'description': 'Choose private continuing context, a shared queue, or both.', 'allow_text': True,
        'options': [
            {'id': 'private', 'label': 'Shared support',
             'description': 'Share expertise while keeping each person\'s preferences and history private.'},
            {'id': 'queue', 'label': 'Shared support',
             'description': 'Keep the shared queue and its case history available to the support team.'},
        ],
    }
    state = service.store.save(state, state['version'])
    updated = answer(client, state, text='Keep private preferences separate from the shared queue.',
                     selected=['queue', 'private'], **({'combine_options': True} if combine else {}))
    call, context = agent.calls[-1], agent.calls[-1]['guide_context']
    outgoing = json.loads(call['prompt'].split('Current setup turn:\n', 1)[1])
    assert call['prompt'].startswith(GUIDE_PROMPT + '\n\n')
    assert set(outgoing) == {'guide_id', 'guide_version', 'action', 'current_user_message'}
    assert (outgoing['guide_id'], outgoing['guide_version'], outgoing['action']) == (state['id'], state['version'], 'answer')
    current = context['current_user_message']
    assert outgoing['current_user_message']['id'] == current['id']
    assert outgoing['current_user_message']['text'] == current['text']
    assert current['text'].endswith('Shared support; Shared support; Keep private preferences separate from the shared queue.')
    assert outgoing['current_user_message'].get('combine_options', False) is combine
    assert json.dumps(context['brief'], ensure_ascii=True) not in call['prompt']
    assert context['brief']['facts']['inputs']['summary'] not in call['prompt']
    assert context['edit_base_token'] not in call['prompt']

    inspection = inspected_guide_turn(call, tmp_path)
    guided = inspection['guided_setup']
    assert guided['brief'] == context['brief']
    assert guided['current_question'] == state['question']
    assert [row['id'] for row in guided['evidence_messages']] == [row['id'] for row in context['evidence_messages']]
    assert guided['answered_choices'][-1]['selected_options'] == [state['question']['options'][1], state['question']['options'][0]]
    assert guided['answered_choices'][-1].get('combine_options', False) is combine
    assert guided['implementation_actions'] == context['implementation_actions']
    assert guided['edit_base_token'] == context['edit_base_token']
    assert inspection['blueprint'] == call['blueprint']

    binding = legacy_binding({'kind': 'user_input', 'cursor': 'console:20', 'interaction_id': 'native-turn',
                              'payload': {'content': call['prompt']}}, updated)
    assert binding == {'interaction_id': 'native-turn', 'cursor': 'console:20',
                       'user_message_id': current['id'], 'user_text': current['text'], 'timestamp_ms': None}


def test_lean_manual_turn_keeps_full_changed_plan_and_rationale_in_inspection(guide_client, tmp_path):
    client, agent, _ = guide_client
    state = answer(client, start(client))
    submitted = copy.deepcopy(state['proposal'])
    submitted['project']['name'] = 'Continuing comparison team'
    submitted['agent_classes'][0]['instructions'] += ' Preserve each original source quote and its revision.'
    rationale = 'Keep original quotes and explain corrections without replacing their history.'
    answer(client, state, action='manual', blueprint=submitted, text=rationale)
    call, context = agent.calls[-1], agent.calls[-1]['guide_context']
    outgoing = json.loads(call['prompt'].split('Current setup turn:\n', 1)[1])
    assert outgoing['current_user_message']['text'] == rationale
    assert outgoing['action'] == 'manual'
    assert not {'brief', 'evidence_messages', 'manual_edits', 'manual_changes', 'edit_base_token'} & outgoing.keys()
    assert json.dumps(submitted, ensure_ascii=True) not in call['prompt']
    inspection = inspected_guide_turn(call, tmp_path)
    guided = inspection['guided_setup']
    assert inspection['blueprint'] == submitted
    assert guided['manual_plan_submitted'] is True
    assert guided['manual_changes'] == ['Project updated', 'Agent responsibilities updated']
    assert guided['manual_edits'][-1] == context['manual_edits'][-1]
    assert guided['manual_edits'][-1]['rationale'] == rationale
    assert {row['path'] for row in guided['manual_edits'][-1]['changed_paths']} == {
        '/project/name', '/agent_classes/0/instructions'}
    assert guided['brief'] == context['brief']
    assert guided['edit_base_token'] == context['edit_base_token']


def test_recovered_advanced_intent_uses_current_plan_and_exact_human_delta(guide_client, tmp_path):
    client, agent, service = guide_client
    state = answer(client, start(client))
    base = copy.deepcopy(state['proposal'])
    intended = copy.deepcopy(base)
    intended['project']['name'] = 'My deliberate observing crew'
    intended['agent_classes'][0]['instructions'] = 'Keep this exact human observing caveat. ' + 'Synthetic evidence only. ' * 300
    # A real intervening guide publication advances the authority and changes
    # unrelated fields while the human's separate draft still has its old base.
    def completed_reply(context, blueprint):
        blueprint['project']['description'] = 'Newer guide-owned observing purpose.'
        blueprint['agent_classes'][0]['description'] = 'Newer guide-owned responsibility.'
        return {'guide': {'message': 'The current guide proposal is revised.', 'summary': '', 'warnings': [],
                          'question': default_question('connections'), 'brief': context['brief'],
                          'design_changed': True}, 'proposal': blueprint}
    agent.reply = completed_reply
    current = answer(client, state, action='chat', text='Refine the current responsibilities.')
    agent.reply = None
    current_plan = copy.deepcopy(current['proposal'])
    rationale = 'Reconcile my exact edits with the latest plan; explain conflicts.'
    result = answer(client, current, action='manual', text=rationale,
                    manual_intent={'base': base, 'draft': intended})
    call, context = agent.calls[-1], agent.calls[-1]['guide_context']
    assert call['blueprint'] == current_plan
    assert context['manual_plan_submitted'] is False
    edit = context['manual_edits'][-1]
    assert edit['review_against_current_plan'] is True and edit['rationale'] == rationale
    assert edit['changed_paths'] == [
        {'path': '/agent_classes/0/instructions', 'before': base['agent_classes'][0]['instructions'],
         'after': intended['agent_classes'][0]['instructions'].strip()},
        {'path': '/project/name', 'before': base['project']['name'], 'after': intended['project']['name']},
    ]
    # Blueprint's established whitespace normalization applies to guide context;
    # the durable request still owns the exact raw human intent snapshot.
    assert intended['agent_classes'][0]['instructions'].endswith(' ')
    assert service.store.operation(current['id'])['request']['manual_intent'] == {'base': base, 'draft': intended}
    inspected = inspected_guide_turn(call, tmp_path)
    assert inspected['blueprint'] == current_plan
    assert inspected['guided_setup']['manual_edits'][-1] == edit
    assert result['blueprint'] == current['blueprint'] and result['proposal'] == current_plan
    assert result['proposal']['project']['description'] != intended['project']['description']
    assert result['proposal']['agent_classes'][0]['description'] != intended['agent_classes'][0]['description']
    assert result['messages'][-2]['manual_edit'] == edit
    assert len(edit['changed_paths'][0]['after']) > 6000


@pytest.mark.parametrize('failure', ['provider', 'stale_version', 'invalid_draft', 'no_changes'])
def test_advanced_intent_refusal_preserves_current_saved_authority(guide_client, failure):
    client, agent, service = guide_client
    state = answer(client, start(client))
    before = copy.deepcopy(service.store.get(state['id']))
    previous_operation = copy.deepcopy(service.store.operation(state['id']))
    base, intended = copy.deepcopy(state['proposal']), copy.deepcopy(state['proposal'])
    intended['project']['description'] = 'An exact human purpose for deliberate review.'
    version = state['version']
    if failure == 'provider':
        agent.fail = True
    elif failure == 'stale_version':
        version -= 1
    elif failure == 'invalid_draft':
        intended['agent_classes'][0]['instances'] = True
    else:
        intended = copy.deepcopy(base)
    response = client.post(f"/api/guide/{state['id']}/respond", json={
        'version': version, 'action': 'manual', 'text': 'Review these edits.',
        'manual_intent': {'base': base, 'draft': intended}})
    assert response.status_code == (502 if failure == 'provider' else 409 if failure == 'stale_version' else 422), response.text
    assert service.store.get(state['id']) == before
    if failure != 'provider':
        assert len(agent.calls) == 1
        assert service.store.operation(state['id']) == previous_operation
    else:
        assert agent.calls[-1]['blueprint'] == state['proposal']


@pytest.mark.parametrize('kind', ['chat', 'mixed', 'extra'])
def test_manual_intent_transport_refuses_ambiguous_or_nonmanual_requests(guide_client, kind):
    client, agent, service = guide_client
    state = answer(client, start(client))
    before = copy.deepcopy(service.store.get(state['id']))
    intent = {'base': state['proposal'], 'draft': state['proposal']}
    body = {'version': state['version'], 'action': 'manual', 'manual_intent': intent}
    if kind == 'chat':
        body.update(action='chat', text='Discuss the proposal.')
    elif kind == 'mixed':
        body['blueprint'] = state['proposal']
    else:
        intent['current_plan'] = state['proposal']
    response = client.post(f"/api/guide/{state['id']}/respond", json=body)
    assert response.status_code == 422, response.text
    assert len(agent.calls) == 1 and service.store.get(state['id']) == before


def test_absent_manual_intent_preserves_existing_request_serialization_and_replay(guide_client):
    client, agent, _ = guide_client
    state = answer(client, start(client))
    body = {'version': state['version'], 'action': 'manual', 'selected': [], 'text': 'Keep this purpose.',
            'stage': None, 'blueprint': copy.deepcopy(state['proposal'])}
    body['blueprint']['project']['description'] = 'A deliberately edited current purpose.'
    checked = server.GuideResponse.model_validate(body)
    assert checked.model_dump(mode='json') == body
    ordinary = server.GuideResponse(version=state['version'], action='chat', text='Discuss the current plan.')
    assert ordinary.model_dump(mode='json') == {'version': state['version'], 'action': 'chat', 'selected': [],
                                               'text': 'Discuss the current plan.', 'stage': None, 'blueprint': None}
    first = respond(client, state, **{key: value for key, value in body.items() if key != 'version'})
    second = respond(client, state, **{key: value for key, value in body.items() if key != 'version'})
    assert first.status_code == second.status_code == 200
    assert first.json() == second.json() and len(agent.calls) == 2


def test_deliberate_advanced_intent_replays_exactly_without_another_agent_turn(guide_client):
    client, agent, service = guide_client
    state = answer(client, start(client))
    base = copy.deepcopy(state['proposal'])
    intended = copy.deepcopy(base)
    intended['project']['description'] = 'Exact synthetic intent that remains a separate draft.'
    # Valid user DTOs may omit defaults. Validate before admission while retaining
    # the exact submitted snapshot for browser completion ownership.
    intended['agent_classes'][0].pop('members')
    intended['agent_classes'][0]['tools'].pop('schedule')
    intent = {'base': base, 'draft': intended}
    first = respond(client, state, action='manual', text='Reconcile my edits.', manual_intent=intent)
    second = respond(client, state, action='manual', text='Reconcile my edits.', manual_intent=intent)
    assert first.status_code == second.status_code == 200
    assert first.json() == second.json() and len(agent.calls) == 2
    assert agent.calls[-1]['blueprint'] == base
    assert service.store.operation(state['id'])['request']['manual_intent'] == intent


@pytest.mark.parametrize('checker,available', [('sandbox-exec', True), (None, False)])
def test_guide_receives_actual_build_action_without_inventing_execution(guide_client, monkeypatch, checker, available):
    client, agent, _ = guide_client
    monkeypatch.setattr('launchkit.implementation_workspace._sandbox_executable', lambda: checker)
    agent.reply = review_reply
    state = answer(client, start(client), text='A full concrete brief with all deployment decisions.')
    answer(client, state, action='chat', text='What can I do next?')
    actions = agent.calls[-1]['guide_context']['implementation_actions']
    assert actions['saved_proposal_available'] is True
    assert actions['build_available'] is available
    assert actions['build_action'] == 'Build application'
    assert bool(actions['unavailable_reason']) is not available
    projected = compact_guidance_context(agent.calls[-1]['guide_context'])
    assert projected['implementation_actions'] == actions
    assert 'only prepare a handoff' in agent.calls[-1]['prompt']
    assert 'after this design turn is saved' in actions['admission']


def test_host_capability_is_not_a_build_action_before_a_proposal_is_saved(guide_client, monkeypatch):
    client, agent, _ = guide_client
    monkeypatch.setattr('launchkit.implementation_workspace._sandbox_executable', lambda: 'sandbox-exec')
    answer(client, start(client))
    actions = agent.calls[-1]['guide_context']['implementation_actions']
    assert actions['host_build_capable'] is True
    assert actions['saved_proposal_available'] is False
    assert actions['build_available'] is False
    assert 'Save a concrete proposed agent system first' in actions['unavailable_reason']


def test_current_trial_observations_reach_next_guide_turn_without_persisting_or_polling(guide_client):
    from launchkit.projects import fingerprint
    client, agent, service = guide_client
    state = answer(client, start(client))
    calls = []

    async def provider(detached):
        calls.append(detached)
        return {"current": True, "guide_id": detached["id"], "guide_version": detached["version"],
            "blueprint_hash": fingerprint(detached["proposal"]), "brief_hash": fingerprint(detached["brief"]),
            "mode": "application", "status": "completed", "rehearsal_id": "trial",
            "application_source": {"build_id": "build", "source_digest": "a" * 64},
            "application_observations": [{"kind": "callback_returned", "identity": "librarian:main",
                "name": "propose", "result": {"status": "awaiting-human-confirmation"}}]}

    service.trial_evidence_provider = provider
    assert client.get(f"/api/guide/{state['id']}").status_code == 200
    assert not calls
    answer(client, state, text="The sample only staged proposals. What is missing?")
    context = agent.calls[-1]["guide_context"]
    assert context["trial_evidence"]["observations"][0]["result_status"] == "awaiting-human-confirmation"
    assert compact_guidance_context(context)["trial_evidence"] == context["trial_evidence"]
    assert context["trial_evidence"]["completed_turn_is_behavior_success"] is False
    assert len(calls) == 1 and "trial_evidence" not in service.store.get(state["id"])


def test_trial_provider_failure_does_not_leak_private_details_or_block_interview(guide_client):
    client, agent, service = guide_client
    def unavailable(_state):
        raise RuntimeError("secret local host path")
    service.trial_evidence_provider = unavailable
    state = answer(client, start(client))
    evidence = agent.calls[-1]["guide_context"]["trial_evidence"]
    assert evidence["freshness"] == "unavailable" and evidence["observations"] == []
    assert "secret local host path" not in str(evidence)
    assert state["version"] > 1


def test_owned_installation_context_reaches_turn_without_polling_or_persisting(guide_client):
    from launchkit.projects import fingerprint
    client, agent, service = guide_client
    state = answer(client, start(client))
    calls = []

    async def provider(detached):
        calls.append(copy.deepcopy(detached))
        detached["brief"]["facts"].clear()
        return {"guide_id": state["id"], "guide_version": state["version"], "current": True,
            "blueprint_hash": fingerprint(state["proposal"]), "deployment_id": "owned-install",
            "build_id": "installed-build", "source_digest": "a" * 64, "source_verified": True,
            "status": "stopped", "destination": "/private/folder", "settings": {"KEY": "secret"}}

    service.deployment_evidence_provider = provider
    assert client.get(f"/api/guide/{state['id']}").status_code == 200
    assert not calls
    answer(client, state, text="It is installed and stopped. What should we improve next?")
    context = agent.calls[-1]["guide_context"]
    evidence = context["deployment_evidence"]
    assert evidence["current"] and evidence["owned_source_verified"] and evidence["status"] == "stopped"
    assert not evidence["native_runtime_available"] and not evidence["live_ready"]
    assert compact_guidance_context(context)["deployment_evidence"] == evidence
    assert len(calls) == 1 and "deployment_evidence" not in service.store.get(state["id"])
    assert context["brief"]["facts"] and "secret" not in str(evidence) and "/private/folder" not in str(evidence)


def test_installation_observer_failure_does_not_block_repair_or_leak_private_values(guide_client):
    client, agent, service = guide_client
    def unavailable(_state):
        raise RuntimeError("private deployment credential")
    service.deployment_evidence_provider = unavailable
    state = answer(client, start(client))
    evidence = agent.calls[-1]["guide_context"]["deployment_evidence"]
    assert evidence["freshness"] == "unavailable" and not evidence["owned_source_verified"]
    assert "private deployment credential" not in str(evidence) and state["version"] > 1


def test_remote_installation_context_reaches_interview_without_local_receipt_polling_or_persistence(guide_client):
    client, agent, service = guide_client
    state = answer(client, start(client))
    calls = []
    async def provider(detached):
        calls.append(copy.deepcopy(detached))
        assert "deployment_evidence" not in detached and "remote_deployment_evidence" not in detached
        detached["brief"]["facts"].clear()
        return {"guide_id": state["id"], "guide_version": state["version"],
            "deployment_id": "22222222-1234-1234-1234-123456789abc",
            "build_id": "11111111-1234-1234-1234-123456789abc",
            "accepted_source_digest": "a" * 64, "package_sha256": "b" * 64,
            "transferred_source_digest": "c" * 64, "controller_sha256": "d" * 64,
            "owned_package_verified": True, "source_binding_current": True,
            "observation_fresh": True, "observation": {"available": True},
            "remote_admission_requested": True, "admitted": True, "status": "stopped",
            "last_known_status": "stopped", "worker_live": False,
            "endpoint": {"host": "private-host", "user": "private-user"},
            "destination": "/private/folder", "settings": {"KEY": "secret-token"}}
    service.remote_deployment_evidence_provider = provider
    assert client.get(f"/api/guide/{state['id']}").status_code == 200
    assert not calls
    answer(client, state, text="It is installed remotely and stopped. What is next?")
    context = agent.calls[-1]["guide_context"]
    evidence = context["remote_deployment_evidence"]
    assert context["deployment_evidence"]["freshness"] == "missing"
    assert evidence["current"] and evidence["owned_source_verified"]
    assert evidence["status"] == "stopped" and evidence["installation_admitted"]
    assert not evidence["native_runtime_available"] and not evidence["live_ready"]
    assert compact_guidance_context(context)["remote_deployment_evidence"] == evidence
    assert len(calls) == 1 and context["brief"]["facts"]
    saved = service.store.get(state["id"])
    assert "deployment_evidence" not in saved and "remote_deployment_evidence" not in saved
    for private in ("private-host", "private-user", "/private/folder", "secret-token"):
        assert private not in str(evidence)
    assert "missing local installation does not mean the remote" in GUIDE_PROMPT


@pytest.mark.parametrize("failed_lane", ["local", "remote"])
def test_installation_observer_lanes_fail_independently_without_private_error_leakage(guide_client, failed_lane):
    from launchkit.projects import fingerprint
    client, agent, service = guide_client
    state = answer(client, start(client))
    def unavailable(_state): raise RuntimeError("private-observer-secret")
    def local(_state):
        return {"guide_id": state["id"], "guide_version": state["version"], "current": True,
            "blueprint_hash": fingerprint(state["proposal"]), "deployment_id": "local-install",
            "build_id": "local-build", "source_digest": "e" * 64, "source_verified": True,
            "status": "stopped"}
    def remote(_state):
        return {"guide_id": state["id"], "guide_version": state["version"],
            "deployment_id": "22222222-1234-1234-1234-123456789abc",
            "build_id": "11111111-1234-1234-1234-123456789abc",
            "accepted_source_digest": "a" * 64, "package_sha256": "b" * 64,
            "transferred_source_digest": "c" * 64, "controller_sha256": "d" * 64,
            "owned_package_verified": True, "source_binding_current": True,
            "observation_fresh": True, "observation": {"available": True}, "status": "stopped"}
    service.deployment_evidence_provider = unavailable if failed_lane == "local" else local
    service.remote_deployment_evidence_provider = unavailable if failed_lane == "remote" else remote
    answer(client, state, text="Keep my saved installation and tell me the next useful step.")
    context = agent.calls[-1]["guide_context"]
    failed = context["deployment_evidence" if failed_lane == "local" else "remote_deployment_evidence"]
    other = context["remote_deployment_evidence" if failed_lane == "local" else "deployment_evidence"]
    assert failed["freshness"] == "unavailable" and not failed["owned_source_verified"]
    assert other["owned_source_verified"] and other["status"] == "stopped"
    assert "private-observer-secret" not in str(context)


def test_server_wires_remote_interview_observer_with_detached_state(monkeypatch):
    calls = []
    async def observer(identity, *, state):
        calls.append((identity, state))
        return {"owned": "inert-observer"}
    monkeypatch.setattr(server.remote_deployment, "guide_status", observer)
    state = {"id": "guide-identity"}
    assert asyncio.run(server.guide.remote_deployment_evidence_provider(state)) == {"owned": "inert-observer"}
    assert calls == [(state["id"], state)]


def test_compact_guidance_keeps_provenance_choices_and_manual_intent():
    question = default_question('connections')
    question['description'] = 'Repeated presentation detail. ' * 500
    first = {'id': 'request', 'role': 'user', 'text': 'Use Slack and my local text notes.',
             'intent_only': False, 'question': question}
    decision = {'id': 'choice', 'role': 'user', 'text': 'Local text files; Slack',
                'intent_only': False, 'question': question}
    edits = [{'message_id': 'choice', 'rationale': 'Keep this owner name.',
              'changed_paths': [{'path': '/agent_classes/0/name', 'before': 'Assistant', 'after': 'House owner'}]}]
    context = {'action': 'answer', 'brief': {}, 'current_user_message': decision,
               'evidence_messages': [first, decision], 'conversation': [first, decision],
               'answers': [{'message_id': 'choice', 'question': question, 'selected': ['files', 'slack']}],
               'manual_edits': edits, 'manual_choices': {'large_snapshot': 'x' * 10000}}
    before = copy.deepcopy(context)
    projected = compact_guidance_context(context)
    assert context == before
    assert projected['answered_choices'][0]['selected'] == ['Local text files', 'Slack']
    assert projected['manual_edits'] == edits
    assert [(row['id'], row['text'], row['intent_only']) for row in projected['evidence_messages']] == [
        ('request', first['text'], False), ('choice', decision['text'], False)]
    publication = {'question': default_question('purpose'), 'message': 'Which context should be retained?',
                   'brief': {'facts': {'outcome': fact('Understand household decisions.', 'request')},
                             'requirements': [{'id': 'channel', 'outcome': 'Slack chat',
                                               'source_message_ids': ['request'], 'decision_message_ids': ['choice'],
                                               'disposition': 'cancelled'}]}}
    assert validate_guide_publication(publication, projected, None) == validate_guide_publication(publication, context, None)
    assert 'manual_choices' not in projected
    assert 'description' not in projected['current_user_message']['question']


def test_starts_with_one_question_and_no_assumed_architecture(guide_client):
    client, agent, _ = guide_client
    state = client.get('/api/guide/new').json()
    assert state['id'] is None and state['stage'] == 'purpose'
    assert state['question']['options'] == []
    assert state['question']['title'] == 'What do you want to build?'
    assert state['blueprint'] is None and state['proposal'] is None
    assert state['messages'][0]['question'] == state['question']
    assert state['brief'] == {'facts': {}, 'requirements': []}
    assert not state['readiness']['ready'] and agent.calls == []


def test_context_multiselect_keeps_combined_interfaces_and_user_evidence(guide_client):
    client, agent, service = guide_client
    state = start(client)
    question = {
        'id': 'support_interfaces', 'stage': 'purpose', 'multiple': True,
        'title': 'How should your staff use the support assistant?',
        'description': 'Choose the surfaces that fit your team.', 'allow_text': True,
        'options': [
            {'id': 'console', 'label': 'Admin console', 'description': 'Inspect the team and review work.'},
            {'id': 'slack', 'label': 'Slack', 'description': 'Staff ask from their existing channel.'},
            {'id': 'custom', 'label': 'Custom web app', 'description': 'Give staff a dedicated interface.'},
        ],
    }
    state['question'] = question
    state['messages'].append({'role': 'user', 'text': 'An internal support assistant for our staff.', 'id': 'support-brief'})
    state = service.store.save(state, state['version'])

    def record_surfaces(context, blueprint):
        current = context['current_user_message']
        requirements = [{
            'id': choice['id'], 'outcome': choice['label'],
            'disposition': 'supported' if choice['id'] == 'console' else 'open',
            'extension': '' if choice['id'] == 'console' else 'Implement its authenticated app adapter.',
            'source_message_ids': [current['id']],
        } for choice in question['options']]
        return {'guide': {
            'message': 'All three surfaces belong in the intended experience.',
            'question': default_question('destination'),
            'brief': {'facts': {'interfaces': fact(current['text'], current['id'])},
                      'requirements': requirements},
        }, 'proposal': None}

    agent.reply = record_surfaces
    updated = answer(client, state, text='Staff may use either, I use the console.',
                     selected=['console', 'slack', 'custom'])
    context = agent.calls[-1]['guide_context']
    assert context['current_user_message']['selected'] == ['console', 'slack', 'custom']
    assert context['current_user_message']['text'] == 'Admin console; Slack; Custom web app; Staff may use either, I use the console.'
    assert context['current_user_message']['intent_only'] is False
    assert updated['answers'][-1]['selected'] == ['console', 'slack', 'custom']
    assert {item['id'] for item in updated['brief']['requirements']} == {'console', 'slack', 'custom'}
    assert updated['brief']['facts']['interfaces']['source_message_ids'] == [context['current_user_message']['id']]
    assert service.store.get(updated['id'])['brief'] == updated['brief']
    assert updated['proposal'] is None and not updated['readiness']['ready']


def legacy_category_start(client, service):
    state = start(client)
    question = {**state["question"], "options": [
        {"id": "custom", "label": "Custom", "description": "Something of my own"},
        {"id": "personal", "label": "Personal", "description": "Personal assistance"},
    ]}
    # An existing pre-redesign question still validates an already selected answer.
    state["question"] = question
    state["messages"].append({"role": "user", "text": "I am considering something custom", "id": "legacy-user"})
    return service.store.save(state, state["version"])


def test_bare_category_does_not_create_team_or_count_as_concrete_outcome(guide_client):
    client, agent, service = guide_client
    state = answer(client, legacy_category_start(client, service), text='', selected=['custom'])
    assert state['stage'] == 'purpose' and state['proposal'] is None
    assert state['question']['options'] == [] and not state['completed_stages']
    assert agent.calls[-1]['guide_context']['current_user_message']['intent_only']
    state = answer(client, state)
    assert state['brief']['facts']['outcome']['status'] == 'confirmed' and state['proposal'] is not None


def test_misleading_category_confirmation_is_rejected_atomically(guide_client):
    client, agent, service = guide_client
    agent.reply = review_reply
    initial = legacy_category_start(client, service)
    response = respond(client, initial, selected=['custom'])
    assert response.status_code == 422 and 'category' in response.text
    saved = client.get(f"/api/guide/{initial['id']}").json()
    assert saved['version'] == initial['version'] and saved['messages'] == initial['messages']


def test_rich_brief_can_reach_review_without_five_redundant_questions(guide_client):
    client, agent, _ = guide_client
    agent.reply = review_reply
    state = answer(client, start(client), text='A full concrete brief with all deployment decisions.')
    assert state['stage'] == 'review' and state['status'] == 'review'
    assert len(state['answers']) == 1 and state['readiness']['ready']
    assert set(state['brief']['facts']) == set(REVIEW_TOPICS) and state['blueprint'] is None
    applied = client.post(f"/api/guide/{state['id']}/apply", json={'version': state['version']})
    assert applied.status_code == 200, applied.text
    assert applied.json()['status'] == 'ready'
    assert applied.json()['blueprint']['project']['name'] == 'Heat-pump comparison' and len(agent.calls) == 1


def refinement_reply(context, blueprint):
    brief = copy.deepcopy(context['brief']) if context['brief']['facts'] else complete_brief(context['current_user_message']['id'])
    brief['requirements'] = brief['requirements'] or [{
        'id': 'staff_app', 'outcome': 'Staff use a separate authenticated web app.',
        'extension': 'Build the staff interface.', 'disposition': 'open',
        'source_message_ids': [context['current_user_message']['id']],
    }]
    brief['facts']['audience'] = fact('Fourteen staff share the existing system.', context['current_user_message']['id'])
    return {'guide': {'publication_mode': 'refinement', 'question': None,
        'message': 'The correction is saved. The staff interface remains application work to build.',
        'brief': brief, 'design_changed': True}, 'proposal': blueprint}


def test_initial_refinement_saves_a_draft_without_forcing_question_or_final_review(guide_client):
    client, agent, service = guide_client
    agent.reply = refinement_reply
    state = answer(client, start(client), text='Fourteen staff need a shared comparison helper and a staff UI.')
    assert state['status'] == 'refining' and state['question'] is None and state['stage'] == 'purpose'
    assert state['proposal'] and state['blueprint'] is None
    assert state['readiness']['context_ready'] is True and state['readiness']['ready'] is False
    assert state['readiness']['decision_blockers'] == []
    assert state['readiness']['implementation_blockers'] == ['Implementation still required: Staff use a separate authenticated web app.']
    assert state['messages'][-1]['question'] is None
    saved = service.store.get(state['id'])
    restored = client.get(f"/api/guide/{state['id']}").json()
    assert restored['status'] == 'refining' and restored['question'] is None
    assert restored['brief'] == saved['brief'] and restored['proposal'] == saved['proposal']
    assert all(message.get('action') != 'apply' for message in restored['messages'])
    assert len(agent.calls) == 1


@pytest.mark.parametrize('prior_status', ['question', 'review', 'ready'])
@pytest.mark.parametrize('action', ['chat', 'manual'])
def test_refinement_correction_rebinds_full_application_scope_without_plan_acceptance(guide_client, prior_status, action):
    from launchkit.artifact_binding import inputs, matches
    client, agent, service = guide_client
    agent.reply = review_reply
    state = answer(client, start(client))
    if prior_status == 'ready':
        response = client.post(f"/api/guide/{state['id']}/apply", json={'version': state['version']})
        assert response.status_code == 200
        state = response.json()
    elif prior_status == 'question':
        state = service.store.save({**state, 'status': 'question', 'stage': 'autonomy',
                                   'question': default_question('autonomy')}, state['version'])
    before = copy.deepcopy(state)
    old_receipt = {'guide_id': state['id'], **inputs(state)}
    assert matches(old_receipt, state)
    agent.reply = refinement_reply
    extra = {}
    if action == 'manual':
        changed = copy.deepcopy(state['proposal'] or state['blueprint'])
        changed['project']['description'] = 'Staff compare and discuss the original source proposals.'
        extra['blueprint'] = changed
    response = respond(client, state, action=action, text='Fourteen staff will share it. Save this correction and keep refining.', **extra)
    assert response.status_code == 200, response.text
    revised = response.json()
    assert revised['status'] == 'refining' and revised['question'] is None
    assert revised['version'] == before['version'] + 1
    assert revised['stage'] == before['stage']
    assert revised['blueprint'] == (changed if action == 'manual' else before['blueprint'])
    assert revised['proposal']
    assert revised['brief']['facts']['audience']['source_message_ids'] == [revised['messages'][-2]['id']]
    assert all(revised['brief']['facts'][topic] == item for topic, item in before['brief']['facts'].items() if topic != 'audience')
    assert revised['fact_history'][-1]['topic'] == 'audience' and revised['fact_history'][-1]['obsolete'] is True
    assert matches(old_receipt, revised) is False
    assert revised['implementation_evidence']['acceptance_current'] is False
    assert revised['implementation_evidence']['ready_to_package'] is False
    assert [message for message in revised['messages'] if message.get('action') == 'apply'] == [
        message for message in before['messages'] if message.get('action') == 'apply']
    if action == 'manual':
        assert revised['proposal']['project']['description'] == changed['project']['description']
        assert revised['manual_edits'][-1]['message_id'] == revised['messages'][-2]['id']
    projected = compact_guidance_context(agent.calls[-1]['guide_context'])
    assert projected['readiness'] == review_readiness(before['brief'])
    assert 'publication_mode:"refinement"' in agent.calls[-1]['prompt']


def ready_refinement(client, agent):
    def reply(context, blueprint):
        result = review_reply(context, blueprint)
        result['guide']['publication_mode'] = 'refinement'
        result['guide']['brief']['facts']['destination'] = fact('Propose a local Mac.', None, 'proposed')
        return result
    agent.reply = reply
    return answer(client, start(client))


def test_ready_refinement_accepts_explicitly_without_an_extra_agent_turn(guide_client):
    client, agent, service = guide_client
    state = ready_refinement(client, agent)
    assert state['readiness']['ready'] is True and state['status'] == 'refining'
    assert state['question'] is None and state['blueprint'] is None
    assert service.store.get(state['id'])['status'] == 'refining'
    response = client.post(f"/api/guide/{state['id']}/apply", json={'version': state['version']})
    assert response.status_code == 200, response.text
    accepted = response.json()
    assert len(agent.calls) == 1
    assert accepted['version'] == state['version'] + 1
    assert accepted['status'] == 'ready' and accepted['blueprint'] == state['proposal']
    assert accepted['proposal'] is None and accepted['readiness']['ready'] is True
    assert accepted['messages'][:-2] == state['messages']
    acceptance = accepted['messages'][-2]
    assert acceptance['role'] == 'user' and acceptance['action'] == 'apply'
    assert acceptance['text'] == 'I accept this setup and its proposed defaults.'
    for topic, previous in state['brief']['facts'].items():
        current = accepted['brief']['facts'][topic]
        if previous['status'] == 'proposed':
            assert current == {**previous, 'status': 'confirmed',
                               'source_message_ids': [*previous['source_message_ids'], acceptance['id']]}
        else:
            assert current == previous


@pytest.mark.parametrize('prior_status', ['refining', 'review'])
def test_explicit_plan_acceptance_retains_unimplemented_requirements_on_reload(guide_client, prior_status):
    client, agent, service = guide_client
    def reply(context, blueprint):
        result = refinement_reply(context, blueprint)
        result['guide']['brief']['facts']['destination'] = fact('Propose a local Mac.', None, 'proposed')
        result['guide']['brief']['requirements'].append({
            'id': 'restart_records', 'outcome': 'Staff records survive a restart.',
            'extension': 'Build durable transactions and test restart recovery.', 'disposition': 'open',
            'source_message_ids': [context['current_user_message']['id']],
        })
        return result
    agent.reply = reply
    state = answer(client, start(client), text='Staff need an authenticated interface and reliable records.')
    state['status'] = prior_status
    # Stale pessimistic readiness cannot replace the actual decision evidence.
    state['readiness'] = {'ready': False, 'context_ready': False, 'decision_blockers': ['Stale decision.']}
    state = service.store.save(state, state['version'])
    before = copy.deepcopy(service.store.get(state['id']))
    draft = client.get(f"/api/guide/{state['id']}").json()
    assert draft['status'] == prior_status and draft['question'] is None
    assert draft['readiness']['context_ready'] is True and draft['readiness']['decision_blockers'] == []
    assert draft['readiness']['ready'] is False and len(draft['readiness']['implementation_blockers']) == 2
    assert draft['brief'] == before['brief'] and draft['proposal'] == before['proposal']
    assert service.store.get(state['id']) == before
    assert draft['brief']['facts']['destination']['status'] == 'proposed' and len(agent.calls) == 1

    response = client.post(f"/api/guide/{state['id']}/apply", json={'version': state['version']})
    assert response.status_code == 200, response.text
    accepted = response.json()
    assert len(agent.calls) == 1 and accepted['version'] == state['version'] + 1
    assert accepted['status'] == 'ready' and accepted['question'] is None
    assert accepted['blueprint'] == before['proposal'] and accepted['proposal'] is None
    assert accepted['brief']['requirements'] == before['brief']['requirements']
    assert all(row['disposition'] == 'open' and row['decision_message_ids'] == [] for row in accepted['brief']['requirements'])
    assert accepted['readiness']['context_ready'] is True and accepted['readiness']['decision_blockers'] == []
    assert accepted['readiness']['ready'] is False and len(accepted['readiness']['implementation_blockers']) == 2
    assert accepted['implementation_evidence']['status'] == 'not-built'
    assert accepted['implementation_evidence']['ready_to_package'] is False
    assert accepted['messages'][:-2] == before['messages']
    acceptance = accepted['messages'][-2]
    assert acceptance['role'] == 'user' and acceptance['action'] == 'apply'
    assert acceptance['text'] == 'I accept this setup and its proposed defaults.'
    for topic, previous in before['brief']['facts'].items():
        current = accepted['brief']['facts'][topic]
        assert current == ({**previous, 'status': 'confirmed',
                            'source_message_ids': [*previous['source_message_ids'], acceptance['id']]}
                           if previous['status'] == 'proposed' else previous)
    assert 'Your plan is accepted.' in accepted['messages'][-1]['text']
    assert 'not implemented yet' in accepted['messages'][-1]['text']
    assert 'ready to package' not in accepted['messages'][-1]['text']
    restored = client.get(f"/api/guide/{state['id']}").json()
    for key in ('version', 'status', 'question', 'blueprint', 'proposal', 'brief', 'messages', 'readiness'):
        assert restored[key] == accepted[key]
    assert len(agent.calls) == 1


@pytest.mark.parametrize('blocked', ['question', 'review_question', 'status', 'proposal', 'incomplete', 'decision', 'deferral', 'invalid'])
def test_refinement_acceptance_rechecks_actual_state_and_canonical_brief(guide_client, blocked):
    client, agent, service = guide_client
    state = ready_refinement(client, agent)
    if blocked in {'question', 'review_question'}:
        state['question'] = default_question('destination')
        if blocked == 'review_question':
            state['status'] = 'review'
    elif blocked == 'status':
        state['status'] = 'question'
    elif blocked == 'proposal':
        state['proposal'] = None
    elif blocked == 'incomplete':
        del state['brief']['facts']['authority']
    elif blocked == 'invalid':
        state['brief']['facts']['authority']['status'] = 'invented'
    elif blocked == 'decision':
        state['brief']['requirements'] = [{'id': 'staff_interface', 'outcome': 'Staff use a separate interface.',
            'disposition': 'open', 'source_message_ids': [state['messages'][-2]['id']], 'extension': ''}]
    elif blocked == 'deferral':
        from launchkit.guide_models import conversation_message
        decision = conversation_message('user', 'Defer the staff interface until after the console first version.',
                                        stage='purpose', action='chat')
        state['brief']['requirements'] = [{'id': 'staff_interface', 'outcome': 'Staff use a separate interface.',
            'disposition': 'deferred', 'source_message_ids': [state['messages'][-2]['id']],
            'extension': 'Implement the staff interface.', 'decision_message_ids': [decision['id']]}]
        state['brief']['facts']['first_version'] = fact('Propose a console-only first version.', None, 'proposed')
        state['messages'].append(decision)
    if blocked in {'decision', 'deferral'}:
        canonical = review_readiness(state['brief'])
        assert canonical['context_ready'] is True and canonical['decision_blockers']
    # A saved readiness projection cannot authorize acceptance of changed facts.
    state['readiness'] = {'ready': True, 'context_ready': True, 'decision_blockers': [], 'implementation_blockers': []}
    state = service.store.save(state, state['version'])
    before = service.store.get(state['id'])
    response = client.post(f"/api/guide/{state['id']}/apply", json={'version': state['version']})
    assert response.status_code == 422, response.text
    assert service.store.get(state['id']) == before
    assert len(agent.calls) == 1


def test_running_operation_blocks_ready_refinement_before_respond_holds_service_lock(guide_client):
    client, agent, service = guide_client
    state = ready_refinement(client, agent)
    before = service.store.get(state['id'])
    operation = {'status': 'running', 'version': state['version'], 'request_id': 'owned-pending-answer'}
    service.store.save_operation(state['id'], operation)
    assert service.lock.locked() is False
    response = client.post(f"/api/guide/{state['id']}/apply", json={'version': state['version']})
    assert response.status_code == 503, response.text
    assert service.store.get(state['id']) == before
    assert service.store.operation(state['id']) == operation
    assert len(agent.calls) == 1


def test_refinement_acceptance_keeps_the_exact_saved_version_fence(guide_client):
    client, agent, service = guide_client
    state = ready_refinement(client, agent)
    before = service.store.get(state['id'])
    response = client.post(f"/api/guide/{state['id']}/apply", json={'version': state['version'] - 1})
    assert response.status_code == 409, response.text
    assert service.store.get(state['id']) == before
    assert len(agent.calls) == 1


def test_next_question_may_target_any_material_group_without_new_design(guide_client):
    client, agent, _ = guide_client
    state = answer(client, start(client))
    def cloud_question(context, blueprint):
        return {'guide': {'message': 'Which cloud project should we prepare?', 'question': default_question('destination'),
                          'brief': context['brief']}, 'proposal': None}
    agent.reply = cloud_question
    updated = answer(client, state, text='Use Google Cloud; other details later.')
    assert updated['stage'] == 'destination' and updated['proposal'] == state['proposal'] and updated['changes'] == []


@pytest.mark.parametrize('saved_location', ['proposal', 'blueprint'])
def test_operational_status_publication_saves_answer_without_changing_application_scope(guide_client, saved_location):
    client, agent, service = guide_client
    state = answer(client, start(client))
    source = next(message['id'] for message in state['messages'] if message['role'] == 'user')
    state['brief'] = complete_brief(source)
    state['brief']['requirements'] = [{
        'id': name, 'outcome': outcome, 'disposition': 'open',
        'extension': 'Independent application evidence is still needed.',
        'source_message_ids': [source],
    } for name, outcome in [('console', 'Keep the admin Console.'),
                            ('slack', 'Staff use Slack.'), ('custom', 'Staff also use a custom UI.')]]
    state['brief']['facts']['interfaces']['summary'] = 'Admin Console, staff Slack and custom UI.'
    state['readiness'] = review_readiness(state['brief'])
    if saved_location == 'blueprint':
        state['blueprint'], state['proposal'] = state['proposal'], None
    state = service.store.save(state, state['version'])
    state = client.get(f"/api/guide/{state['id']}").json()
    before = copy.deepcopy(state)
    message = 'The installation is stopped. Open Manage application when you want to start it. No real job is verified.'

    def operational_reply(context, blueprint):
        assert context['saved_plan_available'] is True
        assert context['current_status'] == before['status']
        assert context['current_question'] == before['question']
        assert blueprint == before[saved_location]
        return {'guide': {'question': None, 'message': message, 'brief': copy.deepcopy(context['brief']),
                          'design_changed': False, 'summary': 'Review the stopped installation.',
                          'warnings': ['Runtime startup is separate from your job working.']},
                'proposal': None}

    agent.reply = operational_reply
    response = respond(client, state, action='chat', text='Review current progress. What is next?')
    assert response.status_code == 200, response.text
    saved = response.json()
    for key in ('brief', 'blueprint', 'proposal', 'readiness', 'stage', 'status', 'answers',
                'manual_edits', 'fact_history', 'reopened_topics', 'completed_stages'):
        assert saved[key] == before[key], key
    # Operational chat does not answer the pending decision.
    assert saved['question'] == before['question'] and not saved['readiness']['ready']
    assert saved['version'] == before['version'] + 1
    assert saved['messages'][:-2] == before['messages']
    assert saved['messages'][-2]['text'] == 'Review current progress. What is next?'
    assert saved['messages'][-1]['text'] == message
    assert saved['messages'][-1]['stage'] == before['stage']
    assert saved['messages'][-1]['question'] is None
    assert saved['summary'] == 'Review the stopped installation.' and saved['changes'] == []
    assert service.store.get(state['id'])['brief'] == before['brief']
    projected = compact_guidance_context(agent.calls[-1]['guide_context'])
    assert projected['current_status'] == before['status']
    assert projected['current_question'] == before['question'] and projected['saved_plan_available'] is True
    assert 'Every completed guided turn must call publish_guide_step or publish_guide_edit' in agent.calls[-1]['prompt']


@pytest.mark.parametrize('change', ['fact', 'requirement', 'no_saved_plan', 'design_changed', 'manual', 'answer', 'help', 'revisit'])
def test_status_null_cannot_bypass_changed_scope_or_final_review_contract(change):
    brief = complete_brief('request')
    brief['requirements'] = [{'id': 'custom_ui', 'outcome': 'Keep the custom UI.',
                              'disposition': 'open', 'source_message_ids': ['request']}]
    context = {'action': 'chat', 'saved_plan_available': True, 'brief': copy.deepcopy(brief),
               'evidence_messages': [{'id': 'request', 'role': 'user', 'text': 'Keep my custom UI.'}]}
    publication = {'question': None, 'message': 'An operational next step.',
                   'brief': copy.deepcopy(brief), 'design_changed': False}
    if change == 'fact':
        publication['brief']['facts']['destination']['summary'] = 'A different machine.'
    elif change == 'requirement':
        publication['brief']['requirements'][0]['disposition'] = 'supported'
    elif change == 'no_saved_plan':
        context['saved_plan_available'] = False
    elif change == 'design_changed':
        publication['design_changed'] = True
    else:
        context['action'] = change
    before = copy.deepcopy((publication, context))
    with pytest.raises(ValueError):
        validate_guide_publication(publication, context, None)
    assert (publication, context) == before


def test_null_fresh_proposal_still_requires_normal_final_review_readiness():
    brief = complete_brief('request')
    brief['requirements'] = [{'id': 'custom_ui', 'outcome': 'Keep the custom UI.',
                              'disposition': 'open', 'source_message_ids': ['request']}]
    context = {'action': 'chat', 'saved_plan_available': True, 'brief': copy.deepcopy(brief),
               'evidence_messages': [{'id': 'request', 'role': 'user', 'text': 'Keep my custom UI.'}]}
    publication = {'question': None, 'message': 'Ready for review.', 'brief': brief, 'design_changed': False}
    with pytest.raises(ValueError, match='not ready for review'):
        validate_guide_publication(publication, context, {'project': {'name': 'A fresh design'}})


@pytest.mark.parametrize('changed', [True, False])
def test_mixed_chat_and_submitted_blueprint_cannot_save_a_status_only_response(guide_client, changed):
    client, agent, service = guide_client
    state = answer(client, start(client))
    submitted = copy.deepcopy(state['proposal'])
    if changed:
        submitted['project']['name'] = 'An edited technical plan'
    before = copy.deepcopy(service.store.get(state['id']))

    def operational_reply(context, blueprint):
        assert context['action'] == 'chat' and context['saved_plan_available'] is True
        assert context['manual_plan_submitted'] is True
        return {'guide': {'question': None, 'message': 'Only an operational status.',
                          'brief': copy.deepcopy(context['brief']), 'design_changed': False}, 'proposal': None}

    agent.reply = operational_reply
    response = respond(client, state, action='chat', text='Review this saved progress.', blueprint=submitted)
    assert response.status_code == 422
    assert service.store.get(state['id']) == before
    assert compact_guidance_context(agent.calls[-1]['guide_context'])['manual_plan_submitted'] is True


def test_native_prose_without_publication_has_safe_typed_failure_and_preserves_setup(guide_client):
    client, agent, service = guide_client
    state = answer(client, start(client))
    agent.reply = lambda context, blueprint: {'message': 'Private output is not a publication.', 'proposal': None}
    response = respond(client, state, action='chat', text='What is next?')
    assert response.status_code == 422
    saved = client.get(f"/api/guide/{state['id']}").json()
    assert saved['version'] == state['version'] and saved['messages'] == state['messages']
    assert saved['brief'] == state['brief'] and saved['proposal'] == state['proposal']
    assert saved['operation']['status'] == 'failed'
    assert saved['operation']['failure'] == {'phase': 'publication', 'category': 'publication_missing'}
    assert saved['operation']['request']['text'] == 'What is next?'
    assert 'Private output' not in str(saved['operation']) and 'did not save' in saved['operation']['detail']


def test_fact_sources_must_be_actual_user_messages(guide_client):
    client, agent, _ = guide_client
    initial = start(client)
    def fabricated(context, blueprint):
        result = review_reply(context, blueprint)
        result['guide']['brief']['facts']['destination']['source_message_ids'] = [initial['messages'][0]['id']]
        return result
    agent.reply = fabricated
    response = respond(client, initial, text='Compare heat pumps.')
    assert response.status_code == 422 and 'non-user' in response.text
    assert client.get(f"/api/guide/{initial['id']}").json()['version'] == initial['version']


def test_open_requested_feature_blocks_review_and_cannot_disappear(guide_client):
    client, agent, _ = guide_client
    def open_gap(context, blueprint):
        result = review_reply(context, blueprint)
        result['guide']['question'] = default_question('purpose')
        result['guide']['brief']['requirements'] = [{
            'id': 'staff_app', 'outcome': 'Staff use a separate authenticated web app.',
            'supported_subset': 'Operator console.', 'extension': 'Build an authenticated staff interface.',
            'disposition': 'open', 'source_message_ids': [context['current_user_message']['id']],
        }]
        return result
    agent.reply = open_gap
    state = answer(client, start(client), text='Staff need their own web app.')
    assert not state['readiness']['ready']
    assert state['readiness']['context_ready'] is True
    agent.reply = review_reply
    response = respond(client, state, text='Tell me more.')
    assert response.status_code == 422 and 'drop a requested feature' in response.text
    def unresolved_review(context, blueprint):
        return {'guide': {'message': 'Ready.', 'question': None, 'brief': context['brief']}, 'proposal': blueprint}
    agent.reply = unresolved_review
    response = respond(client, state, text='Please review.')
    assert response.status_code == 422 and 'not ready' in response.text


def test_deferral_needs_later_user_choice(guide_client):
    client, agent, _ = guide_client
    def silently_defer(context, blueprint):
        result = review_reply(context, blueprint)
        source = context['current_user_message']['id']
        result['guide']['brief']['requirements'] = [{
            'id': 'staff_app', 'outcome': 'Staff web app.', 'extension': 'Build an interface.',
            'disposition': 'deferred', 'source_message_ids': [source], 'decision_message_ids': [source],
        }]
        result['guide']['brief']['facts']['first_version'] = fact('Console only.', source)
        return result
    agent.reply = silently_defer
    response = respond(client, start(client), text='Build a staff web app and a console.')
    assert response.status_code == 422 and 'cannot authorize its deferral' in response.text


def test_initial_later_phase_and_partial_support_keep_scope_open_without_authorizing_deferral():
    """Captured publication repairs preserve scope instead of weakening evidence rules."""
    source = 'initial-request'
    brief = complete_brief(source)
    brief['facts']['first_version'] = fact('Local exported text initially; live sources later.', source)
    brief['requirements'] = [
        {'id': 'local_evidence', 'outcome': 'Review exported evidence with immutable provenance.',
         'supported_subset': 'Workspace UTF-8 text reads.',
         'extension': 'Build validated intake and an immutable evidence manifest.',
         'disposition': 'open', 'source_message_ids': [source]},
        {'id': 'live_sources', 'outcome': 'Connect live sources later.',
         'extension': 'Later source-specific read-only ingestion and authentication.',
         'disposition': 'open', 'source_message_ids': [source]},
    ]
    context = {'brief': {}, 'evidence_messages': [
        {'id': source, 'role': 'user', 'text': 'Review exported text now; connect live sources later.'}],
        'current_user_message': {'id': source, 'intent_only': False}}
    publication = {'message': 'The early design preserves both phases.',
                   'question': default_question('autonomy'), 'brief': brief}

    partial_as_supported = copy.deepcopy(publication)
    partial_as_supported['brief']['requirements'][0]['disposition'] = 'supported'
    with pytest.raises(ValueError, match='Missing application work'):
        validate_guide_publication(partial_as_supported, context, None)

    initial_as_deferral = copy.deepcopy(publication)
    initial_as_deferral['brief']['requirements'][1].update(
        disposition='deferred', decision_message_ids=[source])
    with pytest.raises(ValueError, match='cannot authorize its deferral'):
        validate_guide_publication(initial_as_deferral, context, None)

    accepted = validate_guide_publication(publication, context, None)
    assert accepted['brief']['facts']['first_version'] == brief['facts']['first_version']
    assert [item['disposition'] for item in accepted['brief']['requirements']] == ['open', 'open']
    assert all(item['decision_message_ids'] == [] for item in accepted['brief']['requirements'])
    assert accepted['brief']['requirements'][0]['supported_subset'] == 'Workspace UTF-8 text reads.'
    assert 'Later source-specific' in accepted['brief']['requirements'][1]['extension']
    assert review_readiness(accepted['brief'])['context_ready'] is True
    assert review_readiness(accepted['brief'])['ready'] is False


def test_plan_updates_require_concrete_outcome_but_questions_do_not(guide_client):
    client, agent, _ = guide_client
    def premature(context, blueprint):
        return {'guide': {'question': default_question('connections'), 'message': 'Here is your team.',
                          'brief': {'facts': {}, 'requirements': []}, 'design_changed': True}, 'proposal': blueprint}
    agent.reply = premature
    state = start(client)
    response = respond(client, state, text='I am not sure yet.')
    assert response.status_code == 422 and 'concrete outcome' in response.text
    def clarify(context, blueprint):
        result = premature(context, blueprint)
        result['proposal'] = None
        result['guide']['design_changed'] = False
        result['guide']['question'] = default_question('purpose')
        result['guide']['question'].update(id='first_job', options=[], title='Which job takes up your time?')
        return result
    agent.reply = clarify
    state = answer(client, state, text='I am not sure yet.')
    assert state['proposal'] is None and state['stage'] == 'purpose'


def test_manual_feedback_tracks_changed_fields_without_freezing_old_defaults(guide_client):
    client, agent, _ = guide_client
    state = answer(client, start(client))
    blueprint = copy.deepcopy(state['proposal'])
    blueprint['project']['name'] = 'My deliberate name'
    result = answer(client, state, action='manual', blueprint=blueprint, text='Keep this project name.')
    context = agent.calls[-1]['guide_context']
    assert context['manual_edits'][-1]['changed_paths'] == [{
        'path': '/project/name', 'before': state['proposal']['project']['name'], 'after': 'My deliberate name'}]
    assert context['manual_edits'][-1]['rationale'] == 'Keep this project name.'
    assert not context['preserve_manual_choices'] and result['blueprint'] == blueprint
    assert result['messages'][-2]['action'] == 'manual'


def test_revisit_preserves_plan_and_independent_facts(guide_client):
    client, agent, _ = guide_client
    agent.reply = review_reply
    state = answer(client, start(client), text='A complete brief.')
    previous_plan = copy.deepcopy(state['proposal'])
    destination = copy.deepcopy(state['brief']['facts']['destination'])
    inputs = copy.deepcopy(state['brief']['facts']['inputs'])
    history = copy.deepcopy(state['fact_history'])
    response = respond(client, state, action='back', stage='connections')
    assert response.status_code == 200, response.text
    state = response.json()
    assert state['stage'] == 'connections' and state['status'] == 'question'
    assert state['proposal'] == previous_plan and state['brief']['facts']['destination'] == destination
    # Reopening a topic does not retract the user's supplied information.
    assert state['brief']['facts']['inputs'] == inputs
    assert state['reopened_topics'] == ['inputs'] and len(state['answers']) == 1
    assert state['fact_history'] == history and state['manual_choices'] is None
    assert 'destination' in state['completed_stages']


@pytest.mark.parametrize('group', list(TOPIC_GROUPS))
@pytest.mark.parametrize('case,inputs', [
    ('astrophotography', 'Site forecast, horizon constraints and retained observing-session evidence.'),
    ('household', 'Household calendars, school notices and each family member\'s private preferences.'),
    ('multi_user_team', 'Customer case evidence with staff assignments and private customer context.'),
    ('temporary_fleet', 'Batch evidence partitions for temporary investigations; repository context persists.'),
])
def test_revisit_uses_saved_context_without_vendor_choices_or_new_native_turn(guide_client, group, case, inputs):
    """Back changes interview focus, not the saved design or its manual intent."""
    client, agent, service = guide_client

    def contextual_reply(context, blueprint):
        brief = copy.deepcopy(context['brief'])
        if not brief['facts']:
            brief = complete_brief(context['current_user_message']['id'])
            brief['facts']['outcome']['summary'] = f'Explore the {case} system and its responsibilities.'
            brief['facts']['inputs']['summary'] = inputs
        return {'guide': {'question': None, 'message': 'The current scope is recorded.',
                          'brief': brief, 'design_changed': True}, 'proposal': blueprint}

    agent.reply = contextual_reply
    state = answer(client, start(client), text=f'Explore the {case} system using {inputs}')
    manual = copy.deepcopy(state['proposal'])
    manual['project']['name'] = f'My {case} system'
    state = answer(client, state, action='manual', blueprint=manual,
                   text='Keep this project name while we revisit the scope.')
    before = copy.deepcopy(state)
    call_count = len(agent.calls)
    response = respond(client, state, action='back', stage=group)
    assert response.status_code == 200, response.text
    after = response.json()
    topic = TOPIC_GROUPS[group][0]
    question = GuideQuestion.model_validate(after['question'])
    assert before['brief']['facts'][topic]['summary'] in question.description
    assert question.options == [] and question.multiple is False
    assert len(agent.calls) == call_count
    assert after['proposal'] == before['proposal'] and after['blueprint'] == before['blueprint']
    assert after['manual_choices'] == before['manual_choices']
    assert after['manual_edits'] == before['manual_edits']
    assert after['brief']['requirements'] == before['brief']['requirements']
    # Navigation preserves evidence; a later checked publication owns revisions.
    assert after['brief']['facts'] == before['brief']['facts']
    assert after['fact_history'] == before['fact_history']
    assert service.store.get(after['id'])['question'] == after['question']


def test_correction_replaces_fact_and_records_obsolete_evidence(guide_client):
    client, agent, _ = guide_client
    agent.reply = review_reply
    state = answer(client, start(client), text='A full brief.')
    original_source = state['brief']['facts']['destination']['source_message_ids']
    def correct(context, blueprint):
        brief = copy.deepcopy(context['brief'])
        brief['facts']['destination'] = fact('Linux server.', context['current_user_message']['id'])
        blueprint['target'] = 'linux'
        return {'guide': {'message': 'I have changed the destination to Linux.', 'question': None,
                          'brief': brief, 'design_changed': True}, 'proposal': blueprint}
    agent.reply = correct
    state = answer(client, state, action='chat', text='Actually use my Linux server.')
    assert state['brief']['facts']['destination']['summary'] == 'Linux server.'
    assert state['brief']['facts']['destination']['source_message_ids'] != original_source
    history = state['fact_history'][-1]
    assert history['topic'] == 'destination' and history['fact']['summary'] == 'Local Mac.'
    assert client.get(f"/api/guide/{state['id']}").json()['brief'] == state['brief']


def test_changed_plan_requires_new_proposal(guide_client):
    client, agent, _ = guide_client
    initial = start(client)
    def missing(context, blueprint):
        result = review_reply(context, blueprint)
        result['guide']['question'], result['proposal'] = default_question('knowledge'), None
        return result
    agent.reply = missing
    response = respond(client, initial, text='A concrete job.')
    assert response.status_code == 422 and 'propose the changed plan' in response.text
    assert client.get(f"/api/guide/{initial['id']}").json()['version'] == initial['version']


def test_failed_agent_and_stale_answer_do_not_replace_state(guide_client):
    client, agent, _ = guide_client
    state = start(client)
    agent.fail = True
    response = respond(client, state, text='Organize documents.')
    assert response.status_code == 502 and 'private details' not in response.text
    assert client.get(f"/api/guide/{state['id']}").json()['version'] == state['version']
    agent.fail = False
    latest = answer(client, state)
    assert respond(client, state, text='Stale answer.').status_code == 409
    assert client.get(f"/api/guide/{state['id']}").json()['version'] == latest['version']


def test_unavailable_agent_preserves_answer(guide_client, monkeypatch):
    from launchkit import guide as module
    client, agent, _ = guide_client
    state = start(client)
    monkeypatch.setattr(module, 'planner_capability', lambda: {'available': False})
    assert respond(client, state, text='Help me plan.').status_code == 503 and agent.calls == []
    assert client.get(f"/api/guide/{state['id']}").json()['answers'] == []


def test_history_and_plan_survive_explanation_chat_and_reload(guide_client):
    client, agent, _ = guide_client
    initial = start(client)
    opening = copy.deepcopy(initial['messages'][0])
    state = answer(client, initial)
    updated = answer(client, state, action='chat', text='What are my options?')
    assert updated['proposal'] == state['proposal'] and updated['changes'] == []
    assert updated['messages'][0] == opening
    assert updated['messages'][-1]['question'] == updated['question']
    reloaded = client.get(f"/api/guide/{state['id']}").json()
    assert reloaded['messages'] == updated['messages']
    assert len({message['id'] for message in reloaded['messages']}) == len(reloaded['messages'])


def test_acceptance_confirms_visible_proposed_defaults(guide_client):
    client, agent, _ = guide_client
    def defaults(context, blueprint):
        result = review_reply(context, blueprint)
        result['guide']['brief']['facts']['knowledge'] = fact('Internal notes without a knowledge service.', None, 'proposed')
        return result
    agent.reply = defaults
    state = answer(client, start(client), text='A complete brief with memory left to your recommendation.')
    assert state['brief']['facts']['knowledge']['status'] == 'proposed'
    applied = client.post(f"/api/guide/{state['id']}/apply", json={'version': state['version']})
    assert applied.status_code == 200
    saved = applied.json()
    acceptance = saved['messages'][-2]
    assert acceptance['role'] == 'user' and acceptance['action'] == 'apply'
    assert saved['brief']['facts']['knowledge']['source_message_ids'] == [acceptance['id']]
    assert saved['brief']['facts']['knowledge']['status'] == 'confirmed'


def test_legacy_transcripts_get_no_invented_evidence(guide_client):
    client, _, service = guide_client
    state = service.store.create({'stage': 'purpose', 'question': default_question('purpose'),
                                  'messages': [{'role': 'assistant', 'text': 'An older conversation'}]})
    first = client.get(f"/api/guide/{state['id']}").json()
    second = client.get(f"/api/guide/{state['id']}").json()
    assert first['messages'][0]['id'] == second['messages'][0]['id']
    assert 'question' not in first['messages'][0] and first['brief'] == {'facts': {}, 'requirements': []}


@pytest.mark.parametrize('status', ['review', 'ready'])
def test_legacy_proposal_without_answers_can_resume_without_inventing_context(guide_client, status):
    from launchkit.blueprint import starter_blueprint
    from launchkit.guide_models import starter_state
    client, agent, service = guide_client
    original = starter_state({})
    original.pop('answers')
    original.pop('brief')
    proposal = starter_blueprint().model_dump(mode='json')
    original.update(status=status, stage='review', question=None, proposal=proposal)
    saved = service.store.create(original)
    raw = copy.deepcopy(service.store.get(saved['id']))

    opened = client.get(f"/api/guide/{saved['id']}").json()
    assert opened['answers'] == []
    assert opened['proposal'] == proposal and opened['messages'] == raw['messages']
    assert opened['brief'] == {'facts': {}, 'requirements': []}
    assert len(opened['readiness']['decision_blockers']) == 9
    assert service.store.get(saved['id']) == raw
    assert 'answers' not in raw

    response = respond(client, opened, action='chat', text='Continue from my saved proposal.')
    assert response.status_code == 200, response.text
    assert len(agent.calls) == 1
    context = agent.calls[0]['guide_context']
    assert context['answers'] == [] and context['brief'] == {'facts': {}, 'requirements': []}
    assert agent.calls[0]['blueprint'] == proposal
    assert context['current_user_message']['text'] == 'Continue from my saved proposal.'
    assert response.json()['version'] == saved['version'] + 1


def test_manual_role_change_does_not_lock_every_class_field(guide_client):
    client, agent, _ = guide_client
    state = answer(client, start(client))
    blueprint = copy.deepcopy(state['proposal'])
    blueprint['agent_classes'][0]['name'] = 'Case owner'
    answer(client, state, action='manual', blueprint=blueprint, text='Use a more meaningful name.')
    edits = agent.calls[-1]['guide_context']['manual_edits'][-1]['changed_paths']
    assert len(edits) == 1 and edits[0]['path'] == '/agent_classes/0/name'


def test_user_can_choose_supported_first_version_after_extension_explanation(guide_client):
    client, agent, _ = guide_client
    def initial_gap(context, blueprint):
        result = review_reply(context, blueprint)
        result['guide']['question'] = default_question('purpose')
        result['guide']['brief']['requirements'] = [{
            'id': 'staff_app', 'outcome': 'Staff web app.', 'supported_subset': 'Operator console.',
            'extension': 'Build a staff interface.', 'disposition': 'open',
            'source_message_ids': [context['current_user_message']['id']],
        }]
        return result
    agent.reply = initial_gap
    state = answer(client, start(client), text='Build a staff app and a console.')
    def defer(context, blueprint):
        brief = copy.deepcopy(context['brief'])
        brief['requirements'][0].update(disposition='deferred', decision_message_ids=[context['current_user_message']['id']])
        brief['facts']['first_version'] = fact('Use the console initially; add staff app later.', context['current_user_message']['id'])
        return {'guide': {'question': None, 'message': 'The supported first version is ready.', 'brief': brief}, 'proposal': blueprint}
    agent.reply = defer
    state = answer(client, state, text='Start with console only; we will add the staff app later.')
    assert state['readiness']['ready'] and state['stage'] == 'review'
    assert state['brief']['requirements'][0]['disposition'] == 'deferred'
    assert state['brief']['facts']['first_version']['status'] == 'confirmed'


def test_bare_category_cannot_skip_to_connector_question_even_without_plan(guide_client):
    client, agent, service = guide_client
    def skip(context, blueprint):
        return {'guide': {'message': 'Pick your connections.', 'question': default_question('connections'),
                          'brief': {'facts': {}, 'requirements': []}}, 'proposal': None}
    agent.reply = skip
    response = respond(client, legacy_category_start(client, service), selected=['custom'])
    assert response.status_code == 422 and 'Keep the purpose open' in response.text


def test_independent_confirmed_facts_cannot_vanish_on_correction(guide_client):
    client, agent, _ = guide_client
    agent.reply = review_reply
    state = answer(client, start(client), text='A full brief.')
    def forget(context, blueprint):
        brief = copy.deepcopy(context['brief'])
        del brief['facts']['destination']
        return {'guide': {'message': 'New question.', 'question': default_question('purpose'), 'brief': brief}, 'proposal': None}
    agent.reply = forget
    response = respond(client, state, action='chat', text='Let us change the comparison rubric.')
    assert response.status_code == 422 and 'Preserve independent brief facts' in response.text
    assert client.get(f"/api/guide/{state['id']}").json()['brief']['facts']['destination'] == state['brief']['facts']['destination']


def test_cancelled_scope_stays_history_without_becoming_future_work(guide_client):
    client, agent, _ = guide_client
    def original(context, blueprint):
        result = review_reply(context, blueprint)
        result['guide']['brief']['requirements'] = [{
            'id': 'three_specialists', 'outcome': 'Three specialist reviewers.',
            'supported_subset': 'Three durable agent identities.', 'disposition': 'supported',
            'source_message_ids': [context['current_user_message']['id']],
        }]
        return result
    agent.reply = original
    state = answer(client, start(client), text='Start with three reviewers.')
    request_id = state['brief']['requirements'][0]['source_message_ids'][0]
    def cancel(context, blueprint):
        brief = copy.deepcopy(context['brief'])
        brief['requirements'][0].update(disposition='cancelled', decision_message_ids=[context['current_user_message']['id']])
        return {'guide': {'question': None, 'message': 'The old review team is no longer part of this setup.', 'brief': brief}, 'proposal': blueprint}
    agent.reply = cancel
    state = answer(client, state, action='chat', text='I no longer want three reviewers. Use one assistant.')
    requirement = state['brief']['requirements'][0]
    assert requirement['disposition'] == 'cancelled' and requirement['source_message_ids'] == [request_id]
    assert requirement['decision_message_ids'] == [state['messages'][-2]['id']]
    assert state['readiness']['ready'] and 'first_version' not in state['brief']['facts']


def test_implementation_handoff_is_explicit_and_does_not_complete_requested_build(guide_client):
    client, agent, _ = guide_client
    def initial(context, blueprint):
        result = review_reply(context, blueprint)
        result['guide']['question'] = default_question('purpose')
        result['guide']['brief']['requirements'] = [{
            'id': 'staff_app', 'outcome': 'Authenticated staff web app.', 'supported_subset': 'Manager console.',
            'extension': 'Implement authenticated staff routing and tenant access policy.', 'disposition': 'open',
            'source_message_ids': [context['current_user_message']['id']],
        }]
        return result
    agent.reply = initial
    state = answer(client, start(client), text='I need the complete staff web app.')
    def handoff(context, blueprint):
        brief = copy.deepcopy(context['brief'])
        brief['requirements'][0].update(disposition='handoff', decision_message_ids=[context['current_user_message']['id']])
        question = default_question('purpose')
        question.update(id='handoff_contract', options=[], title='Which staff identity provider should the handoff use?')
        return {'guide': {'question': question, 'message': 'I will prepare an implementation handoff. The staff app still needs to be built.', 'brief': brief}, 'proposal': None}
    agent.reply = handoff
    state = answer(client, state, text='Prepare an implementation handoff; I still need this feature.')
    assert state['status'] == 'handoff' and not state['readiness']['ready']
    assert 'Implementation still required' in ' '.join(state['readiness']['blockers'])
    assert client.post(f"/api/guide/{state['id']}/apply", json={'version': state['version']}).status_code == 422
    def premature(context, blueprint):
        return {'guide': {'question': None, 'message': 'Ready to package.', 'brief': context['brief']}, 'proposal': blueprint}
    agent.reply = premature
    response = respond(client, state, text='Use our existing sign-in provider.')
    assert response.status_code == 422 and 'Implementation still required' in response.text


def test_legacy_ready_plan_reopens_for_review_without_rewriting_saved_history(guide_client):
    client, _, service = guide_client
    state = start(client)
    from launchkit.blueprint import starter_blueprint
    original = service.store.save({**state, 'status': 'review', 'question': None,
        'proposal': starter_blueprint().model_dump(mode='json')}, state['version'])
    response = client.get(f"/api/guide/{state['id']}")
    assert response.status_code == 200
    restored = response.json()
    assert restored['status'] == 'question' and not restored['readiness']['ready']
    assert restored['question']['id'] == 'review_saved_setup'
    assert restored['proposal'] == original['proposal'] and restored['messages'] == original['messages']
    assert service.store.get(state['id']) == original


def test_visualization_feedback_ordinary_publication_reload_preserve_and_clear(guide_client, tmp_path):
    client, agent, service = guide_client
    original_message = agent.message
    omitted = object()
    feedback = ["  Keep the circular arrangement.\nShow the handoffs slowly.\t  ", omitted, None, "", omitted]

    async def publish(prompt, blueprint, *, guide_context):
        result = await original_message(prompt, blueprint, guide_context=guide_context)
        value = feedback.pop(0)
        if value is not omitted:
            result['guide']['visualization_feedback'] = value
        return result

    agent.message = publish
    state = answer(client, start(client))
    expected = {'text': "  Keep the circular arrangement.\nShow the handoffs slowly.\t  ",
                'source_message_id': state['messages'][-2]['id']}
    assert state['visualization_feedback'] == expected
    authority = {key: copy.deepcopy(state[key]) for key in ('brief', 'proposal', 'blueprint', 'readiness')}
    assert GuideStore(service.store.root).get(state['id'])['visualization_feedback'] == expected
    for turn in ('Unrelated audience detail.', 'Keep working on the existing plan.'):
        state = answer(client, state, text=turn, action='chat')
        assert state['visualization_feedback'] == expected
        inspected = inspected_guide_turn(agent.calls[-1], tmp_path)['guided_setup']
        assert inspected['visualization_feedback'] == expected
        inspected['visualization_feedback']['text'] = 'Caller mutation'
        assert agent.calls[-1]['guide_context']['visualization_feedback'] == expected
    state = answer(client, state, text='Clear the previous animation preference.', action='chat')
    assert state['visualization_feedback'] is None
    assert GuideStore(service.store.root).get(state['id'])['visualization_feedback'] is None
    state = answer(client, state, text='Discuss the job again.', action='chat')
    assert state['visualization_feedback'] is None
    assert compact_guidance_context(agent.calls[-1]['guide_context'])['visualization_feedback'] is None
    assert {key: state[key] for key in authority} == authority
    assert not {'visualization_feedback', 'source_message_id'} & state['proposal'].keys()
    assert 'visualization_feedback' in agent.calls[-1]['prompt']


@pytest.mark.parametrize('saved_location', ['proposal', 'blueprint'])
def test_visualization_feedback_status_only_publication_preserves_application_scope(guide_client, saved_location):
    from launchkit.artifact_binding import inputs, matches
    from test_implementation_evidence import checked
    client, agent, service = guide_client
    if saved_location == 'blueprint':
        agent.reply = review_reply
    state = answer(client, start(client))
    if saved_location == 'blueprint':
        response = client.post(f"/api/guide/{state['id']}/apply", json={'version': state['version']})
        assert response.status_code == 200, response.text
        state = response.json()
    scope = inputs(state)
    receipt = checked(state)
    receipt['brief_hash'] = scope['brief_hash']
    original_receipt = copy.deepcopy(receipt)
    service.implementation_evidence_provider = lambda current: {
        **copy.deepcopy(receipt), 'current_guide_version': current['version']}
    state = client.get(f"/api/guide/{state['id']}").json()
    assert matches(receipt, state) and state['implementation_evidence']['acceptance_current']
    before = copy.deepcopy(state)

    def animation_reply(context, blueprint):
        assert blueprint == before[saved_location]
        return {'guide': {'question': None, 'message': 'The animation author will use that feedback.',
            'brief': copy.deepcopy(context['brief']), 'design_changed': False,
            'visualization_feedback': 'Keep the outer specialists legible and animate one handoff at a time.'},
            'proposal': None}

    agent.reply = animation_reply
    state = answer(client, state, action='chat', text='Improve the animation only; keep this team and its behavior.')
    assert state['visualization_feedback'] == {
        'text': 'Keep the outer specialists legible and animate one handoff at a time.',
        'source_message_id': state['messages'][-2]['id']}
    for key in ('brief', 'blueprint', 'proposal', 'readiness', 'stage', 'status', 'answers',
                'manual_edits', 'fact_history', 'reopened_topics', 'completed_stages', 'question'):
        assert state[key] == before[key], key
    assert state['version'] == before['version'] + 1
    assert service.store.get(state['id'])['visualization_feedback'] == state['visualization_feedback']
    assert inputs(state) == scope and matches(receipt, state)
    assert receipt == original_receipt
    for key in ('build_id', 'source_digest', 'source_verified', 'acceptance_current',
                'ready_to_package', 'requirements'):
        assert state['implementation_evidence'][key] == before['implementation_evidence'][key], key


@pytest.mark.parametrize('invalid', [True, 23, {}, [], 'x' * 1001, 'bad\x00text', 'bad\x7ftext', 'bad\ud800text',
                                  pytest.param(' \t\r\n ', id='whitespace-only')])
def test_visualization_feedback_rejects_invalid_text_without_coercion(invalid):
    args = {'question': default_question('purpose'), 'message': 'Keep the current design.',
            'brief': {}, 'visualization_feedback': invalid}
    with pytest.raises(ValueError):
        validate_guide_publication(args, {'brief': {}, 'action': 'chat'}, None)


def test_visualization_feedback_accepts_bounded_unicode_and_line_spacing():
    text = '\U0001f9ed' * 997 + '\t\r\n'
    args = {'question': default_question('purpose'), 'message': 'Keep the current design.',
            'brief': {}, 'visualization_feedback': text}
    assert len(text) == 1000
    assert validate_guide_publication(args, {'brief': {}, 'action': 'chat'}, None)['visualization_feedback'] == text


def test_visualization_feedback_stale_publication_cannot_replace_newer_saved_feedback(guide_client):
    client, agent, service = guide_client
    state = answer(client, start(client))
    seen = {}

    def stale_reply(context, blueprint):
        current = service.store.get(state['id'])
        current['visualization_feedback'] = {'text': 'The already committed newer preference.',
            'source_message_id': current['messages'][-2]['id']}
        seen['newer'] = service.store.save(current, current['version'])
        return {'guide': {'question': None, 'message': 'An older animation update.',
            'brief': copy.deepcopy(context['brief']), 'visualization_feedback': 'This stale update must not save.'},
            'proposal': None}

    agent.reply = stale_reply
    response = respond(client, state, action='chat', text='Adjust this animation.')
    assert response.status_code == 409
    assert service.store.get(state['id']) == seen['newer']
