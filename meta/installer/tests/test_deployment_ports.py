"""Offline bind fixtures and real temporary deployment admission; no sockets or children."""
from copy import deepcopy
import errno
import hashlib
import json
import uuid
from types import SimpleNamespace

import pytest

from launchkit import deployment_ports as ports
from launchkit import deployment as deployment
from launchkit import deployment_worker as worker
from launchkit.deployment import materialize, read_receipt, update_receipt
from test_deployment import Guide, Implementation, Service, submit


def observation(port, status="available", **extra):
    return {"scope": "local-console-port-observation", "port": port, "status": status,
            "observed_at": "2026-10-03T08:00:00+00:00", **extra}


def fake_sockets(monkeypatch, failures=(), alternate=59123, *, option_failures=(),
                 close_failures=(), creation_failure=None, platform="posix"):
    attempts, opened = [], []

    class Probe:
        def __init__(self, family, kind):
            assert (family, kind) == ("ipv4", "stream")
            if creation_failure is not None:
                raise OSError(creation_failure, "private-error-text-canary")
            self.closed = False
            self.ordinal = len(opened)
            self.events = [("open", family, kind)]
            self.options = []
            opened.append(self)

        def __enter__(self):
            return self

        def __exit__(self, *_):
            self.closed = True
            self.events.append(("close",))
            if self.ordinal < len(close_failures) and close_failures[self.ordinal] is not None:
                raise OSError(close_failures[self.ordinal], "private-error-text-canary")

        def setsockopt(self, level, option, value):
            assert (level, option, value) == ("socket-level", "reuse-address", 1)
            self.events.append(("option", level, option, value))
            self.options.append((level, option, value))
            if self.ordinal < len(option_failures) and option_failures[self.ordinal] is not None:
                raise OSError(option_failures[self.ordinal], "private-error-text-canary")

        def bind(self, address):
            self.events.append(("bind", address))
            attempts.append(address)
            if len(attempts) <= len(failures) and failures[len(attempts) - 1] is not None:
                raise OSError(failures[len(attempts) - 1], "private-error-text-canary")

        def getsockname(self):
            return "127.0.0.1", alternate

        def connect(self, *_):
            raise AssertionError("A port observation must never contact a service")

        def listen(self, *_):
            raise AssertionError("A port observation must not reserve a listener")

    monkeypatch.setattr(ports, "os", SimpleNamespace(name=platform))
    monkeypatch.setattr(ports, "socket", SimpleNamespace(AF_INET="ipv4", SOCK_STREAM="stream", socket=Probe,
                        SOL_SOCKET="socket-level", SO_REUSEADDR="reuse-address"))
    return attempts, opened


@pytest.mark.parametrize("port", [True, False, 0, 1023, 65536, "4110", 4110.0, None])
def test_invalid_port_does_not_open_a_socket(monkeypatch, port):
    attempts, opened = fake_sockets(monkeypatch)
    with pytest.raises(ValueError, match="1024 to 65535"):
        ports.observe_local_console_port(port)
    assert attempts == opened == []


def test_free_port_observation_closes_without_listener_or_alternate(monkeypatch):
    attempts, opened = fake_sockets(monkeypatch)
    result = ports.observe_local_console_port(4110, suggest_alternate=True)
    assert result["status"] == "available" and result["port"] == 4110
    assert "before startup" in result["detail"] and "alternate_port" not in result
    assert attempts == [("127.0.0.1", 4110)] and all(probe.closed for probe in opened)
    assert opened[0].events == [("open", "ipv4", "stream"),
        ("option", "socket-level", "reuse-address", 1), ("bind", ("127.0.0.1", 4110)), ("close",)]


def test_only_actual_address_in_use_is_occupied_and_suggestion_is_not_selection(monkeypatch):
    attempts, opened = fake_sockets(monkeypatch, [errno.EADDRINUSE, None])
    result = ports.observe_local_console_port(4110, suggest_alternate=True)
    assert result["status"] == "occupied" and result["port"] == 4110 and result["alternate_port"] == 59123
    assert attempts == [("127.0.0.1", 4110), ("127.0.0.1", 0)]
    assert all(probe.closed for probe in opened) and "private-error" not in json.dumps(result)
    assert all(probe.events[1] == ("option", "socket-level", "reuse-address", 1) for probe in opened)
    assert "unavailable on this computer" in result["detail"]
    assert "another application" not in result["detail"].lower()
    assert "existing application" not in str(ports.LocalConsolePortUnavailable(result)).lower()


def test_non_unix_observation_does_not_enable_reuse_address(monkeypatch):
    attempts, opened = fake_sockets(monkeypatch, platform="nt")
    result = ports.observe_local_console_port(4110)
    assert result["status"] == "available" and attempts == [("127.0.0.1", 4110)]
    assert opened[0].events == [("open", "ipv4", "stream"), ("bind", ("127.0.0.1", 4110)), ("close",)]
    assert opened[0].options == [] and opened[0].closed


@pytest.mark.parametrize("error", [errno.EACCES, errno.EADDRINUSE])
def test_reuse_option_failure_is_unknown_without_binding_or_suggesting(monkeypatch, error):
    attempts, opened = fake_sockets(monkeypatch, option_failures=[error])
    result = ports.observe_local_console_port(4110, suggest_alternate=True)
    assert result["status"] == "unknown" and "alternate_port" not in result
    assert attempts == [] and len(opened) == 1 and opened[0].closed
    assert "private-error" not in json.dumps(result)
    fake_sockets(monkeypatch, option_failures=[error])
    with pytest.raises(ports.LocalConsolePortUnavailable) as caught:
        ports.require_available_console_port(4110)
    assert caught.value.observation["status"] == "unknown"


def test_alternate_option_failure_preserves_exact_bind_conflict(monkeypatch):
    attempts, opened = fake_sockets(monkeypatch, [errno.EADDRINUSE], option_failures=[None, errno.EACCES])
    result = ports.observe_local_console_port(4110, suggest_alternate=True)
    assert result["status"] == "occupied" and "alternate_port" not in result
    assert attempts == [("127.0.0.1", 4110)] and len(opened) == 2
    assert all(probe.closed for probe in opened) and "private-error" not in json.dumps(result)


def test_socket_creation_failure_is_unknown_without_service_action(monkeypatch):
    attempts, opened = fake_sockets(monkeypatch, creation_failure=errno.EMFILE)
    result = ports.observe_local_console_port(4110, suggest_alternate=True)
    assert result["status"] == "unknown" and "alternate_port" not in result
    assert attempts == opened == [] and "private-error" not in json.dumps(result)


def test_close_failure_does_not_claim_an_available_port(monkeypatch):
    attempts, opened = fake_sockets(monkeypatch, close_failures=[errno.EIO])
    result = ports.observe_local_console_port(4110, suggest_alternate=True)
    assert result["status"] == "unknown" and "alternate_port" not in result
    assert attempts == [("127.0.0.1", 4110)] and opened[0].closed
    assert "private-error" not in json.dumps(result)


@pytest.mark.parametrize("error", [errno.EACCES, errno.EMFILE, errno.EADDRNOTAVAIL])
def test_other_bind_errors_are_unknown_not_occupied_or_available(monkeypatch, error):
    attempts, opened = fake_sockets(monkeypatch, [error])
    result = ports.observe_local_console_port(4110, suggest_alternate=True)
    assert result["status"] == "unknown" and "alternate_port" not in result
    assert len(attempts) == 1 and all(probe.closed for probe in opened)
    assert "private-error" not in json.dumps(result)
    fake_sockets(monkeypatch, [error])
    with pytest.raises(ports.LocalConsolePortUnavailable):
        ports.require_available_console_port(4110)


def test_failed_suggestion_keeps_real_occupied_observation(monkeypatch):
    _, opened = fake_sockets(monkeypatch, [errno.EADDRINUSE, errno.EMFILE])
    result = ports.observe_local_console_port(4110, suggest_alternate=True)
    assert result["status"] == "occupied" and "alternate_port" not in result
    assert all(probe.closed for probe in opened)


@pytest.fixture
def service(tmp_path, monkeypatch):
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "a" * 64
    monkeypatch.setattr(deployment, "require_available_console_port", lambda port: observation(port))
    monkeypatch.setattr(deployment, "observe_local_console_port", lambda port, **_: observation(port))
    monkeypatch.setattr(worker, "require_available_console_port", lambda port: observation(port))
    return Service(tmp_path / "private-server", guide, implementation)


def inventory(root):
    return {str(path.relative_to(root)): hashlib.sha256(path.read_bytes()).hexdigest()
            for path in root.rglob("*") if path.is_file()}


async def test_options_observe_chosen_local_port_with_exact_source_without_changing_default(service, monkeypatch):
    calls = []
    monkeypatch.setattr(deployment, "observe_local_console_port", lambda port, **kw:
        calls.append((port, kw)) or observation(port, "occupied", alternate_port=59123))
    result = await service.options(service.guide.identity, service.implementation.identity, console_port=4110)
    assert calls == [(4110, {"suggest_alternate": True})]
    assert result["console_port"] == 18088
    assert result["console_port_observation"]["port"] == 4110
    assert result["source_digest"] == "a" * 64 and result["build_id"] == service.implementation.identity
    assert not service.root.exists() and not service.tasks and not service.processes


async def test_remote_package_options_do_not_implicitly_probe_local_or_cloud_ports(service, monkeypatch):
    target = "linux" if service.guide.state["proposal"]["target"] == "mac" else "mac"
    service.guide.state["proposal"]["target"] = target
    service.implementation.original = deepcopy(service.guide.state)
    service.implementation.definition["target"] = target
    monkeypatch.setattr(deployment, "observe_local_console_port", lambda *_, **__: pytest.fail("No local probe for remote target"))
    result = await service.options(service.guide.identity, service.implementation.identity)
    assert result["supported"] is False and "console_port_observation" not in result


@pytest.mark.parametrize("status", ["occupied", "unknown"])
async def test_known_pre_admission_conflict_writes_no_settings_owner_receipt_or_worker(service, tmp_path, monkeypatch, status):
    target = tmp_path / "application"
    before = inventory(tmp_path)
    def blocked(port):
        raise ports.LocalConsolePortUnavailable(observation(port, status))
    monkeypatch.setattr(deployment, "require_available_console_port", blocked)
    with pytest.raises(ports.LocalConsolePortUnavailable):
        await submit(service, target, settings={"MOBKIT_PORT": "4110", "OPENAI_API_KEY": "synthetic-private-value"})
    assert inventory(tmp_path) == before and not target.exists()
    assert not service.root.exists() and not service.tasks and not service.processes and not service.worker_admissions


async def test_user_selected_alternate_is_admitted_and_active_recovery_does_not_probe_its_own_port(service, tmp_path, monkeypatch):
    target = tmp_path / "application"
    calls = []
    monkeypatch.setattr(deployment, "require_available_console_port", lambda port: calls.append(port) or observation(port))
    first = await submit(service, target, settings={"MOBKIT_PORT": "59123", "OPENAI_API_KEY": "synthetic-private-value"})
    try:
        assert calls == [59123] and 'MOBKIT_PORT="59123"' in (target / ".env").read_text()
        assert first["build_id"] == service.implementation.identity
        monkeypatch.setattr(deployment, "require_available_console_port", lambda *_: pytest.fail("Active exact recovery must not reject its own listener"))
        recovered = await submit(service, target, settings={"MOBKIT_PORT": "59123", "OPENAI_API_KEY": "synthetic-private-value"})
        assert recovered["deployment_id"] == first["deployment_id"] and len(service.tasks) == 1
    finally:
        await service.shutdown()


async def stopped_install(service, target):
    await submit(service, target, settings={"MOBKIT_PORT": "59123", "OPENAI_API_KEY": "synthetic-private-value"})
    directory = service._directory(service.guide.identity)
    materialize(directory)
    await service.shutdown()
    update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None)
    from launchkit.templates.storage_setup import project_lock
    with project_lock(target):
        pass
    return directory, read_receipt(directory)


async def test_stopped_restart_rejects_actual_saved_port_before_journal_or_new_task(service, tmp_path, monkeypatch):
    directory, receipt = await stopped_install(service, tmp_path / "application")
    before = inventory(tmp_path)
    calls = []
    def blocked(port):
        calls.append(port)
        raise ports.LocalConsolePortUnavailable(observation(port, "occupied"))
    monkeypatch.setattr(deployment, "require_available_console_port", blocked)
    with pytest.raises(ports.LocalConsolePortUnavailable, match="59123"):
        await service.action(service.guide.identity, receipt["deployment_id"], "restart")
    assert calls == [59123] and inventory(tmp_path) == before
    assert read_receipt(directory) == receipt and not service.tasks and not service.processes


async def test_historical_installed_settings_check_is_exact_and_does_not_expose_saved_port_or_values(service, tmp_path, monkeypatch):
    _, receipt = await stopped_install(service, tmp_path / "application")
    service.implementation.accepted = False
    calls = []
    monkeypatch.setattr(deployment, "observe_local_console_port", lambda port, **kw:
        calls.append(port) or observation(port, "occupied", alternate_port=59124))
    before = inventory(tmp_path)
    options = await service.settings_options(service.guide.identity, receipt["deployment_id"], console_port=59123)
    assert options["source_digest"] == "a" * 64 and options["deployment_id"] == receipt["deployment_id"]
    assert options["scope"] == "verified-installed-private-configuration" and calls == [59123]
    assert options["console_port"] == 18088 and "synthetic-private-value" not in json.dumps(options)
    assert inventory(tmp_path) == before and not service.tasks


async def test_stopped_same_install_busy_port_preserves_existing_private_values_and_cleanup(service, tmp_path, monkeypatch):
    directory, receipt = await stopped_install(service, tmp_path / "application")
    before = inventory(tmp_path)
    def blocked(port):
        assert port == 59124
        raise ports.LocalConsolePortUnavailable(observation(port, "occupied"))
    monkeypatch.setattr(deployment, "require_available_console_port", blocked)
    with pytest.raises(ports.LocalConsolePortUnavailable):
        await submit(service, tmp_path / "application", settings={"MOBKIT_PORT": "59124", "OPENAI_API_KEY": "new-synthetic-private-value"})
    assert inventory(tmp_path) == before and read_receipt(directory) == receipt
    assert not service.tasks and not service.processes


async def test_modified_installed_source_blocks_restart_before_port_observation(service, tmp_path, monkeypatch):
    directory, receipt = await stopped_install(service, tmp_path / "application")
    (tmp_path / "application/app.py").write_text("changed-source-fixture\n")
    before = inventory(tmp_path)
    monkeypatch.setattr(deployment, "require_available_console_port", lambda *_: pytest.fail("Source authority must be checked first"))
    with pytest.raises(ValueError, match="differs from the checked package"):
        await service.action(service.guide.identity, receipt["deployment_id"], "restart")
    assert inventory(tmp_path) == before and read_receipt(directory) == receipt
    assert not service.tasks and not service.processes


@pytest.mark.parametrize("status", ["occupied", "unknown"])
async def test_worker_race_recheck_records_real_bound_port_failure_before_pipe_or_child(service, tmp_path, monkeypatch, status):
    directory, receipt = await stopped_install(service, tmp_path / "application")
    supervisor = worker.Supervisor(directory)
    def blocked(port):
        raise ports.LocalConsolePortUnavailable(observation(port, status))
    monkeypatch.setattr(worker, "require_available_console_port", blocked)
    monkeypatch.setattr(worker.os, "pipe", lambda: pytest.fail("No readiness pipe on known port conflict"))
    monkeypatch.setattr(worker.asyncio, "create_subprocess_exec", lambda *_, **__: pytest.fail("No child on known port conflict"))
    with pytest.raises(ports.LocalConsolePortUnavailable):
        await supervisor.start(service.implementation.definition)
    observed = read_receipt(directory)
    value = observed["startup_port_observation"]
    assert value["status"] == status and value["port"] == 59123
    assert value["attempt_id"] == observed["startup_attempt_id"]
    assert value["deployment_id"] == receipt["deployment_id"] and value["package_sha256"] == receipt["package_sha256"]
    assert observed["startup_console_health"] is None and supervisor.child is None


async def test_updated_source_port_failure_binds_the_actual_activated_package_not_constructor_history(service, tmp_path, monkeypatch):
    from launchkit import deployment_updates as updates
    directory, previous = await stopped_install(service, tmp_path / "application")
    supervisor = worker.Supervisor(directory)
    service.implementation.identity = str(uuid.uuid4())
    service.implementation.digest = "b" * 64
    service.implementation.extra_files = {"application/feature.py": "FEATURE = True\n"}
    staged = await service.stage_update(service.guide.identity, previous["deployment_id"],
                                      service.guide.state["version"], service.implementation.identity)
    files = updates.activate(directory, staged["update"]["update_id"])
    current = read_receipt(directory)
    assert current["package_sha256"] != previous["package_sha256"]
    assert supervisor.receipt["package_sha256"] == previous["package_sha256"]
    def blocked(port):
        raise ports.LocalConsolePortUnavailable(observation(port, "occupied"))
    monkeypatch.setattr(worker, "require_available_console_port", blocked)
    monkeypatch.setattr(worker.os, "pipe", lambda: pytest.fail("No pipe before known port conflict"))
    with pytest.raises(ports.LocalConsolePortUnavailable):
        await supervisor.start(json.loads(files["deployment.json"][0]))
    observed = read_receipt(directory)
    assert observed["startup_port_observation"]["package_sha256"] == current["package_sha256"]
    assert observed["startup_port_observation"]["attempt_id"] == observed["startup_attempt_id"]
    assert observed["build_id"] == current["build_id"] and supervisor.child is None


def test_existing_private_port_validation_fails_closed_without_disclosing_other_values(tmp_path):
    target = tmp_path / "application"
    target.mkdir()
    (target / ".env").write_text('MOBKIT_PORT="invalid"\nOPENAI_API_KEY="synthetic-private-value"\n')
    (target / ".env").chmod(0o600)
    with pytest.raises(ValueError, match="Choose a Console port") as failure:
        ports.installed_console_port(target, 4110)
    assert "synthetic-private-value" not in str(failure.value)
