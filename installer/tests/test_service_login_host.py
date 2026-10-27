"""Real SDK/private-pipe composition controls with synthetic native responses.

Run with FORGE_SERVICE_LOGIN_SDK_WHEEL pointing to the pinned development wheel.
No installed runtime, provider, credential, or native admission is qualified.
"""
import asyncio
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import sys
import threading
import time
from types import ModuleType, SimpleNamespace
from urllib.parse import urlencode
import zipfile

import httpx
import pytest

from launchkit.service_login_host import ForgeServiceLoginHost, ServiceLoginUnavailable
from launchkit.templates_v2.console_auth import ConsoleConnectLogin

ORIGIN = 'https://forge.example.invalid'
REDIRECT = ORIGIN + '/services/mcp/callback'
WHEEL_SHA = '034e9044ef524a60f0dd4bbc0c37ce6094819bd08ca1a87d31ed2686e30cfa8c'
SOURCES = {
    'host_control': '578c788b711da5c9c1ee79e9813278951f6c95d5cd299503342e939942301211',
    'mcp_auth': '9649007dd435566f100d3302ad860ebdde78145064e066576b1e59cebb76c806',
    'mcp_auth_native': 'ebb5656c931677030f6ee41e0f4f981ce4395565cf9f9dd073e165cdf3b2a70b',
}


@pytest.fixture
def sdk(monkeypatch, tmp_path):
    selected = os.environ.get('FORGE_SERVICE_LOGIN_SDK_WHEEL')
    if not selected:
        pytest.skip('Explicit pinned development SDK wheel required for pipe controls.')
    wheel = Path(selected)
    assert hashlib.sha256(wheel.read_bytes()).hexdigest() == WHEEL_SHA

    class UncalledDependency:
        def __new__(cls, *args, **kwargs):
            raise AssertionError('No Meta process or runtime may be constructed.')

    for name in ('meerkat_meta', 'meerkat_toolkit'):
        package = ModuleType(name)
        package.__path__ = []
        monkeypatch.setitem(sys.modules, name, package)
    for name, attribute in (('meerkat_meta._transport', 'PersistentTransport'),
                            ('meerkat_meta.runtime', 'MetaRuntime')):
        module = ModuleType(name)
        setattr(module, attribute, UncalledDependency)
        monkeypatch.setitem(sys.modules, name, module)

    def load(name, path):
        spec = importlib.util.spec_from_file_location(name, path)
        module = importlib.util.module_from_spec(spec)
        monkeypatch.setitem(sys.modules, name, module)
        spec.loader.exec_module(module)
        return module

    modules = {}
    with zipfile.ZipFile(wheel) as archive:
        for name, digest in SOURCES.items():
            data = archive.read('meerkat_toolkit/' + name + '.py')
            assert hashlib.sha256(data).hexdigest() == digest
            path = tmp_path / (name + '.py')
            path.write_bytes(data)
            modules[name] = load('meerkat_toolkit.' + name, path)
    load('launchkit.templates_v2.native_console_login', Path(__file__).parents[1] /
         'launchkit/templates_v2/native_console_login.py')
    return SimpleNamespace(**modules)


class PipeNative:
    """The actual SDK talks over anonymous pipes to these explicit fixtures."""
    def __init__(self, sdk):
        child_read, parent_write = os.pipe()
        parent_read, child_write = os.pipe()
        self.client = sdk.host_control.PrivateHostClient(parent_write, parent_read, timeout=3)
        self.reader = os.fdopen(child_read, 'rb')
        self.writer = os.fdopen(child_write, 'wb', buffering=0)
        self.commands, self.errors, self.handlers = [], [], []
        self.lock = threading.Lock()
        self.gates = {k: threading.Event() for k in ('enroll', 'describe', 'start', 'complete', 'retire')}
        for gate in self.gates.values():
            gate.set()
        self.redirect = REDIRECT
        self.joined = self.duplicate_state = self.bad_cancel = False
        self.retire_releases_enroll = False
        self.ceremonies, self.completed = {}, set()
        self.transport_close = self.client.close
        self.close_calls = 0

        def forbidden_close():
            self.close_calls += 1
            raise AssertionError('Forge cannot close the shared native client.')

        self.client.close = forbidden_close
        self.thread = threading.Thread(target=self.read, daemon=True)
        self.thread.start()

    def read(self):
        for line in self.reader:
            request = json.loads(line)
            self.commands.append(request)
            thread = threading.Thread(target=self.reply, args=(request,), daemon=True)
            self.handlers.append(thread)
            thread.start()

    def reply(self, request):
        try:
            cmd, identifier = request['command'], request['id']
            if cmd['kind'] == 'enroll_console_login':
                assert self.gates['enroll'].wait(5)
                value = {'kind': 'console_login_enrolled', 'enrollment_id': identifier}
            elif cmd['kind'] == 'retire_console_login':
                if self.retire_releases_enroll:
                    self.gates['enroll'].set()
                assert self.gates['retire'].wait(5)
                value = {'kind': 'console_login_retired', 'enrollment_id': cmd['enrollment_id'],
                         'observation': {'fenced': True, 'reservations_drained': True, 'uncertain': False}}
            else:
                assert cmd['kind'] == 'hosted_mcp_auth'
                op = cmd['operation']
                action = op['action']
                if action in self.gates:
                    assert self.gates[action].wait(5)
                target = op.get('target') or self.ceremonies[op['ceremony_id']]
                if action == 'describe':
                    result = {'mcp': target, 'redirect_uri': self.redirect}
                elif action == 'start':
                    self.ceremonies[identifier] = target
                    state = 'same-state' if self.duplicate_state else 'state-' + str(identifier)
                    result = {'mcp': target, 'disposition': 'joined' if self.joined else 'started',
                              'ceremony_id': None if self.joined else identifier, 'state': state,
                              'authorize_url': 'https://provider.invalid/authorize?' + urlencode({'state': state}),
                              'redirect_uri': self.redirect}
                elif action == 'complete':
                    self.completed.add(op['ceremony_id'])
                    result = {'mcp': target, 'account_id': target['oauth_account'], 'expires_at': None,
                              'scopes': ['read'], 'has_refresh_token': True}
                elif action == 'status':
                    result = {'mcp': target, 'phase': 'authorized', 'account_id': target['oauth_account'],
                              'expires_at': None}
                elif action == 'cancel':
                    done = op['ceremony_id'] in self.completed
                    result = {'mcp': {} if self.bad_cancel else target, 'cancelled': not done, 'completed': done}
                elif action == 'release':
                    result = {'released': True}
                else:
                    raise AssertionError(action)
                value = {'kind': 'hosted_mcp_auth', 'value': result}
            with self.lock:
                self.writer.write(json.dumps({'version': 1, 'id': identifier, 'result': value}).encode() + b'\n')
        except BaseException as error:
            self.errors.append(error)

    def matching(self, action):
        return [r for r in self.commands if r['command'].get('operation', {}).get('action') == action
                or r['command']['kind'] == action]

    def dispose(self):
        for gate in self.gates.values():
            gate.set()
        self.transport_close()
        self.thread.join(3)
        for thread in self.handlers:
            thread.join(3)
        self.reader.close()
        self.writer.close()
        assert not self.thread.is_alive()
        assert not any(t.is_alive() for t in self.handlers)
        assert not self.errors
        assert self.close_calls == 0


@pytest.fixture
def native(sdk):
    fixture = PipeNative(sdk)
    yield fixture
    fixture.dispose()


def make(sdk, native, *, scope='original-signed-session'):
    current = {'value': True}
    login = ConsoleConnectLogin('fixture-subject', scope,
        int(time.time() * 1000) + 60000, ORIGIN, lambda: current['value'])
    targets = {name: sdk.mcp_auth.McpTarget(name, 'https://' + name + '.example.invalid/mcp', 'fixture-account')
               for name in ('slack', 'second')}
    host = ForgeServiceLoginHost(native.client, login, issuer='fixture-issuer', redirect_uri=REDIRECT, targets=targets)
    return host, login, current


async def until(predicate):
    async with asyncio.timeout(3):
        while not predicate():
            await asyncio.sleep(.001)


def body(state, **fields):
    return json.dumps({'query': urlencode({'state': state, **fields})}).encode()


def test_original_enrollment_and_cancelled_begin_are_retained(sdk, native):
    async def exercise():
        host, login, _ = make(sdk, native)
        native.gates['enroll'].clear()
        host.start()
        waiter = asyncio.create_task(host.begin(login, 'slack'))
        try:
            await until(lambda: native.matching('enroll_console_login'))
            assert not native.matching('describe')
            waiter.cancel()
            with pytest.raises(asyncio.CancelledError):
                await waiter
            native.gates['enroll'].set()
            assert await host.begin(login, 'slack') == 'started'
            assert await host.begin(login, 'slack') == 'started'
            assert len(native.matching('enroll_console_login')) == len(native.matching('start')) == 1
        finally:
            native.gates['enroll'].set()
            await host.close()
        [start] = native.matching('start')
        [cancel] = native.matching('cancel')
        assert cancel['command']['operation']['ceremony_id'] == start['id']
        assert len(native.matching('release')) == len(native.matching('retire_console_login')) == 1
        assert host.retired
    asyncio.run(exercise())


def test_two_targets_share_uri_and_keep_native_completion_separate(sdk, native):
    async def exercise():
        host, login, _ = make(sdk, native)
        host.start()
        try:
            assert await host.begin(login, 'slack') == await host.begin(login, 'second') == 'started'
            native.gates['complete'].clear()
            for name in ('slack', 'second'):
                state = host._attempts[name]['callback'].state
                assert host.receive(login, origin=ORIGIN, body=body(state, code='code+/', iss='https://issuer.invalid/')) == 202
                assert host.receive(login, origin=ORIGIN, body=body(state, code='repeat')) == 400
            await until(lambda: len(native.matching('complete')) == 2)
            observer = asyncio.create_task(host.wait(login, 'slack'))
            await asyncio.sleep(0)
            assert not observer.done()
            observer.cancel()
            with pytest.raises(asyncio.CancelledError):
                await observer
            native.gates['complete'].set()
            for name in ('slack', 'second'):
                result = await host.wait(login, name)
                assert result.phase == 'authorized' and result.scopes == ('read',)
                assert (await host.observe(login, name)).phase == 'authorized'
            for request in native.matching('complete'):
                op = request['command']['operation']
                assert op['code'] == 'code+/' and op['iss'] == 'https://issuer.invalid/'
                assert op['redirect_uri'] == REDIRECT
        finally:
            native.gates['complete'].set()
            await host.close()
        assert len(native.matching('start')) == len(native.matching('cancel')) == len(native.matching('release')) == 2
        assert len(native.matching('enroll_console_login')) == len(native.matching('retire_console_login')) == 1
    asyncio.run(exercise())


@pytest.mark.parametrize('bad_query', [
    'state={state}&state={state}&code=x', 'state={state}&code=x&code=y',
    'state={state}&code=x&iss=a&iss=b', 'state={state}&code=%FF',
    'state={state}&code=x&error=denied', 'state=other&code=x',
])
def test_sdk_refuses_ambiguous_callback_without_native_completion(sdk, native, bad_query):
    async def exercise():
        host, login, _ = make(sdk, native)
        host.start()
        try:
            await host.begin(login, 'slack')
            state = host._attempts['slack']['callback'].state
            raw = json.dumps({'query': bad_query.format(state=state)}).encode()
            assert host.receive(login, origin=ORIGIN, body=raw) == 400
            assert not native.matching('complete')
        finally:
            await host.close()
    asyncio.run(exercise())


@pytest.mark.parametrize('stage', ['enroll', 'describe', 'start'])
def test_logout_joins_original_pending_work_even_after_waiter_cancellation(sdk, native, stage):
    async def exercise():
        host, login, _ = make(sdk, native)
        native.gates[stage].clear()
        native.retire_releases_enroll = True
        host.start()
        begin = asyncio.create_task(host.begin(login, 'slack'))
        try:
            await until(lambda: native.matching('enroll_console_login' if stage == 'enroll' else stage))
            close = asyncio.create_task(host.close())
            await until(lambda: host._closed)
            assert host.receive(login, origin=ORIGIN, body=body('x', code='x')) == 403
            if stage != 'enroll':
                assert not close.done()
                close.cancel()
                await asyncio.sleep(0)
                assert not close.done()
                native.gates[stage].set()
                with pytest.raises(asyncio.CancelledError):
                    await close
            else:
                await asyncio.wait_for(close, 3)
            with pytest.raises(ServiceLoginUnavailable):
                await begin
            await host.close()
            assert host.retired
            assert len(native.matching('retire_console_login')) == 1
            assert len(native.matching('start')) == (1 if stage == 'start' else 0)
        finally:
            for gate in native.gates.values():
                gate.set()
            await asyncio.gather(begin, return_exceptions=True)
            await host.close()
    asyncio.run(exercise())


def test_mismatched_redirect_is_retained_and_never_starts(sdk, native):
    async def exercise():
        host, login, _ = make(sdk, native)
        native.redirect = REDIRECT + '/wrong'
        host.start()
        try:
            for _ in range(2):
                with pytest.raises(ServiceLoginUnavailable):
                    await host.begin(login, 'slack')
            assert len(native.matching('describe')) == 1
            assert not native.matching('start')
        finally:
            await host.close()
    asyncio.run(exercise())


def test_joined_attempt_has_no_callback_or_cancel_authority(sdk, native):
    async def exercise():
        host, login, _ = make(sdk, native)
        native.joined = True
        host.start()
        try:
            assert await host.begin(login, 'slack') == 'joined'
            assert not host._states
            assert host.receive(login, origin=ORIGIN, body=body('x', code='x')) == 400
        finally:
            await host.close()
        assert not native.matching('cancel') and not native.matching('release')
    asyncio.run(exercise())


def test_ambiguous_native_state_fences_both_original_attempts(sdk, native):
    async def exercise():
        host, login, _ = make(sdk, native)
        native.duplicate_state = True
        host.start()
        try:
            await host.begin(login, 'slack')
            with pytest.raises(ServiceLoginUnavailable):
                await host.begin(login, 'second')
            assert host.receive(login, origin=ORIGIN, body=body('same-state', code='x')) == 403
        finally:
            await host.close()
        assert len(native.matching('cancel')) == len(native.matching('release')) == 2
    asyncio.run(exercise())


def test_unconfirmed_cleanup_is_retained_without_retry(sdk, native):
    async def exercise():
        host, login, _ = make(sdk, native)
        native.bad_cancel = True
        host.start()
        await host.begin(login, 'slack')
        for _ in range(2):
            with pytest.raises(ServiceLoginUnavailable):
                await host.close()
        assert not host.retired
        assert len(native.matching('cancel')) == len(native.matching('release')) == 1
        assert len(native.matching('retire_console_login')) == 1
    asyncio.run(exercise())


def test_real_route_requires_retained_context_and_preserves_receipt_headers(sdk, native, monkeypatch):
    from launchkit import server

    async def exercise():
        host, login, current = make(sdk, native)
        context = {'value': login}
        monkeypatch.setattr(server, 'service_login_route', None)
        route = server.select_service_login_host(host, lambda request: context['value'])
        try:
            with pytest.raises(ServiceLoginUnavailable):
                server.select_service_login_host(host, lambda request: login)
            await host.begin(login, 'slack')
            state = host._attempts['slack']['callback'].state
            async with httpx.AsyncClient(transport=httpx.ASGITransport(app=server.app), base_url='http://testserver') as client:
                async def post(payload, headers=None, suffix=''):
                    response = await client.post('/services/mcp/callback' + suffix, content=payload,
                        headers=headers or {'Origin': ORIGIN, 'Content-Type': 'application/json'})
                    assert response.headers['cache-control'] == 'no-store'
                    assert int(response.headers['content-length']) == len(response.content)
                    assert b'secret' not in response.content
                    return response.status_code

                copied = ConsoleConnectLogin(login.principal, login.login_scope_id, login.expires_at_ms, login.origin, login.is_current)
                context['value'] = copied
                assert await post(body(state, code='secret')) == 403
                context['value'] = login
                assert await post(body(state, code='secret'), {'Origin': 'https://other.invalid', 'Content-Type': 'application/json'}) == 403
                assert await post(body(state, code='secret'), [('Origin', ORIGIN), ('Origin', ORIGIN), ('Content-Type', 'application/json')]) == 403
                assert await post(body(state, code='secret'), {'Origin': ORIGIN, 'Content-Type': 'text/plain'}) == 400
                assert await post(body(state, code='secret'), suffix='?code=secret') == 400
                assert await post(b'{"query":"a","query":"b"}') == 400
                assert await post(b'x' * 65537) == 413
                assert not native.matching('complete')
                assert await post(body(state, code='secret')) == 202
                assert (await host.wait(login, 'slack')).phase == 'authorized'
                current['value'] = False
                assert await post(body(state, code='secret')) == 403
        finally:
            await route.shutdown()
    asyncio.run(exercise())


def test_server_lifespan_joins_original_login_owner(sdk, native, monkeypatch):
    from launchkit import server

    async def no_reconciliation():
        pass

    observed = []

    async def stop_selected(owners):
        selected = [o for o in owners if o.name == 'protected service login']
        assert len(selected) == 1
        observed.extend(selected)
        await selected[0].service.shutdown()
        return []

    monkeypatch.setattr(server, 'service_login_route', None)
    monkeypatch.setattr(server.provider_settings, 'restore', lambda: None)
    monkeypatch.setattr(server.visual_review_runtime, 'reconcile_terminated', no_reconciliation)
    monkeypatch.setattr(server, 'stop_owners', stop_selected)

    async def exercise():
        host, login, _ = make(sdk, native)
        async with server.lifespan(server.app):
            route = server.select_service_login_host(host, lambda request: login)
            await host.begin(login, 'slack')
        assert host.retired and observed[0].service is route
        assert len(native.matching('retire_console_login')) == 1
    asyncio.run(exercise())


def test_expired_login_refuses_native_actions_even_if_resolver_is_stale(sdk, native, monkeypatch):
    async def exercise():
        host, login, _ = make(sdk, native)
        host.start()
        try:
            await host.begin(login, 'slack')
            monkeypatch.setattr('launchkit.service_login_host.time.time', lambda: login.expires_at_ms / 1000 + 1)
            with pytest.raises(ServiceLoginUnavailable):
                await host.observe(login, 'slack')
            with pytest.raises(ServiceLoginUnavailable):
                await host.begin(login, 'second')
            assert host.receive(login, origin=ORIGIN, body=body('x', code='secret')) == 403
            assert len(native.matching('start')) == 1 and not native.matching('status')
        finally:
            await host.close()
    asyncio.run(exercise())


def test_original_browser_sessions_route_callbacks_independently(sdk, native, monkeypatch):
    from launchkit import server

    async def exercise():
        first, login_a, _ = make(sdk, native, scope='browser-a')
        second, login_b, _ = make(sdk, native, scope='browser-b')
        context = {'value': login_a, 'calls': 0}

        def resolver(request):
            context['calls'] += 1
            return context['value']

        monkeypatch.setattr(server, 'service_login_route', None)
        route = server.select_service_login_host(first, resolver)
        assert server.select_service_login_host(second, resolver) is route
        try:
            await asyncio.gather(first.begin(login_a, 'slack'), second.begin(login_b, 'slack'))
            assert len(native.matching('enroll_console_login')) == 2
            state_a = first._attempts['slack']['callback'].state
            state_b = second._attempts['slack']['callback'].state
            assert state_a != state_b
            async with httpx.AsyncClient(transport=httpx.ASGITransport(app=server.app), base_url='http://testserver') as client:
                async def post(state):
                    before = context['calls']
                    response = await client.post('/services/mcp/callback', content=body(state, code='secret'),
                        headers={'Origin': ORIGIN, 'Content-Type': 'application/json'})
                    assert context['calls'] == before + 1
                    return response.status_code

                assert await post(state_b) == 400  # State cannot select another login.
                context['value'] = ConsoleConnectLogin(login_a.principal, login_a.login_scope_id,
                    login_a.expires_at_ms, login_a.origin, login_a.is_current)
                assert await post(state_a) == 403  # Equal labels do not identify the original object.
                assert not native.matching('complete')
                context['value'] = login_a
                assert await post(state_a) == 202
                context['value'] = login_b
                assert await post(state_a) == 400
                assert await post(state_b) == 202
            assert (await first.wait(login_a, 'slack')).phase == 'authorized'
            assert (await second.wait(login_b, 'slack')).phase == 'authorized'
        finally:
            await route.shutdown()
        assert route.retired and first.retired and second.retired
        assert len(native.matching('retire_console_login')) == 2
    asyncio.run(exercise())


def test_session_logout_joins_original_cleanup_without_fencing_sibling(sdk, native, monkeypatch):
    from launchkit import server

    async def exercise():
        first, login_a, current_a = make(sdk, native, scope='browser-a')
        second, login_b, _ = make(sdk, native, scope='browser-b')
        resolver = lambda request: login_a
        monkeypatch.setattr(server, 'service_login_route', None)
        route = server.select_service_login_host(first, resolver)
        server.select_service_login_host(second, resolver)
        retirement = None
        try:
            await asyncio.gather(first.begin(login_a, 'slack'), second.begin(login_b, 'slack'))
            native.gates['retire'].clear()
            current_a['value'] = False
            route.revoke(login_a)
            with pytest.raises(ServiceLoginUnavailable):
                await first.observe(login_a, 'slack')
            retirement = asyncio.create_task(route.retire(login_a))
            await until(lambda: len(native.matching('retire_console_login')) == 1)
            retirement.cancel()
            await asyncio.sleep(.01)
            assert not retirement.done() and not first.retired
            assert (await second.observe(login_b, 'slack')).phase == 'authorized'
            assert not second.retired
            native.gates['retire'].set()
            with pytest.raises(asyncio.CancelledError):
                await retirement
            assert first.retired and not second.retired
            await route.retire(login_a)  # Joins the same completed cleanup.
            assert len(native.matching('retire_console_login')) == 1
            assert not route.retired
        finally:
            native.gates['retire'].set()
            if retirement is not None:
                await asyncio.gather(retirement, return_exceptions=True)
            await route.shutdown()
        assert len(native.matching('retire_console_login')) == 2
    asyncio.run(exercise())


def test_route_refuses_duplicate_native_scope_before_second_enrollment(sdk, native, monkeypatch):
    from launchkit import server

    async def exercise():
        host, login, _ = make(sdk, native)
        duplicate, _, _ = make(sdk, native)
        resolver = lambda request: login
        monkeypatch.setattr(server, 'service_login_route', None)
        route = server.select_service_login_host(host, resolver)
        try:
            with pytest.raises(ServiceLoginUnavailable):
                server.select_service_login_host(duplicate, resolver)
            await host.begin(login, 'slack')
            await route.retire(login)
            with pytest.raises(ServiceLoginUnavailable):
                server.select_service_login_host(duplicate, resolver)
            assert duplicate._loop is None
            assert len(native.matching('enroll_console_login')) == 1
        finally:
            await route.shutdown()
    asyncio.run(exercise())


@pytest.mark.parametrize('bad_cancel', [False, True])
def test_route_shutdown_fences_and_joins_every_owner_after_cancellation(sdk, native, monkeypatch, bad_cancel):
    from launchkit import server

    async def exercise():
        first, login_a, _ = make(sdk, native, scope='browser-a')
        second, login_b, _ = make(sdk, native, scope='browser-b')
        third, _, _ = make(sdk, native, scope='browser-c')
        resolver = lambda request: login_a
        monkeypatch.setattr(server, 'service_login_route', None)
        route = server.select_service_login_host(first, resolver)
        server.select_service_login_host(second, resolver)
        await asyncio.gather(first.begin(login_a, 'slack'), second.begin(login_b, 'slack'))
        native.bad_cancel = bad_cancel
        native.gates['retire'].clear()
        shutdown = asyncio.create_task(route.shutdown())
        try:
            await until(lambda: len(native.matching('retire_console_login')) == 2)
            shutdown.cancel()
            await asyncio.sleep(.01)
            assert not shutdown.done()
            for host, login in ((first, login_a), (second, login_b)):
                with pytest.raises(ServiceLoginUnavailable):
                    await host.observe(login, 'slack')
            with pytest.raises(ServiceLoginUnavailable):
                server.select_service_login_host(third, resolver)
        finally:
            native.gates['retire'].set()
            result = await asyncio.gather(shutdown, return_exceptions=True)
        assert isinstance(result[0], ServiceLoginUnavailable if bad_cancel else asyncio.CancelledError)
        assert route.retired is (not bad_cancel)
        assert first.retired is (not bad_cancel) and second.retired is (not bad_cancel)
        assert len(native.matching('cancel')) == len(native.matching('release')) == 2
        if bad_cancel:
            with pytest.raises(ServiceLoginUnavailable):
                await route.shutdown()
        else:
            await route.shutdown()
        assert len(native.matching('retire_console_login')) == 2
        assert third._loop is None
    asyncio.run(exercise())
