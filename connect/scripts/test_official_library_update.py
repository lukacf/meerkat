#!/usr/bin/env python3
"""Inert fixtures for the retained official-consumer maintenance reader."""
from __future__ import annotations

import base64
import hashlib
import io
import importlib.util
import json
from pathlib import Path
import shutil
import tarfile
import unittest
from unittest.mock import patch

import test_library_update as support
from test_library_update import digest, raw, tar_bytes


class OfficialTests(unittest.TestCase):
    setUp = support.PublicCommandTests.setUp
    ref = support.PublicCommandTests.ref
    run_cli = support.PublicCommandTests.run_cli
    rejected = support.PublicCommandTests.rejected
    update_json = support.PublicCommandTests.update_json

    def fixture(self, name, component='github', platform='macos-arm64', failed=False):
        directory = self.root / name
        bundle = directory / 'bundle'
        bundle.mkdir(parents=True)
        files = {component + '/mcp.template.json': raw({'command': 'DO_NOT_EXECUTE', 'env': {'TOKEN': 'synthetic'}}),
                 component + '/UPSTREAM-PROFILE.md': b'Inert guide\n'}
        revision = 'a' * 40
        comp = {'catalog_id': component, 'source_revision': revision, 'runtime_profile': platform,
                'configuration_template': component + '/mcp.template.json', 'selected_operations': ['read']}
        provenance = {}
        installed = {}
        if component == 'github':
            payload = b'#!/bin/sh\necho THIS_MUST_NEVER_EXECUTE\nexit 99\n'
            files['github/github-mcp-server'] = payload
            files['github/notices/LICENSE'] = b'Inert notice\n'
            comp.update(artifact='github/github-mcp-server', artifact_sha256=digest(payload))
            archive = io.BytesIO()
            with tarfile.open(fileobj=archive, mode='w:gz') as target:
                item = tarfile.TarInfo('github-mcp-server-' + revision + '/main.go')
                content = b'// Inert source\n'
                item.size = len(content)
                target.addfile(item, io.BytesIO(content))
            (directory / 'source.tar.gz').write_bytes(archive.getvalue())
            audit = {'source_revision': revision, 'source_archive_sha256': digest(archive.getvalue()),
                     'source_files': [{'path': 'main.go', 'sha256': digest(content)}],
                     'binary_sha256': digest(payload), 'binary_dependencies': [
                         {'path': 'example.invalid/inert', 'version': 'v1.0.0', 'sum': 'h1:inert'}],
                     'notices': [{'retained': 'notices/LICENSE', 'sha256': digest(files['github/notices/LICENSE'])}]}
            (directory / 'audit.json').write_bytes(raw(audit))
            build = {'source_revision': revision, 'binary_sha256': digest(payload),
                     'source_audit_sha256': digest(raw(audit)), 'status': 'passed_offline_crossbuild_only',
                     'target': 'linux/amd64', 'cgo_enabled': False,
                     'linked_dependencies': [['example.invalid/inert', 'v1.0.0', 'h1:inert']]}
            (directory / 'build.json').write_bytes(raw(build))
            provenance = {role: self.ref(directory / file) for role, file in
                          [('source_archive', 'source.tar.gz'), ('source_audit', 'audit.json'), ('build_receipt', 'build.json')]}
            installed = {p[len('github/'):]: {'sha256': digest(data), 'bytes': len(data)} for p, data in files.items()
                         if p in ['github/github-mcp-server', 'github/notices/LICENSE']}
        else:
            comp.update(package_json='playwright/package.json', package_lock='playwright/package-lock.json',
                        closure='playwright/closure.json')
            packages, lock, source_match = [], {'': {'dependencies': {'@playwright/mcp': '1.0.0'}}}, {}
            for package in ['@playwright/mcp', 'playwright', 'playwright-core']:
                location = 'node_modules/' + package
                payloads = {'package.json': raw({'name': package, 'version': '1.0.0', 'engines': {'node': '>=20'}}),
                            'index.js': b'throw new Error("must never execute");', 'cli.js': b'// Inert CLI',
                            'LICENSE': b'Inert notice'}
                if package != '@playwright/mcp':
                    payloads.update(NOTICE=b'Inert notice', **{'ThirdPartyNotices.txt': b'Inert notices'})
                archive = 'playwright/archives/' + package.replace('/', '_') + '.tgz'
                archive_data = tar_bytes(payloads)
                files[archive] = archive_data
                sri = 'sha512-' + base64.b64encode(hashlib.sha512(archive_data).digest()).decode()
                row = {'id': package, 'version': '1.0.0', 'path': archive, 'url': 'https://example.invalid/inert',
                       'integrity': sri, 'sha256': digest(archive_data), 'bytes': len(archive_data),
                       'install_path': location, 'payload_files': len(payloads), 'engines': {'node': '>=20'}}
                packages.append(row)
                lock[location] = {'version': row['version'], 'integrity': sri, 'resolved': row['url'], 'engines': row['engines']}
                for p, data in payloads.items():
                    installed[location + '/' + p] = {'sha256': digest(data), 'bytes': len(data)}
                    if p in ['LICENSE', 'NOTICE', 'ThirdPartyNotices.txt']:
                        files['playwright/notices/' + package.replace('/', '__') + '/' + p] = data
                    if package == '@playwright/mcp' and p in ['index.js', 'cli.js', 'LICENSE']:
                        source_match[p] = {'sha256': digest(data), 'equal': True}
            files['playwright/package.json'] = raw({'dependencies': {'@playwright/mcp': '1.0.0'}})
            files['playwright/package-lock.json'] = raw({'lockfileVersion': 3, 'packages': lock})
            files['playwright/closure.json'] = raw({'packages': packages,
                'package_json_sha256': digest(files['playwright/package.json']),
                'package_lock_sha256': digest(files['playwright/package-lock.json'])})
            (directory / 'source-match.json').write_bytes(raw(source_match))
            provenance['source_match'] = self.ref(directory / 'source-match.json')
        files['delivery.json'] = raw({'format': 'meerkat-toolkit.official-consumers.v1', 'components': [comp]})
        for p, data in files.items():
            target = bundle / p
            target.parent.mkdir(parents=True, exist_ok=True)
            target.write_bytes(data)
        contents = {'format': 1, 'files': {p: {'sha256': digest(data), 'bytes': len(data)} for p, data in files.items()}}
        (bundle / 'CONTENTS.json').write_bytes(raw(contents))
        outcome = {'family': component, 'edition': 'initial', 'tools': ['read'], 'status': 'failed' if failed else 'passed'}
        if failed:
            del outcome['tools']
        receipt = {'contents_sha256': digest(raw(contents)), 'delivery_sha256': digest(files['delivery.json']),
                   'status': 'partial_protocol_failure' if failed else 'passed',
                   'bundle_unchanged': True, 'bundle_unchanged_during_protocol': True,
                   'bundle': '/does/not/exist/historical/bundle',
                   'installations': {component + '-initial': {'directory': '/does/not/exist/historical/install', 'files': installed}},
                   'protocol': {'consumers': [outcome], 'outcomes': [outcome]}}
        if component == 'playwright':
            receipt['installations'][component + '-initial']['versions'] = {p: '1.0.0' for p in ['@playwright/mcp', 'playwright', 'playwright-core']}
        (directory / 'receipt.json').write_bytes(raw(receipt))
        (directory / 'schemas.json').write_bytes(raw({'result_type': 'complete', 'next_cursor': None,
            'tools': [{'name': 'read', 'description': 'Inert operation', 'input_schema': {'type': 'object'}}]}))
        spec = {'schema_version': 1, 'family': 'official-consumer', 'component': component,
                'platform': platform, 'edition': 'initial', 'provenance': provenance,
                **{k: self.ref(directory / p) for k, p in [('contents', 'bundle/CONTENTS.json'),
                    ('delivery', 'bundle/delivery.json'), ('receipt', 'receipt.json'), ('schemas', 'schemas.json')]}}
        (directory / 'selection.json').write_bytes(raw(spec))
        return self.ref(directory / 'selection.json')

    def repin(self, name, role, filename):
        directory = self.root / name
        spec = json.loads((directory / 'selection.json').read_bytes())
        if role.startswith('provenance/'):
            spec['provenance'][role.split('/')[1]] = self.ref(directory / filename)
        else:
            spec[role] = self.ref(directory / filename)
        (directory / 'selection.json').write_bytes(raw(spec))
        return self.ref(directory / 'selection.json')

    def rebind_bundle(self, name):
        directory = self.root / name
        bundle = directory / 'bundle'
        members = {str(p.relative_to(bundle)): {'sha256': digest(p.read_bytes()), 'bytes': p.stat().st_size}
                   for p in bundle.rglob('*') if p.is_file() and p.name != 'CONTENTS.json'}
        (bundle / 'CONTENTS.json').write_bytes(raw({'format': 1, 'files': members}))
        self.update_json(directory / 'receipt.json', lambda v: v.update(
            contents_sha256=digest((bundle / 'CONTENTS.json').read_bytes()),
            delivery_sha256=digest((bundle / 'delivery.json').read_bytes())))
        self.repin(name, 'contents', 'bundle/CONTENTS.json')
        self.repin(name, 'delivery', 'bundle/delivery.json')
        return self.repin(name, 'receipt', 'receipt.json')

    def copy_fixture(self, source_name, name):
        # Keep archive bytes identical even across gzip timestamp boundaries.
        directory = self.root / name
        shutil.copytree(self.root / source_name, directory)
        selection = directory / 'selection.json'
        spec = json.loads(selection.read_bytes())
        references = [spec[key] for key in ['contents', 'delivery', 'receipt', 'schemas']]
        for reference in references + list(spec['provenance'].values()):
            self.assertTrue(reference['path'].startswith(source_name + '/'))
            reference['path'] = name + reference['path'][len(source_name):]
        selection.write_bytes(raw(spec))
        return self.ref(selection)

    def test_both_platforms_both_components_are_inert_and_read_only(self):
        for component in ['github', 'playwright']:
            for platform in ['macos-arm64', 'linux-x86_64']:
                with self.subTest(component=component, platform=platform):
                    ref = self.fixture(component + platform, component, platform)
                    before = {str(p): digest(p.read_bytes()) for p in self.root.rglob('*') if p.is_file()}
                    result = self.run_cli(ref, ref)
                    self.assertEqual(result.returncode, 0, result.stderr)
                    value = json.loads(result.stdout)
                    self.assertEqual(value['before']['verification_observations']['component_status'], 'passed')
                    self.assertEqual(before, {str(p): digest(p.read_bytes()) for p in self.root.rglob('*') if p.is_file()})
                    self.assertTrue(all(not d['added'] and not d['changed'] and not d['removed'] for d in value['changes'].values()))
                    self.assertEqual(result.stdout, self.run_cli(ref, ref).stdout)

    def test_failed_linux_browser_stays_failed_with_a_complete_tool_listing(self):
        ref = self.fixture('failed', 'playwright', 'linux-x86_64', failed=True)
        value = json.loads(self.run_cli(ref, ref).stdout)
        self.assertEqual(value['before']['verification_observations']['component_status'], 'failed')
        self.assertEqual(value['before']['verification_observations']['run_status'], 'partial_protocol_failure')
        self.assertIn('outcome: failed', self.run_cli(ref, ref, fmt='markdown').stdout)

    def test_macos_legacy_absence_and_explicit_failure_are_distinct(self):
        for status in [None, 'failed']:
            name = 'legacy' if status is None else 'failed'
            self.fixture(name)
            def change(value):
                row = value['protocol']['consumers'][0]
                if status is None:
                    del row['status']
                else:
                    row.update(status='failed', error='Private failure detail')
            self.update_json(self.root / name / 'receipt.json', change)
            ref = self.repin(name, 'receipt', 'receipt.json')
            result = self.run_cli(ref, ref)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout)['before']['verification_observations']['component_status'],
                             'passed' if status is None else 'failed')
            self.assertNotIn('Private failure detail', result.stdout)

    def test_explicit_unknown_or_contradictory_outcome_is_refused(self):
        for platform in ['macos-arm64', 'linux-x86_64']:
            for field in ['status', 'error', 'error_type', 'traceback']:
                name = platform + field
                self.fixture(name, platform=platform)
                def change(value):
                    rows = value['protocol']['consumers' if platform == 'macos-arm64' else 'outcomes']
                    rows[0][field] = 'Private invalid outcome'
                self.update_json(self.root / name / 'receipt.json', change)
                ref = self.repin(name, 'receipt', 'receipt.json')
                result = self.rejected(ref, ref)
                self.assertNotIn('Private invalid outcome', result.stderr)

    def test_schema_metadata_drift_and_unbound_capture_scope(self):
        before, after = self.fixture('before'), self.fixture('after')
        self.update_json(self.root / 'after/schemas.json', lambda v: v['tools'][0].update(description='Changed privately'))
        after = self.repin('after', 'schemas', 'schemas.json')
        value = json.loads(self.run_cli(before, after).stdout)
        self.assertEqual(value['changes']['operations']['changed']['github/read']['changed_fields'], ['/description'])
        self.assertEqual(value['after']['verification_observations']['schema_association'], 'separately_pinned_capture_not_session_attestation')
        self.assertNotIn('Changed privately', json.dumps(value))

    def test_selected_delivery_contract_changes_without_binary_or_tool_drift(self):
        cases = [
            ('github', 'required_secret_environment', ['OLD_PRIVATE_ENV'], ['NEW_PRIVATE_ENV']),
            ('playwright', 'whole_endpoint_exposed', False, True),
            ('playwright', 'browser_distributed', False, True),
            ('github', 'target', 'old-private-target', 'new-private-target'),
            ('github', 'notices', 'old-private-notices', 'new-private-notices'),
            ('playwright', 'proof_origin', {'record': 'old-private-origin'}, {'record': 'new-private-origin'}),
        ]
        for index, (component, field, old, new) in enumerate(cases):
            with self.subTest(component=component, field=field):
                before_name, after_name = f'before-{index}', f'after-{index}'
                self.fixture(before_name, component)
                self.copy_fixture(before_name, after_name)
                for name, value in [(before_name, old), (after_name, new)]:
                    self.update_json(self.root / name / 'bundle/delivery.json',
                        lambda v: v['components'][0].update({field: value}))
                before, after = self.rebind_bundle(before_name), self.rebind_bundle(after_name)
                result = self.run_cli(before, after)
                self.assertEqual(result.returncode, 0, result.stderr)
                report = json.loads(result.stdout)
                changed = report['changes']['profiles']['changed']
                self.assertEqual(list(changed), [component])
                self.assertEqual(changed[component]['changed_fields'],
                                 ['/' + field + ('/record' if field == 'proof_origin' else '')])
                self.assertFalse(report['same_display_version_different_artifact'])
                self.assertEqual(report['changes']['operations']['changed'], {})
                self.assertEqual(report['changes']['payloads']['changed'], {})
                self.assertNotIn(str(old), result.stdout)
                self.assertNotIn(str(new), result.stdout)
                markdown = self.run_cli(before, after, fmt='markdown')
                self.assertEqual(markdown.returncode, 0, markdown.stderr)
                profiles = markdown.stdout.split('## profiles\n')[1].split('## ')[0]
                self.assertIn('changed: 1;', profiles)
                self.assertIn('- changed: "' + component + '"', profiles)

    def test_selected_delivery_absence_null_false_and_removal_stay_distinct(self):
        for index, (old, new, expected) in enumerate([
            ({}, {'browser_distributed': False}, ['/browser_distributed']),
            ({'browser_distributed': False}, {'browser_distributed': None}, ['/browser_distributed']),
            ({'browser_distributed': None}, {}, ['/browser_distributed']),
        ]):
            before_name, after_name = f'absent-before-{index}', f'absent-after-{index}'
            self.fixture(before_name, 'playwright')
            self.copy_fixture(before_name, after_name)
            for name, fields in [(before_name, old), (after_name, new)]:
                self.update_json(self.root / name / 'bundle/delivery.json',
                                 lambda v: v['components'][0].update(fields))
            before, after = self.rebind_bundle(before_name), self.rebind_bundle(after_name)
            result = self.run_cli(before, after)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(json.loads(result.stdout)['changes']['profiles']['changed']['playwright']['changed_fields'], expected)

    def test_shared_delivery_role_and_scope_are_reported_without_values(self):
        for index, field in enumerate(['role', 'scope']):
            before_name, after_name = f'global-before-{index}', f'global-after-{index}'
            before = self.fixture(before_name)
            self.copy_fixture(before_name, after_name)
            self.update_json(self.root / after_name / 'bundle/delivery.json',
                             lambda v: v.update({field: 'PRIVATE_SHARED_VALUE'}))
            after = self.rebind_bundle(after_name)
            result = self.run_cli(before, after)
            self.assertEqual(result.returncode, 0, result.stderr)
            change = json.loads(result.stdout)['changes']['delivery_metadata']
            self.assertEqual(change['added'], [field])
            self.assertNotIn('PRIVATE_SHARED_VALUE', result.stdout)
            markdown = self.run_cli(before, after, fmt='markdown')
            section = markdown.stdout.split('## delivery_metadata\n')[1].split('## ')[0]
            self.assertIn('- added: "' + field + '"', section)
            self.assertNotIn('PRIVATE_SHARED_VALUE', markdown.stdout)

    def test_unselected_component_changes_do_not_become_selected_profile_drift(self):
        self.fixture('sibling-before')
        self.copy_fixture('sibling-before', 'sibling-after')
        for name, value in [('sibling-before', False), ('sibling-after', True)]:
            self.update_json(self.root / name / 'bundle/delivery.json',
                lambda v: v['components'].append({'catalog_id': 'playwright', 'whole_endpoint_exposed': value}))
        before, after = self.rebind_bundle('sibling-before'), self.rebind_bundle('sibling-after')
        result = self.run_cli(before, after)
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertNotEqual(report['before']['inputs']['delivery']['sha256'], report['after']['inputs']['delivery']['sha256'])
        self.assertTrue(all(not row['added'] and not row['removed'] and not row['changed']
                            for row in report['changes'].values()))

    def test_changed_payload_without_rebinding_is_refused(self):
        ref = self.fixture('one')
        (self.root / 'one/bundle/github/github-mcp-server').write_bytes(b'changed')
        self.rejected(ref, ref, 'input-digest-mismatch')

    def test_coherently_changed_binary_keeps_version_but_reports_byte_drift(self):
        before, after = self.fixture('before'), self.fixture('after')
        directory = self.root / 'after'
        changed = b'Inert changed binary bytes'
        (directory / 'bundle/github/github-mcp-server').write_bytes(changed)
        self.update_json(directory / 'bundle/delivery.json', lambda v: v['components'][0].update(artifact_sha256=digest(changed)))
        self.update_json(directory / 'audit.json', lambda v: v.update(binary_sha256=digest(changed)))
        self.update_json(directory / 'build.json', lambda v: v.update(binary_sha256=digest(changed),
                         source_audit_sha256=digest((directory / 'audit.json').read_bytes())))
        self.update_json(directory / 'receipt.json', lambda v: v['installations']['github-initial']['files'].update({
                         'github-mcp-server': {'sha256': digest(changed), 'bytes': len(changed)}}))
        self.repin('after', 'provenance/source_audit', 'audit.json')
        self.repin('after', 'provenance/build_receipt', 'build.json')
        after = self.rebind_bundle('after')
        result = self.run_cli(before, after)
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertTrue(report['same_display_version_different_artifact'])
        self.assertIn('github/github-mcp-server', report['changes']['payloads']['changed'])
        self.assertEqual(report['changes']['source_files']['changed'], {})

    def test_changed_receipt_map_cannot_authorize_payload(self):
        ref = self.fixture('one')
        self.update_json(self.root / 'one/receipt.json', lambda v: v['installations']['github-initial']['files']['github-mcp-server'].update(sha256='0' * 64))
        changed = self.repin('one', 'receipt', 'receipt.json')
        self.rejected(changed, changed, 'recorded-installation-payload-mismatch')

    def test_wrong_bundle_receipt_is_refused(self):
        ref = self.fixture('one')
        self.update_json(self.root / 'one/receipt.json', lambda v: v.update(contents_sha256='0' * 64))
        changed = self.repin('one', 'receipt', 'receipt.json')
        self.rejected(changed, changed, 'official-receipt-bundle-mismatch')

    def test_partial_or_duplicate_schema_is_refused(self):
        for kind in ['partial', 'duplicate']:
            ref = self.fixture(kind)
            def change(v):
                if kind == 'partial': v['next_cursor'] = 'more'
                else: v['tools'].append(v['tools'][0])
            self.update_json(self.root / kind / 'schemas.json', change)
            ref = self.repin(kind, 'schemas', 'schemas.json')
            self.rejected(ref, ref)

    def test_unknown_or_duplicate_outcomes_are_refused(self):
        for kind in ['unknown', 'duplicate']:
            self.fixture(kind, 'playwright', 'linux-x86_64')
            def change(v):
                if kind == 'unknown': v['protocol']['outcomes'][0]['status'] = 'pending'
                else: v['protocol']['outcomes'].append(v['protocol']['outcomes'][0])
            self.update_json(self.root / kind / 'receipt.json', change)
            ref = self.repin(kind, 'receipt', 'receipt.json')
            self.rejected(ref, ref)

    def test_source_or_build_claim_tampering_is_refused(self):
        for role, filename, change in [
            ('source_audit', 'audit.json', lambda v: v['source_files'][0].update(sha256='0' * 64)),
            ('build_receipt', 'build.json', lambda v: v.update(binary_sha256='0' * 64)),
            ('source_audit', 'audit.json', lambda v: v['notices'][0].update(sha256='0' * 64)),
        ]:
            name = str(len(list(self.root.iterdir())))
            self.fixture(name)
            self.update_json(self.root / name / filename, change)
            ref = self.repin(name, 'provenance/' + role, filename)
            self.rejected(ref, ref)

    def test_runtime_version_claim_tampering_is_refused(self):
        self.fixture('one', 'playwright')
        self.update_json(self.root / 'one/receipt.json', lambda v: v['installations']['playwright-initial']['versions'].update(playwright='2.0.0'))
        ref = self.repin('one', 'receipt', 'receipt.json')
        self.rejected(ref, ref, 'recorded-installation-version-mismatch')

    def test_external_symlink_and_malformed_schema_are_refused(self):
        self.fixture('one')
        target = self.root / 'one/schemas.json'
        target.unlink()
        target.symlink_to('/etc/hosts')
        ref = self.repin('one', 'schemas', 'schemas.json')
        self.rejected(ref, ref, 'input-escapes-root')

    def test_cross_component_compare_is_refused(self):
        self.rejected(self.fixture('github'), self.fixture('playwright', 'playwright'), 'incomparable-package-identities')

    def test_nested_lock_version_drift_is_not_hidden_by_outer_pins(self):
        self.fixture('one', 'playwright')
        lock = self.root / 'one/bundle/playwright/package-lock.json'
        self.update_json(lock, lambda v: v['packages']['node_modules/playwright-core'].update(version='2.0.0'))
        self.update_json(self.root / 'one/bundle/playwright/closure.json',
                         lambda v: v.update(package_lock_sha256=digest(lock.read_bytes())))
        ref = self.rebind_bundle('one')
        self.rejected(ref, ref, 'npm-closure-lock-mismatch')

    def test_removed_notice_is_not_hidden_by_outer_pins(self):
        self.fixture('one', 'playwright')
        (self.root / 'one/bundle/playwright/notices/playwright/NOTICE').unlink()
        ref = self.rebind_bundle('one')
        self.rejected(ref, ref, 'incomplete-playwright-notices')

    def test_crossbuild_dependency_drift_is_refused(self):
        self.fixture('one', 'github', 'linux-x86_64')
        self.update_json(self.root / 'one/build.json', lambda v: v['linked_dependencies'][0].__setitem__(1, 'v2.0.0'))
        ref = self.repin('one', 'provenance/build_receipt', 'build.json')
        self.rejected(ref, ref, 'crossbuild-dependency-mismatch')

    def test_bool_format_and_malformed_file_map_are_refused(self):
        for index, change in enumerate([lambda v: v.update(format=True),
                                       lambda v: v['files'].update({'../escape': {'sha256': '0' * 64, 'bytes': 1}}),
                                       lambda v: v['files'].update({'github/bad': {'sha256': '0' * 64, 'bytes': True}})]):
            name = str(index)
            self.fixture(name)
            self.update_json(self.root / name / 'bundle/CONTENTS.json', change)
            ref = self.repin(name, 'contents', 'bundle/CONTENTS.json')
            self.rejected(ref, ref)

    def test_source_archive_link_is_rejected_even_when_audit_digest_agrees(self):
        self.fixture('one')
        stream = io.BytesIO()
        with tarfile.open(fileobj=stream, mode='w:gz') as target:
            item = tarfile.TarInfo('github-mcp-server-' + 'a' * 40 + '/main.go')
            item.type, item.linkname = tarfile.SYMTYPE, '/etc/hosts'
            target.addfile(item)
        (self.root / 'one/source.tar.gz').write_bytes(stream.getvalue())
        self.update_json(self.root / 'one/audit.json', lambda v: v.update(source_archive_sha256=digest(stream.getvalue())))
        self.update_json(self.root / 'one/build.json', lambda v: v.update(source_audit_sha256=digest((self.root / 'one/audit.json').read_bytes())))
        self.repin('one', 'provenance/source_archive', 'source.tar.gz')
        self.repin('one', 'provenance/source_audit', 'audit.json')
        ref = self.repin('one', 'provenance/build_receipt', 'build.json')
        self.rejected(ref, ref, 'unsupported-source-archive-member')

    def test_truncated_source_archive_is_a_controlled_refusal(self):
        self.fixture('one')
        path = self.root / 'one/source.tar.gz'
        path.write_bytes(path.read_bytes()[:50])
        self.update_json(self.root / 'one/audit.json', lambda v: v.update(source_archive_sha256=digest(path.read_bytes())))
        self.repin('one', 'provenance/source_archive', 'source.tar.gz')
        ref = self.repin('one', 'provenance/source_audit', 'audit.json')
        self.rejected(ref, ref, 'invalid-or-unreadable-selected-evidence')

    def test_expanded_pax_metadata_is_bounded_before_both_tar_readers(self):
        spec = importlib.util.spec_from_file_location('maintenance_reader', support.SCRIPT)
        reader = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(reader)
        for name, read in [
            ('github-mcp-server-' + 'a' * 40 + '/main.go', lambda data: reader.source_tar(data, 'a' * 40)),
            ('package/index.js', lambda data: reader.archive_payload(data, 'activepieces-npm')),
        ]:
            stream = io.BytesIO()
            with tarfile.open(fileobj=stream, mode='w:gz', format=tarfile.PAX_FORMAT) as target:
                item = tarfile.TarInfo(name)
                item.pax_headers = {'comment': 'x' * 65536}
                item.size = 1
                target.addfile(item, io.BytesIO(b'x'))
            # Small limits exercise the real decompression guard without a
            # large allocation; metadata is larger than either allowed budget.
            with patch.object(reader, 'MAX_TOTAL', 8), patch.object(reader, 'TAR_METADATA_ALLOWANCE', 10240):
                with self.assertRaisesRegex(reader.CheckError, 'expanded-tar-size-limit'):
                    read(stream.getvalue())


if __name__ == '__main__':
    unittest.main(verbosity=2)
