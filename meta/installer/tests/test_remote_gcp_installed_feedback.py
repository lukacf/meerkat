"""Offline frozen GCP first-use and feedback, without cloud, SSH or app execution."""
import asyncio
from copy import deepcopy
import hashlib
import io
import json
import time
import uuid
from unittest.mock import AsyncMock
import zipfile

import pytest

from launchkit.deployment import DeploymentService
from launchkit.projects import DraftConflict
from launchkit.remote_deployment import RemoteDeploymentService, read_receipt, update_receipt
from launchkit.remote_installed_feedback import validate_remote_binding, remote_use_binding
from test_remote_deployment import Guide, Transport
from test_remote_gcp_handoff import CloudPackage, CloudTransport, connect_cloud
from test_remote_installed_feedback import archive_bytes, record_bytes, feedback_body, reports, rewrite_package


TASK = {"label": "Read a saved plan", "request": "Read my saved plan without changing it."}
PRIVATE = "OFFLINE_GCP_PRIVATE_FIXTURE_CANARY"


class CheckedCloudPackage(CloudPackage):
    async def status(self, identity, *, state=None):
        return {**await super().status(identity, state=state), "source_verified": True}

    async def package(self, identity, build_id, *, state=None):
        payload, slug = await super().package(identity, build_id, state=state)
        with zipfile.ZipFile(io.BytesIO(payload)) as archive:
            files = {name: archive.read(name) for name in archive.namelist()}
        definition = json.loads(files["deployment.json"])
        definition.update(name="Cloud Field Notes", description="Keep field observations and prepare useful plans.",
            entrypoint="lead:main", agents=[{"identity": "lead:main", "name": "Field Guide"}])
        files["deployment.json"] = json.dumps(definition).encode()
        files["application/first-use.json"] = json.dumps({"tasks": [TASK]}).encode()
        files["application/trial.json"] = json.dumps({"fixtures": {"private": PRIVATE}}).encode()
        files["application/unused.py"] = b"raise AssertionError('Do not execute generated code in offline metadata checks')\n"
        return archive_bytes(files), slug


async def prepared_cloud(tmp_path):
    guide = Guide()
    guide.state["proposal"]["target"] = "gcp"
    guide.state["proposal"]["project"]["slug"] = "host-test-team"
    guide.state["proposal"]["gcp"].update(project="fixture-project", zone="europe-west1-b")
    implementation = CheckedCloudPackage(guide)
    local = DeploymentService(tmp_path, guide, implementation)
    operation = str(uuid.uuid4())
    endpoint = {"transport": "gcp-iap", "operation_id": operation, "account": "fixture@example.test",
        "project": "fixture-project", "zone": "europe-west1-b", "instance": "host-test-team",
        "instance_id": "1234567890123456789", "ownership_token": uuid.UUID(operation).hex,
        "host": "compute.1234567890123456789", "user": "mobkit", "ssh_port": 22}
    service = RemoteDeploymentService(tmp_path, guide, implementation, local,
        transport=Transport(), cloud_transport=CloudTransport(endpoint))
    await connect_cloud(service)
    directory = service.directory(guide.identity)
    # This is an explicitly offline host receipt fixture, not an installed VM.
    update_receipt(directory, status="stopped", admitted=True, remote_admission_requested=True,
        worker_live=False, source_current=True, ready=False, cleanup_confirmed=True,
        pending_operation=None, observation={"available": True}, phase="stop")
    service.last_observed[directory.name] = time.monotonic()
    implementation.submit = AsyncMock(return_value={"build_id": "new-candidate", "status": "queued"})
    return service, directory


async def test_gcp_frozen_first_use_keeps_exact_endpoint_package_and_target_without_remote_requests(tmp_path):
    service, directory = await prepared_cloud(tmp_path)
    receipt = read_receipt(directory)
    before, calls = record_bytes(service), deepcopy(service.cloud_transport.calls)
    metadata = await service.use(service.guide.identity, directory.name)
    assert metadata["scope"] == "frozen-remote-package-source"
    assert metadata["first_use"]["tasks"] == [TASK]
    assert metadata["entrypoint"] == {"identity": "lead:main", "name": "Field Guide"}
    assert metadata["remote_binding"]["endpoint"] == service.cloud_transport.endpoint
    assert metadata["remote_binding"] == remote_use_binding(receipt)
    assert metadata["package_sha256"] == hashlib.sha256((directory / "package.zip").read_bytes()).hexdigest()
    assert metadata["installed_source_verified"] is True and metadata["native_runtime_available"] is False
    assert metadata["application_outcomes_verified"] is False
    assert PRIVATE not in json.dumps(metadata)
    assert record_bytes(service) == before and service.cloud_transport.calls == calls
    assert not service.transport.calls
    with zipfile.ZipFile(directory / "package.zip") as archive:
        assert json.loads(archive.read("deployment.json"))["target"] == "gcp"


async def test_gcp_human_success_recovery_keeps_full_scope_and_does_not_claim_verified_domain_outcome(tmp_path):
    service, directory = await prepared_cloud(tmp_path)
    receipt = read_receipt(directory)
    package = (directory / "package.zip").read_bytes()
    calls = deepcopy(service.cloud_transport.calls)
    request_id = str(uuid.uuid4())
    kwargs = {"request_id": request_id, "task": TASK["request"]}
    args = (service.guide.identity, 4, directory.name, receipt["build_id"], receipt["accepted_source_digest"], receipt["package_sha256"])
    first = await service.record_success(*args, **kwargs)
    second = await service.record_success(*args, **kwargs)
    assert first == second
    use = await service.use(service.guide.identity, directory.name)
    assert use["success_report"] == first and use["success_report"]["request_id"] == request_id
    assert len(list((directory / "human-success-reports").glob("*.json"))) == 2
    assert use["application_outcomes_verified"] is False
    saved = json.loads((directory / "human-success-reports" / (request_id + ".json")).read_text())
    assert saved["binding"]["remote_binding"]["endpoint"] == service.cloud_transport.endpoint
    assert saved["binding"]["target"] == "remote"
    assert (directory / "package.zip").read_bytes() == package
    assert service.cloud_transport.calls == calls and not service.transport.calls


async def test_gcp_explicit_repair_admits_only_current_candidate_and_preserves_frozen_cloud_source(tmp_path):
    service, directory = await prepared_cloud(tmp_path)
    body = await feedback_body(service)
    before, calls = record_bytes(service), deepcopy(service.cloud_transport.calls)
    result = await service.feedback(service.guide.identity, **body)
    assert result == {"build_id": "new-candidate", "status": "queued"}
    args, kwargs = service.implementation.submit.await_args
    assert args[:2] == (service.guide.identity, 4)
    assert kwargs == {"expected_previous_build_id": body["continuation_build_id"],
        "expected_source_digest": body["continuation_source_digest"]}
    human, remote = reports(args[2])
    assert human["reported_task"] == body["task"] and human["reported_outcome"] == "partly"
    assert remote["remote_binding"]["endpoint"] == service.cloud_transport.endpoint
    assert remote["application_outcomes_verified"] is False and remote["destination_unchanged"] is True
    assert "do not contact or modify this destination" in args[2] and PRIVATE not in args[2]
    assert service.guide.state["proposal"]["target"] == "gcp"
    assert record_bytes(service) == before and service.cloud_transport.calls == calls


@pytest.mark.parametrize("field", ["account", "project", "zone", "instance", "instance_id", "ownership_token", "operation_id", "host"])
async def test_foreign_cloud_binding_cannot_admit_repair_or_substitute_destination(tmp_path, field):
    service, directory = await prepared_cloud(tmp_path)
    body = await feedback_body(service)
    endpoint = body["remote_binding"]["endpoint"]
    if field == "account": endpoint[field] = "other@example.test"
    elif field == "ownership_token": endpoint[field] = "f" * 32
    elif field == "operation_id": endpoint[field] = str(uuid.uuid4())
    elif field == "instance_id":
        endpoint[field] = "987654321"
        endpoint["host"] = "compute.987654321"
    elif field == "host": endpoint[field] = "compute.987654321"
    else: endpoint[field] = "other-destination"
    before, calls = record_bytes(service), deepcopy(service.cloud_transport.calls)
    with pytest.raises((ValueError, DraftConflict)):
        await service.feedback(service.guide.identity, **body)
    service.implementation.submit.assert_not_awaited()
    assert record_bytes(service) == before and service.cloud_transport.calls == calls


@pytest.mark.parametrize("change", ["cloud-operation", "receipt-linux", "package-linux", "bad-endpoint", "missing-operation"])
async def test_current_receipt_target_operation_and_frozen_package_must_join_before_use(tmp_path, change):
    service, directory = await prepared_cloud(tmp_path)
    if change == "cloud-operation": update_receipt(directory, cloud_operation_id=str(uuid.uuid4()))
    if change == "receipt-linux": update_receipt(directory, target="linux")
    if change == "missing-operation": update_receipt(directory, cloud_operation_id=None)
    if change == "bad-endpoint": update_receipt(directory, endpoint={"host": "owned.example", "user": "test", "ssh_port": 22})
    if change == "package-linux":
        def mutate(files):
            definition = json.loads(files["deployment.json"])
            definition["target"] = "linux"
            files["deployment.json"] = json.dumps(definition).encode()
        rewrite_package(directory, mutate)
    before, calls = record_bytes(service), deepcopy(service.cloud_transport.calls)
    with pytest.raises(DraftConflict): await service.use(service.guide.identity, directory.name)
    assert record_bytes(service) == before and service.cloud_transport.calls == calls


def test_legacy_linux_endpoint_binding_remains_exact_and_cannot_accept_new_cloud_fields():
    from launchkit.remote_installed_feedback import remote_use_binding
    endpoint = {"host": "owned.example", "user": "test", "ssh_port": 22}
    receipt = {"guide_id": str(uuid.uuid4()), "guide_version": 4, "build_id": str(uuid.uuid4()),
        "deployment_id": str(uuid.uuid4()), "accepted_source_digest": "a" * 64, "package_sha256": "b" * 64,
        "transferred_source_digest": "c" * 64, "controller_sha256": "d" * 64,
        "endpoint": endpoint, "host": {"uid": 1000, "home": "/home/test", "destination": "/home/test/src/team"},
        "destination": "/home/test/src/team", "host_key_fingerprint": "SHA256:" + "A" * 43}
    assert validate_remote_binding(remote_use_binding(receipt))["endpoint"] == endpoint
    with pytest.raises((ValueError, DraftConflict)):
        remote_use_binding({**receipt, "endpoint": {**endpoint, "transport": "gcp-iap"}})
