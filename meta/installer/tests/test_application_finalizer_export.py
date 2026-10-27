"""Isolated source export controls, without Toolkit/native/runtime execution."""
import json
from pathlib import Path
import subprocess
import sys

import pytest

from launchkit.compiler import completed_application_policy_inputs, connect_application_finalizer_export
from launchkit.connect_application_policy import completed_application_policy_inputs as policy_inputs
from launchkit.generator import PackageFile
from test_completed_application_policy import fixture
from test_helperless_completed_bundle import fixture as helperless_fixture


def test_compiler_compatibility_wrapper_preserves_exact_inputs_and_package_file(tmp_path):
    f = fixture(tmp_path)
    exported = policy_inputs(f.bundle, f.source_policy)
    public = completed_application_policy_inputs(f.bundle, f.source_policy)
    assert isinstance(public['acquisition'], PackageFile)
    assert public['local_policy_input'] == exported['local_policy_input']
    assert public['source_artifacts'] == exported['source_artifacts']
    for name in ('path', 'content', 'mode'):
        assert getattr(public['acquisition'], name) == getattr(exported['acquisition'], name)
    assert public['acquisition'].as_bytes() == exported['acquisition'].as_bytes()


@pytest.mark.parametrize("helperless", [False, True])
def test_actual_finalizer_export_imports_and_projects_without_forge_or_site_packages(tmp_path, helperless):
    retained = tmp_path / 'retained'
    retained.mkdir(mode=0o700)
    f = helperless_fixture(retained) if helperless else fixture(retained)
    root = tmp_path / 'export'
    root.mkdir(mode=0o700)
    files = connect_application_finalizer_export()
    expected = {'launchkit/__init__.py', 'launchkit/connect_application_finalizer.py',
        'launchkit/connect_application_policy.py', 'launchkit/connect_installer_caller.py',
        'launchkit/connect_policy_input.py', 'launchkit/connect_application_startup.py',
        'launchkit/connect_application_callbacks.py'}
    assert {item.path for item in files} == expected
    for item in files:
        destination = root / item.path
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(item.as_bytes())
        destination.chmod(item.mode)
        if item.path != 'launchkit/__init__.py':
            assert item.as_bytes() == (Path(__file__).parents[1] / item.path).read_bytes()
    assert (root / 'launchkit/__init__.py').read_bytes() == b''
    pins = f.bundle.pins()
    pins['source_policy'] = f.source_policy.pin()
    pin_file = tmp_path / 'bundle-pins.json'
    pin_file.write_text(json.dumps(pins))
    expected_bytes = completed_application_policy_inputs(f.bundle, f.source_policy)
    code = '''
import json, pathlib, sys
sys.path.insert(0, sys.argv[1])
from launchkit.connect_application_finalizer import ApplicationPolicyFinalizer, application_command_executor
from launchkit.connect_application_policy import completed_application_policy_inputs
from launchkit.connect_installer_caller import CompletedApplicationSetupBundle, ExactFile
p = json.loads(pathlib.Path(sys.argv[2]).read_text())
def pin(value): return ExactFile(pathlib.Path(value['path']), value['sha256'])
def directory(value): return pathlib.Path(value['path']), (value['device'], value['inode'])
bundle = CompletedApplicationSetupBundle(
    directory=directory(p['directory']),
    resources_directory=directory(p['resources_directory']) if 'resources_directory' in p else None,
    manifest=pin(p['manifest']), setup=pin(p['setup']), application_tool_policy=pin(p['application_tool_policy']),
    resource_handoff=pin(p['resource_handoff']) if 'resource_handoff' in p else None, setup_input=None,
    member_surfaces=tuple((pin(row['original']), pin(row['output'])) for row in p['member_surfaces']),
    source_files=tuple(pin(row) for row in p['source_files']), _require_current=lambda: None,
    preparation_plan=pin(p['preparation_plan']))
result = completed_application_policy_inputs(bundle, pin(p['source_policy']))
assert not any(name.startswith(('launchkit.compiler', 'launchkit.generator', 'launchkit.blueprint',
    'launchkit.schema', 'pydantic', 'meerkat_toolkit', 'meerkat_meta')) for name in sys.modules)
print(json.dumps({'policy': result['local_policy_input'].decode(),
    'acquisition': result['acquisition'].as_bytes().decode(),
    'artifact_names': [pin.path.name for pin in result['source_artifacts']]}))
'''
    process = subprocess.run([sys.executable, '-I', '-S', '-c', code, str(root), str(pin_file)],
        cwd=root, capture_output=True, timeout=5)
    assert process.returncode == 0, process.stderr.decode()
    result = json.loads(process.stdout)
    assert result['policy'].encode() == expected_bytes['local_policy_input']
    assert result['acquisition'].encode() == expected_bytes['acquisition'].as_bytes()
    assert result['artifact_names'] == [pin.path.name for pin in expected_bytes['source_artifacts']]
    assert not f.calls
