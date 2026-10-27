import io
import json
import zipfile
from types import SimpleNamespace

import pytest
from fastapi.testclient import TestClient

from launchkit import server
from launchkit.blueprint import starter_blueprint
from launchkit.projects import ProjectStore


@pytest.fixture
def client(tmp_path, monkeypatch):
    monkeypatch.setattr(server, 'projects', ProjectStore(tmp_path))
    return TestClient(server.app)


@pytest.fixture
def blueprint():
    return starter_blueprint().model_dump(mode='json')


def test_native_blueprint_schema_and_manual_import(client, blueprint):
    assert client.get('/api/blueprint/schema').json()['properties']['schema_version']['const']==2
    response=client.post('/api/blueprint/import',json=blueprint)
    assert response.status_code==200
    assert response.json()['blueprint']==blueprint
    blueprint['agent_classes'][0]['tools']['mcp']=[{'connection_id':'unknown','tool_names':[]}]
    assert client.post('/api/blueprint/import',json=blueprint).status_code==422


def test_large_population_can_be_imported_saved_and_packaged(client, blueprint):
    blueprint['agent_classes'][1]['instances'] = 512
    result = client.post('/api/blueprint/import', json=blueprint)
    assert result.status_code == 200, result.text
    assert result.json()['blueprint']['agent_classes'][1]['instances'] == 512
    saved = client.put('/api/projects/' + blueprint['project']['slug'],
                       json={'blueprint': blueprint, 'expected_revision': 0})
    assert saved.status_code == 200, saved.text
    package = client.post('/api/blueprint/package', json=blueprint)
    assert package.status_code == 200, package.text
    with zipfile.ZipFile(io.BytesIO(package.content)) as archive:
        runtime = json.loads(archive.read(blueprint['project']['slug'] + '/deployment.json'))
    assert len(runtime['agents']) == 514
    assert runtime['agents'][-2]['identity'] == 'research:512'


def test_project_save_is_revision_checked(client, blueprint):
    url='/api/projects/'+blueprint['project']['slug']
    result=client.put(url,json={'blueprint':blueprint,'expected_revision':0})
    assert result.status_code==200
    assert result.json()['revision']==1
    blueprint['project']['name']='Changed'
    assert client.put(url,json={'blueprint':blueprint,'expected_revision':0}).status_code==409
    assert client.get(url).json()['blueprint']['project']['name']!='Changed'
    assert client.put(url,json={'blueprint':blueprint,'expected_revision':1}).json()['revision']==2
    assert client.get(url+'?revision=1').json()['revision']==1
    assert len(client.get(url+'/history').json()['revisions'])==2


def test_expanded_api_rejects_foreign_origins_and_large_puts(client, blueprint):
    assert client.post('/api/blueprint/import',json=blueprint,headers={'Origin':'https://evil.example'}).status_code==403
    assert client.put('/api/projects/huge',content='x'*1_000_001).status_code==413


def test_large_animation_admits_exact_graph_without_dropping_identity_context(client, blueprint, monkeypatch):
    from launchkit.blueprint import Blueprint
    from copy import deepcopy
    blueprint['agent_classes'][1]['instances'] = 512
    extra = deepcopy(blueprint['agent_classes'][1])
    extra.update(id='research-extra', name='Additional research context', instances=96)
    blueprint['agent_classes'].append(extra)
    identities = Blueprint.model_validate(blueprint).identities()
    assert len(identities) == 610
    graph = {'nodes': [{'id': identity, 'kind': 'agent', 'name': identity,
                        'instructions': 'Detailed domain context. ' * 80,
                        'role': 'A distinct configured member', 'tools': {'memory': True}}
                       for identity in identities],
             'edges': [{'id': f'peer-{index}', 'source': identities[0], 'target': identity,
                        'kind': 'peer', 'label': 'Configured communication'}
                       for index, identity in enumerate(identities[1:])],
             'phases': []}
    payload = {'blueprint': blueprint, 'model': graph, 'stream_id': 'test-large-graph'}
    encoded = json.dumps(payload).encode()
    assert 1_000_000 < len(encoded) < 8_000_000
    admitted = []

    class Operations:
        async def submit(self, visualizer, specification, model, **options):
            admitted.append((specification, model, options))
            return {'id': 'test-large-graph', 'status': 'queued'}

    monkeypatch.setattr(server, 'visualization_operations', Operations())
    response = client.post('/api/visualizations', content=encoded,
                           headers={'Content-Type': 'application/json'})
    assert response.status_code == 202, response.text
    assert len(admitted) == 1
    assert admitted[0][0] == blueprint
    assert admitted[0][1] == graph
    assert admitted[0][2]['stream_id'] == 'test-large-graph'


@pytest.mark.parametrize('claimed_length', [None, '0'])
def test_animation_size_limit_bounds_headers_and_streamed_bytes(client, claimed_length):
    headers = {'Content-Type': 'application/json'}
    if claimed_length is not None:
        headers['Content-Length'] = claimed_length
    response = client.post('/api/visualizations', content=b'x' * 8_000_001, headers=headers)
    assert response.status_code == 413


def test_mcp_import_removes_secret_values(client):
    response=client.post('/api/connectors/import',json={'mcpServers':{'test':{'command':'npx','args':['test-mcp@1.0.0'],'env':{'API_TOKEN':'secret-must-not-return'}}}})
    assert response.status_code==200,response.text
    assert 'secret-must-not-return' not in response.text
    assert '${' in response.json()['connections'][0]['env']['API_TOKEN']


def test_v2_preview_matches_download_and_remains_secret_free(client, blueprint):
    response=client.post('/api/blueprint/validate',json=blueprint)
    assert response.status_code==200,response.text
    assert response.json()['valid'] is True
    expected={f['path']:f['content'] for f in response.json()['files']}
    result=client.post('/api/blueprint/package',json=blueprint)
    assert result.status_code==200,result.text
    with zipfile.ZipFile(io.BytesIO(result.content)) as archive:
        prefix=blueprint['project']['slug']+'/'
        assert {n.removeprefix(prefix) for n in archive.namelist()}==set(expected)
        for path, content in expected.items():
            assert archive.read(prefix+path).decode()==content
        assert archive.getinfo(prefix+'install.sh').external_attr >> 16 & 0o111


def test_mcp_inspection_checks_missing_values_before_launch(client, monkeypatch):
    monkeypatch.delenv('LAUNCH_TEST_MCP_TOKEN',raising=False)
    response=client.post('/api/connectors/inspect',json={'connection':{'id':'remote','name':'Remote','transport':'http','url':'https://example.com/mcp','headers':{'Authorization':'Bearer ${LAUNCH_TEST_MCP_TOKEN}'}}})
    assert response.status_code==422
    assert 'LAUNCH_TEST_MCP_TOKEN' in response.text


def test_mcp_inspection_returns_real_transport_metadata_and_closes(client, monkeypatch):
    from launchkit.templates_v2 import mcp_connections
    from types import SimpleNamespace
    closed=[]
    class FakeConnection:
        def __init__(self,config):
            self.tools={}
        async def start(self):
            self.tools={'read':SimpleNamespace(name='read',description='Read a document',inputSchema={'type':'object'},annotations=SimpleNamespace(readOnlyHint=True,destructiveHint=False))}
        async def close(self):closed.append(True)
    monkeypatch.setattr(mcp_connections,'MCPConnection',FakeConnection)
    response=client.post('/api/connectors/inspect',json={'connection':{'id':'local','name':'Local','transport':'stdio','command':'test-only'}})
    assert response.status_code==200,response.text
    assert response.json()['tools'][0]['read_only'] is True
    assert closed==[True]


@pytest.mark.parametrize('operation_status,context_identity,exposes_live', [
    ('running', 'current', True),
    ('running', 'other', False),
    ('completed', 'current', False),
    ('failed', 'current', False),
    ('interrupted', 'current', False),
    ('running', None, False),
    (None, 'current', False),
])
def test_live_proposal_is_scoped_to_its_running_setup(
    client, monkeypatch, blueprint, operation_status, context_identity, exposes_live,
):
    identity = '67f2f55d-8fd4-4d92-b999-14fb240e5730'
    calls = []

    async def read(service, requested_identity):
        calls.append((service, requested_identity))
        return {'id': identity, 'version': 1,
                'operation': {'status': operation_status, 'version': 1, 'request_id': 'a' * 64},
                'proposal': None}

    context = None if context_identity is None else {
        'guide_id': identity if context_identity == 'current' else 'another-setup',
        'guide_version': 1, 'operation_request_id': 'a' * 64,
    }
    monkeypatch.setattr(server, 'guide_operations', SimpleNamespace(read=read))
    monkeypatch.setattr(server, 'architect', SimpleNamespace(
        guide_context=context, proposal={'blueprint': blueprint}, guide_result=None,
        message_lock=SimpleNamespace(locked=lambda: True),
    ))

    response = client.get(f'/api/guide/{identity}')
    assert response.status_code == 200, response.text
    assert calls == [(server.guide, identity)]
    assert response.json()['proposal'] is None
    if exposes_live:
        assert response.json()['live_design'] == {
            'guide_id': identity, 'guide_version': 1, 'request_id': 'a' * 64,
            'blueprint': blueprint, 'brief': None, 'context_pending': True}
    else:
        assert 'live_design' not in response.json()


def test_native_guide_timeline_validates_limit_before_delegating(client, monkeypatch):
    identity = '67f2f55d-8fd4-4d92-b999-14fb240e5730'
    calls = []
    native_page = {'available': True, 'frames': [{'id': 'native-input', 'kind': 'user_input',
        'interaction_id': 'native-turn', 'payload': {'content': 'My agent context'}}],
        'bindings': [], 'next_cursor': 'console:8', 'exhausted': True}

    async def read(requested_identity, *, after, limit):
        calls.append((requested_identity, after, limit))
        return native_page

    monkeypatch.setattr(server, 'guide_timeline', SimpleNamespace(read=read))
    for invalid_limit in ['0', '501', '-1', 'invalid']:
        response = client.get(f'/api/guide/{identity}/timeline', params={'limit': invalid_limit})
        assert response.status_code == 422
    assert calls == []

    response = client.get(f'/api/guide/{identity}/timeline', params={'after': 'console:7', 'limit': 32})
    assert response.status_code == 200, response.text
    assert response.json() == native_page
    assert calls == [(identity, 'console:7', 32)]
