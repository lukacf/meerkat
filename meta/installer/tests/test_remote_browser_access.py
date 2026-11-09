"""Browser-access authority and lifecycle tests, without a real SSH destination."""
import asyncio
from copy import deepcopy
import uuid

from fastapi.testclient import TestClient
import pytest

from launchkit.projects import DraftConflict
from launchkit.remote_browser_access import browser_responds, console_port
from launchkit.remote_deployment import update_receipt
from test_remote_deployment import service, connect, install, longest_loop_gap, slow_package_reads


class Handle:
    def __init__(self):
        self.live, self.cleanup, self.closes = True, True, 0

    async def observe(self):
        return self.live

    async def close(self):
        self.closes += 1
        if self.cleanup:
            self.live = False
        return self.cleanup


class Forward:
    def __init__(self):
        self.calls, self.handles, self.error = [], [], None

    async def open(self, endpoint, **kwargs):
        self.calls.append((deepcopy(endpoint), deepcopy(kwargs)))
        if self.error:
            raise self.error
        handle = Handle()
        self.handles.append(handle)
        return handle


@pytest.fixture
async def access(service):
    receipt = await connect(service)
    await install(service, receipt)
    directory = service.directory(service.guide.identity)
    receipt = update_receipt(directory)
    service.transport.remote = {"status": "running", "admitted": True,
        "operation_id": receipt["install_operation_id"], "worker_live": False, "source_current": True,
        "ready": True, "service_owned": True, "service_observed": True, "service": {
            "service": "mobkit-launch." + str(uuid.uuid4()) + ".service", "source_digest": receipt["transferred_source_digest"],
            "console_url": "http://127.0.0.1:18088/console", "status": "running", "ready": True,
            "source_current": True, "definition_verified": True, "scope": "native-bootstrap-and-console"}}
    receipt = await service.status(service.guide.identity, force=True)
    browser = service.browser_access
    browser.transport = Forward()
    browser.monitor_interval = 999
    browser.checks = []
    async def check(port, authentication):
        browser.checks.append((port, authentication))
        return True
    browser.browser_check = check
    yield browser, receipt
    await service.shutdown()


async def opened(browser, receipt):
    return await browser.open(receipt["guide_id"], receipt["guide_version"], receipt["build_id"], receipt["deployment_id"])


async def test_reload_only_observes_and_never_spawns_or_contacts_destination(access):
    browser, receipt = access
    before = len(browser.remote.transport.calls)
    value = await browser.status(receipt["guide_id"], receipt["deployment_id"])
    assert value["status"] == "closed" and value["local_url"] is None
    assert not browser.transport.calls and len(browser.remote.transport.calls) == before
    assert value["guide_version"] == receipt["guide_version"] and value["build_id"] == receipt["build_id"]
    assert value["scope"]["authentication"] == "loopback-only" and value["cleanup_confirmed"] is True


async def test_startup_unconfirmed_cleanup_retains_exact_custody_and_blocks_replacement(access):
    from launchkit.ssh_browser_access import BrowserAccessError
    browser, receipt = access
    handle = Handle()
    handle.cleanup = False
    error = BrowserAccessError("cleanup-unavailable")
    error.handle = handle
    browser.transport.error = error
    result = await opened(browser, receipt)
    assert result["status"] == "cleanup-needed" and result["cleanup_confirmed"] is False
    key = receipt["guide_id"], receipt["deployment_id"]
    assert browser.records[key]["handle"] is handle
    assert (await opened(browser, receipt))["status"] == "cleanup-needed"
    assert len(browser.transport.calls) == 1
    handle.cleanup = True
    assert (await browser.close(*key, result["access_id"]))["cleanup_confirmed"] is True


async def test_explicit_open_rejoins_exact_service_and_publishes_only_credential_free_url(access):
    browser, receipt = access
    before = len(browser.remote.transport.calls)
    value = await opened(browser, receipt)
    assert value["status"] == "ready" and value["native_ready"]
    assert value["local_url"] == receipt["service"]["console_url"]
    assert value["service_name"] == receipt["service"]["service"]
    assert value["service_source_digest"] == receipt["transferred_source_digest"]
    assert len(browser.remote.transport.calls) == before + 2
    assert browser.transport.calls == [(receipt["endpoint"], {
        "expected_fingerprint": receipt["host_key_fingerprint"], "port": 18088})]
    assert browser.checks == [(18088, "loopback-only")]
    assert value["scope"]["custom_interfaces_verified"] is False
    assert "password" not in repr(value) and "private-host-test-value" not in repr(value)
    again = await opened(browser, receipt)
    assert again["access_id"] == value["access_id"] and len(browser.transport.calls) == 1


@pytest.mark.parametrize("key,value", [("admitted", False), ("worker_live", True),
    ("service_owned", False), ("service_observed", False), ("source_current", False),
    ("ready", False), ("status", "stopped")])
async def test_unready_remote_observation_cannot_admit_forward(access, key, value):
    browser, receipt = access
    browser.remote.transport.remote[key] = value
    with pytest.raises(DraftConflict):
        await opened(browser, receipt)
    assert not browser.transport.calls


@pytest.mark.parametrize("key,value", [("ready", False), ("source_current", False),
    ("definition_verified", False), ("status", "active"), ("scope", "custom-ui"),
    ("service", "foreign.service"), ("source_digest", "b" * 64),
    ("console_url", "http://127.0.0.1:99999/console"),
    ("console_url", "http://127.0.0.1:18088/console?auth_token=canary"),
    ("console_url", "http://example.invalid:18088/console")])
async def test_unverified_or_mismatched_service_cannot_admit_forward(access, key, value):
    browser, receipt = access
    browser.remote.transport.remote["service"][key] = value
    with pytest.raises(DraftConflict):
        await opened(browser, receipt)
    assert not browser.transport.calls


async def test_acceptance_change_revokes_ready_forward_without_claiming_remote_stop(access):
    browser, receipt = access
    value = await opened(browser, receipt)
    handle = browser.transport.handles[0]
    browser.remote.implementation.accepted = False
    result = await browser.status(receipt["guide_id"], receipt["deployment_id"])
    assert result["status"] == "unavailable" and result["local_url"] is None
    assert handle.closes == 1 and "may still be running" in result["detail"]
    assert browser.remote.transport.remote["status"] == "running"
    assert result["access_id"] == value["access_id"]


async def test_lost_pinned_remote_observation_revokes_forward(access):
    browser, receipt = access
    await opened(browser, receipt)
    browser.remote.transport.fail = "status"
    browser.remote.last_observed[receipt["deployment_id"]] = -100
    result = await browser.status(receipt["guide_id"], receipt["deployment_id"])
    assert result["status"] == "unavailable" and result["local_url"] is None
    assert not browser.transport.handles[0].live


async def test_changed_owned_unit_cannot_inherit_existing_access(access):
    browser, receipt = access
    await opened(browser, receipt)
    browser.remote.transport.remote["service"]["service"] = "mobkit-launch." + str(uuid.uuid4()) + ".service"
    browser.remote.last_observed[receipt["deployment_id"]] = -100
    result = await browser.status(receipt["guide_id"], receipt["deployment_id"])
    assert result["local_url"] is None and not browser.transport.handles[0].live


async def test_remote_status_reconciliation_withdraws_access_on_source_change(access):
    browser, receipt = access
    await opened(browser, receipt)
    browser.remote.implementation.accepted = False
    await browser.remote.status(receipt["guide_id"])
    assert not browser.transport.handles[0].live
    assert browser.public((receipt["guide_id"], receipt["deployment_id"]))["local_url"] is None


@pytest.mark.parametrize("value", [None, 12, [], {}])
async def test_malformed_fresh_service_identity_revokes_existing_access(access, value):
    browser, receipt = access
    await opened(browser, receipt)
    browser.remote.transport.remote["service"]["service"] = value
    browser.remote.last_observed[receipt["deployment_id"]] = -100
    result = await browser.status(receipt["guide_id"], receipt["deployment_id"])
    assert result["local_url"] is None and not browser.transport.handles[0].live


async def test_background_monitor_revokes_source_change_without_browser_polling(access):
    browser, receipt = access
    browser.monitor_interval = .01
    await opened(browser, receipt)
    browser.remote.implementation.accepted = False
    async def wait_for_exit():
        while browser.transport.handles[0].live:
            await asyncio.sleep(.01)
    await asyncio.wait_for(wait_for_exit(), 2)
    assert browser.public((receipt["guide_id"], receipt["deployment_id"]))["local_url"] is None


async def test_exact_close_is_available_after_source_changes_and_is_idempotent(access):
    browser, receipt = access
    result = await opened(browser, receipt)
    browser.remote.implementation.accepted = False
    with pytest.raises(DraftConflict):
        await browser.close(receipt["guide_id"], receipt["deployment_id"], str(uuid.uuid4()))
    assert browser.transport.handles[0].live
    for _ in range(2):
        closed = await browser.close(receipt["guide_id"], receipt["deployment_id"], result["access_id"])
        assert closed["status"] == "closed" and closed["local_url"] is None
    assert browser.transport.handles[0].closes == 1


async def test_unconfirmed_local_cleanup_never_claims_closed_or_admits_replacement(access):
    browser, receipt = access
    result = await opened(browser, receipt)
    handle = browser.transport.handles[0]
    handle.cleanup = False
    closed = await browser.close(receipt["guide_id"], receipt["deployment_id"], result["access_id"])
    assert closed["status"] == "cleanup-needed" and closed["local_url"] is None
    observed = await browser.status(receipt["guide_id"], receipt["deployment_id"])
    assert observed["status"] == "cleanup-needed" and observed["local_url"] is None
    replacement = await opened(browser, receipt)
    assert replacement["status"] == "cleanup-needed" and len(browser.transport.calls) == 1
    handle.cleanup = True
    assert (await browser.close(receipt["guide_id"], receipt["deployment_id"], result["access_id"]))["status"] == "closed"


async def test_failed_browser_check_closes_forward_before_returning(access):
    browser, receipt = access
    async def missing(*_):
        return False
    browser.browser_check = missing
    value = await opened(browser, receipt)
    assert value["status"] == "unavailable" and value["local_url"] is None
    assert browser.transport.handles[0].closes == 1


async def test_source_change_during_http_check_cannot_publish_old_link(access):
    browser, receipt = access
    async def changing(*_):
        browser.remote.implementation.accepted = False
        return True
    browser.browser_check = changing
    result = await opened(browser, receipt)
    assert result["local_url"] is None and not browser.transport.handles[0].live


async def test_existing_access_source_drift_during_open_does_not_admit_replacement(access):
    browser, receipt = access
    await opened(browser, receipt)
    async def changing(*_):
        browser.remote.implementation.accepted = False
        return True
    browser.browser_check = changing
    result = await opened(browser, receipt)
    assert result["local_url"] is None and len(browser.transport.calls) == 1


async def test_partial_observation_cannot_inherit_previous_service_authority(access):
    browser, receipt = access
    await opened(browser, receipt)
    browser.remote.transport.remote.pop("service")
    browser.remote.last_observed[receipt["deployment_id"]] = -100
    result = await browser.status(receipt["guide_id"], receipt["deployment_id"])
    assert result["local_url"] is None and not browser.transport.handles[0].live


async def test_cancellation_after_forward_start_cleans_owned_forward(access):
    browser, receipt = access
    entered = asyncio.Event()
    async def pending(*_):
        entered.set()
        await asyncio.Event().wait()
    browser.browser_check = pending
    task = asyncio.create_task(opened(browser, receipt))
    await asyncio.wait_for(entered.wait(), 30)
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert not browser.transport.handles[0].live
    assert browser.public((receipt["guide_id"], receipt["deployment_id"]))["local_url"] is None


async def test_service_stop_closes_owned_forward_before_remote_control(access):
    browser, receipt = access
    await opened(browser, receipt)
    async def hook(payload):
        if payload["action"] == "stop":
            assert not browser.transport.handles[0].live
    browser.remote.transport.hook = hook
    result = await browser.remote.action(receipt["guide_id"], receipt["deployment_id"], "stop")
    assert result["status"] == "stopped"
    assert browser.public((receipt["guide_id"], receipt["deployment_id"]))["local_url"] is None


async def test_shutdown_closes_only_local_forward_and_joins_monitor(access):
    browser, receipt = access
    await opened(browser, receipt)
    await browser.remote.shutdown()
    assert not browser.monitors and not browser.transport.handles[0].live
    assert browser.remote.transport.remote["status"] == "running"


async def test_public_ready_link_expires_even_when_monitor_is_delayed(access):
    browser, receipt = access
    await opened(browser, receipt)
    key = receipt["guide_id"], receipt["deployment_id"]
    browser.records[key]["verified_at"] -= 16
    result = browser.public(key)
    assert result["status"] == "unavailable" and result["local_url"] is None


def test_console_port_rejects_noncanonical_or_credential_bearing_addresses():
    assert console_port("http://127.0.0.1:65535/console") == 65535
    for value in (None, "http://localhost:18088/console", "http://127.0.0.1:1023/console",
            "http://127.0.0.1:65536/console", "http://user@127.0.0.1:18088/console", "http://127.0.0.1:18088/console#token"):
        assert console_port(value) is None


@pytest.mark.parametrize("authentication,response,expected", [
    ("local-password", b"HTTP/1.1 303 See Other\r\nLocation: /login\r\n\r\n", True),
    ("local-password", b"HTTP/1.1 200 OK\r\n\r\n", False),
    ("local-password", b"HTTP/1.1 303 See Other\r\nLocation: http://foreign.invalid/login\r\n\r\n", False),
    ("local-password", b"HTTP/1.1 303 See Other\r\nLocation: /login\r\nLocation: /other\r\n\r\n", False),
    ("loopback-only", b"HTTP/1.1 200 OK\r\n\r\n", True),
    ("loopback-only", b"HTTP/1.1 502 Bad Gateway\r\n\r\n", False),
    ("loopback-only", b"HTTP/1.1 200 OK\r\nX-Large: " + b"a" * 9000 + b"\r\n\r\n", False)])
async def test_browser_check_is_bounded_and_never_follows_redirect_or_sends_credentials(authentication, response, expected):
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
        assert await browser_responds(port, authentication) is expected
        assert requests == [(f"GET /console HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\nAccept: text/html\r\n\r\n").encode()]
    finally:
        server.close()
        await server.wait_closed()


async def test_api_forbids_caller_routing_and_redacts_unknown_input(access, monkeypatch):
    from launchkit import server
    browser, receipt = access
    monkeypatch.setattr(server, "remote_deployment", browser.remote)
    client = TestClient(server.app)
    base = f"/api/guide/{receipt['guide_id']}/remote-deployment/browser-access"
    body = {"version": receipt["guide_version"], "build_id": receipt["build_id"],
        "deployment_id": receipt["deployment_id"], "entry_id": "native-console"}
    value = client.get(base, params={"deployment_id": receipt["deployment_id"]})
    assert value.status_code == 200 and value.json()["status"] == "closed"
    for extra in ({"host": "private-canary"}, {"port": 9000}, {"command": "private-canary"}):
        value = client.post(base + "/open", json={**body, **extra})
        assert value.status_code == 422 and "private-canary" not in value.text
    assert not browser.transport.calls
    value = client.post(base + "/open", json=body, headers={"Origin": "http://foreign.invalid"})
    assert value.status_code == 403 and not browser.transport.calls


async def test_transcript_only_revision_preserves_exact_native_console_connection(access):
    browser, receipt = access
    browser.remote.guide.state["version"] = 5
    ready = await browser.open(receipt["guide_id"], 5, receipt["build_id"], receipt["deployment_id"])
    assert ready["status"] == "ready" and ready["native_ready"]
    assert ready["guide_version"] == 4 and ready["current_guide_version"] == 5
    assert ready["local_url"] == receipt["service"]["console_url"]
    key = browser.key(receipt["guide_id"], receipt["deployment_id"])
    binding = deepcopy(browser.records[key]["binding"])
    handle = browser.transport.handles[0]
    browser.remote.guide.state["version"] = 6
    current = await browser.status(receipt["guide_id"], receipt["deployment_id"])
    assert current["status"] == "ready" and current["current_guide_version"] == 6
    assert current["guide_version"] == 4 and current["access_id"] == ready["access_id"]
    assert current["local_url"] == ready["local_url"]
    assert browser.records[key]["binding"] == binding and binding["guide_version"] == 4
    assert len(browser.transport.calls) == 1 and handle.closes == 0
    with pytest.raises(DraftConflict, match="guide changed"):
        await browser.open(receipt["guide_id"], 5, receipt["build_id"], receipt["deployment_id"])
    assert len(browser.transport.calls) == 1


async def test_native_console_post_cannot_publish_a_link_if_chat_changes_during_http_check(access):
    browser, receipt = access
    browser.remote.guide.state["version"] = 5
    async def changed(port, authentication):
        browser.remote.guide.state["version"] = 6
        return True
    browser.browser_check = changed
    result = await browser.open(receipt["guide_id"], 5, receipt["build_id"], receipt["deployment_id"])
    assert result["status"] == "unavailable" and result["local_url"] is None
    assert not result["native_ready"] and result["cleanup_confirmed"]
    assert len(browser.transport.handles) == 1 and browser.transport.handles[0].closes == 1
    assert browser.remote.transport.remote["status"] == "running"


@pytest.mark.parametrize("change", ["blueprint", "brief"])
async def test_changed_inputs_after_chat_revoke_original_native_console_without_replacement(access, change):
    browser, receipt = access
    ready = await opened(browser, receipt)
    browser.remote.guide.state["version"] = 5
    if change == "blueprint": browser.remote.guide.state["proposal"]["project"]["name"] += " changed"
    else: browser.remote.guide.state["brief"] = {"purpose": "Changed required outcome"}
    result = await browser.status(receipt["guide_id"], receipt["deployment_id"])
    assert result["status"] == "unavailable" and result["local_url"] is None
    assert result["access_id"] == ready["access_id"]
    assert not result["native_ready"] and result["cleanup_confirmed"]
    assert len(browser.transport.calls) == 1 and browser.transport.handles[0].closes == 1
    assert browser.remote.transport.remote["status"] == "running"


async def test_closed_console_reads_attest_current_source_before_first_and_reopened_connection(access):
    browser, receipt = access
    before = len(browser.remote.transport.calls)
    closed = await browser.status(receipt["guide_id"], receipt["deployment_id"])
    assert closed["status"] == "closed" and closed["current_guide_version"] == 4
    assert closed["local_url"] is None and not browser.transport.calls
    assert len(browser.remote.transport.calls) == before
    browser.remote.guide.state["version"] = 5
    closed = await browser.status(receipt["guide_id"], receipt["deployment_id"])
    assert closed["status"] == "closed" and closed["current_guide_version"] == 5 and closed["guide_version"] == 4
    assert len(browser.remote.transport.calls) == before
    opened_current = await browser.open(receipt["guide_id"], 5, receipt["build_id"], receipt["deployment_id"])
    await browser.close(receipt["guide_id"], receipt["deployment_id"], opened_current["access_id"])
    browser.remote.guide.state["version"] = 6
    closed = await browser.status(receipt["guide_id"], receipt["deployment_id"])
    assert closed["status"] == "closed" and closed["current_guide_version"] == 6 and closed["guide_version"] == 4
    assert closed["local_url"] is None and closed["access_id"] == opened_current["access_id"]
    reopened = await browser.open(receipt["guide_id"], 6, receipt["build_id"], receipt["deployment_id"])
    assert reopened["status"] == "ready" and reopened["current_guide_version"] == 6
    await browser.close(receipt["guide_id"], receipt["deployment_id"], reopened["access_id"])
    browser.remote.guide.state["brief"] = {"purpose": "Changed requirements"}
    stale = await browser.status(receipt["guide_id"], receipt["deployment_id"])
    assert stale["local_url"] is None and stale["current_guide_version"] is None


async def test_pending_harmless_guide_turn_keeps_readonly_access_but_blocks_new_post(access):
    browser, receipt = access
    ready = await opened(browser, receipt)
    browser.remote.guide.state.update(version=5, operation={"status": "running"})
    observed = await browser.remote.status(receipt["guide_id"])
    current = await browser.status(receipt["guide_id"], receipt["deployment_id"])
    assert observed["source_binding_current"] and observed["current_guide_version"] == 5
    assert current["status"] == "ready" and current["current_guide_version"] == 5
    assert current["access_id"] == ready["access_id"] and current["local_url"] == ready["local_url"]
    assert len(browser.transport.calls) == 1 and browser.transport.handles[0].closes == 0
    with pytest.raises(DraftConflict, match="guide changed"):
        await browser.open(receipt["guide_id"], 5, receipt["build_id"], receipt["deployment_id"])
    assert len(browser.transport.calls) == 1 and browser.transport.handles[0].closes == 0
    browser.remote.guide.state["brief"] = {"purpose": "An actual changed requirement during the turn"}
    changed = await browser.status(receipt["guide_id"], receipt["deployment_id"])
    assert changed["status"] == "unavailable" and changed["local_url"] is None
    assert browser.transport.handles[0].closes == 1


async def test_remote_browser_access_open_leaves_the_event_loop_free(access, monkeypatch):
    browser, receipt = access
    slow_package_reads(monkeypatch)
    value, gap = await longest_loop_gap(opened(browser, receipt))
    assert value["status"] == "ready" and value["local_url"] == receipt["service"]["console_url"]
    assert gap < 0.5


async def test_remote_browser_access_status_leaves_the_event_loop_free(access, monkeypatch):
    browser, receipt = access
    ready = await opened(browser, receipt)
    slow_package_reads(monkeypatch)
    value, gap = await longest_loop_gap(browser.status(receipt["guide_id"], receipt["deployment_id"]))
    assert value["status"] == "ready" and value["access_id"] == ready["access_id"]
    assert gap < 0.5


async def test_remote_browser_access_closed_scope_leaves_the_event_loop_free(access, monkeypatch):
    browser, receipt = access
    slow_package_reads(monkeypatch)
    value, gap = await longest_loop_gap(browser.status(receipt["guide_id"], receipt["deployment_id"]))
    assert value["status"] == "closed" and value["scope"]["authentication"] == "loopback-only"
    assert gap < 0.5
