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
import subprocess
import tarfile

import pytest

SPEC = importlib.util.spec_from_file_location("connect_candidate_publication", Path(__file__).with_name("connect-candidate.py"))
p = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(p)


def dump(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value, sort_keys=True, indent=2) + "\n")


def pin(path):
    return {"path": str(path), "size": path.stat().st_size, "sha256": p.digest(path)}


def verified_statement(path, request):
    repo = "https://github.com/lukacf/meerkat-connect"
    identity = repo + "/.github/workflows/connect-candidate.yml@" + request["source_ref"]
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
    c.native = {name: {"repository": "lukacf/" + repo, "version": "0.9.4", "tag": "v0.9.4", "source_sha": sha * 40}
                for name, repo, sha in (("core", "meerkat", "c"), ("meta", "meerkat-meta", "d"))}
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
    c.git_commands = []
    c.git_refs = {'HEAD^{commit}': 'd' * 40, 'refs/tags/v0.9.4^{commit}': 'd' * 40}
    real_output = subprocess.check_output
    def output(argv, **kwargs):
        args = [str(x) for x in argv]
        if args[0] == 'git':
            command = args[3:]
            c.git_commands.append(command)
            if command[:1] == ['rev-parse']:
                assert command[:2] == ['rev-parse', '--verify']
                return c.git_refs[command[2]] + '\n'
            if command[:1] == ['status']:
                return ''
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
    c.accept = lambda: p.accept_publication(c.root / 'request.json', c.candidate, c.meta, c.root / 'accepted')
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


@pytest.mark.parametrize('tag', ['', 'v0.9.4-rc.1'])
def test_exact_rc_selection_accepts_without_final_release_tag(fixture, tag):
    c = fixture
    for selected in c.native.values():
        selected['tag'] = tag
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


@pytest.mark.parametrize('tag', ['', 'v0.9.4'])
def test_selected_meta_commit_mismatch_refuses_before_reader(fixture, tag):
    c = fixture
    c.native['meta']['tag'] = tag
    reselect_native(c)
    c.git_refs['HEAD^{commit}'] = 'e' * 40
    with pytest.raises(ValueError, match='exact clean selected Meta checkout'):
        c.accept()
    assert not c.reader_calls


def test_selected_meta_tag_must_resolve_to_exact_selected_commit(fixture):
    c = fixture
    c.git_refs['refs/tags/v0.9.4^{commit}'] = 'e' * 40
    with pytest.raises(ValueError, match='exact clean selected Meta checkout'):
        c.accept()
    assert not c.reader_calls


@pytest.mark.parametrize('field,value', [
    ('source_sha', 'main'), ('source_sha', 'D' * 40), ('source_sha', None),
    ('version', '0.9.3'), ('repository', 'other/meerkat-meta'),
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
            actual = [str(source) if x == 'https://github.com/lukacf/meerkat-meta.git' else x for x in args]
            if self.wrong_commit and name == 'meta-fetch':
                actual[-1] = other
            log = work / (name + '.log')
            with log.open('xb') as output:
                subprocess.run(actual, cwd=cwd, stdout=output, stderr=subprocess.STDOUT, check=True)
            return {}, log
    return git, work, selected, other, LocalRecorder()


@pytest.mark.parametrize('tag', ['', 'v0.9.4', 'v0.9.4-rc.1'])
def test_source_fetch_uses_exact_commit_and_only_explicit_tag(local_native_fetch, tag):
    git, work, selected, _, record = local_native_fetch
    if tag:
        git('-c', 'user.name=Offline Control', '-c', 'user.email=offline@example.invalid',
            'tag', '-a', tag, selected, '-m', 'selected tag')
    checkout = p.fetch_native_source(record, work, 'meta', {
        'repository': 'lukacf/meerkat-meta', 'source_sha': selected, 'tag': tag})
    assert subprocess.check_output(['git', '-C', str(checkout), 'rev-parse', 'HEAD'], text=True).strip() == selected
    assert (checkout / 'payload').read_text() == 'selected commit\n'
    assert record.commands[1][-1] == selected
    tag_fetches = [args for args in record.commands if args[:2] == ['git', 'fetch'] and args[-1].startswith('refs/tags/')]
    assert [args[-1] for args in tag_fetches] == (['refs/tags/' + tag + ':refs/tags/' + tag] if tag else [])


@pytest.mark.parametrize('mismatch', ['commit', 'tag'])
def test_source_fetch_mismatch_refuses_before_checkout(local_native_fetch, mismatch):
    git, work, selected, other, record = local_native_fetch
    tag = ''
    if mismatch == 'tag':
        tag = 'v0.9.4'
        git('tag', tag, other)
    else:
        record.wrong_commit = True
    with pytest.raises(ValueError, match='fetched native commit differs|selected native tag points elsewhere'):
        p.fetch_native_source(record, work, 'meta', {
            'repository': 'lukacf/meerkat-meta', 'source_sha': selected, 'tag': tag})
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
    def publish():return p.publish_accepted(accepted,p.digest(accepted/'acceptance.json'),c.root/'publication')
    return transport,publish


def test_publication_uploads_only_accepted_bytes_then_emits_original_inventory(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch)
    result=publish()
    assert t.uploads==list(c.files)
    assert result['status']=='verified_public_inventory' and result['selection']==c.selection
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


def test_selected_acceptance_hash_and_allowlist_are_rechecked(fixture,monkeypatch):
    c=fixture;t,publish=prepare_publish(c,monkeypatch)
    directory=c.root/'accepted';selected=p.digest(directory/'acceptance.json')
    value=p.read_json(directory/'acceptance.json');value['scope']='different';dump(directory/'acceptance.json',value)
    with pytest.raises(ValueError,match='acceptance changed'):
        p.publish_accepted(directory,selected,c.root/'publication')
    assert not t.uploads


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
    assert result['selection']==c.selection
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
    def command(argv,*,stdin=None):
        commands.append((argv,stdin.read()))
        return {'controlled':'upload receipt'}
    monkeypatch.setattr(p,'command_json',command)
    with p.tempfile.TemporaryFile('w+b',dir=tmp_path) as retained:
        retained.write(b'exact accepted bytes');retained.seek(0)
        result=p.publication_api('https://uploads.github.com/controlled',upload=retained)
        assert not retained.closed
    assert result=={'controlled':'upload receipt'}
    assert commands[0][0][-2:]==['--input','-']
    assert commands[0][1]==b'exact accepted bytes'


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
