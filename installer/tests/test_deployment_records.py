"""Original receipt lookups use real private files and no live service calls."""
import ast
import base64
import hashlib
import io
import json
import logging
from pathlib import Path
from types import SimpleNamespace
import uuid
import zipfile

from fastapi import FastAPI, HTTPException
from fastapi.responses import JSONResponse
from fastapi.testclient import TestClient
import pytest

from launchkit.deployment import DeploymentService, package_files
from launchkit.deployment_records import original_deployment_record
from launchkit.gcp_provisioning import GcpProvisioningService
from launchkit.projects import DraftConflict, fingerprint
from launchkit.remote_deployment import RemoteDeploymentService, source_archive

KINDS = ("deployment", "remote-deployment", "gcp-deployment")


def write(path, value):
    path.parent.mkdir(parents=True, mode=0o700, exist_ok=True)
    path.write_bytes(value if isinstance(value, bytes) else json.dumps(value).encode())
    path.chmod(0o600)


def snapshot(root):
    return {str(p.relative_to(root)): (p.read_bytes(), p.stat().st_mtime_ns, p.stat().st_mode)
            for p in root.rglob("*") if p.is_file()}


def saved(tmp_path, kind):
    guide_id, record_id = str(uuid.uuid4()), str(uuid.uuid4())
    cls = {"deployment": DeploymentService, "remote-deployment": RemoteDeploymentService,
           "gcp-deployment": GcpProvisioningService}[kind]
    # Exercise the real record methods without constructing provider owners.
    service = object.__new__(cls)
    service.root = tmp_path / "owned-workspace" / kind
    key = "operation_id" if kind == "gcp-deployment" else "deployment_id"
    directory = (service.root / "gcp-provisioning" / guide_id if kind == "gcp-deployment"
                 else service.root / guide_id / record_id)
    path = directory / (record_id + ".json" if kind == "gcp-deployment" else "receipt.json")
    receipt = {"guide_id": guide_id, key: record_id, "status": "running",
               "destination": "/original/application", "private_settings": "private-canary",
               "console_url": "private-console-canary", "observation": {"available": True}}
    if kind == "remote-deployment":
        payload = io.BytesIO()
        with zipfile.ZipFile(payload, "w") as archive:
            for name, body in {"deployment.json": "{}", "app.py": "# fixture", "install.sh": "# fixture",
                               "implementation-report.json": '{"guide_version":4}'}.items():
                archive.writestr(name, body)
        package, controller = payload.getvalue(), b"# original controller"
        receipt.update(guide_version=4, endpoint={"host": "original.example"},
            package_sha256=hashlib.sha256(package).hexdigest(),
            controller_sha256=hashlib.sha256(controller).hexdigest(),
            transferred_source_digest=source_archive(package_files(package))[1])
        write(directory / "package.zip", package)
        write(directory / "controller.py", controller)
    elif kind == "gcp-deployment":
        account, project = "owner@example.com", "owned-cloud-project"
        request = {"guide_id": guide_id, "version": 4, "build_id": "original-build",
                   "account": account, "project": project}
        access = {"user": "mobkit", "public_key": "ssh-ed25519 " + base64.b64encode(
            b"\x00\x00\x00\x0bssh-ed25519\x00\x00\x00\x20" + b"x" * 32).decode()}
        script = "# fixture\n"
        receipt.update(protocol=1, status="blocked", phase="checking", request=request,
            request_fingerprint=fingerprint(request), account=account,
            account_fingerprint=fingerprint({"account": account, "project": project}),
            ownership_token=uuid.UUID(record_id).hex,
            binding={"guide_id": guide_id, "guide_version": 4, "build_id": "original-build",
                     "blueprint_hash": "a" * 64, "source_digest": "b" * 64, "package_sha256": "c" * 64},
            destination={"project": project, "zone": "europe-west1-b", "instance": "original",
                         "machine_type": "e2-standard-2", "disk_gb": 30, "console_port": 8787},
            access=access, bootstrap_script=script, bootstrap_sha256=hashlib.sha256(script.encode()).hexdigest(),
            public_metadata_sha256=fingerprint({"enable-guest-attributes": "TRUE",
                "ssh-keys": access["user"] + ":" + access["public_key"], "startup-script": script}),
            resources={}, pending=None)
    write(path, receipt)
    return SimpleNamespace(service=service, guide_id=guide_id, record_id=record_id,
                           path=path, receipt=receipt, key=key)


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", KINDS)
async def test_original_survives_newer_selection_and_lookup_changes_no_files(tmp_path, kind):
    item = saved(tmp_path, kind)
    newer_id = str(uuid.uuid4())
    newer = {**item.receipt, item.key: newer_id, "status": "stopped"}
    if kind == "gcp-deployment":
        newer.update(status="blocked", ownership_token=uuid.UUID(newer_id).hex)
        newer_path = item.path.with_name(newer_id + ".json")
        latest = item.path.parent / "latest.json"
    else:
        newer_path = item.path.parent.parent / newer_id / "receipt.json"
        latest = item.path.parent.parent / "latest.json"
    write(newer_path, newer)
    write(latest, {item.key: newer_id})
    before = snapshot(tmp_path)
    result = await original_deployment_record(kind, item.service, item.guide_id, item.record_id)
    assert result["record_id"] == item.record_id
    assert result["recorded_status"] == item.receipt["status"]
    assert result["observation"] == {"available": False, "reason": "retained_record_only"}
    assert "private-canary" not in json.dumps(result) and "private-console-canary" not in json.dumps(result)
    assert "conversation" not in result and "status" not in result
    assert snapshot(tmp_path) == before
    # A missing or unreadable latest pointer also cannot redirect exact lookup.
    write(latest, b"not json")
    unchanged = snapshot(tmp_path)
    assert await original_deployment_record(kind, item.service, item.guide_id, item.record_id) == result
    assert snapshot(tmp_path) == unchanged
    assert before[str(item.path.relative_to(tmp_path))] == unchanged[str(item.path.relative_to(tmp_path))]


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", KINDS)
async def test_other_workspace_or_guide_never_reads_original(tmp_path, kind):
    item = saved(tmp_path, kind)
    foreign = object.__new__(type(item.service))
    foreign.root = tmp_path / "other-workspace"
    before = snapshot(tmp_path)
    for service, guide in ((foreign, item.guide_id), (item.service, str(uuid.uuid4()))):
        with pytest.raises(LookupError, match="does not prove removal"):
            await original_deployment_record(kind, service, guide, item.record_id)
    assert not foreign.root.exists()
    assert snapshot(tmp_path) == before
    forged = {**item.receipt, "guide_id": str(uuid.uuid4())}
    write(item.path, forged)
    with pytest.raises(DraftConflict):
        await original_deployment_record(kind, item.service, item.guide_id, item.record_id)


@pytest.mark.asyncio
async def test_missing_remote_package_is_failure_not_missing_receipt(tmp_path):
    item = saved(tmp_path, "remote-deployment")
    (item.path.parent / "package.zip").unlink()
    with pytest.raises(FileNotFoundError):
        await original_deployment_record("remote-deployment", item.service, item.guide_id, item.record_id)


@pytest.mark.asyncio
async def test_explicit_recorded_cloud_removal_is_preserved_but_missing_is_unknown(tmp_path):
    item = saved(tmp_path, "gcp-deployment")
    item.receipt.update(status="removed", removal={"request_id": str(uuid.uuid4()),
        "acknowledge_data_loss": True, "resources": {key: {"status": "absent"} for key in ("vm", "disk", "firewall")}})
    write(item.path, item.receipt)
    result = await original_deployment_record("gcp-deployment", item.service, item.guide_id, item.record_id)
    assert result["recorded_status"] == "removed" and not result["observation"]["available"]
    item.path.unlink()
    with pytest.raises(LookupError, match="does not prove removal"):
        await original_deployment_record("gcp-deployment", item.service, item.guide_id, item.record_id)


def client_for(item):
    source = Path(__file__).resolve().parents[1] / "launchkit/server.py"
    selected = {"guide_result", "original_local_deployment_record", "original_remote_deployment_record",
                "original_gcp_deployment_record"}
    body = [node for node in ast.parse(source.read_text()).body
            if isinstance(node, ast.AsyncFunctionDef) and node.name in selected]
    assert len(body) == len(selected)
    app = FastAPI()
    namespace = {"app": app, "HTTPException": HTTPException, "JSONResponse": JSONResponse,
        "DraftConflict": DraftConflict, "LOG": logging.getLogger(__name__),
        "original_deployment_record": original_deployment_record,
        **{name: item.service for name in ("deployment", "remote_deployment", "gcp_provisioning")},
        **{name: type(name, (RuntimeError,), {}) for name in ("InstalledFeedbackNotAdmitted",
            "InstalledSuccessNotAdmitted", "BuildSelectionUnavailable", "RemoteProductActionNotSent", "ArchitectUnavailable")}}
    exec(compile(ast.Module(body=body, type_ignores=[]), str(source), "exec"), namespace)
    return TestClient(app)


@pytest.mark.parametrize("kind", KINDS)
def test_routes_require_exact_id_and_separate_missing_from_failed_read(tmp_path, monkeypatch, kind):
    item = saved(tmp_path, kind)
    client = client_for(item)
    url = f"/api/guide/{item.guide_id}/{kind}/record"
    params = {item.key: item.record_id}
    assert client.get(url).status_code == 422
    assert client.post(url, params=params).status_code == 405
    assert client.get(url, params={item.key: "../private"}).status_code == 422
    assert client.get(url, params=params).json()["record_id"] == item.record_id
    missing = client.get(url, params={item.key: str(uuid.uuid4())})
    assert missing.status_code == 404 and "does not prove removal" in missing.json()["detail"]
    def unreadable(*args):
        raise PermissionError("private-path-and-credential-canary")
    monkeypatch.setattr("launchkit.deployment_records.private_file", unreadable)
    failed = client.get(url, params=params)
    assert failed.status_code == 502 and "unknown" in failed.json()["detail"]
    assert "canary" not in failed.text


@pytest.mark.asyncio
@pytest.mark.parametrize("failure", ("corrupt", "symlink", "oversized"))
async def test_invalid_local_record_is_not_missing_or_authoritative(tmp_path, failure):
    item = saved(tmp_path, "deployment")
    if failure == "corrupt":
        write(item.path, b"not json")
    elif failure == "oversized":
        write(item.path, b" " * (1024 * 1024 + 1))
    else:
        destination = item.path.with_name("other.json")
        item.path.rename(destination)
        item.path.symlink_to(destination)
    with pytest.raises(DraftConflict):
        await original_deployment_record("deployment", item.service, item.guide_id, item.record_id)
