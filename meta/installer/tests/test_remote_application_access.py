"""Declared application access, shared forwarding and source custody tests."""
import asyncio
from copy import deepcopy
import hashlib
import io
import json
import subprocess
import sys
import uuid
import zipfile

import pytest
from fastapi.testclient import TestClient

from launchkit.remote_browser_access import application_responds
from launchkit.remote_deployment import read_receipt
from test_remote_deployment import service, connect, install
from test_remote_browser_access import Forward
from launchkit.projects import DraftConflict

ENTRIES = [
    {"id": "staff", "label": "Staff workspace", "path": "/staff", "port": 18089},
    {"id": "admin", "label": "Admin settings", "path": "/admin", "port": 18089},
    {"id": "reports", "label": "Reports", "path": "/reports", "port": 18090},
    {"id": "native-console", "label": "App home", "path": "/", "port": 18088},
]


def test_frozen_bundled_controller_boots_isolated_below_linux_single_argument_limit():
    from launchkit.remote_interface_bundle import controller_bytes
    from launchkit.remote_deployment import controller_program
    content = controller_bytes()
    program = controller_program(content)
    assert len(program.encode()) < 64 * 1024
    result = subprocess.run([sys.executable, "-I", "-c", program], input=b"{}",
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=5, check=True)
    assert not result.stderr
    assert json.loads(result.stdout.decode().partition(" ")[2])["ready"] is False


@pytest.fixture
async def app_access(service):
    original = service.implementation.package
    metadata = json.dumps({"entries": ENTRIES}).encode()
    async def package(*args, **kwargs):
        payload, slug = await original(*args, **kwargs)
        result = io.BytesIO()
        with zipfile.ZipFile(io.BytesIO(payload)) as source, zipfile.ZipFile(result, "w") as target:
            for name in source.namelist():
                target.writestr(source.getinfo(name), source.read(name))
            target.writestr(zipfile.ZipInfo("application/experience.json"), metadata)
        return result.getvalue(), slug
    service.implementation.package = package
    receipt = await connect(service)
    await install(service, receipt)
    saved = read_receipt(service.directory(service.guide.identity))
    service.transport.remote = {"status": "running", "admitted": True,
        "operation_id": saved["install_operation_id"], "worker_live": False, "source_current": True,
        "ready": True, "service_owned": True, "service_observed": True,
        "service": {"service": "mobkit-launch." + str(uuid.uuid4()) + ".service",
            "source_digest": saved["transferred_source_digest"], "console_url": "http://127.0.0.1:18088/console",
            "status": "running", "ready": True, "source_current": True, "definition_verified": True,
            "scope": "native-bootstrap-and-console"},
        "application_experiences": {"protocol": 1, "supported": True, "status": "resolved",
            "source_digest": saved["accepted_source_digest"], "installed_source_digest": saved["transferred_source_digest"],
            "metadata_sha256": hashlib.sha256(metadata).hexdigest(), "entries": [
                {**{key: entry[key] for key in ("id", "label", "path")}, "destination_port": entry["port"],
                 "listener_owned": True, "listener_token": "d" * 64, "reason": None, "verified": False}
                for entry in ENTRIES]}}
    receipt = await service.status(service.guide.identity, force=True)
    browser = service.browser_access
    browser.transport = Forward()
    browser.monitor_interval = 999
    browser.application_checks = []
    async def custom_check(port, path):
        browser.application_checks.append((port, path))
        return 303
    async def native_check(*_):
        return True
    browser.application_check, browser.browser_check = custom_check, native_check
    yield browser, receipt
    await service.shutdown()


async def open_app(browser, receipt, identity="staff"):
    return await browser.open(receipt["guide_id"], receipt["guide_version"], receipt["build_id"],
        receipt["deployment_id"], "application:" + identity)


async def test_declared_custom_entry_opens_exact_port_path_without_claiming_signin_or_workflow(app_access):
    browser, receipt = app_access
    result = await open_app(browser, receipt)
    assert result["status"] == "ready" and result["native_ready"] is True
    assert result["local_url"] == "http://127.0.0.1:18089/staff"
    assert result["console_url"] == "http://127.0.0.1:18088/console"
    assert result["transport_reachable"] is True and result["http_status"] == 303
    assert all(result[key] is False for key in ("application_verified", "authentication_verified", "workflow_verified"))
    assert result["metadata_sha256"] == receipt["application_experiences"]["metadata_sha256"]
    assert browser.transport.calls[0][1]["port"] == 18089
    assert browser.application_checks == [(18089, "/staff")]
    again = await open_app(browser, receipt)
    assert again["access_id"] == result["access_id"] and len(browser.transport.calls) == 1


async def test_custom_reload_only_observes_without_ssh_or_forward(app_access):
    browser, receipt = app_access
    before = len(browser.remote.transport.calls)
    result = await browser.status(receipt["guide_id"], receipt["deployment_id"], "application:staff")
    assert result["status"] == "closed" and result["local_url"] is None
    assert result["metadata_sha256"] == receipt["application_experiences"]["metadata_sha256"]
    assert len(browser.remote.transport.calls) == before and not browser.transport.calls


async def test_same_port_entries_share_one_handle_and_close_withdraws_every_alias_before_wait(app_access):
    browser, receipt = app_access
    staff = await open_app(browser, receipt)
    admin = await open_app(browser, receipt, "admin")
    assert staff["access_id"] != admin["access_id"] and len(browser.transport.calls) == 1
    handle = browser.transport.handles[0]
    original = handle.close
    async def closing():
        for entry in ("staff", "admin"):
            public = browser.public(browser.key(receipt["guide_id"], receipt["deployment_id"], "application:" + entry))
            assert public["local_url"] is None and public["status"] == "closed"
        return await original()
    handle.close = closing
    result = await browser.close(receipt["guide_id"], receipt["deployment_id"], admin["access_id"])
    assert result["entry_id"] == "application:admin" and result["cleanup_confirmed"] is True
    assert handle.closes == 1
    assert (await browser.status(receipt["guide_id"], receipt["deployment_id"], "application:staff"))["status"] == "closed"


async def test_native_and_custom_same_port_share_handle_without_identifier_collision(app_access):
    browser, receipt = app_access
    native = await browser.open(receipt["guide_id"], receipt["guide_version"], receipt["build_id"], receipt["deployment_id"])
    custom = await open_app(browser, receipt, "native-console")
    assert native["entry_id"] == "native-console" and custom["entry_id"] == "application:native-console"
    assert len(browser.transport.calls) == 1 and custom["local_url"] == "http://127.0.0.1:18088/"
    await browser.close(receipt["guide_id"], receipt["deployment_id"], custom["access_id"])
    assert browser.public(browser.key(receipt["guide_id"], receipt["deployment_id"]))["local_url"] is None


async def test_different_ports_remain_independent_until_service_stop(app_access):
    browser, receipt = app_access
    staff = await open_app(browser, receipt)
    reports = await open_app(browser, receipt, "reports")
    assert len(browser.transport.calls) == 2
    await browser.close(receipt["guide_id"], receipt["deployment_id"], staff["access_id"])
    assert browser.transport.handles[1].live is True
    assert (await browser.status(receipt["guide_id"], receipt["deployment_id"], "application:reports"))["access_id"] == reports["access_id"]
    await browser.remote.action(receipt["guide_id"], receipt["deployment_id"], "stop")
    assert all(not handle.live for handle in browser.transport.handles)


async def test_uncertain_shared_cleanup_blocks_a_new_alias_and_preserves_exact_close(app_access):
    browser, receipt = app_access
    staff = await open_app(browser, receipt)
    handle = browser.transport.handles[0]
    handle.cleanup = False
    await browser.close(receipt["guide_id"], receipt["deployment_id"], staff["access_id"])
    admin = await open_app(browser, receipt, "admin")
    assert admin["status"] == "cleanup-needed" and admin["local_url"] is None
    assert admin["cleanup_confirmed"] is False and len(browser.transport.calls) == 1
    assert browser.records[browser.key(receipt["guide_id"], receipt["deployment_id"], "application:admin")]["handle"] is handle
    browser.remote.implementation.accepted = False
    handle.cleanup = True
    result = await browser.close(receipt["guide_id"], receipt["deployment_id"], admin["access_id"])
    assert result["cleanup_confirmed"] is True and not handle.live


@pytest.mark.parametrize("key,value", [("path", "/foreign"), ("destination_port", 19000),
    ("listener_owned", False), ("listener_token", None), ("verified", True), ("label", "private-canary")])
async def test_destination_cannot_change_source_declarations_or_claim_unowned_listener(app_access, key, value):
    browser, receipt = app_access
    browser.remote.transport.remote["application_experiences"]["entries"][0][key] = value
    with pytest.raises(DraftConflict):
        await open_app(browser, receipt)
    assert not browser.transport.calls
    saved = read_receipt(browser.remote.directory(receipt["guide_id"]))
    assert "private-canary" not in json.dumps(saved)


async def test_old_frozen_controller_preserves_native_access_without_custom_capability(app_access):
    browser, receipt = app_access
    browser.remote.transport.remote.pop("application_experiences")
    with pytest.raises(DraftConflict):
        await open_app(browser, receipt)
    native = await browser.open(receipt["guide_id"], receipt["guide_version"], receipt["build_id"], receipt["deployment_id"])
    assert native["status"] == "ready" and len(browser.transport.calls) == 1


async def test_listener_change_during_http_check_withdraws_source_bound_link(app_access):
    browser, receipt = app_access
    async def changing(*_):
        browser.remote.transport.remote["application_experiences"]["entries"][0]["listener_token"] = "e" * 64
        return 200
    browser.application_check = changing
    result = await open_app(browser, receipt)
    assert result["status"] == "unavailable" and result["local_url"] is None
    assert not browser.transport.handles[0].live


@pytest.mark.parametrize("trigger", ["status", "open", "reconcile", "stop"])
async def test_global_drift_withdraws_every_port_before_first_cleanup_await(app_access, trigger):
    browser, receipt = app_access
    await open_app(browser, receipt)
    await open_app(browser, receipt, "reports")
    handle = browser.transport.handles[0]
    original = handle.close
    entered, proceed = asyncio.Event(), asyncio.Event()
    async def pending_close():
        entered.set()
        await proceed.wait()
        return await original()
    handle.close = pending_close
    if trigger != "stop":
        browser.remote.implementation.accepted = False
    if trigger == "status":
        task = asyncio.create_task(browser.status(receipt["guide_id"], receipt["deployment_id"], "application:staff"))
    elif trigger == "open":
        task = asyncio.create_task(open_app(browser, receipt))
    elif trigger == "stop":
        task = asyncio.create_task(browser.remote.action(receipt["guide_id"], receipt["deployment_id"], "stop"))
    else:
        task = asyncio.create_task(browser.reconcile(receipt, False))
    await asyncio.wait_for(entered.wait(), 30)
    try:
        for entry in ("staff", "reports"):
            value = browser.public(browser.key(receipt["guide_id"], receipt["deployment_id"], "application:" + entry))
            assert value["local_url"] is None and value["native_ready"] is False
    finally:
        proceed.set()
        try:
            await asyncio.wait_for(task, 2)
        except DraftConflict:
            assert trigger == "open"
    assert all(not owned.live for owned in browser.transport.handles)


async def test_custom_cancellation_after_start_cleans_exact_owned_handle(app_access):
    browser, receipt = app_access
    entered = asyncio.Event()
    async def pending(*_):
        entered.set()
        await asyncio.Event().wait()
    browser.application_check = pending
    task = asyncio.create_task(open_app(browser, receipt))
    await asyncio.wait_for(entered.wait(), 30)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert not browser.transport.handles[0].live
    assert browser.public(browser.key(receipt["guide_id"], receipt["deployment_id"], "application:staff"))["local_url"] is None


@pytest.mark.parametrize("fault", ["selected", "frozen", "context", "scope", "existing-http"])
async def test_open_authority_faults_close_all_ports(app_access, monkeypatch, fault):
    browser, receipt = app_access
    await open_app(browser, receipt)
    await open_app(browser, receipt, "reports")
    if fault == "selected":
        original = browser.remote.directory
        monkeypatch.setattr(browser.remote, "directory", lambda guide, deployment=None:
            original(guide, deployment) if deployment else original(guide) / "other-selected")
    elif fault == "frozen":
        (browser.remote.directory(receipt["guide_id"]) / "controller.py").write_bytes(b"changed frozen controller")
    else:
        async def broken(*args, **kwargs):
            raise ValueError("private-canary")
        monkeypatch.setattr(browser, {"context": "_context", "scope": "_scope", "existing-http": "application_check"}[fault], broken)
    try:
        value = await open_app(browser, receipt)
        assert value["local_url"] is None and "private-canary" not in json.dumps(value)
    except DraftConflict as error:
        assert "private-canary" not in str(error)
    assert all(not handle.live for handle in browser.transport.handles)
    assert all(record["public"]["local_url"] is None for record in browser.records.values())


async def test_wrong_caller_version_is_side_effect_free_for_valid_open_ports(app_access):
    browser, receipt = app_access
    await open_app(browser, receipt)
    await open_app(browser, receipt, "reports")
    with pytest.raises(DraftConflict):
        await browser.open(receipt["guide_id"], receipt["guide_version"] - 1, receipt["build_id"],
            receipt["deployment_id"], "application:staff")
    assert all(handle.live for handle in browser.transport.handles)
    assert all(record["public"]["local_url"] for record in browser.records.values())


@pytest.mark.parametrize("fault", ["missing", "metadata"])
async def test_lost_application_observation_withdraws_all_custom_ports_before_cleanup(app_access, fault):
    browser, receipt = app_access
    await open_app(browser, receipt)
    await open_app(browser, receipt, "reports")
    native = await browser.open(receipt["guide_id"], receipt["guide_version"], receipt["build_id"], receipt["deployment_id"])
    original = browser.transport.handles[0].close
    entered, proceed = asyncio.Event(), asyncio.Event()
    async def pending():
        entered.set()
        await proceed.wait()
        return await original()
    browser.transport.handles[0].close = pending
    if fault == "missing":
        browser.remote.transport.remote.pop("application_experiences")
    else:
        browser.remote.transport.remote["application_experiences"]["metadata_sha256"] = "e" * 64
    browser.remote.last_observed[receipt["deployment_id"]] = -100
    task = asyncio.create_task(browser.status(receipt["guide_id"], receipt["deployment_id"], "application:staff"))
    await asyncio.wait_for(entered.wait(), 30)
    try:
        for entry in ("staff", "reports"):
            assert browser.public(browser.key(receipt["guide_id"], receipt["deployment_id"], "application:" + entry))["local_url"] is None
        assert browser.public(browser.key(receipt["guide_id"], receipt["deployment_id"]))["local_url"] == native["local_url"]
    finally:
        proceed.set()
        await asyncio.wait_for(task, 2)
    assert browser.transport.handles[2].live


async def test_closed_entry_observation_of_corrupt_frozen_source_revokes_other_ports(app_access):
    browser, receipt = app_access
    staff = await open_app(browser, receipt)
    await open_app(browser, receipt, "reports")
    await browser.close(receipt["guide_id"], receipt["deployment_id"], staff["access_id"])
    (browser.remote.directory(receipt["guide_id"]) / "controller.py").write_bytes(b"changed frozen source")
    with pytest.raises(DraftConflict):
        await browser.status(receipt["guide_id"], receipt["deployment_id"], "application:staff")
    assert all(not handle.live for handle in browser.transport.handles)


@pytest.mark.parametrize("response,expected", [
    (b"HTTP/1.1 303 See Other\r\nLocation: http://other.invalid/login\r\nSet-Cookie: private-canary\r\n\r\n", 303),
    (b"HTTP/1.1 401 Unauthorized\r\n\r\n", 401),
    (b"HTTP/1.1 404 Missing\r\n\r\n", 404),
    (b"HTTP/1.1 999 Invalid\r\n\r\n", None),
    (b"HTTP/1.1 200 OK\r\nX-Large: " + b"a" * 9000 + b"\r\n\r\n", None)])
async def test_custom_http_probe_is_bounded_and_retains_only_numeric_status(response, expected):
    requests = []
    async def handler(reader, writer):
        requests.append(await reader.readuntil(b"\r\n\r\n"))
        writer.write(response)
        await writer.drain()
        writer.close()
        await writer.wait_closed()
    server = await asyncio.start_server(handler, "127.0.0.1", 0)
    try:
        port = server.sockets[0].getsockname()[1]
        assert await application_responds(port, "/staff") == expected
        assert requests == [(f"GET /staff HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nAccept: text/html\r\n\r\n").encode()]
    finally:
        server.close()
        await server.wait_closed()


async def test_api_admits_declared_identifier_only_and_keeps_caller_routing_forbidden(app_access, monkeypatch):
    from launchkit import server
    browser, receipt = app_access
    monkeypatch.setattr(server, "remote_deployment", browser.remote)
    client = TestClient(server.app)
    base = f"/api/guide/{receipt['guide_id']}/remote-deployment/browser-access"
    body = {"version": receipt["guide_version"], "build_id": receipt["build_id"],
        "deployment_id": receipt["deployment_id"], "entry_id": "application:staff"}
    result = client.get(base, params={"deployment_id": receipt["deployment_id"], "entry_id": "application:staff"})
    assert result.status_code == 200 and result.json()["status"] == "closed"
    for extra in ({"path": "/private-canary"}, {"port": 19000}, {"host": "private-canary"}):
        result = client.post(base + "/open", json={**body, **extra})
        assert result.status_code == 422 and "private-canary" not in result.text
    assert not browser.transport.calls
    result = client.post(base + "/open", json=body)
    assert result.status_code == 200 and result.json()["local_url"] == "http://127.0.0.1:18089/staff"


async def test_transcript_only_revision_preserves_declared_ui_access_and_original_listener_custody(app_access):
    browser, receipt = app_access
    before = len(browser.remote.transport.calls)
    closed = await browser.status(receipt["guide_id"], receipt["deployment_id"], "application:staff")
    assert closed["status"] == "closed" and closed["current_guide_version"] == 4
    assert closed["local_url"] is None and not browser.transport.calls
    assert len(browser.remote.transport.calls) == before
    browser.remote.guide.state["version"] = 5
    closed = await browser.status(receipt["guide_id"], receipt["deployment_id"], "application:staff")
    assert closed["status"] == "closed" and closed["current_guide_version"] == 5
    assert closed["guide_version"] == 4 and closed["local_url"] is None
    ready = await browser.open(receipt["guide_id"], 5, receipt["build_id"], receipt["deployment_id"], "application:staff")
    assert ready["status"] == "ready" and ready["current_guide_version"] == 5
    assert ready["guide_version"] == 4 and ready["local_url"] == "http://127.0.0.1:18089/staff"
    key = browser.key(receipt["guide_id"], receipt["deployment_id"], "application:staff")
    binding = deepcopy(browser.records[key]["binding"])
    browser.remote.guide.state["version"] = 6
    observed = await browser.status(receipt["guide_id"], receipt["deployment_id"], "application:staff")
    assert observed["status"] == "ready" and observed["current_guide_version"] == 6
    assert observed["access_id"] == ready["access_id"] and observed["local_url"] == ready["local_url"]
    assert browser.records[key]["binding"] == binding and binding["guide_version"] == 4
    assert len(browser.transport.calls) == 1 and browser.transport.handles[0].closes == 0
    browser.remote.guide.state["brief"] = {"purpose": "Changed application interface requirements"}
    changed = await browser.status(receipt["guide_id"], receipt["deployment_id"], "application:staff")
    assert changed["status"] == "unavailable" and changed["local_url"] is None
    assert browser.transport.handles[0].closes == 1 and len(browser.transport.calls) == 1
