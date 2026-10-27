"""Immutable request recovery; fake package owner, no real installation."""
import copy
import hashlib
import http.client
import importlib.util
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('recovery_fixtures', ROOT / 'tests/test_reconciliation.py')
fixtures = importlib.util.module_from_spec(spec)
spec.loader.exec_module(fixtures)
backend = fixtures.backend


def inventory(root):
    return {str(p.relative_to(root)): hashlib.sha256(p.read_bytes()).hexdigest()
            for p in root.rglob('*') if p.is_file()}


class RecoveryTests(fixtures.ReconciliationMixin, unittest.TestCase):
    def recover(self, original, identifier, **changes):
        projection = original.jobs[identifier]['recovery_request']
        args = dict(recover_workspace=original.workspace, recover_request=identifier,
                    recover_request_sha256=projection['sha256'])
        args.update(changes)
        return self.controller(**args)

    def test_retained_before_worker_and_no_verifier_required_at_install(self):
        original = self.controller(verifier=None, verifier_sha256=None)
        original.prepare({'family': 'agno'}); original.join()
        request = self.checked(original)
        real_start = threading.Thread.start
        seen = []
        def start(thread):
            identifier = original.job['id']
            data = backend.read_file(original.workspace, 'requests/' + identifier + '.json')
            seen.append(backend.decode(data))
            self.assertIn(identifier, original.install_intents)
            self.assertEqual(original.validations[request['validation_id']]['job_id'], identifier)
            self.assertEqual(backend.digest(data), original.job['recovery_request']['sha256'])
            real_start(thread)
        with patch.object(threading.Thread, 'start', start):
            job = original.install(request)['job']
        original.join()
        self.assertEqual(len(seen), 1)
        self.assertNotIn('verifier', seen[0])
        self.assertEqual(seen[0]['selection'], self.request())
        self.assertEqual(original.install(request)['job']['id'], job['id'])
        recovered = self.recover(original, job['id'])
        self.assertEqual(recovered.runner.calls, [])
        state = recovered.state()
        self.assertEqual(state['mode'], 'recovery')
        self.assertEqual(state['job']['status'], 'unconfirmed')
        self.assertTrue(state['job']['recovered'])
        self.assertFalse(recovered.validations)
        self.assertFalse(recovered.commands)
        before = inventory(original.workspace)
        result = recovered.reconcile({'job_id': job['id']})
        self.assertEqual(result['observation']['status'], 'matched')
        self.assertEqual(before, inventory(original.workspace))
        self.assertEqual(recovered.job['status'], 'unconfirmed')
        for call, captured in recovered.runner.calls:
            self.assertIn('--verify-existing', call)
            self.assertTrue(captured)

    def test_no_replay_or_setup_even_with_original_validation(self):
        original, request, identifier = self.installed(lost=True)
        recovered = self.recover(original, identifier)
        for path, value in [('/api/prepare', {'family': 'agno'}), ('/api/check', self.request()),
                            ('/api/help', {'family': 'agno', 'profile': 'searxng'}), ('/api/install', request)]:
            self.error('recovery_read_only', lambda: recovered.dispatch(path, value))
        self.assertEqual(recovered.runner.calls, [])
        self.assertEqual(recovered.job['error'], 'historical_outcome_unknown')

    def test_failed_durable_publish_starts_no_thread(self):
        for mode in ('config_sync', 'request_write', 'request_sync', 'workspace_sync', 'parent_sync', 'request_readback'):
            with self.subTest(mode=mode):
                original = self.prepared(); request = self.checked(original)
                count = len(original.runner.calls)
                real_sync, real_write, real_read = backend.sync_file, backend.write_new, backend.read_file
                real_directory_sync = backend.sync_directory
                def directory_sync(path):
                    if (mode == 'workspace_sync' and path == original.workspace) or (mode == 'parent_sync' and path == original.workspace.parent):
                        raise OSError('PRIVATE_FAILURE')
                    real_directory_sync(path)
                def sync(root, name):
                    if (mode == 'config_sync' and root.name == 'configurations') or (mode == 'request_sync' and root.name == 'requests'):
                        raise OSError('PRIVATE_FAILURE')
                    return real_sync(root, name)
                def write(path, data):
                    if mode == 'request_write' and path.parent.name == 'requests':
                        path.write_bytes(b'{')
                        raise OSError('PRIVATE_FAILURE')
                    return real_write(path, data)
                def read(root, name, limit=backend.MAX_JSON):
                    if mode == 'request_readback' and root.name == 'requests':
                        return b'{}'
                    return real_read(root, name, limit)
                with patch.object(backend, 'sync_directory', directory_sync), patch.object(backend, 'sync_file', sync), patch.object(backend, 'write_new', write), patch.object(backend, 'read_file', read), patch.object(threading.Thread, 'start') as start:
                    with self.assertRaises((OSError, backend.SetupError)):
                        original.install(request)
                    start.assert_not_called()
                self.assertEqual(len(original.runner.calls), count)
                self.assertFalse(original.install_intents)
                self.assertIsNone(original.validations[request['validation_id']]['job_id'])
                self.assertTrue(original.operation_lock.acquire(blocking=False)); original.operation_lock.release()
                self.assertFalse(original.closing)
                self.assertFalse(original.recovery_only)
                if mode == 'config_sync':
                    # Failure before canonical reservation is operation-local.
                    self.assertFalse(original.request_retention_failed)
                    job = original.install(request)['job']; original.join()
                    self.assertEqual(original.jobs[job['id']]['status'], 'succeeded')
                else:
                    retained = {p: p.read_bytes() for p in original.workspace.rglob('*') if p.is_file()}
                    self.assertTrue(original.request_retention_failed)
                    self.error('recovery_request_unavailable', lambda: original.install(request))
                    self.error('recovery_request_unavailable', lambda: original.prepare({'family': 'services'}))
                    self.assertEqual({p: p.read_bytes() for p in retained}, retained)
                    self.assertEqual(len(original.runner.calls), count)

    def test_thread_start_refusal_preserves_unknown_request_without_retry(self):
        original = self.prepared(); request = self.checked(original)
        count = len(original.runner.calls)
        with patch.object(threading.Thread, 'start', side_effect=RuntimeError('PRIVATE_FAILURE')):
            job = original.install(request)['job']
        self.assertEqual(job['status'], 'unconfirmed')
        self.assertEqual(len(original.runner.calls), count)
        self.assertEqual(original.install(request)['job'], job)
        recovered = self.recover(original, job['id'])
        self.assertEqual(recovered.reconcile({'job_id': job['id']})['observation']['status'], 'unconfirmed')
        self.assertEqual(recovered.runner.calls, [])

    def test_constructor_failures_release_lock_and_preserve_unknown(self):
        original = self.prepared(); request = self.checked(original)
        count = len(original.runner.calls)
        with patch.object(threading, 'Thread', side_effect=RuntimeError('constructor failed')):
            job = original.install(request)['job']
        self.assertEqual(job['status'], 'unconfirmed')
        self.assertEqual(len(original.runner.calls), count)
        self.assertTrue(original.operation_lock.acquire(blocking=False)); original.operation_lock.release()
        self.assertEqual(original.install(request)['job'], job)
        pending = self.controller()
        with patch.object(threading, 'Thread', side_effect=RuntimeError('constructor failed')):
            prepare = pending.prepare({'family': 'agno'})['job']
        self.assertEqual(prepare['status'], 'unconfirmed')
        self.assertEqual(pending.families['agno']['status'], 'unconfirmed')
        self.assertFalse(pending.runner.calls)
        self.assertTrue(pending.operation_lock.acquire(blocking=False)); pending.operation_lock.release()

    def test_event_allocation_failure_occurs_before_operation_lock(self):
        original = self.prepared(); request = self.checked(original)
        with patch.object(threading, 'Event', side_effect=RuntimeError('event allocation failed')):
            with self.assertRaises(RuntimeError):
                original.install(request)
        self.assertTrue(original.operation_lock.acquire(blocking=False)); original.operation_lock.release()
        self.assertFalse(original.install_intents)
        self.assertEqual(list((original.workspace / 'requests').iterdir()), [])

    def test_failure_after_actual_thread_creation_does_not_dispatch(self):
        original = self.prepared(); request = self.checked(original)
        count = len(original.runner.calls)
        real_start = threading.Thread.start
        def start(thread):
            real_start(thread)
            raise RuntimeError('after thread creation')
        with patch.object(threading.Thread, 'start', start):
            job = original.install(request)['job']
        original.join()
        self.assertEqual(job['status'], 'unconfirmed')
        self.assertEqual(len(original.runner.calls), count)
        self.assertTrue(original.operation_lock.acquire(blocking=False)); original.operation_lock.release()
        self.assertTrue(all(not thread.is_alive() for thread in original.threads))
        self.assertEqual(original.install(request)['job'], job)

    def test_host_selection_and_schema_mismatches_refuse_before_new_workspace(self):
        original, _, identifier = self.installed()
        source = original.workspace / 'requests' / (identifier + '.json')
        raw = source.read_bytes(); record = backend.decode(raw)
        mutations = [lambda r: r.update(schema_version=True), lambda r: r.update(extra='unexpected'),
                     lambda r: r.update(workspace=str(self.root)), lambda r: r.update(job_id='0' * 32),
                     lambda r: r.update(validation_id='../escape'), lambda r: r.update(server_name='changed'),
                     lambda r: r.update(contents_sha256='0' * 64), lambda r: r.update(installer_sha256='0' * 64),
                     lambda r: r['runtimes'].update(python='/unselected/python'),
                     lambda r: r['selection'].update(family='outside'), lambda r: r['selection'].update(operations=[]),
                     lambda r: r['selection'].update(profile='changed'),
                     lambda r: r['selection']['values'].update(timeout=999),
                     lambda r: r['selection']['values'].update(profile='searxng')]
        for mutation in mutations:
            value = copy.deepcopy(record); mutation(value); data = backend.encoded(value); source.write_bytes(data)
            target = self.root / ('bad-' + str(mutations.index(mutation)))
            with self.subTest(value=value), self.assertRaises(backend.SetupError):
                self.recover(original, identifier, workspace=target, recover_request_sha256=backend.digest(data))
            self.assertFalse(target.exists())
        source.write_bytes(raw)
        for changes in ({'recover_request_sha256': '0' * 64}, {'recover_request_sha256': None},
                        {'verifier': None, 'verifier_sha256': None}, {'families': ('services',)}, {'node': None}):
            target = self.root / ('bad-flags-' + str(len(list(self.root.iterdir()))))
            with self.subTest(changes=changes), self.assertRaises(backend.SetupError):
                self.recover(original, identifier, workspace=target, **changes)
            self.assertFalse(target.exists())

    def test_symlinks_and_changed_config_refuse(self):
        original, _, identifier = self.installed()
        request_file = original.workspace / 'requests' / (identifier + '.json')
        record = backend.decode(request_file.read_bytes())
        config = original.workspace / 'configurations' / (record['validation_id'] + '.json')
        for path in (request_file, config, original.workspace / 'installations'):
            saved = path.with_name(path.name + '.saved'); path.rename(saved); path.symlink_to(saved)
            try:
                with self.assertRaises((backend.SetupError, OSError)):
                    self.recover(original, identifier)
            finally:
                path.unlink(); saved.rename(path)
        alias = self.root / 'alias'; alias.symlink_to(original.workspace, target_is_directory=True)
        self.error('invalid_recovery_path', lambda: self.recover(original, identifier, recover_workspace=alias))
        config.write_bytes(b'{}')
        self.error('recovery_configuration_mismatch', lambda: self.recover(original, identifier))

    def test_changed_request_after_import_refuses_without_child(self):
        original, _, identifier = self.installed()
        recovered = self.recover(original, identifier)
        (original.workspace / 'requests' / (identifier + '.json')).write_bytes(b'{}')
        self.assertEqual(recovered.reconcile({'job_id': identifier})['observation']['status'], 'unconfirmed')
        self.assertEqual(recovered.runner.calls, [])

    def test_http_recovered_state_and_effect_refusal(self):
        original, request, identifier = self.installed(lost=True)
        recovered = self.recover(original, identifier)
        server = backend.Server(recovered, 0)
        thread = threading.Thread(target=server.serve_forever, daemon=True); thread.start()
        try:
            conn = http.client.HTTPConnection('127.0.0.1', server.server_port, timeout=5)
            headers = {'Origin': server.origin, 'Host': '127.0.0.1:' + str(server.server_port),
                       'X-Toolkit-Session': server.capability, 'Content-Type': 'application/json'}
            def call(path, payload):
                conn.request('POST', path, body=backend.encoded(payload), headers=headers)
                response = conn.getresponse(); return response.status, json.loads(response.read())
            status, value = call('/api/state', {}); self.assertEqual(status, 200); self.assertEqual(value['mode'], 'recovery')
            status, value = call('/api/install', request); self.assertEqual(status, 409); self.assertEqual(value, {'error': 'recovery_read_only'})
            status, value = call('/api/reconcile', {'job_id': identifier}); self.assertEqual(status, 200); self.assertEqual(value['observation']['status'], 'matched')
            self.assertEqual(len(recovered.runner.calls), 1)
            conn.close()
        finally:
            server.shutdown(); thread.join(5); server.server_close()

    def test_fresh_process_recovers_only_original_request(self):
        original, _, identifier = self.installed(lost=True)
        original.close_admission(); original.join()
        args = {'bundle': str(self.bundle), 'contents_sha256': self.sha,
                'workspace': str(self.root / 'new-process'), 'families': list(backend.FAMILIES),
                'python': sys.executable, 'node': sys.executable, 'npm_cli': sys.executable,
                'verifier': str(self.bundle / 'install-library-consumer.py'),
                'verifier_sha256': self.manifest['files']['install-library-consumer.py']['sha256'],
                'recover_workspace': str(original.workspace), 'recover_request': identifier,
                'recover_request_sha256': original.job['recovery_request']['sha256']}
        selection = self.root / 'selection.json'; selection.write_text(json.dumps(args))
        before = inventory(original.workspace)
        code = """import importlib.util,json,pathlib,sys
p=pathlib.Path(sys.argv[1]); spec=importlib.util.spec_from_file_location('fixtures',p); m=importlib.util.module_from_spec(spec); spec.loader.exec_module(m)
a=json.loads(pathlib.Path(sys.argv[2]).read_text())
for k in ('bundle','workspace','verifier','recover_workspace'): a[k]=pathlib.Path(a[k])
a['runner_factory']=m.ReconciliationRunner
c=m.backend.Controller(**a)
assert c.runner.calls==[]
first=c.state(); result=c.reconcile({'job_id':a['recover_request']})
assert all('--verify-existing' in argv for argv,_ in c.runner.calls)
print(json.dumps({'first':first,'result':result,'calls':len(c.runner.calls),'validations':len(c.validations)}))
"""
        result = subprocess.run([sys.executable, '-I', '-B', '-c', code, str(ROOT / 'tests/test_reconciliation.py'), str(selection)],
                                env={'PATH': '/usr/bin:/bin', 'PYTHONDONTWRITEBYTECODE': '1'}, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        observed = json.loads(result.stdout)
        self.assertEqual(observed['first']['job']['error'], 'historical_outcome_unknown')
        self.assertEqual(observed['result']['observation']['status'], 'matched')
        self.assertEqual(observed['calls'], 1); self.assertEqual(observed['validations'], 0)
        self.assertEqual(before, inventory(original.workspace))


if __name__ == '__main__':
    unittest.main()
