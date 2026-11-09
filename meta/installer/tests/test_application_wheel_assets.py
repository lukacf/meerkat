"""Offline ownership and byte-preservation controls, never live install proof."""
import asyncio
import hashlib
import io
import json
import os
from pathlib import Path
import zipfile

import pytest

from launchkit.acceptance import SemanticAcceptance
from launchkit.application_snapshot import write_trial_source
from launchkit.application_trial import source_files
from launchkit.blueprint import starter_blueprint
from launchkit.generator import PackageFile
from launchkit.implementation import update_receipt
from launchkit.implementation_workspace import ImplementationWorkspace, WorkspaceError, _sandbox_executable
from test_implementation import service, workspace as build_workspace


ASSET = "application/vendor/example_dependency-1.0-py3-none-any.whl"


def wheel_bytes(content=b"binary-wheel-canary\x00\xff"):
    output = io.BytesIO()
    with zipfile.ZipFile(output, "w", zipfile.ZIP_DEFLATED) as archive:
        archive.writestr("example_dependency/payload.bin", content)
        archive.writestr("example_dependency-1.0.dist-info/METADATA", "Name: example-dependency\nVersion: 1.0\n")
        archive.writestr("example_dependency-1.0.dist-info/WHEEL", "Wheel-Version: 1.0\nTag: py3-none-any\n")
        archive.writestr("example_dependency-1.0.dist-info/RECORD", "")
    return output.getvalue()


@pytest.fixture
def project(tmp_path):
    result = ImplementationWorkspace(tmp_path / "workspace", starter_blueprint())
    result.seed()
    return result


@pytest.fixture
def artifact(tmp_path):
    path = tmp_path / "dependency.whl"
    content = wheel_bytes()
    path.write_bytes(content)
    return path, content, hashlib.sha256(content).hexdigest()


def adopt(project, artifact, path=ASSET):
    source, _, digest = artifact
    return project.adopt_wheel(path, source, digest)


def test_host_adoption_preserves_bytes_inventory_and_metadata_only_projections(project, artifact):
    before = project.digest()
    result = adopt(project, artifact)
    assert result["digest"] != before
    assert result["sha256"] == artifact[2] and result["size"] == len(artifact[1])
    exported = project.export_files(require_verified=False)
    item = next(item for item in exported if item.path == ASSET)
    assert item.content == artifact[1] and item.as_bytes() == artifact[1]
    assert item.mode == 0o644
    sums = next(item.content for item in exported if item.path == "SHA256SUMS")
    assert artifact[2] + "  " + ASSET + "\n" in sums
    metadata = item.to_dict()
    assert set(metadata) == {"path", "mode", "binary", "size", "sha256"}
    public = json.dumps([project.list_files(), metadata, result])
    assert str(artifact[0]) not in public and "binary-wheel-canary" not in public
    state = json.loads(project._state_path.read_text())
    assert state["adopted_wheels"][ASSET] == {"sha256": artifact[2], "size": len(artifact[1]), "mode": 0o644}
    assert "source" not in state["adopted_wheels"][ASSET]
    assert adopt(project, artifact) == result


def test_agent_text_tools_cannot_read_replace_or_batch_edit_host_wheels(project, artifact):
    adopt(project, artifact)
    digest = project.digest()
    operations = [
        lambda: project.read_file(ASSET),
        lambda: project.write_file(ASSET, "replacement"),
        lambda: project.edit_file(ASSET, artifact[2], "PK", "edited"),
        lambda: project.apply_batch([{"kind": "write", "path": ASSET, "expected_sha256": artifact[2], "content": "replacement"}]),
        lambda: project.apply_batch([{"kind": "edit", "path": ASSET, "expected_sha256": artifact[2], "edits": [{"old_text": "PK", "new_text": "edited"}]}]),
    ]
    for operation in operations:
        with pytest.raises(WorkspaceError, match="Host-adopted|host-adopted"):
            operation()
        assert project.digest() == digest
        assert (project.project / ASSET).read_bytes() == artifact[1]


@pytest.mark.parametrize("path,content", [
    (ASSET, b"unadopted ordinary text"),
    (ASSET, b"unadopted\x00\xffbinary"),
    ("assets/unadopted.bin", b"\xff\x00"),
])
def test_unadopted_wheel_or_binary_cannot_enter_verified_inventory(project, path, content):
    target = project.project / path
    target.parent.mkdir(parents=True, exist_ok=True)
    target.write_bytes(content)
    with pytest.raises(WorkspaceError, match="not adopted|UTF-8|NUL"):
        project.export_files(require_verified=False)


@pytest.mark.parametrize("change", ["bytes", "mode", "missing", "ownership"])
def test_tampered_adopted_assets_or_private_ownership_are_rejected(project, artifact, change):
    adopt(project, artifact)
    path = project.project / ASSET
    if change == "bytes":
        path.write_bytes(artifact[1] + b"changed")
    elif change == "mode":
        path.chmod(0o755)
    elif change == "missing":
        path.unlink()
    else:
        state = json.loads(project._state_path.read_text())
        state["adopted_wheels"][ASSET]["sha256"] = "0" * 64
        project._state_path.write_text(json.dumps(state))
    with pytest.raises(WorkspaceError, match="wheel.*(changed|missing)|ownership"):
        project.digest()


@pytest.mark.parametrize("path", ["vendor/example.whl", "application/example.whl", "application/vendor/nested/example.whl", "application/vendor/../example.whl"])
def test_host_adoption_restricts_destination_and_hash_before_source_changes(project, artifact, path):
    before = project.digest()
    with pytest.raises(WorkspaceError):
        adopt(project, artifact, path)
    with pytest.raises(WorkspaceError, match="SHA256"):
        project.adopt_wheel(ASSET, artifact[0], "0" * 64)
    assert project.digest() == before


@pytest.mark.parametrize("kind", ["symlink", "hardlink"])
def test_host_adoption_rejects_linked_artifacts(project, artifact, tmp_path, kind):
    linked = tmp_path / "linked.whl"
    if kind == "symlink":
        linked.symlink_to(artifact[0])
    else:
        os.link(artifact[0], linked)
    before = project.digest()
    with pytest.raises(WorkspaceError, match="ordinary|links"):
        project.adopt_wheel(ASSET, linked, artifact[2])
    assert project.digest() == before


@pytest.mark.parametrize("replace", [False, True])
@pytest.mark.parametrize("failure", ["save", "interruption"])
def test_interrupted_asset_state_save_uses_existing_source_transaction_recovery(project, artifact, monkeypatch, replace, failure):
    if replace:
        adopt(project, artifact)
    before_digest = project.digest()
    before_state = project._state_path.read_bytes()
    original = (project.project / ASSET).read_bytes() if replace else None
    artifact[0].write_bytes(wheel_bytes(b"new dependency payload\x00\xfe"))
    digest = hashlib.sha256(artifact[0].read_bytes()).hexdigest()
    original_save = project._save_state

    def interrupted(state):
        if ASSET in state.get("adopted_wheels", {}):
            if failure == "interruption":
                raise KeyboardInterrupt("Controlled interrupted ownership publication")
            raise OSError("Controlled failed ownership publication")
        return original_save(state)

    monkeypatch.setattr(project, "_save_state", interrupted)
    with pytest.raises(KeyboardInterrupt if failure == "interruption" else OSError):
        project.adopt_wheel(ASSET, artifact[0], digest)
    monkeypatch.setattr(project, "_save_state", original_save)
    reopened = ImplementationWorkspace(project.root, project.blueprint)
    assert reopened.digest() == before_digest
    assert reopened._state_path.read_bytes() == before_state
    assert not reopened._batch_path.exists()
    if replace:
        assert (reopened.project / ASSET).read_bytes() == original
    else:
        assert not (reopened.project / ASSET).exists()


def test_lost_observation_after_state_commit_retains_exact_adoption_for_readback(project, artifact, monkeypatch):
    original_save = project._save_state

    def committed_then_lost(state):
        original_save(state)
        raise OSError("Controlled observation loss after commit")

    monkeypatch.setattr(project, "_save_state", committed_then_lost)
    with pytest.raises(OSError):
        adopt(project, artifact)
    reopened = ImplementationWorkspace(project.root, project.blueprint)
    assert next(row for row in reopened.list_files() if row["path"] == ASSET)["sha256"] == artifact[2]
    assert (reopened.project / ASSET).read_bytes() == artifact[1]
    assert reopened.verification() is None and not reopened._batch_path.exists()


def test_text_digest_and_projection_contract_stays_unchanged(project):
    files = project.export_files(require_verified=False)
    old_digest = hashlib.sha256(json.dumps([
        {"path": item.path, "sha256": hashlib.sha256(item.content.encode("utf-8")).hexdigest(), "mode": item.mode}
        for item in files], ensure_ascii=True, sort_keys=True, separators=(",", ":")).encode()).hexdigest()
    assert project.digest() == old_digest
    assert PackageFile("text.txt", "line\n", 0o600).to_dict() == {"path": "text.txt", "content": "line\n", "mode": 0o600}
    target = project.project / "application/newlines.txt"
    target.parent.mkdir(exist_ok=True)
    target.write_bytes(b"first\r\nsecond\r\n")
    assert project.read_file("application/newlines.txt") == "first\nsecond\n"


def test_wheel_survives_existing_trial_snapshot_reader_and_text_batch(project, artifact, tmp_path):
    adopt(project, artifact)
    project.apply_batch([{"kind": "write", "path": "application/message.txt", "expected_sha256": None, "content": "hello\n"}])
    files = project.export_files(require_verified=False)
    target = tmp_path / "trial"
    target.mkdir()
    write_trial_source(target, files)
    reread = source_files(target / "application-source")
    assert ImplementationWorkspace._digest(reread) == project.digest()
    assert next(item.content for item in reread if item.path == ASSET) == artifact[1]


def checked_application(project, path, digest):
    project.write_file("application/__init__.py", "def build(context):\n    return object()\n")
    project.write_file("tests/test_dependency.py", f'''import hashlib, pathlib, unittest
class Dependency(unittest.TestCase):
    def test_source_contains_exact_dependency(self):
        source = pathlib.Path(__file__).resolve().parents[1] / {path!r}
        self.assertEqual(hashlib.sha256(source.read_bytes()).hexdigest(), {digest!r})
''')
    result = project.check()
    assert result["ready_to_package"], result
    return result


def accepted_application(project, directory, frozen, path, digest):
    review = SemanticAcceptance(directory / "acceptance", project, frozen)
    review.begin_round()
    review.bind_run("fixture-wheel-review")
    review.write_test("test_dependency.py", f'''import hashlib, pathlib, unittest, zipfile
class Dependency(unittest.TestCase):
    def test_export_contains_ordinary_exact_wheel(self):
        source = pathlib.Path(__file__).resolve().parents[0] / {path!r}
        # The host harness imports tests from another protected directory;
        # its working directory is the actual frozen source snapshot.
        source = pathlib.Path.cwd() / {path!r}
        self.assertEqual(hashlib.sha256(source.read_bytes()).hexdigest(), {digest!r})
        with zipfile.ZipFile(source) as archive:
            self.assertTrue(any(name.endswith('.dist-info/METADATA') for name in archive.namelist()))
''')
    execution = review.check()
    assert execution["status"] == "pass", execution
    review.publish({"summary": "Offline fixture checks exact dependency source bytes only.",
        "requirements": [{"id": "requested-outcome", "status": "tested", "source_paths": [path],
            "test_ids": [row["id"] for row in execution["test_results"]],
            "evidence": "Exact wheel bytes observed in the protected source snapshot.",
            "remaining_dependencies": ["Live installation and runtime behavior remain untested."]}],
        "counterexamples": [], "remaining_dependencies": ["Live installation remains untested."]})
    review.complete_run("fixture-wheel-review", {"kind": "interaction_complete", "interaction_id": "fixture-wheel-review", "cursor": "fixture:42"})
    assert review.verification()["ready_to_package"]
    return review


async def package_flow(service, artifact):
    if not _sandbox_executable():
        pytest.skip("Protected offline checks have no unsandboxed fallback.")
    path = "application/vendor/" + artifact[0].name
    prepared = []

    def prepare(guide_id, build_id, project):
        prepared.append((guide_id, build_id, adopt(project, artifact, path)))

    service.prepare_workspace = prepare
    admitted = await service.submit(service.guide.identity, 3)
    task = service.tasks.pop(admitted["build_id"])
    task.cancel()
    await asyncio.gather(task, return_exceptions=True)
    assert prepared[0][:2] == (service.guide.identity, admitted["build_id"])
    project = build_workspace(service, admitted)
    directory = service._directory(service.guide.identity, admitted["build_id"])
    checks = checked_application(project, path, artifact[2])
    frozen = json.loads((directory / "input.json").read_text())
    review = accepted_application(project, directory, frozen, path, artifact[2])
    update_receipt(directory, status="completed", receipt=checks, checks=checks["checks"], summary="Offline wheel ownership fixture",
        unresolved=["Live installation remains untested."], cleanup_pending=False, cleanup_in_progress=False, worker_stop_confirmed=True)
    payload, _ = await service.package(service.guide.identity, admitted["build_id"])
    with zipfile.ZipFile(io.BytesIO(payload)) as archive:
        assert archive.read(path) == artifact[1]
        report = json.loads(archive.read("implementation-report.json"))
        assert report["verification"]["digest"] == project.digest() == review.verification()["source_digest"]
        for row in archive.read("SHA256SUMS").decode().splitlines():
            expected, name = row.split("  ", 1)
            assert hashlib.sha256(archive.read(name)).hexdigest() == expected
        assert not any("verification-key" in name or "workspace-state" in name for name in archive.namelist())
    assert str(artifact[0]) not in json.dumps(review.context())
    with pytest.raises(WorkspaceError, match="metadata only"):
        review.read_source(path)
    return admitted, project, review


async def test_existing_package_owner_binds_wheel_checks_acceptance_and_continuation(service, artifact):
    try:
        first, project, review = await package_flow(service, artifact)
        digest = project.digest()
        second = await service.submit(service.guide.identity, 3, "Improve the requested application")
        continued = build_workspace(service, second)
        path = "application/vendor/" + artifact[0].name
        assert (continued.project / path).read_bytes() == artifact[1]
        assert continued.verification() is None and not second["ready_to_package"]
        assert project.digest() == digest and review.verification()["ready_to_package"]
    finally:
        await service.shutdown()


async def test_real_qualified_private_wheel_through_existing_package_owner(service):
    configured = os.environ.get("FORGE_TEST_WHEEL_PATH")
    expected = os.environ.get("FORGE_TEST_WHEEL_SHA256")
    if not configured or not expected:
        pytest.skip("Set the exact read-only qualified wheel path and hash for this offline control.")
    source = Path(configured)
    content = source.read_bytes()
    assert hashlib.sha256(content).hexdigest() == expected
    try:
        _, project, review = await package_flow(service, (source, content, expected))
        path = "application/vendor/" + source.name
        frozen = review.root / "round-1/source" / path
        frozen.write_bytes(content + b"tampered")
        assert review.verification() is None
        project.write_file("application/new_fact.txt", "Changed user requirement\n")
        assert project.verification() is None
        with pytest.raises(ValueError, match="passing executable checks"):
            await service.package(service.guide.identity)
    finally:
        await service.shutdown()


async def test_failed_host_preparation_never_publishes_or_starts_build(service, artifact, monkeypatch):
    # Preparation fails before any isolated checker or generated code runs.
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: "/inert/never-executed-checker")
    def prepare(guide_id, build_id, project):
        adopt(project, artifact)
        raise WorkspaceError("Controlled preparation refusal")

    service.prepare_workspace = prepare
    with pytest.raises(WorkspaceError, match="preparation refusal"):
        await service.submit(service.guide.identity, 3)
    assert not service.tasks and not service.processes
    assert not list(service.root.glob("*/latest.json"))
    assert not list(service.root.glob("*/*/receipt.json"))
    retained = list((service.root.parent / "unadmitted-builds" / service.guide.identity).glob("*/workspace/project/" + ASSET))
    assert len(retained) == 1 and retained[0].read_bytes() == artifact[1]
