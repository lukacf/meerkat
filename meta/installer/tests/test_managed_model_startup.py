"""Linux model-config custody using actual startup owners and inert SDK edges.

No credential file is read, and no native gateway, provider, socket or process
is entered. The original bootstrap source/lease readers remain real. SDK
specimens exercise selected config, sequencing and cleanup only.
"""
import asyncio
from copy import deepcopy
import os
from pathlib import Path
import sys
import threading
from types import ModuleType, SimpleNamespace

import pytest

from launchkit import connect_application_startup as startup
from launchkit import deployment_application_bootstrap as bootstrap
from launchkit.connect_installer_caller import CustodyError
from launchkit.deployment import materialize, read_receipt, update_receipt
from launchkit.templates.storage_setup import project_lock
from test_application_startup import Fixture as FreshFixture
from test_application_restart import RestartFixture
from test_deployment_application_bootstrap import (AdmissionOnly, CONFIG, POLICY, Inputs,
    inputs, no_runtime_operations, write)
from test_deployment import Guide, Implementation, submit


MANAGED = {"classes": [{"model_auth": "managed_host"}]}
API_KEY = {"classes": [{}]}


@pytest.fixture
async def managed_inputs(tmp_path, monkeypatch):
    tmp_path.chmod(0o700)
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "b" * 64
    implementation.definition["agents"][0]["profile"] = "helper-source"
    implementation.definition["classes"][0]["model_auth"] = "managed_host"
    implementation.extra_files = {"config/mob.toml": CONFIG, "application/tool-policy.json": POLICY,
        "remote_operations.py": "# inert maintained command source specimen\n"}
    service = AdmissionOnly(tmp_path / "private-server", guide, implementation)
    destination = tmp_path / "application"
    await submit(service, destination)
    directory = service._directory(guide.identity)
    materialize(directory)
    receipt = update_receipt(directory, status="stopped")
    return Inputs(directory, receipt, destination, monkeypatch)


def model_selection(root, monkeypatch, *, environment=True):
    private = root / "selected-model"
    private.mkdir(mode=0o700)
    host_config = write(private / "host.toml", b"# inert nonsecret host-config declaration\n")
    credential_root = private / "native-config-root"
    credential_root.mkdir(mode=0o700)
    monkeypatch.setattr(sys, "platform", "linux")
    if environment:
        monkeypatch.setenv("XDG_CONFIG_HOME", str(credential_root))
    return {"host_config": host_config.pin(), "credential_config_root": str(credential_root)}


def selected_fixture(root, monkeypatch, mode):
    fixture = (FreshFixture if mode == "fresh" else RestartFixture)(root, monkeypatch)
    selection = model_selection(root, monkeypatch)
    if mode == "fresh":
        fixture.owner = startup.FreshApplicationStartup(project_lease=fixture.lease,
            accepted_source=fixture.source, gateway=fixture.gateway,
            preparation_plan=fixture.plan, source_policy=fixture.policy,
            activation_template=fixture.template,
            command=lambda *a, **kw: pytest.fail("No command is authorized in this custody control"),
            command_timeout=5, startup_timeout=60,
            provision_attempt_path=root / "provision.json", working_directory=fixture.workspace,
            bundle_path=root / "bundle", activation_path=root / "activation.json",
            model_configuration=selection)
    else:
        fixture.owner = fixture.make_owner(model_configuration=selection)
    return fixture, selection


@pytest.mark.parametrize("mode", ["fresh", "restore"])
async def test_selected_model_config_reaches_only_original_runtime_and_cleanup(tmp_path, monkeypatch, mode):
    fixture, selection = selected_fixture(tmp_path, monkeypatch, mode)
    original_plan = fixture.plan.read()
    fixture.owner.require_model_configuration(MANAGED)
    await fixture.owner.prepare(fixture.app)
    config = fixture.config()
    assert config.meerkat_config_path == selection["host_config"]["path"]
    assert fixture.plan.read() == original_plan
    assert fixture.app._application_callbacks is fixture.owner.callbacks
    native_root = Path(selection["credential_config_root"])
    # Native may create state under this root. Only its metadata is observed.
    (native_root / "native-owned-child").mkdir(mode=0o700)
    fixture.owner.require_model_configuration(MANAGED)
    runtime = await fixture.owner.connect(config, fixture.app)
    assert runtime is fixture.owner.runtime is fixture.app.context.runtime
    assert runtime.config.meerkat_config_path == selection["host_config"]["path"]
    assert fixture.events.count("construct") == 1 and fixture.events.count("connect") == 1
    assert fixture.plan.read() == original_plan
    await fixture.owner.shutdown(runtime)
    assert fixture.events.count("shutdown") == 1


@pytest.mark.parametrize("mode", ["fresh", "restore"])
async def test_managed_intent_without_owner_selection_refuses_but_api_key_default_is_unchanged(
        tmp_path, monkeypatch, mode):
    fixture = (FreshFixture if mode == "fresh" else RestartFixture)(tmp_path, monkeypatch)
    fixture.owner.require_model_configuration(API_KEY)
    with pytest.raises(CustodyError, match="original trusted startup model configuration"):
        fixture.owner.require_model_configuration(MANAGED)
    await fixture.owner.prepare(fixture.app)
    config = fixture.config()
    assert getattr(config, "meerkat_config_path", None) is None
    assert not fixture.events


@pytest.mark.parametrize("mode", ["fresh", "restore"])
@pytest.mark.parametrize("change", ["builder-path", "config-path", "environment", "host-bytes", "root-mode", "owner-selection"])
async def test_changed_model_selection_refuses_before_any_runtime(tmp_path, monkeypatch, mode, change):
    fixture, selection = selected_fixture(tmp_path, monkeypatch, mode)
    await fixture.owner.prepare(fixture.app)
    if change == "builder-path":
        with pytest.raises(CustodyError, match="cannot replace"):
            fixture.owner.builder_config(persistent_state=str(tmp_path / ".mobkit/state"),
                meerkat_config_path=str(tmp_path / "other.toml"))
    else:
        config = fixture.config()
        if change == "config-path":
            config.meerkat_config_path = str(tmp_path / "other.toml")
        elif change == "environment":
            monkeypatch.setenv("XDG_CONFIG_HOME", str(tmp_path / "unselected-root"))
        elif change == "host-bytes":
            Path(selection["host_config"]["path"]).write_bytes(b"changed selected model config\n")
        elif change == "root-mode":
            Path(selection["credential_config_root"]).chmod(0o500)
        else:
            fixture.owner._model_configuration = None
        with pytest.raises(CustodyError, match="model config|credential|model configuration"):
            await fixture.owner.connect(config, fixture.app)
    assert fixture.events == [] and fixture.owner.runtime is None and fixture.app.context.runtime is None


@pytest.mark.parametrize("mode", ["fresh", "restore"])
async def test_model_environment_change_during_connect_joins_same_runtime_before_refusal(
        tmp_path, monkeypatch, mode):
    fixture, selection = selected_fixture(tmp_path, monkeypatch, mode)
    await fixture.owner.prepare(fixture.app)
    config = fixture.config()
    runtime_module = fixture.modules["meerkat_toolkit.application_runtime"] if mode == "fresh" else fixture.modules["meerkat_toolkit.host_control"]
    runtime_type = runtime_module.ToolkitApplicationRuntime if mode == "fresh" else runtime_module.ToolkitMobKitRuntime
    original = runtime_type.connect
    async def change_environment(runtime):
        await original(runtime)
        os.environ["XDG_CONFIG_HOME"] = str(tmp_path / "changed-during-native-await")
    monkeypatch.setattr(runtime_type, "connect", change_environment)
    with pytest.raises(CustodyError, match="original Linux credential environment"):
        await fixture.owner.connect(config, fixture.app)
    assert fixture.events.count("construct") == fixture.events.count("connect") == fixture.events.count("shutdown") == 1
    assert fixture.app.context.runtime is None and fixture.owner.runtime is not None
    assert fixture.owner._model_configuration.selection() == selection


@pytest.mark.parametrize("kind", ["fresh", "restore"])
async def test_bootstrap_observes_model_files_off_loop_with_actual_source_and_project_lease(
        inputs, monkeypatch, kind):
    selection = model_selection(inputs.private, monkeypatch)
    selected = inputs.fresh if kind == "fresh" else inputs.restore
    selected["model_configuration"] = selection
    admission, observed = inputs.admit(kind)
    assert observed.model_configuration.selection() == selection
    factory = bootstrap.BootstrapFactory(inputs.directory, admission)
    loop_thread, reads = threading.get_ident(), []
    original = startup._read
    def read(path, *args, **kwargs):
        if path == Path(selection["host_config"]["path"]):
            reads.append(threading.get_ident())
            assert threading.get_ident() != loop_thread
        assert Path(selection["credential_config_root"]) not in Path(path).parents
        return original(path, *args, **kwargs)
    monkeypatch.setattr(startup, "_read", read)
    with project_lock(inputs.destination) as lease:
        async with factory(lease) as owner:
            assert owner.lease is lease and factory.source.receipt == inputs.receipt
            owner.require_model_configuration(MANAGED)
            await owner.prepare(inputs.app())
            assert owner._model_configuration.selection() == selection
            (Path(selection["credential_config_root"]) / "native-persistence-child").mkdir(mode=0o700)
            factory.require_current()
            owner._require_current()
        assert factory.source._closed
    assert reads


@pytest.mark.parametrize("change", ["bytes", "same-bytes-replacement", "root-replacement", "root-mode", "config-link"])
def test_original_bootstrap_model_observation_rejects_selected_custody_drift(inputs, monkeypatch, change):
    selection = model_selection(inputs.private, monkeypatch)
    inputs.restore["model_configuration"] = selection
    _, observed = inputs.admit("restore")
    config, root = Path(selection["host_config"]["path"]), Path(selection["credential_config_root"])
    if change == "bytes":
        original_mtime = config.stat().st_mtime_ns
        config.write_bytes(b"changed nonsecret model selection\n")
        os.utime(config, ns=(original_mtime, original_mtime))
    elif change == "same-bytes-replacement":
        replacement = write(config.with_name("same-bytes.toml"), config.read_bytes())
        replacement.path.replace(config)
    elif change == "root-replacement":
        root.rename(root.with_name("retained-native-root"))
        root.mkdir(mode=0o700)
    elif change == "root-mode":
        root.chmod(0o500)
    else:
        target = write(config.with_name("unselected.toml"), config.read_bytes())
        config.unlink()
        config.symlink_to(target.path)
    with pytest.raises(CustodyError, match="custody changed|symlink"):
        observed.require_current()


@pytest.mark.parametrize("change", ["none", "unknown-field", "relative-root", "bad-pin"])
def test_startup_optional_model_selection_remains_closed(inputs, change):
    selection = {"host_config": inputs.plan.pin(), "credential_config_root": str(inputs.private)}
    if change == "none":
        selection = None
    elif change == "unknown-field":
        selection["credential_bytes"] = "not an allowed carrier"
    elif change == "relative-root":
        selection["credential_config_root"] = "relative-root"
    else:
        selection["host_config"]["sha256"] = "not-a-sha256"
    value = {**inputs.fresh, "model_configuration": selection}
    with pytest.raises(CustodyError):
        bootstrap._shape(value)
    assert bootstrap._shape(inputs.fresh) == "fresh"


@pytest.mark.parametrize("mutate_environment", [False, True])
def test_selected_linux_root_is_set_before_genuine_sdk_import_and_cannot_change_before_app(
        inputs, monkeypatch, mutate_environment):
    selection = model_selection(inputs.private, monkeypatch, environment=False)
    monkeypatch.setenv("XDG_CONFIG_HOME", "untrusted inherited root")
    inputs.restore["model_configuration"] = selection
    admission, _ = inputs.admit("restore")
    imported = []
    def load(name):
        imported.append(name)
        assert os.environ["XDG_CONFIG_HOME"] == selection["credential_config_root"]
        if name == "meerkat_toolkit.application_runtime" and mutate_environment:
            os.environ["XDG_CONFIG_HOME"] = "changed by import"
        if name != "app":
            assert str(inputs.destination) not in sys.path
            return ModuleType(name)
        def main(*, application_startup_factory):
            assert application_startup_factory.inputs["model_configuration"] == selection
        return SimpleNamespace(__file__=str(inputs.destination / "app.py"), main=main)
    monkeypatch.setattr(bootstrap.importlib, "import_module", load)
    monkeypatch.setattr(sys, "path", list(sys.path))
    monkeypatch.setattr(sys, "argv", list(sys.argv))
    if mutate_environment:
        with pytest.raises(CustodyError, match="original Linux credential environment"):
            bootstrap.run_child(str(inputs.directory), admission["path"], admission["sha256"])
        assert "app" not in imported
    else:
        bootstrap.run_child(str(inputs.directory), admission["path"], admission["sha256"])
        assert imported[-1] == "app"


def test_non_linux_selected_credential_root_refuses_without_changing_default_startup(inputs, monkeypatch):
    selection = model_selection(inputs.private, monkeypatch)
    inputs.restore["model_configuration"] = selection
    monkeypatch.setattr(sys, "platform", "darwin")
    with pytest.raises(CustodyError, match="original Linux host"):
        inputs.admit("restore")
    inputs.restore.pop("model_configuration")
    _, observed = inputs.admit("restore")
    assert observed.model_configuration is None


async def test_checked_installed_managed_intent_needs_actual_model_selection(managed_inputs, monkeypatch):
    selected = managed_inputs
    with pytest.raises(CustodyError, match="Managed-host classes"):
        selected.admit("restore")
    selection = model_selection(selected.private, monkeypatch)
    selected.restore["model_configuration"] = selection
    admission, observation = selected.admit("restore")
    assert observation.model_configuration.selection() == selection
    assert bootstrap.read_bootstrap_admission(selected.directory, admission)["application_startup"]["model_configuration"] == selection
    assert read_receipt(selected.directory) == selected.receipt


@pytest.mark.parametrize("change", ["removed", "other-root", "other-pin"])
async def test_restore_history_keeps_exact_original_fresh_model_selection(inputs, monkeypatch, change):
    selection = model_selection(inputs.private, monkeypatch)
    inputs.fresh["model_configuration"] = selection
    admission, _ = inputs.admit("fresh")
    factory = bootstrap.BootstrapFactory(inputs.directory, admission)
    with project_lock(inputs.destination) as lease:
        async with factory(lease):
            pass
    # Only private finite lineage is exercised. These are inert output files,
    # not a qualified native finalization or runtime Restore.
    bundle = Path(inputs.fresh["bundle_path"])
    bundle.mkdir(mode=0o700)
    activation = write(Path(inputs.fresh["activation_path"]), {"inert": "activation"})
    policy = write(bundle / "policy.json", {"inert": "policy"})
    setup = write(bundle / "setup.json", {"inert": "completed setup"})
    manifest = write(bundle / "manifest.json", {"artifacts": [
        {"path": policy.path.name, "sha256": policy.sha256},
        {"path": setup.path.name, "sha256": setup.sha256}]})
    selected = write(inputs.private / "actual-restore.json", {"deployment_root": str(inputs.destination),
        "activation": activation.pin(), "policy_manifest": manifest.pin(), "local_policy": policy.pin()})
    restored = {"kind": "restore", "gateway": inputs.fresh["gateway"],
        "preparation_plan": inputs.fresh["preparation_plan"], "selection_file": selected.pin(),
        "completed_setup": setup.pin(), "model_configuration": deepcopy(selection)}
    bootstrap.require_restore_history_current(inputs.directory, restored)
    if change == "removed":
        restored.pop("model_configuration")
    elif change == "other-root":
        other = inputs.private / "other-native-root"
        other.mkdir(mode=0o700)
        restored["model_configuration"]["credential_config_root"] = str(other)
    else:
        other = write(inputs.private / "other-host.toml", b"# different nonsecret original host selection\n")
        restored["model_configuration"]["host_config"] = other.pin()
    with pytest.raises(CustodyError, match="original Fresh model configuration"):
        bootstrap.require_restore_history_current(inputs.directory, restored)
