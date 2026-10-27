"""A staged product runtime update against this Forge's interpreter at Apply.

The update worker moves the installed runtime aside before it installs the
selected product, so an interpreter that product cannot use must be refused at
admission. On the shipping entry (the launchers' registry, which selects no
replacement supplier), Apply of a Meta Toolkit application that has not
finished its setup refuses first, and after setup Stage blocks a runtime change.
staged_runtime_refusal is the admission check for the replacement path. Inert
registry, product and package fixtures only; no worker, application, SDK,
provider or native program runs.
"""
import re
from uuid import uuid4

import pytest

from launchkit import deployment, deployment_updates as updates
from launchkit.local_application_registry import FIRST_SETUP_UPDATE, LocalApplicationRegistry
from launchkit.runtime_selection import candidate_selection
from test_local_application_registry import (registry, new_guide, build, accept_source_fixture,
    install_fixture, no_processes)
from test_meta_toolkit_initial_install import CP312_LINUX, ORIGINAL_INTERPRETER_REFUSAL, closure
from test_meta_toolkit_preserved_refresh import products
from test_meta_toolkit_runtime_selection import product, no_product_execution


async def staged_product_refresh(f, products, monkeypatch):
    saved = new_guide(f)
    first = await build(f, saved)
    await accept_source_fixture(f, first)
    directory, _ = await install_fixture(f, saved, first, monkeypatch)
    new_target = products[1].target
    # The restarted launcher selects the newer product for a runtime refresh.
    current_host = LocalApplicationRegistry(guide_store=f.store, implementation=f.implementation,
        deployment=f.service, runtime_target=new_target)
    f.implementation.prepare_workspace = current_host.prepare_workspace
    saved = f.store.get(saved["id"])
    refreshed = await f.implementation.submit(saved["id"], saved["version"], refresh_framework=True,
        preserve_design=True, expected_selected_build_id=first["build_id"], runtime_target=new_target)
    await accept_source_fixture(f, refreshed)
    staged = await f.service.stage_update(saved["id"], directory.name, f.store.get(saved["id"])["version"],
        refreshed["build_id"])
    update = staged["update"]
    assert update["status"] == "staged" and update["blockers"] == []
    assert update["runtime_dependency_update"] is not None
    return saved["id"], directory, update


def changed_interpreter(target, interpreter=None):
    """Forge restarted on another CPython: a cp311 closure under a cp312 Forge."""
    return ORIGINAL_INTERPRETER_REFUSAL(closure(
        "pydantic_core-2.27.2-cp311-cp311-manylinux_2_17_x86_64.manylinux2014_x86_64.whl",
        "httpx-0.28.1-py3-none-any.whl"), CP312_LINUX)


async def test_apply_before_first_setup_refuses_on_the_shipping_entry(registry, products, monkeypatch):
    f = registry
    guide_id, directory, update = await staged_product_refresh(f, products, monkeypatch)
    monkeypatch.setattr(deployment, "selected_interpreter_refusal", changed_interpreter)
    journal = updates.update_directory(directory, update["update_id"]) / "update.json"
    before = (directory / "receipt.json").read_bytes(), journal.read_bytes()
    f.service.launched.clear()
    # The browser always sends an action request. The application never
    # entered its setup, so no update worker could start it.
    with pytest.raises(ValueError, match=re.escape(FIRST_SETUP_UPDATE)):
        await f.service.action(guide_id, directory.name, "apply-update", update["update_id"],
            action_request_id=str(uuid4()))
    assert f.service.launched == [] and not f.service.tasks
    assert ((directory / "receipt.json").read_bytes(), journal.read_bytes()) == before


def staged_product_update(target):
    return {"runtime_dependency_update": {"runtime_candidate": candidate_selection(target)}}


def test_staged_runtime_refusal_names_the_cpython_a_selected_product_needs(product, monkeypatch):
    monkeypatch.setattr(deployment, "selected_interpreter_refusal", changed_interpreter)
    refusal = deployment.staged_runtime_refusal(staged_product_update(product.target))
    assert re.search(r"built for CPython 3\.11, but this Forge runs CPython 3\.12.*PYTHON_BIN", refusal)


def test_staged_runtime_refusal_admits_an_interpreter_the_product_supports(product):
    # The inert product's dependency closure is pure Python, so this Forge's
    # own interpreter can install it.
    assert deployment.staged_runtime_refusal(staged_product_update(product.target)) is None


def test_staged_runtime_refusal_names_a_glibc_floor_above_this_computer(product, monkeypatch):
    # Apply asks Stage's glibc question before the worker moves the runtime
    # aside. The inert product's floor is modelled at its cheap reader.
    from launchkit import runtime_candidates
    from test_meta_toolkit_initial_install import CP311_LINUX
    selected = candidate_selection(product.target)

    def declared(target):
        assert candidate_selection(target) == selected
        return (2, 39)
    monkeypatch.setattr(runtime_candidates, "declared_glibc_floor", declared)
    monkeypatch.setattr(deployment, "forge_interpreter", lambda: {**CP311_LINUX, "glibc_minimum": [2, 35]})
    refusal = deployment.staged_runtime_refusal(staged_product_update(product.target))
    assert re.search(r"glibc 2\.39 or newer, but this computer has glibc 2\.35", refusal or "")


@pytest.mark.parametrize("update", [{}, {"runtime_dependency_update": None},
    {"runtime_dependency_update": {"runtime_candidate": None}}], ids=["no-update", "source-only", "no-product"])
def test_staged_runtime_refusal_ignores_an_update_without_a_selected_product(update):
    assert deployment.staged_runtime_refusal(update) is None
