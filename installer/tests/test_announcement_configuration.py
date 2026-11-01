"""Source-only regressions for the packaged native declaration boundary.

These fakes exercise configuration and shutdown, not a collector, native grant,
provider call, subscription expansion or delivered announcement.
"""
import asyncio
import copy
import hashlib
import json
import os
from types import SimpleNamespace

import pytest

from launchkit.blueprint import starter_blueprint
from launchkit.compiler import DECLARATIONS_PATH, generate_files, maintained_host_export
from test_application_extension import load_host


@pytest.fixture
def configuration(monkeypatch, tmp_path):
    return load_host(monkeypatch, tmp_path)


def write_declarations(root, value):
    path = root / DECLARATIONS_PATH
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
def test_absent_declarations_do_not_enable_announcements(tmp_path, meta, configuration):
    assert configuration.announcement_options(tmp_path, meta=meta) == {}


def test_native_role_templates_and_revision_are_captured_without_roster_expansion(tmp_path, configuration):
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


def test_legacy_runtime_refuses_present_source_instead_of_silently_ignoring_it(tmp_path, configuration):
    write_declarations(tmp_path, declarations())
    with pytest.raises(ValueError, match="selected Meta runtime"):
        configuration.announcement_options(tmp_path, meta=False)


@pytest.mark.parametrize("text", [
    '{"revision":7,"revision":8}',
    '{"revision":7,"subscriptions":[{"member":{"identity":"first","identity":"second"}}]}',
    '{"revision":NaN}', '{"revision":Infinity}', '{"revision":1e400}', '[]',
])
def test_ambiguous_or_non_json_source_is_refused_before_native_start(tmp_path, text, configuration):
    path = write_declarations(tmp_path, {})
    path.write_text(text)
    with pytest.raises(ValueError):
        configuration.announcement_options(tmp_path, meta=True)


@pytest.mark.parametrize("kind", ["file", "broken", "parent"])
def test_sidecar_cannot_escape_through_symlinks(tmp_path, kind, configuration):
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
        (root / DECLARATIONS_PATH).symlink_to(target if kind == "file" else external / "missing")
    with pytest.raises(ValueError, match="ordinary packaged file"):
        configuration.announcement_options(root, meta=True)


def test_oversized_sidecar_is_refused_without_truncating_it(tmp_path, monkeypatch, configuration):
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

    async def announcement_declarations_apply(self, value):
        self.reads.append("apply")
        assert value == declarations()
        return SimpleNamespace(revision=self.revision, records=self.records)


@pytest.mark.asyncio
async def test_old_plain_announcements_do_not_qualify_as_source_support(configuration):
    handle = NativeHandle()
    runtime = SimpleNamespace(mob_handle=lambda: handle)
    config = SimpleNamespace(mob_announcements={"enabled": True, "declarations": declarations()})
    with pytest.raises(RuntimeError, match="does not support incoming"):
        await configuration.require_announcement_sources(runtime, config)
    assert handle.reads == ["capabilities"]


@pytest.mark.asyncio
async def test_wrong_applied_revision_is_refused(configuration):
    handle = NativeHandle([configuration.SOURCES_CAPABILITY], revision=6)
    with pytest.raises(RuntimeError, match="did not apply"):
        await configuration.require_announcement_sources(SimpleNamespace(mob_handle=lambda: handle),
            SimpleNamespace(mob_announcements={"declarations": declarations()}), timeout=0.01)


@pytest.mark.asyncio
async def test_pending_native_records_are_not_replaced_or_claimed_ready(configuration):
    records = [{"kind": "role_child", "status": "pending", "refusal": {"kind": "governed_runtime"}}]
    before = copy.deepcopy(records)
    handle = NativeHandle([configuration.SOURCES_CAPABILITY, "future.capability"], records=records)
    await configuration.require_announcement_sources(SimpleNamespace(mob_handle=lambda: handle),
        SimpleNamespace(mob_announcements={"declarations": declarations()}))
    assert handle.records == before
    assert handle.reads == ["capabilities", "declarations", "apply"]


@pytest.mark.asyncio
async def test_no_source_selection_requires_no_announcement_methods(configuration):
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


def test_generated_and_maintained_hosts_keep_the_closed_inventory():
    from launchkit.deployment import package_files
    from launchkit.deployment_sources import compile_native_source, read_package_source
    from test_deployment_native_team import accepted_guide
    files = generate_files(starter_blueprint())
    assert "announcement_configuration.py" not in files
    assert DECLARATIONS_PATH not in files
    maintained = {item.path: item.content for item in maintained_host_export()}
    assert set(maintained) == {"app.py", "console_auth.py", "console_connect.py",
        "console_presentation.py", "console_shell.py", "console_shell.mjs", "console_shell.css"}
    assert maintained["app.py"] == files["app.py"]
    payload, source, _ = compile_native_source(accepted_guide().state, 4)
    assert read_package_source(package_files(payload)) == source


@pytest.mark.parametrize("selected,change", [(selected, change)
    for selected in (False, True) for change in ("write", "batch", "raw", "delete", "adopt")
    if selected or change != "delete"])
def test_real_retained_framework_repair_protects_declarations(tmp_path, selected, change):
    from launchkit.implementation_workspace import ImplementationWorkspace, WorkspaceError
    from test_implementation_workspace import accepted_framework_snapshot
    original = ImplementationWorkspace(tmp_path / "original", starter_blueprint())
    original.seed()
    source = tmp_path / "selected-declarations.json"
    source.write_text(json.dumps(declarations()))
    source.chmod(0o644)
    if selected:
        original.adopt_application_inputs({DECLARATIONS_PATH: source}, require_current=lambda: None)
    snapshot = accepted_framework_snapshot(original)
    repaired = ImplementationWorkspace(tmp_path / "repair", starter_blueprint())
    repaired.seed(snapshot)
    if selected:
        assert repaired.read_file(DECLARATIONS_PATH) == source.read_text()
        assert DECLARATIONS_PATH in repaired.framework_provenance()["protected_application_paths"]
    before = repaired.digest()
    new = json.dumps({**declarations(), "revision": 8})
    with pytest.raises(WorkspaceError):
        if change == "write":
            repaired.write_file(DECLARATIONS_PATH, new)
        elif change == "batch":
            repaired.apply_batch([{"kind": "write", "path": DECLARATIONS_PATH,
                "content": new, "expected_sha256": hashlib.sha256(source.read_bytes()).hexdigest() if selected else None}])
        elif change == "adopt":
            source.write_text(new)
            repaired.adopt_application_inputs({DECLARATIONS_PATH: source}, require_current=lambda: None)
        else:
            path = repaired.project / DECLARATIONS_PATH
            path.parent.mkdir(exist_ok=True)
            if change == "delete" and selected:
                path.unlink()
            else:
                path.write_text(new)
            repaired.export_files(require_verified=False)
    if change in {"write", "batch", "adopt"}:
        assert repaired.digest() == before
        repaired.write_file("application/ordinary.py", "# Application repair remains available.\n")
        assert repaired.digest() != before


@pytest.mark.parametrize("replacement", ["symlink", "fifo", "directory"])
def test_sidecar_swap_between_stat_and_open_refuses_without_following_or_blocking(
        tmp_path, monkeypatch, configuration, replacement):
    path = write_declarations(tmp_path, declarations())
    external = tmp_path / "external.json"
    external.write_text(json.dumps({**declarations(), "revision": 99}))
    real_open = os.open
    swapped = False
    def swap(name, flags, *args, **kwargs):
        nonlocal swapped
        if name == "announcement-declarations.json" and not swapped:
            swapped = True
            assert flags & os.O_NOFOLLOW and flags & os.O_NONBLOCK
            assert "dir_fd" in kwargs
            if replacement == "directory":
                path.parent.rename(tmp_path / "retained-application")
                path.parent.mkdir()
                path.write_text(external.read_text())
            else:
                path.unlink()
                if replacement == "fifo":
                    os.mkfifo(path)
                else:
                    path.symlink_to(external)
        return real_open(name, flags, *args, **kwargs)
    monkeypatch.setattr(configuration.os, "open", swap)
    with pytest.raises(ValueError, match="ordinary packaged file|changed"):
        configuration.announcement_options(tmp_path, meta=True)
    assert swapped and external.read_text() == json.dumps({**declarations(), "revision": 99})


@pytest.mark.asyncio
async def test_gate_waits_for_detached_reconciliation_then_verifies_native_digest(configuration):
    class Delayed(NativeHandle):
        async def announcement_declarations_get(self):
            self.revision = 6 if "declarations" not in self.reads else 7
            return await super().announcement_declarations_get()
    handle = Delayed([configuration.SOURCES_CAPABILITY])
    await configuration.require_announcement_sources(SimpleNamespace(mob_handle=lambda: handle),
        SimpleNamespace(mob_announcements={"declarations": declarations()}))
    assert handle.reads == ["capabilities", "declarations", "declarations", "apply"]


@pytest.mark.asyncio
async def test_newer_native_revision_has_explicit_restore_refusal(configuration):
    handle = NativeHandle([configuration.SOURCES_CAPABILITY], revision=8)
    with pytest.raises(RuntimeError, match="revision 8.*revision 7.*not supported"):
        await configuration.require_announcement_sources(SimpleNamespace(mob_handle=lambda: handle),
            SimpleNamespace(mob_announcements={"declarations": declarations()}))
    assert handle.reads == ["capabilities", "declarations"]


@pytest.mark.asyncio
async def test_native_digest_conflict_is_preserved_without_forge_digest_or_retry(configuration):
    refusal = RuntimeError("declaration_digest_conflict")
    class Conflicting(NativeHandle):
        async def announcement_declarations_apply(self, value):
            await super().announcement_declarations_apply(value)
            raise refusal
    handle = Conflicting([configuration.SOURCES_CAPABILITY])
    with pytest.raises(RuntimeError) as caught:
        await configuration.require_announcement_sources(SimpleNamespace(mob_handle=lambda: handle),
            SimpleNamespace(mob_announcements={"declarations": declarations()}))
    assert caught.value is refusal
    assert handle.reads == ["capabilities", "declarations", "apply"]


@pytest.mark.asyncio
async def test_cancelled_reconciliation_exits_before_native_apply(configuration):
    entered = asyncio.Event()
    class Pending(NativeHandle):
        async def announcement_declarations_get(self):
            entered.set()
            await asyncio.Future()
    handle = Pending([configuration.SOURCES_CAPABILITY])
    task = asyncio.create_task(configuration.require_announcement_sources(
        SimpleNamespace(mob_handle=lambda: handle),
        SimpleNamespace(mob_announcements={"declarations": declarations()})))
    await entered.wait()
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert "apply" not in handle.reads
