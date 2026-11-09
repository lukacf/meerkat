"""Target admission with inert ELF fixtures; no GitHub binary is executed."""
import asyncio
import argparse
import json
import struct
import subprocess
import sys
import unittest
from pathlib import Path
from unittest.mock import patch

import test_official_github as legacy

owner = legacy.owner
backend = legacy.backend


def elf():
    result = bytearray(160)
    result[:16] = bytes.fromhex('7f454c46020101000000000000000000')
    struct.pack_into('<HHIQQQIHHHHHH', result, 16,
                     2, 62, 1, 0x400078, 64, 0, 0, 64, 56, 1, 0, 0, 0)
    struct.pack_into('<IIQQQQQQ', result, 64, 1, 5, 0, 0x400000, 0x400000, 160, 160, 4096)
    result[120:] = b'INERT_NOT_AN_EXECUTABLE'.ljust(40, b'!')
    return bytes(result)


def repin(root, *, binary=None, target='linux-x86_64', omit_target=False, prose='fixture only'):
    descriptor = json.loads((root / 'delivery.json').read_text())
    selected = descriptor['components'][0]
    selected['runtime_profile'] = prose
    if omit_target:
        selected.pop('target', None)
    else:
        selected['target'] = target
    if binary is not None:
        (root / 'github/github-mcp-server').write_bytes(binary)
        selected['artifact_sha256'] = owner.sha(binary)
    (root / 'delivery.json').write_bytes(owner.encoded(descriptor))
    manifest = json.loads((root / 'CONTENTS.json').read_text())
    for name in ('delivery.json', 'github/github-mcp-server'):
        data = (root / name).read_bytes()
        manifest['files'][name] = {'sha256': owner.sha(data), 'bytes': len(data)}
    (root / 'CONTENTS.json').write_bytes(owner.encoded(manifest))
    return owner.sha((root / 'CONTENTS.json').read_bytes())


class LinuxOwnerTests(legacy.Fixture, unittest.TestCase):
    def setUp(self):
        super().setUp()
        self.sha = repin(self.bundle, binary=elf())

    def test_linux_describe_config_install_and_readback_bind_target_without_execution(self):
        with patch.object(owner.sys, 'platform', 'linux'), patch('platform.machine', return_value='x86_64'), \
                patch.object(owner.subprocess, 'Popen', side_effect=AssertionError('no subprocess')), \
                patch.object(owner.os, 'getenv', side_effect=AssertionError('no environment lookup')):
            description = owner.official_consumer(self.args(official_describe=True))
            checked = owner.official_consumer(self.args(check_config_only=True))
            installed = owner.official_consumer(self.args())
            verified = owner.official_consumer(self.args(verify_existing=True))
        for result in (description, description['help'], checked, installed, verified):
            self.assertEqual(result['target'], 'linux-x86_64')
        self.assertEqual(checked['credential_environment_references'], [
            {'field_path': ['config', 'token_env'], 'environment_variable': owner.OFFICIAL_ENV}])
        self.assertEqual(installed['command'][1:], owner.OFFICIAL_ARGS)
        self.assertEqual(installed['mcp_configuration']['mcpServers']['selected-github']['env'], {})
        self.assertEqual(installed['inputs']['github/github-mcp-server'], owner.sha(elf()))
        self.assertEqual(len(list((self.root / 'installation/notices').iterdir())), 78)

    def test_wrong_host_platform_or_architecture_refuses_before_destination_work(self):
        for platform, machine in [('darwin', 'arm64'), ('linux', 'aarch64'), ('win32', 'x86_64')]:
            for action in ({}, {'verify_existing': True}):
                with self.subTest(platform=platform, machine=machine, action=action), \
                        patch.object(owner.sys, 'platform', platform), patch('platform.machine', return_value=machine), \
                        patch.object(owner, 'new_destination', side_effect=AssertionError('destination examined')), \
                        patch.object(owner, 'existing_destination', side_effect=AssertionError('destination examined')):
                    with self.assertRaises(owner.InstallError):
                        owner.official_consumer(self.args(**action))
        self.assertFalse((self.root / 'installation').exists())

    def test_unknown_targets_and_linux_prose_without_target_do_not_select_linux(self):
        for target in (None, True, 0, '', 'linux-arm64', 'linux', [], {}):
            with self.subTest(target=target):
                self.sha = repin(self.bundle, target=target)
                with self.assertRaises(owner.InstallError):
                    owner.official_consumer(self.args(official_describe=True))
        self.sha = repin(self.bundle, omit_target=True, prose='Linux x86_64 static executable')
        with self.assertRaises(owner.InstallError):
            owner.official_consumer(self.args(official_describe=True))

    def test_crossed_binary_target_and_static_elf_header_fail_closed(self):
        invalid = [b'', elf()[:63], bytes.fromhex('cffaedfe0c0000010000000002000000') + b'INERT']
        for offset, fmt, value in [
                (4, 'B', 1), (5, 'B', 2), (16, '<H', 3), (18, '<H', 183),
                (20, '<I', 0), (32, '<Q', 159), (48, '<I', 1), (52, '<H', 63),
                (54, '<H', 55), (56, '<H', 0), (56, '<H', 129),
                (64, '<I', 3), (64, '<I', 2), (68, '<I', 4),
                (72, '<Q', 160), (96, '<Q', 161), (104, '<Q', 159), (24, '<Q', 1)]:
            value_bytes = bytearray(elf()); struct.pack_into(fmt, value_bytes, offset, value)
            invalid.append(bytes(value_bytes))
        for index, binary in enumerate(invalid):
            with self.subTest(index=index):
                self.sha = repin(self.bundle, binary=binary)
                with self.assertRaises(owner.InstallError):
                    owner.official_consumer(self.args(official_describe=True))
        self.sha = repin(self.bundle, binary=elf(), target='macos-arm64')
        with self.assertRaises(owner.InstallError):
            owner.official_consumer(self.args(official_describe=True))
        self.assertFalse((self.root / 'installation').exists())

    def test_runtime_prose_is_not_execution_policy(self):
        self.sha = repin(self.bundle, prose='macOS arm64 - intentionally contradictory untrusted prose')
        self.assertEqual(owner.official_consumer(self.args(official_describe=True))['target'], 'linux-x86_64')

    def test_receipt_target_or_selected_target_change_cannot_reconcile(self):
        with patch.object(owner.sys, 'platform', 'linux'), patch('platform.machine', return_value='x86_64'):
            owner.official_consumer(self.args())
            path = self.root / 'installation/ready/installed.json'
            receipt = json.loads(path.read_text()); receipt['target'] = 'macos-arm64'
            path.write_bytes(owner.encoded(receipt))
            with self.assertRaises(owner.InstallError):
                owner.official_consumer(self.args(verify_existing=True))

    def test_fixed_operations_and_reference_remain_closed(self):
        original = json.loads(self.config.read_text())
        for change in ({'operations': ['get_me']}, {'config': {'token_env': 'OTHER_REF'}},
                       {'config': {'token_env': owner.OFFICIAL_ENV, 'target': 'linux-x86_64'}},
                       {'target': 'linux-x86_64'}):
            value = {**original, **change}; self.config.write_bytes(owner.encoded(value))
            with self.assertRaises(owner.InstallError):
                owner.official_consumer(self.args(check_config_only=True))

    def test_public_cli_linux_metadata_and_host_refusal(self):
        base = [sys.executable, '-I', '-B', str(legacy.INSTALLER), '--official-bundle', str(self.bundle),
                '--contents-sha256', self.sha, '--component', 'github']
        env = {'PATH': '/usr/bin:/bin', 'PYTHONDONTWRITEBYTECODE': '1'}
        described = subprocess.run(base + ['--official-describe'], capture_output=True, env=env, timeout=10)
        self.assertEqual(described.returncode, 0, described.stderr)
        self.assertEqual(json.loads(described.stdout)['target'], 'linux-x86_64')
        common = ['--config', str(self.config), '--config-sha256', owner.sha(self.config.read_bytes()), '--server-name', 'selected-github']
        checked = subprocess.run(base + common + ['--check-config-only'], capture_output=True, env=env, timeout=10)
        self.assertEqual(checked.returncode, 0, checked.stderr)
        self.assertEqual(json.loads(checked.stdout)['target'], 'linux-x86_64')
        if sys.platform != 'linux':
            refused = subprocess.run(base + common + ['--destination', str(self.root / 'installation')],
                                     capture_output=True, env=env, timeout=10)
            self.assertEqual(refused.returncode, 2)
            self.assertFalse((self.root / 'installation').exists())


class LegacyTargetTests(legacy.Fixture, unittest.TestCase):
    def test_explicit_macos_and_omitted_target_preserve_help_and_configuration(self):
        before = owner.official_consumer(self.args(official_describe=True))
        config_before = owner.official_consumer(self.args(check_config_only=True))
        self.sha = repin(self.bundle, target='macos-arm64', prose='Linux x86_64 - not a selector')
        after = owner.official_consumer(self.args(official_describe=True))
        before.pop('contents_sha256'); after.pop('contents_sha256')
        self.assertEqual(before, after)
        self.assertEqual(owner.official_consumer(self.args(check_config_only=True)), config_before)
        self.assertNotIn('target', config_before)

    def test_target_projection_is_bounded_passthrough_not_target_selection(self):
        self.assertEqual(backend.target_projection('official-github', {}), {})
        self.assertEqual(backend.target_projection('official-github', {'target': 'macos-arm64'}), {})
        self.assertEqual(backend.target_projection('official-github', {'target': 'future-target'}),
                         {'target': 'future-target'})
        for target in (None, True, 12, [], {}, '', 'linux x86_64', 'x' * 65):
            with self.subTest(target=target), self.assertRaises(backend.SetupError):
                backend.target_projection('official-github', {'target': target})


class InProcessLinuxRunner(legacy.RecordingRunner):
    """Use the exact owner with a Linux host fixture for install/readback only.

    Metadata and configuration still use real Python child processes. This
    runner does not claim an actual Linux host or executable launch.
    """
    def run(self, argv, *, capture=False):
        if '--destination' not in argv:
            return super().run(argv, capture=capture)
        self.calls.append(list(argv))
        assert argv[:3] == [str(Path(sys.executable).resolve()), '-I', '-B']
        assert Path(argv[3]).read_bytes() == legacy.INSTALLER.read_bytes()
        value = lambda name: argv[argv.index(name) + 1]
        args = argparse.Namespace(official_bundle=value('--official-bundle'),
            contents_sha256=value('--contents-sha256'), component=value('--component'),
            destination=value('--destination'), config=value('--config'),
            config_sha256=value('--config-sha256'), server_name=value('--server-name'),
            official_describe=False, check_config_only=False, verify_existing='--verify-existing' in argv)
        with patch.object(owner.sys, 'platform', 'linux'), patch('platform.machine', return_value='x86_64'), \
                patch.object(owner.subprocess, 'Popen', side_effect=AssertionError('no official subprocess')):
            result = owner.official_consumer(args)
        if self.drop and not args.verify_existing:
            return 2, b''
        return 0, owner.encoded(result)


class LinuxMCPMetadataTests(legacy.Fixture, unittest.IsolatedAsyncioTestCase):
    async def test_real_setup_mcp_projects_linux_help_and_config_without_binary_execution(self):
        self.sha = repin(self.bundle, binary=elf())
        controller = self.controller()
        async with legacy.connection(controller) as (client, _):
            prepared = await client.call_tool('setup_prepare', {'family': 'official-github'})
            self.assertFalse(prepared.is_error)
            await asyncio.to_thread(controller.join)
            self.assertEqual(controller.job['status'], 'succeeded')
            help_result = await client.call_tool('setup_help', {'family': 'official-github', 'profile': 'selected-reads'})
            self.assertFalse(help_result.is_error)
            help_value = help_result.structured_content
            self.assertEqual(help_value['target'], 'linux-x86_64')
            self.assertIn('Linux x86_64', help_value['prerequisites'][0]['instruction'])
            self.assertFalse(help_value['selectable_operations'])
            checked = await client.call_tool('setup_check', self.request())
            self.assertFalse(checked.is_error)
            self.assertEqual(checked.structured_content['target'], 'linux-x86_64')
            self.assertEqual(checked.structured_content['credential_environment_references'], [
                {'field_path': ['config', 'token_env'], 'environment_variable': owner.OFFICIAL_ENV}])
        self.assertFalse(any('--destination' in call for call in controller.runner.calls))

    async def test_mcp_linux_install_replay_and_recovered_readback_project_same_target(self):
        self.sha = repin(self.bundle, binary=elf())
        pin = owner.sha(legacy.INSTALLER.read_bytes())
        controller = self.controller(runner_factory=InProcessLinuxRunner,
            verifier=legacy.INSTALLER.resolve(), verifier_sha256=pin)
        async with legacy.connection(controller) as (client, _):
            await client.call_tool('setup_prepare', {'family': 'official-github'})
            await asyncio.to_thread(controller.join)
            checked = (await client.call_tool('setup_check', self.request())).structured_content
            self.assertEqual(checked['target'], 'linux-x86_64')
            request = dict(self.request(), validation_id=checked['validation_id'],
                           configuration_sha256=checked['configuration_sha256'])
            first = (await client.call_tool('setup_install', request)).structured_content['job']
            await asyncio.to_thread(controller.join)
            replay = (await client.call_tool('setup_install', request)).structured_content['job']
            self.assertEqual(replay['id'], first['id'])
            self.assertEqual(replay['status'], 'succeeded')
            self.assertEqual(replay['result']['target'], 'linux-x86_64')
            observed = (await client.call_tool('setup_reconcile', {'job_id': first['id']})).structured_content
            self.assertEqual(observed['observation']['status'], 'matched')
            self.assertEqual(observed['observation']['result']['target'], 'linux-x86_64')
        self.assertEqual(len([call for call in controller.runner.calls
                              if '--destination' in call and '--verify-existing' not in call]), 1)
        recovered = self.controller(runner_factory=InProcessLinuxRunner,
            verifier=legacy.INSTALLER.resolve(), verifier_sha256=pin,
            recover_workspace=controller.workspace, recover_request=first['id'],
            recover_request_sha256=first['recovery_request']['sha256'])
        self.assertEqual(recovered.runner.calls, [])
        async with legacy.connection(recovered) as (client, _):
            observed = (await client.call_tool('setup_reconcile', {'job_id': first['id']})).structured_content
            self.assertEqual(observed['observation']['status'], 'matched')
            self.assertEqual(observed['observation']['result']['target'], 'linux-x86_64')
            state = (await client.call_tool('setup_state', {})).structured_content
            self.assertEqual(state['job']['status'], 'unconfirmed')
        self.assertEqual(len(recovered.runner.calls), 1)
        self.assertIn('--verify-existing', recovered.runner.calls[0])

    async def test_mcp_legacy_help_check_install_and_readback_keep_target_omitted(self):
        pin = owner.sha(legacy.INSTALLER.read_bytes())
        controller = self.controller(verifier=legacy.INSTALLER.resolve(), verifier_sha256=pin)
        async with legacy.connection(controller) as (client, _):
            await client.call_tool('setup_prepare', {'family': 'official-github'})
            await asyncio.to_thread(controller.join)
            help_value = (await client.call_tool('setup_help',
                {'family': 'official-github', 'profile': 'selected-reads'})).structured_content
            checked = (await client.call_tool('setup_check', self.request())).structured_content
            self.assertNotIn('target', help_value)
            self.assertNotIn('target', checked)
            request = dict(self.request(), validation_id=checked['validation_id'],
                           configuration_sha256=checked['configuration_sha256'])
            first = (await client.call_tool('setup_install', request)).structured_content['job']
            await asyncio.to_thread(controller.join)
            replay = (await client.call_tool('setup_install', request)).structured_content['job']
            self.assertEqual(replay['status'], 'succeeded')
            self.assertNotIn('target', replay['result'])
            observed = (await client.call_tool('setup_reconcile', {'job_id': first['id']})).structured_content
            self.assertEqual(observed['observation']['status'], 'matched')
            self.assertNotIn('target', observed['observation']['result'])


class LinuxProjectionRefusalTests(legacy.Fixture, unittest.TestCase):
    def setUp(self):
        super().setUp()
        self.sha = repin(self.bundle, binary=elf())

    def controller(self, **updates):
        return super().controller(runner_factory=InProcessLinuxRunner,
            verifier=legacy.INSTALLER.resolve(), verifier_sha256=owner.sha(legacy.INSTALLER.read_bytes()), **updates)

    def test_missing_changed_or_malformed_check_target_is_refused_before_install(self):
        controller = self.controller()
        controller.prepare({'family': 'official-github'}); controller.join()
        original = controller.runner.run
        for target in (None, 'macos-arm64', 'future-target', 'x' * 65):
            def changed(argv, *, capture=False):
                code, raw = original(argv, capture=capture)
                if '--check-config-only' in argv:
                    value = json.loads(raw)
                    if target is None: value.pop('target')
                    else: value['target'] = target
                    raw = owner.encoded(value)
                return code, raw
            with self.subTest(target=target), patch.object(controller.runner, 'run', side_effect=changed):
                with self.assertRaises(backend.SetupError) as error:
                    controller.check(self.request())
                self.assertEqual(error.exception.code, 'invalid_target_projection')
        self.assertFalse(any('--destination' in call for call in controller.runner.calls))

    def test_changed_receipt_target_cannot_be_reported_as_installed(self):
        for target in (None, 'macos-arm64', 'future-target'):
            controller = self.controller(); request = self.checked(controller)
            original = controller.runner.run
            def changed(argv, *, capture=False):
                code, raw = original(argv, capture=capture)
                if '--destination' in argv and '--verify-existing' not in argv:
                    path = Path(argv[argv.index('--destination') + 1]) / 'ready/installed.json'
                    value = json.loads(path.read_text())
                    if target is None: value.pop('target')
                    else: value['target'] = target
                    path.write_bytes(owner.encoded(value))
                return code, raw
            with self.subTest(target=target), patch.object(controller.runner, 'run', side_effect=changed):
                controller.install(request); controller.join()
            self.assertEqual(controller.job['status'], 'unconfirmed')
            self.assertNotIn('result', controller.job)

    def test_missing_or_changed_help_target_cannot_validate_linux_configuration(self):
        controller = self.controller()
        controller.prepare({'family': 'official-github'}); controller.join()
        original = controller.runner.run
        for target in (None, 'macos-arm64', 'future-target'):
            def changed(argv, *, capture=False):
                code, raw = original(argv, capture=capture)
                if '--official-describe' in argv:
                    value = json.loads(raw)
                    if target is None: value['help'].pop('target')
                    else: value['help']['target'] = target
                    raw = owner.encoded(value)
                return code, raw
            with self.subTest(target=target), patch.object(controller.runner, 'run', side_effect=changed):
                with self.assertRaises(backend.SetupError) as error:
                    controller.check(self.request())
                self.assertEqual(error.exception.code, 'invalid_target_projection')
        self.assertFalse(any('--destination' in call for call in controller.runner.calls))

    def test_missing_or_changed_proof_target_cannot_reconcile(self):
        controller = self.controller(); request = self.checked(controller)
        job = controller.install(request)['job']; controller.join()
        original = controller.runner.run
        for target in (None, 'macos-arm64', 'future-target'):
            def changed(argv, *, capture=False):
                code, raw = original(argv, capture=capture)
                if '--verify-existing' in argv:
                    value = json.loads(raw)
                    if target is None: value.pop('target')
                    else: value['target'] = target
                    raw = owner.encoded(value)
                return code, raw
            with self.subTest(target=target), patch.object(controller.runner, 'run', side_effect=changed):
                observed = controller.reconcile({'job_id': job['id']})
                self.assertEqual(observed['observation']['status'], 'unconfirmed')
                self.assertNotIn('result', observed['observation'])
        self.assertEqual(controller.job['status'], 'succeeded')


if __name__ == '__main__':
    unittest.main()
