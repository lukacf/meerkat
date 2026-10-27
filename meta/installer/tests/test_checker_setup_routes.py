"""Wizard prerequisite routes use fictional services and no downloads."""
import asyncio
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest
from fastapi.testclient import TestClient

from launchkit import server


@pytest.fixture
def api(monkeypatch):
    missing = {'status': 'needs_preparation', 'can_prepare': True, 'detail': 'Fixture checker missing'}
    service = SimpleNamespace(status=AsyncMock(return_value=missing),
        prepare=AsyncMock(return_value={'status': 'preparing', 'can_prepare': False, 'detail': 'Fixture preparation'}))
    implementation = SimpleNamespace(lock=asyncio.Lock(), tasks={}, processes={})
    monkeypatch.setattr(server, 'checker_setup', service)
    monkeypatch.setattr(server, 'implementation', implementation)
    return TestClient(server.app), service, implementation


def test_read_never_prepares_and_explicit_bodyless_post_does(api):
    client, service, _ = api
    assert client.get('/api/checker-setup').json()['status'] == 'needs_preparation'
    service.prepare.assert_not_awaited()
    response = client.post('/api/checker-setup')
    assert response.status_code == 202 and response.json()['status'] == 'preparing'
    service.prepare.assert_awaited_once_with()


def test_request_cannot_choose_arbitrary_executables_or_destination(api):
    client, service, _ = api
    response = client.post('/api/checker-setup', json={'command': 'private-input-canary', 'path': '/private-input-canary'})
    assert response.status_code == 422 and 'private-input-canary' not in response.text
    service.prepare.assert_not_awaited()


@pytest.mark.parametrize('owner', ['lock', 'tasks', 'processes'])
def test_active_application_owner_keeps_checker_fixed(api, owner):
    client, service, implementation = api
    if owner == 'lock':
        asyncio.run(implementation.lock.acquire())
    else:
        getattr(implementation, owner)['owned'] = object()
    try:
        assert client.post('/api/checker-setup').status_code == 409
        service.prepare.assert_not_awaited()
    finally:
        if owner == 'lock': implementation.lock.release()


def test_cross_site_preparation_is_refused(api):
    client, service, _ = api
    assert client.post('/api/checker-setup', headers={'Origin': 'https://elsewhere.example'}).status_code == 403
    service.prepare.assert_not_awaited()
