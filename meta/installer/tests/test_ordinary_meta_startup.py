"""Ordinary source/configuration seam with the actual Meta SDK configuration.

Gateway and compiled-policy files are inert specimens. Only the connection
tests replace MetaRuntime with a bounded fixture; no native, model, compiler,
authentication, Console or installed application success is established.
"""
from types import SimpleNamespace
import hashlib

import pytest

from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.ordinary_meta_startup import OrdinaryMetaStartup
from meerkat_meta.builder import MetaBuilderConfig


def pin(path, content, mode=0o600):
    path.write_bytes(content)
    path.chmod(mode)
    return ExactFile(path, hashlib.sha256(content).hexdigest())


@pytest.fixture
def case(tmp_path):
    tmp_path.chmod(0o700)
    (tmp_path / 'config').mkdir(mode=0o700)
    f = SimpleNamespace(root=tmp_path, current=True, events=[])

    def current():
        if not f.current:
            raise CustodyError('original source owner is unavailable')

    async def asynchronous():
        current()

    f.lease = SimpleNamespace(root=tmp_path, require_current=current)
    f.source = SimpleNamespace(receipt={'destination': str(tmp_path)},
        require_owner_current=current, require_current=current,
        require_filesystem_current=current, require_current_async=asynchronous)
    f.gateway = pin(tmp_path / 'ordinary-rpc', b'inert executable specimen\n', 0o700)
    f.definition = pin(tmp_path / 'config/mob.toml', b'[mob]\nid="ordinary-test"\n', 0o644)
    f.policy = pin(tmp_path / 'compiled-policy', b'{"inert_policy_specimen":true}\n')
    f.app = SimpleNamespace(context=SimpleNamespace(root=tmp_path, runtime=None),
        dynamic=False, _application_callbacks=None,
        bind_schedule_runnables=lambda runtime: f.events.append('bind'))
    f.owner = OrdinaryMetaStartup(project_lease=f.lease, accepted_source=f.source,
        gateway=f.gateway, definition=f.definition, compiled_policies=(f.policy,),
        startup_timeout=30)
    return f


async def configure(f, **overrides):
    await f.owner.prepare(f.app)
    return f.owner.builder_config(**{
        'persistent_state': str(f.root / '.mobkit/state'),
        'roster_provider': f.app, 'topology_provider': f.app,
        'session_builder': object(), 'http_listen': '127.0.0.1:0',
        'console_require_app_auth': True, **overrides})


@pytest.mark.asyncio
async def test_actual_meta_config_retains_exact_inputs_without_toolkit_or_helper(case):
    f = case
    before = sorted(p.relative_to(f.root).as_posix() for p in f.root.rglob('*'))
    config = await configure(f)
    assert type(config) is MetaBuilderConfig
    assert config.mob_config_inline.encode() == f.definition.path.read_bytes()
    assert [value.encode() for value in config.application_tool_policies] == [f.policy.read()]
    assert config.prepared_application_bundle is None and config.prepared_application_digest is None
    assert config.session_builder is not None and config.console_require_app_auth is True
    assert f.app._application_callbacks is None and f.app.context.runtime is None
    assert not (f.root / 'shared').exists()
    assert sorted(p.relative_to(f.root).as_posix() for p in f.root.rglob('*')) == before


@pytest.mark.asyncio
async def test_absent_compiled_policy_preserves_sdk_optional_policy_semantics(case):
    f = case
    f.owner = OrdinaryMetaStartup(project_lease=f.lease, accepted_source=f.source,
        gateway=f.gateway, definition=f.definition, compiled_policies=(), startup_timeout=30)
    config = await configure(f)
    assert config.application_tool_policies is None
    assert f.app._application_callbacks is None and f.app.context.runtime is None


@pytest.mark.asyncio
async def test_actual_meta_runtime_store_provider_is_retained(case):
    store = object()
    config = await configure(case, runtime_store=store)
    assert config.runtime_store is store
    case.owner._check_config(config, case.app)


@pytest.mark.asyncio
async def test_dynamic_roster_refuses_an_ordinary_start_first_with_its_own_text(case):
    """A dynamic agent roster cannot start as an ordinary Meta application.

    The refusal names it ahead of the generic fixed-application refusal, which
    the callbacks set here would also raise. No runtime is selected.
    """
    f = case
    f.app.dynamic = True
    f.app._application_callbacks = object()
    with pytest.raises(CustodyError) as refused:
        await f.owner.prepare(f.app)
    text = str(refused.value)
    assert 'dynamic agent roster' in text and 'fixed roster' in text, text
    from launchkit.connect_application_startup import DYNAMIC_ROSTER_UNSUPPORTED
    assert text == DYNAMIC_ROSTER_UNSUPPORTED
    assert f.owner.runtime is None and f.events == [] and f.app.context.runtime is None


@pytest.mark.asyncio
@pytest.mark.parametrize('change', ['writable', 'symlink', 'hardlink', 'fifo'])
async def test_public_definition_modes_do_not_admit_unsafe_source(case, change):
    f = case
    path = f.definition.path
    if change == 'writable':
        path.chmod(0o666)
    elif change == 'hardlink':
        import os
        os.link(path, path.with_name('second-link'))
    else:
        original = path.with_name('original')
        path.rename(original)
        if change == 'symlink':
            path.symlink_to(original)
        else:
            import os
            os.mkfifo(path)
    with pytest.raises(CustodyError):
        await f.owner.prepare(f.app)
    assert f.owner.runtime is None


@pytest.mark.asyncio
@pytest.mark.parametrize('field,value', [
    ('mob_config_path', '/another/mob.toml'), ('mob_config_inline', '[mob]'),
    ('prepared_application_bundle', b'inert'), ('prepared_application_digest', 'another'),
    ('application_tool_policies', ['another']), ('gateway_bin', '/another/gateway'),
    ('persistent_state', '/another/state'), ('declare_spec_update_expected_revision', 2),
    ('roster_provider', object()), ('topology_provider', object()),
    ('meerkat_config_path', '/another/host-config'),
])
async def test_competing_selected_inputs_refused_before_runtime(case, field, value):
    with pytest.raises(CustodyError):
        await configure(case, **{field: value})
    assert case.owner.runtime is None and case.app.context.runtime is None


@pytest.mark.asyncio
async def test_managed_model_requires_original_selected_configuration(case):
    with pytest.raises(CustodyError, match='original trusted startup model'):
        case.owner.require_model_configuration({'classes': [{'model_auth': 'managed_host'}]})


@pytest.mark.asyncio
@pytest.mark.parametrize('change', ['definition', 'policy', 'gateway', 'lost-source', 'changed-root'])
async def test_source_and_owner_changes_refuse_before_native_construction(case, monkeypatch, change):
    f = case
    config = await configure(f)
    if change in {'definition', 'policy', 'gateway'}:
        getattr(f, change).path.write_bytes(b'changed source')
    elif change == 'lost-source':
        f.current = False
    else:
        f.lease.root = f.root / 'replacement'
    import meerkat_meta.runtime
    monkeypatch.setattr(meerkat_meta.runtime, 'MetaRuntime', lambda config: pytest.fail('No native constructor expected'))
    with pytest.raises(CustodyError):
        await f.owner.connect(config, f.app)
    assert f.owner.runtime is None and f.app.context.runtime is None


def runtime_fixture(f, monkeypatch, *, fail=False, change=None):
    class Runtime:
        def __init__(self, config):
            self.config = config
            f.events.append('construct')

        async def connect(self):
            f.events.append('connect')
            if change is not None:
                change()
            if fail:
                raise RuntimeError('bounded SDK failure specimen')

        async def shutdown(self):
            f.events.append('shutdown')

    import meerkat_meta.runtime
    monkeypatch.setattr(meerkat_meta.runtime, 'MetaRuntime', Runtime)
    return Runtime


@pytest.mark.asyncio
async def test_one_connected_owner_and_idempotent_joined_shutdown(case, monkeypatch):
    f = case
    config = await configure(f)
    Runtime = runtime_fixture(f, monkeypatch)
    runtime = await f.owner.connect(config, f.app)
    assert type(runtime) is Runtime and runtime is f.owner.runtime is f.app.context.runtime
    with pytest.raises(CustodyError, match='already entered'):
        await f.owner.connect(config, f.app)
    await f.owner.shutdown(runtime)
    await f.owner.shutdown(runtime)
    assert f.events == ['construct', 'bind', 'connect', 'shutdown']
    with pytest.raises(CustodyError, match='original runtime'):
        await f.owner.shutdown(object())


@pytest.mark.asyncio
@pytest.mark.parametrize('change', ['failure', 'source-changed'])
async def test_failed_start_settles_same_runtime_without_retry(case, monkeypatch, change):
    f = case
    config = await configure(f)
    runtime_fixture(f, monkeypatch, fail=change == 'failure',
        change=(lambda: f.policy.path.write_bytes(b'changed source')) if change == 'source-changed' else None)
    with pytest.raises((RuntimeError, CustodyError)):
        await f.owner.connect(config, f.app)
    assert f.app.context.runtime is None
    assert f.events == ['construct', 'bind', 'connect', 'shutdown']
    with pytest.raises(CustodyError, match='already entered'):
        await f.owner.connect(config, f.app)


@pytest.mark.asyncio
@pytest.mark.parametrize('field,value', [
    ('session_builder', object()), ('runtime_store', object()), ('agent_customizer', object()),
    ('auth_config', object()), ('http_listen', '0.0.0.0:9999'),
    ('console_require_app_auth', False), ('meerkat_config_path', '/another/config'),
])
async def test_changed_config_cannot_replace_original_providers_or_http_selection(case, monkeypatch, field, value):
    f = case
    config = await configure(f)
    setattr(config, field, value)
    import meerkat_meta.runtime
    monkeypatch.setattr(meerkat_meta.runtime, 'MetaRuntime', lambda config: pytest.fail('No native constructor expected'))
    with pytest.raises(CustodyError):
        await f.owner.connect(config, f.app)
    assert f.owner.runtime is None


@pytest.mark.asyncio
async def test_provider_change_during_startup_settles_the_original_runtime(case, monkeypatch):
    f = case
    config = await configure(f)
    runtime_fixture(f, monkeypatch, change=lambda: setattr(config, 'session_builder', object()))
    with pytest.raises(CustodyError):
        await f.owner.connect(config, f.app)
    assert f.app.context.runtime is None
    assert f.events == ['construct', 'bind', 'connect', 'shutdown']
