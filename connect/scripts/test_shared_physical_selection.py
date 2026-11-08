"""Offline selection/oracle controls using fixture files and mocked process discovery.

These are not native workflow, process ownership, or physical-entry acceptance.
"""
import contextlib
import copy
import hashlib
import json
import os
from pathlib import Path
import shlex
import socket
import tempfile
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import verify_supplied_shared_workflow as w


def write(path, value):
    raw = json.dumps(value, sort_keys=True).encode()
    path.write_bytes(raw)
    path.chmod(0o600)
    return hashlib.sha256(raw).hexdigest()


@contextlib.contextmanager
def fixture(restored=None):
    with tempfile.TemporaryDirectory(dir='/private/tmp') as root_name, \
            tempfile.TemporaryDirectory(prefix='connect-setup-', dir='/tmp') as socket_name:
        root, sockets = Path(root_name).resolve(), Path(socket_name).resolve()
        artifact = root / 'state/.meerkat-connect/fixture/setup'
        config = artifact / 'prepared/config/mcp.json'
        helper = artifact / 'prepared/distribution/setup/local_session.py'
        config.parent.mkdir(parents=True)
        helper.parent.mkdir(parents=True)
        for path in (config, helper, helper.with_name('mcp_adapter.py'), helper.with_name('backend.py')):
            path.write_bytes(b'fixture pinned bytes')
        digest = hashlib.sha256(b'fixture pinned bytes').hexdigest()
        python = '/selected/python'
        args = ['-i', 'PATH=/usr/bin:/bin', 'PYTHONDONTWRITEBYTECODE=1', python, '-I', '-B', str(helper),
                '--configuration', str(config), '--configuration-sha256', digest, '--helper-sha256', digest,
                '--adapter-sha256', digest, '--backend-sha256', digest]
        handoff = artifact / 'prepare.stdout'
        handoff_digest = write(handoff, {'mcpServers': {'toolkit-setup': {'command': '/usr/bin/env', 'args': args}}})
        owner = 'a' * 64
        lifetime_path = artifact / ('shared-lifetime-' + owner + '.json')
        fence = {key: {'device': 1, 'inode': value} for value, key in enumerate(('root', 'directory', 'lease'), 1)}
        lifetime = {'schema_version': 2, 'owner': owner, 'artifact_directory': str(artifact), 'fence': fence,
                    'command': '/usr/bin/env', 'arguments': args, 'peers': 1, 'restored': restored,
                    'preparer': {'prepare.stdout': handoff_digest}}
        lifetime_digest = write(lifetime_path, lifetime)
        physical = {'artifact_directory': str(artifact), 'owner': owner,
                    'lifetime_sha256': lifetime_digest, 'fence': fence}
        record = {'id': w.ADMISSION, 'class': {'id': w.CLASS_ID}, 'instance': {'member': w.G_ID},
                  'workflow': {'workflow': 'fixture-workflow'}, 'route': {'id': 'fixture-route'},
                  'destination': {'context': 'fixture-session', 'context_generation': 1},
                  'phase': {'kind': 'prepared', 'context': 'fixture-session', 'context_generation': 1}}
        document = {'schema_version': 1, 'admission': copy.deepcopy(record), 'session': 'fixture-session',
                    'resources': {'artifact_directory': str(artifact),
                                  'handoff': {'path': str(handoff), 'sha256': handoff_digest}},
                    'physical': physical, 'closed': None}
        record['phase']['kind'] = 'active'
        observed = {'status': {'identity': w.G_ID, 'state': 'active', 'session_id': 'fixture-session'},
                    'canonical': {'state': 'observed', 'authorization': {'state': 'succeeded'}, 'record': record}}
        def select(value):
            raw = json.dumps(value, sort_keys=True).encode()
            digest = hashlib.sha256(raw).hexdigest()
            path = artifact / ('workflow-resource-' + digest + '.json')
            path.write_bytes(raw)
            path.chmod(0o600)
            record['workflow_resource'] = {'path': str(path), 'sha256': digest}
            return path
        select(document)
        peer_path = sockets / '0.sock'
        with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
            listener.bind(str(peer_path))
            peer_path.chmod(0o600)
            listener.listen(1)
            lifetime_args = ['--shared-lifetime', str(lifetime_path), '--shared-lifetime-sha256', lifetime_digest]
            commands = {10: args[3:] + ['--shared-peers', '1', '--shared-close-id', owner] + lifetime_args,
                        11: args[3:7] + ['--shared-peer', str(peer_path)] + lifetime_args}
            if restored is not None:
                commands[10].extend(['--shared-restore-sha256', restored])
            yield SimpleNamespace(root=root, artifact=artifact, handoff=handoff, observed=observed,
                physical=physical, document=document, select=select, commands=commands, python=python,
                lifetime_path=lifetime_path, lifetime=lifetime, peer_path=peer_path)


class SharedPhysicalSelectionControls(unittest.TestCase):
    def inspect(self, case, **kwargs):
        def ps(command, **unused):
            self.assertEqual(command[:2], ['/bin/ps', '-p'])
            return SimpleNamespace(returncode=0, stdout=shlex.join(case.commands[int(command[2])]) + '\n')
        with patch.object(w.v, 'child_snapshot', return_value=[{'pid': pid} for pid in case.commands]), \
                patch.object(w.v.subprocess, 'run', side_effect=ps):
            return w.shared_peer(case.root, 7, case.python, **kwargs)

    def test_current_launch_joins_canonical_resource_and_both_lifetime_commands(self):
        with fixture() as case:
            result = self.inspect(case, observed=case.observed)
            self.assertEqual(result['physical'], case.physical)
            self.assertEqual(result['canonical_resource']['sha256'],
                             case.observed['canonical']['record']['workflow_resource']['sha256'])
            self.assertEqual(result['path'], str(case.peer_path))
            self.assertEqual(result['lifetime_file']['sha256'], case.physical['lifetime_sha256'])
            self.assertIsNone(result['restore_sha256'])
            explicit = self.inspect(case, physical=case.physical)
            self.assertEqual(explicit['physical'], result['physical'])
            self.assertNotIn('canonical_resource', explicit)
            with self.assertRaisesRegex(AssertionError, 'one canonical source'):
                self.inspect(case, observed=case.observed, physical=case.physical)

    def test_restored_controller_requires_its_exact_retained_snapshot(self):
        with fixture('b' * 64) as case:
            result = self.inspect(case, observed=case.observed)
            self.assertEqual(result['restore_sha256'], 'b' * 64)
            case.commands[10][-1] = 'c' * 64
            with self.assertRaisesRegex(AssertionError, 'immutable native lifetime'):
                self.inspect(case, observed=case.observed)

    def test_refused_foreign_or_noncurrent_observation_cannot_select_physical_owner(self):
        for field, value in [('authorization', {'state': 'refused'}), ('state', 'unsettled')]:
            with self.subTest(field=field), fixture() as case:
                case.observed['canonical'][field] = value
                with self.assertRaises(AssertionError):
                    w.selected_shared_physical(case.root, case.observed)
        for field, value in [('identity', 'foreign'), ('session_id', 'foreign'), ('state', 'retired')]:
            with self.subTest(field=field), fixture() as case:
                case.observed['status'][field] = value
                with self.assertRaises(AssertionError):
                    w.selected_shared_physical(case.root, case.observed)

    def test_changed_hash_closed_missing_physical_or_foreign_admission_refuses(self):
        changes = [('closed', lambda d: d.update(closed={'old': True})),
                   ('physical', lambda d: d.pop('physical')),
                   ('workflow', lambda d: d['admission']['workflow'].update(workflow='foreign')),
                   ('session', lambda d: d.update(session='foreign')),
                   ('physical-artifact', lambda d: d['physical'].update(artifact_directory='/private/tmp/foreign'))]
        for label, mutate in changes:
            with self.subTest(label=label), fixture() as case:
                changed = copy.deepcopy(case.document)
                mutate(changed)
                case.select(changed)
                with self.assertRaises(AssertionError):
                    w.selected_shared_physical(case.root, case.observed)
        with fixture() as case:
            selected = Path(case.observed['canonical']['record']['workflow_resource']['path'])
            selected.write_bytes(selected.read_bytes() + b' ')
            with self.assertRaisesRegex(AssertionError, 'digest changed'):
                w.selected_shared_physical(case.root, case.observed)

    def test_canonical_resource_cannot_borrow_changed_preparer_or_other_acceptance_root(self):
        with fixture() as case:
            with tempfile.TemporaryDirectory(dir='/private/tmp') as other:
                with self.assertRaisesRegex(AssertionError, 'escaped'):
                    w.selected_shared_physical(Path(other), case.observed)
            case.handoff.write_bytes(case.handoff.read_bytes() + b' ')
            with self.assertRaisesRegex(AssertionError, 'digest changed'):
                w.selected_shared_physical(case.root, case.observed)

    def test_missing_or_foreign_bridge_lifetime_is_not_current_process_proof(self):
        for label, change in [('legacy', lambda argv: argv.__delitem__(slice(6, None))),
                              ('path', lambda argv: argv.__setitem__(7, '/tmp/foreign.json')),
                              ('hash', lambda argv: argv.__setitem__(9, 'f' * 64))]:
            with self.subTest(label=label), fixture() as case:
                change(case.commands[11])
                with self.assertRaisesRegex(AssertionError, 'unique owned shared peer'):
                    self.inspect(case, observed=case.observed)
        with fixture() as case:
            case.commands[12] = list(case.commands[11])
            with self.assertRaisesRegex(AssertionError, 'unique owned shared peer'):
                self.inspect(case, observed=case.observed)

    def test_changed_physical_fence_or_controller_owner_refuses(self):
        with fixture() as case:
            changed = copy.deepcopy(case.document)
            changed['physical']['fence']['lease']['inode'] += 1
            case.select(changed)
            with self.assertRaisesRegex(AssertionError, 'immutable native lifetime'):
                self.inspect(case, observed=case.observed)
        with fixture() as case:
            case.commands[10][-5] = 'f' * 64
            with self.assertRaisesRegex(AssertionError, 'physical owner identity'):
                self.inspect(case, observed=case.observed)


if __name__ == '__main__':
    unittest.main()
