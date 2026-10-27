"""Actual isolated Python execution, without SSH, sockets or native services."""
import base64
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import zlib

from launchkit.remote_interface_bundle import controller_bytes
from launchkit import application_experiences, remote_destination
from launchkit.templates_v2 import console_configuration


def test_bundle_contains_exact_canonical_parser_before_main_and_retains_binding_contract():
    content = controller_bytes()
    namespace = {'__name__': 'fixture', 'CONTROLLER_SOURCE': content}
    exec(compile(content, '<frozen-controller>', 'exec'), namespace)
    assert namespace['APPLICATION_EXPERIENCES_SOURCE'] == Path(application_experiences.__file__).read_bytes()
    assert namespace['CONSOLE_CONFIGURATION_SOURCE'] == Path(console_configuration.__file__).read_bytes()
    assert namespace['_controller_source']() == content
    assert namespace['BINDING_KEYS'] == remote_destination.BINDING_KEYS
    assert content.index(b'APPLICATION_EXPERIENCES_SOURCE = ') < content.index(b"if __name__ == '__main__':")
    assert content.index(b'CONSOLE_CONFIGURATION_SOURCE = ') < content.index(b"if __name__ == '__main__':")
    assert hashlib.sha256(content).hexdigest() != hashlib.sha256(Path(remote_destination.__file__).read_bytes()).hexdigest()


def test_actual_isolated_bundled_parser_uses_verified_dependency_without_mutable_imports(tmp_path):
    root = tmp_path.resolve()
    private = root / '.mobkit/launch-remote'
    private.mkdir(parents=True, mode=0o700)
    helper = Path(application_experiences.__file__).parent / 'templates_v2/application_configuration.py'
    exact = helper.read_bytes()
    snapshot = private / 'application-configuration.py'
    snapshot.write_bytes(exact)
    snapshot.chmod(0o600)
    # Both tempting mutable module paths must remain inert under python -I.
    (root / 'application_configuration.py').write_text("raise RuntimeError('mutable-canary')")
    (root / 'application').mkdir()
    (root / 'application/__init__.py').write_text("raise RuntimeError('mutable-canary')")
    payload = base64.b64encode(zlib.compress(controller_bytes(), level=9)).decode()
    assert len(payload.encode('ascii')) < 128 * 1024
    program = '''import base64,json,pathlib,sys,zlib
s=zlib.decompress(base64.b64decode(sys.argv[1]));n={'__name__':'fixture','CONTROLLER_SOURCE':s}
exec(compile(s,'<frozen-controller>','exec'),n)
root=pathlib.Path(sys.argv[2]); owner={'snapshots':{'application-configuration.py':sys.argv[3]}}
p=n['_experience_parser'](root,owner)
files={'application/experience.json':b'{"entries":[{"id":"reader","label":"Reader","path":"/","port_field":"APP_UI_PORT"}]}',
'application/configuration.json':b'{"fields":[{"key":"APP_UI_PORT","label":"Port","secret":false,"required":true}]}'}
r=p.resolve_application_experiences(files,{'environment':[]},source_digest='a'*64,
configuration_values={'APP_UI_PORT':'9912','PASSWORD':'private-canary'})
print(json.dumps(r))
'''
    result = subprocess.run([sys.executable, '-I', '-c', program, payload, str(root), hashlib.sha256(exact).hexdigest()],
        cwd=root, env={'PATH': os.environ.get('PATH', '/usr/bin:/bin')}, capture_output=True, text=True, timeout=8)
    assert result.returncode == 0, result.stderr
    parsed = json.loads(result.stdout)
    assert parsed['entries'][0]['destination_port'] == 9912
    assert 'private-canary' not in result.stdout and 'mutable-canary' not in result.stdout
    snapshot.write_text("raise RuntimeError('private-canary')")
    refused = subprocess.run([sys.executable, '-I', '-c', program, payload, str(root), hashlib.sha256(exact).hexdigest()],
        cwd=root, env={'PATH': os.environ.get('PATH', '/usr/bin:/bin')}, capture_output=True, text=True, timeout=8)
    assert refused.returncode != 0 and 'private-canary' not in refused.stdout + refused.stderr


def test_actual_isolated_controller_enforces_console_rules_without_accepted_or_mutable_helper_imports(tmp_path):
    (tmp_path / 'console_configuration.py').write_text("raise RuntimeError('mutable-canary')")
    payload = base64.b64encode(zlib.compress(controller_bytes(), level=9)).decode()
    assert len(payload.encode('ascii')) < 128 * 1024
    program = '''import base64,json,pathlib,sys,zlib
s=zlib.decompress(base64.b64decode(sys.argv[1]));n={'__name__':'fixture','CONTROLLER_SOURCE':s}
exec(compile(s,'<frozen-controller>','exec'),n)
fields=[{'key':'CONSOLE_USER_ID','label':'Console user','secret':False,'required':True},
{'key':'CONSOLE_LOGIN_PASSWORD','label':'Console password','secret':True,'required':True}]
files={'application_configuration.py':b'def validate_application_fields(value, existing_keys=()):\\n    return value["fields"]\\n',
'application/console.json':b'{"authentication":"local-password"}',
'application/configuration.json':json.dumps({'fields':fields}).encode()}
fields,validator=n['_fields'](files,{'environment':[]},with_validator=True)
root=pathlib.Path(sys.argv[2])/'unchanged'
try:n['_configuration'](root,{'CONSOLE_USER_ID':'owner','CONSOLE_LOGIN_PASSWORD':'bad-marker'},fields,validator)
except n['ControllerError'] as error:print(json.dumps({'code':error.code,'fields':error.fields,'root_exists':root.exists()}))
else:raise RuntimeError('Expected rejection')
'''
    result = subprocess.run([sys.executable, '-I', '-c', program, payload, str(tmp_path)],
        cwd=tmp_path, env={'PATH': os.environ.get('PATH', '/usr/bin:/bin')}, capture_output=True, text=True, timeout=8)
    assert result.returncode == 0, result.stderr
    value = json.loads(result.stdout)
    assert value['code'] == 'configuration-invalid' and not value['root_exists']
    assert value['fields'] == [{'key': 'CONSOLE_LOGIN_PASSWORD', 'label': 'Console password',
                               'message': 'Use 12-4096 characters for the Console login password.'}]
    assert 'bad-marker' not in result.stdout and 'mutable-canary' not in result.stdout + result.stderr
