import copy
import json
import tomllib

import pytest

from launchkit.architect import ArchitectService
from launchkit.blueprint import Blueprint, Connection, starter_blueprint
from launchkit.capability_evidence import extract_tools
from launchkit.compiler import environment_fields, generate_files, mob_toml
from launchkit.connector_discovery import normalize_registry_entry
from launchkit.design_knowledge import design_findings
from test_compiler import load_template


def test_named_domain_owners_have_stable_identity_and_individual_skills():
    value = starter_blueprint().model_dump()
    role = value['agent_classes'][1]
    role['members'] = [dict(key='energy', name='Energy specialist', instructions='Own the energy evidence and tariff assumptions.'),
                       dict(key='water', name='Water specialist', instructions='Own water use and water-saving comparisons.')]
    for step in value['flows'][0]['steps']:
        step['agent'] = step['agent'].replace('research:1', 'research:energy').replace('research:2', 'research:water')
    spec = Blueprint.model_validate(value)
    files = generate_files(spec)
    native = tomllib.loads(files['config/mob.toml'])
    runtime = json.loads(files['deployment.json'])
    assert 'research:energy' in spec.identities()
    assert runtime['agents'][1]['name'] == 'Energy specialist'
    assert native['flows']['briefing']['steps']['challenge']['role'] == 'research__water'
    assert 'Own water use' in native['skills']['research__water_member']['content']
    assert all('launch_comms' in profile['skills'] for profile in native['profiles'].values())
    assert 'reply_to_peer' in native['skills']['launch_comms']['content']
    role['instances'] = 1
    with pytest.raises(ValueError, match='match the instance count'):
        Blueprint.model_validate(value)


def test_reviewed_synthesis_receives_original_evidence():
    native = tomllib.loads(mob_toml(starter_blueprint()))
    synthesis = native['flows']['briefing']['steps']['synthesize']
    assert set(synthesis['depends_on']) == {'investigate', 'challenge', 'review'}
    assert '{{ steps.investigate }}' in synthesis['message']
    assert not design_findings(starter_blueprint())
    old = starter_blueprint().model_dump()
    old['flows'][0]['steps'][-1]['depends_on'] = ['review']
    assert any(item['code'] == 'indirect_evidence' for item in design_findings(Blueprint.model_validate(old)))


def test_named_context_owners_need_no_fixed_workflow():
    value = starter_blueprint().model_dump()
    value['flows'] = []
    role = value['agent_classes'][1]
    role['members'] = [
        dict(key='energy', name='Energy expert', instructions='Retain tariff history, supplier constraints and energy decisions.'),
        dict(key='water', name='Water expert', instructions='Retain water use evidence, maintenance context and water decisions.'),
    ]
    assert not design_findings(Blueprint.model_validate(value))
    role['members'] = []
    assert any(item['code'] == 'population_contribution'
               for item in design_findings(Blueprint.model_validate(value)))


def test_optional_region_keeps_nonsensitive_optional_meaning_and_is_omitted(monkeypatch):
    normalized = normalize_registry_entry({'server': {'name': 'example/regions', 'version': '1.0.0',
        'packages': [{'registryType': 'npm', 'identifier': 'example-mcp', 'version': '1.0.0', 'transport': {'type': 'stdio'},
                     'environmentVariables': [{'name': 'REGION', 'isSecret': False, 'isRequired': False, 'description': 'Optional region'}],
                     'packageArguments': [{'type': 'named', 'name': '--scope', 'valueHint': 'SCOPE', 'isRequired': False}]}]}})
    config = normalized['connection']
    spec = starter_blueprint().model_dump()
    spec['connections'] = [config]
    spec['agent_classes'][0]['tools']['mcp'] = [{'connection_id': config['id'], 'tool_names': ['read']}]
    fields = environment_fields(Blueprint.model_validate(spec))
    region_key = next(item['name'] for item in config['setup_inputs'] if 'REGION' in item['name'])
    region = next(item for item in fields if item['key'] == region_key)
    assert region['secret'] is False and region['required'] is False
    for item in config['setup_inputs']:
        monkeypatch.delenv(item['name'], raising=False)
    resolved = load_template('mcp_connections').configured_transport(config)
    assert resolved['env'] == {}
    assert '--scope' not in resolved['args']
    monkeypatch.setenv(region_key, 'eu')
    assert load_template('mcp_connections').configured_transport(config)['env']['REGION'] == 'eu'


def test_publisher_tool_evidence_does_not_treat_inline_examples_as_capabilities():
    document = '## Tools\n- **get_file_contents** - Read repository files\n- **issue_write** - Create or update an issue\nExample: `invented_tool`\n'
    assert {item['name'] for item in extract_tools(document)} == {'get_file_contents', 'issue_write'}


@pytest.mark.asyncio
async def test_guided_proposals_reject_invented_scopes_and_require_reference(tmp_path):
    service = ArchitectService(tmp_path)
    base = starter_blueprint().model_dump(mode='json')
    service.set_context(base)
    service.guide_context = {'action': 'answer'}
    assert not (await service.propose({'blueprint': base}))['valid']
    await service.read_design_reference({})
    config = Connection(id='source', name='Source', transport='http', url='https://example.test/mcp').model_dump(mode='json')
    service.discovered['source'] = {**config, 'tool_evidence': {'source_url': 'https://example.test/docs', 'sha256': 'a' * 64, 'retrieved_at': '2026-09-27', 'tool_names': ['read_item', 'write_item']}}
    candidate = copy.deepcopy(base)
    candidate['connections'] = [config]
    candidate['agent_classes'][0]['tools']['mcp'] = [{'connection_id': 'source', 'tool_names': ['invented_read_all']}]
    assert not (await service.propose({'blueprint': candidate}))['valid']
    candidate['agent_classes'][0]['tools']['mcp'][0]['tool_names'] = ['read_item']
    assert (await service.propose({'blueprint': candidate}))['valid']
    assert service.proposal['blueprint']['connections'][0]['tool_evidence']['tool_names'] == ['read_item', 'write_item']


def test_downloaded_install_sequence_includes_selected_knowledge_before_handshake():
    base = starter_blueprint().model_dump()
    base['knowledge']['mode'] = 'local'
    text = generate_files(Blueprint.model_validate(base))['README.md']
    assert text.index('manage.py up') < text.index('--connections-check')
    assert text.index('manage.py verify') < text.index('--connections-check')
    base['target'] = 'gcp'
    base['gcp']['project'] = 'test-project-123'
    base['gcp'].update(machine_type='e2-standard-8', disk_gb=100)
    text = generate_files(Blueprint.model_validate(base))['README.md'].split('## Agent classes')[0]
    assert 'On your computer, review' in text and 'On the VM' in text
    assert 'Run `bash install.sh`' not in text


def test_configuration_and_toolsets_are_not_capability_evidence():
    text = '''# Server
## Tools
- **read_file** - Read a file
## Toolsets
| Toolset | Description |
| `copilot_spaces` | A group of tools |
## Configuration
| Variable | Description |
| `API_ACCESS_TOKEN` | Secret input |
'''
    assert [item['name'] for item in extract_tools(text)] == ['read_file']


@pytest.mark.parametrize('header', [dict(default='us'), dict(value='us')])
def test_optional_header_defaults_survive_export_and_resolve_without_environment(header, monkeypatch):
    result = normalize_registry_entry({'server': {'name':'test/headers','version':'1.0.0','remotes':[{'type':'streamable-http','url':'https://example.test/mcp','headers':[{'name':'X-Region','isRequired':False,'isSecret':False,**header}]}]}})
    config = result['connection']
    field = config['setup_inputs'][0]
    monkeypatch.delenv(field['name'], raising=False)
    assert not field['required'] and not field['secret'] and field['default'] == 'us'
    assert load_template('mcp_connections').configured_transport(config)['headers']['X-Region'] == 'us'
    assert load_template('mcp_connections').configuration_value(field) == 'us'


def test_variable_header_keeps_optional_default(monkeypatch):
    entry = {'server': {'name':'test/variables','version':'1.0.0','remotes':[{'type':'streamable-http','url':'https://example.test/mcp','headers':[{'name':'X-Region','value':'{region}','isRequired':False,'isSecret':False,'variables':{'region':{'default':'eu','isRequired':False}}}]}]}}
    config = normalize_registry_entry(entry)['connection']
    field = config['setup_inputs'][0]
    monkeypatch.delenv(field['name'], raising=False)
    plan = starter_blueprint().model_dump()
    plan['connections'] = [config]
    exported = json.loads(generate_files(Blueprint.model_validate(plan))['config/connections.json'])
    assert load_template('mcp_connections').configured_transport(exported[0])['headers']['X-Region'] == 'eu'
    entry['server']['remotes'][0]['headers'][0]['value'] = 'region-{region}'
    unresolved = normalize_registry_entry(entry)
    assert not unresolved['installable'] and unresolved['connection'] is None
    assert any('complete header value in one local environment input' in warning for warning in unresolved['warnings'])
