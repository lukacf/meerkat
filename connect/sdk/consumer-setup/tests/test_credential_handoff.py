"""Credential-name handoff only; fake package owners never look up secrets."""
import asyncio
import copy
import http.client
import importlib.util
import json
from pathlib import Path
import threading
import unittest
from unittest.mock import patch

from mcp import Client

ROOT = Path(__file__).resolve().parents[1]
def load(name, path):
    spec = importlib.util.spec_from_file_location(name, path)
    value = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(value)
    return value
fixtures = load('handoff_fixtures', ROOT / 'tests/test_reconciliation.py')
backend = fixtures.backend
adapter = load('handoff_adapter', ROOT / 'mcp_adapter.py')
adapter.backend = backend
CANARY = 'PRIVATE_DIAGNOSTIC_DO_NOT_REFLECT'


class ReferenceRunner(fixtures.ReconciliationRunner):
    def __init__(self, workspace):
        super().__init__(workspace)
        self.supported = True
        self.transform_projection = lambda value: value
        self.after_check = lambda path: None

    def run(self, argv, capture=False):
        code, raw = super().run(argv, capture=capture)
        if 'config-help' in argv:
            value = backend.decode(raw)
            if 'config_fields' in value and isinstance(value['config_fields'], list):
                value['config_fields'] += [
                    {'name': name, 'kind': 'string', 'required': False, 'credential_reference': True}
                    for name in ('token_env', 'optional_env')]
            return code, backend.encoded(value)
        if 'check-config' not in argv or code:
            return code, raw
        path = Path(argv[argv.index('--config') + 1])
        selected = backend.decode(path.read_bytes())
        family = 'activepieces' if any('activepieces' in arg for arg in argv) else 'services' if any('services' in arg for arg in argv) else 'agno'
        values = selected if family == 'activepieces' else selected['config']
        source = {'profile': selected['profile'], 'valid': True, 'scope': 'configuration_only', 'diagnostic': CANARY}
        if self.supported:
            names = ('token_env', 'optional_env') if family == 'agno' else ('token_env',)
            source['credential_environment_references'] = [
                {'field_path': [name] if family == 'activepieces' else ['config', name], 'environment_variable': values[name]}
                for name in names if name in values]
        source = self.transform_projection(source)
        raw = source if isinstance(source, bytes) else backend.encoded(source)
        self.after_check(path)
        return code, raw


class HandoffTests(fixtures.ReconciliationMixin, unittest.TestCase):
    def controller(self, **changes):
        changes.setdefault('runner_factory', ReferenceRunner)
        return super().controller(**changes)

    def selected(self, family='agno'):
        if family == 'agno':
            return self.request()
        if family == 'services':
            return {'family': family, 'profile': 'slack', 'operations': ['auth_test'],
                    'values': {'token_env': 'SLACK_TOKEN', 'timeout_seconds': 10}}
        return {'family': family, 'profile': 'test-write', 'operations': ['test_write'], 'values': {'token_env': 'AP_TOKEN'}}

    def test_optional_aliases_flat_and_nested_paths_with_exact_bindings(self):
        for family in backend.FAMILIES:
            with self.subTest(family=family):
                controller = self.prepared(family)
                selected = self.selected(family)
                if family == 'agno':
                    selected['values'].update(token_env='PATH', optional_env='PATH')
                result = controller.check(selected)
                self.assertEqual(result['family'], family)
                self.assertEqual(result['profile'], selected['profile'])
                self.assertEqual(result['configuration_sha256'], backend.digest(backend.configuration_bytes(selected)))
                self.assertEqual(result['contents_sha256'], controller.contents_sha256)
                refs = result['credential_environment_references']
                names = ['token_env', 'optional_env'] if family == 'agno' else ['token_env']
                self.assertEqual([r['field_path'] for r in refs], [[name] if family == 'activepieces' else ['config', name] for name in names])
                self.assertEqual([r['environment_variable'] for r in refs], [selected['values'][name] for name in names])
                self.assertNotIn(CANARY, json.dumps(result))
                install_request = dict(selected, validation_id=result['validation_id'], configuration_sha256=result['configuration_sha256'])
                # Caller mutation must not change the retained validated result.
                refs[0]['environment_variable'] = 'MUTATED'
                job = controller.install(install_request)['job']; controller.join()
                actual = controller.state()['job']['result']
                self.assertEqual(actual['credential_environment_references'][0]['environment_variable'], selected['values']['token_env'])
                self.assertEqual(actual['configuration_sha256'], result['configuration_sha256'])
                self.assertEqual(controller.install(install_request)['job']['id'], job['id'])
                self.assertEqual(sum('--destination' in a and '--config' in a for a, _ in controller.runner.calls), 1)
                self.assertNotIn('env', next(iter(actual['mcp_configuration']['mcpServers'].values())))

    def test_supported_empty_optional_absent_and_unsupported_are_distinct(self):
        controller = self.prepared()
        selected = self.selected()
        empty = controller.check(selected)
        self.assertEqual(empty['credential_environment_references'], [])
        selected['values']['token_env'] = 'TOKEN_REF'
        one = controller.check(selected)
        self.assertEqual(one['credential_environment_references'], [{'field_path': ['config', 'token_env'], 'environment_variable': 'TOKEN_REF'}])
        controller.runner.supported = False
        old = controller.check(selected)
        self.assertNotIn('credential_environment_references', old)
        job = controller.install(dict(selected, validation_id=old['validation_id'], configuration_sha256=old['configuration_sha256']))
        controller.join()
        self.assertNotIn('credential_environment_references', controller.state()['job']['result'])

    def test_malformed_or_incomplete_projection_refuses_without_reflection(self):
        controller = self.prepared()
        selected = self.selected(); selected['values']['token_env'] = 'GOOD_REF'
        valid_row = {'field_path': ['config', 'token_env'], 'environment_variable': 'GOOD_REF'}
        mutations = [
            lambda s: dict(s, valid=1), lambda s: dict(s, valid=False), lambda s: dict(s, profile='other'),
            lambda s: dict(s, scope=CANARY), lambda s: dict(s, credential_environment_references=None),
            lambda s: dict(s, credential_environment_references=[]),
            lambda s: dict(s, credential_environment_references=[valid_row, valid_row]),
            lambda s: dict(s, credential_environment_references=[dict(valid_row, environment_variable=CANARY)]),
            lambda s: dict(s, credential_environment_references=[dict(valid_row, environment_variable='secret value')]),
            lambda s: dict(s, credential_environment_references=[dict(valid_row, field_path=['config', 'host'])]),
            lambda s: dict(s, credential_environment_references=[dict(valid_row, field_path=['config.token_env'])]),
            lambda s: dict(s, credential_environment_references=[dict(valid_row, secret=CANARY)]),
            lambda s: dict(s, credential_environment_references=[dict(valid_row, field_path=['config', 1])]),
            lambda s: dict(s, credential_environment_references=[valid_row] * 257),
            lambda s: b'{"valid":true,"valid":false,"diagnostic":"'+CANARY.encode()+b'"}',
            lambda s: b'NOT_JSON_'+CANARY.encode(),
        ]
        for mutation in mutations:
            with self.subTest(mutation=mutations.index(mutation)):
                controller.runner.transform_projection = mutation
                with self.assertRaises(backend.SetupError) as caught:
                    controller.check(selected)
                self.assertEqual(caught.exception.code, 'invalid_credential_projection')
                self.assertNotIn(CANARY, str(caught.exception))
        self.assertEqual(controller.validations, {})
        self.assertFalse(any('--config' in args and '--destination' in args for args, _ in controller.runner.calls))

    def test_projection_change_or_configuration_mutation_cannot_dispatch_install(self):
        for change in ('support', 'config_during_check', 'config_during_revalidation'):
            with self.subTest(change=change):
                controller = self.prepared()
                selected = self.selected(); selected['values']['token_env'] = 'TOKEN_REF'
                if change == 'config_during_check':
                    controller.runner.after_check = lambda path: path.write_bytes(b'{}')
                    self.error('configuration_changed', lambda: controller.check(selected))
                    self.assertEqual(controller.validations, {})
                    continue
                result = controller.check(selected)
                if change == 'support': controller.runner.supported = False
                else: controller.runner.after_check = lambda path: path.write_bytes(b'{}')
                controller.install(dict(selected, validation_id=result['validation_id'], configuration_sha256=result['configuration_sha256']))
                controller.join()
                self.assertEqual(controller.state()['job']['status'], 'unconfirmed')
                self.assertNotIn('result', controller.state()['job'])
                self.assertFalse(any('--config' in args and '--destination' in args for args, _ in controller.runner.calls))

    def test_reconcile_and_recovery_do_not_restore_reference_observations(self):
        original = self.prepared()
        selected = self.selected(); selected['values']['token_env'] = 'TOKEN_REF'
        result = original.check(selected)
        job = original.install(dict(selected, validation_id=result['validation_id'], configuration_sha256=result['configuration_sha256']))['job']
        original.join()
        self.assertIn('credential_environment_references', original.state()['job']['result'])
        before = len(original.runner.calls)
        observed = original.reconcile({'job_id': job['id']})['observation']
        self.assertNotIn('credential_environment_references', observed['result'])
        self.assertEqual(len(original.runner.calls), before + 1)
        self.assertIn('--verify-existing', original.runner.calls[-1][0])
        record = backend.decode((original.workspace / 'requests' / (job['id'] + '.json')).read_bytes())
        self.assertNotIn('credential_environment_references', record)
        recovered = self.controller(recover_workspace=original.workspace, recover_request=job['id'],
            recover_request_sha256=job['recovery_request']['sha256'])
        self.assertEqual(recovered.runner.calls, [])
        self.assertNotIn('credential_environment_references', json.dumps(recovered.state()))
        self.assertEqual(recovered.reconcile({'job_id': job['id']})['observation']['status'], 'matched')
        self.assertEqual(len(recovered.runner.calls), 1)
        self.assertIn('--verify-existing', recovered.runner.calls[0][0])
        self.assertNotIn('credential_environment_references', json.dumps(recovered.state()))

    def test_http_exposes_only_validated_references_and_redacts_bad_output(self):
        controller = self.prepared()
        server = backend.Server(controller)
        thread = threading.Thread(target=server.serve_forever); thread.start()
        connection = http.client.HTTPConnection('127.0.0.1', server.server_port, timeout=3)
        try:
            selected = self.selected(); selected['values']['token_env'] = 'TOKEN_REF'
            headers = {'Origin': server.origin, 'X-Toolkit-Session': server.capability, 'Content-Type': 'application/json'}
            connection.request('POST', '/api/check', body=backend.encoded(selected), headers=headers)
            reply = connection.getresponse(); data = reply.read()
            self.assertEqual(reply.status, 200)
            self.assertEqual(json.loads(data)['credential_environment_references'][0]['environment_variable'], 'TOKEN_REF')
            self.assertNotIn(CANARY.encode(), data)
            controller.runner.transform_projection = lambda s: dict(s, credential_environment_references=[{'private': CANARY}])
            connection.request('POST', '/api/check', body=backend.encoded(selected), headers=headers)
            reply = connection.getresponse(); data = reply.read()
            self.assertEqual(reply.status, 400)
            self.assertEqual(json.loads(data), {'error': 'invalid_credential_projection'})
            self.assertNotIn(CANARY.encode(), data)
        finally:
            connection.close(); server.shutdown(); server.server_close(); thread.join(3)


class McpHandoffTests(fixtures.ReconciliationMixin, unittest.IsolatedAsyncioTestCase):
    async def test_real_mcp_preserves_projection_and_error_redaction(self):
        controller = self.controller(runner_factory=ReferenceRunner)
        controller.prepare({'family': 'services'}); controller.join()
        owner = adapter.SetupAdapter(controller)
        selected = {'family': 'services', 'profile': 'slack', 'operations': ['auth_test'],
                    'values': {'token_env': 'SLACK_TOKEN', 'timeout_seconds': 10}}
        try:
            async with Client(adapter.create_server(owner)) as client:
                result = await client.call_tool('setup_check', selected)
                self.assertFalse(result.is_error)
                value = result.structured_content
                self.assertEqual(json.loads(result.content[0].text), value)
                self.assertEqual(value['credential_environment_references'], [{'field_path': ['config', 'token_env'], 'environment_variable': 'SLACK_TOKEN'}])
                self.assertNotIn(CANARY, json.dumps(value))
                controller.runner.transform_projection = lambda s: dict(s, valid=False)
                refused = await client.call_tool('setup_check', selected)
                self.assertTrue(refused.is_error)
                self.assertEqual(refused.structured_content, {'error': 'invalid_credential_projection'})
                self.assertNotIn(CANARY, json.dumps(refused.structured_content))
        finally:
            await owner.aclose()
