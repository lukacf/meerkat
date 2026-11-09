"""Private bootstrap byte and child-lifetime controls with inert SDK boundaries.

No gateway, application command, provider, service or socket is executed. The
synthetic selected inputs establish custody only, never native qualification.
"""
import asyncio
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import threading
from types import ModuleType, SimpleNamespace

import pytest

from launchkit import deployment_application_bootstrap as module
from launchkit.connect_application_startup import FreshApplicationStartup, RestoredApplicationStartup
from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.deployment import materialize, read_receipt, update_receipt
from launchkit.templates.storage_setup import project_lock
from test_deployment import Guide, Implementation, Service, submit


CONFIG = '''[mob]
id = "bootstrap-app"
[profiles.helper-source]
model = "synthetic-model"
provider = "openai"
[profiles.helper-source.tools]
comms = true
shell = false
'''
POLICY = json.dumps({"provider_id": "application", "policy_id": "selected-tools", "revision": 1,
    "members": [{"member_identity": "lead:main", "grants": []}]}) + "\n"


def write(path, value, mode=0o600):
    content = value if isinstance(value, bytes) else json.dumps(value, sort_keys=True).encode() + b"\n"
    path.write_bytes(content)
    path.chmod(mode)
    return ExactFile(path, hashlib.sha256(content).hexdigest())


class AdmissionOnly(Service):
    def _launch(self, directory, mode):
        pass


@pytest.fixture(autouse=True)
def no_runtime_operations(monkeypatch):
    def forbidden(*args, **kwargs):
        raise AssertionError("Bootstrap custody controls cannot execute or signal a process or use a socket.")
    for owner, name in ((subprocess, "run"), (subprocess, "Popen"), (os, "system"),
            (os, "kill"), (os, "killpg"), (socket, "create_connection"),
            (asyncio, "create_subprocess_exec"), (asyncio, "create_subprocess_shell")):
        monkeypatch.setattr(owner, name, forbidden)


class Inputs:
    def __init__(self, directory, receipt, destination, monkeypatch):
        self.directory, self.receipt, self.destination = directory, receipt, destination
        private = destination / ".mobkit/bootstrap-inputs"
        private.mkdir(mode=0o700)
        self.private = private
        self.workspace = private / "working"
        self.workspace.mkdir(mode=0o700)
        member = {"mob_id": "bootstrap-app", "role": "helper-source", "member": "synthetic-native-member"}
        intent = json.loads(POLICY)
        intent["members"][0]["member_identity"] = member["member"]
        intent["source"] = {"source_id": receipt["build_id"],
            "source_digest": "sha256:" + hashlib.sha256(POLICY.encode()).hexdigest()}
        self.plan_value = {"schema": 1, "deployment_root": str(destination), "hold_timeout_ms": 30_000,
            "startup": {"kind": "initialize"},
            "application": {"schema": 4, "definition": {"format": "toml", "text": CONFIG},
                "realm": {"realm_id": "synthetic-realm", "context_root": str(destination),
                    "state_root": str(destination / ".mobkit/state")},
                "roster": [{"identity": "lead:main", "profile": "helper-source"}],
                "members": [{"member": member}], "helper": {"member": member},
                "helper_profile": "dedicated", "tool_policy": intent}}
        self.plan = write(private / "plan.json", self.plan_value)
        self.gateway = write(private / "gateway", b"synthetic gateway, never executed\n", 0o700)
        self.policy = write(private / "source-policy.json", POLICY.encode())
        self.template = write(private / "activation-template.json", {"synthetic": "native template"})
        self.fresh = {"kind": "fresh", "gateway": self.gateway.pin(), "preparation_plan": self.plan.pin(),
            "source_policy": self.policy.pin(), "activation_template": self.template.pin(),
            "provision_attempt_path": str(private / "original-provision.json"),
            "working_directory": str(self.workspace), "bundle_path": str(destination / "fresh-bundle"),
            "activation_path": str(destination / "fresh-activation.json")}
        bundle = destination / "original-bundle"
        bundle.mkdir(mode=0o700)
        self.activation = write(destination / "original-activation.json", {"synthetic": "activation"})
        self.local_policy = write(bundle / "policy.json", {"synthetic": "compiled native policy"})
        self.setup = write(bundle / "setup.json", {"synthetic": "completed native setup"})
        # The retained completion specimen includes the deployment parents;
        # ordinary Restore must verify them without creating replacements.
        for name in ("shared", "shared/data", "shared/control"):
            (destination / name).mkdir(mode=0o700, exist_ok=True)
        self.manifest = write(bundle / "manifest.json", {"artifacts": [
            {"path": self.local_policy.path.name, "sha256": self.local_policy.sha256},
            {"path": self.setup.path.name, "sha256": self.setup.sha256}]})
        (destination / "current").symlink_to(self.activation.path.name)
        self.selection = write(private / "restore-selection.json", {"deployment_root": str(destination),
            "activation": self.activation.pin(), "policy_manifest": self.manifest.pin(),
            "local_policy": self.local_policy.pin()})
        self.restore = {"kind": "restore", "gateway": self.gateway.pin(),
            "preparation_plan": self.plan.pin(), "selection_file": self.selection.pin(),
            "completed_setup": self.setup.pin()}
        remote = ModuleType("remote_operations")
        remote.__file__ = str(destination / "remote_operations.py")
        remote.command = lambda *args, **kwargs: pytest.fail("No application command is authorized in this control")
        monkeypatch.setitem(sys.modules, "remote_operations", remote)

    def admit(self, kind="fresh"):
        return module.retain_bootstrap_admission(self.directory, read_receipt(self.directory),
            self.fresh if kind == "fresh" else self.restore)

    def app(self):
        return SimpleNamespace(context=SimpleNamespace(root=self.destination, runtime=None), dynamic=False,
            _application_callbacks=None)


@pytest.fixture
async def inputs(tmp_path, monkeypatch):
    tmp_path.chmod(0o700)
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "b" * 64
    implementation.definition["agents"][0]["profile"] = "helper-source"
    implementation.extra_files = {"config/mob.toml": CONFIG, "application/tool-policy.json": POLICY,
        "remote_operations.py": "# inert maintained command source specimen\n"}
    service = AdmissionOnly(tmp_path / "private-server", guide, implementation)
    destination = tmp_path / "application"
    receipt = await submit(service, destination)
    directory = service._directory(guide.identity)
    materialize(directory)
    receipt = update_receipt(directory, status="stopped")
    return Inputs(directory, receipt, destination, monkeypatch)


@pytest.mark.parametrize("hosted_base", [None, "https://console.example.test/native-auth"])
async def test_fresh_retains_real_source_and_original_startup_with_durable_entry(inputs, hosted_base):
    pin, observation = inputs.admit()
    observation.require_current()
    factory = module.BootstrapFactory(inputs.directory, pin, hosted_mcp_redirect_base=hosted_base)
    with project_lock(inputs.destination) as lease:
        async with factory(lease) as startup:
            assert isinstance(startup, FreshApplicationStartup)
            assert startup.hosted_mcp_redirect_base == hosted_base
            assert startup.console_public_origin == (None if hosted_base is None else "https://console.example.test")
            assert startup.lease is lease and factory.source.receipt == inputs.receipt
            assert startup.caller.provision_attempt_path == Path(inputs.fresh["provision_attempt_path"])
            assert startup._startup_timeout == 240
            app = inputs.app()
            await startup.prepare(app)
            assert startup.application is app and app._application_callbacks is startup.callbacks
            assert factory.source.exact_file("application/tool-policy.json").read() == POLICY.encode()
            assert (inputs.directory / module.FRESH_ENTRY_NAME).is_file()
            assert not Path(inputs.fresh["provision_attempt_path"]).exists()
        assert factory.source._closed and not factory.source._invalidated
        with pytest.raises(CustodyError, match="already entered"):
            async with factory(lease):
                pytest.fail("Single original factory cannot enter twice")
    with pytest.raises(CustodyError, match="fresh"):
        inputs.admit()
    inputs.fresh["provision_attempt_path"] = str(inputs.private / "different-provision.json")
    with pytest.raises(CustodyError, match="fresh"):
        inputs.admit()


@pytest.mark.parametrize("hosted_base", [None, "https://console.example.test/native-auth"])
async def test_restore_retains_original_completed_setup_and_allows_an_explicit_later_restore(inputs, hosted_base):
    pin, _ = inputs.admit("restore")
    factory = module.BootstrapFactory(inputs.directory, pin, hosted_mcp_redirect_base=hosted_base)
    with project_lock(inputs.destination) as lease:
        async with factory(lease) as startup:
            assert isinstance(startup, RestoredApplicationStartup)
            assert startup.hosted_mcp_redirect_base == hosted_base
            assert startup.console_public_origin == (None if hosted_base is None else "https://console.example.test")
            app = inputs.app()
            await startup.prepare(app)
            assert startup.setup == inputs.setup and startup._setup_bytes == inputs.setup.read()
            assert startup.callbacks is app._application_callbacks
            assert not (inputs.workspace / "finalization.json").exists()
        assert factory.source._closed
    new_pin, _ = inputs.admit("restore")
    assert new_pin != pin


async def test_original_envelope_survives_legitimate_launch_lifecycle_and_status_only(inputs):
    pin, observation = inputs.admit("restore")
    before = module.read_bootstrap_admission(inputs.directory, pin)
    update_receipt(inputs.directory, status="queued", interactive_lifecycle_id="new-original-launch",
        interactive_cleanup_attempt_id=None, interactive_cleanup=None)
    assert module.read_bootstrap_admission(inputs.directory, pin) == before
    with pytest.raises(CustodyError, match="changed"):
        observation.require_current()
    factory = module.BootstrapFactory(inputs.directory, pin)
    with project_lock(inputs.destination) as lease:
        async with factory(lease):
            factory.require_current()
            assert factory.source.receipt["interactive_lifecycle_id"] == "new-original-launch"


@pytest.mark.parametrize("name", ["package", "gateway", "installed"])
async def test_prequeue_observation_refuses_same_bytes_atomic_replacement(inputs, name):
    _, observation = inputs.admit()
    path = {"package": inputs.directory / "package.zip", "gateway": inputs.gateway.path,
        "installed": inputs.destination / "application/tool-policy.json"}[name]
    replacement = path.with_name(path.name + ".replacement")
    write(replacement, path.read_bytes(), path.stat().st_mode & 0o777)
    replacement.replace(path)
    with pytest.raises(CustodyError, match="changed"):
        observation.require_current()


@pytest.mark.parametrize("change", ["definition", "member", "policy", "manifest"])
async def test_admission_refuses_mixed_original_source_and_native_inputs(inputs, change):
    if change == "manifest":
        write(inputs.manifest.path, {"artifacts": [{"path": inputs.setup.path.name, "sha256": "a" * 64}]})
        with pytest.raises(CustodyError):
            inputs.admit("restore")
        return
    value = deepcopy(inputs.plan_value)
    if change == "definition":
        value["application"]["definition"]["text"] = CONFIG.replace("bootstrap-app", "different-app")
    elif change == "member":
        value["application"]["members"][0]["member"]["role"] = "wrong-role"
    else:
        value["application"]["tool_policy"]["members"][0]["grants"] = [
            {"tool_name": "unadmitted-tool", "action": "invoke", "consequence": "r3"}]
    inputs.fresh["preparation_plan"] = write(inputs.plan.path, value).pin()
    with pytest.raises(CustodyError):
        inputs.admit()


async def test_fresh_owner_publication_may_change_current_without_invalidating_source(inputs):
    pin, _ = inputs.admit()
    factory = module.BootstrapFactory(inputs.directory, pin)
    with project_lock(inputs.destination) as lease:
        async with factory(lease) as startup:
            await startup.prepare(inputs.app())
            original = startup.finalizer.original
            new = write(inputs.destination / "synthetic-next-activation.json", {"synthetic": "owned publication"})
            pointer = inputs.destination / "current"
            pointer.unlink()
            pointer.symlink_to(new.path.name)
            factory.require_current()
            await factory.source.require_current_async()
            assert startup.finalizer.original == original


async def test_restore_refuses_current_pointer_change_after_original_factory_entry(inputs):
    pin, _ = inputs.admit("restore")
    factory = module.BootstrapFactory(inputs.directory, pin)
    with project_lock(inputs.destination) as lease:
        with pytest.raises(CustodyError, match="current"):
            async with factory(lease):
                (inputs.destination / "current").unlink()
                factory.require_current()
        assert factory.source._closed


async def test_child_factory_refuses_mutating_original_selection_and_wrong_project(inputs):
    pin, _ = inputs.admit()
    factory = module.BootstrapFactory(inputs.directory, pin)
    factory.inputs["gateway"]["sha256"] = "a" * 64
    with pytest.raises(CustodyError, match="selection changed"):
        factory.require_current()
    factory = module.BootstrapFactory(inputs.directory, pin)
    lease = SimpleNamespace(root=inputs.private, require_current=lambda: None)
    with pytest.raises(CustodyError, match="another project"):
        async with factory(lease):
            pytest.fail("Wrong project cannot acquire source")
    assert factory.source is None


async def test_original_child_envelope_swap_during_joined_input_read_is_refused(inputs, monkeypatch):
    pin, _ = inputs.admit("restore")
    factory = module.BootstrapFactory(inputs.directory, pin)
    original = module._observe_inputs
    def read_and_swap(*args, **kwargs):
        result = original(*args, **kwargs)
        path = Path(pin["path"])
        replacement = path.with_name(path.name + ".replacement")
        write(replacement, path.read_bytes())
        replacement.replace(path)
        return result
    monkeypatch.setattr(module, "_observe_inputs", read_and_swap)
    with project_lock(inputs.destination) as lease:
        with pytest.raises(CustodyError, match="envelope changed"):
            async with factory(lease):
                pytest.fail("A substituted original envelope cannot enter the child")
        assert factory.source is None


async def test_failed_original_shutdown_is_not_retried_and_keeps_exact_source(inputs):
    pin, _ = inputs.admit("restore")
    factory = module.BootstrapFactory(inputs.directory, pin)
    calls = []
    failure = RuntimeError("inert original shutdown failed")
    class Runtime:
        async def shutdown(self):
            calls.append(self)
            raise failure
    runtime = Runtime()
    with project_lock(inputs.destination) as lease:
        try:
            with pytest.raises(RuntimeError) as observed:
                async with factory(lease) as startup:
                    startup.runtime = runtime
                    await startup.shutdown(runtime)
            assert observed.value is failure and calls == [runtime]
            assert not factory.source._closed and factory._shutdown_task.exception() is failure
            with pytest.raises(RuntimeError) as repeated:
                await factory.startup.shutdown(runtime)
            assert repeated.value is failure and calls == [runtime]
        finally:
            # Test-only descriptor teardown, without claiming runtime cleanup.
            factory.source.close()


async def test_repeated_cancel_joins_exact_shutdown_before_source_close(inputs):
    pin, _ = inputs.admit("restore")
    factory = module.BootstrapFactory(inputs.directory, pin)
    entered, release = asyncio.Event(), asyncio.Event()
    calls = []
    class Runtime:
        async def shutdown(self):
            calls.append(self)
            entered.set()
            await release.wait()
    runtime = Runtime()
    with project_lock(inputs.destination) as lease:
        async def owner():
            async with factory(lease) as startup:
                startup.runtime = runtime
                await startup.shutdown(runtime)
        task = asyncio.create_task(owner())
        await entered.wait()
        task.cancel()
        await asyncio.sleep(0)
        task.cancel()
        await asyncio.sleep(0)
        assert not task.done() and not factory.source._closed and calls == [runtime]
        release.set()
        with pytest.raises(asyncio.CancelledError):
            await task
        assert factory.source._closed and calls == [runtime]


def test_fixed_child_arguments_and_sdk_import_precede_authored_path(inputs, monkeypatch):
    pin, _ = inputs.admit("restore")
    argv = module.bootstrap_child_arguments(inputs.directory, pin)
    assert argv[:2] == ("-I", "-c") and argv[3:] == (str(inputs.directory), pin["path"], pin["sha256"])
    imported = []
    def load(name):
        if name != "app":
            assert str(inputs.destination) not in sys.path
            imported.append(name)
            return ModuleType(name)
        assert imported == ["meerkat_meta.builder", "meerkat_toolkit.host_control", "meerkat_toolkit.application_runtime"]
        def main(*, application_startup_factory):
            assert isinstance(application_startup_factory, module.BootstrapFactory)
        return SimpleNamespace(__file__=str(inputs.destination / "app.py"), main=main)
    monkeypatch.setattr(module.importlib, "import_module", load)
    monkeypatch.setattr(sys, "path", list(sys.path))
    monkeypatch.setattr(sys, "argv", list(sys.argv))
    module.run_child(str(inputs.directory), pin["path"], pin["sha256"])


async def test_actual_source_retention_file_checks_run_off_loop(inputs, monkeypatch):
    pin, _ = inputs.admit("restore")
    factory = module.BootstrapFactory(inputs.directory, pin)
    loop_thread = threading.get_ident()
    checked = []
    original = module._observe_inputs
    def observe(*args, **kwargs):
        checked.append(threading.get_ident())
        assert threading.get_ident() != loop_thread
        return original(*args, **kwargs)
    monkeypatch.setattr(module, "_observe_inputs", observe)
    with project_lock(inputs.destination) as lease:
        async with factory(lease):
            factory.source.require_owner_current()
            await factory.source.require_current_async()
    assert checked and factory.source._closed


def test_installed_source_swap_during_sdk_import_refuses_authored_import(inputs, monkeypatch):
    pin, _ = inputs.admit("restore")
    imported = []
    def load(name):
        imported.append(name)
        assert name != "app"
        if name == "meerkat_toolkit.application_runtime":
            path = inputs.destination / "app.py"
            replacement = path.with_name("app.py.replacement")
            write(replacement, path.read_bytes(), path.stat().st_mode & 0o777)
            replacement.replace(path)
        return ModuleType(name)
    monkeypatch.setattr(module.importlib, "import_module", load)
    with pytest.raises(CustodyError, match="changed"):
        module.run_child(str(inputs.directory), pin["path"], pin["sha256"])
    assert "app" not in imported


async def test_bootstrap_factory_refuses_hosted_selection_for_ordinary_kind(inputs, monkeypatch):
    pin, _ = inputs.admit()
    original = module.read_bootstrap_admission
    def ordinary(*args, **kwargs):
        selected = deepcopy(original(*args, **kwargs))
        selected["application_startup"]["kind"] = "ordinary"
        return selected
    monkeypatch.setattr(module, "read_bootstrap_admission", ordinary)
    with pytest.raises(CustodyError, match="requires a Toolkit application startup"):
        module.BootstrapFactory(inputs.directory, pin,
            hosted_mcp_redirect_base="https://console.example.test/native-auth")
