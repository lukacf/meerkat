"""Role consumer lifecycle with owner/native behavior explicitly fixture-backed.

The private host owner classes must be supplied by the source composition lane.
These controls never launch a native process or claim root credential custody.
"""
import asyncio
import threading
from pathlib import Path
from types import SimpleNamespace
from unittest.mock import Mock
import weakref

import pytest

from launchkit import bootstrap, receiver_sdk, role_runtime as roles
from launchkit.guide_transport import SelectedGuideTransport

host = pytest.importorskip("workspace_runtime_admission_client")
transport = pytest.importorskip("workspace_gateway_transport")


@pytest.fixture(autouse=True)
def reset(monkeypatch):
    monkeypatch.setattr(roles, "_preparations", None)
    monkeypatch.setattr(roles, "_sealed", False)
    monkeypatch.setattr(roles, "_claimed", weakref.WeakSet())


@pytest.fixture
def original(monkeypatch, request):
    from launchkit.protected_startup import SelectedProtectedRuntime
    args = (receiver_sdk.SELECTED_VERSION, receiver_sdk.declared_core_version(),
            "a" * 40, "b" * 40, {"provider": "openai", "model": "gpt-6.1-sol"})
    selected = (SelectedProtectedRuntime(*args) if getattr(request, "param", None) == "metadata"
                else SelectedGuideTransport(Mock(), *args))
    monkeypatch.setattr(bootstrap, "_selected_guide_runtime", selected)
    return selected


def admitted(role="planner", state="/protected/planner/state", *, close=None):
    # Actual nominal types, with no claim to protocol/native evidence here.
    value = object.__new__(host.AdmittedGatewayRuntime)
    value.role, value.persistent_state = role, state
    value.factory = object.__new__(transport.RestrictedGatewayTransportFactory)
    value.check = Mock()
    value.close = close or Mock()
    return value


class Builder:
    def __init__(self, runtime=None):
        self.runtime = runtime or SimpleNamespace(shutdown=Mock())
        self.factories = []
        self.entered, self.release = asyncio.Event(), asyncio.Event()
        self.release.set()
    def gateway_transport(self, factory):
        self.factories.append(factory)
        return self
    async def build(self):
        self.entered.set()
        await self.release.wait()
        if isinstance(self.runtime, BaseException):
            raise self.runtime
        return self.runtime


async def until(predicate):
    async with asyncio.timeout(3):
        while not predicate():
            await asyncio.sleep(.002)


def test_selection_seals_and_cannot_replace():
    callback = Mock()
    manager = roles.select_runtime_preparer(callback)
    assert roles.select_runtime_preparer(callback) is manager
    with pytest.raises(RuntimeError): roles.select_runtime_preparer(Mock())
    assert roles.seal_runtime_preparer() is manager
    with pytest.raises(RuntimeError): roles.select_runtime_preparer(Mock())


@pytest.mark.asyncio
async def test_missing_hook_cannot_consume_global_factory(original):
    with pytest.raises(RuntimeError, match="per-role preparer"):
        await roles.prepare_runtime("planner", original, None, [])
    original.factory.assert_not_called()
    roles.seal_runtime_preparer()
    with pytest.raises(RuntimeError): roles.select_runtime_preparer(Mock())


@pytest.mark.asyncio
async def test_fresh_architect_planner_and_refresh_keep_startup_route(original, tmp_path):
    values, calls, cleanup = [], [], []
    def prepare(**kw):
        calls.append((kw, threading.get_ident()))
        value = admitted(kw['role'], '/protected/' + str(len(values)) + '/state')
        values.append(value)
        return value
    manager = roles.select_runtime_preparer(prepare)
    for role in ['architect', 'planner', 'architect']:
        owner = await roles.prepare_runtime(role, original, tmp_path / 'legacy', cleanup)
        assert owner.selected.model_configuration == original.model_configuration
        assert owner.selected.meta_source == original.meta_source
        assert owner.persistent_state == values[-1].persistent_state
        assert owner.selected.factory is values[-1].factory
    assert len({id(v.factory) for v in values}) == 3
    assert all(kw['timeout'] == 60 and thread != threading.get_ident() for kw, thread in calls)
    assert bootstrap.selected_guide_runtime() is original
    assert not list(tmp_path.iterdir())
    original.factory.assert_not_called()
    await manager.join()
    assert all(v.close.call_count == 1 for v in values)


@pytest.mark.asyncio
@pytest.mark.parametrize('kind', ['file', 'directory', 'symlink'])
@pytest.mark.parametrize("original", ["factory", "metadata"], indirect=True)
async def test_legacy_conflict_preserved_and_original_closed(original, tmp_path, kind):
    path = tmp_path / 'legacy'
    if kind == 'file': path.write_bytes(b'original')
    elif kind == 'directory': path.mkdir()
    else: path.symlink_to(tmp_path / 'absent')
    before = path.lstat()
    value = admitted('architect')
    manager = roles.select_runtime_preparer(lambda **_: value)
    with pytest.raises(bootstrap.GatewayInstallError, match='saved state is preserved'):
        await roles.prepare_runtime('architect', original, path, [])
    assert path.lstat() == before
    await manager.join()
    value.close.assert_called_once()


@pytest.mark.asyncio
@pytest.mark.parametrize("original", ["factory", "metadata"], indirect=True)
async def test_unreadable_legacy_refuses_without_assuming_absence(original, monkeypatch):
    value = admitted('architect')
    manager = roles.select_runtime_preparer(lambda **_: value)
    def unreadable(_): raise PermissionError('fixture unreadable')
    monkeypatch.setattr(Path, 'lstat', unreadable)
    with pytest.raises(PermissionError):
        await roles.prepare_runtime('architect', original, '/legacy/state', [])
    await manager.join()
    value.close.assert_called_once()


@pytest.mark.asyncio
@pytest.mark.parametrize("original", ["factory", "metadata"], indirect=True)
async def test_cancelled_admission_joins_late_original_before_close(original):
    entered, release = threading.Event(), threading.Event()
    value, cleanup = admitted(), []
    def prepare(**_):
        entered.set()
        assert release.wait(3)
        return value
    manager = roles.select_runtime_preparer(prepare)
    observer = asyncio.create_task(roles.prepare_runtime('planner', original, None, cleanup))
    await until(entered.is_set)
    observer.cancel()
    with pytest.raises(asyncio.CancelledError): await observer
    join = asyncio.create_task(manager.join())
    await asyncio.sleep(.01)
    assert not join.done() and not value.close.called
    release.set()
    await join
    assert cleanup[0].prepare_task.done()
    value.close.assert_called_once()


@pytest.mark.asyncio
@pytest.mark.parametrize("original", ["factory", "metadata"], indirect=True)
async def test_cancelled_close_observer_retains_same_cleanup_and_http_join(original):
    entered, release = threading.Event(), threading.Event()
    calls = []
    def close():
        calls.append(threading.get_ident()); entered.set()
        assert release.wait(3)
    value = admitted(close=close)
    manager = roles.select_runtime_preparer(lambda **_: value)
    owner = await roles.prepare_runtime('planner', original, None, [])
    observer = asyncio.create_task(owner.shutdown())
    await until(entered.is_set)
    observer.cancel()
    with pytest.raises(asyncio.CancelledError): await observer
    final = asyncio.create_task(manager.join())
    await asyncio.sleep(.01)
    final.cancel()
    await asyncio.sleep(.01)
    assert not final.done()
    release.set()
    with pytest.raises(BaseExceptionGroup) as error: await final
    assert any(isinstance(e, asyncio.CancelledError) for e in error.value.exceptions)
    assert len(calls) == 1 and calls[0] != threading.get_ident()
    assert owner.stop_task.done() and manager.stop_task.done()


@pytest.mark.asyncio
@pytest.mark.parametrize("original", ["factory", "metadata"], indirect=True)
async def test_cancelled_build_is_joined_then_runtime_and_channel_close(original):
    events, cleanup = [], []
    value = admitted(close=lambda: events.append('channel'))
    manager = roles.select_runtime_preparer(lambda **_: value)
    owner = await roles.prepare_runtime('planner', original, None, cleanup)
    async def shutdown(): events.append('runtime')
    runtime = SimpleNamespace(shutdown=shutdown)
    builder = Builder(runtime); builder.release.clear()
    observer = asyncio.create_task(owner.build(builder))
    await builder.entered.wait(); observer.cancel()
    with pytest.raises(asyncio.CancelledError): await observer
    closing = asyncio.create_task(manager.join())
    await asyncio.sleep(.01)
    assert not closing.done() and events == []
    builder.release.set(); await closing
    assert events == ['runtime', 'channel']
    assert builder.factories == [value.factory]
    with pytest.raises(RuntimeError): await owner.build(Builder(runtime))


@pytest.mark.asyncio
async def test_sdk_and_original_cleanup_failures_both_retained(original):
    async def stop(): raise ValueError('fixture SDK cleanup')
    def close(): raise RuntimeError('fixture unobserved native stop after local cleanup')
    value = admitted(close=close)
    manager = roles.select_runtime_preparer(lambda **_: value)
    cleanup = []
    owner = await roles.prepare_runtime('planner', original, None, cleanup)
    runtime = await owner.build(Builder(SimpleNamespace(shutdown=stop)))
    with pytest.raises(BaseExceptionGroup) as failure:
        await roles.shutdown_runtime(runtime, cleanup)
    assert len(failure.value.exceptions) == 2
    with pytest.raises(BaseExceptionGroup) as repeated: await owner.shutdown()
    assert repeated.value is failure.value
    assert manager.owners == [owner]


@pytest.mark.asyncio
@pytest.mark.parametrize('change', ['role', 'factory', 'type'])
async def test_invalid_owner_refused_and_only_actual_owner_closed(original, change):
    value = admitted()
    if change == 'role': value.role = 'architect'
    elif change == 'factory': value.factory = Mock()
    else: value = SimpleNamespace(role='planner', close=Mock())
    manager = roles.select_runtime_preparer(lambda **_: value)
    with pytest.raises((TypeError, RuntimeError)):
        await roles.prepare_runtime('planner', original, None, [])
    await manager.join()
    assert value.close.call_count == (0 if change == 'type' else 1)


@pytest.mark.asyncio
async def test_replayed_original_cannot_be_consumed_or_closed_twice(original):
    value = admitted()
    manager = roles.select_runtime_preparer(lambda **_: value)
    first = await roles.prepare_runtime('planner', original, None, [])
    with pytest.raises(RuntimeError, match='one consumer'):
        await roles.prepare_runtime('planner', original, None, [])
    await manager.join()
    value.close.assert_called_once()
    with pytest.raises(RuntimeError, match='closed'):
        await roles.prepare_runtime('planner', original, None, [])


@pytest.mark.asyncio
async def test_critic_refuses_without_allocating_or_reusing_guide(original, tmp_path):
    from launchkit.visual_review_runtime import VisualReviewRuntime
    prepare = Mock()
    roles.select_runtime_preparer(prepare)
    with pytest.raises(ValueError, match='generation contract'):
        await roles.prepare_runtime('critic', original, tmp_path, [])
    critic = VisualReviewRuntime(tmp_path / 'critic'); critic.reviews = object()
    with pytest.raises(ValueError, match='unavailable'): await critic.get_runtime()
    prepare.assert_not_called(); original.factory.assert_not_called()


@pytest.mark.asyncio
@pytest.mark.parametrize("original", ["factory", "metadata"], indirect=True)
async def test_planner_requests_select_state_before_builder_without_temporary_path(original, monkeypatch):
    from meerkat_meta import Meta
    from launchkit import planner
    states, factories, values = [], [], []
    def prepare(**kw):
        value = admitted(kw['role'], '/protected/planner-' + str(len(values)) + '/state')
        values.append(value)
        return value
    manager = roles.select_runtime_preparer(prepare)
    async def send(value): return SimpleNamespace(completion_baseline='original baseline')
    async def completed(baseline, **_):
        import json
        assert baseline == 'original baseline'
        result = planner.starter_design(planner.DesignRequest(description='Organize household project notes.'))
        return json.dumps({'agents': [a.model_dump() for a in result.agents], 'summary': result.summary})
    async def stop(): pass
    runtime = SimpleNamespace(shutdown=stop,
        agent=lambda _: SimpleNamespace(send=send, wait_for_completion=completed))
    class PlanningBuilder(Builder):
        def persistent_state(self, path): states.append(path); return self
        def gateway_transport(self, factory):
            assert states[-1] == values[-1].persistent_state
            factories.append(factory); return self
        def __getattr__(self, _): return lambda *a, **k: self
    monkeypatch.setattr(Meta, 'builder', lambda: PlanningBuilder(runtime))
    monkeypatch.setattr(planner.tempfile, 'TemporaryDirectory', Mock(side_effect=AssertionError('no temporary competing store')))
    for _ in range(2):
        result = await planner.design_team(planner.DesignRequest(description='Organize household project notes.'))
        assert result.source == 'agent'
    assert states == [v.persistent_state for v in values]
    assert factories == [v.factory for v in values]
    assert all(v.close.call_count == 1 for v in values)
    assert manager.owners == []
    if type(original) is SelectedGuideTransport:
        original.factory.assert_not_called()
    else:
        assert not hasattr(original, "factory")


# Reuse only the existing local startup fixture; no upstream owner tests are collected.
from test_architect_startup_recovery import startup


@pytest.mark.asyncio
@pytest.mark.parametrize("original", ["factory", "metadata"], indirect=True)
async def test_architect_refresh_uses_new_owner_and_explicit_state(startup, original, monkeypatch):
    import meerkat_meta
    import test_architect_startup_recovery as fixture
    service, controller = startup
    monkeypatch.setattr(bootstrap, '_selected_guide_runtime', original)
    values, states, factories = [], [], []
    def prepare(**kw):
        value = admitted(kw['role'], '/protected/architect-' + str(len(values)) + '/state')
        values.append(value); return value
    manager = roles.select_runtime_preparer(prepare)
    previous_builder = meerkat_meta.Meta.builder
    def builder():
        current = previous_builder()
        def state(path): states.append(path); return current
        def factory(value):
            assert states[-1] == values[-1].persistent_state
            factories.append(value); return current
        current.persistent_state, current.gateway_transport = state, factory
        return current
    monkeypatch.setattr(meerkat_meta.Meta, 'builder', builder)
    for _ in range(2):
        runtime = fixture.FakeRuntime(); controller.outcomes.append(runtime)
        await service.start()
        assert service.runtime is runtime
        assert service.setup_runtime_selection is original
        await service.shutdown()
        assert runtime.shutdown_calls == 1
    assert states == [v.persistent_state for v in values]
    assert factories == [v.factory for v in values]
    assert controller.gateways == []
    assert manager.owners == []
    if type(original) is SelectedGuideTransport:
        original.factory.assert_not_called()
    else:
        assert not hasattr(original, "factory")


@pytest.mark.asyncio
async def test_runtime_admission_cannot_move_to_foreign_loop(original):
    manager = roles.select_runtime_preparer(Mock())
    manager.check()
    def foreign(): return asyncio.run(manager.shutdown())
    with pytest.raises(RuntimeError, match='original event loop'):
        await asyncio.to_thread(foreign)
    assert manager.stop_task is None


@pytest.mark.asyncio
async def test_runtime_admission_refuses_foreign_process_before_shutdown(monkeypatch):
    manager = roles.select_runtime_preparer(Mock())
    monkeypatch.setattr(roles.os, 'getpid', lambda: manager.pid + 1)
    with pytest.raises(RuntimeError, match='original process'):
        await manager.shutdown()
    assert manager.stop_task is None


@pytest.mark.asyncio
async def test_actual_http_lifespan_fences_and_joins_admission_even_after_other_stop_failure(monkeypatch):
    import ast
    from contextlib import asynccontextmanager, nullcontext
    import launchkit
    source = Path(launchkit.__path__[0]) / 'server.py'
    tree = ast.parse(source.read_bytes(), str(source))
    node = next(n for n in tree.body if isinstance(n, ast.AsyncFunctionDef) and n.name == 'lifespan')
    events = []
    async def join(): events.append('joined')
    manager = SimpleNamespace(begin_shutdown=lambda: events.append('fenced'), join=join)
    monkeypatch.setattr(roles, 'seal_runtime_preparer', lambda: manager)
    async def stop_owners(owners):
        events.append('other owners stopped')
        return [('fixture other owner', ValueError('original other-owner failure'))]
    async def reconcile(): return True
    namespace = {'__package__': 'launchkit', 'asynccontextmanager': asynccontextmanager,
        'runtime_diagnostics': nullcontext, 'provider_settings': SimpleNamespace(restore=lambda: None),
        'visual_review_runtime': SimpleNamespace(reconcile_terminated=reconcile),
        'stop_owners': stop_owners, 'Owner': lambda *a, **k: a,
        'provider_setup_route': None, 'service_login_route': None}
    for name in ['guide_operations', 'visualization_operations', 'visualization_reviews',
                 'remote_deployment', 'gcp_provisioning', 'gcp_preflight', 'deployment',
                 'rehearsal', 'implementation', 'checker_setup', 'toolkit_saved_requests', 'architect']:
        namespace[name] = object()
    exec(compile(ast.Module(body=[node], type_ignores=[]), str(source), 'exec'), namespace)
    with pytest.raises(BaseExceptionGroup) as failure:
        async with namespace['lifespan'](None): events.append('serving')
    assert events == ['serving', 'fenced', 'other owners stopped', 'joined']
    assert str(failure.value.exceptions[0]) == 'original other-owner failure'
