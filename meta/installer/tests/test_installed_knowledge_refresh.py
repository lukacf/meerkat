"""Owned startup applies saved knowledge keys with inert command boundaries."""
import ast
import asyncio
from contextlib import contextmanager
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
from types import SimpleNamespace

import pytest

from launchkit import deployment_worker as worker
from launchkit import deployment_service as services
from launchkit.deployment import materialize, read_receipt, update_receipt
from launchkit.elephant_setup import generate_elephant_files
from launchkit.projects import DraftConflict
from launchkit.templates.storage_setup import project_lock
from test_deployment import Guide, Implementation, Service, submit
from test_deployment_settings import body
from test_knowledge_key_recovery import load_manager


class AppBoundary(RuntimeError):
    pass


@pytest.fixture
async def installed(tmp_path, monkeypatch, request):
    first_setup = getattr(request, "param", None) == "first-setup"
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "a" * 64
    knowledge = {"mode": "local", "token_env": "ELEPHANT_TOKEN", "space_env": "ELEPHANT_SPACE_ID"}
    implementation.definition["knowledge"] = knowledge
    implementation.definition["environment"].append({"key": "ANTHROPIC_API_KEY", "label": "Extraction key",
        "secret": True, "required": True})
    implementation.extra_files = generate_elephant_files(knowledge, "test-team", "anthropic")
    if getattr(request, "param", None) == "old-helper":
        source = Path(__file__).parent / "fixtures/knowledge_key_recovery/manage.py.before"
        assert hashlib.sha256(source.read_bytes()).hexdigest() == "01b42737cdf1c91a7b35e426ddd6ddf94a93f68e16e177f25d8e90d4e05b8918"
        implementation.extra_files["knowledge/manage.py"] = source.read_text()
    service = Service(tmp_path / "server", guide, implementation)
    destination = tmp_path / "installed"
    receipt = await submit(service, destination, settings={"OPENAI_API_KEY": "old-embedding-canary",
        "ANTHROPIC_API_KEY": "old-extraction-canary"})
    directory = service._directory(guide.identity)
    materialize(directory)
    await service.shutdown()
    update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None,
        process_live=False, runtime_pid=None, knowledge_managed=not first_setup)
    manager = load_manager(destination)
    monkeypatch.setattr(manager.getpass, "getpass", lambda *args: pytest.fail("Saved fixture keys need no prompt."))
    local = {}
    if not first_setup:
        manager.setup(manager.load_settings())
        local = manager.read_env(destination / "knowledge/.env")
        local["PRIVATE_EXTRA"] = "retained-knowledge-extra-canary"
        manager.write_env(destination / "knowledge/.env", local)
    retained = []
    retained_names = ["data/records.bin", ".mobkit/state/history.bin"]
    if not first_setup:
        retained_names.append("knowledge/.source/retained.bin")
    for name in retained_names:
        path = destination / name
        path.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
        path.write_bytes(b"Private synthetic state must remain unchanged.\n")
        path.chmod(0o600)
        retained.append(path)
    before = {path: (path.read_bytes(), path.stat().st_ino) for path in retained}
    observation = SimpleNamespace(service=service, directory=directory, destination=destination,
        definition=deepcopy(implementation.definition), manager=manager, old=local, before=before,
        calls=[], queries=[], container_keys={key: local.get(key) for key in ("OPENAI_API_KEY", "ANTHROPIC_API_KEY")},
        during=lambda phase: None, foreign=False, failed_operation=None, app_boundaries=0, supervisors=[], leases=[])
    real_lock = worker.project_lock

    @contextmanager
    def leased(root):
        with real_lock(root) as lease:
            observation.leases.append(lease)
            yield lease

    monkeypatch.setattr(worker, "project_lock", leased)
    real_class = worker.Supervisor

    class InertSupervisor(real_class):
        def __init__(self, folder):
            super().__init__(folder)
            observation.supervisors.append(self)

        async def query(self, arguments):
            self.require_child_slot()
            observation.queries.append(arguments)
            if arguments[:2] == ["docker", "ps"]:
                return "" if first_setup else "a" * 12
            assert arguments == ["docker", "inspect", "--format", "{{json .Config.Labels}}", "a" * 12]
            observation.during("preflight")
            return json.dumps({"com.docker.compose.project.working_dir": str(destination / "knowledge"),
                "com.docker.compose.project.config_files": str(destination / "knowledge/compose.json")}
                if not observation.foreign else {"com.docker.compose.project.working_dir": "/other/folder"})

        async def spawn_child(self, *arguments, **options):
            self.require_child_slot()
            assert self.leases_current()
            assert options["cwd"] == destination and options["env"] == self.environment
            assert options["stderr"] is asyncio.subprocess.STDOUT
            assert options["stdout"] is asyncio.subprocess.PIPE
            operation = arguments[-1]
            assert arguments == (str(destination / ".venv/bin/python"), "knowledge/manage.py", operation)
            observation.calls.append(operation)
            if operation == "setup":
                manager.setup(manager.load_settings())
            else:
                assert operation == "up"
                local = manager.read_env(destination / "knowledge/.env")
                observation.container_keys = {key: local[key] for key in observation.container_keys}
            observation.during(operation)
            current = manager.read_env(destination / "knowledge/.env")
            # Setup reports no new generated credentials. Up may emit any
            # current private value, after the owner refreshes its redactor.
            display = current.values() if operation == "up" else [*observation.old.values(),
                *[manager.read_env(destination / ".env")[key] for key in observation.container_keys]]
            output = (" ".join(display) + "\n").encode()

            class Pipe:
                async def read(self, size):
                    nonlocal output
                    result, output = output, b""
                    return result

            process = SimpleNamespace(returncode=7 if observation.failed_operation == operation else 0, stdout=Pipe())

            async def wait():
                return process.returncode

            process.wait = wait
            self.child = process
            return process, False

        def leases_current(self):
            observation.leases[-1].require_current()
            return True

        async def terminate(self, timeout=None):
            if self.child is not None:
                self.child.returncode = 0

    if os.environ.get("FORGE_TEST_INSTALLED_KNOWLEDGE_BASELINE") == "1":
        source = Path(__file__).parents[1] / ".artifacts/forge-installed-knowledge-refresh-20261010/deployment_worker.py.before"
        assert hashlib.sha256(source.read_bytes()).hexdigest() == "6f99b6974daaa7dfee0ee1750ccb6c121fd339b9dc4111469e09b9d9953b33a3"
        cls = next(node for node in ast.parse(source.read_bytes()).body if isinstance(node, ast.ClassDef) and node.name == "Supervisor")
        method = next(node for node in cls.body if isinstance(node, ast.AsyncFunctionDef) and node.name == "start")
        scope = dict(worker.__dict__)
        exec(compile(ast.Module(body=[method], type_ignores=[]), str(source), "exec"), scope)
        InertSupervisor.start = scope["start"]
    if os.environ.get("FORGE_TEST_MANAGED_KNOWLEDGE_BASELINE") == "1":
        source = Path(__file__).parents[1] / ".artifacts/forge-installed-knowledge-refresh-20261010/first-setup-control/deployment_worker.py.before"
        assert hashlib.sha256(source.read_bytes()).hexdigest() == "5e4488d7efae7a0e89bf8a73e1e4733b81b09c8734e5081e62aab49e56e2a0ce"
        cls = next(node for node in ast.parse(source.read_bytes()).body if isinstance(node, ast.ClassDef) and node.name == "Supervisor")
        method = next(node for node in cls.body if isinstance(node, ast.AsyncFunctionDef) and node.name == "prepare_knowledge")
        scope = dict(worker.__dict__)
        exec(compile(ast.Module(body=[method], type_ignores=[]), str(source), "exec"), scope)
        InertSupervisor.prepare_knowledge = scope["prepare_knowledge"]
    observation.supervisor = InertSupervisor(directory)
    monkeypatch.setattr(worker, "Supervisor", InertSupervisor)
    monkeypatch.setattr(worker, "require_available_console_port", lambda port: {"status": "available", "port": port})

    def app_pipe():
        observation.app_boundaries += 1
        for lease in observation.leases:
            with pytest.raises(RuntimeError, match="no longer current"):
                lease.require_current()
        raise AppBoundary("Inert native application boundary; no app is executed.")

    monkeypatch.setattr(worker.os, "pipe", app_pipe)
    monkeypatch.setattr(asyncio, "create_subprocess_exec", lambda *args, **kwargs: pytest.fail("No real child may run."))
    monkeypatch.setattr(manager.subprocess, "run", lambda *args, **kwargs: pytest.fail("No Git, Docker or provider may run."))
    monkeypatch.setattr(manager.subprocess, "Popen", lambda *args, **kwargs: pytest.fail("No subprocess may run."))
    monkeypatch.setattr(worker.os, "killpg", lambda *args: pytest.fail("No OS group may be signalled."))
    return observation


async def rotate(installed):
    receipt = read_receipt(installed.directory)
    installed.service._launch = lambda *args, **kwargs: pytest.fail("Saving keys must not launch.")
    await installed.service.configure(installed.service.guide.identity, **body(installed.service, receipt),
        settings={"OPENAI_API_KEY": "new-embedding-canary", "ANTHROPIC_API_KEY": "new-extraction-canary"})
    assert installed.container_keys == {key: installed.old[key] for key in installed.container_keys}
    assert not installed.calls


def preserved(installed):
    assert all(path.read_bytes() == data and path.stat().st_ino == inode
        for path, (data, inode) in installed.before.items())
    values = installed.manager.read_env(installed.destination / "knowledge/.env")
    assert all(values[key] == installed.old[key] for key in ("JWT_SECRET", "SURREALDB_PASS", "PRIVATE_EXTRA"))


@pytest.mark.parametrize("mode", ["start", "service"])
async def test_saved_configure_refreshes_owned_knowledge_before_current_worker_app_boundary(installed, monkeypatch, mode):
    await rotate(installed)
    supervisor = installed.supervisor
    monkeypatch.setattr(worker, "Supervisor", lambda _: supervisor)
    if mode == "service":
        update_receipt(installed.directory, execution_mode="background")
        # Service observation is inert; the real run mode still checks exact PID ownership.
        service = {"fixture": "owned service"}
        monkeypatch.setattr(services, "verify_service", lambda *args: service)
        class Manager:
            async def observe(self, selected):
                assert selected is service
                return {"pid": os.getpid()}
        monkeypatch.setattr(services, "UserServiceManager", Manager)
    async def owner(_):
        await asyncio.Event().wait()
    monkeypatch.setattr(worker, "owner_input", owner)
    monkeypatch.setattr(asyncio.get_running_loop(), "add_signal_handler", lambda *args: None)
    assert await worker.run(installed.directory, mode) == 1  # AppBoundary is deliberately not Ready.
    assert installed.calls == ["setup", "up"] and installed.app_boundaries == 1
    assert installed.container_keys == {"OPENAI_API_KEY": "new-embedding-canary", "ANTHROPIC_API_KEY": "new-extraction-canary"}
    receipt = read_receipt(installed.directory)
    assert receipt["health"] == "failed" and receipt["runtime_pid"] is None
    assert receipt["knowledge_ownership"]["working_directory"] == str(installed.destination / "knowledge")
    display = json.dumps(receipt.get("logs", []))
    assert all(value not in display for value in [*installed.old.values(), *installed.container_keys.values()])
    preserved(installed)


async def test_matching_files_still_apply_to_stale_container_on_fresh_start(installed):
    await rotate(installed)
    installed.manager.setup(installed.manager.load_settings())
    with pytest.raises(AppBoundary):
        await installed.supervisor.start(installed.definition)
    assert installed.calls == ["setup", "up"] and installed.app_boundaries == 1
    assert installed.container_keys["OPENAI_API_KEY"] == "new-embedding-canary"
    preserved(installed)


async def test_initial_install_is_not_repeated_by_same_supervisor_start(installed, monkeypatch):
    from launchkit import deployment_cache
    monkeypatch.setattr(deployment_cache, "seed_release_cache", lambda *args: None)
    real_command = installed.supervisor.command
    ordinary = []
    async def command(arguments, phase, timeout, **options):
        if "knowledge/manage.py" in arguments:
            return await real_command(arguments, phase, timeout, **options)
        ordinary.append(arguments)
    installed.supervisor.command = command
    await installed.supervisor.install(installed.definition)
    with pytest.raises(AppBoundary):
        await installed.supervisor.start(installed.definition)
    assert installed.calls == ["setup", "up"] and installed.app_boundaries == 1
    assert ordinary == [["/bin/bash", "install.sh", "--non-interactive"],
        [str(installed.destination / ".venv/bin/python"), "app.py", "--doctor"],
        [str(installed.destination / ".venv/bin/python"), "app.py", "--application-check"]]


@pytest.mark.parametrize("installed", ["first-setup"], indirect=True)
async def test_initial_install_creates_missing_owned_knowledge_credentials(installed, monkeypatch):
    from launchkit import deployment_cache
    monkeypatch.setattr(deployment_cache, "seed_release_cache", lambda *args: None)
    root = installed.destination
    before_project = installed.manager.read_env(root / ".env")
    assert not (root / "knowledge/.env").exists()
    assert not {"ELEPHANT_TOKEN", "ELEPHANT_SPACE_ID"}.intersection(before_project)
    assert read_receipt(installed.directory)["knowledge_managed"] is False
    real_command = installed.supervisor.command
    ordinary = []

    async def command(arguments, phase, timeout, **options):
        if "knowledge/manage.py" in arguments:
            return await real_command(arguments, phase, timeout, **options)
        ordinary.append(arguments)

    installed.supervisor.command = command
    await installed.supervisor.install(installed.definition)
    project = installed.manager.read_env(root / ".env")
    knowledge = installed.manager.read_env(root / "knowledge/.env")
    generated = {"ELEPHANT_TOKEN", "ELEPHANT_SPACE_ID"}
    assert {key: value for key, value in project.items() if key not in generated} == before_project
    assert installed.container_keys == {key: before_project[key] for key in ("OPENAI_API_KEY", "ANTHROPIC_API_KEY")}
    assert all(knowledge[key] for key in ("JWT_SECRET", "SURREALDB_PASS", "ELEPHANT_TOKEN", "ELEPHANT_OPERATOR_TOKEN"))
    assert knowledge["ELEPHANT_TOKEN"] != knowledge["ELEPHANT_OPERATOR_TOKEN"]
    assert project["ELEPHANT_TOKEN"] == knowledge["ELEPHANT_TOKEN"]
    assert project["ELEPHANT_SPACE_ID"] == knowledge["ELEPHANT_SPACE_ID"] == installed.definition["slug"]
    assert not {"JWT_SECRET", "SURREALDB_PASS", "ELEPHANT_OPERATOR_TOKEN"}.intersection(project)
    assert (root / "knowledge/.env").stat().st_mode & 0o777 == 0o600
    assert all(path.read_bytes() == data and path.stat().st_ino == inode
        for path, (data, inode) in installed.before.items())
    assert installed.calls == ["setup", "up"] and installed.app_boundaries == 0
    assert installed.supervisor.child is None and read_receipt(installed.directory)["knowledge_managed"] is True
    assert ordinary == [["/bin/bash", "install.sh", "--non-interactive"],
        [str(root / ".venv/bin/python"), "app.py", "--doctor"],
        [str(root / ".venv/bin/python"), "app.py", "--application-check"]]
    display = json.dumps(read_receipt(installed.directory).get("logs", []))
    assert all(value not in display for value in knowledge.values())


@pytest.mark.parametrize("missing", ["env", "JWT_SECRET", "SURREALDB_PASS", "empty-jwt", "empty-database"])
async def test_managed_knowledge_missing_retained_secrets_refuses_before_setup(installed, missing):
    path = installed.destination / "knowledge/.env"
    project_before = (installed.destination / ".env").read_bytes()
    if missing == "env":
        path.unlink()
    else:
        values = installed.manager.read_env(path)
        if missing.startswith("empty-"):
            values["JWT_SECRET" if missing == "empty-jwt" else "SURREALDB_PASS"] = ""
        else:
            values.pop(missing)
        installed.manager.write_env(path, values)
    knowledge_before = path.read_bytes() if path.exists() else None
    with pytest.raises(DraftConflict, match="Restore the original private knowledge/.env") as caught:
        await installed.supervisor.start(installed.definition)
    assert installed.calls == [] and installed.queries == [] and installed.app_boundaries == 0
    assert installed.container_keys == {key: installed.old[key] for key in installed.container_keys}
    assert (path.read_bytes() if path.exists() else None) == knowledge_before
    assert (installed.destination / ".env").read_bytes() == project_before
    assert all(path.read_bytes() == data and path.stat().st_ino == inode
        for path, (data, inode) in installed.before.items())
    assert all(value not in str(caught.value) for value in installed.old.values())


@pytest.mark.parametrize("managed,mode", [(False, "local"), (1, "local"), (True, "remote"), (True, "none")])
async def test_start_does_not_mutate_unmanaged_or_nonlocal_knowledge(installed, managed, mode):
    update_receipt(installed.directory, knowledge_managed=managed)
    installed.definition["knowledge"]["mode"] = mode
    with pytest.raises(AppBoundary):
        await installed.supervisor.start(installed.definition)
    assert installed.calls == [] and installed.queries == []


@pytest.mark.parametrize("installed", ["old-helper"], indirect=True)
async def test_old_checked_helper_cannot_restart_knowledge_with_revoked_saved_keys(installed):
    await rotate(installed)
    with pytest.raises(RuntimeError, match="Update the accepted application source") as caught:
        await installed.supervisor.start(installed.definition)
    assert installed.calls == ["setup"] and installed.app_boundaries == 0
    assert all(value not in str(caught.value) for value in installed.old.values())
    assert installed.container_keys["OPENAI_API_KEY"] == "old-embedding-canary"
    preserved(installed)


async def test_knowledge_up_failure_keeps_app_stopped_after_joined_command_cleanup(installed):
    installed.failed_operation = "up"
    with pytest.raises(RuntimeError, match="Knowledge service step failed"):
        await installed.supervisor.start(installed.definition)
    assert installed.calls == ["setup", "up"] and installed.app_boundaries == 0
    assert installed.supervisor.child is None
    assert getattr(installed.supervisor, "_knowledge_prepared", None) is None
    preserved(installed)


@pytest.mark.parametrize("boundary", ["preflight", "setup", "up"])
async def test_stop_during_preparation_prevents_next_step_and_app(installed, boundary):
    installed.during = lambda phase: installed.supervisor.stop.set() if phase == boundary else None
    with pytest.raises(asyncio.CancelledError):
        await installed.supervisor.start(installed.definition)
    assert installed.calls == ([] if boundary == "preflight" else ["setup"] if boundary == "setup" else ["setup", "up"])
    assert installed.app_boundaries == 0 and installed.supervisor.child is None
    preserved(installed)


@pytest.mark.parametrize("change", ["managed", "instance", "package", "helper", "project", "knowledge"])
async def test_changed_original_custody_or_private_input_refuses_before_setup(installed, change):
    def changed(phase):
        if phase != "preflight":
            return
        if change == "managed":
            update_receipt(installed.directory, knowledge_managed=False)
        elif change == "instance":
            update_receipt(installed.directory, service_instance_id="another-owned-instance")
        elif change == "package":
            (installed.directory / "package.zip").write_bytes(b"changed archive")
        elif change == "helper":
            (installed.destination / "knowledge/manage.py").write_text("changed source")
        else:
            path = installed.destination / (".env" if change == "project" else "knowledge/.env")
            path.write_bytes(path.read_bytes() + b"OTHER_PRIVATE='changed-canary'\n")
    installed.during = changed
    with pytest.raises((DraftConflict, ValueError)):
        await installed.supervisor.start(installed.definition)
    assert installed.calls == [] and installed.app_boundaries == 0


async def test_foreign_container_refuses_before_any_manager_mutation(installed):
    installed.foreign = True
    with pytest.raises(ValueError, match="belongs to another folder"):
        await installed.supervisor.start(installed.definition)
    assert installed.calls == [] and installed.app_boundaries == 0


async def test_setup_cannot_publish_new_unrelated_private_knowledge_setting(installed):
    def changed(phase):
        if phase == "setup":
            values = installed.manager.read_env(installed.destination / "knowledge/.env")
            values["UNEXPECTED_PRIVATE"] = "new-unrelated-canary"
            installed.manager.write_env(installed.destination / "knowledge/.env", values)
    installed.during = changed
    with pytest.raises(DraftConflict, match="Unrelated private knowledge settings"):
        await installed.supervisor.start(installed.definition)
    assert installed.calls == ["setup"] and installed.app_boundaries == 0
    assert installed.manager.read_env(installed.destination / "knowledge/.env")["UNEXPECTED_PRIVATE"] == "new-unrelated-canary"


async def test_busy_project_lease_refuses_without_entering_docker_or_manager(installed):
    with project_lock(installed.destination):
        with pytest.raises(RuntimeError, match="already running"):
            await installed.supervisor.start(installed.definition)
    assert installed.calls == [] and installed.queries == [] and installed.app_boundaries == 0


async def test_available_port_refusal_precedes_knowledge_mutation(installed, monkeypatch):
    def occupied(port):
        raise worker.LocalConsolePortUnavailable({"status": "occupied", "port": port})
    monkeypatch.setattr(worker, "require_available_console_port", occupied)
    with pytest.raises(worker.LocalConsolePortUnavailable):
        await installed.supervisor.start(installed.definition)
    assert installed.calls == [] and installed.queries == [] and installed.app_boundaries == 0


async def test_update_checked_start_reaches_same_owned_preparation(installed, monkeypatch):
    await rotate(installed)
    source = Path(worker.__file__)
    outer = next(node for node in ast.parse(source.read_bytes()).body if isinstance(node, ast.AsyncFunctionDef) and node.name == "update_runtime")
    function = next(node for node in outer.body if isinstance(node, ast.AsyncFunctionDef) and node.name == "checked_start")
    files = worker.package_files((installed.directory / "package.zip").read_bytes())
    checks = []
    async def command(arguments, phase, timeout, **options):
        assert arguments[:2] == [str(installed.destination / ".venv/bin/python"), "app.py"]
        checks.append(arguments[-1])
    updates = SimpleNamespace(checkpoint=lambda *args, **kwargs: None,
        _require_runtime_framework_configuration=lambda *args: None)
    scope = {**worker.__dict__, "supervisor": installed.supervisor, "directory": installed.directory,
        "recover_current": False, "product_update": False, "current_transaction": lambda: (read_receipt(installed.directory), installed.directory, {}),
        "updates": updates, "expected_binding": None, "expected_installation": None, "dependency_update": None,
        "native_update": False, "replacement": None, "owner_start": False, "command": command,
        "publish": lambda *args: pytest.fail("No native readiness is fabricated.")}
    exec(compile(ast.Module(body=[function], type_ignores=[]), str(source), "exec"), scope)
    with pytest.raises(AppBoundary):
        await scope["checked_start"](files, False)
    assert checks == ["--doctor", "--application-check"]
    assert installed.calls == ["setup", "up"] and installed.app_boundaries == 1
