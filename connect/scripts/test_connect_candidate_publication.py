"""Offline publication controls with explicit fake gh responses and reader.

These exercise byte/owner/transport refusals. They do not verify signatures,
qualify the published Meta package, publish assets, or run an installed product.
"""
from pathlib import Path
import copy
import hashlib
import importlib.util
import io
import json
import os
import re
import shutil
import subprocess
import tarfile
import time

import pytest

SPEC = importlib.util.spec_from_file_location("connect_candidate_publication", Path(__file__).with_name("connect-candidate.py"))
p = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(p)

# The publishing environment supplies the private names (CONNECT_PUBLICATION_PRIVATE_NAMES).
# Here the neutral renames that stand in for the original private names in these
# fixtures are the supplied names, so every refusal keeps its original meaning.
STAND_IN_PRIVATE_NAMES = r"paired[ _-]?host|organizational host|memory_?store|LICENSE-EXTRACTED"


@pytest.fixture(autouse=True)
def supplied_private_names(monkeypatch):
    monkeypatch.setattr(p, 'PRIVATE', p.private_pattern(STAND_IN_PRIVATE_NAMES))


def dump(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, sort_keys=True, indent=2) + "\n")


def pin(path):
    return {"path": str(path), "size": path.stat().st_size, "sha256": p.digest(path)}


def native_selection(core_version, meta_version=None):
    if meta_version is None:
        meta_version = core_version
    return {name: {"repository": "lukacf/" + repo, "version": version, "tag": "v" + version, "source_sha": sha * 40}
            for name, repo, sha, version in (("core", "meerkat", "c", core_version),
                                             ("meta", "meerkat-meta", "d", meta_version))}


def verified_statement(path, request):
    repo = "https://github.com/" + p.PUBLICATION_REPO
    identity = f"{repo}/{p.PUBLICATION_WORKFLOW}@" + request["source_ref"]
    invocation = f"{repo}/actions/runs/{request['run_id']}/attempts/{request['run_attempt']}"
    cert = {"issuer": "https://token.actions.githubusercontent.com", "subjectAlternativeName": identity,
        "buildSignerURI": identity, "buildSignerDigest": request["source_sha"], "sourceRepositoryURI": repo,
        "sourceRepositoryDigest": request["source_sha"], "sourceRepositoryRef": request["source_ref"],
        "buildConfigURI": identity, "buildConfigDigest": request["source_sha"], "runnerEnvironment": "github-hosted",
        "buildTrigger": "workflow_dispatch", "runInvocationURI": invocation}
    statement = {"_type": "https://in-toto.io/Statement/v1", "predicateType": p.SLSA,
        "subject": [{"name": path.name, "digest": {"sha256": p.digest(path)}}],
        "predicate": {"buildDefinition": {"buildType": "https://actions.github.io/buildtypes/workflow/v1",
            "externalParameters": {"workflow": {"repository": repo, "path": p.PUBLICATION_WORKFLOW, "ref": request["source_ref"]}},
            "internalParameters": {"github": {"event_name": "workflow_dispatch", "runner_environment": "github-hosted"}},
            "resolvedDependencies": [{"uri": f"git+{repo}@{request['source_ref']}", "digest": {"gitCommit": request["source_sha"]}}]},
            "runDetails": {"builder": {"id": identity}, "metadata": {"invocationId": invocation}}}}
    return [{"verificationResult": {"signature": {"certificate": cert}, "verifiedTimestamps": [{"fixture": True}], "statement": statement}}]


# The upload receipt check: synthetic scan reports for the candidate's exact files. Made-up digests;
# the tool commit is the request's source commit. No caller gives the check a pin: it reads the pins
# of the target, and the coverage exceptions file, from the commit of the checkout it runs from. So
# these tests run a copy of the check in made-up checkouts (scratch git repositories).
RECEIPT_PINS = {'name_list_sha256': 'a1' * 32, 'rules_sha256': '8b' * 32, 'non_published_list_sha256': 'd5' * 32,
                'detector_pinned_sha256': 'b2' * 32, 'detector_current_sha256': 'c3' * 32}
RECEIPT_EXCEPTIONS = '{"schema": "release-payload-privacy-scan/coverage-exceptions/1", "exceptions": []}\n'
# The check as the candidate tool finds it: the file beside it, in its own checkout.
CHECKOUT_RECEIPT_CHECK = p.RECEIPT_CHECK


def receipt_checkout(root, pins):
    """A made-up checkout whose commit holds a copy of the receipt check, pins (the same for every
    target) and a coverage exceptions file. Returns the check there."""
    (root / 'scripts').mkdir(parents=True)
    (root / 'release').mkdir()
    shutil.copyfile(CHECKOUT_RECEIPT_CHECK, root / 'scripts' / CHECKOUT_RECEIPT_CHECK.name)
    dump(root / 'release/upload-receipt-pins.json',
         {'schema': 'release-upload-receipt-check/pins/2',
          'targets': {target: {'names': 'holds-target', **pins} for target in p.TARGETS}})
    (root / 'release/payload-scan-coverage-exceptions.json').write_text(RECEIPT_EXCEPTIONS)
    environment = {key: value for key, value in os.environ.items() if not key.startswith('GIT_')}
    for args in (['init', '-q'], ['add', '-A'], ['commit', '-q', '-m', 'the check and its pins']):
        subprocess.run(['git', '-C', str(root), '-c', 'user.name=test', '-c', 'user.email=test@example.invalid',
                        '-c', 'commit.gpgsign=false', '-c', 'core.hooksPath=/dev/null', *args],
                       check=True, capture_output=True, env=environment)
    return root / 'scripts' / CHECKOUT_RECEIPT_CHECK.name


@pytest.fixture(scope='session')
def receipt_checks(tmp_path_factory):
    """The check in three checkouts: one that pins what the synthetic reports state, one that pins
    another detector build, and one with a pin that no commit has set."""
    base = tmp_path_factory.mktemp('receipt-checkouts')
    return {'pinned': receipt_checkout(base / 'pinned', RECEIPT_PINS),
            'other_pin': receipt_checkout(base / 'other', {**RECEIPT_PINS, 'detector_current_sha256': '0' * 64}),
            'null_pin': receipt_checkout(base / 'null', {**RECEIPT_PINS, 'name_list_sha256': None})}


@pytest.fixture(autouse=True)
def committed_receipt_pins(monkeypatch, receipt_checks):
    monkeypatch.setattr(p, 'RECEIPT_CHECK', receipt_checks['pinned'])


def scan_receipt(c, **changes):
    """A passing upload receipt written beside the case, changed as asked ('privacy', 'credential' or
    'both': fields of a scan's part to set; 'top': fields of the document; 'payload': a function of
    the payload rows; 'document': the whole document). The receipt only: no full report, no pin."""
    try:
        commit = json.loads((c.root / 'request.json').read_text())['source_sha']
    except (OSError, ValueError, KeyError, TypeError):
        commit = '0' * 40
    rows = [{'name': name, 'bytes': (c.candidate / name).stat().st_size, 'sha256': p.digest(c.candidate / name)}
            for name in c.files if (c.candidate / name).is_file()]
    rows = changes.get('payload', lambda value: value)(rows)
    scan = {'status': 'PASS', 'release': True, 'generated_at': time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime()),
            'tool_commit': commit}
    privacy = {**scan, 'tool_sha256': '9a' * 32, 'report_sha256': '1b' * 32, 'exit_code': 0,
               'rules_sha256': RECEIPT_PINS['rules_sha256'], 'name_list_sha256': RECEIPT_PINS['name_list_sha256'],
               'non_published_list_sha256': RECEIPT_PINS['non_published_list_sha256'],
               'coverage_exceptions_sha256': hashlib.sha256(RECEIPT_EXCEPTIONS.encode()).hexdigest(), 'blocking_rules': True,
               'name_rules_all_blocking': True, 'no_exemptions': True, 'accepted_only_with_proof': True,
               'accepted_proof_tools': {'zq-prover-one': 2}}
    credential = {**scan, 'tool_sha256': '7c' * 32, 'report_sha256': '2c' * 32,
                  'detector_pinned_sha256': RECEIPT_PINS['detector_pinned_sha256'],
                  'detector_current_sha256': RECEIPT_PINS['detector_current_sha256'],
                  'nothing_refused': True, 'nothing_pending': True, 'nothing_unread_unaccepted': True}
    for part, own in ((privacy, 'privacy'), (credential, 'credential')):
        part.update(changes.get('both', {}))
        part.update(changes.get(own, {}))
    document = {'schema': 'release-upload-receipt/3', 'target': c.target, 'payload': rows, 'privacy': privacy,
                'credential': credential, **changes.get('top', {})}
    scan_receipt.count += 1
    path = c.root / f'upload-receipt-in-{scan_receipt.count}.json'
    dump(path, changes.get('document', document))
    return {'upload_receipt': path, 'max_age_hours': None}


# What the scans' full reports hold beyond a receipt: category and rule names and counts. Made up.
FULL_PRIVACY_REPORT = {'schema': 'release-payload-privacy-scan/report/3', 'status': 'PASS', 'release': True, 'exit_code': 0,
                       'rules': {'sha256': '8b' * 32, 'total': 4, 'blocking': 3, 'review_outside_paths': 0},
                       'hits': {'accepted': {'madeupclass': 2}},
                       'review_hits': {'review_madeupclass': 4}, 'accepted': {'count': 2, 'sha256': 'e5' * 32}}


scan_receipt.count = 0


@pytest.fixture(params=tuple(p.TARGETS))
def fixture(tmp_path, monkeypatch, request):
    class Case:
        pass
    c = Case()
    c.target = request.param
    c.files = p.publication_files(c.target)
    c.subjects = c.files[:2]
    c.root, c.candidate, c.meta = tmp_path, tmp_path / "candidate", tmp_path / "meta"
    c.candidate.mkdir()
    c.export = tmp_path / "connect-export"
    c.artifacts = c.export / "artifacts"
    c.artifacts.mkdir(parents=True)
    c.request = {"source_sha": "a" * 40, "source_tree": "b" * 40, "source_ref": "refs/heads/main",
        "run_id": 123, "run_attempt": 2, "inputs_sha256": "0" * 64, "connect_version": "1.2.3",
        "release_id": 456, "tag": "candidate-1.2.3", "target": c.target}
    c.native = native_selection("0.9.4")
    # Controlled package with the same public function signatures. This is not
    # the actual published Meta owner and is never release qualification.
    c.reader_calls = []
    package = c.meta / "installer/launchkit"
    package.mkdir(parents=True)
    (package / "templates").mkdir()
    (package / "__init__.py").write_text("")
    (package / "sdk_wheel_integrity.py").write_text("# controlled reader fixture\n")
    (package / "templates/install_candidate.py").write_text("# controlled reader fixture\n")
    (package / "runtime_selection.py").write_text('''from .runtime_candidates import selected_runtime_artifacts

def parse_candidate_selection(value):
    assert value['format'] == 'forge-meta-toolkit-runtime-selection-v1'
    return value
''')
    (package / "packaged_runtime.py").write_text('''from pathlib import Path

def relocate_selection(selection, paths, root):
    return {'selection': selection, 'paths': paths, 'root': root}
''')
    (package / "runtime_candidates.py").write_text('''from pathlib import Path
from types import SimpleNamespace
import hashlib,json

def selected_runtime_artifacts(target):
    root = target['root']
    for required in ('build-record.json','resource-build-record.json','gateway-verification.json'):
        assert (root / 'artifacts' / required).is_file(), required
    items = [x for x in sorted((root/'artifacts').rglob('*')) if x.is_file()
             and x.name not in ('RUNTIME-SELECTION.json','product.tar.gz.sha256')]
    return SimpleNamespace(origin='installed-gateway-assembly', evidence=tuple(
        SimpleNamespace(path=x,size=x.stat().st_size,sha256=hashlib.sha256(x.read_bytes()).hexdigest()) for x in items))
''')
    inputs = {"target": c.target, "native_release": c.native, "connect_version": c.request["connect_version"],
              "forge_reader_source_pins": {name: p.digest(c.meta / name) for name in p.FORGE_READERS}}
    dump(c.export / "inputs-selection.json", inputs)
    c.request["inputs_sha256"] = p.digest(c.export / "inputs-selection.json")
    for name in ["build-record.json", "resource-build-record.json", "gateway-verification.json", "source-manifest.json", "model-catalog.json"]:
        dump(c.artifacts / name, {"controlled_offline_fixture": name})
    for name in ["meta.whl", "toolkit.whl", "dependency.whl", "requirements.txt", "product.tar.gz", "product.tar.gz.sha256"]:
        (c.artifacts / name).write_bytes(("controlled bytes " + name).encode())
    dump(c.artifacts / "assembly.json", {"scope": p.SCOPE, "release_version": "1.2.3", "archive": "product.tar.gz"})
    c.producer_root = ("/home/runner/work/_temp" if c.target == "linux-x86_64" else "/Users/runner/work/_temp")
    producer_artifacts = c.producer_root + "/connect-candidate/artifacts"
    def selected(name):
        return {**pin(c.artifacts / name), "path": producer_artifacts + "/" + name}
    c.selection = {"format": "forge-meta-toolkit-runtime-selection-v1", "artifact_root": producer_artifacts,
        "target": c.target, "assembly": selected("assembly.json"), "source_manifest": selected("source-manifest.json"),
        "meta_wheel": selected("meta.whl"), "toolkit_wheel": selected("toolkit.whl"), "dependency_requirements": selected("requirements.txt"),
        "model_catalog": selected("model-catalog.json"), "dependency_wheels": [selected("dependency.whl")],
        "meta": {"controlled_fixture": True}, "toolkit": {"controlled_fixture": True}}
    dump(c.artifacts / "RUNTIME-SELECTION.json", c.selection)
    def pack():
        contents = [{"path": x.relative_to(c.export).as_posix(), "size": x.stat().st_size, "sha256": p.digest(x)}
                    for x in sorted(c.export.rglob('*')) if x.is_file() and x.name != 'CONTENTS.json']
        dump(c.export / "CONTENTS.json", {"schema_version": 1, "files": contents})
        archive = c.candidate / c.subjects[0]
        archive.unlink(missing_ok=True)
        p.archive_tree(c.export, archive)
        manifest = {"schema_version": 1, "kind": "connect-installed-candidate", "repository": p.PUBLICATION_REPO,
            "source_sha": c.request["source_sha"], "source_tree": c.request["source_tree"], "workflow_sha": c.request["source_sha"],
            "run_id": str(c.request["run_id"]), "run_attempt": str(c.request["run_attempt"]), "inputs_sha256": c.request["inputs_sha256"],
            "target": c.target, "scope": p.SCOPE, "connect_version": "1.2.3", "native_release": c.native,
            "archive": {"name": archive.name, "size": archive.stat().st_size, "sha256": p.digest(archive)},
            "runtime_selection_sha256": p.digest(c.artifacts / "RUNTIME-SELECTION.json"), "publication": "not_performed"}
        dump(c.candidate / c.subjects[1], manifest)
        (c.candidate / c.files[2]).write_bytes(b'UNTRUSTED CONTROLLED BUNDLE BYTES\n')
        dump(c.root / "request.json", c.request)
    c.pack = pack
    c.pack()
    c.commands, c.mutate_verified = [], None
    c.git_commands, c.git_directories = [], []
    c.git_status = ''
    c.git_refs = {'HEAD^{commit}': 'd' * 40, 'refs/tags/v0.9.4^{commit}': 'd' * 40}
    real_output = subprocess.check_output
    def output(argv, **kwargs):
        args = [str(x) for x in argv]
        if args[0] == 'git':
            command = args[3:]
            c.git_commands.append(command)
            c.git_directories.append(args[2])
            if command[:1] == ['rev-parse']:
                assert command[:2] == ['rev-parse', '--verify']
                return c.git_refs[command[2]] + '\n'
            if command[:1] == ['status']:
                return c.git_status
            raise AssertionError(args)
        if args[:3] == ['gh', 'attestation', 'verify']:
            c.commands.append((args, kwargs.get('env')))
            result = verified_statement(Path(args[3]), c.request)
            if c.mutate_verified:
                c.mutate_verified(result)
            return json.dumps(result).encode()
        if args[0] == p.sys.executable:
            c.reader_calls.append(args)
            return real_output(argv, **kwargs)
        raise AssertionError('offline control attempted external command: ' + repr(args))
    monkeypatch.setattr(p.subprocess, 'check_output', output)
    c.accept = lambda: p.accept_publication(c.root / 'request.json', c.candidate, c.meta, c.root / 'accepted',
                                            receipt=scan_receipt(c))
    return c


def test_acceptance_keeps_original_bytes_and_uses_scrubbed_verifier_and_reader(fixture, monkeypatch):
    c = fixture
    monkeypatch.setenv('GH_TOKEN', 'not-a-real-write-token')
    monkeypatch.setenv('PROVIDER_SECRET', 'not-a-real-provider-secret')
    before = {name: (c.candidate / name).read_bytes() for name in c.files}
    accepted = c.accept()
    assert accepted['publication'] == 'not_performed'
    assert accepted['selection'] == c.selection
    assert len(c.commands) == 2 and len(c.reader_calls) == 1
    for argv, env in c.commands:
        assert '--bundle' in argv and '--deny-self-hosted-runners' in argv
        assert '--source-digest' in argv and '--cert-identity' in argv
        assert 'GH_TOKEN' not in env and 'PROVIDER_SECRET' not in env
    assert {x['path'] for x in accepted['evidence']} >= {'artifacts/build-record.json', 'artifacts/resource-build-record.json',
        'artifacts/gateway-verification.json', 'artifacts/product.tar.gz', 'artifacts/product.tar.gz.sha256'}
    for name in c.files:
        assert (c.root / 'accepted/assets' / name).read_bytes() == before[name] == (c.candidate / name).read_bytes()
    assert not (c.root / 'accepted/publication.json').exists()
    assert ['rev-parse', '--verify', 'refs/tags/v0.9.4^{commit}'] in c.git_commands


def reselect_native(c):
    inputs = p.read_json(c.export / 'inputs-selection.json')
    inputs['native_release'] = c.native
    dump(c.export / 'inputs-selection.json', inputs)
    c.request['inputs_sha256'] = p.digest(c.export / 'inputs-selection.json')
    c.pack()


def select_forge_reader(c, tag='forge-v0.3.0'):
    c.forge = c.root / 'forge'
    shutil.copytree(c.meta, c.forge)
    selected = {'repository': 'lukacf/meerkat-meta', 'source_sha': 'e' * 40, 'tag': tag}
    inputs = p.read_json(c.export / 'inputs-selection.json')
    inputs['forge_reader_source'] = selected
    dump(c.export / 'inputs-selection.json', inputs)
    c.request['inputs_sha256'] = p.digest(c.export / 'inputs-selection.json')
    c.pack()
    c.git_refs['HEAD^{commit}'] = selected['source_sha']
    if tag:
        c.git_refs['refs/tags/' + tag + '^{commit}'] = selected['source_sha']
    c.accept = lambda: p.accept_publication(c.root / 'request.json', c.candidate, c.forge, c.root / 'accepted',
                                            receipt=scan_receipt(c))
    return inputs


@pytest.mark.parametrize('tag', ['', 'forge-v0.3.0'])
@pytest.mark.parametrize('core_version,meta_version', [('0.9.4', '0.9.4'), ('0.9.5', '0.9.5'), ('0.9.7', '0.9.6'), ('0.9.8', '0.9.7')])
def test_independent_forge_reader_acceptance_preserves_native_sdk_selection(fixture, tag, core_version, meta_version):
    c = fixture
    c.native = native_selection(core_version, meta_version)
    reselect_native(c)
    native, selection = copy.deepcopy(c.native), copy.deepcopy(c.selection)
    before = {name: (c.artifacts / name).read_bytes() for name in ('meta.whl', 'toolkit.whl', 'RUNTIME-SELECTION.json')}
    inputs = select_forge_reader(c, tag)
    # The native Meta tree need not contain the separately selected reader.
    shutil.rmtree(c.meta / 'installer')
    accepted = c.accept()
    assert accepted['selection'] == selection and c.native == native
    assert len(c.reader_calls) == 1 and str(c.forge / 'installer') in c.reader_calls[0]
    assert set(c.git_directories) == {str(c.forge)}
    refs = [args[2] for args in c.git_commands if args[:1] == ['rev-parse']]
    assert refs == ['HEAD^{commit}'] + (['refs/tags/' + tag + '^{commit}'] if tag else [])
    assert p.read_json(c.root / 'accepted/unpacked/inputs-selection.json') == inputs
    assert p.read_json(c.root / 'accepted/assets' / c.subjects[1])['native_release'] == native
    assert {name: (c.root / 'accepted/unpacked/artifacts' / name).read_bytes() for name in before} == before


def test_build_uses_independent_reader_for_both_dependency_batches(fixture, monkeypatch):
    c = fixture
    value = select_forge_reader(c)
    shutil.rmtree(c.meta / 'installer')
    value.update(files={name: {} for name in ('sdk.txt', 'sdk.whl', 'runtime.txt', 'runtime.whl')},
                 sdk_build={'requirements': 'sdk.txt', 'wheels': ['sdk.whl']},
                 dependencies={'requirements': 'runtime.txt', 'wheels': ['runtime.whl']})
    selected = c.root / 'producer-inputs.json'
    dump(selected, value)
    production = c.root / 'production'
    production.mkdir()
    monkeypatch.setenv('RUNNER_TEMP', str(production))
    monkeypatch.setenv('TMPDIR', str(c.root))
    monkeypatch.setattr(p, 'ROOT', c.root)
    monkeypatch.setattr(p, 'preflight', lambda **kwargs: (value, 'a' * 40, 'b' * 40))
    monkeypatch.setattr(p, 'input_path', lambda target: selected)
    def download(destination, row):
        destination.parent.mkdir(parents=True, exist_ok=True)
        destination.write_bytes(b'inert dependency bytes')
    monkeypatch.setattr(p, 'download', download)
    monkeypatch.setattr(p, 'inspect_prebuilt', lambda path: None)
    sources, calls = [], []
    def fetch(record, work, domain, selection):
        sources.append((domain, copy.deepcopy(selection)))
        return {'core': c.root / 'core', 'meta': c.meta, 'forge-reader': c.forge}[domain]
    monkeypatch.setattr(p, 'fetch_native_source', fetch)
    class StopBeforeInstallation(Exception):
        pass
    class Recorder:
        def __init__(self, work):
            pass
        def run(self, name, argv, **kwargs):
            calls.append((name, list(argv)))
            if name == 'dependencies-read':
                raise StopBeforeInstallation
            assert name == 'sdk_build-read'
            return {}, None
    monkeypatch.setattr(p, 'Recorder', Recorder)
    with pytest.raises(StopBeforeInstallation):
        p.build()
    assert sources == [('core', c.native['core']), ('meta', c.native['meta']),
                       ('forge-reader', value['forge_reader_source'])]
    assert [name for name, _ in calls] == ['sdk_build-read', 'dependencies-read']
    for _, argv in calls:
        assert argv[-2] == c.forge / 'installer'
        assert 'from launchkit.templates.install_candidate import _read_selected_dependency_wheels' in argv[3]
    assert not (production / 'connect-sdk-build-venv').exists()


@pytest.mark.parametrize('change', ['meta_commit', 'tag', 'dirty', 'cached', 'file'])
def test_independent_forge_reader_refuses_changed_checkout_before_owner(fixture, change):
    c = fixture
    select_forge_reader(c)
    if change == 'meta_commit':
        c.git_refs['HEAD^{commit}'] = c.native['meta']['source_sha']
    elif change == 'tag':
        c.git_refs['refs/tags/forge-v0.3.0^{commit}'] = 'f' * 40
    elif change == 'dirty':
        c.git_status = ' M installer/launchkit/runtime_selection.py'
    elif change == 'cached':
        (c.forge / 'installer/launchkit/cached.pyc').write_bytes(b'not executable')
    else:
        (c.forge / 'installer/launchkit/runtime_selection.py').write_text('# changed source\n')
    with pytest.raises(ValueError, match='exact clean selected Forge|cached Python code|Forge reader source differs'):
        c.accept()
    assert not c.reader_calls and not (c.root / 'accepted/acceptance.json').exists()


@pytest.mark.parametrize('change', ['null', 'missing_sha', 'extra', 'repository', 'mutable_sha', 'uppercase_sha',
                                  'null_tag', 'native_tag', 'unsafe_tag', 'pin_inventory', 'pin_hash'])
def test_explicit_invalid_forge_reader_never_falls_back_to_meta(fixture, change):
    c = fixture
    inputs = select_forge_reader(c)
    selected = inputs['forge_reader_source']
    if change == 'null':
        inputs['forge_reader_source'] = None
    elif change == 'missing_sha':
        selected.pop('source_sha')
    elif change == 'extra':
        selected['version'] = '0.3.0'
    elif change == 'repository':
        selected['repository'] = 'foreign/meerkat-meta'
    elif change == 'mutable_sha':
        selected['source_sha'] = 'release/0.9.5'
    elif change == 'uppercase_sha':
        selected['source_sha'] = 'E' * 40
    elif change == 'null_tag':
        selected['tag'] = None
    elif change == 'native_tag':
        selected['tag'] = 'v0.9.5'
    elif change == 'unsafe_tag':
        selected['tag'] = 'forge-v0.3.0^{commit}'
    elif change == 'pin_inventory':
        inputs['forge_reader_source_pins'].pop(next(iter(p.FORGE_READERS)))
    else:
        inputs['forge_reader_source_pins'][next(iter(p.FORGE_READERS))] = None
    for legacy in (False, True):
        with pytest.raises(ValueError, match='Forge reader'):
            p.checked_forge_reader_source(inputs, allow_legacy=legacy)
    dump(c.export / 'inputs-selection.json', inputs)
    c.request['inputs_sha256'] = p.digest(c.export / 'inputs-selection.json')
    c.pack()
    with pytest.raises(ValueError, match='Forge reader'):
        c.accept()
    assert not c.reader_calls and not c.git_commands


def test_reader_source_is_bound_by_original_attested_input_digest(fixture):
    c = fixture
    inputs = select_forge_reader(c)
    inputs['forge_reader_source']['source_sha'] = 'f' * 40
    dump(c.export / 'inputs-selection.json', inputs)
    c.pack()  # The independently selected original input digest is unchanged.
    with pytest.raises(ValueError, match='original input selection differs'):
        c.accept()
    assert not c.reader_calls and not c.git_commands


def test_reader_omission_is_legacy_acceptance_only(fixture):
    c = fixture
    inputs = p.read_json(c.export / 'inputs-selection.json')
    with pytest.raises(ValueError, match='explicit exact Forge reader source'):
        p.checked_forge_reader_source(inputs)
    assert c.accept()['selection'] == c.selection
    assert set(c.git_directories) == {str(c.meta)}
    assert ['rev-parse', '--verify', 'refs/tags/v0.9.4^{commit}'] in c.git_commands


@pytest.mark.parametrize('flags', [('--forge-reader-source',), ('--meta-source',),
                                  ('--forge-reader-source', '--meta-source')])
def test_publication_reader_cli_alias_is_explicit_and_exclusive(monkeypatch, flags):
    argv = ['connect-candidate.py', 'accept-publication', '--request', '/request',
            '--candidate', '/candidate', '--output', '/output']
    for flag in flags:
        argv += [flag, '/forge' if flag == '--forge-reader-source' else '/legacy']
    calls = []
    monkeypatch.setattr(p.sys, 'argv', argv)
    monkeypatch.setattr(p, 'accept_publication', lambda *args, receipt: calls.append(args))
    if len(flags) == 2:
        with pytest.raises(SystemExit) as error:
            p.main()
        assert error.value.code == 2 and not calls
    else:
        p.main()
        assert calls == [(Path('/request'), Path('/candidate'),
                          Path('/forge' if flags[0] == '--forge-reader-source' else '/legacy'), Path('/output'))]


@pytest.mark.parametrize('core_version,meta_version', [('0.9.4', '0.9.4'), ('0.9.5', '0.9.5'), ('0.9.7', '0.9.6'), ('0.9.8', '0.9.7')])
@pytest.mark.parametrize('tag_suffix', [None, '', '-rc.1'])
def test_exact_rc_selection_accepts_without_final_release_tag(fixture, core_version, meta_version, tag_suffix):
    c = fixture
    c.native = native_selection(core_version, meta_version)
    for selected in c.native.values():
        selected['tag'] = '' if tag_suffix is None else 'v' + selected['version'] + tag_suffix
    tag = c.native['meta']['tag']
    c.git_refs.pop('refs/tags/v0.9.4^{commit}')
    if tag:
        c.git_refs['refs/tags/' + tag + '^{commit}'] = c.native['meta']['source_sha']
    reselect_native(c)
    accepted = c.accept()
    assert accepted['publication'] == 'not_performed' and len(c.reader_calls) == 1
    refs = [args[2] for args in c.git_commands if args[:1] == ['rev-parse']]
    assert refs == ['HEAD^{commit}'] + (['refs/tags/' + tag + '^{commit}'] if tag else [])
    retained = p.read_json(c.root / 'accepted/assets' / c.subjects[1])
    assert retained['native_release'] == c.native


@pytest.mark.parametrize('core_version,meta_version', [('0.9.4', '0.9.4'), ('0.9.5', '0.9.5'), ('0.9.7', '0.9.6'), ('0.9.8', '0.9.7')])
@pytest.mark.parametrize('change', [None, 'sdk_version', 'requirement', 'missing_extra'])
def test_sdk_dependency_matches_selected_native_version(core_version, meta_version, change):
    value = {'native_release': native_selection(core_version, meta_version), 'sdks': {'meta': {'version': meta_version}}}
    sdk = {'optional-dependencies': {'native-host': ['meerkat-meta==' + meta_version]}}
    other = core_version if core_version != meta_version else ('0.9.5' if meta_version == '0.9.4' else '0.9.4')
    if change == 'sdk_version':
        value['sdks']['meta']['version'] = other
    elif change == 'requirement':
        sdk['optional-dependencies']['native-host'] = ['meerkat-meta==' + other]
    elif change == 'missing_extra':
        sdk['optional-dependencies'].pop('native-host')
    p.checked_native_releases(value['native_release'])
    if change is None:
        p.checked_sdk_native_dependency(value, sdk)
    else:
        with pytest.raises(ValueError, match='must match native release|matching native-host dependency'):
            p.checked_sdk_native_dependency(value, sdk)


@pytest.mark.parametrize('core_version,meta_version', [('0.9.4', '0.9.4'), ('0.9.5', '0.9.5'), ('0.9.7', '0.9.6'), ('0.9.8', '0.9.7')])
@pytest.mark.parametrize('git_suffix', ['', '.git'])
@pytest.mark.parametrize('change', [None, 'registry', 'core_version', 'meta_version', 'missing', 'extra',
                                  'foreign_commit', 'foreign_repository', 'foreign_host',
                                  'repository_suffix', 'repository_child', 'missing_commit', 'path'])
def test_native_family_matches_selected_versions_and_sources(core_version, meta_version, git_suffix, change):
    value = {'native_release': native_selection(core_version, meta_version),
             'native_family_packages': ['meerkat-core', 'meerkat-meta']}
    families = [{'name': name, 'version': value['native_release'][domain]['version'],
                 'source': 'git+https://github.com/' + value['native_release'][domain]['repository']
                           + git_suffix + '?rev=selected#' + value['native_release'][domain]['source_sha']}
                for name, domain in [('meerkat-core', 'core'), ('meerkat-meta', 'meta')]]
    other = '0.9.5' if meta_version == '0.9.4' else '0.9.4'
    if change == 'registry':
        for package in families:
            package['source'] = 'registry+https://github.com/rust-lang/crates.io-index'
    elif change in ('core_version', 'meta_version'):
        families[0 if change == 'core_version' else 1]['version'] = other
    elif change == 'missing':
        families.pop()
    elif change == 'extra':
        families.append({**families[0], 'name': 'meerkat-unselected'})
    elif change == 'foreign_commit':
        families[1]['source'] = families[1]['source'].replace('d' * 40, 'e' * 40)
    elif change == 'foreign_repository':
        families[1]['source'] = families[1]['source'].replace('lukacf/', 'foreign/')
    elif change == 'foreign_host':
        families[1]['source'] = families[1]['source'].replace('github.com/', 'github.com.example/')
    elif change == 'repository_suffix':
        families[1]['source'] = families[1]['source'].replace('?rev=', '-other?rev=')
    elif change == 'repository_child':
        families[1]['source'] = families[1]['source'].replace('?rev=', '/child?rev=')
    elif change == 'missing_commit':
        families[1]['source'] = families[1]['source'].split('#', 1)[0]
    elif change == 'path':
        families[1]['source'] = None
    p.checked_native_releases(value['native_release'])
    if change in (None, 'registry'):
        p.checked_native_family(families, value)
    else:
        with pytest.raises(ValueError, match='native family|native Git source|path-sourced native family'):
            p.checked_native_family(families, value)


@pytest.mark.parametrize('core_version,meta_version,tag', [
    ('0.9.4', '0.9.4', ''), ('0.9.4', '0.9.4', 'v0.9.4'),
    ('0.9.7', '0.9.6', ''), ('0.9.7', '0.9.6', 'v0.9.6'),
    ('0.9.8', '0.9.7', ''), ('0.9.8', '0.9.7', 'v0.9.7')])
def test_selected_meta_commit_mismatch_refuses_before_reader(fixture, core_version, meta_version, tag):
    c = fixture
    c.native = native_selection(core_version, meta_version)
    c.native['meta']['tag'] = tag
    reselect_native(c)
    c.git_refs['HEAD^{commit}'] = 'e' * 40
    with pytest.raises(ValueError, match='exact clean selected Forge checkout'):
        c.accept()
    assert not c.reader_calls


@pytest.mark.parametrize('core_version,meta_version', [('0.9.4', '0.9.4'), ('0.9.7', '0.9.6'), ('0.9.8', '0.9.7')])
def test_selected_meta_tag_must_resolve_to_exact_selected_commit(fixture, core_version, meta_version):
    c = fixture
    c.native = native_selection(core_version, meta_version)
    reselect_native(c)
    c.git_refs['refs/tags/v' + meta_version + '^{commit}'] = 'e' * 40
    with pytest.raises(ValueError, match='exact clean selected Forge checkout'):
        c.accept()
    assert not c.reader_calls


@pytest.mark.parametrize('version', ['0.9.7', '0.9.8', '0.10.0', '1.0.0'])
def test_unsupported_native_version_refuses_before_verifier_or_reader(fixture, version):
    c = fixture
    c.native = native_selection(version)
    # Match the attested input bytes, so this is a closed-version refusal.
    reselect_native(c)
    with pytest.raises(ValueError, match='supported Core/Meta release pair required'):
        c.accept()
    assert not c.commands and not c.reader_calls and not c.git_commands


@pytest.mark.parametrize('core_version,meta_version', [
    ('0.9.6', '0.9.6'), ('0.9.6', '0.9.7'), ('0.9.7', '0.9.7'),
    ('0.9.4', '0.9.6'), ('0.9.5', '0.9.6'), ('0.9.7', '0.9.4'), ('0.9.7', '0.9.5'),
    ('0.9.4', '0.9.7'), ('0.9.5', '0.9.7'),
    ('0.9.8', '0.9.4'), ('0.9.8', '0.9.5'), ('0.9.8', '0.9.6')])
def test_unqualified_native_pairs_refuse_before_verifier_or_reader(fixture, core_version, meta_version):
    c = fixture
    c.native = native_selection(core_version, meta_version)
    # Exact input bytes and valid selectors cannot authorize an unsupported pair.
    reselect_native(c)
    with pytest.raises(ValueError, match='supported Core/Meta release pair required'):
        c.accept()
    assert not c.commands and not c.reader_calls and not c.git_commands


@pytest.mark.parametrize('field,value', [
    ('source_sha', 'main'), ('source_sha', 'D' * 40), ('source_sha', None),
    ('version', '0.9.3'), ('version', '0.9.5'), ('version', '0.9.6'), ('repository', 'other/meerkat-meta'),
    ('tag', None), ('tag', '--upload-pack=foreign'), ('tag', 'v0.9.4^{commit}'),
    ('tag', 'v0.9.4..rc1'), ('tag', 'v0.9.4.lock'), ('tag', 'v0.9.4.'),
    ('tag', 'refs/tags/v0.9.4'), ('tag', 'v0.9.4 rc1')])
def test_malformed_native_selector_refuses_before_verifier_or_reader(fixture, field, value):
    c = fixture
    c.native['meta'][field] = value
    c.pack()
    with pytest.raises(ValueError):
        c.accept()
    assert not c.commands and not c.reader_calls and not c.git_commands


def test_native_commit_selection_must_match_original_input_bytes(fixture):
    c = fixture
    c.native['meta']['source_sha'] = 'e' * 40
    c.native['meta']['tag'] = ''
    c.pack()
    with pytest.raises(ValueError, match='original input selection differs'):
        c.accept()
    assert not c.reader_calls and not c.git_commands


@pytest.fixture
def local_native_fetch(tmp_path):
    """Actual Git fetch/peel/checkout, with only the remote URL replaced locally."""
    source, work = tmp_path / 'upstream', tmp_path / 'work'
    source.mkdir(); work.mkdir()
    def git(*args):
        return subprocess.check_output(['git', '-C', str(source), *args], text=True).strip()
    git('init', '-q')
    (source / 'payload').write_text('selected commit\n')
    git('add', 'payload')
    git('-c', 'user.name=Offline Control', '-c', 'user.email=offline@example.invalid',
        'commit', '-qm', 'selected source')
    selected = git('rev-parse', 'HEAD')
    (source / 'payload').write_text('unselected current branch\n')
    git('add', 'payload')
    git('-c', 'user.name=Offline Control', '-c', 'user.email=offline@example.invalid',
        'commit', '-qm', 'unselected source')
    other = git('rev-parse', 'HEAD')
    class LocalRecorder:
        def __init__(self):
            self.commands = []
            self.wrong_commit = False
        def run(self, name, argv, *, cwd=None):
            args = [str(x) for x in argv]
            self.commands.append(args)
            actual = [str(source) if x in ('https://github.com/lukacf/meerkat.git',
                                           'https://github.com/lukacf/meerkat-meta.git') else x for x in args]
            if self.wrong_commit and name in ('core-fetch', 'meta-fetch', 'forge-reader-fetch'):
                actual[-1] = other
            log = work / (name + '.log')
            with log.open('xb') as output:
                subprocess.run(actual, cwd=cwd, stdout=output, stderr=subprocess.STDOUT, check=True)
            return {}, log
    return git, work, selected, other, LocalRecorder()


@pytest.mark.parametrize('domain,tag', [('meta', ''), ('meta', 'v0.9.4'), ('meta', 'v0.9.4-rc.1'),
                                      ('meta', 'v0.9.6'), ('meta', 'v0.9.6-rc.1'),
                                      ('core', 'v0.9.7'), ('core', 'v0.9.7-rc.1'),
                                      ('forge-reader', ''), ('forge-reader', 'forge-v0.3.0')])
def test_source_fetch_uses_exact_commit_and_only_explicit_tag(local_native_fetch, domain, tag):
    git, work, selected, _, record = local_native_fetch
    if tag:
        git('-c', 'user.name=Offline Control', '-c', 'user.email=offline@example.invalid',
            'tag', '-a', tag, selected, '-m', 'selected tag')
    checkout = p.fetch_native_source(record, work, domain, {
        'repository': 'lukacf/meerkat' if domain == 'core' else 'lukacf/meerkat-meta',
        'source_sha': selected, 'tag': tag})
    assert subprocess.check_output(['git', '-C', str(checkout), 'rev-parse', 'HEAD'], text=True).strip() == selected
    assert (checkout / 'payload').read_text() == 'selected commit\n'
    assert record.commands[1][-1] == selected
    tag_fetches = [args for args in record.commands if args[:2] == ['git', 'fetch'] and args[-1].startswith('refs/tags/')]
    assert [args[-1] for args in tag_fetches] == (['refs/tags/' + tag + ':refs/tags/' + tag] if tag else [])


@pytest.mark.parametrize('mismatch', ['commit', 'tag'])
@pytest.mark.parametrize('domain', ['core', 'meta', 'forge-reader'])
def test_source_fetch_mismatch_refuses_before_checkout(local_native_fetch, mismatch, domain):
    git, work, selected, other, record = local_native_fetch
    tag = ''
    if mismatch == 'tag':
        tag = {'core': 'v0.9.7', 'meta': 'v0.9.4', 'forge-reader': 'forge-v0.3.0'}[domain]
        git('tag', tag, other)
    else:
        record.wrong_commit = True
    with pytest.raises(ValueError, match='fetched native commit differs|selected native tag points elsewhere'):
        p.fetch_native_source(record, work, domain, {
            'repository': 'lukacf/meerkat' if domain == 'core' else 'lukacf/meerkat-meta',
            'source_sha': selected, 'tag': tag})
    assert not any(args[:2] == ['git', 'checkout'] for args in record.commands)


@pytest.mark.parametrize('field,value', [('source_sha','e'*40), ('source_tree','f'*40), ('run_id',124),
    ('run_attempt',3), ('inputs_sha256','e'*64), ('connect_version','9.9.9')])
def test_changed_explicit_request_refuses_before_reader(fixture, field, value):
    c = fixture
    request = copy.deepcopy(c.request); request[field] = value; dump(c.root/'request.json',request)
    with pytest.raises(ValueError): c.accept()
    assert not c.reader_calls and not (c.root/'accepted/acceptance.json').exists()


@pytest.mark.parametrize('field', ['issuer','subjectAlternativeName','buildSignerURI','buildSignerDigest','sourceRepositoryURI',
    'sourceRepositoryDigest','sourceRepositoryRef','buildConfigURI','buildConfigDigest','runnerEnvironment','buildTrigger','runInvocationURI'])
def test_verified_certificate_must_match_exact_owner(fixture, field):
    c = fixture
    c.mutate_verified = lambda rows: rows[0]['verificationResult']['signature']['certificate'].__setitem__(field, 'foreign')
    with pytest.raises(ValueError, match='certificate'): c.accept()
    assert not c.reader_calls


def test_raw_bundle_is_never_trusted_when_verifier_fails(fixture, monkeypatch):
    c = fixture
    def fail(*args, **kwargs): raise subprocess.CalledProcessError(1, ['gh','attestation','verify'])
    monkeypatch.setattr(p, 'command_json', fail)
    with pytest.raises(subprocess.CalledProcessError): c.accept()
    assert not c.reader_calls and not (c.root/'accepted/acceptance.json').exists()


@pytest.mark.parametrize('part', ['subject','predicateType','verifiedTimestamps'])
def test_verified_statement_refusals(fixture, part):
    c=fixture
    def mutate(rows):
        result=rows[0]['verificationResult']
        if part=='verifiedTimestamps': result[part]=[]
        elif part=='subject': result['statement'][part][0]['digest']['sha256']='0'*64
        else: result['statement'][part]='unsupported'
    c.mutate_verified=mutate
    with pytest.raises(ValueError): c.accept()
    assert not c.reader_calls


def test_missing_indirect_record_is_refused_by_reader(fixture):
    c=fixture
    (c.artifacts/'gateway-verification.json').unlink();c.pack()
    with pytest.raises(subprocess.CalledProcessError):c.accept()
    assert len(c.reader_calls)==1 and not (c.root/'accepted/acceptance.json').exists()


@pytest.mark.parametrize('scope', ['source_built_development_gateway','unqualified_product_prefix_for_verification'])
def test_development_and_staging_are_not_publishable_preview(fixture,scope):
    c=fixture
    manifest=p.read_json(c.candidate/c.subjects[1]);manifest['scope']=scope;dump(c.candidate/c.subjects[1],manifest)
    with pytest.raises(ValueError,match='scope'):c.accept()
    assert not c.reader_calls


@pytest.mark.parametrize('bad', ['escape','duplicate','link','oversized'])
def test_archive_refusals_are_data_only(tmp_path,bad,monkeypatch):
    archive=tmp_path/'bad.tar.gz'
    with tarfile.open(archive,'w:gz') as tar:
        if bad=='link':
            item=tarfile.TarInfo('connect-export/link');item.type=tarfile.SYMTYPE;item.linkname='../../outside';tar.addfile(item)
        else:
            name='../escape' if bad=='escape' else 'connect-export/payload'
            for _ in range(2 if bad=='duplicate' else 1):
                item=tarfile.TarInfo(name);item.size=4;tar.addfile(item,io.BytesIO(b'data'))
    if bad=='oversized':monkeypatch.setattr(p,'MAX_FILE',3)
    with pytest.raises(ValueError):p.unpack_publication_candidate(archive,tmp_path/'unpacked')
    assert not (tmp_path/'escape').exists() and not (tmp_path/'outside').exists()


class ReleaseTransport:
    """Controlled API/public-GET responses, never a GitHub publication."""
    def __init__(self,c):
        self.c=c;self.files={};self.records={};self.uploads=[];self.draft=False;self.private=False;self.fail_upload=None;self.foreign_url=False
    def record(self,name,raw):
        self.files[name]=raw
        self.records[name]={'id':len(self.records)+100,'name':name,'size':len(raw),'state':'uploaded',
            'browser_download_url':f'https://github.com/{p.PUBLICATION_REPO}/releases/download/{self.c.request["tag"]}/{name}'}
    def api(self,endpoint,*,upload=None):
        if upload is not None:
            name=p.urllib.parse.parse_qs(p.urllib.parse.urlsplit(endpoint).query)["name"][0];self.uploads.append(name)
            if name==self.fail_upload:raise OSError('controlled upload failure')
            assert name not in self.records
            self.record(name,upload.read());return copy.deepcopy(self.records[name])
        if endpoint=='repos/'+p.PUBLICATION_REPO:return {'full_name':p.PUBLICATION_REPO,'private':self.private,'visibility':'private' if self.private else 'public'}
        if '/assets?' in endpoint:return copy.deepcopy(list(self.records.values()))
        if '/git/ref/tags/' in endpoint:return {'object':{'type':'commit','sha':self.c.request['source_sha'],'url':'controlled'}}
        return {'id':self.c.request['release_id'],'tag_name':self.c.request['tag'],'draft':self.draft}
    def download(self,path,row):
        name=Path(row['url']).name
        assert row['url']==self.records[name]['browser_download_url']
        path.parent.mkdir(parents=True,exist_ok=True);path.write_bytes(self.files[name])
        assert path.stat().st_size==row['size'] and p.digest(path)==row['sha256'], 'controlled public bytes differ'


def prepare_publish(c,monkeypatch):
    c.accept();accepted=c.root/'accepted';transport=ReleaseTransport(c)
    monkeypatch.setattr(p,'publication_api',transport.api);monkeypatch.setattr(p,'download',transport.download)
    # The write command must not enter either data-extraction or reader path.
    def forbidden(*args,**kwargs):raise AssertionError('write phase entered product/reader')
    monkeypatch.setattr(p,'unpack_publication_candidate',forbidden);monkeypatch.setattr(p,'read_publication_product',forbidden)
    def publish(receipt=None):return p.publish_accepted(accepted,p.digest(accepted/'acceptance.json'),c.root/'publication',receipt=receipt)
    return transport,publish


def test_acceptance_needs_a_passing_upload_receipt_for_the_exact_candidate_bytes(fixture, monkeypatch, receipt_checks):
    c = fixture
    with pytest.raises(ValueError, match='--upload-receipt'):
        p.accept_publication(c.root / 'request.json', c.candidate, c.meta, c.root / 'accepted')
    assert not (c.root / 'accepted').exists()
    for number, changes in enumerate([
            {'privacy': {'status': 'REFUSED'}}, {'credential': {'status': 'REFUSED'}}, {'privacy': {'status': 'clean'}},
            {'both': {'tool_commit': 'd' * 40}}, {'privacy': {'release': False}}, {'credential': {'release': False}},
            {'privacy': {'exit_code': 1}}, {'privacy': {'blocking_rules': False}}, {'privacy': {'name_list_sha256': '0' * 64}},
            {'privacy': {'rules_sha256': '0' * 64}}, {'privacy': {'coverage_exceptions_sha256': '0' * 64}},
            {'privacy': {'coverage_exceptions_sha256': None}},
            {'privacy': {'non_published_list_sha256': None}}, {'privacy': {'non_published_list_sha256': '0' * 64}},
            {'both': {'generated_at': '2020-01-01T00:00:00Z'}}, {'credential': {'nothing_refused': False}},
            {'credential': {'nothing_pending': False}}, {'credential': {'nothing_unread_unaccepted': False}},
            {'top': {'target': next(target for target in p.TARGETS if target != c.target)}},
            {'payload': lambda rows: rows[1:]}, {'payload': lambda rows: [dict(rows[0], sha256='0' * 64)] + rows[1:]},
            {'payload': lambda rows: [dict(rows[0], bytes=rows[0]['bytes'] + 1)] + rows[1:]},
            # A receipt holds exactly its fields: nothing of a full report rides along, and a full
            # report is no receipt.
            {'privacy': {'hits': FULL_PRIVACY_REPORT['hits']}}, {'privacy': {'rules': FULL_PRIVACY_REPORT['rules']}},
            {'credential': {'findings': {'total': 3, 'refused': 0}}}, {'top': {'label': 'candidate'}},
            {'top': {'privacy': FULL_PRIVACY_REPORT}}, {'document': FULL_PRIVACY_REPORT}]):
        output = c.root / f'refused-{number}'
        with pytest.raises(ValueError, match='upload receipt check refused'):
            p.accept_publication(c.root / 'request.json', c.candidate, c.meta, output, receipt=scan_receipt(c, **changes))
        assert not (output / 'acceptance.json').exists() and not c.reader_calls
    # The pins are the checkout's: the same receipt refuses where the commit pins another build, and
    # where a pin is not set. Nothing the caller passes can stand in for a pin.
    for name in ('other_pin', 'null_pin'):
        monkeypatch.setattr(p, 'RECEIPT_CHECK', receipt_checks[name])
        for extra in ({}, RECEIPT_PINS):
            with pytest.raises(ValueError, match='upload receipt check refused'):
                p.accept_publication(c.root / 'request.json', c.candidate, c.meta, c.root / f'{name}-{len(extra)}',
                                     receipt={**scan_receipt(c), **extra})
            assert not (c.root / f'{name}-{len(extra)}' / 'acceptance.json').exists() and not c.reader_calls
    monkeypatch.setattr(p, 'RECEIPT_CHECK', receipt_checks['pinned'])
    receipt = scan_receipt(c)
    p.accept_publication(c.root / 'request.json', c.candidate, c.meta, c.root / 'accepted', receipt=receipt)
    # The record carries the receipt, byte for byte, and nothing else of the scans: no report file,
    # no category name and no count anywhere in the directory that becomes a public artifact.
    assert (c.root / 'accepted' / p.UPLOAD_RECEIPT).read_bytes() == receipt['upload_receipt'].read_bytes()
    assert not (c.root / 'accepted/scan-reports').exists()
    names = [path.name for path in (c.root / 'accepted').rglob('*') if path.is_file()]
    assert names.count(p.UPLOAD_RECEIPT) == 1 and not [name for name in names if 'report' in name]
    for path in (c.root / 'accepted').iterdir():
        if path.is_file():
            assert b'/report/' not in path.read_bytes() and b'madeupclass' not in path.read_bytes(), path.name


@pytest.mark.parametrize('change', ['null_pin', 'other_pin', 'receipt_removed', 'receipt_replaced_refused',
                                    'receipt_for_other_bytes', 'stale', 'other_commit', 'proof_only', 'full_report',
                                    'reports_instead'])
def test_publication_checks_the_upload_receipt_before_any_api_call_or_write(fixture, monkeypatch, receipt_checks, change):
    c = fixture; t, publish = prepare_publish(c, monkeypatch)
    api_calls = []
    monkeypatch.setattr(p, 'publication_api', lambda *args, **kwargs: api_calls.append(args) or t.api(*args, **kwargs))
    carried = c.root / 'accepted' / p.UPLOAD_RECEIPT
    receipt = None
    if change in ('null_pin', 'other_pin'):
        # The writing checkout's commit pins another build, or has a pin that is not set. Digests the
        # caller passes are not pins.
        monkeypatch.setattr(p, 'RECEIPT_CHECK', receipt_checks[change])
        receipt = dict(RECEIPT_PINS)
    elif change == 'receipt_removed':
        carried.unlink()
    elif change == 'reports_instead':
        # An acceptance record of the earlier form: two full reports and no receipt.
        carried.unlink()
        dump(c.root / 'accepted/scan-reports/privacy-report.json', FULL_PRIVACY_REPORT)
        dump(c.root / 'accepted/scan-reports/credential-report.json', {'status': 'PASS'})
    else:
        changes = {'receipt_replaced_refused': {'credential': {'status': 'REFUSED'}},
                   'receipt_for_other_bytes': {'payload': lambda rows: [dict(rows[0], sha256='0' * 64)] + rows[1:]},
                   'stale': {'both': {'generated_at': '2020-01-01T00:00:00Z'}},
                   'other_commit': {'both': {'tool_commit': 'd' * 40}},
                   'proof_only': {'payload': lambda rows: [{'name': 'PROOF-ONLY-x', 'bytes': 1, 'sha256': '1' * 64}]},
                   'full_report': {'document': FULL_PRIVACY_REPORT}}[change]
        shutil.copyfile(scan_receipt(c, **changes)['upload_receipt'], carried)
    with pytest.raises(ValueError, match='upload receipt'):
        publish(receipt)
    assert not api_calls and not t.uploads
    record = p.read_json(c.root / 'publication/publication.json')
    assert record['status'] == 'refused_or_partial' and record['write_attempted'] == [] and 'scan_receipt' not in record


def test_publication_records_the_receipt_it_checked(fixture, monkeypatch, receipt_checks):
    c = fixture; t, publish = prepare_publish(c, monkeypatch)
    result = publish()
    assert result['scan_receipt'].startswith('UPLOAD-CHECK PASS files=%d ' % len(c.files))
    for name in c.files:
        assert '%s=%s:' % (name, p.digest(c.candidate / name)) in result['scan_receipt']
    assert 'release_commit=' + c.request['source_sha'] in result['scan_receipt']
    assert p.read_json(c.root / 'publication/publication.json')['scan_receipt'] == result['scan_receipt']
    # The record names the receipt, the two full reports it was written from (by digest), the target,
    # the pins file and the commit of the checkout the pins were read from.
    words = result['scan_receipt'].split()
    pins = receipt_checks['pinned'].parent.parent / 'release/upload-receipt-pins.json'
    assert 'target=' + c.target in words and 'pins=' + p.digest(pins) in words
    assert 'receipt=' + p.digest(c.root / 'accepted' / p.UPLOAD_RECEIPT) in words
    assert 'privacy_report=' + '1b' * 32 in words and 'credential_report=' + '2c' * 32 in words
    assert any(re.fullmatch(r'pins_commit=[0-9a-f]{40}', word) for word in words)
    assert (c.root / 'publication' / p.UPLOAD_RECEIPT).read_bytes() == (c.root / 'accepted' / p.UPLOAD_RECEIPT).read_bytes()


def test_acceptance_checks_the_receipt_for_the_attestation_bundle_too(fixture, monkeypatch):
    c = fixture
    bundle = c.files[2]
    assert bundle.endswith('-provenance.jsonl') and (c.candidate / bundle).is_file()
    # A receipt that does not name the bundle, and receipts that name it with another digest or size.
    for number, payload in enumerate([
            lambda rows: [row for row in rows if row['name'] != bundle],
            lambda rows: [dict(row, sha256='0' * 64) if row['name'] == bundle else row for row in rows],
            lambda rows: [dict(row, bytes=row['bytes'] + 1) if row['name'] == bundle else row for row in rows]]):
        output = c.root / f'bundle-refused-{number}'
        with pytest.raises(ValueError, match='upload receipt check refused'):
            p.accept_publication(c.root / 'request.json', c.candidate, c.meta, output, receipt=scan_receipt(c, payload=payload))
        assert not (output / 'acceptance.json').exists() and not c.reader_calls
    # The files the check is given are the three retained copies, the bundle among them.
    commands = []
    real_run = subprocess.run
    def run(command, **kwargs):
        commands.append(command)
        return real_run(command, **kwargs)
    monkeypatch.setattr(p.subprocess, 'run', run)
    p.accept_publication(c.root / 'request.json', c.candidate, c.meta, c.root / 'accepted', receipt=scan_receipt(c))
    checks = [command for command in commands if str(p.RECEIPT_CHECK) in command]
    assert len(checks) == 1
    assert checks[0][-len(c.files):] == [str(c.root / 'accepted/assets' / name) for name in c.files]


def test_no_caller_gives_the_receipt_check_a_pin_or_a_full_report(fixture, monkeypatch):
    c = fixture
    # The candidate tool runs the check of its own checkout, and holds no pin of its own.
    assert CHECKOUT_RECEIPT_CHECK == p.ROOT / 'scripts/release-upload-receipt-check.py'
    assert not hasattr(p, 'RECEIPT_PINS') and not hasattr(p, 'SCAN_REPORTS')
    arguments = type('Arguments', (), {'upload_receipt': 1, 'receipt_max_age_hours': 3})()
    assert p.receipt_inputs(arguments) == {'upload_receipt': 1, 'max_age_hours': 3}
    # The command it runs: the target, the receipt, the release commit, the release's version and the
    # files. Digests in the receipt inputs are not passed on.
    commands = []
    real_run = subprocess.run
    def run(command, **kwargs):
        commands.append(command)
        return real_run(command, **kwargs)
    monkeypatch.setattr(p.subprocess, 'run', run)
    p.accept_publication(c.root / 'request.json', c.candidate, c.meta, c.root / 'accepted',
                         receipt={**scan_receipt(c), **RECEIPT_PINS})
    checks = [command for command in commands if str(p.RECEIPT_CHECK) in command]
    assert len(checks) == 1
    options = [word for word in checks[0] if word.startswith('--')]
    assert options == ['--target', '--receipt', '--release-commit', '--version']
    assert checks[0][checks[0].index('--target') + 1] == c.target
    # The version every file name is held to is the request's: the release the candidate is for.
    assert checks[0][checks[0].index('--version') + 1] == c.request['connect_version']
    assert checks[0][checks[0].index('--receipt') + 1] == str(c.root / 'accepted' / p.UPLOAD_RECEIPT)
    assert not any(value in checks[0] for value in RECEIPT_PINS.values())
    # Its command line has no option for a pin or for a full report.
    for option in ('--name-list-sha256', '--rules-sha256', '--detector-pinned-sha256', '--detector-current-sha256',
                   '--non-published-list-sha256', '--pins', '--privacy-report', '--credential-report'):
        monkeypatch.setattr(p.sys, 'argv', ['connect-candidate.py', 'publish-accepted', '--accepted', '/accepted',
                                            '--acceptance-sha256', '0' * 64, '--output', '/output', option, '1' * 64])
        monkeypatch.setattr(p, 'publish_accepted', lambda *args, **kwargs: pytest.fail('publication was reached'))
        with pytest.raises(SystemExit) as error:
            p.main()
        assert error.value.code == 2


def assert_public_fragment(c, result):
    fragment_path = c.root / 'publication/forge-fragment.json'
    fragment = p.read_json(fragment_path)
    assert set(fragment) == {'files', 'selection', 'paths'}
    assert fragment == {key: result[key] for key in fragment}
    projected = fragment['selection']
    # Reattach the original locators to prove every other value and row order
    # survives, including the complete wheel source-payload/metadata inventory.
    restored = copy.deepcopy(projected)
    assert 'artifact_root' not in restored
    restored['artifact_root'] = c.selection['artifact_root']
    for key, original in c.selection.items():
        if isinstance(original, dict) and 'path' in original:
            assert 'path' not in restored[key]
            restored[key]['path'] = original['path']
    assert len(restored['dependency_wheels']) == len(c.selection['dependency_wheels'])
    for wheel, original in zip(restored['dependency_wheels'], c.selection['dependency_wheels']):
        assert 'path' not in wheel
        wheel['path'] = original['path']
    assert restored == c.selection
    assert c.producer_root.encode() not in fragment_path.read_bytes()
    for key, value in fragment['paths'].items():
        for name in value if key == 'dependency_wheels' else [value]:
            assert p.relative(name) == name
    attribution = p.read_json(c.root / 'publication/forge-fragment-attribution.json')
    assert attribution == result['selection_projection']
    assert attribution['rule'] == 'original-selection-minus-physical-locators-v1'
    assert attribution['fragment'] == {'path': fragment_path.name,
        'size': fragment_path.stat().st_size, 'sha256': p.digest(fragment_path)}
    original = attribution['original_selection']
    assert original == fragment['files']['artifacts/RUNTIME-SELECTION.json']
    assert original['member'] == 'connect-export/artifacts/RUNTIME-SELECTION.json'
    archive = c.root / 'publication/assets' / c.subjects[0]
    assert original['archive_size'] == archive.stat().st_size
    assert original['archive_sha256'] == p.digest(archive)
    with tarfile.open(archive, 'r:gz') as package:
        raw = package.extractfile(original['member']).read()
    assert raw == (c.artifacts / 'RUNTIME-SELECTION.json').read_bytes()
    assert original['size'] == len(raw) and original['sha256'] == hashlib.sha256(raw).hexdigest()
    assert json.loads(raw) == c.selection


def test_publication_uploads_only_accepted_bytes_then_emits_original_inventory(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch)
    result=publish()
    assert t.uploads==list(c.files)
    assert result['status']=='verified_public_inventory'
    assert_public_fragment(c, result)
    assert result['files']['artifacts/product.tar.gz']['member']=='connect-export/artifacts/product.tar.gz'
    assert all(v['url'].endswith('/'+c.subjects[0]) for v in result['files'].values())
    for name in c.files:assert t.files[name]==(c.candidate/name).read_bytes()
    assert len(c.reader_calls)==1


def test_equal_existing_assets_require_public_readback_without_reupload(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch)
    for name in c.files:t.record(name,(c.candidate/name).read_bytes())
    assert publish()['status']=='verified_public_inventory' and not t.uploads


def test_conflicting_existing_asset_is_not_overwritten(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch)
    name=c.subjects[0];raw=(c.candidate/name).read_bytes();t.record(name,b'!'*len(raw))
    with pytest.raises(AssertionError,match='bytes differ'):publish()
    assert not t.uploads and t.files[name]==b'!'*len(raw)
    assert p.read_json(c.root/'publication/publication.json')['status']=='refused_or_partial'


@pytest.mark.parametrize('conflict_index', [1, 2])
def test_later_existing_conflict_refuses_before_any_missing_asset_upload(fixture, monkeypatch, conflict_index):
    c=fixture;conflict=c.files[conflict_index];t,publish=prepare_publish(c,monkeypatch)
    raw=(c.candidate/conflict).read_bytes()
    t.record(conflict,b'!'*len(raw))
    with pytest.raises(AssertionError,match='bytes differ'):
        publish()
    assert not t.uploads
    assert set(t.files)=={conflict}
    assert p.read_json(c.root/'publication/publication.json')['write_attempted']==[]


def test_failed_second_upload_preserves_partial_evidence_without_final_inventory(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch);t.fail_upload=c.subjects[1]
    with pytest.raises(OSError):publish()
    report=p.read_json(c.root/'publication/publication.json')
    assert report['verified_assets']==[c.subjects[0]] and report['write_attempted']==list(c.subjects)
    assert report['status']=='refused_or_partial' and 'files' not in report


def test_draft_release_is_not_treated_as_publication(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch);t.draft=True
    with pytest.raises(ValueError,match='has not published'):publish()
    assert not t.uploads


@pytest.mark.parametrize('mutation', ['scope', 'target', 'artifact_hash', 'wheel_identity', 'dependency'])
def test_selected_acceptance_hash_and_allowlist_are_rechecked(fixture,monkeypatch,mutation):
    c=fixture;t,publish=prepare_publish(c,monkeypatch)
    directory=c.root/'accepted';selected=p.digest(directory/'acceptance.json')
    value=p.read_json(directory/'acceptance.json')
    if mutation == 'scope':
        value['scope'] = 'different'
    elif mutation == 'target':
        value['selection']['target'] = next(target for target in p.TARGETS if target != c.target)
    elif mutation == 'artifact_hash':
        value['selection']['assembly']['sha256'] = 'f' * 64
    elif mutation == 'wheel_identity':
        value['selection']['meta']['version'] = '99.0.0'
    else:
        value['selection']['dependency_wheels'] = []
    dump(directory/'acceptance.json',value)
    with pytest.raises(ValueError,match='acceptance changed'):
        p.publish_accepted(directory,selected,c.root/'publication')
    assert not t.uploads and not (c.root/'publication').exists()


def test_changed_allowlist_never_reaches_upload(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch)
    (c.root/'accepted/assets'/c.subjects[0]).write_bytes(b'changed')
    with pytest.raises(ValueError):publish()
    assert not t.uploads


def test_foreign_public_url_refuses(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch)
    for name in c.files:t.record(name,(c.candidate/name).read_bytes())
    t.records[c.subjects[0]]['browser_download_url']='https://example.invalid/foreign'
    with pytest.raises(ValueError,match='public asset identity'):publish()
    assert not t.uploads


def test_changed_new_asset_after_readback_prevents_final_inventory(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch)
    original_download=t.download
    def read(path,row):
        original_download(path,row)
        if path.name==c.files[-1]:t.records[c.subjects[0]]['id']+=1000
    monkeypatch.setattr(p,'download',read)
    with pytest.raises(ValueError,match='publication changed'):publish()
    report=p.read_json(c.root/'publication/publication.json')
    assert report['status']=='refused_or_partial' and 'files' not in report


def test_foreign_tag_commit_cannot_receive_assets(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch)
    old=t.api
    def api(endpoint,**kwargs):
        result=old(endpoint,**kwargs)
        if '/git/ref/tags/' in endpoint:result['object']['sha']='e'*40
        return result
    monkeypatch.setattr(p,'publication_api',api)
    with pytest.raises(ValueError,match='tag does not select'):publish()
    assert not t.uploads


def test_second_attested_subject_is_independently_required(fixture):
    c=fixture
    def mutate(rows):
        result=rows[0]['verificationResult']
        if result['statement']['subject'][0]['name']==c.subjects[1]:
            result['signature']['certificate']['runInvocationURI']='foreign-attempt'
    c.mutate_verified=mutate
    with pytest.raises(ValueError,match='certificate'):c.accept()
    assert len(c.commands)==2 and not c.reader_calls


def test_archive_link_parent_cannot_be_reinterpreted_as_directory(tmp_path):
    archive=tmp_path/'bad.tar.gz'
    with tarfile.open(archive,'w:gz') as tar:
        item=tarfile.TarInfo('connect-export/artifacts');item.type=tarfile.SYMTYPE;item.linkname='elsewhere';tar.addfile(item)
        item=tarfile.TarInfo('connect-export/artifacts/payload');item.size=4;tar.addfile(item,io.BytesIO(b'data'))
    with pytest.raises(ValueError,match='traverses an archive leaf'):
        p.unpack_publication_candidate(archive,tmp_path/'unpacked')


def test_existing_fetch_binding_retains_target_and_source_fields_and_archive_contract(fixture):
    c=fixture;manifest=p.read_json(c.candidate/c.subjects[1]);payload=c.candidate/c.subjects[0]
    keys=('source_sha','workflow_sha','run_id','run_attempt','inputs_sha256','target')
    expected={k:manifest[k] for k in keys}
    p.checked_candidate_binding(manifest,payload,expected)
    for key in keys:
        changed=copy.deepcopy(manifest);changed[key]='different'
        with pytest.raises(ValueError,match='candidate binding differs'):
            p.checked_candidate_binding(changed,payload,expected)
    changed=copy.deepcopy(manifest);changed['archive']['sha256']='0'*64
    with pytest.raises(ValueError,match='candidate closure differs'):
        p.checked_candidate_binding(changed,payload,expected)


def test_public_read_counters_do_not_change_asset_identity(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch)
    original_download=t.download
    def read(path,row):
        original_download(path,row)
        t.records[path.name]['download_count']=t.records[path.name].get('download_count',0)+1
    monkeypatch.setattr(p,'download',read)
    assert publish()['status']=='verified_public_inventory'
    assert all(row['download_count']==1 for row in t.records.values())


def test_acceptance_replacement_after_read_cannot_change_parsed_selection(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch)
    original_copy=p.copy_publication_input
    observed=[]
    def copy_input(path,output,maximum):
        row=original_copy(path,output,maximum)
        if path.name=='acceptance.json':
            path.write_bytes(b'{"scope":"substituted after selected read"}')
            observed.append(path)
        return row
    monkeypatch.setattr(p,'copy_publication_input',copy_input)
    result=publish()
    assert len(observed)==1 and result['status']=='verified_public_inventory'
    assert_public_fragment(c, result)
    assert p.read_json(observed[0])=={'scope':'substituted after selected read'}


def test_original_assets_replaced_after_retention_never_enter_upload(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch)
    before={name:(c.candidate/name).read_bytes() for name in c.files}
    observed=[]
    def replace(rows):
        if len(c.commands)==3:
            for name in c.files:
                path=c.root/'accepted/assets'/name
                path.unlink();path.write_bytes(b'unaccepted original handoff replacement')
                observed.append(path)
    c.mutate_verified=replace
    assert publish()['status']=='verified_public_inventory'
    assert len(observed)==3 and t.files==before
    assert all(path.read_bytes()==b'unaccepted original handoff replacement' for path in observed)
    assert all('GH_TOKEN' not in env for _,env in c.commands)


def test_replaced_private_path_cannot_substitute_entered_upload_stream(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch)
    original_api=t.api
    mutated=[]
    def api(endpoint,*,upload=None):
        if upload is not None and not mutated:
            path=c.root/'publication/assets'/c.subjects[0]
            path.unlink();path.write_bytes(b'unaccepted replacement before gh stdin read')
            mutated.append(path)
        return original_api(endpoint,upload=upload)
    monkeypatch.setattr(p,'publication_api',api)
    with pytest.raises(ValueError):publish()
    assert len(mutated)==1 and t.uploads==list(c.files)
    for name in c.files:assert t.files[name]==(c.candidate/name).read_bytes()
    report=p.read_json(c.root/'publication/publication.json')
    assert report['status']=='refused_or_partial' and 'files' not in report


def test_upload_command_reads_retained_stdin_not_a_path(tmp_path,monkeypatch):
    commands=[]
    def command(argv,*,stdin=None,**bound):
        commands.append((argv,stdin.read() if stdin is not None else None,bound))
        return {'controlled':'upload receipt'}
    monkeypatch.setattr(p,'command_json',command)
    with p.tempfile.TemporaryFile('w+b',dir=tmp_path) as retained:
        retained.write(b'exact accepted bytes');retained.seek(0)
        result=p.publication_api('https://uploads.github.com/controlled',upload=retained)
        assert not retained.closed
    assert result=={'controlled':'upload receipt'}
    assert commands[0][0][-2:]==['--input','-']
    assert commands[0][1]==b'exact accepted bytes'
    # The upload of a file has a time bound of its own, longer than the one a read keeps.
    p.publication_api('repos/controlled')
    assert commands[0][2]=={'timeout':p.UPLOAD_SECONDS} and p.UPLOAD_SECONDS>=3600 and commands[1][1:]==(None,{})


def test_private_repository_refuses_before_any_upload(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch);t.private=True
    with pytest.raises(ValueError,match='repository is not public'):publish()
    assert not t.uploads and not t.draft
    assert p.read_json(c.root/'publication/publication.json')['status']=='refused_or_partial'


def test_repository_becoming_private_stops_further_uploads(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch)
    original_download=t.download
    def read(path,row):
        original_download(path,row)
        t.private=True
    monkeypatch.setattr(p,'download',read)
    with pytest.raises(ValueError,match='repository is not public'):publish()
    assert t.uploads==[c.subjects[0]]
    report=p.read_json(c.root/'publication/publication.json')
    assert report['status']=='refused_or_partial' and report['verified_assets']==[c.subjects[0]]
    assert 'files' not in report


@pytest.mark.parametrize('target,system,machine', [
    ('linux-x86_64', 'Linux', 'x86_64'), ('macos-arm64', 'Darwin', 'arm64')])
def test_native_target_requires_matching_host(monkeypatch, target, system, machine):
    monkeypatch.setattr(p.platform, 'system', lambda: system)
    monkeypatch.setattr(p.platform, 'machine', lambda: machine)
    p.require_native_host(target)
    other = next(name for name in p.TARGETS if name != target)
    with pytest.raises(ValueError, match='native build host'):
        p.require_native_host(other)
    monkeypatch.setattr(p.platform, 'machine', lambda: 'unsupported')
    with pytest.raises(ValueError, match='native build host'):
        p.require_native_host(target)


@pytest.mark.parametrize('target', [None, '', 'macos-x86_64', 'linux-arm64', '../macos-arm64'])
def test_no_implicit_or_unsupported_target(monkeypatch, target):
    if target is None:
        monkeypatch.delenv('CONNECT_TARGET', raising=False)
    else:
        monkeypatch.setenv('CONNECT_TARGET', target)
    with pytest.raises(ValueError, match='explicit supported target'):
        p.selected_target()


def test_data_only_preflight_does_not_require_target_host(tmp_path, monkeypatch):
    selected = tmp_path / 'inputs.json'
    dump(selected, {'unresolved': None})
    for key, value in {'GITHUB_ACTIONS': 'true', 'GITHUB_EVENT_NAME': 'workflow_dispatch',
        'RUNNER_ENVIRONMENT': 'github-hosted', 'GITHUB_REPOSITORY': p.PUBLICATION_REPO,
        'CONNECT_TARGET': 'macos-arm64', 'CONNECT_INPUTS_SHA256': p.digest(selected)}.items():
        monkeypatch.setenv(key, value)
    monkeypatch.setattr(p.platform, 'system', lambda: 'Linux')
    monkeypatch.setattr(p.platform, 'machine', lambda: 'x86_64')
    monkeypatch.setattr(p, 'source_state', lambda: ('a'*40, 'b'*40))
    monkeypatch.setattr(p, 'input_path', lambda target: selected)
    with pytest.raises(ValueError, match='native build host'):
        p.preflight(native=True)
    # It still checks the exact source/input selection; it merely does not
    # execute a Mac product on the separate Linux attestation job.
    with pytest.raises(ValueError, match='unbound release inputs'):
        p.preflight()


@pytest.mark.parametrize('kind', ['json', 'tar.gz', 'whl'])
def test_retained_metadata_paths_are_preserved_without_opening_them(tmp_path, monkeypatch, kind):
    import zipfile
    referenced = tmp_path / 'historical-source-file'
    referenced.write_bytes(b'not an input or JSON document')
    raw = json.dumps({'source': '/Users/original-owner/source/build.rs',
        'argv': ['/var/tmp/original-build/tool', str(referenced)],
        'example': 'unix:///tmp/example-docker.sock'}).encode()
    artifact = tmp_path / ('original.' + kind)
    if kind == 'tar.gz':
        with tarfile.open(artifact, 'w:gz') as archive:
            member = tarfile.TarInfo('selected/provenance/build.json')
            member.size = len(raw); archive.addfile(member, io.BytesIO(raw))
    elif kind == 'whl':
        with zipfile.ZipFile(artifact, 'w') as archive:
            archive.writestr('original.dist-info/direct_url.json', raw)
    else:
        artifact.write_bytes(raw)
    before = artifact.read_bytes()
    original_open = Path.open
    def guarded_open(path, *args, **kwargs):
        assert path != referenced, 'descriptive producer path became an input locator'
        return original_open(path, *args, **kwargs)
    monkeypatch.setattr(Path, 'open', guarded_open)
    p.inspect_prebuilt(artifact)
    assert artifact.read_bytes() == before


@pytest.mark.parametrize('kind', ['json', 'tar.gz', 'whl'])
@pytest.mark.parametrize('raw', [b'{invalid', b'{"source":"first","source":"second"}'])
def test_retained_metadata_still_requires_valid_unambiguous_json(tmp_path, kind, raw):
    import zipfile
    artifact = tmp_path / ('original.' + kind)
    if kind == 'tar.gz':
        with tarfile.open(artifact, 'w:gz') as archive:
            member = tarfile.TarInfo('selected/provenance/build.json')
            member.size = len(raw); archive.addfile(member, io.BytesIO(raw))
    elif kind == 'whl':
        with zipfile.ZipFile(artifact, 'w') as archive:
            archive.writestr('original.dist-info/direct_url.json', raw)
    else:
        artifact.write_bytes(raw)
    with pytest.raises(ValueError):
        p.inspect_prebuilt(artifact)


def test_target_asset_names_cannot_collide():
    linux, mac = (set(p.publication_files(target)) for target in p.TARGETS)
    assert len(linux) == len(mac) == 3 and linux.isdisjoint(mac)
    assert p.input_path('linux-x86_64') != p.input_path('macos-arm64')


def test_other_target_assets_survive_same_release_publication(fixture, monkeypatch):
    c = fixture
    transport, publish = prepare_publish(c, monkeypatch)
    other = next(target for target in p.TARGETS if target != c.target)
    for name in p.publication_files(other):
        transport.record(name, ('other original target ' + name).encode())
    prior = copy.deepcopy(transport.records)
    prior_bytes = copy.deepcopy(transport.files)
    assert publish()['status'] == 'verified_public_inventory'
    assert transport.uploads == list(c.files)
    assert all(transport.records[name] == record for name, record in prior.items())
    assert all(transport.files[name] == raw for name, raw in prior_bytes.items())


@pytest.mark.parametrize('part', ['manifest', 'inputs', 'selection'])
def test_foreign_target_binding_refuses_before_owner_reader(fixture, part):
    c = fixture
    other = next(target for target in p.TARGETS if target != c.target)
    if part == 'manifest':
        path = c.candidate / c.subjects[1]
        value = p.read_json(path); value['target'] = other; dump(path, value)
    elif part == 'inputs':
        path = c.export / 'inputs-selection.json'
        value = p.read_json(path); value['target'] = other; dump(path, value)
        c.request['inputs_sha256'] = p.digest(path); c.pack()
    else:
        c.selection['target'] = other
        dump(c.artifacts / 'RUNTIME-SELECTION.json', c.selection); c.pack()
    with pytest.raises(ValueError):
        c.accept()
    assert not c.reader_calls and not (c.root / 'accepted/acceptance.json').exists()


def test_historical_paths_in_attested_closure_are_preserved_for_owner_reader(fixture):
    c = fixture
    record = c.artifacts / 'build-record.json'
    dump(record, {'source': '/Users/original-owner/source/build.rs',
                  'output': '/private/var/tmp/original-owner/toolkit_gateway'})
    before = record.read_bytes()
    c.pack()
    accepted = c.accept()
    assert accepted['publication'] == 'not_performed'
    assert (c.root / 'accepted/unpacked/artifacts/build-record.json').read_bytes() == before
    assert len(c.commands) == 2 and len(c.reader_calls) == 1


@pytest.mark.parametrize("url", [
    "https://registry.npmjs.org/npm/-/npm-12.2.0.tgz",
    "https://bootstrap.pypa.io/pip/zipapp/pip-26.2.1.pyz",
])
def test_existing_resource_official_versioned_urls_are_selectable(url):
    p.public_url(url)


@pytest.mark.parametrize("url", [
    "http://registry.npmjs.org/npm/-/npm-12.2.0.tgz",
    "https://user:secret@registry.npmjs.org/npm/-/npm-12.2.0.tgz",
    "https://registry.npmjs.org/npm/-/npm-12.2.0.tgz?token=secret",
    "https://registry.npmjs.org/npm/-/npm-12.2.0.tgz#fragment",
    "https://registry.npmjs.org:444/npm/-/npm-12.2.0.tgz",
    "https://registry.npmjs.org.evil.invalid/npm/-/npm-12.2.0.tgz",
    "https://registry.npmjs.org/other/-/other-12.2.0.tgz",
    "https://registry.npmjs.org/npm/-/npm-latest.tgz",
    "https://registry.npmjs.org/npm/-/npm-12.2.1.tgz",
    "https://registry.npmjs.org/npm/-/%6epm-12.2.0.tgz",
    "https://registry.npmjs.org/npm/else/../-/npm-12.2.0.tgz",
    "http://bootstrap.pypa.io/pip/zipapp/pip-26.2.1.pyz",
    "https://user:secret@bootstrap.pypa.io/pip/zipapp/pip-26.2.1.pyz",
    "https://bootstrap.pypa.io/pip/zipapp/pip-26.2.1.pyz?token=secret",
    "https://bootstrap.pypa.io/pip/zipapp/pip-26.2.1.pyz#fragment",
    "https://bootstrap.pypa.io:444/pip/zipapp/pip-26.2.1.pyz",
    "https://bootstrap.pypa.io.evil.invalid/pip/zipapp/pip-26.2.1.pyz",
    "https://bootstrap.pypa.io/pip/pip.pyz",
    "https://bootstrap.pypa.io/get-pip.py",
    "https://bootstrap.pypa.io/pip/zipapp/pip-26.2.2.pyz",
    "https://bootstrap.pypa.io/pip/zipapp/%70ip-26.2.1.pyz",
    "https://bootstrap.pypa.io/pip/else/../zipapp/pip-26.2.1.pyz",
])
def test_resource_url_additions_do_not_admit_other_locators(url):
    with pytest.raises(ValueError):
        p.public_url(url)


@pytest.mark.parametrize("url", [
    "https://registry.npmjs.org/npm/-/npm-12.2.0.tgz",
    "https://bootstrap.pypa.io/pip/zipapp/pip-26.2.1.pyz",
])
@pytest.mark.parametrize("received", [b"selected bytes", b"short", b"selected bytes extra", b"different data"])
def test_new_resource_hosts_still_require_exact_download_bytes(tmp_path, monkeypatch, url, received):
    expected = b"selected bytes"
    selected = {"url": url, "size": len(expected), "sha256": hashlib.sha256(expected).hexdigest()}
    p.public_url(url)
    calls = []
    def response(actual_url, timeout):
        calls.append((actual_url, timeout))
        return io.BytesIO(received)
    monkeypatch.setattr(p.urllib.request, "urlopen", response)
    target = tmp_path / "download"
    if received == expected:
        p.download(target, selected)
        assert target.read_bytes() == expected
    else:
        with pytest.raises(ValueError, match="public input"):
            p.download(target, selected)
    assert calls == [(url, 60)]


def test_public_projection_preserves_full_wheel_identity_and_dependency_order(fixture, monkeypatch):
    c = fixture
    # Two distinct ordered dependencies expose truncation, sorting or pairing
    # mistakes. Full nested wheel identities must survive locator removal.
    extra = c.artifacts / 'second-dependency.whl'
    extra.write_bytes(b'distinct second dependency')
    c.selection['dependency_wheels'].insert(0, {**pin(extra),
        'path': c.selection['artifact_root'] + '/' + extra.name})
    for name, distribution in [('meta', 'meerkat-mobkit'), ('toolkit', 'meerkat-toolkit')]:
        artifact = c.selection[name + '_wheel']
        c.selection[name] = {'distribution': distribution, 'version': '0.9.5',
            'size': artifact['size'], 'sha256': artifact['sha256'],
            'source_payload': [[distribution.replace('-', '_') + '/__init__.py', '1' * 64, 12]],
            'metadata': [['METADATA', '2' * 64, 34]]}
    dump(c.artifacts / 'RUNTIME-SELECTION.json', c.selection)
    c.pack()
    t, publish = prepare_publish(c, monkeypatch)
    accepted = (c.root / 'accepted/acceptance.json').read_bytes()
    result = publish()
    assert_public_fragment(c, result)
    assert (c.root / 'accepted/acceptance.json').read_bytes() == accepted
    assert p.read_json(c.root / 'accepted/acceptance.json')['selection'] == c.selection
    assert result['selection']['meta'] == c.selection['meta']
    assert result['selection']['toolkit'] == c.selection['toolkit']
    assert [row['sha256'] for row in result['selection']['dependency_wheels']] == [
        p.digest(extra), p.digest(c.artifacts / 'dependency.whl')]
    assert len(c.reader_calls) == 1 and t.uploads == list(c.files)


# These controls enter the real publication command with the existing offline
# reader/transport fixture. They do not claim signatures or Forge acceptance.
def delivery_evidence(c, name, raw):
    path = c.artifacts / name
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_bytes(raw)
    for selected in c.selection.values():
        if isinstance(selected, dict) and selected.get('path') == c.selection['artifact_root'] + '/' + name:
            selected.update(size=len(raw), sha256=hashlib.sha256(raw).hexdigest())
    dump(c.artifacts / 'RUNTIME-SELECTION.json', c.selection)
    c.pack()


def assert_delivery_refused(c, transport, publish, message='Forge pinned text hygiene refused'):
    accepted = (c.root / 'accepted/acceptance.json').read_bytes()
    original = {name: (c.candidate / name).read_bytes() for name in c.files}
    with pytest.raises(ValueError, match=message):
        publish()
    assert not transport.uploads
    report = p.read_json(c.root / 'publication/publication.json')
    assert report['status'] == 'refused_or_partial' and report['write_attempted'] == []
    assert not (c.root / 'publication/forge-fragment.json').exists()
    assert 'files' not in report
    assert (c.root / 'accepted/acceptance.json').read_bytes() == accepted
    assert {name: (c.root / 'publication/assets' / name).read_bytes() for name in c.files} == original
    assert {name: (c.candidate / name).read_bytes() for name in c.files} == original
    assert len(c.commands) == 2 and len(c.reader_calls) == 1


@pytest.mark.parametrize('name', ['build-record.json', 'resource-build-record.json',
    'gateway-verification.json', 'source-manifest.json', 'indirect/retained.jsonl',
    'indirect/notes.md', 'indirect/readme.txt', 'product.tar.gz.sha256'])
def test_publication_scans_every_pinned_text_category_before_write(fixture, monkeypatch, name):
    c = fixture
    raw = (json.dumps({'producer': '/Users/private-person/work/input'}).encode()
           if name.endswith('.json') else b'producer /home/private-person/work/input\n')
    delivery_evidence(c, name, raw)
    t, publish = prepare_publish(c, monkeypatch)
    assert_delivery_refused(c, t, publish)


def test_original_selection_remains_attested_but_personal_paths_refuse_delivery(fixture, monkeypatch):
    c = fixture
    before_root = c.selection['artifact_root']
    c.selection = json.loads(json.dumps(c.selection).replace(before_root, '/Users/private-person/artifacts'))
    dump(c.artifacts / 'RUNTIME-SELECTION.json', c.selection)
    c.pack()
    t, publish = prepare_publish(c, monkeypatch)
    # The prospective fragment has no locators. Its original pinned document
    # still must pass F2; projection is not evidence sanitation.
    assert_delivery_refused(c, t, publish)


@pytest.mark.parametrize('location', ['key', 'value'])
@pytest.mark.parametrize('spelling', ['slash', 'unicode'])
def test_publication_decodes_escaped_json_home_paths(fixture, monkeypatch, location, spelling):
    c = fixture
    text = '/Users/private-person/work/input'
    raw = json.dumps({text: 'ordinary'} if location == 'key' else {'producer': text})
    raw = raw.replace('/', '\\/') if spelling == 'slash' else raw.replace('/', '\\u002f')
    assert '/Users/' not in raw
    delivery_evidence(c, 'gateway-verification.json', raw.encode())
    t, publish = prepare_publish(c, monkeypatch)
    assert_delivery_refused(c, t, publish)


@pytest.mark.parametrize('producer, allowed', [
    ('/Users/runner/work/input', True), ('/home/runner/work/input', True),
    ('/home/.cargo/registry/input', True), ('/Users/.cargo/registry/input', False),
    ('/Users/runner-other/work/input', False), ('/home/Runner/work/input', False),
    ('/home/.cargo-other/work/input', False), ('/Users/private-person/work/input', False),
])
def test_publication_pinned_evidence_has_only_exact_runner_exceptions(fixture, monkeypatch, producer, allowed):
    c = fixture
    delivery_evidence(c, 'build-record.json', json.dumps({'producer': producer}).encode())
    t, publish = prepare_publish(c, monkeypatch)
    if not allowed:
        return assert_delivery_refused(c, t, publish)
    before = (c.root / 'accepted/acceptance.json').read_bytes()
    result = publish()
    assert_public_fragment(c, result)
    assert t.uploads == list(c.files)
    assert (c.root / 'accepted/acceptance.json').read_bytes() == before
    checked = result['forge_delivery']['text_files']
    assert {row['path'] for row in checked} == {row['path'] for row in json.loads(before)['evidence']
        if row['path'].endswith(('.json', '.jsonl', '.md', '.txt', '.sha256'))}
    assert all(row['user_home_paths'] == row['private_names'] == 0 for row in checked)
    for name in c.files:
        assert t.files[name] == (c.candidate / name).read_bytes()


@pytest.mark.parametrize('wheel', ['meta', 'toolkit'])
@pytest.mark.parametrize('inventory', ['files', 'metadata', 'source_payload'])
def test_publication_f1_wheel_entries_keep_exact_private_name_exception(fixture, monkeypatch, wheel, inventory):
    c = fixture
    names = ['licenses/PAIRED_HOST-LIVE.md', 'licenses/LICENSE-EXTRACTED', 'meerkat_toolkit/frontend/paired_host_live.py']
    c.selection[wheel][inventory] = [[name, '1' * 64, 12] for name in names]
    # F2 uses exact selected wheel names for string values, including retained
    # evidence, while F1 permits the structural selection inventory entries.
    delivery_evidence(c, 'build-record.json', json.dumps({'retained_entries': names}).encode())
    t, publish = prepare_publish(c, monkeypatch)
    result = publish()
    assert_public_fragment(c, result)
    assert result['selection'][wheel][inventory] == c.selection[wheel][inventory]


@pytest.mark.parametrize('case', ['value_suffix', 'key', 'unselected_name', 'home_in_selected_name',
    'outside_inventory', 'row_hash'])
def test_publication_f1_never_exempts_paths_keys_or_other_selection_fields(fixture, monkeypatch, case):
    c = fixture
    name = 'licenses/PAIRED_HOST-LIVE.md'
    c.selection['toolkit']['files'] = [[name, '1' * 64, 12]]
    value = {'retained': name}
    fragment_refusal = False
    if case == 'value_suffix':
        value['retained'] += '.other'
    elif case == 'key':
        value = {name: 'ordinary'}
    elif case == 'unselected_name':
        value['retained'] = 'licenses/LICENSE-EXTRACTED'
    elif case == 'home_in_selected_name':
        c.selection['toolkit']['files'][0][0] = '/home/private-person/' + name
        fragment_refusal = True
    elif case == 'outside_inventory':
        c.selection['toolkit']['description'] = name
        fragment_refusal = True
    else:
        c.selection['toolkit']['files'][0][1] = name
        fragment_refusal = True
    delivery_evidence(c, 'build-record.json', json.dumps(value).encode())
    t, publish = prepare_publish(c, monkeypatch)
    assert_delivery_refused(c, t, publish, 'Forge fragment hygiene refused' if fragment_refusal
                            else 'Forge pinned text hygiene refused')


@pytest.mark.parametrize('case', ['exact', 'nested', 'wrong_file', 'sibling', 'key', 'home', 'escaped_home'])
def test_publication_f1b_is_only_manifest_file_path_private_name_exception(fixture, monkeypatch, case):
    c = fixture
    name = 'source-manifest.json'
    value = {'files': [{'path': 'docs/Paired host.md', 'sha256': '1' * 64, 'size': 12}]}
    if case == 'nested':
        value = {'nested': value}
    elif case == 'wrong_file':
        name = 'build-record.json'
    elif case == 'sibling':
        value['files'][0]['note'] = 'Paired host'
    elif case == 'key':
        value['files'][0]['Paired host'] = 'ordinary'
    elif case in ('home', 'escaped_home'):
        value['files'][0]['path'] = '/Users/private-person/docs/Paired host.md'
    raw = json.dumps(value)
    if case == 'escaped_home':
        raw = raw.replace('/', '\\u002f')
    delivery_evidence(c, name, raw.encode())
    t, publish = prepare_publish(c, monkeypatch)
    if case != 'exact':
        return assert_delivery_refused(c, t, publish)
    result = publish()
    assert_public_fragment(c, result)
    assert t.files[c.subjects[0]] == (c.candidate / c.subjects[0]).read_bytes()


@pytest.mark.parametrize('case', ['unselected_private', 'selected_exact', 'selected_plus_other', 'selected_home'])
def test_publication_non_json_text_uses_selected_helper_fallback(fixture, monkeypatch, case):
    c = fixture
    name = 'licenses/PAIRED_HOST-LIVE.md'
    raw = name
    if case != 'unselected_private':
        c.selection['toolkit']['files'] = [[name, '1' * 64, 12]]
    if case == 'selected_plus_other':
        raw += '\nMemoryStore\n'
    elif case == 'selected_home':
        raw += '\n/home/private-person/source\n'
    delivery_evidence(c, 'indirect/retained.jsonl', raw.encode())
    t, publish = prepare_publish(c, monkeypatch)
    if case != 'selected_exact':
        return assert_delivery_refused(c, t, publish)
    assert publish()['status'] == 'verified_public_inventory'


@pytest.mark.parametrize('location', ['selection_key', 'selection_value', 'layout', 'file_name'])
def test_publication_fragment_hygiene_covers_keys_values_layout_and_inventory(fixture, monkeypatch, location):
    c = fixture
    if location == 'selection_key':
        c.selection['/home/runner/unexpected'] = 'ordinary'
    elif location == 'selection_value':
        c.selection['meta']['description'] = '/Users/runner/source'
    elif location == 'file_name':
        delivery_evidence(c, 'indirect/Paired host.txt', b'ordinary contents')
    dump(c.artifacts / 'RUNTIME-SELECTION.json', c.selection)
    c.pack()
    t, publish = prepare_publish(c, monkeypatch)
    if location == 'layout':
        # Select this malformed layout handoff explicitly; original artifact
        # pins remain unchanged. Layout data cannot bypass fragment hygiene.
        accepted = c.root / 'accepted/acceptance.json'
        value = p.read_json(accepted)
        value['paths']['artifact_root'] = 'artifacts/home/runner/unexpected'
        dump(accepted, value)
    assert_delivery_refused(c, t, publish, 'Forge fragment hygiene refused')


@pytest.mark.parametrize('mutation', ['missing', 'changed'])
def test_publication_rechecks_pinned_text_before_scan(fixture, monkeypatch, mutation):
    c = fixture
    t, publish = prepare_publish(c, monkeypatch)
    path = c.root / 'accepted/unpacked/artifacts/gateway-verification.json'
    if mutation == 'missing':
        path.unlink()
        with pytest.raises(FileNotFoundError):
            publish()
        assert not t.uploads
        assert p.read_json(c.root / 'publication/publication.json')['write_attempted'] == []
    else:
        path.write_bytes(b'{"changed":"otherwise hygienic"}')
        assert_delivery_refused(c, t, publish, 'accepted text evidence changed')


def test_publication_scans_retained_text_bytes_despite_path_replacement(fixture, monkeypatch):
    c = fixture
    t, publish = prepare_publish(c, monkeypatch)
    original = p.copy_publication_input
    replaced = []
    def capture(path, stream, maximum):
        row = original(path, stream, maximum)
        if path == c.root / 'accepted/unpacked/artifacts/build-record.json':
            path.unlink()
            path.write_bytes(b'{"replacement":"/Users/private-person/input"}')
            replaced.append(path)
        return row
    monkeypatch.setattr(p, 'copy_publication_input', capture)
    result = publish()
    assert result['status'] == 'verified_public_inventory' and len(replaced) == 1
    for name in c.files:
        assert t.files[name] == (c.candidate / name).read_bytes()


@pytest.mark.parametrize('location', ['key', 'value'])
def test_publication_decodes_escaped_json_private_names(fixture, monkeypatch, location):
    c = fixture
    value = {'Paired host': 'ordinary'} if location == 'key' else {'note': 'Paired host'}
    raw = json.dumps(value).replace('P', '\\u0050')
    assert 'Paired host' not in raw
    delivery_evidence(c, 'gateway-verification.json', raw.encode())
    t, publish = prepare_publish(c, monkeypatch)
    assert_delivery_refused(c, t, publish)


@pytest.mark.parametrize('private', ['Paired host', 'Paired Host', 'organizational host', 'MemoryStore'])
def test_publication_preserves_selected_private_pattern_refusals(fixture, monkeypatch, private):
    c = fixture
    delivery_evidence(c, 'build-record.json', json.dumps({'note': private}).encode())
    t, publish = prepare_publish(c, monkeypatch)
    assert_delivery_refused(c, t, publish)
