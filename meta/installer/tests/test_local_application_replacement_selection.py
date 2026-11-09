"""Trusted replacement supplier propagation with inert product/runtime boundaries.

These source controls do not produce native replacement inputs or activate an
update. They retain the actual callback selected by the synthetic launcher.
"""
import hashlib
import json
from types import SimpleNamespace

import pytest

from launchkit.connect_installer_caller import CustodyError, ExactFile
from launchkit.local_application_registry import LocalApplicationRegistry
from launchkit.local_application_recovery import recover_local_build
from test_local_application_source import initial, create
from test_local_application_registry import (registry, new_guide, build,
    accept_source_fixture, install_fixture, product, no_product_execution, no_processes)
from test_local_application_recovery import restarted_installation, stop
from test_local_application_dispatch import dispatch, callback_snapshot, prepare_build


@pytest.fixture
def replacement_owner(monkeypatch):
    calls = []

    def supplier(*identities):
        calls.append(identities)
        pytest.fail("Source construction and ordinary reads cannot enter replacement selection.")

    original = LocalApplicationRegistry.__init__

    def selected(self, **options):
        original(self, **{**options, "replacement_selection": supplier})

    monkeypatch.setattr(LocalApplicationRegistry, "__init__", selected)
    return supplier, calls


@pytest.fixture
def selected_registry(replacement_owner, request):
    return request.getfixturevalue("registry")


@pytest.fixture
def selected_restarted(replacement_owner, request):
    return request.getfixturevalue("restarted_installation")


async def test_initial_and_continued_build_retain_original_supplier_without_entering_it(
        replacement_owner, selected_registry):
    supplier, calls = replacement_owner
    f = selected_registry
    saved = new_guide(f)
    first = await build(f, saved)
    original = f.registry.host_for_build(saved["id"], first["build_id"])
    assert original._replacement_selection is supplier
    assert f.registry._builds[(saved["id"], first["build_id"])].factory.replacement_selection is supplier
    original.require_current()
    await accept_source_fixture(f, first)
    _, _, recovered_factory, recovered_host = recover_local_build(f.registry, saved["id"], first["build_id"])
    assert recovered_factory.replacement_selection is supplier
    assert recovered_host._replacement_selection is supplier
    changed = f.store.get(saved["id"])
    changed["proposal"]["project"]["name"] += " revised"
    changed = f.store.save(changed, changed["version"])
    second = await build(f, changed, "Refine the application", previous=first)
    continued = f.registry.host_for_build(saved["id"], second["build_id"])
    assert continued is not original and continued._replacement_selection is supplier
    continued.require_current()
    assert not calls


async def test_installed_recovery_and_setup_reads_retain_supplier_without_entering_it(
        replacement_owner, selected_restarted):
    supplier, calls = replacement_owner
    f = selected_restarted
    status = await f.registry.setup_status(f.service, f.guide_id, f.directory.name)
    assert status["reason"] == "owner_reconnect" and not calls
    await stop(f)
    host = f.registry.selected_host(f.guide_id, f.directory.name)
    assert host._replacement_selection is supplier
    factory = f.registry._builds[(f.guide_id, f.installed["build_id"])].factory
    assert factory.replacement_selection is supplier
    await f.registry.setup_status(f.service, f.guide_id, f.directory.name)
    assert not calls


def test_initial_source_retains_exact_supplier_and_only_explicit_replacement_reads_enter_it(initial):
    path = initial.output.parent / "replacement-envelope.json"
    value = {"kind": "replacement", "selection_file": {}, "prior_tool_policy": {},
        "preparation_plan": {}, "gateway": {}, "association_directory": "/not-entered",
        "expected_revision": None}
    raw = json.dumps(value).encode()
    path.write_bytes(raw)
    path.chmod(0o600)
    pin = ExactFile(path, hashlib.sha256(raw).hexdigest())
    calls = []

    def supplier(*identities):
        calls.append(identities)
        return pin

    factory = create(initial, replacement_selection=supplier)
    host = factory()
    assert host._replacement_selection is supplier and not calls
    assert host._selected_replacement(initial.saved["id"], "installation", "update") == value
    assert calls == [(initial.saved["id"], "installation", "update")]
    # The existing native admission, not this read, validates the inner pins.


@pytest.mark.parametrize("where", ["factory", "host"])
def test_source_owner_rejects_replacement_supplier_substitution(initial, where):
    supplier = lambda *args: None
    factory = create(initial, replacement_selection=supplier)
    if where == "factory":
        factory.replacement_selection = lambda *args: None
    else:
        factory.host._replacement_selection = lambda *args: None
    with pytest.raises(CustodyError, match="original.*replacement|original host"):
        factory()
    with pytest.raises(CustodyError, match="original.*replacement"):
        factory.host.require_current()


async def async_supplier(*args):
    raise AssertionError("Never enter async selection.")


@pytest.mark.parametrize("supplier", [object(), async_supplier, lambda: None])
def test_invalid_supplier_refuses_before_source_publication(initial, supplier):
    with pytest.raises(CustodyError):
        create(initial, replacement_selection=supplier)
    assert not initial.output.exists()


def test_absent_supplier_preserves_unavailable_replacement(initial):
    host = create(initial)()
    assert host._replacement_selection is None
    with pytest.raises(CustodyError, match="has not retained"):
        host._selected_replacement(initial.saved["id"], "installation", "update")


def test_registry_rejects_supplier_substitution(replacement_owner, request):
    f = request.getfixturevalue("registry")
    f.registry.replacement_selection = lambda *args: None
    with pytest.raises(CustodyError, match="original services"):
        f.registry._require_registry()


def test_launcher_retains_supplier_and_refuses_changed_reselection(dispatch):
    f = dispatch

    def supplier(*args):
        pytest.fail("Launcher selection and Build cannot enter replacement selection.")

    select = f.namespace["select_local_application_runtime"]
    owner = select(f.initial.target, replacement_selection=supplier)
    assert owner.replacement_selection is supplier
    before = callback_snapshot(f)
    assert select(f.initial.target, replacement_selection=supplier) is owner
    with pytest.raises(Exception, match="original selected application runtime owner"):
        select(f.initial.target, replacement_selection=lambda *args: None)
    assert callback_snapshot(f) == before
    _, host = prepare_build(f, owner)
    assert host._replacement_selection is supplier


@pytest.fixture
def route_supplier(tmp_path, monkeypatch):
    path = tmp_path / "replacement-route-envelope.json"
    value = {"kind": "replacement", "selection_file": {}, "prior_tool_policy": {},
        "preparation_plan": {}, "gateway": {}, "association_directory": "/not-entered",
        "expected_revision": None}
    raw = json.dumps(value).encode()
    path.write_bytes(raw)
    path.chmod(0o600)
    pin = ExactFile(path, hashlib.sha256(raw).hexdigest())
    state = SimpleNamespace(calls=[], change=None, value=value)

    def supplier(*identities):
        state.calls.append(identities)
        if state.change is not None:
            state.change()
        return pin

    state.supplier = supplier
    original = LocalApplicationRegistry.__init__

    def selected(self, **options):
        original(self, **{**options, "replacement_selection": supplier})

    monkeypatch.setattr(LocalApplicationRegistry, "__init__", selected)
    return state


@pytest.fixture
def route_registry(route_supplier, request):
    return request.getfixturevalue("registry")


@pytest.fixture
async def installed_replacement_route(route_registry, route_supplier, monkeypatch):
    f = route_registry
    saved = new_guide(f)
    built = await build(f, saved)
    await accept_source_fixture(f, built)
    directory, _ = await install_fixture(f, saved, built, monkeypatch)
    f.guide_id, f.deployment_id = saved["id"], directory.name
    f.host = f.registry.selected_host(f.guide_id, f.deployment_id)
    f.selection = route_supplier
    return f


def replacement(f):
    return f.registry._selected_replacement(f.guide_id, f.deployment_id, "original-update")


async def test_actual_registry_route_refuses_host_supplier_substitution_before_entry(installed_replacement_route):
    f = installed_replacement_route

    def changed(*args):
        pytest.fail("The replacement route cannot enter a substituted supplier.")

    f.host._replacement_selection = changed
    with pytest.raises(CustodyError, match="original installed replacement selection owner changed"):
        replacement(f)
    assert not f.selection.calls


@pytest.mark.parametrize("where", ["host", "registry"])
async def test_actual_registry_route_refuses_supplier_change_during_callback(installed_replacement_route, where):
    f = installed_replacement_route
    selected = f.host if where == "host" else f.registry
    attribute = "_replacement_selection" if where == "host" else "replacement_selection"
    f.selection.change = lambda: setattr(selected, attribute, lambda *args: None)
    with pytest.raises(CustodyError, match="replacement selection owner changed|original services"):
        replacement(f)
    assert f.selection.calls == [(f.guide_id, f.deployment_id, "original-update")]


async def test_actual_registry_replacement_avoids_initial_acquisition_currentness(installed_replacement_route, monkeypatch):
    f = installed_replacement_route
    monkeypatch.setattr(f.host, "require_current",
        lambda: pytest.fail("Installed replacement cannot replay initial acquisition admission."))
    assert replacement(f) == f.selection.value
    assert f.selection.calls == [(f.guide_id, f.deployment_id, "original-update")]
