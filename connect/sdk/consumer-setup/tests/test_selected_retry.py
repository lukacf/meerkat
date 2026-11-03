"""Explicit selected-package retry proposals preserve old effects and review."""
import asyncio
from concurrent.futures import ThreadPoolExecutor
import copy
import http.client
import json
from pathlib import Path
import shutil
import threading
import unittest
from unittest.mock import patch

from test_selected_setup import SelectedFixture, adapter, backend
from test_selected_mcpb import McpbFixture


def failed(fixture, profile='python', name='choice'):
    c = fixture.controller(); request = fixture.stage(profile, name)
    checked = c.selected_check(request); c.runner.nonzero = True
    job = c.selected_install(fixture.selection(checked))['job']; c.join()
    assert c.jobs[job['id']]['status'] == 'unconfirmed'
    return c, request, checked, c.jobs[job['id']]


def files(path):
    return {str(p.relative_to(path)): p.read_bytes() for p in path.rglob('*') if p.is_file()}


class SelectedRetryTests(SelectedFixture, unittest.TestCase):
    def test_http_admission_and_replay_do_not_install(self):
        c, _, _, first = failed(self)
        browser = backend.BrowserCompanion(c)
        server = browser.server
        def call(token, origin):
            connection = http.client.HTTPConnection(*server.server_address, timeout=3)
            try:
                headers = {'Origin': origin, 'Content-Type': 'application/json'}
                if token is not None: headers['X-Toolkit-Session'] = token
                connection.request('POST', '/api/selected_retry', backend.encoded({'job_id': first['id']}), headers)
                response = connection.getresponse()
                return response.status, json.loads(response.read())
            finally: connection.close()
        try:
            for token, origin in ((None, server.origin), ('wrong', server.origin), (server.capability, 'https://foreign.invalid')):
                self.assertEqual(call(token, origin), (403, {'error': 'request_not_allowed'}))
            self.assertEqual(len(c.validations), 1)
            status, fresh = call(server.capability, server.origin)
            self.assertEqual(status, 200); self.assertEqual(fresh['retry_of'], first['id'])
            self.assertEqual(call(server.capability, server.origin), (200, fresh))
            self.assertIsNone(fresh['job_id']); self.assertEqual(len(c.jobs), 1)
        finally: browser.close()

    def test_fresh_proposal_only_then_explicit_install_keeps_original_replays(self):
        for profile in ('python', 'node'):
            with self.subTest(profile=profile):
                c, request, original, job = failed(self, profile, profile)
                c.reconcile({'job_id': job['id']})
                prior_job = copy.deepcopy(job); observations = copy.deepcopy(c.observations)
                old_path = Path(original['destination']); before = files(old_path)
                calls = len(c.runner.calls)
                fresh = c.selected_retry({'job_id': job['id']})
                self.assertEqual(len(c.runner.calls), calls)
                self.assertEqual(len(c.jobs), 1)
                self.assertEqual(fresh['retry_of'], job['id']); self.assertIsNone(fresh['job_id'])
                self.assertNotEqual(fresh['validation_id'], original['validation_id'])
                self.assertNotEqual(fresh['effect_sha256'], original['effect_sha256'])
                self.assertNotEqual(fresh['destination'], original['destination'])
                self.assertFalse(Path(fresh['destination']).exists())
                self.assertEqual(fresh['files'], original['files'])
                self.assertEqual(c.selected_check(request), original)
                self.assertEqual(c.selected_install(self.selection(original))['job'], prior_job)
                self.assertEqual(c.selected_retry({'job_id': job['id']}), fresh)
                # A new effect must be reviewed; the old effect cannot consume it.
                with self.assertRaisesRegex(backend.SetupError, 'validation_changed'):
                    c.selected_install({'validation_id': fresh['validation_id'], 'effect_sha256': original['effect_sha256']})
                shutil.rmtree(self.staging / profile)
                c.runner.nonzero = False
                new_job = c.selected_install(self.selection(fresh))['job']; c.join()
                self.assertEqual(c.jobs[new_job['id']]['status'], 'succeeded')
                self.assertEqual(new_job['retry_of'], job['id'])
                self.assertEqual(c.selected_retry({'job_id': job['id']})['job_id'], new_job['id'])
                self.assertEqual(c.selected_install(self.selection(fresh))['job']['id'], new_job['id'])
                self.assertEqual(files(old_path), before); self.assertEqual(c.jobs[job['id']], prior_job)
                self.assertEqual(c.observations, observations)
                self.assertEqual(c.reconcile({'job_id': new_job['id']})['observation']['status'], 'matched')

    def test_failed_successor_can_be_retried_but_ancestor_never_branches(self):
        c, _, original, first = failed(self)
        fresh = c.selected_retry({'job_id': first['id']})
        second = c.selected_install(self.selection(fresh))['job']; c.join()
        third = c.selected_retry({'job_id': second['id']})
        self.assertNotEqual(fresh['validation_id'], third['validation_id'])
        self.assertEqual(c.selected_retry({'job_id': first['id']})['validation_id'], fresh['validation_id'])
        self.assertEqual(c.selected_retry({'job_id': second['id']}), third)
        self.assertEqual(c.selected_install(self.selection(original))['job']['id'], first['id'])
        self.assertEqual(c.selected_attempts, 3)

    def test_replay_is_observation_after_source_change_and_busy(self):
        c, _, old, first = failed(self)
        fresh = c.selected_retry({'job_id': first['id']})
        (c.workspace / 'selections' / old['validation_id'] / 'artifact.whl').write_bytes(b'changed')
        c.operation_lock.acquire()
        try: self.assertEqual(c.selected_retry({'job_id': first['id']}), fresh)
        finally: c.operation_lock.release()
        c.runner.nonzero = False
        job = c.selected_install(self.selection(fresh))['job']; c.join()
        self.assertEqual(c.jobs[job['id']]['status'], 'succeeded')

    def test_refuses_running_succeeded_foreign_recovery_and_disabled(self):
        c, _, old, first = failed(self)
        for status in ('running', 'succeeded'):
            first['status'] = status
            with self.assertRaisesRegex(backend.SetupError, 'selected_retry_unavailable'):
                c.selected_retry({'job_id': first['id']})
        first['status'] = 'unconfirmed'
        c.operation_lock.acquire()
        try:
            with self.assertRaisesRegex(backend.SetupError, 'operation_in_progress'):
                c.selected_retry({'job_id': first['id']})
        finally: c.operation_lock.release()
        with self.assertRaisesRegex(backend.SetupError, 'unknown_job'):
            self.controller().selected_retry({'job_id': first['id']})
        recovered = self.recover(c, first)
        self.assertFalse(recovered.state()['selected_retry']['available'])
        with self.assertRaisesRegex(backend.SetupError, 'recovery_read_only'):
            recovered.selected_retry({'job_id': first['id']})
        c.selected = None
        self.assertFalse(c.state()['selected_retry']['available'])
        with self.assertRaisesRegex(backend.SetupError, 'selected_setup_unavailable'):
            c.selected_retry({'job_id': first['id']})
        self.assertEqual(c.selected_attempts, 1)

    def test_changed_frozen_source_or_tool_refused_before_copy(self):
        for index, target in enumerate(('effect.json', 'plan.json', 'artifact.whl', 'request', 'tool')):
            with self.subTest(target=target):
                c, _, old, first = failed(self, name='choice' + str(index))
                path = c.workspace / 'selections' / old['validation_id'] / target
                if target == 'request': path = c.workspace / 'requests' / (first['id'] + '.json')
                if target == 'tool': path = c.workspace / 'selected-tools/python'
                path.write_bytes(b'changed')
                with self.assertRaises(backend.SetupError): c.selected_retry({'job_id': first['id']})
                self.assertEqual(c.selected_attempts, 1); self.assertEqual(c.selected_retries, {})
                self.assertEqual(len(list((c.workspace / 'selections').iterdir())), 1)

    def test_partial_copy_and_budget_preserve_previous_artifacts(self):
        c, _, old, first = failed(self)
        before = files(Path(old['destination'])); write = backend.write_new
        def failing(path, data):
            if path.name == 'plan.json': raise OSError('copy interrupted')
            return write(path, data)
        with patch.object(backend, 'write_new', side_effect=failing):
            with self.assertRaises(OSError): c.selected_retry({'job_id': first['id']})
        self.assertEqual(c.selected_attempts, 2); self.assertEqual(c.selected_retries, {})
        self.assertEqual(len(c.validations), 1)
        self.assertEqual(len(list((c.workspace / 'selections').iterdir())), 2)
        self.assertEqual(files(Path(old['destination'])), before)
        c.selected_attempts = 128
        with self.assertRaisesRegex(backend.SetupError, 'new_workspace_required'):
            c.selected_retry({'job_id': first['id']})
        self.assertEqual(len(list((c.workspace / 'selections').iterdir())), 2)

    def test_thread_start_failure_and_durable_lineage_recover_without_authority(self):
        c = self.controller(); old = c.selected_check(self.stage())
        with patch.object(backend.threading.Thread, 'start', side_effect=RuntimeError('not started')):
            first = c.selected_install(self.selection(old))['job']
        self.assertEqual(first['status'], 'unconfirmed')
        fresh = c.selected_retry({'job_id': first['id']})
        with patch.object(backend.threading.Thread, 'start', side_effect=RuntimeError('not started')):
            second = c.selected_install(self.selection(fresh))['job']
        self.assertEqual(c.selected_retry({'job_id': first['id']})['job_id'], second['id'])
        path = c.workspace / 'requests' / (second['id'] + '.json')
        record = backend.decode(path.read_bytes()); self.assertEqual(record['retry_of'], first['id'])
        recovered = self.recover(c, second)
        self.assertEqual(recovered.job['retry_of'], first['id']); self.assertTrue(recovered.job['recovered'])
        with self.assertRaisesRegex(backend.SetupError, 'recovery_read_only'):
            recovered.selected_retry({'job_id': second['id']})
        for value in (None, True, second['id'], 'A' * 32, 'bad'):
            bad = {**record, 'retry_of': value}; raw = backend.encoded(bad); path.write_bytes(raw)
            with self.assertRaisesRegex(backend.SetupError, 'invalid_recovery_request'):
                backend.load_selected_recovery(c.workspace, second['id'], backend.digest(raw), c.selected_binding)

    def test_concurrent_requests_never_allocate_more_than_one_successor(self):
        c, _, _, first = failed(self)
        gate = threading.Barrier(8)
        def request():
            gate.wait(5)
            try: return c.selected_retry({'job_id': first['id']})
            except backend.SetupError as error:
                self.assertEqual(error.code, 'operation_in_progress')
                return None
        with ThreadPoolExecutor(max_workers=8) as executor:
            replies = list(executor.map(lambda _: request(), range(8)))
        accepted = [reply for reply in replies if reply is not None]
        self.assertTrue(accepted)
        self.assertEqual(len({reply['validation_id'] for reply in accepted}), 1)
        self.assertEqual(c.selected_attempts, 2); self.assertEqual(len(c.validations), 2)
        self.assertEqual(c.selected_retry({'job_id': first['id']}), accepted[0])

    def test_final_durable_sync_failure_never_dispatches_and_retains_reviewed_proposal(self):
        c, _, _, first = failed(self)
        prior_job = copy.deepcopy(first)
        fresh = c.selected_retry({'job_id': first['id']})
        calls = len(c.runner.calls); sync = backend.sync_directory
        def fail_final(path):
            if path == c.workspace.parent: raise OSError('durability unavailable')
            return sync(path)
        with patch.object(backend, 'sync_directory', side_effect=fail_final):
            with self.assertRaises(OSError): c.selected_install(self.selection(fresh))
        self.assertEqual(len(c.runner.calls), calls); self.assertEqual(len(c.jobs), 1)
        self.assertFalse(Path(fresh['destination']).exists())
        self.assertEqual(len(list((c.workspace / 'requests').iterdir())), 2)
        retained = files(c.workspace)
        for call in (lambda: c.selected_retry({'job_id': first['id']}),
                     lambda: c.selected_install(self.selection(fresh))):
            with self.assertRaisesRegex(backend.SetupError, 'recovery_request_unavailable'):
                call()
        self.assertEqual(len(c.runner.calls), calls)
        self.assertEqual(files(c.workspace), retained)
        self.assertEqual(c.jobs[first['id']], prior_job)
        self.assertFalse(c.operation_lock.locked())
        self.assertFalse(c.closing)
        self.assertFalse(c.recovery_only)
        # The previous canonical request can still be inspected, without retry.
        observed = c.reconcile({'job_id': first['id']})['observation']
        self.assertEqual(observed['status'], 'matched')
        self.assertEqual(c.jobs[first['id']], prior_job)
        self.assertEqual(c.jobs[first['id']]['status'], 'unconfirmed')
        self.assertEqual(files(c.workspace), retained)


class McpbRetryTests(McpbFixture, unittest.TestCase):
    def test_four_file_snapshot_retry_keeps_configuration_and_uses_existing_owner(self):
        c, _, old, first = failed(self, 'mcpb')
        fresh = c.selected_retry({'job_id': first['id']})
        self.assertEqual(fresh['configuration_sha256'], old['configuration_sha256'])
        snapshot = c.workspace / 'selections' / fresh['validation_id']
        self.assertEqual(set(files(snapshot)), {'package.json', 'activation.json', 'configuration.json', 'artifact.mcpb', 'plan.json', 'effect.json'})
        shutil.rmtree(self.staging); shutil.rmtree(c.workspace / 'selections' / old['validation_id'])
        c.runner.nonzero = False
        second = c.selected_install(self.selection(fresh))['job']; c.join()
        self.assertEqual(c.jobs[second['id']]['status'], 'succeeded')
        self.assertEqual(c.selected_retry({'job_id': first['id']})['job_id'], second['id'])


class SelectedRetryMcpTests(SelectedFixture, unittest.IsolatedAsyncioTestCase):
    async def test_mcp_retries_proposal_only_then_consumes_reviewed_effect(self):
        from mcp import Client
        c, _, old, first = failed(self); owner = adapter.SetupAdapter(c)
        try:
            async with Client(adapter.create_server(owner)) as client:
                tool = next(t for t in (await client.list_tools()).tools if t.name == 'setup_selected_retry')
                self.assertFalse(tool.annotations.read_only_hint); self.assertTrue(tool.annotations.idempotent_hint)
                invalid = await client.call_tool('setup_selected_retry', {'job_id': first['id'], 'destination': '/forbidden'})
                self.assertTrue(invalid.is_error)
                result = await client.call_tool('setup_selected_retry', {'job_id': first['id']})
                self.assertFalse(result.is_error, result); fresh = result.structured_content
                self.assertIsNone(fresh['job_id']); self.assertEqual(len(c.jobs), 1)
                c.runner.nonzero = False
                result = await client.call_tool('setup_selected_install', self.selection(fresh))
                self.assertFalse(result.is_error, result); second = result.structured_content['job']; c.join()
                result = await client.call_tool('setup_selected_retry', {'job_id': first['id']})
                self.assertEqual(result.structured_content['job_id'], second['id'])
        finally: await owner.aclose()

    async def test_cancelled_reply_retains_one_proposal(self):
        c, _, _, first = failed(self); owner = adapter.SetupAdapter(c)
        dispatch = c.dispatch; entered = threading.Event(); release = threading.Event()
        def delayed(route, arguments):
            result = dispatch(route, arguments)
            entered.set(); release.wait(5)
            return result
        try:
            with patch.object(c, 'dispatch', side_effect=delayed):
                task = asyncio.create_task(owner.dispatch('/api/selected_retry', {'job_id': first['id']}))
                self.assertTrue(await asyncio.to_thread(entered.wait, 5))
                task.cancel(); release.set()
                with self.assertRaises(asyncio.CancelledError): await task
            fresh = await owner.dispatch('/api/selected_retry', {'job_id': first['id']})
            self.assertEqual(len(c.validations), 2); self.assertEqual(len(c.jobs), 1)
            self.assertEqual(c.selected_retries[first['id']], fresh['validation_id'])
            self.assertIsNone(fresh['job_id'])
        finally:
            release.set(); await owner.aclose()
