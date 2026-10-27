"""Offline controller authority tests; no SSH, providers or live services.

The fake service helper only writes fixture files. Actual child fixtures are
harmless; the Linux-only case checks sealed requirements descriptors.
"""
import base64
import hashlib
import io
import json
import os
from pathlib import Path
import signal
import subprocess
import sys
import tarfile
import time
import uuid

import pytest

from launchkit import remote_destination as controller
from launchkit.remote_deployment import controller_program
from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_files
from launchkit.templates_v2.console_configuration import CONSOLE_FIELDS


FAKE_SERVICE = '''
class Operations:
    def __init__(self, root=ROOT, home=None):
        self.root, self.home = Path(root), Path(home)
        self.spec = definition(self.root)
        self.path = self.root / '.mobkit/remote-service.json'
    def load(self, *, require_source=True, allow_legacy_directory=False):
        value = json.loads(ordinary(self.path, private=True))
        if value['slug'] != self.spec['slug']:
            raise DestinationError('fixture ownership mismatch')
        if require_source and value['source_digest'] != source_digest(self.root):
            raise DestinationError('fixture source mismatch')
        return value
    def status(self):
        if not self.path.exists():
            return {'status':'not-installed','ready':False}
        value = self.load(require_source=False)
        try: current = source_digest(self.root) == value['source_digest']
        except Exception: current = False
        return {'status':value['status'],'ready':value['status']=='running' and current,
                'source_current':current,'definition_verified':True,
                'service':'mobkit-launch.11111111-1111-4111-8111-111111111111.service'}
    def start(self, *, restart=False):
        write_private(self.path, {'slug':self.spec['slug'],'status':'running',
                                 'source_digest':source_digest(self.root)})
        return self.status()
    def inspect(self, receipt):
        return {'ActiveState':receipt['status'],'MainPID':str(receipt.get('main_pid',0)),
                'ControlPID':str(receipt.get('control_pid',0)),'ControlGroup':receipt.get('control_group','')}
    def stop(self, *, remove=False):
        value = self.load(require_source=False)
        value['status']='inactive'
        write_private(self.path,value)
        return self.status()
'''


def bundle(files):
    manifest = ''.join(hashlib.sha256(content).hexdigest() + '  ' + name + '\n'
                       for name, content in sorted(files.items())).encode()
    output = io.BytesIO()
    with tarfile.open(fileobj=output, mode='w:gz') as archive:
        for name, content in {**files, 'SHA256SUMS': manifest}.items():
            entry = tarfile.TarInfo(name)
            entry.size = len(content)
            entry.mode = 0o600
            archive.addfile(entry, io.BytesIO(content))
    digest = hashlib.sha256()
    for name, content in sorted(files.items()):
        digest.update(name.encode() + b'\0' + hashlib.sha256(content).digest())
    return base64.b64encode(output.getvalue()).decode(), digest.hexdigest()


@pytest.fixture
def destination(tmp_path, monkeypatch):
    base = tmp_path.resolve()
    home = base / 'remote-home'
    home.mkdir(mode=0o700)
    root = home / 'src/example'
    monkeypatch.setattr(controller, '_home', lambda: home)
    monkeypatch.setattr(controller.platform, 'system', lambda: 'Linux')
    monkeypatch.setattr(controller.platform, 'machine', lambda: 'x86_64')
    monkeypatch.setattr(controller.platform, 'libc_ver', lambda: ('glibc', '2.31'))
    monkeypatch.setattr(controller, '_probe', lambda arguments, host: True)
    blueprint = starter_blueprint()
    blueprint.target = 'linux'
    files = {name: content.encode() for name, content in generate_files(blueprint).items()
             if name != 'SHA256SUMS' and not controller._excluded(name)}
    files['remote_operations.py'] += FAKE_SERVICE.encode()
    guide, build, identity, operation = (str(uuid.uuid4()) for _ in range(4))
    files['implementation-report.json'] = json.dumps({'build_id': build, 'guide_version': 3,
        'verification': {'digest': 'a' * 64}}).encode()
    archive, transferred = bundle(files)
    binding = {'guide_id': guide, 'guide_version': 3, 'build_id': build,
               'accepted_source_digest': 'a' * 64, 'package_sha256': 'b' * 64,
               'transferred_source_digest': transferred,
               'controller_sha256': hashlib.sha256(Path(controller.__file__).read_bytes()).hexdigest()}
    root, host = controller._host(str(root))
    request = {'action': 'install', 'deployment_id': identity, 'operation_id': operation,
               'binding': binding, 'destination': str(root), 'archive': archive,
               'settings': {'OPENAI_API_KEY': 'private-canary'},
               'expected_host': {key: host[key] for key in ('uid', 'home', 'destination')}}
    live = {'value': True}
    monkeypatch.setattr(controller, '_spawn', lambda *args: {'pid': 123456, 'start_ticks': '1',
                                                           'uid': os.getuid(), 'boot_id': 'fixture'})
    monkeypatch.setattr(controller, '_live', lambda evidence: bool(evidence and live['value']))
    return root, request, files, live


def install(destination):
    root, request, files, live = destination
    result = controller.handle(request)
    assert 'error' not in result, result
    assert result['admitted'] and result['status'] == 'installing'
    return root, request, live


def test_read_only_preflight_and_exact_host_binding(destination):
    root, request, files, live = destination
    result = controller.handle({**request, 'action': 'preflight'})
    assert result['ready'] and result['status'] == 'preflight'
    assert result['host']['platform'] == 'linux'
    assert not root.exists()
    assert not (root.parents[1] / '.local').exists()
    wrong = {**request, 'expected_host': {**request['expected_host'], 'uid': os.getuid() + 1}}
    result = controller.handle(wrong)
    assert result['error']['code'] == 'host-changed' and not root.exists()


def test_selected_product_archive_refuses_before_destination_lock_or_effects(destination, monkeypatch):
    root, request, files, live = destination
    definition = json.loads(files['deployment.json'])
    definition['runtimeCandidate'] = {'source': 'meta-toolkit-product',
        'selection': 'runtime-candidate.json', 'assembly_sha256': 'c' * 64}
    files['deployment.json'] = json.dumps(definition).encode()
    request['archive'], request['binding']['transferred_source_digest'] = bundle(files)
    archive_before = request['archive']
    # The accepted Linux marker is inert. No native/SDK or remote owner is provided.
    def forbidden(*args, **kwargs):
        pytest.fail('Unsupported product source must refuse before destination effects.')
    for name in ('_lock', '_fields', '_module', '_save_configuration', '_spawn'):
        monkeypatch.setattr(controller, name, forbidden)
    with pytest.raises(controller.ControllerError) as refused:
        controller._archive(request['archive'], request['binding'])
    assert refused.value.code == 'runtime-unsupported'
    result = controller.handle(request)
    assert result['error'] == {'code': 'runtime-unsupported', 'message': controller.PRODUCT_RUNTIME_UNSUPPORTED}
    assert result.get('admitted') is not True and result.get('worker_pid') is None
    assert request['archive'] == archive_before and not root.exists()
    assert not (root.parents[1] / '.local').exists()
    assert 'private-canary' not in json.dumps(result)


@pytest.mark.parametrize('marker', [None, {'source': 'signed-candidate',
    'selection': 'runtime-candidate.json', 'tuple_sha256': 'd' * 64}], ids=['ordinary', 'legacy-admission-only'])
def test_product_archive_refusal_preserves_existing_ordinary_and_legacy_source_admission(destination, marker):
    root, request, files, live = destination
    definition = json.loads(files['deployment.json'])
    if marker is not None:
        definition['runtimeCandidate'] = marker
    files['deployment.json'] = json.dumps(definition).encode()
    request['archive'], request['binding']['transferred_source_digest'] = bundle(files)
    data, selected, admitted = controller._archive(request['archive'], request['binding'])
    assert data == base64.b64decode(request['archive']) and selected == files
    assert admitted['target'] == 'linux' and admitted.get('runtimeCandidate') == marker
    assert not root.exists()


@pytest.mark.parametrize('machine,libc,failed', [
    ('riscv64', ('glibc', '2.36'), 'gateway-architecture'),
    ('x86_64', ('glibc', '2.30'), 'gateway-libc'),
    ('aarch64', ('musl', '1.2.5'), 'gateway-libc'),
    ('x86_64', ('', ''), 'gateway-libc'),
    ('x86_64', ('glibc', 'private-version-canary'), 'gateway-libc'),
])
def test_gateway_compatibility_blocks_before_install_admission_or_private_writes(destination, monkeypatch, machine, libc, failed):
    root, request, files, live = destination
    monkeypatch.setattr(controller.platform, 'machine', lambda: machine)
    monkeypatch.setattr(controller.platform, 'libc_ver', lambda: libc)
    monkeypatch.setattr(controller, '_spawn', lambda *args: pytest.fail('Unsupported hosts must not start installation workers.'))
    monkeypatch.setattr(controller, '_save_configuration', lambda *args: pytest.fail('Unsupported hosts must not write private settings.'))
    checked = controller.handle({**request, 'action': 'preflight'})
    assert checked['ready'] is False and checked['status'] == 'preflight'
    row = next(item for item in checked['checks'] if item['name'] == failed)
    assert row['status'] == 'failed' and 'then recheck' in row['detail']
    assert not root.exists() and not (root.parents[1] / '.local').exists()
    result = controller.handle(request)
    assert result['error']['code'] == 'preflight-failed' and result.get('admitted') is not True
    assert not root.exists()
    assert 'private-canary' not in json.dumps(result) + json.dumps(checked)
    assert 'private-version-canary' not in json.dumps(result) + json.dumps(checked)


def test_gateway_diagnostics_never_copy_platform_probe_values(destination, monkeypatch):
    root, request, files, live = destination
    monkeypatch.setattr(controller.platform, 'machine', lambda: 'private-platform-canary')
    monkeypatch.setattr(controller.platform, 'libc_ver', lambda: ('private-libc-canary', 'private-version-canary'))
    result = controller.handle({**request, 'action': 'preflight'})
    assert result['ready'] is False
    for name in ('gateway-architecture', 'gateway-libc'):
        assert next(item for item in result['checks'] if item['name'] == name)['status'] == 'failed'
    assert 'private-' not in json.dumps(result) and not root.exists()


def test_status_empty_destination_is_explicit_nonadmission(destination):
    root, request, files, live = destination
    result = controller.handle({**request, 'action': 'status'})
    assert result['status'] == 'not-installed' and result['admitted'] is False
    assert result['worker_live'] is False and result['service_owned'] is False
    assert not root.exists()


def test_install_preserves_distinct_bindings_private_values_and_exact_snapshots(destination):
    root, request, live = install(destination)
    owner = controller._json_read(root / '.mobkit/launch-remote/owner.json')
    assert owner['binding'] == request['binding']
    assert owner['binding']['accepted_source_digest'] != owner['binding']['transferred_source_digest']
    assert (root / '.env').stat().st_mode & 0o077 == 0
    assert 'private-canary' in (root / '.env').read_text()
    assert 'private-canary' not in json.dumps(owner)
    for name, digest in owner['snapshots'].items():
        path = root / '.mobkit/launch-remote' / name
        assert hashlib.sha256(path.read_bytes()).hexdigest() == digest
        assert path.stat().st_mode & 0o077 == 0
    before = (root / '.env').read_bytes()
    result = controller.handle({**request, 'settings': {'OPENAI_API_KEY': 'replacement'}})
    assert result['admitted'] and result['operation_id'] == request['operation_id']
    assert (root / '.env').read_bytes() == before


@pytest.mark.parametrize('changed,key,message', [
    ({'TEAM_PASSWORD': 'tiny-marker'}, 'TEAM_PASSWORD', 'Use at least 12 characters.'),
    ({'TEAM_NAME': ' \t '}, 'TEAM_NAME', 'Enter a required value that is not only whitespace.'),
    ({'TEAM_PORT': '65536'}, 'TEAM_PORT', 'Enter a whole number between 1024 and 65535.'),
])
def test_application_constraints_reject_before_destination_writes_or_admission(destination, changed, key, message):
    root, request, files, live = destination
    files['application/configuration.json'] = json.dumps({'fields': [
        {'key': 'TEAM_PASSWORD', 'label': 'Staff password', 'secret': True, 'required': True, 'min_length': 12},
        {'key': 'TEAM_NAME', 'label': 'Staff name', 'secret': False, 'required': True, 'max_length': 80},
        {'key': 'TEAM_PORT', 'label': 'Staff listener', 'secret': False, 'required': False,
         'type': 'integer', 'minimum': 1024, 'maximum': 65535, 'default': '4111'},
    ]}).encode()
    request['archive'], request['binding']['transferred_source_digest'] = bundle(files)
    request['settings'].update({'TEAM_PASSWORD': 'valid-private-password', 'TEAM_NAME': 'Coordinator', **changed})
    result = controller.handle(request)
    assert result['error']['code'] == 'configuration-invalid'
    issue = result['error']['fields'][0]
    assert issue['key'] == key and issue['message'] == message
    assert set(issue) == {'key', 'label', 'message'}
    assert 'private-canary' not in json.dumps(result) and 'tiny-marker' not in json.dumps(result)
    assert not root.exists() and 'admitted' not in result


def test_destination_validation_preserves_secret_text_and_accepts_legacy_helper(destination):
    root, request, files, live = destination
    definition = json.loads(files['deployment.json'])
    files['application/configuration.json'] = json.dumps({'fields': [
        {'key': 'TEAM_PASSWORD', 'label': 'Staff password', 'secret': True, 'required': True,
         'min_length': 12, 'max_length': 80},
    ]}).encode()
    fields, validator = controller._fields(files, definition, with_validator=True)
    exact = ' \tvalid private password\t '
    prepared = controller._configuration(root, {'OPENAI_API_KEY': 'private-canary', 'TEAM_PASSWORD': exact}, fields, validator)
    assert not root.exists()
    payload = prepared[-1].decode().splitlines()
    assert json.loads(next(line.split('=', 1)[1] for line in payload if line.startswith('TEAM_PASSWORD='))) == exact

    files['application_configuration.py'] = b'def validate_application_fields(value, existing_keys=()):\n    return value["fields"]\n'
    files['application/configuration.json'] = json.dumps({'fields': [
        {'key': 'TEAM_PASSWORD', 'label': 'Legacy password', 'secret': True, 'required': True},
    ]}).encode()
    fields, validator = controller._fields(files, definition, with_validator=True)
    assert validator is None
    prepared = controller._configuration(root, {'OPENAI_API_KEY': 'private-canary', 'TEAM_PASSWORD': 'short'}, fields, validator)
    assert not root.exists() and b'"short"' in prepared[-1]


@pytest.mark.parametrize('override', [None, '', '25173'])
def test_retry_configuration_retains_actual_private_console_port_unless_explicitly_replaced(destination, override):
    root, request, files, live = destination
    root.mkdir(mode=0o700, parents=True)
    path = root / '.env'
    before = b'MOBKIT_PORT="24173"\nOPENAI_API_KEY="prior-private-fixture"\n'
    path.write_bytes(before)
    path.chmod(0o600)
    definition = json.loads(files['deployment.json'])
    fields, validator = controller._fields(files, definition, with_validator=True)
    settings = {'OPENAI_API_KEY': 'fresh-private-fixture'}
    if override is not None:
        settings['MOBKIT_PORT'] = override
    prepared = controller._configuration(root, settings, fields, validator)
    values = {key: json.loads(raw) for line in prepared[-1].decode().splitlines() if line and not line.startswith('#')
              for key, raw in [line.split('=', 1)]}
    assert values['MOBKIT_PORT'] == (override or '24173')
    assert values['OPENAI_API_KEY'] == 'fresh-private-fixture'
    assert prepared[1] == before and path.read_bytes() == before, 'Preparation preserves the actual private file before admission.'


def test_invalid_destination_default_is_refused_without_publication(destination):
    root, request, files, live = destination
    files['application/configuration.json'] = json.dumps({'fields': [
        {'key': 'TEAM_NAME', 'label': 'Staff name', 'secret': False, 'required': False,
         'max_length': 3, 'default': 'invalid-default-marker'},
    ]}).encode()
    request['archive'], request['binding']['transferred_source_digest'] = bundle(files)
    result = controller.handle(request)
    assert result['error']['code'] == 'configuration-invalid' and not root.exists()
    assert 'invalid-default-marker' not in json.dumps(result)


def test_destination_validates_retained_private_values_without_overwriting_them(destination):
    root, request, files, live = destination
    files['application/configuration.json'] = json.dumps({'fields': [
        {'key': 'TEAM_PASSWORD', 'label': 'Staff password', 'secret': True, 'required': True, 'min_length': 12},
    ]}).encode()
    fields, validator = controller._fields(files, json.loads(files['deployment.json']), with_validator=True)
    root.mkdir(parents=True, mode=0o700)
    path = root / '.env'
    before = b'OPENAI_API_KEY="private-canary"\nTEAM_PASSWORD="tiny-marker"\n'
    path.write_bytes(before)
    path.chmod(0o600)
    with pytest.raises(controller.ControllerError) as failure:
        controller._configuration(root, {}, fields, validator)
    assert failure.value.code == 'configuration-invalid'
    assert failure.value.fields == [{'key': 'TEAM_PASSWORD', 'label': 'Staff password',
                                    'message': 'Use at least 12 characters.'}]
    assert path.read_bytes() == before and not list(root.glob('.env.*'))


@pytest.mark.parametrize('changed,key', [
    ({'CONSOLE_USER_ID': 'private principal-marker'}, 'CONSOLE_USER_ID'),
    ({'CONSOLE_LOGIN_PASSWORD': 'tiny-marker'}, 'CONSOLE_LOGIN_PASSWORD'),
    ({'CONSOLE_LOGIN_PASSWORD': 'x' * 4097}, 'CONSOLE_LOGIN_PASSWORD'),
    ({'CONSOLE_LOGIN_PASSWORD': 'private-surrogate-marker-\ud800'}, 'CONSOLE_LOGIN_PASSWORD'),
])
def test_console_intrinsic_rules_reject_before_destination_private_or_source_writes(destination, changed, key):
    root, request, files, live = destination
    files['application/console.json'] = b'{"authentication":"local-password"}'
    files['application/configuration.json'] = json.dumps({'fields': list(CONSOLE_FIELDS)}).encode()
    request['archive'], request['binding']['transferred_source_digest'] = bundle(files)
    request['settings'].update({'CONSOLE_USER_ID': 'owner@example.test', 'CONSOLE_LOGIN_PASSWORD': 'valid-password-123', **changed})
    result = controller.handle(request)
    assert result['error']['code'] == 'configuration-invalid'
    assert [field['key'] for field in result['error']['fields']] == [key]
    assert not root.exists()
    assert 'tiny-marker' not in json.dumps(result) and 'principal-marker' not in json.dumps(result)
    assert 'surrogate-marker' not in json.dumps(result)


def test_console_destination_preserves_exact_password_and_validates_legacy_scalar_helper(destination):
    root, request, files, live = destination
    files['application/console.json'] = b'{"authentication":"local-password"}'
    files['application/configuration.json'] = json.dumps({'fields': list(CONSOLE_FIELDS)}).encode()
    # Previously accepted packages need not gain new source or a new scalar API.
    files.pop('console_configuration.py')
    files['application_configuration.py'] = b'def validate_application_fields(value, existing_keys=()):\n    return value["fields"]\n'
    fields, validator = controller._fields(files, json.loads(files['deployment.json']), with_validator=True)
    exact = ' \tvalid private password\t '
    prepared = controller._configuration(root, {'OPENAI_API_KEY': 'private-canary',
        'CONSOLE_USER_ID': 'owner@example.test', 'CONSOLE_LOGIN_PASSWORD': exact}, fields, validator)
    rows = prepared[-1].decode().splitlines()
    assert json.loads(next(row.split('=', 1)[1] for row in rows if row.startswith('CONSOLE_LOGIN_PASSWORD='))) == exact
    assert not root.exists()
    root.mkdir(parents=True, mode=0o700)
    path = root / '.env'
    before = b'OPENAI_API_KEY="private-canary"\nCONSOLE_USER_ID="owner"\nCONSOLE_LOGIN_PASSWORD="tiny-marker"\n'
    path.write_bytes(before)
    path.chmod(0o600)
    with pytest.raises(controller.ControllerError) as failure:
        controller._configuration(root, {}, fields, validator)
    assert failure.value.fields[0]['key'] == 'CONSOLE_LOGIN_PASSWORD'
    assert path.read_bytes() == before and not list(root.glob('.env.*'))
    # Identical field names in a custom app have no intrinsic Console policy.
    files.pop('application/console.json')
    fields, validator = controller._fields(files, json.loads(files['deployment.json']), with_validator=True)
    assert validator is None
    controller._configuration(root, {}, fields, validator)


def add_console_metadata(destination):
    root, request, files, live = destination
    files['application/console.json'] = b'{"authentication":"local-password"}'
    files['application/configuration.json'] = json.dumps({'fields': list(CONSOLE_FIELDS)}).encode()
    request['archive'], request['binding']['transferred_source_digest'] = bundle(files)
    request['settings'].update({'CONSOLE_USER_ID': 'owner@example.test', 'CONSOLE_LOGIN_PASSWORD': 'valid-password-123'})


def test_console_restart_rejects_retained_intrinsic_failure_without_new_operation_or_writes(destination):
    add_console_metadata(destination)
    root, request, live = install(destination)
    completed_service(root, live, status='inactive')
    path = root / '.env'
    before = path.read_bytes().replace(b'"valid-password-123"', b'"tiny-marker"')
    path.write_bytes(before)
    directory = root / '.mobkit/launch-remote'
    journal = (directory / 'operation.json').read_bytes()
    result = controller.handle({**request, 'action': 'restart', 'operation_id': str(uuid.uuid4())})
    assert result['error']['code'] == 'configuration-invalid'
    assert result['error']['fields'][0]['key'] == 'CONSOLE_LOGIN_PASSWORD'
    assert path.read_bytes() == before and (directory / 'operation.json').read_bytes() == journal
    assert 'tiny-marker' not in json.dumps(result)
    # Invalid credentials must never prevent stopping the owned service.
    stopped = controller.handle({**request, 'action': 'stop', 'operation_id': str(uuid.uuid4())})
    assert 'error' not in stopped and stopped['admitted']


def test_console_retry_rejects_invalid_private_replacement_without_changing_prior_custody(destination, monkeypatch):
    add_console_metadata(destination)
    root, request, live = failed_install(destination, monkeypatch)
    directory = root / '.mobkit/launch-remote'
    journal, settings = (directory / 'operation.json').read_bytes(), (root / '.env').read_bytes()
    retry = retry_request(request, settings={'CONSOLE_LOGIN_PASSWORD': 'tiny-marker'})
    result = controller.handle(retry)
    assert result['error']['code'] == 'configuration-invalid'
    assert result['error']['fields'][0]['key'] == 'CONSOLE_LOGIN_PASSWORD'
    assert (directory / 'operation.json').read_bytes() == journal and (root / '.env').read_bytes() == settings
    assert 'tiny-marker' not in json.dumps(result)


def test_missing_worker_remains_custody_and_blocks_new_operation(destination):
    root, request, live = install(destination)
    live['value'] = False
    result = controller.handle({**request, 'action': 'status'})
    assert result['status'] == 'recovery-needed' and result['worker_live'] is False
    assert result['worker_pid'] == 123456 and result['cleanup_confirmed'] is False
    assert result['admitted'] is True
    refused = controller.handle({**request, 'action': 'stop', 'operation_id': str(uuid.uuid4())})
    assert refused['error']['code'] == 'recovery-needed'


@pytest.mark.parametrize('missing', ['operation.json', 'owner.json'])
def test_missing_joining_journal_never_erases_admitted_custody(destination, missing):
    root, request, live = install(destination)
    (root / '.mobkit/launch-remote' / missing).unlink()
    result = controller.handle({**request, 'action': 'status'})
    if missing == 'operation.json':
        assert result['status'] == 'recovery-needed' and result['admitted'] is True
    else:
        assert result['error']['code'] == 'recovery-needed' and result.get('admitted') is not False
    before = (root / '.env').read_bytes()
    refused = controller.handle({**request, 'settings': {'OPENAI_API_KEY': 'replacement'}})
    assert refused['error']['code'] == 'recovery-needed'
    assert (root / '.env').read_bytes() == before


def failed_install(destination, monkeypatch):
    """Offline terminal failure; the process-death seam is tested separately."""
    root, request, live = install(destination)
    path = root / '.mobkit/launch-remote/operation.json'
    job = controller._json_read(path)
    job['worker']['boot_id'] = str(uuid.uuid4())
    job.update(status='failed', phase='attention-required', failure_phase='dependencies',
               cleanup_confirmed=True, error_code='operation-failed')
    controller._json_write(path, job)
    live['value'] = False
    evidence = dict(job['worker'])
    monkeypatch.setattr(controller, '_worker_dead', lambda value: value == evidence and not live['value'])
    return root, request, live


def retry_request(request, **overrides):
    return {key: value for key, value in {**request, 'action': 'retry-install',
            'operation_id': str(uuid.uuid4()), 'previous_operation_id': request['operation_id'],
            'settings': {'OPENAI_API_KEY': 'fresh-private-canary'}, **overrides}.items() if key != 'archive'}


def test_explicit_retry_preserves_history_data_binding_and_replay_cannot_replace_settings(destination, monkeypatch):
    root, request, live = failed_install(destination, monkeypatch)
    directory = root / '.mobkit/launch-remote'
    original = {name: (directory / name).read_bytes() for name in ('owner.json', 'admission.json', 'operation.json')}
    (root / 'data').mkdir()
    (root / 'data/retained').write_text('native data')
    observation = controller.handle({**request, 'action': 'status'})
    assert observation['status'] == 'failed' and observation['retryable'] is True
    assert observation['retry_of_operation_id'] == request['operation_id']
    assert observation['failure_phase'] == 'dependencies'
    calls = []
    def spawn(root, owner, operation_id):
        calls.append(operation_id)
        live['value'] = True
        return {'pid': 123456, 'start_ticks': '2', 'uid': os.getuid(), 'boot_id': str(uuid.uuid4())}
    monkeypatch.setattr(controller, '_spawn', spawn)
    retry = retry_request(request)
    result = controller.handle(retry)
    assert result['admitted'] and result['status'] == 'installing' and result['operation_id'] == retry['operation_id']
    assert result['previous_operation_id'] == request['operation_id'] and not result['retryable']
    assert (directory / 'operations' / (request['operation_id'] + '.json')).read_bytes() == original['operation.json']
    assert (directory / 'owner.json').read_bytes() == original['owner.json']
    assert (directory / 'admission.json').read_bytes() == original['admission.json']
    assert (root / 'data/retained').read_text() == 'native data'
    saved = (root / '.env').read_bytes()
    assert b'fresh-private-canary' in saved
    replay = controller.handle({**retry, 'settings': {'OPENAI_API_KEY': 'replay-must-not-save'}})
    assert replay['operation_id'] == retry['operation_id'] and replay['admitted']
    assert calls == [retry['operation_id']] and (root / '.env').read_bytes() == saved
    wrong = controller.handle({**retry, 'previous_operation_id': str(uuid.uuid4())})
    assert wrong['error']['code'] == 'operation-conflict' and calls == [retry['operation_id']]
    assert 'private-canary' not in json.dumps(result) + json.dumps(replay)


def test_gateway_failure_blocks_new_retry_but_preserves_existing_replay_and_stop_custody(destination, monkeypatch):
    root, request, live = failed_install(destination, monkeypatch)
    directory = root / '.mobkit/launch-remote'
    owner = controller._json_read(directory / 'owner.json')
    module, operations = controller._runtime(root, owner)
    operations.start()
    operations.stop()
    before = {path.relative_to(root): path.read_bytes() for path in root.rglob('*') if path.is_file()}
    monkeypatch.setattr(controller.platform, 'libc_ver', lambda: ('musl', '1.2.5'))
    calls = []
    monkeypatch.setattr(controller, '_spawn', lambda root, owner, operation_id:
                        calls.append(operation_id) or {'pid': 123456, 'start_ticks': '2',
                                                       'uid': os.getuid(), 'boot_id': str(uuid.uuid4())})
    refused = controller.handle(retry_request(request))
    assert refused['error']['code'] == 'preflight-failed' and refused.get('admitted') is not True
    assert {path.relative_to(root): path.read_bytes() for path in root.rglob('*') if path.is_file()} == before
    assert calls == [] and not (directory / 'operations').exists()
    replay = controller.handle(request)
    assert replay['admitted'] and replay['operation_id'] == request['operation_id']
    assert {path.relative_to(root): path.read_bytes() for path in root.rglob('*') if path.is_file()} == before
    stopped = controller.handle({**request, 'action': 'stop', 'operation_id': str(uuid.uuid4())})
    assert stopped['admitted'] and controller._job(root)['action'] == 'stop'
    assert calls == [stopped['operation_id']]
    assert (root / '.env').read_bytes() == before[Path('.env')]
    assert (directory / 'owner.json').read_bytes() == before[Path('.mobkit/launch-remote/owner.json')]


@pytest.mark.parametrize('problem,code', [
    ('live-worker', 'busy'), ('cleanup', 'recovery-needed'), ('command-cleanup', 'recovery-needed'),
    ('missing-evidence', 'recovery-needed'), ('missing-admission', 'recovery-needed'),
    ('missing-job', 'recovery-needed'), ('source', 'source-changed'), ('snapshot', 'snapshot-changed'),
    ('active-service', 'retry-unavailable'), ('unknown-manager', 'recovery-needed'),
    ('predecessor', 'operation-conflict'), ('settings', 'configuration-invalid'),
    ('missing-settings', 'configuration-invalid'), ('preflight', 'preflight-failed'),
])
def test_retry_refusal_preserves_private_inputs_and_original_journal(destination, monkeypatch, problem, code):
    root, request, live = failed_install(destination, monkeypatch)
    directory = root / '.mobkit/launch-remote'
    retry = retry_request(request)
    job = controller._job(root)
    if problem == 'live-worker':
        live['value'] = True
    elif problem == 'cleanup':
        job['cleanup_confirmed'] = False
    elif problem == 'command-cleanup':
        job.update(command=job['worker'], command_cleaned=False)
    elif problem == 'missing-evidence':
        job['worker'] = {}
    elif problem == 'missing-admission':
        (directory / 'admission.json').unlink()
    elif problem == 'source':
        (root / 'app.py').write_text('changed source')
    elif problem == 'snapshot':
        (directory / 'runtime-operations.py').write_text('changed snapshot')
    elif problem in {'active-service', 'unknown-manager'}:
        owner = controller._json_read(directory / 'owner.json')
        module, operations = controller._runtime(root, owner)
        operations.start()
        if problem == 'unknown-manager':
            value = controller._json_read(operations.path)
            value['status'] = 'private-canary-unrecognized'
            controller._json_write(operations.path, value)
    elif problem == 'predecessor':
        retry['previous_operation_id'] = str(uuid.uuid4())
    elif problem == 'settings':
        retry['settings']['UNDECLARED_KEY'] = 'private-canary'
    elif problem == 'missing-settings':
        del retry['settings']
    elif problem == 'preflight':
        monkeypatch.setattr(controller, '_probe', lambda arguments, host: False)
    controller._json_write(directory / 'operation.json', job)
    if problem == 'missing-job':
        (directory / 'operation.json').unlink()
    before = (root / '.env').read_bytes()
    prior = (directory / 'operation.json').read_bytes() if (directory / 'operation.json').exists() else None
    monkeypatch.setattr(controller, '_spawn', lambda *args: pytest.fail('Refused retry must not spawn.'))
    result = controller.handle(retry)
    assert result['error']['code'] == code and result.get('admitted') is not True
    assert (root / '.env').read_bytes() == before
    assert ((directory / 'operation.json').read_bytes() if (directory / 'operation.json').exists() else None) == prior
    assert not (directory / 'operations').exists()
    assert 'private-canary' not in json.dumps(result)


def test_retry_configuration_or_spawn_interruption_retains_new_custody(destination, monkeypatch):
    root, request, live = failed_install(destination, monkeypatch)
    retry = retry_request(request)
    before = (root / '.env').read_bytes()
    monkeypatch.setattr(controller, '_save_configuration', lambda *args: (_ for _ in ()).throw(OSError('private-canary')))
    monkeypatch.setattr(controller, '_spawn', lambda *args: pytest.fail('Configuration interruption must not spawn.'))
    result = controller.handle(retry)
    assert result['error']['code'] == 'recovery-needed' and 'private-canary' not in json.dumps(result)
    assert (root / '.env').read_bytes() == before
    observation = controller.handle({**request, 'action': 'status'})
    assert observation['admitted'] and observation['status'] == 'recovery-needed'
    assert observation['operation_id'] == retry['operation_id'] and not observation['retryable']
    replay = controller.handle(retry)
    assert replay['status'] == 'recovery-needed' and replay['operation_id'] == retry['operation_id']


@pytest.mark.parametrize('custody', ['none', 'main-pid', 'control-pid', 'control-group'])
def test_retry_owned_inactive_service_requires_exact_quiescent_observation(destination, monkeypatch, custody):
    root, request, live = failed_install(destination, monkeypatch)
    owner = controller._json_read(root / '.mobkit/launch-remote/owner.json')
    module, operations = controller._runtime(root, owner)
    operations.start()
    receipt = controller._json_read(operations.path)
    receipt['status'] = 'inactive'
    if custody != 'none':
        receipt[{'main-pid': 'main_pid', 'control-pid': 'control_pid', 'control-group': 'control_group'}[custody]] = (
            '/owned-group' if custody == 'control-group' else 1234)
    controller._json_write(operations.path, receipt)
    observation = controller.handle({**request, 'action': 'status'})
    assert observation['service_owned'] and observation['service_observed']
    assert observation['service']['quiescent'] is (custody == 'none')
    assert observation['retryable'] is (custody == 'none')
    if custody != 'none':
        before = (root / '.env').read_bytes()
        result = controller.handle(retry_request(request))
        assert result['error']['code'] == 'retry-unavailable' and (root / '.env').read_bytes() == before


@pytest.mark.parametrize('custody', ['health', 'service-phase'])
def test_absent_receipt_retry_requires_pre_service_failure_without_health_custody(destination, monkeypatch, custody):
    root, request, live = failed_install(destination, monkeypatch)
    if custody == 'health':
        controller._json_write(root / '.mobkit/remote-service-health.json', {'retained': True})
    else:
        path = root / '.mobkit/launch-remote/operation.json'
        job = controller._json_read(path)
        job['failure_phase'] = 'service-install'
        controller._json_write(path, job)
    observation = controller.handle({**request, 'action': 'status'})
    assert observation['retryable'] is False
    result = controller.handle(retry_request(request))
    assert result['error']['code'] == 'retry-unavailable'


def test_refused_retry_uuid_can_be_reused_after_settings_correction(destination, monkeypatch):
    root, request, live = failed_install(destination, monkeypatch)
    retry = retry_request(request, settings={'UNDECLARED': 'private-canary'})
    result = controller.handle(retry)
    assert result['error']['code'] == 'configuration-invalid'
    observation = controller.handle({**request, 'action': 'status'})
    assert observation['retryable'] and observation['operation_id'] == request['operation_id']
    def spawn(*args):
        live['value'] = True
        return {'pid': 123456, 'start_ticks': '2', 'uid': os.getuid(), 'boot_id': str(uuid.uuid4())}
    monkeypatch.setattr(controller, '_spawn', spawn)
    result = controller.handle({**retry, 'settings': {'OPENAI_API_KEY': 'corrected-private-canary'}})
    assert result['operation_id'] == retry['operation_id'] and result['admitted']
    assert b'corrected-private-canary' in (root / '.env').read_bytes()


def test_archived_retry_uuid_cannot_be_reused_to_repeat_installation(destination, monkeypatch):
    root, request, live = failed_install(destination, monkeypatch)
    monkeypatch.setattr(controller, '_worker_dead', lambda evidence: True)
    def spawn(*args):
        live['value'] = True
        return {'pid': 123456, 'start_ticks': '2', 'uid': os.getuid(), 'boot_id': str(uuid.uuid4())}
    monkeypatch.setattr(controller, '_spawn', spawn)
    first = retry_request(request)
    assert controller.handle(first)['admitted']
    path = root / '.mobkit/launch-remote/operation.json'
    job = controller._json_read(path)
    job.update(status='failed', phase='attention-required', failure_phase='dependencies', cleanup_confirmed=True)
    controller._json_write(path, job)
    live['value'] = False
    second = retry_request(request, previous_operation_id=first['operation_id'])
    assert controller.handle(second)['admitted']
    before = (root / '.env').read_bytes()
    result = controller.handle({**first, 'settings': {'OPENAI_API_KEY': 'must-not-apply'}})
    assert result['error']['code'] == 'operation-conflict'
    assert (root / '.env').read_bytes() == before and controller._job(root)['operation_id'] == second['operation_id']


def test_actual_harmless_failed_phase_then_retry_reaches_fake_owned_service(destination, monkeypatch):
    """Real package-check child; worker death and systemd remain offline seams."""
    root, request, files, live = destination
    def spawn(*args):
        live['value'] = True
        return {'pid': os.getpid(), 'start_ticks': '1', 'uid': os.getuid(), 'boot_id': str(uuid.uuid4())}
    monkeypatch.setattr(controller, '_spawn', spawn)
    root, request, live = install(destination)
    fixture_python(root)
    (root / 'data').mkdir()
    (root / 'data/retained').write_text('preserved across retry')
    def failure(root, owner, operation_id, module):
        controller._progress(root, owner, operation_id, 'dependencies')
        controller._command([sys.executable, '-I', '-c', 'import sys; sys.exit(23)'],
                            root, owner, operation_id, 'dependencies', timeout=10)
    monkeypatch.setattr(controller, '_install_dependencies', failure)
    controller.worker(str(root), request['operation_id'])
    job = controller._job(root)
    assert job['status'] == 'failed' and job['cleanup_confirmed'] is True
    assert job['failure_phase'] == 'dependencies'
    live['value'] = False
    monkeypatch.setattr(controller, '_worker_dead', lambda evidence: not live['value'])
    observation = controller.handle({**request, 'action': 'status'})
    assert observation['retryable'] and observation['retry_of_operation_id'] == request['operation_id']
    retry = retry_request(request)
    assert controller.handle(retry)['operation_id'] == retry['operation_id']
    observed = run_capsule(monkeypatch)
    def accepted_check(root, owner, operation_id, module):
        controller._accepted_python(root, owner, operation_id, controller._accepted_files(root, owner),
                                    'app.py', ['--check'], 'application-source')
    monkeypatch.setattr(controller, '_install_dependencies', accepted_check)
    controller.worker(str(root), retry['operation_id'])
    live['value'] = False
    result = controller.handle({**request, 'action': 'status'})
    assert result['status'] == 'running' and result['ready'] and result['service_owned']
    assert result['operation_id'] == retry['operation_id'] and result['previous_operation_id'] == request['operation_id']
    assert (root / 'data/retained').read_text() == 'preserved across retry'
    assert b'Valid deployment:' in observed[0][1].stdout
    assert 'private-canary' not in json.dumps(result)
    assert b'private-canary' not in observed[0][1].stdout + observed[0][1].stderr


@pytest.mark.parametrize('state,dead', [
    ('absent', True), ('reused', True), ('zombie', True), ('live', False), ('unreadable', False),
    ('malformed', False), ('missing-proc', False), ('new-boot', True),
])
def test_worker_death_requires_readable_birth_evidence(tmp_path, monkeypatch, state, dead):
    processes = tmp_path / 'proc'
    (processes / 'sys/kernel/random').mkdir(parents=True)
    (processes / 'self').mkdir()
    boot = str(uuid.uuid4())
    (processes / 'sys/kernel/random/boot_id').write_text(boot)
    (processes / 'self/stat').write_text('readable process view')
    evidence = {'pid': 4321, 'start_ticks': '123', 'uid': os.getuid(), 'boot_id': boot}
    current = {**evidence, 'state': 'S'}
    if state == 'reused':
        current['start_ticks'] = '456'
    elif state == 'zombie':
        current['state'] = 'Z'
    elif state in {'absent', 'unreadable'}:
        current = None
        if state == 'unreadable':
            (processes / str(evidence['pid'])).mkdir()
    elif state == 'malformed':
        evidence['boot_id'] = None
    elif state == 'missing-proc':
        (processes / 'self/stat').unlink()
    elif state == 'new-boot':
        (processes / 'sys/kernel/random/boot_id').write_text(str(uuid.uuid4()))
    monkeypatch.setattr(controller, 'Path', lambda value: processes if value == '/proc' else Path(value))
    monkeypatch.setattr(controller, '_process', lambda pid: current)
    assert controller._worker_dead(evidence) is dead


def completed_service(root, live, status='running'):
    directory = root / '.mobkit/launch-remote'
    job = controller._json_read(directory / 'operation.json')
    job.update(status='completed', phase='complete', cleanup_confirmed=True)
    controller._json_write(directory / 'operation.json', job)
    owner = controller._json_read(directory / 'owner.json')
    module, operations = controller._runtime(root, owner)
    operations.start()
    if status != 'running':
        value = controller._json_read(operations.path)
        value['status'] = status
        controller._json_write(operations.path, value)
    live['value'] = False


def test_current_native_ready_and_changed_source_running_but_not_ready(destination):
    root, request, live = install(destination)
    completed_service(root, live)
    result = controller.handle({**request, 'action': 'status'})
    assert result['status'] == 'running' and result['ready'] and result['service_owned']
    (root / 'app.py').write_text('changed source')
    result = controller.handle({**request, 'action': 'status'})
    assert result['status'] == 'running' and not result['ready'] and not result['source_current']
    assert result['service_owned'] and result['service_observed']


def test_manager_unknown_is_recovery_and_diagnostics_are_not_echoed(destination):
    root, request, live = install(destination)
    completed_service(root, live, 'private-canary-unexpected-manager-output')
    result = controller.handle({**request, 'action': 'status'})
    assert result['status'] == 'recovery-needed' and not result['ready']
    assert 'private-canary' not in json.dumps(result)


def test_stop_uses_frozen_helper_after_deployment_and_mutable_helper_change(destination):
    root, request, live = install(destination)
    completed_service(root, live)
    marker = root / 'untrusted-executed'
    (root / 'remote_operations.py').write_text('from pathlib import Path\nPath(' + repr(str(marker)) + ').touch()\n')
    (root / 'deployment.json').write_text('{"slug":"other-project","schemaVersion":2}')
    result = controller.handle({**request, 'action': 'stop', 'operation_id': str(uuid.uuid4())})
    assert 'error' not in result and result['admitted']
    assert not marker.exists()
    module, operations = controller._runtime(root, controller._json_read(root / '.mobkit/launch-remote/owner.json'))
    operations.stop()
    assert operations.status()['status'] == 'inactive'


def test_restart_requires_current_source_and_snapshot_tamper_refuses_all_controls(destination):
    root, request, live = install(destination)
    completed_service(root, live)
    (root / 'app.py').write_text('changed')
    result = controller.handle({**request, 'action': 'restart', 'operation_id': str(uuid.uuid4())})
    assert result['error']['code'] == 'source-changed'
    (root / '.mobkit/launch-remote/runtime-operations.py').write_text('private-canary')
    result = controller.handle({**request, 'action': 'stop', 'operation_id': str(uuid.uuid4())})
    assert result['error']['code'] == 'snapshot-changed'
    assert 'private-canary' not in json.dumps(result)


def test_invalid_package_never_executes_helper_or_claims_destination(destination):
    root, request, files, live = destination
    marker = root.parent / 'untrusted-code'
    files['remote_operations.py'] = ('from pathlib import Path\nPath(' + repr(str(marker)) + ').touch()').encode()
    archive, ignored = bundle(files)
    result = controller.handle({**request, 'archive': archive})
    assert result['error']['code'] == 'invalid-package'
    assert not root.exists() and not marker.exists()


@pytest.mark.parametrize('key,value', [('OPENAI_API_KEY', 42), ('PATH', '/bad'), ('MOBKIT_PORT', '80')])
def test_configuration_rejected_before_admission_or_source_materialization(destination, key, value):
    root, request, files, live = destination
    result = controller.handle({**request, 'settings': {key: value}})
    assert result['error']['code'] == 'configuration-invalid'
    assert not root.exists()


@pytest.mark.parametrize('provider_key', ['ANTHROPIC_API_KEY', 'GEMINI_API_KEY'])
def test_local_elephant_embedding_key_matches_host_fields_even_if_missing_native_declaration(destination, provider_key):
    root, request, files, live = destination
    definition = json.loads(files['deployment.json'])
    definition['environment'] = [{'key': provider_key, 'label': 'Provider key', 'secret': True, 'required': True}]
    definition['knowledge'].update(mode='local', token_env='ELEPHANT_TOKEN', space_env='ELEPHANT_SPACE_ID')
    fields = controller._fields(files, definition)
    from launchkit.deployment import configuration_fields
    assert fields == configuration_fields(definition, {'fields': []})
    prepared = controller._configuration(root, {provider_key: 'extraction', 'OPENAI_API_KEY': 'embeddings'}, fields)
    assert b'OPENAI_API_KEY="embeddings"' in prepared[3]


def test_none_and_remote_knowledge_do_not_add_embedding_requirement(destination):
    root, request, files, live = destination
    definition = json.loads(files['deployment.json'])
    definition['environment'] = [{'key': 'ANTHROPIC_API_KEY', 'label': 'Provider', 'secret': True, 'required': True}]
    for mode in ('none', 'remote'):
        definition['knowledge']['mode'] = mode
        assert {row['key'] for row in controller._fields(files, definition)} == {'ANTHROPIC_API_KEY'}


def test_fifo_private_receipt_is_rejected_without_blocking(tmp_path):
    path = tmp_path.resolve() / 'fifo'
    os.mkfifo(path, mode=0o600)
    started = time.monotonic()
    with pytest.raises(controller.ControllerError):
        controller._ordinary(path, private=True)
    assert time.monotonic() - started < 1


def test_actual_isolated_cli_emits_one_bounded_frame_and_no_raw_error(tmp_path):
    source = Path(controller.__file__).read_bytes()
    bootstrap = controller_program(source)
    assert len(bootstrap.encode('utf-8')) < 128 * 1024
    process = subprocess.run([sys.executable, '-I', '-c', bootstrap], input=b'{"action":"private-canary", "action":"status"}',
                             stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
    assert process.returncode == 0 and process.stderr == b''
    rows = process.stdout.decode().splitlines()
    assert len(rows) == 1 and rows[0].startswith(controller.FRAME_PREFIX)
    result = json.loads(rows[0][len(controller.FRAME_PREFIX):])
    assert result['error']['code'] == 'invalid-request'
    assert b'private-canary' not in process.stdout and len(process.stdout) < controller.MAX_RESULT


def test_actual_child_is_cleaned_if_birth_evidence_or_initial_journal_fails(tmp_path, monkeypatch):
    root = tmp_path.resolve()
    host = {'uid': os.getuid(), 'home': str(root), 'destination': str(root)}
    unrelated = subprocess.Popen([sys.executable, '-I', '-c', 'import time; time.sleep(30)'], start_new_session=True)
    captured = []
    popen = subprocess.Popen
    def owned(*args, **kwargs):
        process = popen(*args, **kwargs)
        captured.append(process)
        return process
    monkeypatch.setattr(controller.subprocess, 'Popen', owned)
    if sys.platform == 'linux':
        monkeypatch.setattr(controller, '_progress', lambda *args, **kwargs: (_ for _ in ()).throw(OSError('private-canary')))
    try:
        with pytest.raises(controller.ControllerError) as error:
            controller._command([sys.executable, '-I', '-c', 'import time; time.sleep(30)'],
                                root, {'host': host}, str(uuid.uuid4()), 'fixture', timeout=1)
        assert error.value.cleanup_confirmed is True
        assert captured[0].returncode is not None
        assert unrelated.poll() is None
        assert 'private-canary' not in str(error.value)
    finally:
        os.killpg(unrelated.pid, signal.SIGKILL)
        unrelated.wait(timeout=5)


@pytest.mark.skipif(sys.platform != 'linux', reason='Linux sealed memfd and /proc requirements descriptors required')
def test_actual_dependency_descriptor_carries_exact_bytes_without_package_source_in_argv(tmp_path, monkeypatch):
    root = tmp_path.resolve()
    directory = root / '.mobkit/launch-remote'
    directory.mkdir(parents=True, mode=0o700)
    content = b'ordinary-package==1.2.3\n'
    def consume(arguments, *args, **kwargs):
        path = arguments[-1]
        result = subprocess.run([sys.executable, '-I', '-c', 'import pathlib,sys; print(pathlib.Path(sys.argv[1]).read_text(),end="")', path],
                                pass_fds=kwargs['pass_fds'], capture_output=True, timeout=10)
        assert result.returncode == 0 and result.stdout == content
        assert content.decode().strip() not in repr(arguments)
    monkeypatch.setattr(controller, '_command', consume)
    controller._dependencies(root, {}, str(uuid.uuid4()), {'requirements.txt': content}, 'requirements.txt', 'fixture')


def run_capsule(monkeypatch):
    observed = []
    def actual(arguments, root, owner, operation_id, phase, **kwargs):
        result = subprocess.run(arguments, cwd=root, env=controller._environment(owner['host']),
                                pass_fds=kwargs['pass_fds'], capture_output=True, timeout=15)
        observed.append((arguments, result))
        assert result.returncode == 0, result.stderr.decode()
    monkeypatch.setattr(controller, '_command', actual)
    return observed


def fixture_python(root):
    path = root / '.venv/bin'
    path.mkdir(parents=True, mode=0o700)
    (path / 'python').symlink_to(sys.executable)


def test_actual_capsule_uses_accepted_imports_metadata_and_real_private_root(destination, monkeypatch):
    root, request, live = install(destination)
    fixture_python(root)
    files = {
        'helper.py': b'VALUE="accepted-import"\n',
        'deployment.json': b'{"name":"accepted-metadata"}',
        'application/__init__.py': b'VALUE="accepted-application"\n',
        'probe.py': b'''import importlib.util,json,helper
from pathlib import Path
root=Path(__file__).resolve().parent
spec=importlib.util.spec_from_file_location('fixture_application',root/'application/__init__.py')
module=importlib.util.module_from_spec(spec); spec.loader.exec_module(module)
value={'root':str(root),'import':helper.VALUE,'dynamic':module.VALUE,
       'metadata':json.loads((root/'deployment.json').read_text())['name'],
       'private_present':'private-canary' in (root/'.env').read_text()}
(root/'observed.json').write_text(json.dumps(value))
'''}
    marker = root / 'untrusted-import-ran'
    (root / 'helper.py').write_text('from pathlib import Path\nPath(' + repr(str(marker)) + ').touch()\n')
    (root / 'application').mkdir()
    (root / 'application/__init__.py').write_text('raise RuntimeError("untrusted app")')
    (root / 'deployment.json').write_text('{"name":"changed-metadata"}')
    observed = run_capsule(monkeypatch)
    owner = controller._json_read(root / '.mobkit/launch-remote/owner.json')
    controller._accepted_python(root, owner, str(uuid.uuid4()), files, 'probe.py', [], 'fixture')
    assert json.loads((root / 'observed.json').read_text()) == {'root': str(root), 'import': 'accepted-import',
        'dynamic': 'accepted-application', 'metadata': 'accepted-metadata', 'private_present': True}
    assert not marker.exists() and 'private-canary' not in repr(observed[0][0])


def test_actual_generated_app_check_parity_uses_capsule_without_starting_adapters(destination, monkeypatch):
    root, request, live = install(destination)
    fixture_python(root)
    owner = controller._json_read(root / '.mobkit/launch-remote/owner.json')
    files = controller._accepted_files(root, owner)
    observed = run_capsule(monkeypatch)
    controller._accepted_python(root, owner, str(uuid.uuid4()), files, 'app.py', ['--check'], 'fixture')
    assert b'Valid deployment:' in observed[0][1].stdout
    assert not (root / '.mobkit/state').exists()


def test_actual_capsule_guards_subprocess_after_accepted_settings_change(destination, monkeypatch):
    root, request, live = install(destination)
    fixture_python(root)
    files = {
        'knowledge/settings.json': b'{"accepted":true}',
        'knowledge/manage.py': b'''import json,subprocess,sys
from pathlib import Path
root=Path(__file__).resolve().parent
assert json.loads((root/'settings.json').read_text())=={'accepted':True}
child='import pathlib,sys; pathlib.Path(sys.argv[1]).touch()'
subprocess.run([sys.executable,'-I','-c',child,str(root/'accepted-child-ran')],check=True)
(root/'settings.json').write_text('{"changed":true}')
try:
    subprocess.run([sys.executable,'-I','-c',child,str(root/'changed-child-ran')],check=True)
except RuntimeError as error:
    assert str(error)=='Accepted installation source changed.'
    (root/'guard-observed.json').write_text(json.dumps({
        'guarded':True,'root':str(root),'argument':sys.argv[1]}))
else:
    raise RuntimeError('Changed source must not spawn a child.')
'''}
    for name, content in files.items():
        path = root / name
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
    observed = run_capsule(monkeypatch)
    owner = controller._json_read(root / '.mobkit/launch-remote/owner.json')
    controller._accepted_python(root, owner, str(uuid.uuid4()), files, 'knowledge/manage.py', ['fixture'], 'fixture')
    assert (root / 'knowledge/accepted-child-ran').exists()
    assert not (root / 'knowledge/changed-child-ran').exists()
    assert json.loads((root / 'knowledge/guard-observed.json').read_text()) == {
        'guarded': True, 'root': str(root / 'knowledge'), 'argument': 'fixture'}
    assert 'private-canary' not in repr(observed[0][0])
    assert b'private-canary' not in observed[0][1].stdout + observed[0][1].stderr


def test_mutation_during_dependency_work_refuses_gateway_and_application_execution(destination, monkeypatch):
    root, request, live = install(destination)
    fixture_python(root)
    owner = controller._json_read(root / '.mobkit/launch-remote/owner.json')
    module, operations = controller._runtime(root, owner)
    phases = []
    def mutate(arguments, root, owner, operation_id, phase, **kwargs):
        phases.append(phase)
        (root / 'install_gateway.py').write_text('raise RuntimeError("changed helper must not run")')
        (root / 'app.py').write_text('raise RuntimeError("changed app must not run")')
    monkeypatch.setattr(controller, '_command', mutate)
    monkeypatch.setattr(controller, '_venv_ready', lambda *args: True)
    with pytest.raises(controller.ControllerError) as error:
        controller._install_dependencies(root, owner, str(uuid.uuid4()), module)
    assert error.value.code == 'source-changed' and phases == ['dependencies']


@pytest.mark.parametrize('initial', ['partial-directory', 'interpreter-without-pip'])
def test_actual_owned_partial_venv_is_repaired_without_clearing_data(destination, monkeypatch, initial):
    root, request, live = install(destination)
    virtualenv = root / '.venv'
    if initial == 'interpreter-without-pip':
        result = subprocess.run([sys.executable, '-I', '-m', 'venv', '--without-pip', str(virtualenv)],
                                capture_output=True, timeout=20)
        assert result.returncode == 0
    else:
        (virtualenv / 'bin').mkdir(parents=True)
    (virtualenv / 'retained').write_text('partial native environment data')
    (root / 'data').mkdir()
    (root / 'data/retained').write_text('application data')
    owner = controller._json_read(root / '.mobkit/launch-remote/owner.json')
    module, operations = controller._runtime(root, owner)
    commands = []
    def actual(arguments, root, owner, operation_id, phase, **kwargs):
        commands.append((arguments, phase))
        assert arguments == [sys.executable, '-I', '-m', 'venv', str(virtualenv)]
        result = subprocess.run(arguments, cwd=root, env=controller._environment(owner['host']),
                                capture_output=True, timeout=30)
        assert result.returncode == 0, result.stderr.decode()
    monkeypatch.setattr(controller, '_command', actual)
    monkeypatch.setattr(controller, '_probe', lambda arguments, host: subprocess.run(
        arguments, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, timeout=10).returncode == 0)
    monkeypatch.setattr(controller, '_dependencies', lambda *args: None)
    monkeypatch.setattr(controller, '_accepted_python', lambda *args: None)
    controller._install_dependencies(root, owner, str(uuid.uuid4()), module)
    assert len(commands) == 1 and commands[0][1] == 'python-environment'
    assert controller._venv_ready(root, owner)
    assert (virtualenv / 'retained').read_text() == 'partial native environment data'
    assert (root / 'data/retained').read_text() == 'application data'
    assert 'private-canary' not in repr(commands)


@pytest.mark.parametrize('unsafe', ['bin-link', 'lib-link', 'cfg-link', 'python-link', 'script-hardlink'])
def test_partial_venv_refuses_unsafe_repair_targets(destination, monkeypatch, unsafe):
    root, request, live = install(destination)
    virtualenv = root / '.venv'
    virtualenv.mkdir()
    external = root.parent / 'external-fixture'
    external.mkdir()
    foreign = external / 'retained'
    foreign.write_text('must remain untouched')
    if unsafe in {'bin-link', 'lib-link'}:
        (virtualenv / unsafe.removesuffix('-link')).symlink_to(external, target_is_directory=True)
    elif unsafe == 'cfg-link':
        (virtualenv / 'pyvenv.cfg').symlink_to(foreign)
    else:
        (virtualenv / 'bin').mkdir()
        if unsafe == 'python-link':
            (virtualenv / 'bin/python').symlink_to(foreign)
        else:
            os.link(foreign, virtualenv / 'bin/activate')
    owner = controller._json_read(root / '.mobkit/launch-remote/owner.json')
    module, operations = controller._runtime(root, owner)
    monkeypatch.setattr(controller, '_command', lambda *args: pytest.fail('Unsafe repair must not execute.'))
    with pytest.raises(controller.ControllerError) as error:
        controller._install_dependencies(root, owner, str(uuid.uuid4()), module)
    assert error.value.code == 'unsafe-path' and foreign.read_text() == 'must remain untouched'


def test_service_checks_consume_capsule_and_guard_each_manager_boundary(destination, monkeypatch):
    root, request, live = install(destination)
    owner = controller._json_read(root / '.mobkit/launch-remote/owner.json')
    module, operations = controller._runtime(root, owner)
    checks = []
    monkeypatch.setattr(controller, '_accepted_python', lambda *args: checks.append(args[4:]))
    module['command'] = lambda *args, **kwargs: (0, 'fixed-fixture-observation')
    controller._verified_service_commands(root, owner, str(uuid.uuid4()), module)
    module['command']([str(root / '.venv/bin/python'), 'app.py', '--doctor'])
    assert checks[0][0] == 'app.py' and checks[0][1] == ['--doctor']
    (root / 'app.py').write_text('changed during prior check')
    with pytest.raises(controller.ControllerError) as error:
        module['command'](['/usr/bin/systemctl', '--user', 'start', 'fixture'])
    assert error.value.code == 'source-changed'


def interface_source(destination, monkeypatch, entries=None):
    root, request, files, live = destination
    monkeypatch.setattr(controller, 'APPLICATION_EXPERIENCES_SOURCE',
        Path(controller.__file__).with_name('application_experiences.py').read_bytes(), raising=False)
    if entries is not None:
        files['application/experience.json'] = json.dumps({'entries': entries}).encode()
        files['application/configuration.json'] = json.dumps({'fields': [
            {'key': 'APP_UI_PORT', 'label': 'Interface port', 'secret': False, 'required': True},
            {'key': 'APP_PASSWORD', 'label': 'Password', 'secret': True, 'required': True}]}).encode()
        request['settings'].update(APP_UI_PORT='9912', APP_PASSWORD='private-app-canary', MOBKIT_PORT='9911')
    files['remote_operations.py'] += b'''
original_fixture_status = Operations.status
def interface_fixture_status(self):
    value = original_fixture_status(self)
    if value.get('ready'):
        value.update(console_url='http://127.0.0.1:9911/console',source_digest=source_digest(self.root))
    return value
Operations.status = interface_fixture_status
'''
    request['archive'], request['binding']['transferred_source_digest'] = bundle(files)
    root, request, live = install(destination)
    owner = controller._json_read(root / '.mobkit/launch-remote/owner.json')
    module, operations = controller._runtime(root, owner)
    operations.start()
    job = controller._job(root)
    job.update(status='complete', phase='complete')
    controller._json_write(root / '.mobkit/launch-remote/operation.json', job)
    live['value'] = False
    return root, request, owner


def test_status_resolves_frozen_metadata_actual_ports_and_never_emits_private_values(destination, monkeypatch):
    entries = [{'id': 'extra', 'label': 'Extra view', 'path': '/extra'},
               {'id': 'reader', 'label': 'Reader', 'path': '/reading', 'port_field': 'APP_UI_PORT'},
               {'id': 'literal', 'label': 'Other view', 'path': '/', 'port': 9913}]
    root, request, owner = interface_source(destination, monkeypatch, entries)
    ports = []
    def listener(root, owner, operations, port, deadline):
        ports.append(port)
        return str(port)[0] * 64
    monkeypatch.setattr(controller, '_listener_proof', listener)
    result = controller.handle({**request, 'action': 'status'})
    interfaces = result['application_experiences']
    assert interfaces['supported'] is True and interfaces['status'] == 'resolved'
    assert interfaces['source_digest'] == request['binding']['accepted_source_digest']
    assert interfaces['installed_source_digest'] == request['binding']['transferred_source_digest']
    assert ports == [9911, 9912, 9913]
    assert [row['destination_port'] for row in interfaces['entries']] == ports
    assert all(row['listener_owned'] and row['listener_token'] and row['verified'] is False
               and row['reason'] is None for row in interfaces['entries'])
    assert 'private-app-canary' not in json.dumps(result) and 'private-canary' not in json.dumps(result)
    # Mutable metadata invalidates current source; it never becomes entry authority.
    (root / 'application/experience.json').write_text('{"entries":[]}')
    refused = controller.handle({**request, 'action': 'status'})['application_experiences']
    assert refused['status'] == 'unavailable' and refused['entries'] == []


def test_status_distinguishes_no_interface_metadata_from_unsupported_parser(destination, monkeypatch):
    root, request, owner = interface_source(destination, monkeypatch)
    result = controller.handle({**request, 'action': 'status'})['application_experiences']
    assert result['status'] == 'none' and result['metadata_sha256'] is None and result['entries'] == []
    monkeypatch.delattr(controller, 'APPLICATION_EXPERIENCES_SOURCE')
    result = controller.handle({**request, 'action': 'status'})['application_experiences']
    assert result['supported'] is False and result['status'] == 'unavailable'


@pytest.mark.parametrize('problem', ['listener', 'port', 'metadata', 'private-file', 'active-worker'])
def test_interface_resolution_failure_is_nonblocking_and_does_not_echo_values(destination, monkeypatch, problem):
    entries = [{'id': 'reader', 'label': 'Reader', 'path': '/', 'port_field': 'APP_UI_PORT'}]
    if problem == 'metadata':
        entries[0]['port_field'] = 'APP_PASSWORD'
    root, request, owner = interface_source(destination, monkeypatch, entries)
    calls = []
    def unavailable(*args):
        calls.append(True)
        raise RuntimeError('private-error-canary')
    monkeypatch.setattr(controller, '_listener_proof', unavailable)
    if problem == 'port':
        with (root / '.env').open('a') as stream:
            stream.write('APP_UI_PORT="private-port-canary"\n')
    elif problem == 'private-file':
        (root / '.env').chmod(0o644)
    elif problem == 'active-worker':
        destination[3]['value'] = True
    result = controller.handle({**request, 'action': 'status'})
    assert 'error' not in result
    interfaces = result['application_experiences']
    assert all(row['listener_owned'] is False and row['listener_token'] is None
               and row['verified'] is False for row in interfaces['entries'])
    assert 'private-error-canary' not in json.dumps(result) and 'private-port-canary' not in json.dumps(result)
    if problem != 'listener':
        assert calls == []


@pytest.fixture
def listener_fixture(tmp_path, monkeypatch):
    """Synthetic proc/cgroup files; no real listener or Linux manager is used."""
    base = tmp_path.resolve()
    proc, groups = base / 'proc', base / 'groups'
    unit = 'mobkit-launch.11111111-1111-4111-8111-111111111111.service'
    group = '/user.slice/app.slice/' + unit
    members = groups / group.lstrip('/') / 'cgroup.procs'
    members.parent.mkdir(parents=True)
    members.write_text('42\n')
    for directory in ('self/net', 'self/ns', 'sys/kernel/random', '42/ns', '42/fd'):
        (proc / directory).mkdir(parents=True, exist_ok=True)
    (proc / 'sys/kernel/random/boot_id').write_text('11111111-1111-4111-8111-111111111111\n')
    (proc / 'self/ns/net').symlink_to('net:[123]')
    (proc / '42/ns/net').symlink_to('net:[123]')
    (proc / '42/fd/7').symlink_to('socket:[9876]')
    fields = ['S'] + ['0'] * 18 + ['100'] + ['0'] * 3
    (proc / '42/stat').write_text('42 (private-process-canary) ' + ' '.join(fields))
    (proc / '42/cgroup').write_text('0::' + group + '\n')
    def row(address='0100007F', uid=None, inode=9876, port=9912):
        return f'0: {address}:{port:04X} 00000000:0000 0A 00000000:00000000 00:00000000 00000000 {os.getuid() if uid is None else uid} 0 {inode} 1\n'
    (proc / 'self/net/tcp').write_text('header\n' + row())
    (proc / 'self/net/tcp6').write_text('header\n')
    monkeypatch.setattr(controller, 'PROC_ROOT', proc)
    monkeypatch.setattr(controller, 'CGROUP_ROOT', groups)
    receipt = {'unit_path': str(base / unit), 'source_digest': 'c' * 64}
    state = {'LoadState': 'loaded', 'ActiveState': 'active', 'MainPID': '42', 'ControlPID': '0',
             'ControlGroup': group, 'FragmentPath': receipt['unit_path']}
    class Operations:
        def load(self, **kwargs):
            assert kwargs['require_source'] is True
            return dict(receipt)
        def inspect(self, selected):
            assert selected == receipt
            return dict(state)
    owner = {'deployment_id': '11111111-1111-4111-8111-111111111111', 'binding': {'fixture': 'bound'}}
    return base, proc, members, owner, Operations(), state, row


@pytest.mark.parametrize('address', ['0100007F', '00000000'])
def test_listener_proof_accepts_stable_owned_loopback_or_wildcard_and_token_tracks_birth(listener_fixture, address):
    base, proc, members, owner, operations, state, row = listener_fixture
    (proc / 'self/net/tcp').write_text('header\n' + row(address=address))
    first = controller._listener_proof(base, owner, operations, 9912, time.monotonic() + 4)
    assert len(first) == 64 and 'private-process-canary' not in first
    assert controller._listener_proof(base, owner, operations, 9912, time.monotonic() + 4) == first
    stat_file = proc / '42/stat'
    stat_file.write_text(stat_file.read_text().replace(' 100 ', ' 101 '))
    assert controller._listener_proof(base, owner, operations, 9912, time.monotonic() + 4) != first


@pytest.mark.parametrize('problem', ['foreign', 'multiple', 'ipv6', 'other-address', 'missing-fd', 'group',
    'namespace', 'duplicate-members', 'main-missing', 'manager', 'large-stat', 'pid-bound', 'fd-bound', 'elapsed'])
def test_listener_proof_refuses_foreign_ambiguous_unreadable_or_exhausted_custody(listener_fixture, monkeypatch, problem):
    base, proc, members, owner, operations, state, row = listener_fixture
    if problem == 'foreign':
        (proc / 'self/net/tcp').write_text('header\n' + row(uid=os.getuid() + 1))
    elif problem == 'multiple':
        (proc / 'self/net/tcp').write_text('header\n' + row() + row(inode=9877))
    elif problem == 'ipv6':
        (proc / 'self/net/tcp6').write_text('header\n' + row(address='0' * 32))
    elif problem == 'other-address':
        (proc / 'self/net/tcp').write_text('header\n' + row(address='0200007F'))
    elif problem == 'missing-fd':
        (proc / '42/fd/7').unlink()
    elif problem == 'group':
        (proc / '42/cgroup').write_text('0::/foreign.service\n')
    elif problem == 'namespace':
        (proc / '42/ns/net').unlink()
        (proc / '42/ns/net').symlink_to('net:[999]')
    elif problem == 'duplicate-members':
        members.write_text('42\n42\n')
    elif problem == 'main-missing':
        members.write_text('43\n')
    elif problem == 'manager':
        state['LoadState'] = 'not-found'
    elif problem == 'large-stat':
        (proc / '42/stat').write_text('private-canary' * 8192)
    elif problem == 'pid-bound':
        members.write_text('\n'.join(str(value) for value in range(1, 258)))
    elif problem == 'fd-bound':
        monkeypatch.setattr(controller, 'MAX_LISTENER_FDS', 0)
    deadline = time.monotonic() - 1 if problem == 'elapsed' else time.monotonic() + 4
    with pytest.raises((controller.ControllerError, OSError, ValueError)):
        controller._listener_proof(base, owner, operations, 9912, deadline)


@pytest.mark.parametrize('boundary', ['birth', 'socket', 'membership', 'manager', 'fd'])
def test_listener_proof_rechecks_custody_and_rejects_races(listener_fixture, monkeypatch, boundary):
    base, proc, members, owner, operations, state, row = listener_fixture
    if boundary == 'fd':
        original = os.readlink
        calls = []
        def changed(path, **kwargs):
            if path == '7':
                calls.append(True)
                if len(calls) > 1:
                    return 'socket:[9999]'
            return original(path, **kwargs)
        monkeypatch.setattr(controller.os, 'readlink', changed)
    else:
        name = {'birth': '_listener_birth', 'socket': '_listener_socket',
                'membership': '_listener_members', 'manager': '_listener_manager'}[boundary]
        original = getattr(controller, name)
        calls = []
        def changed(*args, **kwargs):
            value = original(*args, **kwargs)
            calls.append(True)
            if len(calls) >= (3 if boundary == 'birth' else 2):
                if boundary == 'birth':
                    value['start_ticks'] = '999'
                elif boundary == 'socket':
                    value['inode'] = 9999
                elif boundary == 'membership':
                    value.add(43)
                else:
                    value['MainPID'] = '43'
            return value
        monkeypatch.setattr(controller, name, changed)
    with pytest.raises(controller.ControllerError):
        controller._listener_proof(base, owner, operations, 9912, time.monotonic() + 4)


def test_listener_manager_calls_are_bounded_and_original_manager_is_restored(listener_fixture):
    base, proc, members, owner, operations, state, row = listener_fixture
    calls = []
    def manager(arguments, **kwargs):
        calls.append((arguments, kwargs['timeout']))
        return 0, ''
    operations.manager = manager
    def inspect(receipt):
        operations.manager(['show', Path(receipt['unit_path']).name], timeout=60)
        return dict(state)
    operations.inspect = inspect
    assert controller._listener_proof(base, owner, operations, 9912, time.monotonic() + 4)
    assert len(calls) == 2 and all(0 < timeout <= 2 for _, timeout in calls)
    assert operations.manager is manager


def test_listener_numeric_pid_replacement_cannot_change_retained_directory_authority(listener_fixture, monkeypatch):
    base, proc, members, owner, operations, state, row = listener_fixture
    original = controller._listener_birth
    calls = []
    def replaced(*args):
        calls.append(True)
        if len(calls) == 3:
            (proc / '42').rename(proc / 'old-42')
            (proc / '42').mkdir()
        return original(*args)
    monkeypatch.setattr(controller, '_listener_birth', replaced)
    with pytest.raises(controller.ControllerError):
        controller._listener_proof(base, owner, operations, 9912, time.monotonic() + 4)


def test_actual_fixture_listener_proof_is_joined_to_frozen_application_projection(destination, listener_fixture, monkeypatch):
    entries = [{'id': 'reader', 'label': 'Reader', 'path': '/', 'port_field': 'APP_UI_PORT'}]
    root, request, owner = interface_source(destination, monkeypatch, entries)
    base, proc, members, unused_owner, operations, state, row = listener_fixture
    module, _ = controller._runtime(root, owner)
    monkeypatch.setattr(controller, '_runtime', lambda selected, bound: (module, operations))
    observed = {'source_current': True, 'ready': True, 'worker_live': False,
                'service_owned': True, 'service_observed': True,
                'service': {'console_url': 'http://127.0.0.1:9911/console'}}
    result = controller._application_experiences(root, owner, observed)
    assert result['status'] == 'resolved' and len(result['entries']) == 1
    entry = result['entries'][0]
    assert entry['destination_port'] == 9912 and entry['listener_owned'] is True
    assert entry['listener_token'] and entry['verified'] is False and entry['reason'] is None
    assert 'private-canary' not in json.dumps(result)
    (proc / '42/fd/7').unlink()
    result = controller._application_experiences(root, owner, observed)
    assert result['entries'][0]['listener_owned'] is False and result['entries'][0]['listener_token'] is None


def test_product_preflight_names_the_cpython_its_product_needs(destination, monkeypatch):
    """A product whose control tools need another CPython fails one named check, before transfer."""
    root, request, files, live = destination
    from launchkit import remote_product_control as control
    needed = sys.version_info.minor + 1
    manifest = {'format': control.FORMAT, 'interpreter': {'implementation': sys.implementation.name,
        'machine': 'x86_64', 'python_tag': 'cp3' + str(needed), 'abi_tag': 'cp3' + str(needed),
        'glibc_minimum': [2, 31]}}
    monkeypatch.setattr(controller, 'PRODUCT_CONTROL_MANIFEST', json.dumps(manifest).encode(), raising=False)
    monkeypatch.setattr(controller, 'PRODUCT_CONTROL_ADAPTER_SOURCE', Path(control.__file__).read_bytes(),
        raising=False)
    result = controller.handle({**request, 'action': 'preflight'})
    assert 'error' not in result, result
    assert result['ready'] is False and result['status'] == 'preflight'
    (check,) = [row for row in result['checks'] if row['name'] == 'product-interpreter']
    assert check['status'] == 'failed'
    assert 'CPython 3.%d on Linux x86_64 with glibc 2.31 or newer' % needed in check['detail']
    assert 'this destination runs CPython %d.%d on x86_64 with glibc 2.31' % sys.version_info[:2] in check['detail']
    assert 'product_interpreter' not in result and result['private_settings_supported'] is False
    assert not root.exists()
