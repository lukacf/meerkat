"""Source-only regressions for the packaged native declaration boundary.

These fakes exercise configuration and shutdown, not a collector, native grant,
provider call, subscription expansion or delivered announcement.
"""
import copy
import json
from types import SimpleNamespace

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import continuation_contract, generate_files, maintained_host_export
from launchkit.generator import PackageFile
from launchkit.templates_v2 import announcement_configuration as configuration
from test_application_extension import load_host


def write_declarations(root, value):
    path = root / configuration.DECLARATIONS_PATH
    path.parent.mkdir(exist_ok=True)
    path.write_text(json.dumps(value))
    return path


def declarations():
    return {"revision": 7, "sources": [], "subscriptions": [
        {"subscription_id": "updates/review", "members": {
            "mob_id": "app", "role": "application__review"},
         "topics": ["project-changes"], "start": {"kind": "now"}},
        {"subscription_id": "updates/seed", "member": {
            "mob_id": "app", "identity": "review:main"},
         "topics": ["project-changes"], "start": {"kind": "now"}},
    ]}


@pytest.mark.parametrize("meta", [False, True])
def test_absent_declarations_do_not_enable_announcements(tmp_path, meta):
    assert configuration.announcement_options(tmp_path, meta=meta) == {}


def test_native_role_templates_and_revision_are_captured_without_roster_expansion(tmp_path):
    value = declarations()
    path = write_declarations(tmp_path, value)
    options = configuration.announcement_options(tmp_path, meta=True)
    assert options == {"mob_announcements": {"enabled": True, "declarations": value}}
    assert "declarations_path" not in options["mob_announcements"]
    # Changes to the sidecar affect the next start, not this captured start.
    successor = {"revision": 8, "sources": [], "subscriptions": []}
    path.write_text(json.dumps(successor))
    assert options["mob_announcements"]["declarations"] == value
    assert configuration.announcement_options(tmp_path, meta=True)["mob_announcements"]["declarations"] == successor


def test_legacy_runtime_refuses_present_source_instead_of_silently_ignoring_it(tmp_path):
    write_declarations(tmp_path, declarations())
    with pytest.raises(ValueError, match="selected Meta runtime"):
        configuration.announcement_options(tmp_path, meta=False)


@pytest.mark.parametrize("text", [
    '{"revision":7,"revision":8}',
    '{"revision":7,"subscriptions":[{"member":{"identity":"first","identity":"second"}}]}',
    '{"revision":NaN}', '{"revision":Infinity}', '[]',
])
def test_ambiguous_or_non_json_source_is_refused_before_native_start(tmp_path, text):
    path = write_declarations(tmp_path, {})
    path.write_text(text)
    with pytest.raises(ValueError):
        configuration.announcement_options(tmp_path, meta=True)


@pytest.mark.parametrize("kind", ["file", "broken", "parent"])
def test_sidecar_cannot_escape_through_symlinks(tmp_path, kind):
    root = tmp_path / "package"
    root.mkdir()
    external = tmp_path / "external"
    external.mkdir()
    target = external / "announcement-declarations.json"
    target.write_text(json.dumps(declarations()))
    if kind == "parent":
        (root / "application").symlink_to(external, target_is_directory=True)
    else:
        (root / "application").mkdir()
        (root / configuration.DECLARATIONS_PATH).symlink_to(target if kind == "file" else external / "missing")
    with pytest.raises(ValueError, match="ordinary packaged file"):
        configuration.announcement_options(root, meta=True)


def test_oversized_sidecar_is_refused_without_truncating_it(tmp_path, monkeypatch):
    write_declarations(tmp_path, declarations())
    monkeypatch.setattr(configuration, "MAX_DECLARATIONS_BYTES", 4)
    with pytest.raises(ValueError, match="package file limit"):
        configuration.announcement_options(tmp_path, meta=True)


class NativeHandle:
    def __init__(self, features=(), revision=7, records=()):
        self.features, self.revision, self.records = features, revision, records
        self.reads = []

    async def capabilities(self):
        self.reads.append("capabilities")
        return SimpleNamespace(feature_capabilities=self.features,
            methods=["meta/mob_announcements/announce"])

    async def announcement_declarations_get(self):
        self.reads.append("declarations")
        return SimpleNamespace(revision=self.revision, records=self.records)


@pytest.mark.asyncio
async def test_old_plain_announcements_do_not_qualify_as_source_support():
    handle = NativeHandle()
    runtime = SimpleNamespace(mob_handle=lambda: handle)
    config = SimpleNamespace(mob_announcements={"enabled": True, "declarations": declarations()})
    with pytest.raises(RuntimeError, match="does not support incoming"):
        await configuration.require_announcement_sources(runtime, config)
    assert handle.reads == ["capabilities"]


@pytest.mark.asyncio
async def test_wrong_applied_revision_is_refused():
    handle = NativeHandle([configuration.SOURCES_CAPABILITY], revision=6)
    with pytest.raises(RuntimeError, match="did not apply"):
        await configuration.require_announcement_sources(SimpleNamespace(mob_handle=lambda: handle),
            SimpleNamespace(mob_announcements={"declarations": declarations()}))


@pytest.mark.asyncio
async def test_pending_native_records_are_not_replaced_or_claimed_ready():
    records = [{"kind": "role_child", "status": "pending", "refusal": {"kind": "governed_runtime"}}]
    before = copy.deepcopy(records)
    handle = NativeHandle([configuration.SOURCES_CAPABILITY, "future.capability"], records=records)
    await configuration.require_announcement_sources(SimpleNamespace(mob_handle=lambda: handle),
        SimpleNamespace(mob_announcements={"declarations": declarations()}))
    assert handle.records == before
    assert handle.reads == ["capabilities", "declarations"]


@pytest.mark.asyncio
async def test_no_source_selection_requires_no_announcement_methods():
    for options in [None, {"enabled": False}, {"enabled": True}]:
        await configuration.require_announcement_sources(object(), SimpleNamespace(mob_announcements=options))


@pytest.mark.asyncio
@pytest.mark.parametrize("selected_startup", [False, True])
async def test_missing_source_capability_reaps_connected_runtime_before_app_start(tmp_path, monkeypatch, selected_startup):
    module = load_host(monkeypatch, tmp_path)
    native = tmp_path / "config/mob.toml"
    native.parent.mkdir()
    native.write_text('[mob]\nid="app"\n')
    events = []
    handle = NativeHandle()

    class Runtime:
        def __init__(self, config):
            pass

        async def connect(self):
            events.append("connect")

        def mob_handle(self):
            return handle

        async def shutdown(self):
            events.append("shutdown")

    class Application:
        context = SimpleNamespace(root=tmp_path, runtime=None)

        async def load_tool_policy(self, **kwargs):
            pass

        async def require_tool_policy_current(self):
            pass

        def bind_schedule_runnables(self, runtime):
            events.append("bind")

        def _member_mcp_hooks(self):
            return None

    class Startup:
        async def connect(self, config, application):
            runtime = Runtime(config)
            application.context.runtime = runtime
            await runtime.connect()
            return runtime

        async def shutdown(self, runtime):
            await runtime.shutdown()

    import meerkat_meta.runtime
    monkeypatch.setattr(meerkat_meta.runtime, "MetaRuntime", Runtime)
    application = Application()
    config = SimpleNamespace(mob_config_path=str(native),
        mob_announcements={"enabled": True, "declarations": declarations()})
    with pytest.raises(RuntimeError, match="does not support incoming"):
        await module.connect_application_runtime(config, application,
            application_startup=Startup() if selected_startup else None, meta=True)
    assert events == (["connect", "shutdown"] if selected_startup else ["bind", "connect", "shutdown"])
    assert application.context.runtime is None


def test_generated_and_maintained_hosts_ship_their_announcement_module():
    files = generate_files(starter_blueprint())
    assert "announcement_configuration.py" in files
    assert configuration.DECLARATIONS_PATH not in files
    maintained = {item.path: item.content for item in maintained_host_export()}
    assert maintained["announcement_configuration.py"] == files["announcement_configuration.py"]


def test_declaration_changes_cannot_cross_the_source_only_repair_boundary():
    original = PackageFile(configuration.DECLARATIONS_PATH, json.dumps(declarations()))
    changed = PackageFile(configuration.DECLARATIONS_PATH, json.dumps({**declarations(), "revision": 8}))
    assert continuation_contract([original]) != continuation_contract([changed])
    assert continuation_contract([original]) != continuation_contract([])
    assert continuation_contract([original, PackageFile("application/code.py", "old")]) == continuation_contract(
        [original, PackageFile("application/code.py", "new")])
