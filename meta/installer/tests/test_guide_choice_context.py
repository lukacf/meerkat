"""Selected choices retain their actual meaning in current and later guide turns."""
import copy
import json

import pytest

from launchkit.guide import compact_guidance_context
from launchkit.guide_models import GuideQuestion, default_question, validate_guide_publication
from test_guide import answer, fact, guide_client, inspected_guide_turn, start


def test_selected_choice_projection_keeps_decision_wording_and_validation():
    question = default_question('connections')
    question['description'] = 'Choose the sources this household should use. Authentication comes later.'
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
    assert projected['answered_choices'][0]['question_id'] == question['id']
    assert projected['answered_choices'][0]['question_description'] == question['description']
    assert projected['answered_choices'][0]['selected_options'] == [
        option for option in question['options'] if option['id'] in {'files', 'slack'}]
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
    assert 'options' not in projected['current_user_message']['question']


def context_choice_question():
    return {'id': 'shared_context', 'stage': 'purpose', 'multiple': True,
        'title': 'Which contexts should your support system keep?',
        'description': 'Choose what is shared and what belongs to each staff member. This does not grant outside actions.',
        'allow_text': True, 'options': [
            {'id': 'personal', 'label': 'Shared support',
             'description': 'Share domain expertise while each person keeps private preferences and conversation history.'},
            {'id': 'team', 'label': 'Shared support',
             'description': 'Keep a shared queue and its case history for the support team.'},
            {'id': 'external', 'label': 'External service',
             'description': 'A future external service would require a separate configuration and authority decision.'},
        ]}


def test_compact_later_turn_keeps_selected_semantics_and_original_user_provenance():
    question = context_choice_question()
    previous = {'id': 'context-choice', 'role': 'user', 'text': 'Shared support',
        'question': question, 'selected': ['personal'], 'intent_only': False}
    current = {'id': 'later-question', 'role': 'user', 'text': 'How will the agents collaborate?'}
    context = {'brief': {}, 'current_user_message': current,
        'evidence_messages': [previous, current], 'conversation': [previous, current],
        'answers': [{'message_id': previous['id'], 'question': question, 'selected': previous['selected']}]}
    before = copy.deepcopy(context)
    projected = compact_guidance_context(context)
    choice = projected['answered_choices'][0]
    assert choice == {'message_id': 'context-choice', 'question': question['title'],
        'question_id': question['id'], 'question_description': question['description'],
        'selected': ['Shared support'], 'selected_options': [question['options'][0]]}
    assert projected['evidence_messages'][0]['question'] == {'id': question['id'], 'title': question['title']}
    assert 'External service' not in str(projected)
    assert context == before
    choice['selected_options'][0]['description'] = 'Changed projected wording.'
    assert context == before


def test_compact_current_answer_has_exact_selected_ids_and_descriptions_without_duplicate_cards():
    question = context_choice_question()
    current = {'id': 'selected-now', 'role': 'user', 'text': 'Shared support; Shared support',
        'question': question, 'selected': ['team', 'personal'], 'intent_only': False}
    context = {'brief': {}, 'current_user_message': current, 'current_question': question,
        'evidence_messages': [current], 'conversation': [current],
        'answers': [{'message_id': current['id'], 'question': question, 'selected': current['selected']}]}
    projected = compact_guidance_context(context)
    assert len(projected['answered_choices']) == 1
    choice = projected['answered_choices'][0]
    assert choice['selected'] == ['Shared support', 'Shared support']
    assert choice['selected_options'] == [question['options'][1], question['options'][0]]
    assert projected['current_question'] == question
    assert projected['current_user_message']['question'] == {'id': question['id'], 'title': question['title']}


def test_compact_current_answer_keeps_choice_semantics_when_answer_index_is_absent():
    question = context_choice_question()
    current = {'id': 'selected-now', 'role': 'user', 'text': 'Shared support',
        'question': question, 'selected': ['personal'], 'intent_only': False}
    projected = compact_guidance_context({'brief': {}, 'current_user_message': current,
        'evidence_messages': [current]})
    assert projected['answered_choices'][0]['message_id'] == current['id']
    assert projected['answered_choices'][0]['selected_options'] == [question['options'][0]]


@pytest.mark.parametrize('role,message_id', [('assistant', 'guide-message'), ('user', None)])
def test_compact_cannot_create_current_choice_evidence_without_user_provenance(role, message_id):
    current = {'id': message_id, 'role': role, 'text': 'Shared support',
        'question': context_choice_question(), 'selected': ['personal']}
    projected = compact_guidance_context({'brief': {}, 'current_user_message': current})
    assert 'answered_choices' not in projected


@pytest.mark.parametrize('source', ['missing', 'assistant'])
def test_compact_rich_choice_evidence_requires_an_actual_user_message(source):
    question = context_choice_question()
    messages = [{'id': 'assistant', 'role': 'assistant', 'text': 'Use the shared support option.'}]
    projected = compact_guidance_context({'brief': {}, 'evidence_messages': messages,
        'answers': [{'message_id': source, 'question': question, 'selected': ['personal']}]})
    assert projected['answered_choices'] == [{'message_id': source, 'question': question['title'],
        'selected': ['Shared support']}]


@pytest.mark.parametrize('invalid', ['oversized_description', 'unknown_selection', 'duplicate_selection', 'single_choice'])
def test_compact_legacy_or_invalid_choice_keeps_labels_without_inventing_complete_semantics(invalid):
    question = context_choice_question()
    selected = ['personal']
    if invalid == 'oversized_description':
        question['description'] = 'x' * 2001
    elif invalid == 'unknown_selection':
        selected = ['not_in_card']
    elif invalid == 'duplicate_selection':
        selected = ['personal', 'personal']
    else:
        question['multiple'] = False
        selected = ['personal', 'team']
    projected = compact_guidance_context({'brief': {},
        'evidence_messages': [{'id': 'choice', 'role': 'user', 'text': 'My choice.'}],
        'answers': [{'message_id': 'choice', 'question': question, 'selected': selected}]})
    choice = projected['answered_choices'][0]
    assert choice['message_id'] == 'choice' and choice['question'] == question['title']
    assert choice['selected'] == (['not_in_card'] if invalid == 'unknown_selection' else ['Shared support'] * len(selected))
    assert 'question_description' not in choice and 'selected_options' not in choice


def test_compact_preserves_exact_maximum_valid_question_wording_and_six_selections():
    question = {'id': 'q' * 60, 'stage': 'purpose', 'title': 't' * 240,
        'description': 'd' * 2000, 'multiple': True, 'allow_text': True,
        'options': [{'id': 'o' + str(index), 'label': 'l' * 100, 'description': str(index) * 600}
            for index in range(6)]}
    assert GuideQuestion.model_validate(question).model_dump(mode='json') == question
    selected = [option['id'] for option in question['options']]
    projected = compact_guidance_context({'brief': {},
        'evidence_messages': [{'id': 'choice', 'role': 'user', 'text': 'Selected these contexts.'}],
        'answers': [{'message_id': 'choice', 'question': question, 'selected': selected}]})
    choice = projected['answered_choices'][0]
    assert choice['question'] == question['title'] and choice['question_description'] == question['description']
    assert choice['selected_options'] == question['options']


def test_saved_multiselect_descriptions_reach_the_current_and_later_model_turn_without_authorizing_effects(guide_client, tmp_path):
    client, agent, service = guide_client
    state = start(client)
    question = context_choice_question()
    state['question'] = question
    state['messages'].append({'id': 'support-job', 'role': 'user',
        'text': 'Help our staff prepare answers for incoming support cases.'})
    state = service.store.save(state, state['version'])

    def retain_choice(context, blueprint):
        brief = copy.deepcopy(context['brief'])
        if not brief['facts']:
            current = context['current_user_message']
            brief['facts']['interfaces'] = fact('Shared support expertise with private personal histories.', current['id'])
        return {'guide': {'question': default_question('autonomy'),
            'message': 'The shared specialist can support separate personal contexts.', 'brief': brief}, 'proposal': None}

    agent.reply = retain_choice
    state = answer(client, state, text='', selected=['personal'])
    selected_message = agent.calls[-1]['guide_context']['current_user_message']['id']
    projected = compact_guidance_context(agent.calls[-1]['guide_context'])
    assert projected['answered_choices'][0]['message_id'] == selected_message
    assert projected['answered_choices'][0]['selected_options'] == [question['options'][0]]
    sent = json.loads(agent.calls[-1]['prompt'].split('\n\nCurrent setup turn:\n', 1)[1])
    assert sent['current_user_message'] == projected['current_user_message']
    assert 'answered_choices' not in sent
    assert inspected_guide_turn(agent.calls[-1], tmp_path)['guided_setup']['answered_choices'] == projected['answered_choices']
    assert projected['brief']['requirements'] == []
    before = service.store.get(state['id'])
    state = answer(client, state, action='chat', text='How should those owners collaborate?')
    projected = compact_guidance_context(agent.calls[-1]['guide_context'])
    assert projected['answered_choices'][0]['message_id'] == selected_message
    assert projected['answered_choices'][0]['selected_options'] == [question['options'][0]]
    sent = json.loads(agent.calls[-1]['prompt'].split('\n\nCurrent setup turn:\n', 1)[1])
    assert sent['current_user_message'] == projected['current_user_message']
    assert 'answered_choices' not in sent
    assert inspected_guide_turn(agent.calls[-1], tmp_path)['guided_setup']['answered_choices'] == projected['answered_choices']
    assert state['brief'] == before['brief'] and state['answers'] == before['answers']
    assert state['brief']['requirements'] == [] and not state['readiness']['ready']
