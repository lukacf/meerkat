"""Host-selected candidate admission and retained repair with inert artifacts.

Builder processes, SDK/native execution and independent test execution are not
used. The fixture records synthetic host acceptance solely to exercise the
existing HMAC/source boundary. It makes no application or runtime claim.
"""
import asyncio
from copy import deepcopy
from dataclasses import replace
import hashlib
import io
import json
import os
from pathlib import Path
import shutil
import socket
import subprocess
import uuid
import zipfile

import pytest

from launchkit.acceptance import SemanticAcceptance, _json, _sha
from launchkit import compiler
from launchkit.compiler import MOBKIT_VERSION, generate_package
from launchkit.generator import PackageFile
from launchkit.implementation import read_receipt, update_receipt
from launchkit.implementation_workspace import ImplementationWorkspace, WorkspaceError
from launchkit.projects import DraftConflict
from launchkit.runtime_selection import RuntimeCandidateTarget, candidate_selection
import launchkit.implementation_workspace as workspace_module

from test_implementation import Guide, Service, host_receipt, workspace
from test_runtime_candidates import Bundle


@pytest.fixture
def service(tmp_path, monkeypatch):
    monkeypatch.setattr("launchkit.planner.planner_capability", lambda: {"available": True})
    monkeypatch.setattr(workspace_module, "_sandbox_executable", lambda: "/inert/sandbox")
    def forbidden(*args, **kwargs):
        raise AssertionError("Candidate implementation tests cannot execute artifacts or use the network")
    for owner, name in ((subprocess, "run"), (subprocess, "Popen"), (os, "system"),
                        (socket, "create_connection")):
        monkeypatch.setattr(owner, name, forbidden)
    value = Service(tmp_path / "host", Guide())
    # Candidate packages select a synthetic target policy, independent of the
    # CI runner kernel used for actual temporary-file publication.
    selected_target = value.guide.state["proposal"]["target"]
    monkeypatch.setattr("launchkit.deployment._local_host_target", lambda: selected_target)
    for profile in value.guide.state["proposal"]["agent_classes"]:
        profile["model"] = "gpt-6-astra"
    return value


@pytest.fixture
def bundle(tmp_path):
    return Bundle(tmp_path / "external-artifacts")


def target(bundle):
    return RuntimeCandidateTarget(bundle.tuple_path, bundle.root, bundle.binding)


def frozen(service, status):
    return json.loads((service._directory(service.guide.identity, status["build_id"]) / "input.json").read_text())


async def accepted_fixture(service, status):
    """Seal synthetic offline results without running application or native code."""
    task = service.tasks.pop(status["build_id"], None)
    if task is not None:
        task.cancel()
        await asyncio.gather(task, return_exceptions=True)
    project = workspace(service, status)
    if not (project.project / "application/inert.txt").exists():
        project.write_file("application/inert.txt", "Inert authored context.\n")
        os.chmod(project.project / "application/inert.txt", 0o600)
    check = host_receipt(project)
    directory = service._directory(service.guide.identity, status["build_id"])
    update_receipt(directory, status="completed", receipt=check, checks=check["checks"],
        cleanup_pending=False, cleanup_in_progress=False, worker_stop_confirmed=True)
    acceptance = SemanticAcceptance(directory / "acceptance", project, frozen(service, status))
    acceptance.begin_round()
    run = "inert-fixture-review"
    acceptance.bind_run(run)
    acceptance.write_test("test_inert.py", "# Inert retained fixture, not executed.\n")
    with acceptance._locked():
        state = acceptance._state()
        row = acceptance._round(state)
        execution = {"id": "synthetic-host-result", "status": "pass", "tests_run": 1,
            "source_digest": row["source_digest"], "test_digest": _sha(_json(acceptance._tests(row))),
            "test_results": [{"id": "inert-host-result", "status": "pass"}],
            "detail": "Synthetic fixture only. No application or runtime executed."}
        execution["signature"] = acceptance._signed(execution)
        row["execution"] = execution
        acceptance._save(state)
    acceptance.publish({"summary": "Synthetic fixture acceptance, no runtime claim.",
        "requirements": [{"id": item["id"], "status": "tested", "source_paths": ["application/inert.txt"],
            "test_ids": ["inert-host-result"], "evidence": "Inert synthetic fixture for admission checks.",
            "remaining_dependencies": []} for item in acceptance.requirements],
        "counterexamples": [], "remaining_dependencies": ["All actual runtime behavior is untested."]})
    acceptance.complete_run(run, {"kind": "interaction_complete", "interaction_id": run, "cursor": "fixture:1"})
    return project


async def accepted_ordinary(service):
    first = await service.submit(service.guide.identity, 3)
    return first, await accepted_fixture(service, first)


async def selected_candidate(service, bundle):
    first, old = await accepted_ordinary(service)
    selected = await service.submit(service.guide.identity, 3, refresh_framework=True, preserve_design=True,
        expected_selected_build_id=first["build_id"], runtime_target=target(bundle))
    return first, old, selected, workspace(service, selected)


def refresh_options(first, bundle):
    return {"refresh_framework": True, "preserve_design": True,
            "expected_selected_build_id": first["build_id"], "runtime_target": target(bundle)}


def source_bytes(project):
    return {item.path: (item.content, item.mode) for item in project.export_files(require_verified=False)}


async def test_selected_candidate_freezes_identity_protects_source_and_keeps_ordinary_default(service, bundle):
    first, old = await accepted_ordinary(service)
    original = source_bytes(old)
    previous = service._directory(service.guide.identity)
    retained = {name: (previous / name).read_bytes() for name in ("input.json", "receipt.json")}
    selected = await service.submit(service.guide.identity, 3, **refresh_options(first, bundle))
    project = workspace(service, selected)
    data, values = frozen(service, selected), source_bytes(project)
    selection = candidate_selection(target(bundle))
    assert MOBKIT_VERSION == "0.8.47"
    assert data["runtime_candidate"] == data["framework"]["runtime_candidate"] == selection
    assert data["framework"] == project.framework_provenance()
    assert json.loads(values["runtime-candidate.json"][0]) == selection
    assert json.loads(values["deployment.json"][0])["mobkitVersion"] == "0.8.48"
    assert "meerkat-mobkit==0.8.48" in values["requirements.txt"][0]
    for name in original:
        if name.startswith(("application/", "config/")) or name in {"blueprint.json", ".env.example"}:
            assert values[name] == original[name]
    assert not selected["ready_to_package"] and project.verification() is None
    assert data["runtime_refresh_predecessor"] == {"build_id": first["build_id"], "source_digest": old.digest()}
    for name in ("runtime-candidate.json", "runtime_candidates.py", "install_candidate.py"):
        with pytest.raises(WorkspaceError, match="read-only"):
            project.write_file(name, "Replacement\n")
    assert {name: (previous / name).read_bytes() for name in retained} == retained
    assert source_bytes(old) == original
    assert all("candidate" not in item.path for item in generate_package(service.guide.state["proposal"]))
    assert "runtime_candidate" not in old.framework_provenance()
    with pytest.raises(DraftConflict):
        service.selected_runtime_target(service.guide.identity, build_id=selected["build_id"], source_digest=project.digest())
    await service.shutdown()


async def test_exact_candidate_active_and_completed_dedup_and_source_bound_export(service, bundle):
    first, old, selected, project = await selected_candidate(service, bundle)
    options = refresh_options(selected, bundle)
    repeated = await service.submit(service.guide.identity, 3, **options)
    assert repeated["build_id"] == selected["build_id"]
    await accepted_fixture(service, selected)
    resolved = service.selected_runtime_target(service.guide.identity, build_id=selected["build_id"], source_digest=project.digest())
    assert resolved == target(bundle)
    assert (await service.submit(service.guide.identity, 3, **options))["build_id"] == selected["build_id"]
    assert len(list(service.root.glob("*/*/receipt.json"))) == 2
    package, slug = await service.package(service.guide.identity, selected["build_id"])
    with zipfile.ZipFile(io.BytesIO(package)) as archive:
        report = json.loads(archive.read("implementation-report.json"))
        assert report["framework"]["runtime_candidate"] == candidate_selection(target(bundle))
        assert not any(name.endswith((".whl", ".tar.gz")) for name in archive.namelist())
    await service.shutdown()


async def test_completed_candidate_refresh_creates_fresh_checks_when_maintained_host_source_changes(service, bundle, tmp_path, monkeypatch):
    _, _, selected, old = await selected_candidate(service, bundle)
    await accepted_fixture(service, selected)
    before = source_bytes(old)
    directory = service._directory(service.guide.identity)
    retained = {name: (directory / name).read_bytes() for name in ("input.json", "receipt.json")}
    templates = tmp_path / "maintained-templates"
    shutil.copytree(compiler.TEMPLATES_V2, templates)
    shell = templates / "console_shell.css"
    shell.write_text(shell.read_text() + "\n/* Changed maintained shell source. */\n")
    monkeypatch.setattr(compiler, "TEMPLATES_V2", templates)

    refreshed = await service.submit(service.guide.identity, 3, **refresh_options(selected, bundle))
    new = workspace(service, refreshed)
    after = source_bytes(new)
    assert refreshed["build_id"] != selected["build_id"]
    assert after["console_shell.css"][0] == shell.read_text()
    for name, value in before.items():
        if name not in {*compiler.MAINTAINED_HOST_PATHS, "SHA256SUMS"}:
            assert after[name] == value
    data = frozen(service, refreshed)
    assert data["runtime_candidate"] == frozen(service, selected)["runtime_candidate"]
    assert data["runtime_refresh_predecessor"] == {"build_id": selected["build_id"], "source_digest": old.digest()}
    assert data["framework"]["current_compiler_baseline_digest"] != frozen(service, selected)["framework"]["current_compiler_baseline_digest"]
    assert not refreshed["ready_to_package"] and not refreshed["acceptance_trusted"]
    assert new.verification() is None
    assert source_bytes(old) == before
    assert {name: (directory / name).read_bytes() for name in retained} == retained
    assert len(list(service.root.glob("*/*/receipt.json"))) == 3
    await service.shutdown()


@pytest.mark.parametrize("phase", ["queued", "running", "checking"])
async def test_candidate_pending_refresh_reuse_does_not_read_current_compiler(service, bundle, monkeypatch, phase):
    _, _, selected, old = await selected_candidate(service, bundle)
    directory = service._directory(service.guide.identity)
    update_receipt(directory, status=phase)
    before = source_bytes(old)
    def unavailable(*args, **kwargs):
        raise AssertionError("A pending refresh must retain its frozen compiler input")
    monkeypatch.setattr(workspace_module, "generate_package", unavailable)
    repeated = await service.submit(service.guide.identity, 3, **refresh_options(selected, bundle))
    assert repeated["build_id"] == selected["build_id"] and repeated["status"] == phase
    assert source_bytes(old) == before and len(list(service.root.glob("*/*/receipt.json"))) == 2
    await service.shutdown()


async def test_completed_candidate_refresh_refuses_source_change_during_compiler_comparison(service, bundle, monkeypatch):
    _, _, selected, old = await selected_candidate(service, bundle)
    await accepted_fixture(service, selected)
    original = workspace_module.generate_package
    def changed_during_comparison(*args, **kwargs):
        files = original(*args, **kwargs)
        old.write_file("application/during-refresh.txt", "Changed after accepted status.\n")
        return files
    monkeypatch.setattr(workspace_module, "generate_package", changed_during_comparison)
    with pytest.raises(DraftConflict, match="source changed"):
        await service.submit(service.guide.identity, 3, **refresh_options(selected, bundle))
    assert service.selected_build_id(service.guide.identity) == selected["build_id"]
    assert len(list(service.root.glob("*/*/receipt.json"))) == 2
    assert not (await service.status(service.guide.identity))["ready_to_package"]
    assert not service.tasks
    await service.shutdown()


async def test_ordinary_completed_refresh_cannot_satisfy_a_candidate_request(service, bundle, monkeypatch):
    original = workspace_module.generate_package
    def legacy(blueprint):
        files = [PackageFile(item.path, item.content.replace("0.8.47", "0.8.42")
            if item.path in {"deployment.json", "requirements.txt"} else item.content, item.mode)
            for item in original(blueprint) if item.path != "SHA256SUMS"]
        manifest = "".join(f"{hashlib.sha256(item.content.encode()).hexdigest()}  {item.path}\n"
                           for item in sorted(files, key=lambda item: item.path))
        return [*files, PackageFile("SHA256SUMS", manifest)]
    with monkeypatch.context() as old_compiler:
        old_compiler.setattr(workspace_module, "generate_package", legacy)
        first, _ = await accepted_ordinary(service)
    ordinary = await service.submit(service.guide.identity, 3, refresh_framework=True, preserve_design=True,
        expected_selected_build_id=first["build_id"])
    ordinary_project = await accepted_fixture(service, ordinary)
    assert service.selected_runtime_target(service.guide.identity,
        build_id=ordinary["build_id"], source_digest=ordinary_project.digest()) is None
    selected = await service.submit(service.guide.identity, 3, **refresh_options(ordinary, bundle))
    assert selected["build_id"] != ordinary["build_id"]
    assert frozen(service, selected)["runtime_candidate"] == candidate_selection(target(bundle))
    await service.shutdown()


@pytest.mark.parametrize("variation", ["tuple_path", "artifact_root", "another_bundle"])
async def test_a_different_valid_candidate_locator_or_tuple_cannot_deduplicate_or_replace(service, bundle, tmp_path, variation):
    _, _, selected, project = await selected_candidate(service, bundle)
    if variation == "tuple_path":
        copied = bundle.root / "another-tuple.json"
        copied.write_bytes(bundle.tuple_path.read_bytes())
        different = replace(target(bundle), tuple_path=copied)
    elif variation == "artifact_root":
        different = replace(target(bundle), artifact_root=tmp_path)
    else:
        different = target(Bundle(tmp_path / "another-bundle"))
    candidate_selection(different)
    before = source_bytes(project)
    with pytest.raises(DraftConflict, match="separately bound"):
        await service.submit(service.guide.identity, 3, **dict(refresh_options(selected, bundle), runtime_target=different))
    assert service.selected_build_id(service.guide.identity) == selected["build_id"]
    assert source_bytes(project) == before and len(list(service.root.glob("*/*/receipt.json"))) == 2
    await service.shutdown()


@pytest.mark.parametrize("field,value", [("source_sha", "b" * 40), ("sdk_wheel_sha256", "0" * 64),
    ("native_manifest_sha256", "0" * 64), ("native_run_id", 24), ("native_run_attempt", 2),
    ("platform", "x86_64-apple-darwin")])
async def test_caller_identity_drift_refuses_before_a_build_can_be_returned(service, bundle, field, value):
    first, old = await accepted_ordinary(service)
    before = source_bytes(old)
    changed = replace(target(bundle), accepted=replace(bundle.binding, **{field: value}))
    with pytest.raises(ValueError):
        await service.submit(service.guide.identity, 3, **dict(refresh_options(first, bundle), runtime_target=changed))
    assert service.selected_build_id(service.guide.identity) == first["build_id"]
    assert source_bytes(old) == before and len(list(service.root.glob("*/*/receipt.json"))) == 1
    await service.shutdown()


@pytest.mark.parametrize("variation", ["dictionary", "non-refresh", "no-predecessor", "feedback"])
async def test_only_typed_explicit_preserve_design_candidate_admission_is_allowed(service, bundle, variation):
    options = {"runtime_target": target(bundle)}
    if variation == "dictionary":
        options["runtime_target"] = candidate_selection(target(bundle))
    elif variation == "no-predecessor":
        options.update(preserve_design=True, refresh_framework=True)
    elif variation == "feedback":
        options.update(preserve_design=True, refresh_framework=True, expected_selected_build_id=str(uuid.uuid4()), request="Change behavior")
    with pytest.raises(ValueError):
        await service.submit(service.guide.identity, 3, **options)
    assert not service.root.exists() and not service.tasks


@pytest.mark.parametrize("change", ["brief", "design", "unchecked", "unaccepted", "failed"])
async def test_candidate_requires_exact_unchanged_independently_accepted_predecessor(service, bundle, change):
    first, old = await accepted_ordinary(service)
    directory = service._directory(service.guide.identity)
    if change == "brief":
        service.guide.state["brief"]["mission"] = "Different purpose"
    elif change == "design":
        service.guide.state["proposal"]["agent_classes"][0]["instructions"] = "Different expertise"
    elif change == "unchecked":
        old.write_file("application/later.txt", "Unaccepted source\n")
    elif change == "unaccepted":
        (directory / "acceptance" / "acceptance-state.json").unlink()
    else:
        update_receipt(directory, status="failed", ready_to_package=False)
    with pytest.raises((DraftConflict, ValueError)):
        await service.submit(service.guide.identity, 3, **refresh_options(first, bundle))
    assert service.selected_build_id(service.guide.identity) == first["build_id"]
    assert not service.tasks and len(list(service.root.glob("*/*/receipt.json"))) == 1
    await service.shutdown()


async def test_candidate_artifact_drift_after_seed_refuses_before_worker_publication(service, bundle, monkeypatch):
    first, old = await accepted_ordinary(service)
    before = source_bytes(old)
    seed = ImplementationWorkspace.seed
    def mutate_after_seed(project, framework=None, **kwargs):
        result = seed(project, framework, **kwargs)
        if kwargs.get("runtime_target"):
            bundle.wheel_path.write_bytes(bundle.wheel_path.read_bytes() + b"drift")
        return result
    monkeypatch.setattr(ImplementationWorkspace, "seed", mutate_after_seed)
    with pytest.raises(ValueError, match="bytes|wheel"):
        await service.submit(service.guide.identity, 3, **refresh_options(first, bundle))
    assert service.selected_build_id(service.guide.identity) == first["build_id"]
    assert source_bytes(old) == before and not service.tasks
    assert len(list(service.root.glob("*/*/receipt.json"))) == 1
    await service.shutdown()


async def test_candidate_repair_and_reload_preserve_protected_bytes_without_compiler_or_bundle(service, bundle, monkeypatch):
    _, _, selected, old = await selected_candidate(service, bundle)
    await accepted_fixture(service, selected)
    before, provenance = source_bytes(old), old.framework_provenance()
    bundle.tuple_path.unlink()
    def forbidden(*args, **kwargs):
        raise AssertionError("Retained candidate source cannot recompile or read external artifacts")
    monkeypatch.setattr(workspace_module, "generate_package", forbidden)
    monkeypatch.setattr(workspace_module, "generate_runtime_refresh", forbidden)
    monkeypatch.setattr("launchkit.runtime_selection.candidate_selection", forbidden)
    assert service.selected_runtime_target(service.guide.identity,
        build_id=selected["build_id"], source_digest=old.digest()) == target(bundle)
    repaired = await service.submit(service.guide.identity, 3, "Improve the requested outcome",
        expected_selected_build_id=selected["build_id"])
    new = workspace(service, repaired)
    assert source_bytes(new) == before
    data = frozen(service, repaired)
    assert data["runtime_candidate"] == provenance["runtime_candidate"]
    assert data["framework"]["runtime_candidate"] == provenance["runtime_candidate"]
    assert data["framework"]["current_compiler_baseline_digest"] == provenance["current_compiler_baseline_digest"]
    assert not repaired["ready_to_package"] and new.verification() is None
    assert data["framework"]["origin"] == {"build_id": selected["build_id"], "source_digest": old.digest()}
    reloaded = ImplementationWorkspace(new.root, service.guide.state["proposal"])
    reloaded.seed()
    assert source_bytes(reloaded) == before
    await service.shutdown()


@pytest.mark.parametrize("change", ["input-selection", "input-framework", "state-hmac", "selection-source",
    "selection-mode", "source", "checks", "acceptance", "status", "latest", "digest"])
async def test_selected_runtime_recovery_refuses_unbound_or_unaccepted_source(service, bundle, change):
    _, _, selected, project = await selected_candidate(service, bundle)
    await accepted_fixture(service, selected)
    directory = service._directory(service.guide.identity)
    source = project.digest()
    if change.startswith("input-"):
        data = frozen(service, selected)
        if change == "input-selection":
            data["runtime_candidate"]["accepted"]["source_sha"] = "b" * 40
        else:
            data["framework"].pop("runtime_candidate")
        (directory / "input.json").write_text(json.dumps(data))
    elif change == "state-hmac":
        data = json.loads(project._state_path.read_text())
        data["framework"]["runtime_candidate"]["accepted"]["source_sha"] = "b" * 40
        project._state_path.write_text(json.dumps(data))
    elif change == "selection-source":
        (project.project / "runtime-candidate.json").write_text("{}\n")
    elif change == "selection-mode":
        os.chmod(project.project / "runtime-candidate.json", 0o600)
    elif change == "source":
        project.write_file("application/later.txt", "Changed source\n")
    elif change == "checks":
        host_receipt(project, "fail", 1)
    elif change == "acceptance":
        (directory / "acceptance" / "acceptance-state.json").unlink()
    elif change == "status":
        update_receipt(directory, status="failed", ready_to_package=False)
    elif change == "latest":
        (directory.parent / "latest.json").write_text(json.dumps({"build_id": str(uuid.uuid4())}))
    else:
        source = "0" * 64
    with pytest.raises((DraftConflict, ValueError)):
        service.selected_runtime_target(service.guide.identity, build_id=selected["build_id"], source_digest=source)
    assert not service.tasks
    await service.shutdown()


@pytest.mark.parametrize("claim", ["input", "authored-json"])
async def test_json_claim_without_host_selected_framework_cannot_admit_candidate(service, bundle, claim):
    first, project = await accepted_ordinary(service)
    directory = service._directory(service.guide.identity)
    if claim == "input":
        data = frozen(service, first)
        data["runtime_candidate"] = candidate_selection(target(bundle))
        (directory / "input.json").write_text(json.dumps(data))
    else:
        project.write_file("runtime-candidate.json", json.dumps(candidate_selection(target(bundle))))
        await accepted_fixture(service, first)
    with pytest.raises(DraftConflict):
        service.selected_runtime_target(service.guide.identity, build_id=first["build_id"], source_digest=project.digest())
    assert not service.tasks
    await service.shutdown()
