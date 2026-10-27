"""Inert selected-product worker routing, not runtime or native qualification.

The real worker functions run against original byte fixtures and explicit mock
admission, lease and command owners. No native, SDK, dependency, shell or Python
child executes. Mock readiness cannot establish installed application success.
"""
import asyncio
from contextlib import contextmanager, redirect_stdout
from copy import deepcopy
import io
import json
import os
from pathlib import Path
import socket
import subprocess
import sys
import sysconfig
from types import SimpleNamespace

import pytest

from launchkit import deployment_application_bootstrap as bootstrap
from launchkit import deployment_application_startup as startup
from launchkit import deployment_updates as updates
from launchkit import deployment_worker as worker
from launchkit import runtime_candidates as candidates
from launchkit import runtime_dependencies as dependencies
from launchkit import runtime_selection
from launchkit.projects import DraftConflict
from launchkit.templates import install_candidate as installed
from test_meta_toolkit_runtime_selection import FIXTURE, PREFIX, product


@pytest.fixture(autouse=True)
def no_runtime_execution(monkeypatch):
    def forbidden(*args, **kwargs):
        pytest.fail("Inert worker routing attempted a real process, signal or network operation.")
    for owner, name in ((subprocess, "run"), (subprocess, "Popen"), (os, "system"),
            (os, "kill"), (os, "killpg"), (socket, "create_connection"),
            (asyncio, "create_subprocess_exec"), (asyncio, "create_subprocess_shell")):
        monkeypatch.setattr(owner, name, forbidden)


class InertExecutor:
    """Keep the existing async command and selected SDK-pair argument protocol."""
    def __init__(self, destination, events):
        self.destination, self.events = destination, events
        self.commands, self.sdk_calls, self.prefix_calls = [], [], []
        self.stop = asyncio.Event()
        self.after_phase = lambda name: None
        self.site = destination / ".venv/lib/python-fixture/site-packages"

    async def command(self, argv, phase, timeout, **options):
        assert not any("install.sh" in str(value) or value == "--doctor" for value in argv)
        assert timeout == worker.CHECK_TIMEOUT
        self.commands.append((argv, phase, timeout, options))
        if phase == "Selected SDK environment":
            assert argv == [sys.executable, "-I", "-m", "venv", str(self.destination / ".venv")]
            assert not options
            python = self.destination / ".venv/bin/python"
            python.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            python.write_bytes(b"INERT selected interpreter, must never execute\n")
            python.chmod(0o700)
            self.site.mkdir(mode=0o700, parents=True, exist_ok=True)
            result, name = "", "environment"
        elif phase == "Selected SDK site path":
            assert argv[:4] == [str(self.destination / ".venv/bin/python"), "-I", "-S", "-c"]
            assert "sysconfig" in argv[4] and options == {"capture_output": True}
            result, name = json.dumps(str(self.site)), "site"
        elif phase == "Updated application contract":
            assert argv == [str(self.destination / ".venv/bin/python"), "app.py", "--application-check"]
            assert not options
            result, name = "", "application-check"
        else:
            pytest.fail(f"Unexpected command path: {phase}")
        self.events.append(name)
        self.after_phase(name)
        return result

    async def install_selected_sdk_pair(self, *args, **options):
        options["require_current"]()
        self.sdk_calls.append((args, options))
        self.events.append("sdk-pair")
        self.after_phase("sdk-pair")
        options["require_current"]()

    def copy_prefix(self, artifacts, **options):
        options["require_current"]()
        self.prefix_calls.append((artifacts, options))
        self.events.append("prefix-copy")
        self.after_phase("prefix-copy")
        options["require_current"]()
        return self.destination / ".bin" / artifacts.prefix / "bin/toolkit_gateway"


@pytest.fixture
def direct(product, tmp_path, monkeypatch):
    destination = tmp_path.resolve() / "inert direct destination"
    destination.mkdir(mode=0o700)
    events = []
    executor = InertExecutor(destination, events)
    artifacts = candidates.read_meta_toolkit_artifacts(product.target)
    state = SimpleNamespace(product=product, destination=destination, events=events,
        executor=executor, artifacts=artifacts, current=True)
    def fence():
        events.append("owner-fence")
        if not state.current:
            raise DraftConflict("The original inert installation owner changed.")
    state.fence = fence
    monkeypatch.setattr(installed, "install_meta_toolkit_prefix", executor.copy_prefix)
    return state


async def test_direct_preparation_reuses_original_executor_pair_and_prefix_arguments(direct):
    f = direct
    progress = []
    report_progress = progress.append
    result = await worker._install_selected_product(f.executor, f.artifacts, f.fence, f.executor.command,
        report_progress=report_progress)
    assert result == {"sdk_python": f.destination / ".venv/bin/python", "site_packages": f.executor.site}
    args, options = f.executor.sdk_calls[0]
    target = f.product.target
    assert args == (target.meta_wheel.path, target.toolkit_wheel.path)
    current = options["require_current"]
    assert options == {"meta": target.meta, "toolkit": target.toolkit,
        "site_packages": f.executor.site, "sdk_python": f.destination / ".venv/bin/python",
        "dependency_requirements": target.dependency_requirements,
        "dependency_wheels": target.dependency_wheels, "require_current": current}
    assert f.executor.prefix_calls == [(f.artifacts,
        {"deployment_root": f.destination, "require_current": current, "report_progress": report_progress})]
    assert progress == ["Preparing the application environment.",
        "Installing the agent software and its dependencies.",
        "Copying and checking the native runtime files."]
    phases = [value for value in f.events if value != "owner-fence"]
    assert phases == ["environment", "site", "sdk-pair", "prefix-copy"]
    for before, after in zip(phases, phases[1:]):
        assert "owner-fence" in f.events[f.events.index(before) + 1:f.events.index(after)]
    before = len(f.events)
    current()
    assert f.events[before:] == ["owner-fence"]
    f.current = False
    with pytest.raises(DraftConflict, match="owner changed"):
        current()


async def test_site_probe_avoids_debian_system_default_without_running_site_startup(direct, monkeypatch):
    """Execute only the worker's stdlib probe, with Debian's default layout.

    The existing inert executor still owns every command. No interpreter,
    installed .pth, authored application, SDK or native runtime executes.
    """
    f = direct
    original = f.executor.command
    observed = []

    def debian_path(name, scheme=None, vars=None):
        observed.append((name, scheme, vars))
        assert name == "purelib" and vars == {
            "base": str(f.destination / ".venv"), "platbase": str(f.destination / ".venv")}
        if scheme in {"posix_venv", "venv"}:
            return str(f.executor.site)
        # Python 3.11-3.13 with -S may select Debian's system scheme. venv
        # created site-packages, so this plausible path does not exist.
        return str(f.destination / ".venv/local/lib/python3.12/dist-packages")

    async def command(argv, phase, timeout, **options):
        result = await original(argv, phase, timeout, **options)
        if phase != "Selected SDK site path":
            return result
        assert argv[1:4] == ["-I", "-S", "-c"]
        with monkeypatch.context() as patch:
            patch.setattr(sys, "executable", str(f.destination / ".venv/bin/python"))
            patch.setattr(sysconfig, "get_path", debian_path)
            output = io.StringIO()
            with redirect_stdout(output):
                exec(compile(argv[4], "<selected-stdlib-site-probe>", "exec"), {})
        return output.getvalue()

    result = await worker._install_selected_product(f.executor, f.artifacts, f.fence, command)
    assert result["site_packages"] == f.executor.site
    assert observed and observed[0][1] in {"posix_venv", "venv"}
    assert len(f.executor.sdk_calls) == len(f.executor.prefix_calls) == 1


@pytest.mark.parametrize("refusal", ["false", "raises", "awaitable"])
async def test_direct_owner_refusal_precedes_environment_creation_and_commands(direct, refusal):
    async def pending():
        return False
    def refused():
        if refusal == "raises":
            raise DraftConflict("The original inert owner refused.")
        if refusal == "awaitable":
            return pending()
        return False
    with pytest.raises((DraftConflict, candidates.CandidateArtifactError, RuntimeError)):
        await worker._install_selected_product(direct.executor, direct.artifacts, refused, direct.executor.command)
    assert list(direct.destination.iterdir()) == []
    assert not direct.executor.commands and not direct.executor.sdk_calls and not direct.executor.prefix_calls


@pytest.mark.parametrize("phase", ["environment", "site", "sdk-pair", "prefix-copy"])
async def test_direct_changed_owner_stops_before_next_preparation_phase(direct, phase):
    direct.executor.after_phase = lambda current: setattr(direct, "current", False) if current == phase else None
    with pytest.raises(DraftConflict, match="owner changed"):
        await worker._install_selected_product(direct.executor, direct.artifacts, direct.fence, direct.executor.command)
    phases = [value for value in direct.events if value != "owner-fence"]
    expected = ["environment", "site", "sdk-pair", "prefix-copy"]
    assert phases == expected[:expected.index(phase) + 1]


async def test_direct_foreign_site_refuses_before_sdk_or_native_mutation(direct, tmp_path):
    foreign = tmp_path.resolve() / "foreign site"
    foreign.mkdir(mode=0o700)
    original = direct.executor.command
    async def command(*args, **kwargs):
        result = await original(*args, **kwargs)
        return json.dumps(str(foreign)) if args[1] == "Selected SDK site path" else result
    with pytest.raises(candidates.CandidateArtifactError, match="inside"):
        await worker._install_selected_product(direct.executor, direct.artifacts, direct.fence, command)
    assert not direct.executor.sdk_calls and not direct.executor.prefix_calls


@pytest.fixture
def routing(direct, tmp_path, monkeypatch):
    """Mock boundary custody while executing the real update_runtime branches."""
    f = direct
    f.directory = tmp_path.resolve() / "inert worker journal"
    f.directory.mkdir(mode=0o700)
    f.transaction_root = f.directory / "original update"
    f.transaction_root.mkdir(mode=0o700)
    rendered = runtime_selection.candidate_selection(f.product.target)
    f.update = {"update_id": "inert-original-update", "from_build_id": "inert-before", "to_build_id": "inert-after",
        "guide_version": 1, "blueprint_hash": "1" * 64, "source_digest": "2" * 64,
        "brief_hash": "3" * 64, "package_sha256": "4" * 64, "previous_package_sha256": "5" * 64,
        "changes": [], "blockers": [], "native_definition_update": False, "host_instruction_update": False,
        "maintained_host_update": None, "status": "staged", "runtime_admission_possible": False,
        "runtime_dependency_update": {"kind": "selected-meta-toolkit-runtime-dependency-update", "version": 4,
            "from_version": "0.8.48", "to_version": f.product.target.meta.version,
            "native_registry_update": False, "runtime_candidate": rendered}}
    f.binding = updates.update_binding(f.update)
    f.installation = {"deployment_id": "inert-deployment", "guide_id": "inert-guide",
        "destination": str(f.destination), "execution_mode": "interactive", "source_kind": "implemented"}
    f.receipt = {**f.installation, "build_id": f.update["from_build_id"],
        "package_sha256": f.update["previous_package_sha256"], "update": f.update}
    f.retained_prefix = f.product.root / "existing-retained-product" / PREFIX
    f.replacement = {"fixture": FIXTURE, "gateway": {"path": str(f.retained_prefix / "bin/toolkit_gateway"),
        "sha256": f.artifacts.candidate["binary"]["sha256"]}}
    f.selected = {"binding": deepcopy(f.binding), "installation": deepcopy(f.installation),
        "application_startup": f.replacement}
    f.admission = {"path": str(f.directory / "inert-admission.json"), "sha256": "6" * 64}
    f.files = {"deployment.json": (json.dumps({"slug": "inert-app", "mobkitVersion": "0.9.0"}).encode(), 0o644),
        "runtime-candidate.json": (json.dumps(rendered).encode(), 0o644)}
    f.previous_files = {"deployment.json": (b'{"slug":"inert-app","mobkitVersion":"0.8.48"}', 0o644)}
    f.replacement_current, f.lease_current, f.lease_active = True, True, False
    f.checkpoints, f.starts = [], []
    f.executor.directory = f.directory

    def observation_current():
        f.events.append("replacement-check")
        if not f.replacement_current:
            raise DraftConflict("The original inert replacement changed.")
    f.observation = SimpleNamespace(require_current=observation_current)

    def admission(directory, mode, selected):
        assert directory == f.directory and mode == "update" and selected == f.admission
        f.events.append("admission")
        return f.selected
    def transaction(directory, identity, *, expected_binding=None, expected_installation=None):
        assert directory == f.directory and identity == f.update["update_id"]
        f.events.append("transaction")
        if expected_binding is not None:
            updates.require_update_binding(f.update, expected_binding)
        if expected_installation is not None:
            updates.require_update_installation(f.receipt, expected_installation)
        return f.receipt, f.transaction_root, f.update
    def checkpoint(directory, root, current, **options):
        assert (directory, root, current) == (f.directory, f.transaction_root, f.update)
        f.checkpoints.append(deepcopy(options))
        for key in ("status", "runtime_admission_possible"):
            if key in options:
                f.update[key] = options[key]
    def selected_inputs(value, **options):
        assert value == f.replacement and options["binding"] == f.binding
        assert options["installation"] == f.installation and options["retain_observation"] is True
        f.events.append("selected-replacement")
        observation_current()
        return f.replacement, f.observation
    @contextmanager
    def lease(destination):
        assert destination == f.destination and not f.lease_active
        f.lease_active = True
        f.events.append("lease-enter")
        def current():
            f.events.append("lease-check")
            if not f.lease_active or not f.lease_current:
                raise RuntimeError("The original inert project lease changed.")
        f.lease = SimpleNamespace(require_current=current)
        try:
            yield f.lease
        finally:
            f.events.append("lease-exit")
            f.lease_active = False
    def artifacts(target, *, verified_artifacts=None):
        assert target == f.product.target
        if verified_artifacts is None:
            f.events.append("selected-artifacts")
            return f.artifacts
        assert verified_artifacts is f.artifacts
        f.events.append("artifact-recheck")
        return candidates.recheck_meta_toolkit_artifacts(f.artifacts, target)
    def verify_prefix(value, *, deployment_root, prefix_path):
        assert value is f.artifacts and deployment_root == f.destination and prefix_path == f.retained_prefix
        assert f.lease_active
        f.events.append("retained-prefix-readback")
        return prefix_path / "bin/toolkit_gateway"
    def retain(destination, root, **options):
        assert (destination, root) == (f.destination, f.transaction_root) and f.lease_active
        assert options["expected_version"] == "0.8.48" and options["next_runtime_target"] == f.product.target
        options["require_current"]()
        f.events.append("retain-runtime")
        options["require_current"]()
        return {"fixture": FIXTURE, "retained": True}
    def verify_runtime(destination, version, **options):
        assert destination == f.destination and version == f.product.target.meta.version
        assert options == {"runtime_target": f.product.target, "sdk_python": f.destination / ".venv/bin/python",
            "site_packages": f.executor.site}
        f.events.append("runtime-readback")
        return {"fixture": FIXTURE, "sdk": {"fixture": FIXTURE}, "native": {"fixture": FIXTURE}}
    def activate(directory, identity, **options):
        assert options["require_owner_current"] == observation_current
        options["require_owner_current"]()
        f.events.append("source-activation")
        f.receipt.update(build_id=f.update["to_build_id"], package_sha256=f.update["package_sha256"])
        f.update["status"] = "verifying"
        return f.files
    def package(directory, filename, hashed):
        assert filename in {"package.zip", "previous.zip", "candidate.zip"}
        return None, f.previous_files if filename == "previous.zip" else f.files
    def committed(directory, identity, **options):
        assert not options["restored"]
        f.events.append("committed")
        f.update["status"] = "completed"
    f.attempt = SimpleNamespace(pin=lambda: {"path": str(f.transaction_root / "inert-attempt"), "sha256": "7" * 64})
    def retain_attempt(directory, admission, selected):
        assert admission == f.admission and selected is f.selected
        f.events.append("retained-attempt")
        return f.attempt
    async def start(definition, *, on_ready, **options):
        assert options == {"application_startup": {"mode": "update", "admission": f.admission, "attempt": f.attempt}}, (
            "A selected product must never use ordinary Supervisor.start.")
        assert not f.lease_active and definition["mobkitVersion"] == "0.9.0"
        f.starts.append(options)
        f.events.append("replacement-start")
        on_ready()
    f.executor.start = start

    monkeypatch.setattr(updates, "read_update_admission", admission)
    monkeypatch.setattr(updates, "transaction", transaction)
    monkeypatch.setattr(updates, "checkpoint", checkpoint)
    monkeypatch.setattr(updates, "checked_package", package)
    monkeypatch.setattr(updates, "activate", activate)
    monkeypatch.setattr(updates, "committed", committed)
    monkeypatch.setattr(updates, "require_application_policy_continuity", lambda *args: None)
    monkeypatch.setattr(updates, "require_replacement_history_settled", lambda *args: None)
    monkeypatch.setattr(updates, "_require_runtime_framework_configuration", lambda *args: None)
    monkeypatch.setattr(bootstrap, "require_bootstrap_history_settled", lambda *args: None)
    monkeypatch.setattr(startup, "selected_inputs", selected_inputs)
    monkeypatch.setattr(startup, "retain_attempt", retain_attempt)
    monkeypatch.setattr(worker, "read_receipt", lambda directory: f.receipt)
    monkeypatch.setattr(worker, "journal", lambda *args, **kwargs: None)
    monkeypatch.setattr(worker, "verify_destination", lambda *args: None)
    monkeypatch.setattr(worker, "read_settings", lambda *args: {})
    monkeypatch.setattr(worker, "project_lock", lease)
    monkeypatch.setattr(runtime_selection, "selected_runtime_artifacts", artifacts)
    monkeypatch.setattr(installed, "verify_meta_toolkit_prefix", verify_prefix)
    monkeypatch.setattr(dependencies, "retain_runtime", retain)
    monkeypatch.setattr(dependencies, "verify_runtime", verify_runtime)
    def no_fallback(*args, **kwargs):
        pytest.fail("The selected product cannot restore source or use a different startup owner.")
    for name in ("restore", "recover_candidate", "retry_candidate"):
        monkeypatch.setattr(updates, name, no_fallback)
    return f


async def test_update_routes_pair_and_full_prefix_through_original_replacement_and_lease(routing):
    f = routing
    await worker.update_runtime(f.executor, "update", update_selection=f.admission)
    assert f.update["status"] == "completed" and len(f.starts) == 1
    assert len(f.executor.sdk_calls) == len(f.executor.prefix_calls) == 1
    args, options = f.executor.sdk_calls[0]
    assert args == (f.product.target.meta_wheel.path, f.product.target.toolkit_wheel.path)
    assert options["dependency_wheels"] == f.product.target.dependency_wheels
    assert f.executor.prefix_calls[0][0] is f.artifacts
    assert f.executor.prefix_calls[0][1]["require_current"] is options["require_current"]
    phases = [name for name in f.events if name in {
        "selected-replacement", "source-activation", "lease-enter", "retain-runtime", "environment",
        "site", "sdk-pair", "prefix-copy", "runtime-readback", "application-check", "lease-exit",
        "retained-attempt", "replacement-start", "committed"}]
    assert phases == ["selected-replacement", "source-activation", "lease-enter", "retain-runtime",
        "environment", "site", "sdk-pair", "prefix-copy", "runtime-readback", "application-check",
        "lease-exit", "runtime-readback", "selected-replacement", "retained-attempt", "replacement-start", "committed"]
    for before, after in zip(["environment", "site", "sdk-pair"], ["site", "sdk-pair", "prefix-copy"]):
        between = f.events[f.events.index(before) + 1:f.events.index(after)]
        assert {"lease-check", "transaction", "replacement-check", "artifact-recheck"} <= set(between)
    with pytest.raises(RuntimeError, match="lease changed"):
        options["require_current"]()


async def test_update_refuses_missing_original_replacement_before_any_source_or_runtime_mutation(routing):
    f = routing
    f.selected.pop("application_startup")
    with pytest.raises(DraftConflict, match="original trusted replacement"):
        await worker.update_runtime(f.executor, "update", update_selection=f.admission)
    assert not f.executor.commands and not f.executor.sdk_calls and not f.executor.prefix_calls and not f.starts
    assert "source-activation" not in f.events and "retain-runtime" not in f.events
    assert list(f.destination.iterdir()) == []


@pytest.mark.parametrize("requirements", [b"missing-package==1.0.0\n", b"fixture-dependency==9.0.0\n",
    b"fixture-dependency>=1.0.0\n", b"fixture-dependency==1.0.0 --hash=sha256:" + b"0" * 64 + b"\n"])
async def test_update_missing_application_pin_refuses_before_any_source_or_runtime_mutation(routing, requirements):
    f = routing
    f.files["requirements.application.txt"] = (requirements, 0o644)
    f.executor.before_source_change = lambda: pytest.fail("Dependency refusal must precede source preparation.")
    with pytest.raises(candidates.CandidateArtifactError, match="application|Application"):
        await worker.update_runtime(f.executor, "update", update_selection=f.admission)
    assert not f.executor.commands and not f.executor.sdk_calls and not f.executor.prefix_calls and not f.starts
    assert "source-activation" not in f.events and "retain-runtime" not in f.events
    assert list(f.destination.iterdir()) == []


async def test_update_complete_matching_application_batch_proceeds_through_original_owner(routing):
    f = routing
    hashed = f.product.target.dependency_wheels[0].sha256
    f.files["requirements.application.txt"] = (
        f"# accepted application dependency\nfixture_dependency==1.0.0 --hash=sha256:{hashed}\n".encode(), 0o644)
    await worker.update_runtime(f.executor, "update", update_selection=f.admission)
    assert f.update["status"] == "completed" and len(f.starts) == 1
    assert len(f.executor.sdk_calls) == len(f.executor.prefix_calls) == 1
    assert f.executor.sdk_calls[0][1]["dependency_wheels"] == f.product.target.dependency_wheels


@pytest.mark.parametrize("changed", ["lease", "transaction", "replacement", "artifact", "stop"])
async def test_original_owner_change_after_pair_prevents_prefix_copy_and_replacement_start(routing, changed):
    f = routing
    def drift(name):
        if name != "sdk-pair":
            return
        if changed == "lease":
            f.lease_current = False
        elif changed == "transaction":
            f.update["runtime_dependency_update"]["to_version"] = "0.9.99"
        elif changed == "replacement":
            f.replacement_current = False
        elif changed == "artifact":
            f.product.target.toolkit_wheel.path.write_bytes(b"changed inert SDK bytes")
        else:
            f.executor.stop.set()
    f.executor.after_phase = drift
    with pytest.raises((DraftConflict, candidates.CandidateArtifactError, RuntimeError, asyncio.CancelledError)):
        await worker.update_runtime(f.executor, "update", update_selection=f.admission)
    assert len(f.executor.sdk_calls) == 1 and not f.executor.prefix_calls and not f.starts
    assert "application-check" not in f.events and "replacement-start" not in f.events


async def test_unadmitted_gateway_override_precedes_any_sdk_native_or_runtime_retention(routing, monkeypatch):
    f = routing
    monkeypatch.setattr(worker, "read_settings", lambda *args: {"META_FORGE_RPC_GATEWAY_BIN": "/foreign/gateway"})
    with pytest.raises(RuntimeError, match="gateway override"):
        await worker.update_runtime(f.executor, "update", update_selection=f.admission)
    assert not f.executor.commands and not f.executor.sdk_calls and not f.executor.prefix_calls and not f.starts
    assert "retain-runtime" not in f.events
