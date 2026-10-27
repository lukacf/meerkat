"""MCPB custody through the selected setup owner; canonical tool output is inert.

The separate real-tool consumer validates archive semantics. These tests exercise
Controller ownership, exact four-file binding, replay and MCP projections.
"""
import asyncio
import copy
import os
from pathlib import Path
import shutil
import signal
import sys
import threading
import time
import unittest
from unittest.mock import patch

from test_selected_setup import SelectedFixture, SelectedRunner, adapter, backend


class McpbRunner(SelectedRunner):
    def __init__(self, workspace):
        super().__init__(workspace)
        self.check_failure = False
        self.mutate_receipt = None
        self.proof_mutator = None
        self.inspection_budgets = []

    def run(self, argv, capture=False, inspection_timeout=None):
        self.inspection_budgets.append((list(argv), inspection_timeout))
        if len(argv) < 2 or argv[1] not in ('check', 'install', 'verify'):
            return super().run(argv, capture=capture)
        self.calls.append((list(argv), capture))
        def arg(name):
            return argv[argv.index(name) + 1]
        action = argv[1]
        source = Path(arg('--package')).parent
        plan = backend.decode((source / 'package.json').read_bytes())['fixture_plan']
        destination = Path(arg('--destination'))
        if action == 'check':
            assert '--node' not in argv
            assert not destination.exists()
            destination.mkdir()
            (destination / 'inputs').mkdir()
            (destination / 'prepared').mkdir()
            if self.check_failure:
                return 2, b'PRIVATE_OWNER_FAILURE'
            return 0, backend.encoded(plan)
        assert '--node' in argv
        if action == 'install':
            assert '--artifact' in argv
            self.entered.set()
            if self.block:
                self.block.wait(10)
            destination.mkdir()
            (destination / 'ready').mkdir()
            mcp = {'mcpServers': {plan['mcp_name']: {'command': arg('--node'), 'args': [
                str(destination / 'prepared/tree' / plan['entrypoint']['executable']), *plan['entrypoint']['arguments']]}}}
            receipt = {'schema_version': 1, 'scope': 'observed_mcpb_software_installation_only',
                'selection_sha256': arg('--selection-sha256'), 'configuration_sha256': arg('--config-sha256'),
                'package_sha256': backend.digest((source / 'package.json').read_bytes()),
                'destination': str(destination), 'plan': plan,
                'runtime': {'executable': arg('--node'), 'sha256': '7' * 64, 'version': 'v22.0.0'},
                'mcp_configuration': mcp, 'launch_environment': 'Host-owned.'}
            if self.mutate_receipt:
                self.mutate_receipt(receipt)
            # Rust persists canonical JSON without a newline, then adds exactly
            # one newline when printing the same receipt to stdout.
            (destination / 'ready/installed.json').write_bytes(backend.encoded(receipt).removesuffix(b'\n'))
            (destination / 'ready/mcp.json').write_bytes(backend.encoded(receipt['mcp_configuration']).removesuffix(b'\n'))
            return (2 if self.nonzero else 0), b'PRIVATE_OWNER_OUTPUT'
        assert '--artifact' not in argv
        data = (destination / 'ready/installed.json').read_bytes()
        if self.proof_mutator:
            value = backend.decode(data)
            self.proof_mutator(value)
            return 0, backend.encoded(value)
        return 0, data + b'\n'


class McpbFixture(SelectedFixture):
    def setUp(self):
        super().setUp()
        tool = self.root / 'mcpb.tool'
        tool.write_bytes(b'inert canonical MCPB tool\n')
        tool.chmod(0o700)
        self.tools['mcpb'] = {'path': str(tool), 'sha256': backend.digest(tool.read_bytes())}
        self.profile['validators'] = {'mcpb': self.tools['mcpb']}
        self.profile.pop('installer')
        self.write_profile()

    def controller(self, **changes):
        changes.setdefault('runner_factory', McpbRunner)
        changes.setdefault('npm_cli', None)
        return super().controller(**changes)

    def stage(self, profile='mcpb', name='choice', mutate=None):
        if profile != 'mcpb':
            return super().stage(profile, name, mutate)
        source = self.staging / name
        source.mkdir()
        artifact = b'canonical owner validates this inert ZIP fixture'
        config = backend.encoded({'greeting': 'Hej, world'})
        plan = {'schema_version': 1, 'package': {'publisher': 'fixture', 'name': 'chosen', 'version': '1'},
            'package_content_digest': '1' * 64, 'manifest_digest': '2' * 64, 'component': 'server',
            'artifact_digest': backend.digest(artifact), 'mcpb_manifest_digest': '3' * 64,
            'configuration_digest': backend.digest(config), 'prepared_evidence_digest': '5' * 64,
            'entrypoint': {'executable': 'server/main.js', 'arguments': ['--greeting', 'Hej, world']},
            'mcp_name': 'chosen-server', 'minimum_node_version': [22, 0, 0]}
        if mutate:
            mutate(plan)
        for name_, data in {'package.json': backend.encoded({'fixture_plan': plan}),
            'activation.json': backend.encoded({'fixture': 'software-only'}),
            'configuration.json': config, 'artifact.mcpb': artifact}.items():
            (source / name_).write_bytes(data)
        return {'profile': 'mcpb', 'staged_id': name, 'component': 'server',
            'package_sha256': backend.digest((source / 'package.json').read_bytes()),
            'selection_sha256': backend.digest((source / 'activation.json').read_bytes()),
            'configuration_sha256': backend.digest(config)}


class McpbTests(McpbFixture, unittest.TestCase):
    def test_four_frozen_inputs_check_without_runtime_or_install_and_replay(self):
        c = self.controller(); request = self.stage(); checked = c.selected_check(request)
        self.assertEqual(c.selected_check(request), checked)
        self.assertEqual(set(checked['files']), {'package.json', 'activation.json', 'configuration.json', 'artifact.mcpb'})
        self.assertEqual(checked['configuration_sha256'], request['configuration_sha256'])
        self.assertIsNone(c.job); self.assertFalse(Path(checked['destination']).exists())
        self.assertEqual(len(c.runner.calls), 1)
        argv = c.runner.calls[0][0]
        self.assertEqual(argv[1], 'check'); self.assertNotIn('--node', argv)
        scratch = Path(argv[argv.index('--destination') + 1])
        self.assertFalse((scratch / 'ready').exists())
        shutil.rmtree(self.staging / 'choice')
        job = c.selected_install(self.selection(checked))['job']; c.join()
        self.assertEqual(c.job['status'], 'succeeded', c.job)
        self.assertEqual(c.selected_install(self.selection(checked))['job']['id'], job['id'])
        self.assertEqual([a[1] for a, _ in c.runner.calls], ['check', 'install', 'verify'])
        observation = c.reconcile({'job_id': job['id']})['observation']
        self.assertEqual(observation['status'], 'matched')
        self.assertEqual(observation['result']['configuration_sha256'], request['configuration_sha256'])
        self.assertIn('Node executable hash/version', observation['excluded'][0])
        self.assertTrue(c.state()['reconciliation']['available'])
        self.assertTrue(c.job_detail({'job_id': job['id']})['reconciliation']['available'])
        self.assertEqual([(a[1], limit) for a, limit in c.runner.inspection_budgets],
                         [('check', 120.0), ('install', None), ('verify', 120.0), ('verify', 120.0)])

    def test_mixed_and_legacy_profile_compatibility_and_runtime_requirements(self):
        self.profile['validators']['python'] = self.tools['python']; self.write_profile()
        with self.assertRaisesRegex(backend.SetupError, 'invalid_selected_host_profile'):
            self.controller()
        self.profile['installer'] = self.tools['installer']; self.write_profile()
        c = self.controller()
        c.selected_check(self.stage()); c.selected_check(self.stage('python', 'py'))
        self.assertIsNone(c.runner.inspection_budgets[-1][1])
        self.profile['validators'] = {'mcpb': self.tools['mcpb']}; self.write_profile()
        with self.assertRaisesRegex(backend.SetupError, 'invalid_runtime'):
            self.controller(node=None)
        c = self.controller(npm_cli=None)
        self.assertEqual(set(c.selected['validators']), {'mcpb'})

    def test_configuration_required_only_for_mcpb_and_hash_bound(self):
        c = self.controller(); request = self.stage()
        for invalid in ({k:v for k,v in request.items() if k != 'configuration_sha256'},
                        dict(request, configuration_sha256='0' * 64), dict(request, configuration_sha256=True),
                        dict(request, node='/tmp/other'), dict(request, profile='node')):
            with self.assertRaises(backend.SetupError): c.selected_check(invalid)
        self.assertFalse(c.runner.calls)
        checked = c.selected_check(request)
        (self.staging / 'choice/configuration.json').write_bytes(b'{}')
        with self.assertRaises(backend.SetupError): c.selected_check(request)
        c.selected_install(self.selection(checked)); c.join()
        self.assertEqual(c.job['status'], 'succeeded')

    def test_plan_shape_bounds_do_not_accept_unbound_or_hostile_owner_output(self):
        c = self.controller()
        mutations = [lambda p:p.update(schema_version=True), lambda p:p.update(component='other'),
            lambda p:p.update(configuration_digest='0'*64),
            lambda p:p.update(prepared_evidence_digest='no'), lambda p:p.update(minimum_node_version=[True,0,0]),
            lambda p:p.update(minimum_node_version=[1,2,2**32]), lambda p:p.update(closure={}),
            lambda p:p['entrypoint'].update(executable='../escape.js'),
            lambda p:p['entrypoint'].update(arguments=['x'*8193]),
            lambda p:p.update(artifact_digest='0'*64)]
        for index, mutate in enumerate(mutations):
            with self.subTest(index=index), self.assertRaises(backend.SetupError):
                c.selected_check(self.stage(name='bad'+str(index), mutate=mutate))
        self.assertIsNone(c.job)
        self.assertFalse(any(a[1]=='install' for a, _ in c.runner.calls))

    def test_check_failure_leaves_evidence_and_never_creates_proposal_or_job(self):
        c = self.controller(); c.runner.check_failure = True
        with self.assertRaisesRegex(backend.SetupError, 'selected_validation_unavailable'):
            c.selected_check(self.stage())
        self.assertEqual(c.validations, {}); self.assertIsNone(c.job)
        snapshots = list((c.workspace/'selections').iterdir())
        self.assertEqual(len(snapshots), 1)
        self.assertTrue((snapshots[0]/'inspection/prepared').is_dir())
        self.assertFalse((snapshots[0]/'effect.json').exists())

    def test_source_and_frozen_links_tampering_never_followed(self):
        c = self.controller(); request = self.stage()
        for name in ('configuration.json', 'artifact.mcpb'):
            path = self.staging/'choice'/name; data=path.read_bytes(); path.unlink(); path.symlink_to(self.root/'mcpb.tool')
            with self.assertRaises(backend.SetupError): c.selected_check(request)
            path.unlink(); path.write_bytes(data)
        checked=c.selected_check(request)
        path=c.workspace/'selections'/checked['validation_id']/'configuration.json'
        path.unlink(); path.symlink_to(self.staging/'choice/configuration.json')
        with self.assertRaises(backend.SetupError): c.selected_install(self.selection(checked))
        self.assertIsNone(c.job)

    def test_unknown_outcome_replay_and_restart_readback_are_separate(self):
        c=self.controller(); checked=c.selected_check(self.stage()); c.runner.nonzero=True
        job=c.selected_install(self.selection(checked))['job']; c.join()
        self.assertEqual(c.job['status'],'unconfirmed')
        c.selected_install(self.selection(checked))
        self.assertEqual(sum(a[1]=='install' for a,_ in c.runner.calls),1)
        recovered=self.recover(c,job)
        self.assertEqual(recovered.job['status'],'unconfirmed')
        self.assertEqual(recovered.reconcile({'job_id':job['id']})['observation']['status'],'matched')
        self.assertEqual(recovered.job['status'],'unconfirmed')
        self.assertEqual([a[1] for a,_ in recovered.runner.calls],['verify'])
        self.assertEqual(recovered.runner.inspection_budgets[0][1], 120.0)
        with self.assertRaisesRegex(backend.SetupError,'recovery_read_only'):
            recovered.selected_install(self.selection(checked))

    def test_receipt_plan_runtime_configuration_and_exact_proof_binding(self):
        changes = [lambda r:r.update(schema_version=True), lambda r:r.update(configuration_sha256='0'*64),
            lambda r:r.update(package_sha256='0'*64), lambda r:r['plan'].update(prepared_evidence_digest='0'*64),
            lambda r:r['runtime'].update(executable='/other/node'), lambda r:r['runtime'].update(sha256='wrong'),
            lambda r:r['mcp_configuration']['mcpServers']['chosen-server']['args'].append('unapproved')]
        for index, change in enumerate(changes):
            c=self.controller(); checked=c.selected_check(self.stage(name='receipt'+str(index)))
            c.runner.mutate_receipt=change
            c.selected_install(self.selection(checked)); c.join()
            self.assertEqual(c.job['status'],'unconfirmed', index)
        c=self.controller();checked=c.selected_check(self.stage(name='proof'))
        c.selected_install(self.selection(checked));c.join()
        c.runner.proof_mutator=lambda r:r['runtime'].update(version='v23.0.0')
        self.assertEqual(c.reconcile({'job_id':c.job['id']})['observation']['status'],'unconfirmed')

    def test_runner_default_is_unchanged_and_explicit_budget_is_local(self):
        self.assertEqual(backend.INSPECTION_TIMEOUT, 30.0)
        self.assertEqual(backend.MCPB_INSPECTION_TIMEOUT, 120.0)
        c = self.controller(); runner = backend.Runner(c.workspace)
        command = [sys.executable, '-I', '-c', 'import time;time.sleep(.1);print("{}")']
        with patch.object(backend, 'INSPECTION_TIMEOUT', .03):
            with self.assertRaisesRegex(backend.SetupError, 'inspection_unavailable'):
                runner.run(command, capture=True)
            self.assertEqual(runner.run(command, capture=True, inspection_timeout=5), (0, b'{}\n'))
            with self.assertRaisesRegex(backend.SetupError, 'inspection_unavailable'):
                runner.run(command, capture=True)
        with patch.object(backend.subprocess, 'Popen') as spawn:
            for invalid in (0, -1, True, float('inf'), float('nan'), 121):
                with self.assertRaises(backend.SetupError):
                    runner.run(command, capture=True, inspection_timeout=invalid)
            spawn.assert_not_called()

    def test_runner_short_explicit_timeout_kills_owned_group_and_joins_child(self):
        c = self.controller(); runner = backend.Runner(c.workspace)
        command = [sys.executable, '-I', '-c', 'import time;time.sleep(300)']
        original_spawn = backend.subprocess.Popen
        spawned = []
        def spawn(*args, **kwargs):
            process = original_spawn(*args, **kwargs); spawned.append(process); return process
        started = time.monotonic()
        with patch.object(backend.subprocess, 'Popen', side_effect=spawn), patch.object(backend.os, 'killpg', wraps=os.killpg) as kill:
            with self.assertRaisesRegex(backend.SetupError, 'inspection_unavailable'):
                runner.run(command, capture=True, inspection_timeout=.1)
            kill.assert_called_once_with(spawned[0].pid, signal.SIGKILL)
        self.assertIsNotNone(spawned[0].returncode)
        self.assertTrue(spawned[0].stdout.closed)
        self.assertLess(time.monotonic() - started, 3)


class McpbMcpTests(McpbFixture, unittest.IsolatedAsyncioTestCase):
    async def test_actual_mcp_schema_check_install_state_and_recovery(self):
        from mcp import Client
        c=self.controller();request=self.stage(); owner=adapter.SetupAdapter(c)
        try:
            async with Client(adapter.create_server(owner)) as client:
                tools={t.name:t for t in (await client.list_tools()).tools}
                self.assertIn('configuration_sha256',tools['setup_selected_check'].input_schema['properties'])
                bad=await client.call_tool('setup_selected_check',{k:v for k,v in request.items() if k!='configuration_sha256'})
                self.assertTrue(bad.is_error);self.assertFalse(c.runner.calls)
                checked=(await client.call_tool('setup_selected_check',request)).structured_content
                result=await client.call_tool('setup_selected_install',self.selection(checked))
                self.assertFalse(result.is_error, result)
                await asyncio.to_thread(c.join)
                state=(await client.call_tool('setup_state',{})).structured_content
                self.assertEqual(state['job']['family'],'selected-mcpb');self.assertEqual(state['job']['status'],'succeeded')
                job=state['job']
        finally:await owner.aclose()
        recovered=self.recover(c,job);owner=adapter.SetupAdapter(recovered)
        try:
            async with Client(adapter.create_server(owner)) as client:
                self.assertEqual({t.name for t in (await client.list_tools()).tools},{'setup_state','setup_reconcile','setup_recovery_manifest'})
                result=await client.call_tool('setup_reconcile',{'job_id':job['id']})
                self.assertEqual(result.structured_content['observation']['status'],'matched')
        finally:await owner.aclose()

    async def test_cancelled_dispatch_and_close_join_one_install(self):
        c=self.controller();checked=c.selected_check(self.stage());owner=adapter.SetupAdapter(c)
        c.runner.block=threading.Event()
        task=asyncio.create_task(owner.dispatch('/api/selected_install',self.selection(checked)))
        self.assertTrue(await asyncio.to_thread(c.runner.entered.wait,5))
        task.cancel()
        closing=asyncio.create_task(owner.aclose())
        try:
            await asyncio.sleep(.01)
            self.assertFalse(task.done())
            self.assertFalse(closing.done())
            self.assertNotIn('stop',c.job)
            c.runner.block.set()
            with self.assertRaises(asyncio.CancelledError):
                await task
            await closing
            self.assertEqual(c.job['status'],'succeeded')
            self.assertEqual(sum(a[1]=='install' for a,_ in c.runner.calls),1)
        finally:
            c.runner.block.set()
            await asyncio.gather(task,closing,return_exceptions=True)
            await owner.aclose()


if __name__=='__main__':unittest.main()
