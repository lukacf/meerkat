"""Callback transport controls; no provider login or native outcome is modeled as real."""

import base64
import hashlib
import json
from pathlib import Path
import re
import subprocess

import pytest
from fastapi.testclient import TestClient

from launchkit.service_callback import MCP_CALLBACK_PATH, callback_landing


@pytest.fixture
def client():
    from launchkit.server import app
    return TestClient(app)


def test_actual_route_returns_fixed_document_and_exact_csp(client):
    clean = client.get(MCP_CALLBACK_PATH)
    returned = client.get(MCP_CALLBACK_PATH + '?code=private-code&state=private-state&iss=private-issuer')
    assert returned.status_code == 200
    assert returned.content == clean.content == callback_landing().body
    assert b'private-' not in returned.content
    assert returned.headers['cache-control'] == 'no-store'
    assert returned.headers['referrer-policy'] == 'no-referrer'
    assert int(returned.headers['content-length']) == len(returned.content)
    csp = returned.headers['content-security-policy']
    for tag in ('script', 'style'):
        [source] = re.findall(fr'<{tag}>(.*?)</{tag}>', returned.text, re.S)
        digest = base64.b64encode(hashlib.sha256(source.encode()).digest()).decode()
        assert f"{tag}-src 'sha256-{digest}'" in csp
    assert "connect-src 'self'" in csp
    assert "default-src 'none'" in csp
    assert 'unsafe-inline' not in csp


def test_actual_post_cannot_accept_or_echo_without_original_native_owner(client):
    response = client.post(MCP_CALLBACK_PATH + '?code=query-secret',
        content=b'{"query":"state=state-secret&code=body-secret"}',
        headers={'Origin': 'https://mk-installer-02.eu-prod-misc-01.int.p.midasplayer.cloud',
                 'Content-Type': 'application/json'})
    assert response.status_code == 503
    assert response.headers['cache-control'] == 'no-store'
    assert response.headers['referrer-policy'] == 'no-referrer'
    assert 'secret' not in response.text
    assert response.text == 'Connection setup is not available in this Forge session.'


def execute_page(**options):
    # Execute the served script with observable browser boundaries. This does
    # not simulate native OAuth success or replace a real browser acceptance.
    harness = Path(__file__).with_name('service_callback_browser.cjs')
    result = subprocess.run(['node', str(harness)],
        input=json.dumps({'html': callback_landing().body.decode(), **options}),
        capture_output=True, text=True, timeout=10, check=True)
    return json.loads(result.stdout)


def test_raw_query_is_cleared_before_identity_lookup_and_posted_once():
    query = 'code=fixture%2Bcode&state=original%2Fstate&iss=https%3A%2F%2Fslack.com&state=duplicate'
    result = execute_page(query=query, fragment='#private-fragment', status=202)
    assert [event['kind'] for event in result['events']] == ['replace', 'identity', 'post']
    assert result['location'] == MCP_CALLBACK_PATH
    assert result['events'][0]['url'] == MCP_CALLBACK_PATH
    identity, post = result['events'][1:]
    assert identity['url'] == '/__workspace/identity'
    assert post['url'] == MCP_CALLBACK_PATH
    assert json.loads(post['options']['body']) == {'query': query}
    assert post['options']['headers']['X-Forge-Workspace'] == 'a' * 64
    assert post['options']['headers']['Content-Type'] == 'application/json'
    for event in (identity, post):
        assert event['options']['redirect'] == 'error'
        assert event['options']['credentials'] == 'same-origin'
        assert event['options']['cache'] == 'no-store'
        assert event['options']['referrerPolicy'] == 'no-referrer'
    assert result['message'] == 'Callback received. Return to Forge to check whether the connection completed.'
    assert 'fixture' not in result['message']


@pytest.mark.parametrize('options', [
    {'identity_status': 401}, {'identity_status': 503},
    {'identity_type': 'text/html'}, {'identity': {'workspace': 'wrong'}},
    {'identity': {'workspace': ['a' * 64]}}, {'identity': None},
    {'identity_error': True}, {'identity_json_error': True},
])
def test_no_callback_post_when_workspace_lookup_is_unconfirmed(options):
    result = execute_page(query='code=fixture&state=original', **options)
    assert [event['kind'] for event in result['events']] == ['replace', 'identity']
    assert 'could not be confirmed' in result['message']


@pytest.mark.parametrize('options', [
    {'status': 200}, {'status': 401}, {'status': 403}, {'status': 409},
    {'status': 503}, {'post_error': True},
])
def test_unknown_or_rejected_return_never_replays_code(options):
    result = execute_page(query='code=fixture&state=original', **options)
    assert [event['kind'] for event in result['events']] == ['replace', 'identity', 'post']
    assert 'could not be confirmed' in result['message']


@pytest.mark.parametrize('query', ['', 'x' * 8193, 'code=\u00e5'])
def test_empty_or_unbounded_return_is_cleared_without_network(query):
    result = execute_page(query=query)
    assert [event['kind'] for event in result['events']] == ['replace']
    assert 'could not be confirmed' in result['message']


def test_no_network_if_url_cannot_be_cleared():
    result = execute_page(query='code=fixture', clear_error=True)
    assert [event['kind'] for event in result['events']] == ['replace']
    assert 'could not be confirmed' in result['message']
