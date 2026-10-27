"""Read-only retained application ZIP downloads, without runtime or server stores."""
import ast
import json
import logging
import os
from pathlib import Path
import stat
import uuid
from unittest.mock import AsyncMock

from fastapi import FastAPI, HTTPException, Query
from fastapi.responses import JSONResponse, Response
from fastapi.testclient import TestClient
import pytest

from launchkit import connect_accepted_source, connect_installer_caller, deployment
from launchkit.deployment import materialize, read_receipt, update_receipt
from launchkit.projects import DraftConflict
from test_deployment import Guide, Implementation, Service, submit


PRIVATE = "installed-private-package-canary"


def disk_bytes(root):
    return {path.relative_to(root): path.read_bytes() for path in root.rglob("*") if path.is_file()}


@pytest.fixture
async def installed(tmp_path):
    guide = Guide()
    implementation = Implementation(guide)
    implementation.digest = "a" * 64
    service = Service(tmp_path / "server", guide, implementation)
    destination = tmp_path / "installed"
    initial = await submit(service, destination, settings={"OPENAI_API_KEY": PRIVATE})
    directory = service._directory(guide.identity)
    materialize(directory)
    await service.shutdown()
    update_receipt(directory, status="stopped", interactive_lifecycle_id=None, interactive_cleanup=None,
                   process_live=False, runtime_pid=None)
    (destination / "data").mkdir()
    (destination / "data/records.json").write_text('{"keep":"saved application record"}')
    return service, directory, destination, initial


async def download(installed, **changes):
    service, directory, destination, initial = installed
    selected = {"deployment_id": initial["deployment_id"], "build_id": initial["build_id"],
                "source_digest": "a" * 64, "package_sha256": initial["package_sha256"]}
    selected.update(changes)
    return await service.installed_package(service.guide.identity, **selected)


@pytest.mark.parametrize("status", ["stopped", "running"])
async def test_installed_a_download_ignores_failed_unaccepted_b_and_never_reads_private_values(installed, monkeypatch, status):
    service, directory, destination, initial = installed
    package = (directory / "package.zip").read_bytes()
    service.implementation.identity = str(uuid.uuid4())
    service.implementation.accepted = False
    service.implementation.digest = "b" * 64
    service.implementation.status = AsyncMock(side_effect=AssertionError("new candidate status must not supply installed bytes"))
    service.implementation.package = AsyncMock(side_effect=AssertionError("new candidate must not be packaged"))
    monkeypatch.setattr(service.implementation, "selected_build_id",
        lambda *args: pytest.fail("installed download must not read the selected implementation"))
    service.guide.state["version"] += 1
    service.guide.state["proposal"]["project"]["name"] = "Changed unaccepted design"
    service.guide.get = AsyncMock(side_effect=AssertionError("installed download must not read the current guide"))
    update_receipt(directory, status=status, process_live=status == "running",
                   runtime_pid=987654 if status == "running" else None)
    before = disk_bytes(service.root), disk_bytes(destination)
    original_open = Path.open
    original_fd_open = connect_accepted_source.os.open
    destination_info = destination.stat()
    def public_source_only(path, *args, **kwargs):
        if path == destination / ".env":
            raise AssertionError("installed ZIP download must not read private settings")
        return original_open(path, *args, **kwargs)
    def public_descriptor_only(path, flags, *args, **kwargs):
        private = Path(path) == destination / ".env"
        parent_fd = kwargs.get("dir_fd")
        if path == ".env" and parent_fd is not None:
            parent = os.fstat(parent_fd)
            private = (parent.st_dev, parent.st_ino) == (destination_info.st_dev, destination_info.st_ino)
        if private:
            raise AssertionError("installed ZIP download must not open private settings")
        return original_fd_open(path, flags, *args, **kwargs)
    with monkeypatch.context() as guard:
        guard.setattr(Path, "open", public_source_only)
        guard.setattr(connect_accepted_source.os, "open", public_descriptor_only)
        guard.setattr(deployment, "private_settings", lambda *args, **kwargs: pytest.fail("private values were requested"))
        payload = await download(installed)
    assert type(payload) is bytes and payload == package
    assert PRIVATE.encode() not in payload
    assert (disk_bytes(service.root), disk_bytes(destination)) == before
    assert read_receipt(directory)["build_id"] == initial["build_id"] != service.implementation.identity
    service.guide.get.assert_not_awaited()
    service.implementation.status.assert_not_awaited()
    service.implementation.package.assert_not_awaited()
    assert not service.tasks and not service.processes


@pytest.mark.parametrize("field", ["deployment_id", "build_id", "source_digest", "package_sha256"])
async def test_download_requires_each_exact_installed_binding_field(installed, field):
    service, directory, destination, initial = installed
    before = disk_bytes(service.root), disk_bytes(destination)
    wrong = str(uuid.uuid4()) if field.endswith("_id") else "f" * 64
    with pytest.raises((DraftConflict, LookupError)):
        await download(installed, **{field: wrong})
    assert (disk_bytes(service.root), disk_bytes(destination)) == before


@pytest.mark.parametrize("kind", ["native-team", "unknown-source"])
async def test_download_refuses_nonapplication_installed_sources(installed, kind):
    service, directory, destination, initial = installed
    update_receipt(directory, source_kind=kind)
    before = disk_bytes(service.root), disk_bytes(destination)
    with pytest.raises(DraftConflict):
        await download(installed)
    assert (disk_bytes(service.root), disk_bytes(destination)) == before


@pytest.mark.parametrize("change", ["owner", "source", "source-hardlink", "archive", "archive-symlink", "archive-hardlink", "archive-fifo"])
async def test_download_refuses_tampered_owner_source_or_retained_archive(installed, monkeypatch, change):
    service, directory, destination, initial = installed
    package = directory / "package.zip"
    if change == "owner":
        marker = destination / deployment.OWNER_FILE
        owner = json.loads(marker.read_text())
        owner["deployment_id"] = str(uuid.uuid4())
        marker.write_text(json.dumps(owner))
    elif change == "source":
        (destination / "app.py").write_text("changed installed source\n")
    elif change == "source-hardlink":
        os.link(destination / "app.py", destination / "retained-app.py")
    elif change == "archive":
        package.write_bytes(b"changed retained archive\n")
    else:
        retained = directory / "retained.zip"
        package.rename(retained)
        if change == "archive-symlink":
            package.symlink_to(retained)
        elif change == "archive-fifo":
            os.mkfifo(package, mode=0o600)
        else:
            os.link(retained, package)
    before = disk_bytes(service.root), disk_bytes(destination)
    opened = []
    if change == "archive-fifo":
        original_open = connect_installer_caller.os.open
        def bounded_open(path, flags, *args, **kwargs):
            if Path(path) == package:
                opened.append(flags)
                assert flags & os.O_NONBLOCK and flags & os.O_NOFOLLOW
            return original_open(path, flags, *args, **kwargs)
        monkeypatch.setattr(connect_installer_caller.os, "open", bounded_open)
    with pytest.raises(DraftConflict):
        await download(installed)
    assert (disk_bytes(service.root), disk_bytes(destination)) == before
    if change == "archive-fifo":
        assert opened and stat.S_ISFIFO(package.lstat().st_mode)
    assert not service.tasks and not service.processes


@pytest.mark.parametrize("kind", ["latest", "source"])
async def test_download_refuses_fifo_substitution_at_actual_selection_or_source_open(installed, monkeypatch, kind):
    service, directory, destination, initial = installed
    target = directory.parent / "latest.json" if kind == "latest" else destination / "app.py"
    parent = target.parent.stat()
    reader = connect_installer_caller if kind == "latest" else connect_accepted_source
    original_open = reader.os.open
    substituted = False
    newer = None
    def substitute_fifo(path, flags, *args, **kwargs):
        nonlocal substituted, newer
        parent_fd = kwargs.get("dir_fd")
        selected = kind == "latest" and Path(path) == target
        if kind == "source" and path == target.name and parent_fd is not None:
            observed = os.fstat(parent_fd)
            selected = (observed.st_dev, observed.st_ino) == (parent.st_dev, parent.st_ino)
        if selected and not substituted:
            assert flags & os.O_NONBLOCK and flags & os.O_NOFOLLOW
            target.unlink()
            os.mkfifo(target, mode=0o600)
            substituted = True
            newer = disk_bytes(service.root), disk_bytes(destination)
        return original_open(path, flags, *args, **kwargs)
    monkeypatch.setattr(reader.os, "open", substitute_fifo)
    with pytest.raises(DraftConflict):
        await download(installed)
    assert substituted and stat.S_ISFIFO(target.lstat().st_mode)
    assert (disk_bytes(service.root), disk_bytes(destination)) == newer
    assert not service.tasks and not service.processes


async def test_download_refuses_a_linked_latest_installation_pointer(installed):
    service, directory, destination, initial = installed
    latest = directory.parent / "latest.json"
    retained = directory.parent / "retained-latest.json"
    latest.rename(retained)
    latest.symlink_to(retained)
    before = disk_bytes(service.root), disk_bytes(destination)
    with pytest.raises(DraftConflict):
        await download(installed)
    assert latest.is_symlink() and (disk_bytes(service.root), disk_bytes(destination)) == before


@pytest.mark.parametrize("target", ["linux", "gcp"])
async def test_download_refuses_nonlocal_installed_destinations(installed, target):
    service, directory, destination, initial = installed
    update_receipt(directory, target=target)
    before = disk_bytes(service.root), disk_bytes(destination)
    with pytest.raises(DraftConflict):
        await download(installed)
    assert (disk_bytes(service.root), disk_bytes(destination)) == before


async def test_download_preserves_an_incomplete_source_update(installed):
    service, directory, destination, initial = installed
    update_receipt(directory, update={"status": "applying", "update_id": str(uuid.uuid4())})
    before = disk_bytes(service.root), disk_bytes(destination)
    with pytest.raises(DraftConflict):
        await download(installed)
    assert (disk_bytes(service.root), disk_bytes(destination)) == before


@pytest.mark.parametrize("change", ["receipt", "selection", "archive", "source", "owner"])
async def test_download_rechecks_custody_after_awaited_validation(installed, monkeypatch, change):
    service, directory, destination, initial = installed
    original_validation = deployment.await_validation
    changed = False
    newer = None
    async def concurrent_change(function, *args, **kwargs):
        nonlocal changed, newer
        value = await original_validation(function, *args, **kwargs)
        if not changed:
            changed = True
            if change == "receipt":
                update_receipt(directory, build_id=str(uuid.uuid4()))
            elif change == "selection":
                (directory.parent / "latest.json").write_text(json.dumps({"deployment_id": str(uuid.uuid4())}))
            elif change == "archive":
                (directory / "package.zip").write_bytes(b"new archive after validation\n")
            elif change == "owner":
                marker = destination / deployment.OWNER_FILE
                owner = json.loads(marker.read_text())
                owner["deployment_id"] = str(uuid.uuid4())
                marker.write_text(json.dumps(owner))
            else:
                (destination / "app.py").write_bytes(b"new source after validation\n")
            newer = disk_bytes(service.root), disk_bytes(destination)
        return value
    monkeypatch.setattr(deployment, "await_validation", concurrent_change)
    with pytest.raises((DraftConflict, LookupError)):
        await download(installed)
    assert changed and (disk_bytes(service.root), disk_bytes(destination)) == newer
    assert not service.tasks and not service.processes


@pytest.fixture
def api():
    calls = []
    class RetainedPackage:
        async def installed_package(self, *args):
            calls.append(args)
            return b"exact-retained-zip-fixture"
    source = Path(__file__).resolve().parents[1] / "launchkit/server.py"
    tree = ast.parse(source.read_text())
    route = "/api/guide/{identity}/deployment/installed-package"
    body = [node for node in tree.body if isinstance(node, ast.AsyncFunctionDef) and
            (node.name == "guide_result" or any(isinstance(item, ast.Call) and item.args and
             isinstance(item.args[0], ast.Constant) and item.args[0].value == route for item in node.decorator_list))]
    assert len(body) == 2
    app = FastAPI()
    namespace = {"app": app, "deployment": RetainedPackage(), "Query": Query, "Response": Response,
                 "JSONResponse": JSONResponse, "HTTPException": HTTPException, "DraftConflict": DraftConflict,
                 "ArchitectUnavailable": type("Unavailable", (RuntimeError,), {}),
                 "InstalledFeedbackNotAdmitted": type("FeedbackNotAdmitted", (RuntimeError,), {}),
                 "InstalledSuccessNotAdmitted": type("SuccessNotAdmitted", (RuntimeError,), {}),
                 "LOG": logging.getLogger(__name__)}
    exec(compile(ast.Module(body=body, type_ignores=[]), str(source), "exec"), namespace)
    return TestClient(app), calls


def test_installed_download_api_forwards_exact_query_and_uses_fixed_zip_attachment(api):
    client, calls = api
    query = {"deployment_id": str(uuid.uuid4()), "build_id": str(uuid.uuid4()),
             "source_digest": "a" * 64, "package_sha256": "b" * 64}
    response = client.get("/api/guide/owned/deployment/installed-package", params=query)
    assert response.status_code == 200 and response.content == b"exact-retained-zip-fixture"
    assert response.headers["content-type"] == "application/zip"
    assert response.headers["content-disposition"] == 'attachment; filename="installed-application.zip"'
    assert calls == [("owned", query["deployment_id"], query["build_id"], query["source_digest"], query["package_sha256"])]


@pytest.mark.parametrize("missing", ["deployment_id", "build_id", "source_digest", "package_sha256"])
def test_installed_download_api_requires_all_four_fields_without_guide_version(api, missing):
    client, calls = api
    query = {"deployment_id": str(uuid.uuid4()), "build_id": str(uuid.uuid4()),
             "source_digest": "a" * 64, "package_sha256": "b" * 64}
    query.pop(missing)
    response = client.get("/api/guide/owned/deployment/installed-package", params=query)
    assert response.status_code == 422 and calls == []
    assert "guide_version" not in response.text
