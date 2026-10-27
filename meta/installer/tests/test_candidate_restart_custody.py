"""Ordinary cold-restart receipt custody with inert source and artifacts.

The real Supervisor.start path stops before any port probe or process. The
ordinary positive control stops at a substituted port-preflight sentinel.
No SDK payload, native binary, application or provider is executed.
"""
import asyncio
from copy import deepcopy
import json

import pytest

from launchkit import deployment, deployment_worker as worker
from launchkit import runtime_dependencies, runtime_selection
from test_candidate_deployment_updates import crossing, installation


class PortPreflightReached(Exception):
    """Positive-control boundary, before journal writes and process creation."""


def snapshot(root):
    return {str(path.relative_to(root)): (path.read_bytes(), path.stat().st_mode & 0o777)
        for path in root.rglob("*") if path.is_file() and not path.is_symlink()}


@pytest.fixture
def ordinary_restart(tmp_path, crossing):
    directory, destination = installation(tmp_path, crossing)
    settings = destination / ".env"
    settings.write_text("MOBKIT_PORT=18889\nSYNTHETIC_PRIVATE=preserve\n")
    settings.chmod(0o600)
    supervisor = worker.Supervisor(directory)
    _, target, previous, _, _, _ = crossing
    selected = runtime_selection.candidate_selection(target)
    definition = json.loads(previous["deployment.json"][0])
    assert definition["mobkitVersion"] == "0.8.47"
    assert "runtimeCandidate" not in definition
    assert "runtime_candidate" not in deployment.read_receipt(directory)
    return directory, destination, supervisor, previous, definition, selected


def add_carrier(directory, selected, carrier):
    value = None if carrier == "null" else deepcopy(selected)
    if carrier == "foreign":
        value["accepted"]["source_sha"] = "f" * 40
    deployment.update_receipt(directory, runtime_candidate=value)


def guard_boundaries(monkeypatch):
    events = []

    def port_preflight(port):
        assert port == 18889
        events.append("port-preflight")
        raise PortPreflightReached("Ordinary source reached the port-preflight boundary.")

    def forbidden(*args, **kwargs):
        pytest.fail("Ordinary restart custody must precede artifact/runtime/process work.")

    monkeypatch.setattr(worker, "require_available_console_port", port_preflight)
    monkeypatch.setattr(asyncio, "create_subprocess_exec", forbidden)
    monkeypatch.setattr(worker.os, "pipe", forbidden)
    monkeypatch.setattr(runtime_dependencies, "verify_runtime", forbidden)
    monkeypatch.setattr(runtime_selection, "read_candidate_artifacts", forbidden)
    return events


@pytest.mark.parametrize("carrier", ["null", "equal", "foreign"])
async def test_restart_ordinary_source_refuses_every_candidate_carrier_before_port_or_process(
        ordinary_restart, monkeypatch, carrier):
    directory, destination, supervisor, _, definition, selected = ordinary_restart
    add_carrier(directory, selected, carrier)
    events = guard_boundaries(monkeypatch)
    before = snapshot(destination), snapshot(directory)
    with pytest.raises(RuntimeError, match="admitted candidate target differs from this owned deployment source"):
        await supervisor.start(definition)
    assert not events
    assert (snapshot(destination), snapshot(directory)) == before


@pytest.mark.parametrize("carrier", ["null", "equal", "foreign"])
def test_restart_ordinary_source_target_parser_refuses_every_candidate_carrier(
        ordinary_restart, monkeypatch, carrier):
    directory, destination, supervisor, previous, _, selected = ordinary_restart
    add_carrier(directory, selected, carrier)
    events = guard_boundaries(monkeypatch)
    before = snapshot(destination), snapshot(directory)
    with pytest.raises(RuntimeError, match="admitted candidate target differs from this owned deployment source"):
        supervisor._candidate_runtime_target(previous)
    assert not events
    assert (snapshot(destination), snapshot(directory)) == before


async def test_restart_ordinary_source_without_carrier_reaches_only_port_preflight(
        ordinary_restart, monkeypatch):
    directory, destination, supervisor, _, definition, _ = ordinary_restart
    events = guard_boundaries(monkeypatch)
    before = snapshot(destination), snapshot(directory)
    with pytest.raises(PortPreflightReached):
        await supervisor.start(definition)
    assert events == ["port-preflight"]
    assert (snapshot(destination), snapshot(directory)) == before


def test_restart_ordinary_source_without_carrier_has_no_candidate_target(ordinary_restart, monkeypatch):
    directory, destination, supervisor, previous, _, _ = ordinary_restart
    events = guard_boundaries(monkeypatch)
    before = snapshot(destination), snapshot(directory)
    assert supervisor._candidate_runtime_target(previous) is None
    assert not events
    assert (snapshot(destination), snapshot(directory)) == before
