"""Synthetic parser regressions only; no native authority or runtime proof.

Run: python3 -B scripts/test_verify_installed_warm.py
The three verifier modules are imported from this file's sibling directory.
"""
import copy
import importlib.util
import inspect
import json
from pathlib import Path
import sqlite3
import tempfile
import types
import unittest


def sibling(name):
    spec = importlib.util.spec_from_file_location(name, Path(__file__).with_name(name + ".py"))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


r = types.SimpleNamespace(v=sibling("verify_native_gateway_subprocess"),
                          console=sibling("verify_installed_console"), warm=sibling("verify_installed_warm"))


class LineageTests(unittest.TestCase):
    def setUp(self):
        self.scenario = r.warm.create_scenario(r.v, r.console, "http://127.0.0.1:1")
        instructions = (self.scenario.original_request(), r.warm.WARM_REQUEST)
        contexts, expectations, observed = [], [], []
        self.starts = [{"payload": {"input": {"kind": "content", "content": instructions[0]}}}]
        for index, instruction in enumerate(instructions):
            original = "synthetic-original-" + str(index)
            context = {"fresh_original": original}
            expected = {"original": original, "instruction": instruction,
                        "observed_before_answer_ms": 100, "members": []}
            blocks, provider = [], []
            for member in range(index + 1):
                fields = {"schema": "toolkit.approved-service-context.v1", "original": original,
                    "operation": "synthetic-operation-" + str(index), "connection": "synthetic-server-" + str(member),
                    "resource": "synthetic-resource-" + str(member), "allowed_operations": ["synthetic-read-" + str(member)],
                    "configuration_sha256": str(member) * 64,
                    "attachment": "observed_usable_before_input_admission", "meaning": "synthetic reviewed display"}
                expected["members"].append({"fields": fields, "reviewed_expiry_max_ms": 1000})
                text = r.v.canonical({**fields, "expires_at_ms": 500}).decode()
                blocks.append({"type": "text", "text": text})
                provider.append({"role": "user", "content": text})
            blocks.append({"type": "text", "text": instruction})
            self.starts.append({"payload": {"input": {"kind": "content", "content": blocks}}})
            contexts.append(context)
            expectations.append(expected)
            observed.append(provider)
        self.args = [None, None, contexts, {"service_context_display": {
            "expectations": expectations, "validated_observed_blocks": observed}}]

    def test_full_reviewed_context_and_instruction_sequence(self):
        self.check()

    def check(self):
        self.scenario.check_native_instructions(self.starts, self.args[2], self.args[-1])

    def mutate_context(self, transform, index=2, block_index=0):
        block = self.starts[index]['payload']['input']['content'][block_index]
        value = r.v.unique_json(block['text'].encode())
        transform(value)
        block['text'] = r.v.canonical(value).decode()

    def test_actual_instruction_mismatch_refused_for_each_run(self):
        for index in range(3):
            self.setUp()
            content = self.starts[index]['payload']['input']['content']
            if index == 0:
                self.starts[index]['payload']['input']['content'] = content + ' changed'
            else:
                content[-1]['text'] += ' changed'
            with self.subTest(index=index), self.assertRaises(AssertionError):
                self.check()

    def test_missing_extra_and_reordered_contexts_refused(self):
        for operation in ('missing', 'extra', 'reverse', 'instruction_first'):
            self.setUp()
            blocks = self.starts[2]['payload']['input']['content']
            if operation == 'missing':
                del blocks[0]
            elif operation == 'extra':
                blocks.insert(0, copy.deepcopy(blocks[0]))
            elif operation == 'reverse':
                blocks[0], blocks[1] = (blocks[1], blocks[0])
            else:
                blocks[0], blocks[-1] = (blocks[-1], blocks[0])
            with self.subTest(operation=operation), self.assertRaises(AssertionError):
                self.check()

    def test_unknown_block_fields_type_or_opaque_input_refused(self):
        for change in ('extra_field', 'nontext', 'missing_text', 'opaque', 'input_extra'):
            self.setUp()
            value = self.starts[1]['payload']['input']
            if change == 'extra_field':
                value['content'][0]['extra'] = True
            elif change == 'nontext':
                value['content'][0]['type'] = 'image'
            elif change == 'missing_text':
                del value['content'][0]['text']
            elif change == 'opaque':
                value['kind'] = 'opaque'
            else:
                value['extra'] = True
            with self.subTest(change=change), self.assertRaises(AssertionError):
                self.check()

    def test_changed_context_fields_refused(self):
        for field in ('original', 'operation', 'connection', 'resource', 'configuration_sha256', 'attachment', 'meaning', 'schema', 'allowed_operations'):
            self.setUp()
            self.mutate_context(lambda v: v.update({field: ['other'] if field == 'allowed_operations' else 'other'}))
            with self.subTest(field=field), self.assertRaises(AssertionError):
                self.check()

    def test_context_field_set_and_duplicate_json_refused(self):
        for change in ('extra', 'missing', 'duplicate'):
            self.setUp()
            if change == 'extra':
                self.mutate_context(lambda v: v.update(unreviewed=True))
            elif change == 'missing':
                self.mutate_context(lambda v: v.pop('resource'))
            else:
                block = self.starts[2]['payload']['input']['content'][0]
                block['text'] = '{"schema":"duplicate",' + block['text'][1:]
            with self.subTest(change=change), self.assertRaises(AssertionError):
                self.check()

    def test_expiry_type_and_review_bounds_refused(self):
        expected = self.args[-1]['service_context_display']['expectations'][1]
        for expiry in (True, float(expected['members'][0]['reviewed_expiry_max_ms']), expected['observed_before_answer_ms'], expected['members'][0]['reviewed_expiry_max_ms'] + 1):
            self.setUp()
            self.mutate_context(lambda v: v.update(expires_at_ms=expiry))
            with self.subTest(expiry=expiry), self.assertRaises(AssertionError):
                self.check()

    def test_noncanonical_context_bytes_refused(self):
        block = self.starts[2]['payload']['input']['content'][0]
        block['text'] = json.dumps(json.loads(block['text']), indent=2)
        with self.assertRaises(AssertionError):
            self.check()

    def test_wrong_review_expectation_and_provider_bytes_refused(self):
        for change in ('original', 'instruction', 'provider', 'order'):
            self.setUp()
            display = self.args[-1]['service_context_display']
            if change == 'original':
                display['expectations'][1]['original'] = 'foreign'
            elif change == 'instruction':
                display['expectations'][1]['instruction'] = 'other'
            elif change == 'provider':
                display['validated_observed_blocks'][1][0]['content'] += ' '
            else:
                display['expectations'].reverse()
            with self.subTest(change=change), self.assertRaises(AssertionError):
                self.check()

class ControllerRecoveryTests(unittest.TestCase):
    def setUp(self):
        fixture = LineageTests()
        fixture.setUp()
        self.scenario = fixture.scenario
        provider_class = inspect.getclosurevars(self.scenario.provider).nonlocals['WarmProvider']
        # Test the owning history methods without constructing the HTTP owner.
        self.provider = provider_class.__new__(provider_class)
        self.provider.context_expectations = copy.deepcopy(fixture.args[-1]['service_context_display']['expectations'])
        self.provider.context_observations = []
        self.provider.controller_recovery = {'b': None, 'c': None}
        self.provider.controller_history = {'b': None, 'c': None}
        self.provider.b_final_history = None
        blocks = fixture.args[-1]['service_context_display']['validated_observed_blocks']
        self.b = [{'role': 'user', 'content': self.scenario.original_request()},
                  {'role': 'assistant', 'content': r.v.ORIGINAL_A_FINISH, 'tool_calls': []},
                  blocks[0][0], {'role': 'user', 'content': self.scenario.original_request()}]
        self.c_blocks = blocks[1]
        self.feedback = {'role': 'user', 'content': r.v.ORIGINAL_CONTROLLER_FEEDBACK}
        self.config = r.v.config_text(12345, acquisition=True).encode()
        self.target = r.warm.configured_controller_target(self.config, 'mob.fixture')
        self.state = self.native_state('input-b', 'run-b')

    def native_state(self, input_id, run_id):
        def row(operation, observation):
            return {'contributors': [{'input_id': input_id}], 'observation': {
                'operation_id': operation, 'run_id': run_id, 'observation': observation}}
        refused = {'kind': 'refused', 'target': copy.deepcopy(self.target), 'reason': 'reprepare_required'}
        return {'input_id': input_id, 'last_run_id': run_id, 'current_state': 'consumed',
            'terminal_outcome': {'outcome_type': 'consumed'}, 'terminal_completion': {
                'input_id': input_id, 'owner_input_id': input_id, 'completion_input_ids': [input_id],
                'batch_key': {'scope': 'run', 'run_id': run_id},
                'outcome': {'completion_type': 'completed'},
                'phase': {'phase': 'finalized', 'finalization': 'succeeded'}},
            'authorization_audit': [row('refused', copy.deepcopy(refused)), row('refused', refused),
                row('replacement', {'kind': 'prepared', 'target': copy.deepcopy(self.target),
                    'policy': {'publication_sequence': 7, 'observed_at_ms': 100}}),
                row('replacement', {'kind': 'entry'}),
                row('replacement', {'kind': 'outcome', 'outcome': {'kind': 'http_response', 'status': 200}})]}

    def audit(self, state=None, **changes):
        args = {'v': r.v, 'state': self.state if state is None else state, 'session_id': 'session',
                'input_id': 'input-b', 'run_id': 'run-b', 'target': self.target}
        args.update(changes)
        return r.warm.checked_controller_audit(**args)

    def b_history(self, messages=None):
        self.provider.checked_b_history({'messages': self.b if messages is None else messages})

    def c_history(self, feedback=False):
        self.provider.b_final_history = copy.deepcopy(self.b)
        messages = self.b + [{'role': 'assistant', 'content': r.warm.B_FINISH, 'tool_calls': []}]
        messages += self.c_blocks + [{'role': 'user', 'content': r.warm.WARM_REQUEST}]
        if feedback:
            messages.append(copy.deepcopy(self.feedback))
        self.provider.checked_c_history({'messages': messages})
        return messages

    def test_normal_b_c_history_has_no_recovery(self):
        self.b_history()
        self.c_history()
        self.assertEqual(self.provider.controller_recovery, {'b': None, 'c': None})

    def test_b_and_c_recovery_branches_are_separate_and_stable(self):
        self.b.append(copy.deepcopy(self.feedback))
        self.b_history()
        b_record = copy.deepcopy(self.provider.controller_recovery['b'])
        self.b_history()
        c = self.c_history(feedback=True)
        self.provider.checked_c_history({'messages': c})
        self.assertEqual(b_record, self.provider.controller_recovery['b'])
        self.assertNotEqual(b_record, self.provider.controller_recovery['c'])
        self.assertTrue(self.provider.controller_reprepare)

    def test_c_without_recovery_preserves_historical_b_recovery(self):
        self.b.append(copy.deepcopy(self.feedback))
        self.b_history()
        self.c_history()
        self.assertIsNotNone(self.provider.controller_recovery['b'])
        self.assertIsNone(self.provider.controller_recovery['c'])

    def test_old_default_history_refuses_the_exact_feedback(self):
        self.b.append(copy.deepcopy(self.feedback))
        with self.assertRaisesRegex(AssertionError, 'controller feedback'):
            r.v.original_catalog_history(self.b, self.scenario.original_request(), r.v.ACQUISITION_OPERATION,
                3, service_context=json.loads(self.b[2]['content']))

    def test_default_hook_preserves_prior_case_contract(self):
        provider = r.v.InstalledAcquisitionProvider.__new__(r.v.InstalledAcquisitionProvider)
        provider.expected_service_context = json.loads(self.b[2]['content'])
        provider.controller_reprepare = True
        self.assertEqual(provider.checked_original_catalog_history(self.b + [self.feedback], 3), 4)
        late = self.b + [{'role': 'assistant', 'tool_calls': [{'id': 'old-case'}]},
                        {'role': 'tool', 'tool_call_id': 'old-case', 'content': '{}'}, self.feedback]
        with self.assertRaisesRegex(AssertionError, 'controller feedback'):
            provider.checked_original_catalog_history(late, 3)

    def test_notice_after_completed_tool_is_a_model_boundary(self):
        self.b_history()
        call = {'role': 'assistant', 'content': None, 'tool_calls': [{'id': 'first'}]}
        result = {'role': 'tool', 'tool_call_id': 'first', 'content': '{}'}
        self.b += [call, result, copy.deepcopy(self.feedback)]
        self.b_history()
        self.assertEqual(self.provider.controller_recovery['b']['notice_history_index'], 6)
        c = self.c_history()
        c += [call, result, copy.deepcopy(self.feedback)]
        self.provider.checked_c_history({'messages': c})
        self.assertEqual(self.provider.controller_recovery['c']['notice_history_index'], len(c)-1)

    def test_notice_with_unresolved_call_or_unmatched_result_refused(self):
        for message in ({'role': 'assistant', 'content': None, 'tool_calls': [{'id': 'first'}]},
                        {'role': 'tool', 'tool_call_id': 'first', 'content': '{}'}):
            with self.subTest(message=message), self.assertRaises(AssertionError):
                r.warm.checked_controller_notice(self.b + [message, self.feedback], 3,
                    r.v.ORIGINAL_CONTROLLER_FEEDBACK, None, r.v.require)

    def test_history_before_new_notice_cannot_change(self):
        self.b_history()
        changed = copy.deepcopy(self.b)
        changed[1]['content'] += ' changed'
        with self.assertRaisesRegex(AssertionError, 'history changed'):
            self.b_history(changed + [self.feedback])

    def test_repeated_notice_and_extra_fields_refused(self):
        for tail in ([self.feedback, self.feedback], [{**self.feedback, 'extra': True}]):
            with self.subTest(tail=tail), self.assertRaises(AssertionError):
                self.b_history(self.b + tail)

    def test_notice_disappearance_or_changed_history_refused(self):
        self.b.append(copy.deepcopy(self.feedback))
        self.b_history()
        with self.assertRaises(AssertionError):
            self.b_history(self.b[:-1])
        changed = copy.deepcopy(self.b)
        changed[1]['content'] += ' changed'
        with self.assertRaises(AssertionError):
            self.b_history(changed)

    def test_unrelated_notice_refused_in_b_and_c(self):
        for content in ('permission denied', r.v.ORIGINAL_CONTROLLER_FEEDBACK + ' changed'):
            self.setUp()
            with self.subTest(phase='b', content=content), self.assertRaises(AssertionError):
                self.b_history(self.b + [{'role': 'user', 'content': content}])
            c = self.c_history()
            with self.subTest(phase='c', content=content), self.assertRaises(AssertionError):
                self.provider.checked_c_history({'messages': c + [{'role': 'user', 'content': content}]})

    def test_exact_native_replacement_http200_required(self):
        result = self.audit()
        self.assertEqual(result['refused_operation_id'], 'refused')
        self.assertEqual(result['recovery_operation_id'], 'replacement')
        self.assertEqual(result['recovered_http_status'], 200)
        self.assertEqual(result['complete_target'], self.target)

    def successful_model(self, identifier, reprepare=False):
        records = copy.deepcopy(self.state['authorization_audit'][2:])
        for row in records:
            row['observation']['operation_id'] = identifier
        if reprepare:
            refused = copy.deepcopy(self.state['authorization_audit'][0])
            refused['observation']['operation_id'] = identifier
            records = [copy.deepcopy(records[0]), refused] + records
        return records

    def test_recovery_after_prior_model_and_tool_then_later_success(self):
        state = copy.deepcopy(self.state)
        prior = self.successful_model('prior', reprepare=True)
        tool = copy.deepcopy(prior[-3:])
        for row in tool:
            row['observation']['operation_id'] = 'tool-op'
        tool[0]['observation']['observation']['target'] = {
            'kind': 'tool', 'call_id': 'catalog', 'tool_name': 'tool_catalog_search',
            'arguments_digest': 'synthetic', 'owners': []}
        tool[-1]['observation']['observation']['outcome'] = {
            'kind': 'tool_dispatch_returned', 'result_is_error': False,
            'terminal_error': None, 'asynchronous_operations': []}
        state['authorization_audit'] = prior + tool + state['authorization_audit'] + self.successful_model('later', True)
        result = self.audit(state)
        self.assertGreater(result['refused_audit_indices'][0], 0)
        self.assertEqual([x['operation_id'] for x in result['ordered_model_operations']],
                         ['prior', 'refused', 'replacement', 'later'])
        self.assertEqual(result['all_audit_rows_retained'], len(state['authorization_audit']))

    def test_same_replacement_reprepare_and_repeated_prepared_rows(self):
        state = copy.deepcopy(self.state)
        replacement = self.successful_model('replacement', reprepare=True)
        replacement.insert(-2, copy.deepcopy(replacement[-3]))
        state['authorization_audit'] = state['authorization_audit'][:2] + replacement
        self.assertEqual(self.audit(state)['recovery_operation_id'], 'replacement')

    def test_successful_distinct_operations_alone_are_not_recovery(self):
        state = copy.deepcopy(self.state)
        state['authorization_audit'] = self.successful_model('one', True) + self.successful_model('two')
        with self.assertRaisesRegex(AssertionError, 'unique'):
            self.audit(state)

    def test_second_abandoned_operation_is_not_another_recovery(self):
        state = copy.deepcopy(self.state)
        extra = copy.deepcopy(state['authorization_audit'][0])
        extra['observation']['operation_id'] = 'second-refused'
        state['authorization_audit'] += [extra] + self.successful_model('third')
        with self.assertRaisesRegex(AssertionError, 'unique'):
            self.audit(state)

    def test_refused_model_must_not_have_entered_or_produced_outcome(self):
        for change in ('entered', 'outcome', 'refusal_after_entry', 'entry_after_refusal'):
            state = copy.deepcopy(self.state)
            if change in ('entered', 'outcome'):
                row = copy.deepcopy(state['authorization_audit'][0])
                row['observation']['observation'] = ({'kind': 'entry'} if change == 'entered'
                    else {'kind': 'outcome', 'outcome': {'kind': 'http_response', 'status': 200}})
                state['authorization_audit'].insert(2, row)
            elif change == 'refusal_after_entry':
                row = copy.deepcopy(state['authorization_audit'][0])
                row['observation']['operation_id'] = 'replacement'
                state['authorization_audit'].insert(4, row)
            else:
                row = copy.deepcopy(state['authorization_audit'][0])
                row['observation']['operation_id'] = 'replacement'
                state['authorization_audit'].insert(3, row)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                self.audit(state)

    def test_dangling_or_unmatched_effect_refused(self):
        for change in ('no_outcome', 'duplicate_entry', 'no_preparation', 'unknown_outcome'):
            state = copy.deepcopy(self.state)
            records = state['authorization_audit']
            if change == 'no_outcome': records.pop()
            elif change == 'duplicate_entry': records.insert(4, copy.deepcopy(records[3]))
            elif change == 'no_preparation': del records[2]
            else: records[-1]['observation']['observation']['outcome'] = {'kind': 'transport_error'}
            with self.subTest(change=change), self.assertRaises(AssertionError):
                self.audit(state)

    def test_nonmodel_refusal_or_unavailable_is_not_controller_recovery(self):
        for change in ('tool', 'unavailable'):
            state = copy.deepcopy(self.state)
            extra = copy.deepcopy(state['authorization_audit'][0])
            extra['observation']['operation_id'] = 'unrelated'
            if change == 'tool':
                extra['observation']['observation']['target'] = {'kind': 'tool'}
            else:
                extra['observation']['observation']['kind'] = 'authorization_unavailable'
            state['authorization_audit'].insert(0, extra)
            with self.subTest(change=change), self.assertRaises(AssertionError):
                self.audit(state)

    def test_selected_config_port_and_complete_target(self):
        self.assertEqual(self.target['endpoint'], 'http://127.0.0.1:12345/v1/chat/completions')
        for key in self.target:
            state = copy.deepcopy(self.state)
            for index in (0, 1, 2):
                state['authorization_audit'][index]['observation']['observation']['target'][key] = 'foreign'
            with self.subTest(key=key), self.assertRaises(AssertionError):
                self.audit(state)
        changed = r.warm.configured_controller_target(r.v.config_text(12346, acquisition=True).encode(), 'mob.fixture')
        with self.assertRaises(AssertionError):
            self.audit(target=changed)

    def test_denied_malformed_unavailable_and_later_refusals_refused(self):
        for reason in ('denied', 'malformed_facts'):
            state = copy.deepcopy(self.state)
            for index in (0, 1):
                state['authorization_audit'][index]['observation']['observation']['reason'] = reason
            with self.subTest(reason=reason), self.assertRaises(AssertionError):
                self.audit(state)
        for kind in ('refused', 'authorization_unavailable'):
            state = copy.deepcopy(self.state)
            extra = copy.deepcopy(state['authorization_audit'][0])
            extra['observation']['operation_id'] = 'later'
            extra['observation']['observation']['kind'] = kind
            state['authorization_audit'].append(extra)
            with self.subTest(kind=kind), self.assertRaises(AssertionError):
                self.audit(state)

    def test_refused_operation_not_retagged_and_replacement_target_exact(self):
        for change in ('same_id', 'different_target', 'missing_entry', 'http500', 'extra_attempt'):
            state = copy.deepcopy(self.state)
            records = state['authorization_audit']
            if change == 'same_id':
                for row in records[2:]: row['observation']['operation_id'] = 'refused'
            elif change == 'different_target': records[2]['observation']['observation']['target']['endpoint'] += '/other'
            elif change == 'missing_entry': del records[3]
            elif change == 'http500': records[4]['observation']['observation']['outcome']['status'] = 500
            else: records.extend(copy.deepcopy(records[2:]))
            with self.subTest(change=change), self.assertRaises(AssertionError):
                self.audit(state)

    def test_stale_run_input_and_contributors_refused(self):
        for change in ('input', 'run', 'audit_run', 'contributors', 'terminal_run', 'terminal_input'):
            state = copy.deepcopy(self.state)
            if change == 'input': state['input_id'] = 'old'
            elif change == 'run': state['last_run_id'] = 'old'
            elif change == 'audit_run': state['authorization_audit'][0]['observation']['run_id'] = 'old'
            elif change == 'contributors': state['authorization_audit'][0]['contributors'].append({'input_id': 'old'})
            elif change == 'terminal_run': state['terminal_completion']['batch_key']['run_id'] = 'old'
            else: state['terminal_completion']['owner_input_id'] = 'old'
            with self.subTest(change=change), self.assertRaises(AssertionError):
                self.audit(state)

    def test_cancelled_or_unfinalized_state_cannot_claim_success(self):
        for change in ('abandoned', 'cancelled', 'phase', 'finalization'):
            state = copy.deepcopy(self.state)
            if change == 'abandoned': state['current_state'] = 'abandoned'
            elif change == 'cancelled': state['terminal_completion']['outcome']['completion_type'] = 'cancelled'
            elif change == 'phase': state['terminal_completion']['phase']['phase'] = 'pending'
            else: state['terminal_completion']['phase']['finalization'] = 'failed'
            with self.subTest(change=change), self.assertRaises(AssertionError):
                self.audit(state)

    def stopped_fixture(self, root, phases):
        self.scenario.controller_configuration = self.config
        self.scenario.source_owner = {'native_host': {'realm': 'mob.fixture'}}
        self.scenario.final_report = {'accepted': {'session_id': 'session'},
            'completion': {'b_run': 'run-b', 'c_run': 'run-c'},
            'controller_recovery_display': {phase: ({'synthetic_display': phase} if phase in phases else None)
                                             for phase in ('b', 'c')}}
        report = {'model_configuration': r.v.new_artifact(root / 'meerkat.toml', self.config)}
        sources = [{}, {'service_admission': {'input_id': 'input-b'}}, {'service_admission': {'input_id': 'input-c'}}]
        return report, sources

    def database(self, root, rows):
        (root / 'state').mkdir()
        connection = sqlite3.connect(root / 'state/runtime.sqlite')
        try:
            connection.execute('CREATE TABLE runtime_input_states(runtime_id TEXT,input_id TEXT,state_json TEXT)')
            connection.executemany('INSERT INTO runtime_input_states VALUES(?,?,?)', rows)
            connection.commit()
        finally:
            connection.close()

    def test_no_recovery_does_not_open_runtime_database(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            report, sources = self.stopped_fixture(root, [])
            self.scenario.check_controller_audits(root, report, sources)
            observed = json.loads((root / 'warm-controller-recovery.json').read_text())
            self.assertEqual(observed['recoveries'], [])
            self.assertEqual(observed['no_recovery_runs'], ['b', 'c'])
            self.assertFalse((root / 'state').exists())

    def test_exact_stopped_source_input_and_separate_b_c_audits(self):
        with tempfile.TemporaryDirectory() as temp:
            root = Path(temp)
            report, sources = self.stopped_fixture(root, ['b', 'c'])
            rows = [('rt:session:session', 'input-' + phase, json.dumps(self.native_state('input-' + phase, 'run-' + phase)))
                    for phase in ('b', 'c')]
            rows.append(('rt:session:foreign', 'input-b', '{}'))
            self.database(root, rows)
            self.scenario.check_controller_audits(root, report, sources)
            observed = json.loads((root / 'warm-controller-recovery.json').read_text())
            self.assertEqual([x['phase'] for x in observed['recoveries']], ['b', 'c'])
            self.assertEqual([x['audit']['input_id'] for x in observed['recoveries']], ['input-b', 'input-c'])

    def test_missing_duplicate_or_stale_selected_row_refused(self):
        for change in ('missing', 'duplicate', 'stale_run', 'foreign_session'):
            with self.subTest(change=change), tempfile.TemporaryDirectory() as temp:
                root = Path(temp)
                report, sources = self.stopped_fixture(root, ['b'])
                state = self.native_state('input-b', 'stale' if change == 'stale_run' else 'run-b')
                rows = [('rt:session:foreign' if change == 'foreign_session' else 'rt:session:session', 'input-b', json.dumps(state))]
                if change == 'missing': rows = []
                elif change == 'duplicate': rows *= 2
                self.database(root, rows)
                with self.assertRaises(AssertionError):
                    self.scenario.check_controller_audits(root, report, sources)
                self.assertFalse((root / 'warm-controller-recovery.json').exists())


if __name__ == "__main__":
    unittest.main(verbosity=2)
