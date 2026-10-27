#!/usr/bin/env python3
"""Focused public CLI tests using inert, local artifact fixtures, never connectors."""
from __future__ import annotations

import base64
import csv
import hashlib
import io
import json
from pathlib import Path
import subprocess
import sys
import tarfile
import tempfile
import unittest
import zipfile

SCRIPT = Path(__file__).with_name('check-library-update.py')
ENV = {'PATH': '/usr/bin:/bin:/usr/sbin:/sbin', 'LANG': 'en_US.UTF-8',
       'PYTHONHASHSEED': '0', 'PYTHONDONTWRITEBYTECODE': '1'}


def digest(data):
    return hashlib.sha256(data).hexdigest()


def raw(value):
    return json.dumps(value, sort_keys=True, separators=(',', ':')).encode()


def wheel_files():
    prefix = 'meerkat_toolkit_agno-0.1.0.dist-info/'
    files = {
        prefix + 'METADATA': b'Metadata-Version: 2.4\nName: meerkat-toolkit-agno\nVersion: 0.1.0\nRequires-Python: >=3.11\nRequires-Dist: agno==2.6.16\n\n',
        'meerkat_toolkit_agno/__init__.py': b'raise RuntimeError("must never execute artifact")\n',
        'meerkat_toolkit_agno/PROFILE-GUIDE.md': b'Fixture profile guide\n',
    }
    licenses = ['LICENSE-APACHE', 'LICENSE-MIT', 'LICENSE-Agno', 'THIRD_PARTY_NOTICES.md']
    for name in licenses:
        files[prefix + 'licenses/' + name] = b'Inert license fixture\n'
    files['meerkat_toolkit_agno/PACKAGE-SOURCES.json'] = raw({
        'schema_version': 1, 'distribution': 'meerkat-toolkit-agno',
        'source_mapping': {'src/meerkat_toolkit_agno/__init__.py': {'sha256': digest(files['meerkat_toolkit_agno/__init__.py'])}},
        'packaging_sha256': {name: digest(files[prefix + 'licenses/' + name]) for name in licenses},
        'profile_guide': {'sha256': digest(files['meerkat_toolkit_agno/PROFILE-GUIDE.md'])},
    })
    return files


def zip_bytes(files, after_record=None):
    record = 'meerkat_toolkit_agno-0.1.0.dist-info/RECORD'
    rows = [[name, 'sha256=' + base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b'=').decode(), str(len(data))]
            for name, data in sorted(files.items())]
    text = io.StringIO()
    csv.writer(text).writerows(rows + [[record, '', '']])
    files = dict(files, **{record: text.getvalue().encode()})
    if after_record:
        after_record(files)
    out = io.BytesIO()
    with zipfile.ZipFile(out, 'w') as archive:
        for name, data in files.items():
            archive.writestr(name, data)
    return out.getvalue()


def npm_files():
    files = {
        'package.json': raw({'name': '@rkat/toolkit-activepieces', 'version': '0.1.0', 'engines': {'node': '>=22'}, 'dependencies': {'zod': '3.25.76'}}),
        'README.md': b'Inert profile instructions\n',
        'dist/donor-factory.js': b'throw new Error("must never execute artifact");\n',
        'dist/BUILD-DEPENDENCY-LOCK.json': raw({'lockfileVersion': 3, 'packages': {'': {}, 'node_modules/example': {'version': '1.0.0', 'integrity': 'sha512-fixture'}}}),
        'vendor/activepieces/pieces/example.ts': b'// Inert donor fixture\n',
        'licenses/dep/PACKAGE.json': raw({'name': 'example', 'version': '1.0.0', 'license': 'MIT'}),
        'licenses/dep/LICENSE': b'Inert license fixture\n',
    }
    files['vendor/SOURCE-MANIFEST.json'] = raw({'schema_version': 1, 'revision': 'fixed-fixture', 'files': [
        {'path': 'pieces/example.ts', 'sha256': digest(files['vendor/activepieces/pieces/example.ts'])}]})
    files['licenses/BUNDLED-PACKAGES.json'] = raw({'schema_version': 1, 'packages': [{
        'package_path': 'node_modules/example', 'retained_package_json': 'licenses/dep/PACKAGE.json',
        'package_json_sha256': digest(files['licenses/dep/PACKAGE.json']),
        'notices': [{'path': 'licenses/dep/LICENSE', 'sha256': digest(files['licenses/dep/LICENSE'])}],
    }]})
    files['dist/BUILD-MANIFEST.json'] = raw({
        'schema_version': 1, 'upstream_revision': 'fixed-fixture',
        'source_manifest_sha256': digest(files['vendor/SOURCE-MANIFEST.json']),
        'package_lock_sha256': digest(files['dist/BUILD-DEPENDENCY-LOCK.json']),
        'bundle_sha256': digest(files['dist/donor-factory.js']),
    })
    return files


def tar_bytes(files, link=None):
    out = io.BytesIO()
    with tarfile.open(fileobj=out, mode='w:gz') as archive:
        for name, data in files.items():
            item = tarfile.TarInfo('package/' + name)
            item.size = len(data)
            archive.addfile(item, io.BytesIO(data))
        if link:
            item = tarfile.TarInfo('package/escape')
            item.type, item.linkname = tarfile.SYMTYPE, link
            archive.addfile(item)
    return out.getvalue()


class PublicCommandTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='library-update-test-')
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)

    def ref(self, path):
        return {'path': str(path.relative_to(self.root)), 'sha256': digest(path.read_bytes())}

    def fixture(self, name, family='agno-wheel', artifact=None, profiles=None, schema_default=None, dependencies=None):
        directory = self.root / name
        directory.mkdir()
        profiles = profiles or ['fixture']
        descriptions = {profile: {'profile': profile, 'tools': [{'name': 'read', 'description': 'An inert read operation',
            'parameters' if family == 'agno-wheel' else 'inputSchema': {'type': 'object', 'properties': {'flag': {'default': schema_default}}}}]}
            for profile in profiles}
        if family == 'agno-wheel':
            artifact = artifact or zip_bytes(wheel_files())
            deps = dependencies or b'agno==2.6.16\n'
            schemas = raw(descriptions)
            receipt = {'wheel_sha256': digest(artifact), 'console_profiles': profiles,
                       'runtime_constraints_sha256': digest(deps)}
        else:
            artifact = artifact or tar_bytes(npm_files())
            deps = dependencies or raw({'node_modules/zod': {'version': '3.25.76', 'integrity': 'sha512-fixture', 'resolved': 'https://example.invalid/never-fetched'}})
            schemas = raw({'schemas': [{'profile': profile, 'config': {'profile': profile, 'token_env': 'UNSET'},
                'status': 0, 'description': descriptions[profile]} for profile in profiles], 'cases': []})
            receipt = {'archive_sha256': digest(artifact), 'installed_profiles_with_schema_validation': len(profiles),
                       'runtime_dependency_packages': len(json.loads(deps))}
        for filename, data in [('artifact', artifact), ('schemas.json', schemas), ('dependencies', deps), ('receipt.json', raw(receipt))]:
            (directory / filename).write_bytes(data)
        return self.reseal(name, family)

    def reseal(self, name, family='agno-wheel'):
        directory = self.root / name
        members = {p.name: digest(p.read_bytes()) for p in directory.iterdir() if p.name not in ['manifest.json', 'selection.json']}
        (directory / 'manifest.json').write_bytes(raw({'files': members} if family == 'agno-wheel' else members))
        spec = {'schema_version': 1, 'family': family}
        for role, filename in [('artifact', 'artifact'), ('schemas', 'schemas.json'), ('runtime_dependencies', 'dependencies'),
                               ('receipt', 'receipt.json'), ('evidence_manifest', 'manifest.json')]:
            spec[role] = self.ref(directory / filename)
        (directory / 'selection.json').write_bytes(raw(spec))
        return self.ref(directory / 'selection.json')

    def update_json(self, path, change):
        value = json.loads(path.read_bytes())
        change(value)
        path.write_bytes(raw(value))

    def run_cli(self, before, after, proposal=None, fmt='json'):
        args = [sys.executable, str(SCRIPT), '--root', str(self.root), '--format', fmt]
        for side, ref in [('before', before), ('after', after)] + ([('proposal', proposal)] if proposal else []):
            args += ['--' + side, ref['path'], '--' + side + '-sha256', ref['sha256']]
        return subprocess.run(args, env=ENV, capture_output=True, text=True, timeout=15)

    def rejected(self, before, after, reason=None, proposal=None):
        result = self.run_cli(before, after, proposal)
        self.assertEqual(result.returncode, 2, result.stdout + result.stderr)
        self.assertEqual(result.stdout, '')
        self.assertTrue(result.stderr.startswith('library-update: '), result.stderr)
        self.assertNotIn('Traceback', result.stderr)
        if reason:
            self.assertIn(reason, result.stderr)
        return result

    def test_wheel_change_readout_is_deterministic_and_read_only(self):
        before = self.fixture('before')
        files = wheel_files()
        files['meerkat_toolkit_agno/PROFILE-GUIDE.md'] = b'Updated guide\n'
        source = json.loads(files['meerkat_toolkit_agno/PACKAGE-SOURCES.json'])
        source['profile_guide']['sha256'] = digest(files['meerkat_toolkit_agno/PROFILE-GUIDE.md'])
        files['meerkat_toolkit_agno/PACKAGE-SOURCES.json'] = raw(source)
        after = self.fixture('after', artifact=zip_bytes(files), profiles=['fixture', 'added'], dependencies=b'agno==2.6.16\nnew-dep==1.2.3\n')
        tree = {str(p): digest(p.read_bytes()) for p in self.root.rglob('*') if p.is_file()}
        first, second = self.run_cli(before, after), self.run_cli(before, after)
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(first.stdout, second.stdout)
        report = json.loads(first.stdout)
        self.assertEqual(report['result'], 'comparison_only_maintainer_review_required')
        self.assertTrue(report['same_display_version_different_artifact'])
        self.assertEqual(report['changes']['profiles']['added'], ['added'])
        self.assertEqual(report['changes']['runtime_dependencies']['added'], ['new-dep'])
        self.assertIn('PROFILE-GUIDE.md', report['changes']['configuration_docs']['changed'])
        self.assertIsNone(report['proposal'])
        self.assertEqual(tree, {str(p): digest(p.read_bytes()) for p in self.root.rglob('*') if p.is_file()})
        text1, text2 = self.run_cli(before, after, fmt='markdown'), self.run_cli(before, after, fmt='markdown')
        self.assertEqual(text1.returncode, 0, text1.stderr)
        self.assertEqual(text1.stdout, text2.stdout)
        self.assertIn('maintainer review required', text1.stdout)

    def test_npm_valid_observation_does_not_execute_bundle(self):
        before = self.fixture('before', 'activepieces-npm')
        after = self.fixture('after', 'activepieces-npm', profiles=['fixture', 'added'])
        result = self.run_cli(before, after)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(json.loads(result.stdout)['changes']['profiles']['added'], ['added'])

    def test_boolean_numeric_schema_change_is_visible(self):
        before = self.fixture('before', schema_default=True)
        after = self.fixture('after', schema_default=1)
        result = self.run_cli(before, after)
        self.assertEqual(result.returncode, 0, result.stderr)
        change = json.loads(result.stdout)['changes']['operations']['changed']['fixture/read']
        self.assertEqual(change['changed_fields'], ['/parameters/properties/flag/default'])
        self.assertNotEqual(change['before_sha256'], change['after_sha256'])

    def test_removed_profile_and_operation_are_not_replacements(self):
        before = self.fixture('before', profiles=['fixture', 'removed'])
        after = self.fixture('after')
        result = self.run_cli(before, after)
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout)
        self.assertEqual(report['changes']['profiles']['removed'], ['removed'])
        self.assertEqual(report['changes']['operations']['removed'], ['removed/read'])
        self.assertIsNone(report['proposal'])

    def test_wrong_selected_artifact_digest(self):
        before = self.fixture('before')
        (self.root / 'before/artifact').write_bytes(b'tampered')
        self.rejected(before, before, 'input-digest-mismatch')

    def test_schema_pin_must_also_match_evidence_manifest(self):
        before = self.fixture('before')
        self.update_json(self.root / 'before/selection.json', lambda s: s['schemas'].update(sha256='0' * 64))
        before = self.ref(self.root / 'before/selection.json')
        self.rejected(before, before, 'input-not-bound-by-evidence-manifest')

    def test_receipt_cannot_name_another_artifact(self):
        before = self.fixture('before')
        self.update_json(self.root / 'before/receipt.json', lambda r: r.update(wheel_sha256='0' * 64))
        before = self.reseal('before')
        self.rejected(before, before, 'receipt-artifact-mismatch')

    def test_record_catches_payload_changed_after_record_creation(self):
        data = zip_bytes(wheel_files(), lambda files: files.update({'meerkat_toolkit_agno/__init__.py': b'changed'}))
        before = self.fixture('before', artifact=data)
        self.rejected(before, before, 'wheel-record-mismatch')

    def test_packaged_source_hash_catches_stale_manifest(self):
        files = wheel_files()
        files['meerkat_toolkit_agno/__init__.py'] = b'changed with valid RECORD'
        before = self.fixture('before', artifact=zip_bytes(files))
        self.rejected(before, before, 'missing-or-mismatched-artifact-member')

    def test_source_manifest_cannot_omit_packaged_code(self):
        files = wheel_files()
        files['meerkat_toolkit_agno/extra.py'] = b'pass\n'
        before = self.fixture('before', artifact=zip_bytes(files))
        self.rejected(before, before, 'incomplete-wheel-source-manifest')

    def test_missing_npm_license_payload_is_rejected_with_updated_outer_pins(self):
        files = npm_files()
        del files['licenses/dep/LICENSE']
        before = self.fixture('before', 'activepieces-npm', artifact=tar_bytes(files))
        self.rejected(before, before, 'missing-or-mismatched-artifact-member')

    def test_npm_bundle_digest_is_checked(self):
        files = npm_files()
        files['dist/donor-factory.js'] = b'changed'
        before = self.fixture('before', 'activepieces-npm', artifact=tar_bytes(files))
        self.rejected(before, before, 'missing-or-mismatched-artifact-member')

    def test_npm_vendor_code_cannot_escape_source_manifest(self):
        files = npm_files()
        files['vendor/activepieces/extra.ts'] = b'extra'
        before = self.fixture('before', 'activepieces-npm', artifact=tar_bytes(files))
        self.rejected(before, before, 'incomplete-donor-source-manifest')

    def test_zip_traversal_is_rejected(self):
        files = wheel_files()
        files['../escape'] = b'never extracted'
        before = self.fixture('before', artifact=zip_bytes(files))
        self.rejected(before, before, 'invalid-relative-path')
        self.assertFalse((self.root / 'escape').exists())

    def test_tar_link_is_rejected(self):
        before = self.fixture('before', 'activepieces-npm', artifact=tar_bytes(npm_files(), link='/tmp/not-read'))
        self.rejected(before, before, 'unsupported-npm-member')

    def test_symlink_input_escape_is_rejected(self):
        before = self.fixture('before')
        with tempfile.TemporaryDirectory(prefix='library-update-outside-') as outside:
            target = Path(outside) / 'artifact'
            path = self.root / 'before/artifact'
            target.write_bytes(path.read_bytes())
            path.unlink()
            path.symlink_to(target)
            self.rejected(before, before, 'input-escapes-root')

    def test_duplicate_json_key_is_rejected(self):
        before = self.fixture('before')
        path = self.root / 'before/selection.json'
        path.write_bytes(path.read_bytes()[:-1] + b',"family":"agno-wheel"}')
        before = self.ref(path)
        self.rejected(before, before, 'duplicate-json-key')

    def test_boolean_schema_version_is_rejected(self):
        before = self.fixture('before')
        self.update_json(self.root / 'before/selection.json', lambda r: r.update(schema_version=True))
        before = self.ref(self.root / 'before/selection.json')
        self.rejected(before, before, 'unsupported-selection-format')

    def test_unknown_approval_field_is_rejected(self):
        before = self.fixture('before')
        self.update_json(self.root / 'before/selection.json', lambda r: r.update(approved=True))
        before = self.ref(self.root / 'before/selection.json')
        self.rejected(before, before, 'unexpected-record-fields')

    def test_duplicate_tool_and_receipt_profile_disagreement(self):
        for name, duplicate in [('duplicate', True), ('disagree', False)]:
            with self.subTest(name=name):
                before = self.fixture(name)
                path = self.root / name / 'schemas.json'
                self.update_json(path, lambda r: r['fixture']['tools'].append(r['fixture']['tools'][0]) if duplicate else r.update(extra=r['fixture']))
                before = self.reseal(name)
                self.rejected(before, before, 'duplicate-tool-identity' if duplicate else 'profile-receipt-mismatch')

    def test_unpinned_dependency_is_rejected(self):
        before = self.fixture('before', dependencies=b'agno>=2.6\n')
        self.rejected(before, before, 'unsupported-python-pin')

    def test_cross_family_comparison_is_rejected(self):
        before = self.fixture('before')
        after = self.fixture('after', 'activepieces-npm')
        self.rejected(before, after, 'incomparable-package-identities')

    def test_malformed_nested_schema_fails_without_content_or_traceback(self):
        before = self.fixture('before', 'activepieces-npm')
        self.update_json(self.root / 'before/schemas.json', lambda r: r['schemas'][0].update(description='secret-canary'))
        before = self.reseal('before', 'activepieces-npm')
        self.rejected(before, before, 'invalid-or-unreadable-selected-evidence')
        self.assertNotIn('secret-canary', self.run_cli(before, before).stderr)

    def test_recorded_config_contents_are_not_echoed(self):
        before = self.fixture('before', 'activepieces-npm')
        self.update_json(self.root / 'before/schemas.json', lambda r: r['schemas'][0]['config'].update(value='secret-canary'))
        before = self.reseal('before', 'activepieces-npm')
        after = self.fixture('after', 'activepieces-npm')
        result = self.run_cli(before, after)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertNotIn('secret-canary', result.stdout + result.stderr)
        self.assertIn('fixture', json.loads(result.stdout)['changes']['recorded_configuration_hashes']['changed'])

    def test_explicit_replacement_and_deprecation_remain_proposals(self):
        before, after = self.fixture('before'), self.fixture('after', profiles=['new'])
        from_ref = {'artifact_sha256': json.loads((self.root / 'before/selection.json').read_bytes())['artifact']['sha256'], 'profile': 'fixture'}
        to_ref = {'artifact_sha256': json.loads((self.root / 'after/selection.json').read_bytes())['artifact']['sha256'], 'profile': 'new'}
        for action in ['deprecate', 'replace']:
            with self.subTest(action=action):
                value = {'schema_version': 1, 'action': action, 'from': from_ref, 'reason': 'Maintainer proposal only',
                         'migration_guidance': 'Review grants and configuration explicitly; do not retarget automatically.'}
                if action == 'replace':
                    value['replacement'] = to_ref
                path = self.root / (action + '.json')
                path.write_bytes(raw(value))
                result = self.run_cli(before, after, self.ref(path))
                self.assertEqual(result.returncode, 0, result.stderr)
                report = json.loads(result.stdout)
                self.assertEqual(report['proposal']['disposition'], 'proposal_only_no_support_change_or_retargeting')
                self.assertEqual(report['proposal']['from'], from_ref)
                self.assertNotIn('approved', report['proposal'])
                value['from'] = dict(from_ref, artifact_sha256='0' * 64)
                path.write_bytes(raw(value))
                self.rejected(before, after, 'proposal-target-mismatch', self.ref(path))

    def numeric_fixture(self, name, literal):
        self.fixture(name, schema_default='NUMERIC_PROBE')
        path = self.root / name / 'schemas.json'
        captured = path.read_bytes()
        self.assertEqual(captured.count(b'"NUMERIC_PROBE"'), 1)
        path.write_bytes(captured.replace(b'"NUMERIC_PROBE"', literal.encode()))
        return self.reseal(name)

    def reject_numeric_literals(self, literals):
        before = self.fixture('before')
        for index, literal in enumerate(literals):
            with self.subTest(literal=literal):
                after = self.numeric_fixture('after' + str(index), literal)
                result = self.rejected(before, after, 'unsupported-lossy-json-number')
                self.assertNotIn(literal, result.stderr)

    def test_numeric_overflow_is_refused(self):
        self.reject_numeric_literals(['1e309', '1e310'])

    def test_numeric_underflow_is_refused(self):
        self.reject_numeric_literals(['1e-999', '2e-999'])

    def test_numeric_rounding_is_refused(self):
        self.reject_numeric_literals(['0.100000000000000000001', '0.100000000000000000002'])

    def test_common_decimal_change_remains_visible(self):
        before = self.numeric_fixture('before', '0.1')
        after = self.numeric_fixture('after', '0.2')
        result = self.run_cli(before, after)
        self.assertEqual(result.returncode, 0, result.stderr)
        report = json.loads(result.stdout, parse_constant=lambda value: self.fail('Nonfinite JSON output'))
        change = report['changes']['operations']['changed']['fixture/read']
        self.assertEqual(change['changed_fields'], ['/parameters/properties/flag/default'])
        self.assertNotEqual(change['before_sha256'], change['after_sha256'])

    def test_unchanged_common_decimal_is_valid_and_deterministic(self):
        before = self.numeric_fixture('before', '0.1')
        after = self.numeric_fixture('after', '0.1')
        first, second = self.run_cli(before, after), self.run_cli(before, after)
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(first.stdout, second.stdout)
        report = json.loads(first.stdout, parse_constant=lambda value: self.fail('Nonfinite JSON output'))
        self.assertEqual(report['changes']['operations']['changed'], {})
        self.assertEqual(report['changes']['operations']['unchanged_count'], 1)

    def test_exact_finite_and_large_integer_changes_remain_visible(self):
        for index, pair in enumerate([('1.25', '1.5'), ('9007199254740992', '9007199254740993')]):
            with self.subTest(pair=pair):
                before = self.numeric_fixture('before' + str(index), pair[0])
                after = self.numeric_fixture('after' + str(index), pair[1])
                result = self.run_cli(before, after)
                self.assertEqual(result.returncode, 0, result.stderr)
                report = json.loads(result.stdout, parse_constant=lambda value: self.fail('Nonfinite JSON output'))
                change = report['changes']['operations']['changed']['fixture/read']
                self.assertEqual(change['changed_fields'], ['/parameters/properties/flag/default'])
                self.assertNotEqual(change['before_sha256'], change['after_sha256'])

    def test_exact_numeric_spelling_and_nonfinite_tokens(self):
        before = self.numeric_fixture('before', '1.25')
        after = self.numeric_fixture('after', '125e-2')
        first, second = self.run_cli(before, after), self.run_cli(before, after)
        self.assertEqual(first.returncode, 0, first.stderr)
        self.assertEqual(first.stdout, second.stdout)
        self.assertEqual(json.loads(first.stdout)['changes']['operations']['changed'], {})
        for index, literal in enumerate(['NaN', 'Infinity', '-Infinity']):
            with self.subTest(literal=literal):
                after = self.numeric_fixture('constant' + str(index), literal)
                self.rejected(before, after, 'nonfinite-json-number')

def services_files():
    ns = 'meerkat_toolkit_services/'
    prefix = 'meerkat_toolkit_services-0.1.0.dist-info/'
    files = {prefix + 'METADATA': b'Metadata-Version: 2.4\nName: meerkat-toolkit-services\nVersion: 0.1.0\nRequires-Python: >=3.11\nRequires-Dist: httpx==0.28.1\nLicense-Expression: MIT OR Apache-2.0\n\n',
             ns + '__init__.py': b'raise RuntimeError("must never import services")\n',
             ns + 'GUIDE.md': b'Fixture service guide\n', ns + 'HOST.md': b'Fixture host guide\n'}
    for name in ['LICENSE-APACHE', 'LICENSE-MIT', 'THIRD_PARTY_NOTICES.md']:
        files[prefix + 'licenses/' + name] = b'Inert original notice\n'
    other = {'README.md': digest(files[ns + 'GUIDE.md']), 'src/' + ns + 'GUIDE.md': digest(files[ns + 'GUIDE.md']),
             'src/' + ns + 'HOST.md': digest(files[ns + 'HOST.md']), 'pyproject.toml': digest(b'not shipped source metadata')}
    other.update({name: digest(files[prefix + 'licenses/' + name]) for name in ['LICENSE-APACHE', 'LICENSE-MIT', 'THIRD_PARTY_NOTICES.md']})
    files[ns + 'PACKAGE-SOURCES.json'] = raw({'distribution': 'meerkat-toolkit-services', 'namespace': ns[:-1],
        'source_mapping': {'src/' + ns + '__init__.py': {'canonical_source': 'sdk/python/src/meerkat_toolkit/__init__.py', 'sha256': digest(files[ns + '__init__.py'])}},
        'guide_mapping': {'src/' + ns + 'HOST.md': {'canonical_source': 'sdk/python/HOST.md', 'canonical_sha256': digest(b'canonical unshipped guide'), 'sha256': digest(files[ns + 'HOST.md']), 'literal_replacements': [{'from': 'meerkat_toolkit.', 'to': 'meerkat_toolkit_services.'}]}},
        'other_payload_sha256': other, 'scope': 'Source path labels are claims; actual retained bytes are checked.'})
    return files


def services_zip(files, mutate=None):
    prefix = next(name.rsplit('/', 1)[0] + '/' for name in files if name.endswith('.dist-info/METADATA'))
    rows = [[name, 'sha256=' + base64.urlsafe_b64encode(hashlib.sha256(data).digest()).rstrip(b'=').decode(), str(len(data))] for name, data in sorted(files.items())]
    text = io.StringIO(); csv.writer(text).writerows(rows + [[prefix + 'RECORD', '', '']])
    files = dict(files, **{prefix + 'RECORD': text.getvalue().encode()})
    if mutate: mutate(files)
    output = io.BytesIO()
    with zipfile.ZipFile(output, 'w') as z:
        for name, content in files.items(): z.writestr(name, content)
    return output.getvalue()


class ServicesCommandTests(unittest.TestCase):
    setUp = PublicCommandTests.setUp
    ref = PublicCommandTests.ref
    run_cli = PublicCommandTests.run_cli
    rejected = PublicCommandTests.rejected
    update_json = PublicCommandTests.update_json

    def fixture(self, name, files=None, extra=False):
        directory = self.root / name; directory.mkdir()
        artifact = services_zip(services_files() if files is None else files)
        published, descriptions = [], []
        for profile in ['slack', 'google-calendar']:
            ops = ['read'] + (['write'] if extra and profile == 'google-calendar' else [])
            published.append({'profile': profile, 'operations': [{'operation': op, 'tool': profile + '_' + op, 'effect': 'write' if op == 'write' else 'read'} for op in ops]})
            descriptions.append({'profile': profile, 'configuration_sha256': digest(raw({'profile': profile, 'private_setting': 'never print this'})), 'exit_code': 0,
                'description': {'profile': profile, 'tools': [{'name': profile + '_' + op, 'operation': op, 'effect': 'write' if op == 'write' else 'read', 'description': 'Inert operation', 'inputSchema': {'type': 'object', 'properties': {'flag': {'default': False}}}} for op in ops]}})
        schemas = {'schema_version': 1, 'wheel_sha256': digest(artifact), 'profiles': {'profiles': published}, 'descriptions': descriptions}
        deps = b'httpx==0.28.1\n'
        receipt = {'schema_version': 1, 'scope': 'installed_services_description_capture', 'wheel_sha256': digest(artifact), 'runtime_constraints_sha256': digest(deps), 'schemas_sha256': digest(raw(schemas)), 'profiles': [row['profile'] for row in published], 'status': 'passed'}
        for filename, data in [('artifact', artifact), ('schemas.json', raw(schemas)), ('dependencies', deps), ('receipt.json', raw(receipt))]:
            (directory / filename).write_bytes(data)
        return self.reseal(name)

    def reseal(self, name, capture=True):
        directory = self.root / name
        if capture:
            receipt = json.loads((directory / 'receipt.json').read_bytes())
            receipt['schemas_sha256'] = digest((directory / 'schemas.json').read_bytes())
            (directory / 'receipt.json').write_bytes(raw(receipt))
        members = {p.name: digest(p.read_bytes()) for p in directory.iterdir() if p.name not in ['manifest.json', 'selection.json']}
        (directory / 'manifest.json').write_bytes(raw({'files': members}))
        spec = {'schema_version': 1, 'family': 'services-wheel'}
        for role, name in [('artifact', 'artifact'), ('receipt', 'receipt.json'), ('schemas', 'schemas.json'), ('runtime_dependencies', 'dependencies'), ('evidence_manifest', 'manifest.json')]:
            spec[role] = self.ref(directory / name)
        (directory / 'selection.json').write_bytes(raw(spec)); return self.ref(directory / 'selection.json')

    def test_services_comparison_is_inert_complete_and_deterministic(self):
        before = self.fixture('before'); after = self.fixture('after', extra=True)
        tree = {str(p): digest(p.read_bytes()) for p in self.root.rglob('*') if p.is_file()}
        first = self.run_cli(before, after); second = self.run_cli(before, after)
        self.assertEqual(first.returncode, 0, first.stderr); self.assertEqual(first.stdout, second.stdout)
        report = json.loads(first.stdout)
        self.assertEqual(report['changes']['operations']['added'], ['google-calendar/google-calendar_write'])
        self.assertEqual(report['changes']['operations']['unchanged_count'], 2)
        self.assertEqual(report['result'], 'comparison_only_maintainer_review_required')
        self.assertGreater(report['changes']['retained_source_claims']['unchanged_count'], 0)
        self.assertIn('pyproject.toml', report['before']['capture_scope']['unverified_source_files'])
        markdown = self.run_cli(before, after, fmt='markdown')
        self.assertEqual(markdown.returncode, 0, markdown.stderr)
        self.assertIn('capture', markdown.stdout)
        self.assertNotIn('never print this', first.stdout + markdown.stdout)
        self.assertEqual(tree, {str(p): digest(p.read_bytes()) for p in self.root.rglob('*') if p.is_file()})

    def test_missing_duplicate_or_failed_capture_refuses(self):
        before = self.fixture('before')
        mutations = [lambda s: s['descriptions'].pop(), lambda s: s['descriptions'].append(s['descriptions'][0]), lambda s: s['descriptions'][0].update(exit_code=1), lambda s: s['descriptions'][0].update(exit_code=False), lambda s: s['descriptions'][0]['description']['tools'].pop(), lambda s: s['descriptions'][0]['description']['tools'].append(s['descriptions'][0]['description']['tools'][0]), lambda s: s['profiles']['profiles'].append(s['profiles']['profiles'][0]), lambda s: s['profiles']['profiles'][0]['operations'].append(s['profiles']['profiles'][0]['operations'][0])]
        for index, change in enumerate(mutations):
            with self.subTest(case=index):
                name = 'bad' + str(index); self.fixture(name)
                self.update_json(self.root / name / 'schemas.json', change)
                self.rejected(before, self.reseal(name))

    def test_wrong_artifact_or_schema_receipt_is_not_repaired_by_outer_pins(self):
        before = self.fixture('before')
        for index, (filename, field) in enumerate([('receipt.json', 'wheel_sha256'), ('schemas.json', 'wheel_sha256'), ('receipt.json', 'schemas_sha256'), ('receipt.json', 'runtime_constraints_sha256')]):
            with self.subTest(field=field, filename=filename):
                name='bad'+str(index); self.fixture(name)
                self.update_json(self.root/name/filename, lambda s: s.update({field: '0'*64}))
                self.rejected(before, self.reseal(name, capture=field!='schemas_sha256'))

    def test_capture_schema_and_effect_changes_remain_visible(self):
        before=self.fixture('before'); self.fixture('after')
        def change(s):
            s['profiles']['profiles'][0]['operations'][0]['effect']='write'
            s['descriptions'][0]['description']['tools'][0]['effect']='write'
            s['descriptions'][0]['description']['tools'][0]['inputSchema']['properties']['flag']['default']=0
        self.update_json(self.root/'after/schemas.json',change)
        result=self.run_cli(before,self.reseal('after')); self.assertEqual(result.returncode,0,result.stderr)
        fields=json.loads(result.stdout)['changes']['operations']['changed']['slack/slack_read']['changed_fields']
        self.assertEqual(fields,['/effect','/inputSchema/properties/flag/default'])

    def test_capture_effect_or_operation_mismatch_refuses(self):
        before=self.fixture('before')
        for index,(field,value) in enumerate([('effect','write'),('name','another'),('operation','another'),('inputSchema',[]),('description',5)]):
            name='bad'+str(index);self.fixture(name)
            self.update_json(self.root/name/'schemas.json',lambda s:s['descriptions'][0]['description']['tools'][0].update({field:value}))
            self.rejected(before,self.reseal(name))

    def test_services_record_source_guides_and_notices_are_checked(self):
        before=self.fixture('before')
        ns='meerkat_toolkit_services/'
        mutations=[lambda f:f.update({ns+'__init__.py':b'changed source'}), lambda f:f.update({ns+'HOST.md':b'changed guide'}),lambda f:f.pop('meerkat_toolkit_services-0.1.0.dist-info/licenses/LICENSE-MIT'),lambda f:f.update({ns+'unmapped.py':b'print("unmapped")'}),lambda f:f.update({ns+'unmapped.md':b'unmapped docs'})]
        for index,change in enumerate(mutations):
            with self.subTest(case=index):
                files=services_files();change(files);after=self.fixture('bad'+str(index),files=files);self.rejected(before,after)

    def test_services_metadata_and_source_layout_refuse(self):
        before=self.fixture('before');ns='meerkat_toolkit_services/'
        def alter_source(f, fn):
            s=json.loads(f[ns+'PACKAGE-SOURCES.json']);fn(s);f[ns+'PACKAGE-SOURCES.json']=raw(s)
        changes=[lambda f:alter_source(f,lambda s:s.update(namespace='foreign')),lambda f:alter_source(f,lambda s:s.update(distribution='foreign')),lambda f:alter_source(f,lambda s:s['guide_mapping']["src/"+ns+'HOST.md'].update(sha256='0'*64)),lambda f:alter_source(f,lambda s:s['source_mapping'].clear()),lambda f:alter_source(f,lambda s:s['other_payload_sha256'].update({'README.md':'0'*64})),lambda f:alter_source(f,lambda s:s['other_payload_sha256'].update({'unknown.txt':digest(b'absent')}))]
        for index,change in enumerate(changes):
            files=services_files();change(files);self.rejected(before,self.fixture('bad'+str(index),files=files))

    def test_services_absent_source_claim_change_visible_but_not_read(self):
        before=self.fixture('before');f=services_files();s=json.loads(f['meerkat_toolkit_services/PACKAGE-SOURCES.json'])
        s['other_payload_sha256']['pyproject.toml']=digest(b'changed unshipped build metadata');f['meerkat_toolkit_services/PACKAGE-SOURCES.json']=raw(s)
        result=self.run_cli(before,self.fixture('after',files=f));self.assertEqual(result.returncode,0,result.stderr)
        report=json.loads(result.stdout);self.assertTrue(report['same_display_version_different_artifact'])
        self.assertIn('pyproject.toml',report['changes']['retained_source_claims']['changed'])
        self.assertEqual(report['changes']['package_requirements']['changed'],{})

    def test_services_invalid_receipt_and_configuration_hash_refuse(self):
        before=self.fixture('before')
        cases=[('receipt.json',lambda r:r.update(scope='approved')),('receipt.json',lambda r:r.update(status='failed')),('receipt.json',lambda r:r.update(profiles=['slack'])),('receipt.json',lambda r:r.update(schema_version=True)),('schemas.json',lambda r:r.update(schema_version=True)),('schemas.json',lambda r:r['descriptions'][0].update(configuration_sha256='not-a-digest'))]
        for index,(filename,change) in enumerate(cases):
            name='bad'+str(index);self.fixture(name);self.update_json(self.root/name/filename,change);self.rejected(before,self.reseal(name))

    def test_services_source_path_is_not_followed_or_echoed(self):
        before=self.fixture('before');f=services_files();s=json.loads(f['meerkat_toolkit_services/PACKAGE-SOURCES.json'])
        s['source_mapping']['src/meerkat_toolkit_services/__init__.py']['canonical_source']='../../private-marker'
        f['meerkat_toolkit_services/PACKAGE-SOURCES.json']=raw(s)
        result=self.rejected(before,self.fixture('bad',files=f));self.assertNotIn('private-marker',result.stderr)

    def test_services_unbound_capture_and_cross_family_refuse(self):
        before=self.fixture('before');after=self.fixture('after')
        self.update_json(self.root/'after/manifest.json',lambda m:m['files'].pop('schemas.json'))
        spec=json.loads((self.root/'after/selection.json').read_bytes());spec['evidence_manifest']=self.ref(self.root/'after/manifest.json');(self.root/'after/selection.json').write_bytes(raw(spec))
        self.rejected(before,self.ref(self.root/'after/selection.json'),'input-not-bound-by-evidence-manifest')
        # A real different family is still not a valid replacement target.
        legacy_owner=PublicCommandTests(); legacy_owner.root=self.root
        legacy=legacy_owner.fixture('legacy')
        self.rejected(before,legacy,'incomparable-package-identities')

    def test_services_declared_replacement_is_only_a_proposal(self):
        before=self.fixture('before');after=self.fixture('after',extra=True)
        proposal={'schema_version':1,'action':'replace','from':{'artifact_sha256':digest((self.root/'before/artifact').read_bytes()),'profile':'slack'},'replacement':{'artifact_sha256':digest((self.root/'after/artifact').read_bytes()),'profile':'google-calendar'},'reason':'Explicit reason','migration_guidance':'Review before activation'}
        (self.root/'proposal.json').write_bytes(raw(proposal))
        result=self.run_cli(before,after,self.ref(self.root/'proposal.json'));self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(json.loads(result.stdout)['proposal']['disposition'],'proposal_only_no_support_change_or_retargeting')

    def replace_artifact(self, name, artifact):
        (self.root/name/'artifact').write_bytes(artifact)
        for filename in ['schemas.json','receipt.json']:
            self.update_json(self.root/name/filename, lambda value: value.update(wheel_sha256=digest(artifact)))
        return self.reseal(name)

    def test_services_record_tampering_is_refused_after_capture_rebinding(self):
        before=self.fixture('before');self.fixture('after')
        archive=services_zip(services_files(),lambda f:f.update({'meerkat_toolkit_services/__init__.py':b'tampered'}))
        self.rejected(before,self.replace_artifact('after',archive),'wheel-record-mismatch')

    def test_services_zip_links_traversal_and_foreign_payload_refuse(self):
        before=self.fixture('before')
        for index,kind in enumerate(['symlink','traversal','foreign']):
            name='bad'+str(index);self.fixture(name)
            if kind=='foreign':
                files=services_files();files['other.pth']=b'import dangerous\n';archive=services_zip(files)
            else:
                output=io.BytesIO(services_zip(services_files()))
                with zipfile.ZipFile(output,'a') as z:
                    info=zipfile.ZipInfo('../outside' if kind=='traversal' else 'link')
                    if kind=='symlink': info.external_attr=(0o120777 << 16)
                    z.writestr(info,b'ignored')
                archive=output.getvalue()
            self.rejected(before,self.replace_artifact(name,archive))

    def test_services_pinned_dependency_change_is_reported(self):
        before=self.fixture('before');self.fixture('after')
        deps=b'httpx==0.28.2\n';(self.root/'after/dependencies').write_bytes(deps)
        self.update_json(self.root/'after/receipt.json',lambda r:r.update(runtime_constraints_sha256=digest(deps)))
        result=self.run_cli(before,self.reseal('after'));self.assertEqual(result.returncode,0,result.stderr)
        self.assertEqual(list(json.loads(result.stdout)['changes']['runtime_dependencies']['changed']),['httpx'])

    def test_services_duplicate_profiles_and_normalized_dependencies_refuse(self):
        before=self.fixture('before');self.fixture('profiles')
        self.update_json(self.root/'profiles/receipt.json',lambda r:r.update(profiles=['slack','slack']))
        self.rejected(before,self.reseal('profiles'),'profile-receipt-mismatch')
        self.fixture('deps');deps=b'httpx==0.28.1\nHTTPX==0.28.1\n';(self.root/'deps/dependencies').write_bytes(deps)
        self.update_json(self.root/'deps/receipt.json',lambda r:r.update(runtime_constraints_sha256=digest(deps)))
        self.rejected(before,self.reseal('deps'),'duplicate-python-pin')

    def test_services_versioned_notice_paths_use_stable_content_keys(self):
        before=self.fixture('before')
        for changed_notice in [False,True]:
            files={name.replace('0.1.0.dist-info','0.2.0.dist-info'):value for name,value in services_files().items()}
            meta='meerkat_toolkit_services-0.2.0.dist-info/METADATA'
            files[meta]=files[meta].replace(b'Version: 0.1.0',b'Version: 0.2.0')
            if changed_notice:
                notice='meerkat_toolkit_services-0.2.0.dist-info/licenses/THIRD_PARTY_NOTICES.md'
                files[notice]=b'Changed notice text\n'
                manifest='meerkat_toolkit_services/PACKAGE-SOURCES.json'
                source=json.loads(files[manifest]);source['other_payload_sha256']['THIRD_PARTY_NOTICES.md']=digest(files[notice])
                files[manifest]=raw(source)
            result=self.run_cli(before,self.fixture('after'+str(changed_notice),files=files))
            self.assertEqual(result.returncode,0,result.stderr)
            report=json.loads(result.stdout);notices=report['changes']['notices']
            self.assertEqual(notices['added'],[]);self.assertEqual(notices['removed'],[])
            self.assertEqual(list(notices['changed']),['THIRD_PARTY_NOTICES.md'] if changed_notice else [])
            self.assertEqual(notices['unchanged_count'],2 if changed_notice else 3)
            self.assertIn(meta,report['changes']['payloads']['added'])
        files=services_files()
        files['meerkat_toolkit_services-0.1.0.dist-info/licenses/nested/LICENSE-MIT']=b'An added dependency license\n'
        result=self.run_cli(before,self.fixture('nested',files=files))
        self.assertEqual(result.returncode,0,result.stderr)
        notices=json.loads(result.stdout)['changes']['notices']
        self.assertEqual(notices['added'],['nested/LICENSE-MIT'])
        self.assertEqual(notices['changed'],{});self.assertEqual(notices['unchanged_count'],3)


if __name__ == '__main__':
    unittest.main(verbosity=2)
