"""Offline cloud handoff through real remote admission, never a VM or SSH."""
import asyncio
import base64
from copy import deepcopy
import hashlib
import io
import json
import uuid
import zipfile

import pytest

from launchkit.artifact_binding import proposal_hash
from launchkit.deployment import DeploymentService
from launchkit.gcp_transport import validate_gcp_endpoint
from launchkit.projects import DraftConflict
from launchkit.remote_deployment import RemoteDeploymentService, read_receipt, binding, update_receipt
from launchkit.remote_destination import _archive
from test_remote_deployment import Guide, Implementation, Transport, longest_loop_gap, slow_package_hashing
from test_remote_browser_access import Handle


class CloudPackage(Implementation):
    async def package(self, identity, build_id, *, state=None):
        payload, slug = await super().package(identity, build_id, state=state)
        with zipfile.ZipFile(io.BytesIO(payload)) as archive:
            files = {item.filename: archive.read(item) for item in archive.infolist()}
        definition = json.loads(files["deployment.json"])
        definition.update(schemaVersion=2, target="gcp")
        files["deployment.json"] = json.dumps(definition).encode()
        files.update({"install_gateway.py": b"# offline fixture\n", "requirements.txt": b"# offline fixture\n"})
        output = io.BytesIO()
        with zipfile.ZipFile(output, "w") as archive:
            for name, content in files.items():
                archive.writestr(zipfile.ZipInfo(name), content)
        return output.getvalue(), slug


class CloudTransport(Transport):
    def __init__(self, endpoint):
        super().__init__()
        self.endpoint = deepcopy(endpoint)
        self.verifications, self.forwards, self.handles = [], [], []
        self.verify_reply = None

    async def verify(self, endpoint):
        validate_gcp_endpoint(endpoint)
        self.verifications.append(deepcopy(endpoint))
        if endpoint != self.endpoint:
            raise DraftConflict("The original owned VM/account/resource receipt does not match.")
        return deepcopy(self.verify_reply or endpoint)

    async def request(self, endpoint, *args, **kwargs):
        await self.verify(endpoint)
        return await super().request(endpoint, *args, **kwargs)

    async def open(self, endpoint, **kwargs):
        await self.verify(endpoint)
        self.forwards.append((deepcopy(endpoint), deepcopy(kwargs)))
        handle = Handle()
        self.handles.append(handle)
        return handle


@pytest.fixture
def cloud_service(tmp_path):
    guide = Guide()
    guide.state["proposal"]["target"] = "gcp"
    guide.state["proposal"]["project"]["slug"] = "host-test-team"
    guide.state["proposal"]["gcp"].update(project="fixture-project", zone="europe-west1-b")
    implementation = CloudPackage(guide)
    local = DeploymentService(tmp_path, guide, implementation)
    operation = str(uuid.uuid4())
    endpoint = {"transport": "gcp-iap", "operation_id": operation, "account": "fixture@example.test",
        "project": "fixture-project", "zone": "europe-west1-b", "instance": "host-test-team",
        "instance_id": "1234567890123456789", "ownership_token": uuid.UUID(operation).hex,
        "host": "compute.1234567890123456789", "user": "mobkit", "ssh_port": 22}
    cloud = CloudTransport(endpoint)
    return RemoteDeploymentService(tmp_path, guide, implementation, local, transport=Transport(), cloud_transport=cloud)


async def ready_cloud(service):
    selected = await service.accepted(service.guide.identity, 4, service.implementation.identity, target="gcp")
    return {"protocol": 1, "guide_id": service.guide.identity, "operation_id": service.cloud_transport.endpoint["operation_id"],
        "status": "vm-ready", "vm_verified": True, "endpoint": deepcopy(service.cloud_transport.endpoint),
        "binding": {"guide_id": service.guide.identity, "guide_version": 4, "build_id": service.implementation.identity,
            "blueprint_hash": proposal_hash(service.guide.state["proposal"]), "source_digest": service.implementation.digest,
            "package_sha256": hashlib.sha256(selected["payload"]).hexdigest()},
        "destination": {"project": "fixture-project", "zone": "europe-west1-b", "instance": "host-test-team"},
        "resources": {"vm": {"id": "1234567890123456789"}, "disk": {"id": "223"}, "firewall": {"id": "224"}}}


async def connect_cloud(service):
    cloud = await ready_cloud(service)
    receipt = await service.connect_cloud(service.guide.identity, 4, service.implementation.identity, cloud)
    await asyncio.gather(*list(service.tasks.values()))
    return await service.status(service.guide.identity), cloud


async def install_cloud(service, receipt):
    return await service.install(service.guide.identity, 4, service.implementation.identity, receipt["deployment_id"],
        {"OPENAI_API_KEY": "private-cloud-fixture-value"})


async def test_cloud_handoff_keeps_exact_accepted_gcp_zip_source_and_target(cloud_service):
    service = cloud_service
    original, _ = await service.implementation.package(service.guide.identity, service.implementation.identity)
    receipt, cloud = await connect_cloud(service)
    directory = service.directory(service.guide.identity)
    assert (directory / "package.zip").read_bytes() == original
    assert receipt["package_sha256"] == hashlib.sha256(original).hexdigest()
    assert receipt["target"] == "gcp" and receipt["cloud_operation_id"] == cloud["operation_id"]
    assert receipt["endpoint"] == cloud["endpoint"]
    assert receipt["can_install"] and receipt["source_binding_current"]
    await install_cloud(service, receipt)
    sent = service.cloud_transport.calls[-1]["payload"]
    _, files, definition = _archive(sent["archive"], binding(read_receipt(directory)))
    assert definition["target"] == "gcp"
    with zipfile.ZipFile(io.BytesIO(original)) as archive:
        assert files["deployment.json"] == archive.read("deployment.json")
        assert files["app.py"] == archive.read("app.py")
    assert not service.transport.calls
    assert sent["settings"]["OPENAI_API_KEY"] == "private-cloud-fixture-value"
    assert all(b"private-cloud-fixture-value" not in path.read_bytes() for path in directory.iterdir() if path.is_file())


async def test_old_linux_public_connect_cannot_connect_a_gcp_package(cloud_service):
    service = cloud_service
    with pytest.raises(DraftConflict):
        await service.connect(service.guide.identity, 4, service.implementation.identity,
            "owned.example", "test", 22, "~/src/team")
    assert not service.transport.calls and not service.cloud_transport.calls and not service.root.exists()


@pytest.mark.parametrize("change", ["guide", "build", "source", "package", "project", "zone", "instance", "account", "numeric-id", "token", "ready"])
async def test_cloud_handoff_rejects_foreign_or_unready_binding_before_storing_private_destination(cloud_service, change):
    service = cloud_service
    cloud = await ready_cloud(service)
    if change == "guide": cloud["guide_id"] = str(uuid.uuid4())
    elif change == "ready": cloud["vm_verified"] = False
    elif change in {"build", "source", "package"}:
        name = {"build": "build_id", "source": "source_digest", "package": "package_sha256"}[change]
        cloud["binding"][name] = str(uuid.uuid4()) if change == "build" else "b" * 64
    else:
        name = {"numeric-id": "instance_id", "token": "ownership_token"}.get(change, change)
        cloud["endpoint"][name] = ("other-project" if change == "project" else "other") if change not in {"numeric-id", "token", "account"} else (
            "987654" if change == "numeric-id" else "f" * 32 if change == "token" else "other@example.test")
        if change == "numeric-id":
            cloud["endpoint"]["host"] = "compute." + cloud["endpoint"]["instance_id"]
    with pytest.raises((DraftConflict, ValueError)):
        await service.connect_cloud(service.guide.identity, 4, service.implementation.identity, cloud)
    assert not service.root.exists() and not service.cloud_transport.calls and not service.transport.calls


async def test_cloud_operation_cannot_be_spliced_onto_another_exact_endpoint(cloud_service):
    service = cloud_service
    cloud = await ready_cloud(service)
    cloud["operation_id"] = str(uuid.uuid4())
    with pytest.raises(DraftConflict):
        await service.connect_cloud(service.guide.identity, 4, service.implementation.identity, cloud)
    assert not service.root.exists() and not service.cloud_transport.calls


async def test_cloud_verify_return_must_confirm_the_exact_requested_endpoint(cloud_service):
    service = cloud_service
    service.cloud_transport.verify_reply = {**service.cloud_transport.endpoint, "instance_id": "987654"}
    with pytest.raises(DraftConflict):
        await service.connect_cloud(service.guide.identity, 4, service.implementation.identity, await ready_cloud(service))
    assert not service.root.exists() and not service.cloud_transport.calls


async def test_install_and_retry_keep_receipt_gcp_target_and_exact_source(cloud_service):
    service = cloud_service
    receipt, _ = await connect_cloud(service)
    await install_cloud(service, receipt)
    directory = service.directory(service.guide.identity)
    original = (directory / "package.zip").read_bytes()
    prior = read_receipt(directory)["install_operation_id"]
    service.cloud_transport.remote = {"status": "failed", "admitted": True, "operation_id": prior,
        "worker_live": False, "cleanup_confirmed": True, "source_current": True,
        "service_observed": True, "service_owned": False, "service": {"status": "not-installed"},
        "retryable": True, "retry_of_operation_id": prior, "failure_phase": "dependencies"}
    failed = await service.status(service.guide.identity, force=True)
    assert failed["can_retry_install"] and failed["target"] == "gcp"
    retried = await service.retry_install(service.guide.identity, 4, service.implementation.identity, receipt["deployment_id"],
        prior, {"OPENAI_API_KEY": "private-cloud-retry-value"})
    sent = service.cloud_transport.calls[-1]["payload"]
    assert sent["action"] == "retry-install" and sent["previous_operation_id"] == prior
    assert "archive" not in sent and retried["target"] == "gcp"
    assert (directory / "package.zip").read_bytes() == original and not service.transport.calls


async def running_cloud_console(service):
    receipt, cloud = await connect_cloud(service)
    await install_cloud(service, receipt)
    stored = read_receipt(service.directory(service.guide.identity))
    service.cloud_transport.remote = {"status": "running", "admitted": True, "operation_id": stored["install_operation_id"],
        "worker_live": False, "source_current": True, "ready": True,
        "service_owned": True, "service_observed": True, "service": {
            "service": "mobkit-launch." + str(uuid.uuid4()) + ".service", "source_digest": stored["transferred_source_digest"],
            "console_url": "http://127.0.0.1:18088/console", "status": "running", "ready": True,
            "source_current": True, "definition_verified": True, "scope": "native-bootstrap-and-console"}}
    receipt = await service.status(service.guide.identity, force=True)
    async def checked(port, authentication):
        assert (port, authentication) == (18088, "loopback-only")
        return True
    service.browser_access.browser_check = checked
    service.browser_access.monitor_interval = 999
    return receipt, cloud


async def test_cloud_browser_forward_uses_cloud_transport_and_owned_exact_service(cloud_service):
    service = cloud_service
    receipt, _ = await running_cloud_console(service)
    try:
        result = await service.browser_access.open(receipt["guide_id"], 4, receipt["build_id"], receipt["deployment_id"])
        assert result["status"] == "ready" and result["native_ready"]
        assert service.cloud_transport.forwards == [(receipt["endpoint"], {"expected_fingerprint": receipt["host_key_fingerprint"], "port": 18088})]
        assert not service.transport.calls
    finally:
        await service.shutdown()
    assert service.cloud_transport.handles[0].closes == 1


def original_cloud_receipt(cloud):
    return {**deepcopy(cloud), "account": cloud["endpoint"]["account"],
        "ownership_token": cloud["endpoint"]["ownership_token"]}


@pytest.mark.parametrize("state", ["removing", "removal-required", "removed"])
async def test_cloud_removal_preserves_history_projects_lifecycle_and_skips_all_transport(cloud_service, state):
    service = cloud_service
    receipt, cloud = await connect_cloud(service)
    await install_cloud(service, receipt)
    directory = service.directory(service.guide.identity)
    original = (directory / "receipt.json").read_bytes()
    stored = read_receipt(directory)
    # A remote worker may remain live or unreachable. Whole-VM deletion ends it;
    # coordination must neither require nor invent its graceful termination.
    assert stored["worker_live"] is True and stored["status"] == "installing"
    service.guide.state["version"] += 1
    async def lifecycle(endpoint):
        assert endpoint == cloud["endpoint"]
        return state
    service.cloud_removal_status = lifecycle
    calls = len(service.cloud_transport.calls)
    verifications = len(service.cloud_transport.verifications)
    assert await service.prepare_cloud_removal(original_cloud_receipt(cloud)) is True
    async def forbidden_probe(*args):
        raise AssertionError("A removal lifecycle cannot probe the destination")
    service._probe = forbidden_probe
    result = await service.status(service.guide.identity, force=True)
    assert result["cloud_status"] == state and result["status"] == stored["status"]
    assert result["worker_live"] is True and result["service"] == stored["service"]
    assert not result["ready"] and result["observation"] == {"available": False}
    assert not any(result[name] for name in ("can_install", "can_retry_install", "can_restart", "can_stop"))
    assert result["fields"] == [] and (directory / "receipt.json").read_bytes() == original
    assert len(service.cloud_transport.calls) == calls and len(service.cloud_transport.verifications) == verifications
    assert "private-cloud-fixture-value" not in json.dumps(result)


@pytest.mark.parametrize("action", ["connect", "install", "retry", "restart", "stop", "rpc"])
async def test_removal_intent_blocks_each_remote_cloud_admission_before_transport(cloud_service, action):
    service = cloud_service
    receipt, cloud = await connect_cloud(service)
    async def lifecycle(endpoint):
        assert endpoint == cloud["endpoint"]
        return "removal-required"
    service.cloud_removal_status = lifecycle
    calls = len(service.cloud_transport.calls)
    verifications = len(service.cloud_transport.verifications)
    with pytest.raises(DraftConflict, match="removal has been requested"):
        if action == "connect":
            await service.connect_cloud(service.guide.identity, 4, service.implementation.identity, cloud)
        elif action == "install":
            await install_cloud(service, receipt)
        elif action == "retry":
            await service.retry_install(service.guide.identity, 4, service.implementation.identity,
                receipt["deployment_id"], str(uuid.uuid4()), {"OPENAI_API_KEY": "private-cloud-retry-value"})
        elif action == "rpc":
            directory = service.directory(service.guide.identity)
            await service.rpc(directory, read_receipt(directory), "status")
        else:
            await service.action(service.guide.identity, receipt["deployment_id"], action)
    assert len(service.cloud_transport.calls) == calls and len(service.cloud_transport.verifications) == verifications


@pytest.mark.parametrize("change", ["missing", "account", "project", "zone", "instance", "instance_id", "ownership_token", "user", "operation", "target"])
async def test_removal_coordination_rejects_spliced_or_missing_saved_remote_endpoint(cloud_service, change):
    service = cloud_service
    receipt, cloud = await connect_cloud(service)
    directory = service.directory(service.guide.identity)
    selected = deepcopy(receipt["endpoint"])
    if change == "missing": selected = None
    elif change == "operation": selected["operation_id"] = str(uuid.uuid4())
    elif change != "target": selected[change] = "PRIVATE_ENDPOINT_CANARY"
    update_receipt(directory, endpoint=selected, **({"target": "linux"} if change == "target" else {}))
    calls = len(service.cloud_transport.calls)
    with pytest.raises(DraftConflict) as failure:
        await service.prepare_cloud_removal(original_cloud_receipt(cloud))
    assert "PRIVATE_ENDPOINT_CANARY" not in str(failure.value)
    assert len(service.cloud_transport.calls) == calls


async def test_partial_known_cloud_vm_needs_no_ready_endpoint_or_remote_installation(cloud_service):
    service = cloud_service
    cloud = original_cloud_receipt(await ready_cloud(service))
    cloud.pop("endpoint")
    assert cloud["resources"]["vm"]
    assert await service.prepare_cloud_removal(cloud) is True
    assert not service.root.exists() and not service.cloud_transport.calls


async def test_missing_linked_remote_inventory_cannot_claim_clean_local_custody(cloud_service, tmp_path):
    service = cloud_service
    cloud = original_cloud_receipt(await ready_cloud(service))
    service.root.mkdir()
    (service.root / service.guide.identity).symlink_to(tmp_path / "missing-inventory", target_is_directory=True)
    with pytest.raises(DraftConflict, match="inventory contains a link"):
        await service.prepare_cloud_removal(cloud)
    assert not service.cloud_transport.calls


async def test_missing_cloud_endpoint_cannot_skip_an_existing_matching_installation(cloud_service):
    service = cloud_service
    _, cloud = await connect_cloud(service)
    original = original_cloud_receipt(cloud)
    original.pop("endpoint")
    with pytest.raises(DraftConflict, match="different cloud removal destination"):
        await service.prepare_cloud_removal(original)


async def test_removal_waits_for_local_install_admission_then_preserves_destination_worker(cloud_service):
    service = cloud_service
    receipt, cloud = await connect_cloud(service)
    entered, release = asyncio.Event(), asyncio.Event()
    async def hold(payload):
        if payload["action"] == "install":
            entered.set()
            await release.wait()
    service.cloud_transport.hook = hold
    installing = asyncio.create_task(install_cloud(service, receipt))
    await asyncio.wait_for(entered.wait(), 30)
    async def lifecycle(endpoint):
        return "removing"
    service.cloud_removal_status = lifecycle
    removing = asyncio.create_task(service.prepare_cloud_removal(original_cloud_receipt(cloud)))
    await asyncio.sleep(0)
    assert not removing.done()
    release.set()
    result = await asyncio.wait_for(installing, 2)
    assert result["worker_live"] is True
    original = (service.directory(service.guide.identity) / "receipt.json").read_bytes()
    assert await asyncio.wait_for(removing, 2) is True
    assert (service.directory(service.guide.identity) / "receipt.json").read_bytes() == original
    assert [call["payload"]["action"] for call in service.cloud_transport.calls].count("install") == 1
    assert not any(call["payload"]["action"] == "stop" for call in service.cloud_transport.calls)


async def test_removal_during_install_preflight_cannot_admit_or_send_private_application_setup(cloud_service):
    service = cloud_service
    receipt, cloud = await connect_cloud(service)
    entered, release = asyncio.Event(), asyncio.Event()
    async def hold(payload):
        if payload["action"] == "preflight":
            entered.set()
            await release.wait()
    service.cloud_transport.hook = hold
    installing = asyncio.create_task(install_cloud(service, receipt))
    await asyncio.wait_for(entered.wait(), 30)
    async def lifecycle(endpoint):
        return "removing"
    service.cloud_removal_status = lifecycle
    removing = asyncio.create_task(service.prepare_cloud_removal(original_cloud_receipt(cloud)))
    release.set()
    with pytest.raises(DraftConflict, match="removal has been requested"):
        await asyncio.wait_for(installing, 2)
    assert await asyncio.wait_for(removing, 2) is True
    stored = read_receipt(service.directory(service.guide.identity))
    assert stored["remote_admission_requested"] is False and not stored.get("pending_operation")
    assert not any(call["payload"]["action"] == "install" for call in service.cloud_transport.calls)
    assert not any("settings" in call["payload"] for call in service.cloud_transport.calls)


async def test_unreachable_remote_worker_does_not_strand_explicit_whole_vm_removal(cloud_service):
    service = cloud_service
    receipt, cloud = await connect_cloud(service)
    await install_cloud(service, receipt)
    service.cloud_transport.fail = "status"
    unavailable = await service.status(service.guide.identity, force=True)
    assert unavailable["status"] == "observation-unavailable" and unavailable["worker_live"] is True
    original = (service.directory(service.guide.identity) / "receipt.json").read_bytes()
    calls = len(service.cloud_transport.calls)
    async def lifecycle(endpoint):
        return "removing"
    service.cloud_removal_status = lifecycle
    assert await service.prepare_cloud_removal(original_cloud_receipt(cloud)) is True
    assert len(service.cloud_transport.calls) == calls
    assert (service.directory(service.guide.identity) / "receipt.json").read_bytes() == original


async def test_removal_scans_original_installation_even_when_it_is_not_latest(cloud_service):
    from launchkit.remote_deployment import write_private
    service = cloud_service
    receipt, cloud = await connect_cloud(service)
    directory = service.directory(service.guide.identity)
    write_private(directory.parent / "latest.json", {"deployment_id": str(uuid.uuid4())})
    update_receipt(directory, endpoint=None)
    with pytest.raises(DraftConflict, match="no verified original endpoint"):
        await service.prepare_cloud_removal(original_cloud_receipt(cloud))


async def test_removal_unconfirmed_local_tunnel_cleanup_blocks_then_reuses_exact_handle(cloud_service):
    service = cloud_service
    receipt, cloud = await running_cloud_console(service)
    browser = service.browser_access
    try:
        opened = await browser.open(receipt["guide_id"], 4, receipt["build_id"], receipt["deployment_id"])
        handle = service.cloud_transport.handles[0]
        original = (service.directory(service.guide.identity) / "receipt.json").read_bytes()
        async def lifecycle(endpoint):
            return "removal-required"
        service.cloud_removal_status = lifecycle
        handle.cleanup = False
        with pytest.raises(DraftConflict, match="has not confirmed cleanup"):
            await service.prepare_cloud_removal(original_cloud_receipt(cloud))
        closed = browser.public(browser.key(receipt["guide_id"], receipt["deployment_id"]))
        assert closed["status"] == "cleanup-needed" and closed["local_url"] is None
        assert closed["access_id"] == opened["access_id"] and handle.live
        with pytest.raises(DraftConflict):
            await browser.open(receipt["guide_id"], 4, receipt["build_id"], receipt["deployment_id"])
        assert len(service.cloud_transport.forwards) == 1
        handle.cleanup = True
        assert await service.prepare_cloud_removal(original_cloud_receipt(cloud)) is True
        assert not handle.live and handle.closes >= 2
        assert (service.directory(service.guide.identity) / "receipt.json").read_bytes() == original
    finally:
        for handle in service.cloud_transport.handles:
            handle.cleanup = True
        await service.shutdown()


async def test_removal_waits_for_inflight_local_tunnel_and_withdraws_before_ready_publication(cloud_service):
    service = cloud_service
    receipt, cloud = await running_cloud_console(service)
    browser = service.browser_access
    entered, release = asyncio.Event(), asyncio.Event()
    async def held_check(port, authentication):
        entered.set()
        await release.wait()
        return True
    browser.browser_check = held_check
    opening = asyncio.create_task(browser.open(receipt["guide_id"], 4, receipt["build_id"], receipt["deployment_id"]))
    await asyncio.wait_for(entered.wait(), 30)
    async def lifecycle(endpoint):
        return "removing"
    service.cloud_removal_status = lifecycle
    removing = asyncio.create_task(service.prepare_cloud_removal(original_cloud_receipt(cloud)))
    await asyncio.sleep(0)
    assert not removing.done() and service.cloud_transport.handles[0].live
    release.set()
    try:
        result = await asyncio.wait_for(opening, 2)
        assert result["local_url"] is None and not result["native_ready"]
        assert await asyncio.wait_for(removing, 2) is True
        assert not service.cloud_transport.handles[0].live
        assert not any(call["payload"]["action"] == "stop" for call in service.cloud_transport.calls)
    finally:
        release.set()
        await service.shutdown()


@pytest.mark.parametrize("state", ["removing", "removal-required", "removed"])
async def test_removal_intent_blocks_new_tunnel_without_destination_probe(cloud_service, state):
    service = cloud_service
    receipt, _ = await running_cloud_console(service)
    calls = len(service.cloud_transport.calls)
    async def lifecycle(endpoint):
        return state
    service.cloud_removal_status = lifecycle
    async def forbidden_probe(*args):
        raise AssertionError("A removal lifecycle cannot probe the destination")
    service._probe = forbidden_probe
    with pytest.raises(DraftConflict):
        await service.browser_access.open(receipt["guide_id"], 4, receipt["build_id"], receipt["deployment_id"])
    assert not service.cloud_transport.forwards and len(service.cloud_transport.calls) == calls


async def test_cloud_connect_hashes_the_package_off_the_event_loop(cloud_service, monkeypatch):
    service = cloud_service
    cloud = await ready_cloud(service)
    slow_package_hashing(monkeypatch)
    receipt, gap = await longest_loop_gap(service.connect_cloud(service.guide.identity, 4,
        service.implementation.identity, cloud))
    await asyncio.gather(*list(service.tasks.values()))
    assert receipt["target"] == "gcp" and receipt["package_sha256"] == cloud["binding"]["package_sha256"]
    assert gap < 0.5
