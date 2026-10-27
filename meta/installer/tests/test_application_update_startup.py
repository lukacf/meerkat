"""Replacement startup custody with real source leases and inert SDK specimens.

No native validation, member construction, installed acceptance or Ready is
simulated. The imported SDK names below are exclusively test-owned modules.
"""
import asyncio
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
from types import ModuleType, SimpleNamespace

import pytest

from launchkit import connect_application_update_startup as module
from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.templates.storage_setup import project_lock
from test_accepted_source_custody import admitted, retain
from test_application_callbacks import Fixture as CallbackFixture


def write(path, data, mode=0o600):
    path.write_bytes(data)
    path.chmod(mode)
    return ExactFile(path, hashlib.sha256(data).hexdigest())


def content(value):
    return (json.dumps(value, ensure_ascii=True, separators=(",", ":")) + "\n").encode()


@pytest.fixture(autouse=True)
def no_native_operations(monkeypatch):
    def forbidden(*args, **kwargs):
        raise AssertionError("Replacement source controls cannot spawn or signal a process.")
    for name in ("create_subprocess_exec", "create_subprocess_shell"):
        monkeypatch.setattr(asyncio, name, forbidden)
    monkeypatch.setattr(subprocess, "Popen", forbidden)
    monkeypatch.setattr(os, "kill", forbidden)
    monkeypatch.setattr(os, "killpg", forbidden)


class Fixture:
    def __init__(self, root, source, lease, monkeypatch):
        self.root, self.source, self.lease = root, source, lease
        self.events, self.current = [], True
        self.native_outcome = object()
        self.after_connect = lambda: None
        self.connect_error = self.shutdown_error = None
        self.bind_error = None
        private = root / ".mobkit/update-inputs"
        private.mkdir(mode=0o700)
        self.callback_fixture = CallbackFixture(private, native=True)
        self.plan_value = {**self.callback_fixture.value, "deployment_root": str(root),
            "application": {**self.callback_fixture.value["application"], "realm": {
                "realm_id": "selected-realm", "context_root": str(root),
                "state_root": str(root / ".mobkit/state")},
                "helper_tools": {"opaque": "original helper resource"},
                "tool_policy": {"opaque": "native compiler input"}}}
        self.plan = write(self.callback_fixture.path, content(self.plan_value))
        self.compilation = write(private / "compilation.json", content(self.plan_value["application"]))
        self.policy = write(private / "candidate-policy.json", b'{"opaque":"candidate-policy"}\n')
        self.prior = write(private / "actual-prior-policy.json", b'{"opaque":"prior-after-update"}\n')
        self.gateway = write(private / "gateway", b"inert gateway bytes; never execute\n", 0o700)
        bundle = root / "original-bundle"
        bundle.mkdir(mode=0o700)
        self.activation = write(root / "original-activation.json", b'{"opaque":"original activation"}\n')
        self.manifest = write(bundle / "manifest.json", b'{"opaque":"native manifest"}\n')
        self.local_policy = write(bundle / "policy.json", b'{"opaque":"original bootstrap policy"}\n')
        (root / "current").symlink_to(self.activation.path.name)
        self.selected = {"update_id": "same-admitted-update", "predecessor": {
            "deployment_root": str(root), "activation": self.activation.pin(),
            "policy_manifest": self.manifest.pin(), "local_policy": self.local_policy.pin()},
            "compilation_input": self.compilation.pin(), "application_tool_policy": self.policy.pin()}
        self.selection = write(private / "selected-update.json", content(self.selected))
        self.associations = private / "associations"
        self.associations.mkdir(mode=0o700)
        self.app = SimpleNamespace(context=SimpleNamespace(root=root, runtime=None),
            dynamic=False, _application_callbacks=None, bind_schedule_runnables=self.bind)
        self.modules = {}
        for name in ("meerkat_meta", "meerkat_meta.builder", "meerkat_toolkit",
                     "meerkat_toolkit.host_control", "meerkat_toolkit.application_update"):
            self.modules[name] = ModuleType(name)
            monkeypatch.setitem(sys.modules, name, self.modules[name])
        class Config:
            def __init__(self, **options):
                self.__dict__.update(options)
        class Artifact:
            def __init__(self, path, sha256):
                self.path, self.sha256 = path, sha256
        class Startup:
            def __init__(self, selection, prior_tool_policy, association_directory):
                self.selection, self.prior_tool_policy = selection, prior_tool_policy
                self.association_directory = association_directory
        fixture = self
        class Runtime:
            def __init__(self, config, startup):
                fixture.events.append("construct")
                self.config, self.startup = config, startup
                self.closed = False
            async def connect(self):
                fixture.events.append("connect")
                fixture.after_connect()
                if fixture.connect_error is not None:
                    raise fixture.connect_error
                return fixture.native_outcome
            async def shutdown(self):
                fixture.events.append("shutdown")
                if fixture.shutdown_error is not None:
                    raise fixture.shutdown_error
                self.closed = True
        self.modules["meerkat_meta.builder"].MetaBuilderConfig = Config
        self.modules["meerkat_toolkit.host_control"].SelectedHostArtifact = Artifact
        self.modules["meerkat_toolkit.host_control"].ApplicationUpdateStartup = Startup
        self.modules["meerkat_toolkit.application_update"].ToolkitApplicationUpdateRuntime = Runtime
        self.owner = self.make_owner()

    def make_owner(self, **changes):
        values = dict(project_lease=self.lease, accepted_source=self.source,
            gateway=self.gateway, selection_file=self.selection,
            prior_tool_policy=self.prior, preparation_plan=self.plan,
            association_directory=self.associations, expected_revision=7, startup_timeout=60)
        return module.ReplacementApplicationStartup(**{**values, **changes})

    def bind(self, runtime):
        self.events.append("bind")
        assert self.app.context.runtime is runtime
        if self.bind_error is not None:
            raise self.bind_error

    def config(self, **changes):
        return self.owner.builder_config(roster_provider=self.app, topology_provider=self.app,
            persistent_state=str(self.root / ".mobkit/state"), session_builder=object(),
            host_runnables=["original-handler"], **changes)


@pytest.fixture
async def fixture(tmp_path, monkeypatch):
    directory, receipt, root = await admitted(tmp_path,
        extra_files={"assets/user-context.txt": b"actual original accepted source\n"})
    loop_thread = threading.get_ident()
    current = [True]
    def owner_current():
        assert threading.get_ident() == loop_thread
        if not current[0]:
            raise CustodyError("Original admitted update changed.")
    with project_lock(root) as lease:
        source = await retain(directory, receipt, lease, current=owner_current)
        f = Fixture(root, source, lease, monkeypatch)
        f.current_selection = current
        try:
            yield f
        finally:
            source.close()


@pytest.mark.parametrize("revision", [7, None])
async def test_exact_replacement_uses_original_config_callbacks_and_actual_outcome(fixture, revision):
    f = fixture
    f.owner = f.make_owner(expected_revision=revision)
    await f.owner.prepare(f.app)
    config = f.config()
    # The real generated host fills conventional file options after building.
    runtime = await f.owner.connect(config, f.app)
    assert f.events == ["construct", "bind", "connect"]
    assert runtime is f.owner.runtime is f.app.context.runtime
    assert runtime.config is config and config.roster_provider is f.app
    assert f.owner.native_outcome is f.native_outcome
    assert runtime.startup.selection.__dict__ == {"path": str(f.selection.path), "sha256": f.selection.sha256}
    assert runtime.startup.prior_tool_policy.__dict__ == {"path": str(f.prior.path), "sha256": f.prior.sha256}
    assert runtime.startup.association_directory == str(f.associations)
    assert runtime.startup.prior_tool_policy.path != str(f.local_policy.path)
    assert config.declare_spec_update_expected_revision is revision
    assert f.app._application_callbacks is f.owner.callbacks
    f.owner.callbacks.observe_roster_context({"mob_definition": f.callback_fixture.observed})
    assert f.owner.callbacks.helper_identity == "helper:setup"
    assert not list(f.associations.iterdir())
    assert (f.root / "current").readlink().name == f.activation.path.name
    with pytest.raises(CustodyError):
        await f.owner.connect(config, f.app)
    with pytest.raises(CustodyError):
        await f.owner.shutdown(object())
    await f.owner.shutdown(runtime)
    assert runtime.closed and f.events[-1] == "shutdown"


async def test_dynamic_roster_refuses_a_replacement_start_first_with_its_own_text(fixture):
    """A dynamic agent roster cannot take a replacement start.

    The refusal names it ahead of the generic fixed-application refusal, which
    the callbacks set here would also raise. Nothing is constructed or bound.
    """
    f = fixture
    f.app.dynamic = True
    f.app._application_callbacks = object()
    with pytest.raises(CustodyError) as refused:
        await f.owner.prepare(f.app)
    text = str(refused.value)
    assert "dynamic agent roster" in text and "fixed roster" in text, text
    from launchkit.connect_application_startup import DYNAMIC_ROSTER_UNSUPPORTED
    assert text == DYNAMIC_ROSTER_UNSUPPORTED
    assert not f.events and f.app.context.runtime is None and f.owner.callbacks is None


@pytest.mark.asyncio
async def test_replacement_with_ordinary_declarations_keeps_the_typed_refusal(fixture):
    from launchkit.connect_application_startup import DYNAMIC_ROSTER_UNSUPPORTED
    f = fixture
    value = json.loads(f.plan.read())
    value["application"]["member_classes"] = [{"id": "reader", "profile": "reusable-reader",
        "factory": "application:reader", "callbacks": [], "grants": []}]
    f.plan = write(f.plan.path, content(value))
    f.owner = f.make_owner()
    f.app.dynamic = True
    with pytest.raises(CustodyError) as refused:
        await f.owner.prepare(f.app)
    assert str(refused.value) == DYNAMIC_ROSTER_UNSUPPORTED
    assert not f.events


@pytest.mark.parametrize("pin_name", ["selection", "prior", "plan", "compilation", "activation", "gateway"])
async def test_retained_input_changes_refuse_before_sdk_construction(fixture, pin_name):
    f = fixture
    await f.owner.prepare(f.app)
    config = f.config()
    getattr(f, pin_name).path.write_bytes(b"different original input\n")
    with pytest.raises(CustodyError):
        await f.owner.connect(config, f.app)
    assert not f.events and f.app.context.runtime is None


async def test_candidate_callback_plan_must_join_exact_selected_compilation(fixture):
    f = fixture
    candidate = deepcopy(f.plan_value["application"])
    candidate["definition"]["definition"]["profiles"]["reader-source"]["model"] = "other-source"
    f.compilation = write(f.compilation.path, content(candidate))
    f.selected["compilation_input"] = f.compilation.pin()
    f.selection = write(f.selection.path, content(f.selected))
    f.owner = f.make_owner()
    with pytest.raises(CustodyError, match="callback plan"):
        await f.owner.prepare(f.app)
    assert f.app._application_callbacks is None and not f.events


@pytest.mark.parametrize("revision", [7, None])
async def test_helperless_replacement_refuses_before_binding_callbacks_or_constructing_sdk(fixture, revision):
    from launchkit.connect_installer_caller import HELPERLESS_UPDATE_UNAVAILABLE
    f = fixture
    plan = deepcopy(f.plan_value)
    for key in ("helper", "helper_tools", "helper_profile"):
        plan["application"].pop(key, None)
    f.plan = write(f.plan.path, content(plan))
    f.compilation = write(f.compilation.path, content(plan["application"]))
    f.selected["compilation_input"] = f.compilation.pin()
    f.selection = write(f.selection.path, content(f.selected))
    before = f.plan.read(), f.selection.read(), f.activation.read()
    f.owner = f.make_owner(expected_revision=revision)
    with pytest.raises(CustodyError) as refused:
        await f.owner.prepare(f.app)
    assert str(refused.value) == HELPERLESS_UPDATE_UNAVAILABLE
    assert not f.events and f.app.context.runtime is None
    assert f.app._application_callbacks is None and f.owner.callbacks is None
    assert (f.plan.read(), f.selection.read(), f.activation.read()) == before


@pytest.mark.parametrize("target", ["private", "installed", "admission"])
async def test_full_accepted_source_is_fenced_with_unchanged_native_inputs(fixture, target):
    f = fixture
    await f.owner.prepare(f.app)
    config = f.config()
    if target == "admission":
        f.current_selection[0] = False
    else:
        root = f.source.root if target == "private" else f.root
        (root / "assets/user-context.txt").write_bytes(b"different unselected application context\n")
    assert f.selection.read() == content(f.selected)
    with pytest.raises(CustodyError):
        await f.owner.connect(config, f.app)
    assert not f.events and f.source._invalidated


@pytest.mark.parametrize("change", ["callbacks", "dynamic", "session", "handler", "revision", "definition", "policy"])
async def test_selected_callback_and_builder_configuration_cannot_be_replaced(fixture, change):
    f = fixture
    await f.owner.prepare(f.app)
    config = f.config()
    if change == "callbacks":
        f.app._application_callbacks = object()
    elif change == "dynamic":
        f.app.dynamic = True
    elif change == "session":
        config.session_builder = object()
    elif change == "handler":
        config.host_runnables.append("unselected-handler")
    elif change == "revision":
        config.declare_spec_update_expected_revision = True
    elif change == "definition":
        config.mob_config_inline = "other-definition"
    else:
        config.application_tool_policies = ["other-policy"]
    with pytest.raises(CustodyError) as refused:
        await f.owner.connect(config, f.app)
    if change == "dynamic":
        # The configuration check names a roster that turned dynamic after prepare.
        assert "dynamic agent roster" in str(refused.value), str(refused.value)
    assert not f.events


@pytest.mark.parametrize("target", ["source", "directory", "config"])
async def test_post_native_return_change_closes_exact_runtime_and_withholds_current_success(fixture, target):
    f = fixture
    await f.owner.prepare(f.app)
    config = f.config()
    def change():
        if target == "source":
            (f.root / "assets/user-context.txt").write_bytes(b"changed while native connect settled\n")
        elif target == "directory":
            f.associations.rename(f.associations.with_name("retained-original-associations"))
            f.associations.mkdir(mode=0o700)
        else:
            config.host_runnables.append("changed-after-admission")
    f.after_connect = change
    with pytest.raises(CustodyError):
        await f.owner.connect(config, f.app)
    assert f.owner.native_outcome is f.native_outcome
    assert f.owner.runtime.closed and f.app.context.runtime is None
    assert f.events == ["construct", "bind", "connect", "shutdown"]


@pytest.mark.parametrize("phase", ["bind", "native"])
async def test_native_or_bind_failure_preserved_after_same_runtime_shutdown(fixture, phase):
    f = fixture
    error = RuntimeError("original exact native or bind failure")
    if phase == "bind":
        f.bind_error = error
    else:
        f.connect_error = error
    await f.owner.prepare(f.app)
    config = f.config()
    with pytest.raises(RuntimeError) as caught:
        await f.owner.connect(config, f.app)
    assert caught.value is error and f.owner.runtime.closed
    assert f.app.context.runtime is None and f.events[-1] == "shutdown"


async def test_shutdown_failure_retains_both_original_errors_and_exact_handle(fixture):
    f = fixture
    f.connect_error = RuntimeError("actual native outcome unknown")
    f.shutdown_error = OSError("actual same child cleanup unconfirmed")
    await f.owner.prepare(f.app)
    with pytest.raises(BaseExceptionGroup) as caught:
        await f.owner.connect(f.config(), f.app)
    assert caught.value.exceptions == (f.connect_error, f.shutdown_error)
    assert f.owner.runtime is f.app.context.runtime and not f.owner.runtime.closed


async def test_repeated_cancel_joins_original_shutdown_before_releasing_runtime(fixture, monkeypatch):
    f = fixture
    await f.owner.prepare(f.app)
    error = asyncio.CancelledError("original native caller cancellation")
    f.connect_error = error
    entered, release = asyncio.Event(), asyncio.Event()
    runtime_type = f.modules["meerkat_toolkit.application_update"].ToolkitApplicationUpdateRuntime
    async def delayed(runtime):
        entered.set()
        await release.wait()
        f.events.append("shutdown")
        runtime.closed = True
    monkeypatch.setattr(runtime_type, "shutdown", delayed)
    task = asyncio.create_task(f.owner.connect(f.config(), f.app))
    await entered.wait()
    task.cancel()
    await asyncio.sleep(0)
    task.cancel()
    await asyncio.sleep(0)
    assert not task.done() and f.app.context.runtime is f.owner.runtime
    release.set()
    with pytest.raises(asyncio.CancelledError) as caught:
        await task
    assert caught.value is error and f.owner.runtime.closed
    assert f.app.context.runtime is None
