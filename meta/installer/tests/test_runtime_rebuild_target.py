"""Rebuild with updated runtime compiles for the runtime this Forge ships with.

A build without a host-selected product (an ordinary build an earlier Forge
made, here a specimen compiled without a runtime target) is rebuilt on the
launcher's selected Meta runtime, never on the compiler's MobKit default.
Without a selected runtime the rebuild is refused by name. The builder is
inert; no SDK, provider, gateway or application executes.
"""
import json

import pytest
from fastapi import HTTPException

from launchkit import server
from launchkit.compiler import MOBKIT_VERSION
from launchkit.local_application_registry import LocalApplicationRegistry
from launchkit.runtime_selection import candidate_selection, selected_runtime_version
from test_local_application_registry import (
    registry, product, no_product_execution, no_processes, build, accept_source_fixture, workspace,
)
from test_ordinary_application_registry import ordinary_guide


def package(implementation, result):
    return {item.path: item.as_bytes()
        for item in workspace(implementation, result).export_files(require_verified=False)}


async def earlier_ordinary_build(f):
    """A completed build an earlier Forge made without a product runtime."""
    saved = ordinary_guide(f)
    first = await build(f, saved, ordinary=True)
    await accept_source_fixture(f, first)
    files = package(f.implementation, first)
    assert json.loads(files["deployment.json"])["mobkitVersion"] == MOBKIT_VERSION
    return saved, first, files


def rebuild_request(saved, first):
    return server.ImplementationRequest(version=saved["version"], previous_build_id=first["build_id"],
        refresh_framework=True, request="Rebuild this application using the current runtime. Preserve its "
        "requested behavior, agent design and authored application code."
        f"\n\nRefresh the runtime in build {first['build_id']}.")


async def test_rebuild_with_updated_runtime_compiles_on_the_launchers_meta_runtime(registry, monkeypatch):
    f = registry
    saved, first, old = await earlier_ordinary_build(f)
    monkeypatch.setattr(server, "implementation", f.implementation)
    monkeypatch.setattr(server, "application_host", f.registry)
    result = await server.build_implementation(saved["id"], rebuild_request(saved, first))
    assert result["build_id"] != first["build_id"]
    files = package(f.implementation, result)
    assert json.loads(files["deployment.json"])["mobkitVersion"] == selected_runtime_version(f.target)
    assert selected_runtime_version(f.target) != MOBKIT_VERSION
    assert "meerkat-mobkit" not in files["requirements.txt"].decode()
    frozen = json.loads((f.implementation._directory(saved["id"], result["build_id"]) / "input.json").read_bytes())
    assert frozen["runtime_candidate"] == candidate_selection(f.target)
    # The rebuild keeps the application source the earlier build authored.
    assert files["application/inert.txt"] == old["application/inert.txt"]


async def test_rebuild_with_updated_runtime_without_a_selected_runtime_is_refused_by_name(registry, monkeypatch):
    f = registry
    saved, first, _ = await earlier_ordinary_build(f)
    monkeypatch.setattr(server, "implementation", f.implementation)
    monkeypatch.setattr(server, "application_host", None)
    with pytest.raises(HTTPException) as refused:
        await server.build_implementation(saved["id"], rebuild_request(saved, first))
    assert refused.value.status_code == 409
    assert "no selected Meta runtime" in refused.value.detail
    assert f.implementation.selected_build_id(saved["id"]) == first["build_id"]


async def test_rebuild_with_updated_runtime_under_a_launcher_without_a_runtime_is_refused_by_name(
        registry, monkeypatch):
    f = registry
    saved, first, _ = await earlier_ordinary_build(f)
    unselected = LocalApplicationRegistry(guide_store=f.store, implementation=f.implementation,
        deployment=f.service)
    monkeypatch.setattr(server, "implementation", f.implementation)
    monkeypatch.setattr(server, "application_host", unselected)
    with pytest.raises(HTTPException) as refused:
        await server.build_implementation(saved["id"], rebuild_request(saved, first))
    assert refused.value.status_code == 409
    assert "no selected Meta runtime" in refused.value.detail
    assert f.implementation.selected_build_id(saved["id"]) == first["build_id"]
