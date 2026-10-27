"""Runtime-only admission preserves accepted source without inheriting proof."""

import asyncio
import hashlib
import json
import os
import threading
from unittest.mock import AsyncMock, Mock

import pytest

from launchkit.acceptance import SemanticAcceptance
from launchkit.generator import PackageFile
from launchkit.implementation import read_receipt, update_receipt
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.projects import DraftConflict
import launchkit.implementation_workspace as workspace_module

from test_implementation import complete, host_receipt, service, workspace
from test_implementation_stop_route import stop_api
from test_runtime_candidates import Bundle
from launchkit.runtime_selection import RuntimeCandidateTarget


async def accepted_legacy(service, monkeypatch):
    for profile in service.guide.state["proposal"]["agent_classes"]:
        profile["model"] = "gpt-6-astra"
    original = workspace_module.generate_package

    def legacy_files(blueprint):
        files = []
        for item in original(blueprint):
            if item.path == "SHA256SUMS":
                continue
            content = item.content
            if item.path in {"deployment.json", "requirements.txt"}:
                content = content.replace("0.8.47", "0.8.42")
            files.append(PackageFile(item.path, content, item.mode))
        manifest = "".join(f"{hashlib.sha256(item.content.encode()).hexdigest()}  {item.path}\n"
                           for item in sorted(files, key=lambda item: item.path))
        return [*files, PackageFile("SHA256SUMS", manifest)]

    with monkeypatch.context() as old_compiler:
        old_compiler.setattr(workspace_module, "generate_package", legacy_files)
        first = await service.submit(service.guide.identity, 3, "Preserve the original useful behavior.")
        project = workspace(service, first)
        project.write_file("application/retained.txt", "Accepted authored context.\n")
        os.chmod(project.project / "application/retained.txt", 0o600)
        await complete(service, first)
    return first, project


async def test_runtime_refresh_copies_exact_design_authored_modes_and_tests_without_old_proof(service, monkeypatch):
    first, old = await accepted_legacy(service, monkeypatch)
    old_directory = service._directory(service.guide.identity)
    old_input = (old_directory / "input.json").read_bytes()
    old_receipt = (old_directory / "receipt.json").read_bytes()
    old_source = old.digest()
    old_files = {item.path: (item.content, item.mode) for item in old.export_files(require_verified=False)}
    (old_directory / "runtime-settings.json").write_text('{"private": "fixture only"}')
    frozen_old = json.loads(old_input)
    old_acceptance = SemanticAcceptance(old_directory / "acceptance", old, frozen_old)
    tests = {name: old_acceptance.read_test(name) for name in old_acceptance.context()["acceptance_tests"]}

    refreshed = await service.submit(service.guide.identity, 3, refresh_framework=True,
        preserve_design=True, expected_selected_build_id=first["build_id"])
    new_directory = service._directory(service.guide.identity)
    new = workspace(service, refreshed)
    files = {item.path: (item.content, item.mode) for item in new.export_files(require_verified=False)}
    for name in old_files:
        if name.startswith("application/") or name.startswith("config/") or name == "blueprint.json":
            assert files[name] == old_files[name]
    assert "meerkat-mobkit==0.8.47" in files["requirements.txt"][0]
    assert files["requirements.txt"][0] == old_files["requirements.txt"][0].replace("0.8.42", "0.8.47")
    frozen = json.loads((new_directory / "input.json").read_text())
    assert frozen["preserve_design"] is True and frozen["request"] == ""
    assert frozen["runtime_refresh_source_digest"] == new.digest()
    assert frozen["runtime_refresh_predecessor"] == {"build_id": first["build_id"], "source_digest": old_source}
    assert frozen["framework"] == new.framework_provenance()
    assert frozen["framework"]["basis"] == "runtime-refresh"
    assert frozen["framework"]["previous"] == frozen["runtime_refresh_predecessor"]
    assert frozen["implementation_requests"] == ["Preserve the original useful behavior."]
    assert frozen["previous_implementation"]["unresolved"] == json.loads(old_receipt)["unresolved"]
    retained = SemanticAcceptance(new_directory / "acceptance", new, frozen)
    assert {name: retained.read_test(name) for name in retained.context()["acceptance_tests"]} == tests
    assert retained.verification() is None and new.verification() is None
    assert retained.history()[0]["execution"] is None and retained.history()[0]["publication"] is None
    assert not refreshed["ready_to_package"] and not refreshed["acceptance_trusted"]
    assert not (new_directory / "runtime-settings.json").exists()
    assert (old_directory / "input.json").read_bytes() == old_input
    assert (old_directory / "receipt.json").read_bytes() == old_receipt
    assert old.digest() == old_source
    repeated = await service.submit(service.guide.identity, 3, refresh_framework=True,
        preserve_design=True, expected_selected_build_id=refreshed["build_id"])
    assert repeated["build_id"] == refreshed["build_id"]
    await service.shutdown()


@pytest.mark.parametrize("change", ["brief", "typed-brief", "design", "unchecked", "unaccepted", "failed"])
async def test_runtime_refresh_refuses_changed_or_unaccepted_predecessor(service, monkeypatch, change):
    if change == "typed-brief":
        service.guide.state["brief"]["synthetic_count"] = 1
    first, old = await accepted_legacy(service, monkeypatch)
    directory = service._directory(service.guide.identity)
    if change == "brief":
        service.guide.state["brief"]["mission"] = "A different requested outcome."
    elif change == "typed-brief":
        service.guide.state["brief"]["synthetic_count"] = True
    elif change == "design":
        service.guide.state["proposal"]["agent_classes"][0]["instructions"] = "A different role boundary."
    elif change == "unchecked":
        old.write_file("application/later.py", "CHANGED = True\n")
    elif change == "unaccepted":
        (directory / "acceptance" / "acceptance-state.json").unlink()
    else:
        update_receipt(directory, status="failed", ready_to_package=False)
    with pytest.raises((DraftConflict, ValueError), match="accepted|acceptance|binding"):
        await service.submit(service.guide.identity, 3, refresh_framework=True,
            preserve_design=True, expected_selected_build_id=first["build_id"])
    assert not service.tasks and len(list(service.root.glob("*/*/receipt.json"))) == 1
    await service.shutdown()


@pytest.mark.parametrize("change", ["source", "checks", "selection"])
async def test_runtime_refresh_rechecks_exact_predecessor_before_worker_admission(service, monkeypatch, change):
    first, old = await accepted_legacy(service, monkeypatch)
    directory = service._directory(service.guide.identity)
    entered, release = threading.Event(), threading.Event()
    original = ImplementationWorkspace.seed

    def paused(new, framework=None, **kwargs):
        if kwargs.get("runtime_refresh"):
            entered.set()
            assert release.wait(5)
        return original(new, framework, **kwargs)

    monkeypatch.setattr(ImplementationWorkspace, "seed", paused)
    task = asyncio.create_task(service.submit(service.guide.identity, 3, refresh_framework=True,
        preserve_design=True, expected_selected_build_id=first["build_id"]))
    try:
        assert await asyncio.to_thread(entered.wait, 5)
        if change == "source":
            old.write_file("application/later.py", "CHANGED = True\n")
        elif change == "checks":
            host_receipt(old, "fail", 1)
        else:
            (directory.parent / "latest.json").unlink()
        release.set()
        with pytest.raises((DraftConflict, LookupError), match="changed|accepted|selection|selected|saved"):
            await task
        assert not service.tasks
        assert len(list(service.root.glob("*/*/receipt.json"))) == 1
        assert read_receipt(directory)["build_id"] == first["build_id"]
    finally:
        release.set()
        await asyncio.gather(task, return_exceptions=True)
        await service.shutdown()


@pytest.mark.parametrize("options", [
    {"preserve_design": 1}, {"preserve_design": "true"},
    {"preserve_design": True},
    {"preserve_design": True, "refresh_framework": True},
    {"preserve_design": True, "refresh_framework": True, "expected_selected_build_id": None},
    {"preserve_design": True, "refresh_framework": True, "expected_selected_build_id": "selected", "request": "Change behavior"},
    {"preserve_design": True, "refresh_framework": True, "expected_selected_build_id": "selected", "trial_replay_capsule": None},
])
async def test_runtime_refresh_refuses_ambiguous_or_feedback_admission_before_preparation(service, options):
    with pytest.raises(ValueError):
        await service.submit(service.guide.identity, 3, **options)
    assert not service.root.exists() and not service.tasks


def accepted_route_status(**changes):
    return {"build_id": "accepted-build", "status": "completed", "current_guide_version": 8,
        "current": True, "source_verified": True, "acceptance_trusted": True,
        "acceptance_current": True, "ready_to_package": True, "source_digest": "a" * 64, **changes}


@pytest.mark.parametrize("candidate", [False, True])
def test_runtime_refresh_http_route_forwards_exact_selected_build(stop_api, tmp_path, candidate):
    client, endpoint = stop_api
    bundle = Bundle(tmp_path / "external-artifacts") if candidate else None
    target = RuntimeCandidateTarget(bundle.tuple_path, bundle.root, bundle.binding) if bundle else None
    endpoint.status = AsyncMock(return_value=accepted_route_status())
    endpoint.selected_runtime_target = Mock(return_value=target)
    endpoint.submit.return_value = {"build_id": "new", "status": "queued"}
    response = client.post("/api/guide/selected-guide/implementation", json={
        "version": 8, "previous_build_id": "accepted-build", "refresh_framework": True, "preserve_design": True})
    assert response.status_code == 202
    endpoint.status.assert_awaited_once_with("selected-guide")
    endpoint.selected_runtime_target.assert_called_once_with("selected-guide",
        build_id="accepted-build", source_digest="a" * 64)
    endpoint.submit.assert_awaited_once_with("selected-guide", 8, "",
        expected_selected_build_id="accepted-build", refresh_framework=True, preserve_design=True, runtime_target=target)


@pytest.mark.parametrize("changes", [
    {"build_id": "different-build"}, {"current_guide_version": 9}, {"status": "running"},
    {"current": False}, {"source_verified": False}, {"acceptance_trusted": False},
    {"acceptance_current": False}, {"ready_to_package": False},
])
def test_runtime_refresh_http_refuses_changed_or_unaccepted_selection_before_target_recovery(stop_api, changes):
    client, endpoint = stop_api
    endpoint.status = AsyncMock(return_value=accepted_route_status(**changes))
    endpoint.selected_runtime_target = Mock()
    response = client.post("/api/guide/selected-guide/implementation", json={
        "version": 8, "previous_build_id": "accepted-build", "refresh_framework": True, "preserve_design": True})
    assert response.status_code == 409
    endpoint.selected_runtime_target.assert_not_called()
    endpoint.submit.assert_not_awaited()


def test_runtime_refresh_http_refuses_source_race_at_existing_target_owner(stop_api):
    client, endpoint = stop_api
    endpoint.status = AsyncMock(return_value=accepted_route_status())
    endpoint.selected_runtime_target = Mock(side_effect=DraftConflict("The selected accepted runtime source changed."))
    response = client.post("/api/guide/selected-guide/implementation", json={
        "version": 8, "previous_build_id": "accepted-build", "refresh_framework": True, "preserve_design": True})
    assert response.status_code == 409
    endpoint.submit.assert_not_awaited()


@pytest.mark.parametrize("body", [
    {"version": 8, "preserve_design": True},
    {"version": 8, "preserve_design": True, "refresh_framework": True},
    {"version": 8, "preserve_design": True, "refresh_framework": True, "previous_build_id": None},
    {"version": 8, "preserve_design": True, "refresh_framework": True, "previous_build_id": "accepted", "request": "Change roles"},
    {"version": 8, "preserve_design": 1, "refresh_framework": True, "previous_build_id": "accepted"},
    {"version": 8, "preserve_design": "true", "refresh_framework": True, "previous_build_id": "accepted"},
    {"version": 8, "preserve_design": True, "refresh_framework": True, "previous_build_id": "accepted", "runtime_target": {}},
])
def test_runtime_refresh_http_refuses_ambiguous_selection_and_feedback(stop_api, body):
    client, endpoint = stop_api
    assert client.post("/api/guide/selected-guide/implementation", json=body).status_code == 422
    endpoint.submit.assert_not_awaited()
