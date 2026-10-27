"""Host-side deployment admission, with no real SSH host or provider access."""
import asyncio
import base64
from copy import deepcopy
import hashlib
import io
import json
import os
import tarfile
import time
import uuid
import zipfile

import pytest
from fastapi.testclient import TestClient

from launchkit.blueprint import starter_blueprint
from launchkit.deployment import DeploymentService
from launchkit.projects import DraftConflict, fingerprint
from launchkit.remote_deployment import RemoteDeploymentService, read_receipt, source_archive, update_receipt, write_private
from launchkit.ssh_transport import SshTransportError
from launchkit.templates_v2.console_configuration import CONSOLE_FIELDS


class Guide:
    def __init__(self):
        self.identity = str(uuid.uuid4())
        proposal = starter_blueprint().model_dump(mode="json")
        proposal["target"] = "linux"
        self.state = {"id": self.identity, "version": 4, "proposal": proposal}

    async def get(self, identity):
        if identity != self.identity:
            raise LookupError("Unknown setup")
        return deepcopy(self.state)


class Implementation:
    def __init__(self, guide):
        self.guide, self.identity, self.accepted = guide, str(uuid.uuid4()), True
        self.digest = "a" * 64
        self.original = deepcopy(guide.state)
        self.runtime_candidate = None
        self.application_configuration, self.extra_files = None, {}
        self.mcp_connections, self.mcp_grants = [], []

    async def status(self, identity, *, state=None):
        if state is None:
            state = await self.guide.get(identity)
        else:
            assert state["id"] == identity
        from launchkit.artifact_binding import inputs
        current = inputs(state) == inputs(self.original)
        return {"guide_id": identity, "guide_version": self.original["version"], "build_id": self.identity,
            "current": current, "current_guide_version": state["version"] if current else None,
            "ready_to_package": True, "status": "completed", "source_digest": self.digest,
            "acceptance_current": self.accepted, "acceptance_trusted": self.accepted,
            "acceptance": {"status": "pass"} if self.accepted else None}

    async def package(self, identity, build_id, *, state=None):
        assert build_id == self.identity
        if state is None:
            await self.guide.get(identity)
        else:
            assert state["id"] == identity
        definition = {"port": 18088, "target": self.original["proposal"]["target"], "slug": "host-test-team",
            "environment": [{"key": "OPENAI_API_KEY", "label": "OpenAI key", "secret": True, "required": True}],
            "agents": [{"identity": "lead:main"}], "classes": [{"tools": {"mcp": self.mcp_grants, "knowledge": False}}],
            "mcpConnections": self.mcp_connections,
            "knowledge": {"mode": "none"}}
        if self.runtime_candidate is not None:
            definition["runtimeCandidate"] = deepcopy(self.runtime_candidate)
        report = {"build_id": build_id, "guide_version": self.original["version"],
            "brief_hash": fingerprint(self.original.get("brief") or {}),
            "blueprint_hash": fingerprint(self.original["proposal"]), "verification": {"digest": self.digest}}
        output = io.BytesIO()
        with zipfile.ZipFile(output, "w") as archive:
            for name, content in {"deployment.json": json.dumps(definition), "implementation-report.json": json.dumps(report),
                "app.py": "# accepted test source\n", "install.sh": "exit 0\n", "application/__init__.py": "",
                "remote_operations.py": "# helper test source\n", "application_configuration.py": "# configuration\n",
                "mcp_connections.py": "# connectors\n", "setup.py": "# setup\n"}.items():
                archive.writestr(zipfile.ZipInfo(name), content)
            if self.application_configuration is not None:
                archive.writestr(zipfile.ZipInfo("application/configuration.json"), json.dumps(self.application_configuration))
            for name, content in self.extra_files.items():
                archive.writestr(zipfile.ZipInfo(name), content)
        return output.getvalue(), "host-test-team"


class Transport:
    def __init__(self):
        self.calls, self.hook, self.fail = [], None, None
        self.key = "SHA256:" + "A" * 43
        self.host = {"uid": 1000, "home": "/home/test", "destination": "/home/test/src/team", "platform": "linux", "python": "3.12.1"}
        self.remote = None

    async def request(self, endpoint, program, payload, timeout=30, expected_fingerprint=None):
        self.calls.append({"endpoint": deepcopy(endpoint), "payload": deepcopy(payload), "pin": expected_fingerprint})
        if self.hook:
            await self.hook(payload)
        if self.fail and payload["action"] == self.fail:
            raise SshTransportError("timeout")
        result = {"protocol": 1, "deployment_id": payload["deployment_id"], "binding": deepcopy(payload["binding"]),
                  "_transport": {"host_key_fingerprint": self.key}}
        if payload["action"] == "preflight":
            return {**result, "host": deepcopy(self.host), "ready": True,
                    "checks": [{"name": "Linux destination", "status": "pass", "detail": "Owned test destination"}]}
        if payload["action"] == "status":
            return {**result, **(self.remote or {"status": "not-installed", "admitted": False, "ready": False})}
        if payload["action"] in {"install", "retry-install"}:
            self.remote = {"status": "installing", "admitted": True, "operation_id": payload["operation_id"],
                           "worker_live": True, "source_current": True, "ready": False}
            if payload["action"] == "retry-install":
                self.remote["previous_operation_id"] = payload["previous_operation_id"]
        else:
            self.remote = {"status": "stopped" if payload["action"] == "stop" else "running", "admitted": True,
                "operation_id": payload["operation_id"], "worker_live": False, "source_current": True,
                "ready": payload["action"] != "stop", "service_owned": True, "service_observed": True,
                "service": {"unit": "owned-test.service", "scope": "native-bootstrap-and-console"}}
        return {**result, **self.remote}


@pytest.fixture
def service(tmp_path):
    guide = Guide()
    implementation = Implementation(guide)
    local = DeploymentService(tmp_path, guide, implementation)
    return RemoteDeploymentService(tmp_path, guide, implementation, local, transport=Transport())


async def connect(service):
    receipt = await service.connect(service.guide.identity, 4, service.implementation.identity,
                                    "owned.example", "test", 22, "~/src/team")
    assert receipt["fields"] == [] and not receipt["can_install"]
    await asyncio.gather(*list(service.tasks.values()))
    return await service.status(service.guide.identity)


async def install(service, receipt, **changes):
    return await service.install(service.guide.identity, 4, service.implementation.identity, receipt["deployment_id"],
                                 {"OPENAI_API_KEY": "private-host-test-value", **changes})


async def running(service, receipt):
    service.transport.remote = {"status": "running", "admitted": True,
        "operation_id": read_receipt(service.directory(service.guide.identity))["install_operation_id"],
        "worker_live": False, "source_current": True, "ready": True,
        "service_owned": True, "service_observed": True, "service": {"unit": "owned-test.service"}}
    return await service.status(service.guide.identity, force=True)


@pytest.mark.parametrize("entrypoint", ["accepted", "connect"])
async def test_selected_product_runtime_refuses_before_remote_connection_or_custody(service, entrypoint):
    # This is the compiler's Linux product marker, not qualified native input.
    service.implementation.runtime_candidate = {"source": "meta-toolkit-product",
        "selection": "runtime-candidate.json", "assembly_sha256": "c" * 64}
    state = deepcopy(service.guide.state)
    before, _ = await service.implementation.package(service.guide.identity, service.implementation.identity)
    with pytest.raises(DraftConflict, match="local native setup owner.*Remote installation is not supported"):
        if entrypoint == "accepted":
            await service.accepted(service.guide.identity, 4, service.implementation.identity)
        else:
            await service.connect(service.guide.identity, 4, service.implementation.identity,
                                  "owned.example", "test", 22, "~/src/team")
    after, _ = await service.implementation.package(service.guide.identity, service.implementation.identity)
    assert after == before and service.guide.state == state
    assert service.transport.calls == [] and service.tasks == {}
    assert not service.root.exists()


@pytest.mark.parametrize("marker", [None, {"source": "signed-candidate",
    "selection": "runtime-candidate.json", "tuple_sha256": "d" * 64}], ids=["ordinary", "legacy-admission-only"])
async def test_remote_product_refusal_preserves_existing_ordinary_and_legacy_admission(service, marker):
    # Preserves source admission only; it does not qualify private/native remote installation.
    service.implementation.runtime_candidate = deepcopy(marker)
    selected = await service.accepted(service.guide.identity, 4, service.implementation.identity)
    assert selected["spec"]["target"] == "linux"
    assert selected["spec"].get("runtimeCandidate") == marker
    assert json.loads(selected["files"]["deployment.json"][0]).get("runtimeCandidate") == marker
    assert service.transport.calls == [] and service.tasks == {} and not service.root.exists()


async def test_detached_remote_interview_observer_uses_owned_status_without_guide_or_browser_reentry(service):
    receipt = await connect(service)
    await install(service, receipt)
    service.transport.remote = {"status": "running", "admitted": True,
        "operation_id": read_receipt(service.directory(service.guide.identity))["install_operation_id"],
        "worker_live": False, "source_current": True, "ready": True,
        "service_owned": True, "service_observed": True,
        "service": {"status": "running", "ready": True, "definition_verified": True,
            "source_current": True, "source_digest": receipt["transferred_source_digest"],
            "service": "mobkit-launch.33333333-1234-1234-1234-123456789abc.service",
            "scope": "native-bootstrap-and-console", "console_url": "http://127.0.0.1:18088/console"}}
    state = deepcopy(service.guide.state)
    async def forbidden(*args, **kwargs):
        raise AssertionError("Interview evidence must not reread guide or reconcile browser access")
    service.guide.get = forbidden
    service.browser_access.reconcile = forbidden
    service.last_observed.clear()
    before = len(service.transport.calls)
    result = await service.guide_status(state["id"], state=state)
    assert result["owned_package_verified"] and result["source_binding_current"]
    assert result["observation_fresh"] and result["status"] == "running"
    assert [call["payload"]["action"] for call in service.transport.calls[before:]] == ["status"]
    assert not service.tasks
    service.tasks[result["deployment_id"]] = asyncio.current_task()
    try:
        held = await service.guide_status(state["id"], state=state)
        assert held["status"] == "running" and not held["observation_fresh"]
        assert len(service.transport.calls) == before + 1
    finally:
        service.tasks.pop(result["deployment_id"])
    with pytest.raises(ValueError, match="matching detached"):
        await service.guide_status(state["id"], state={**state, "id": "another"})
    service.last_observed.clear()
    service.transport.fail = "status"
    unknown = await service.guide_status(state["id"], state=state)
    assert unknown["owned_package_verified"] and unknown["status"] == "observation-unavailable"
    assert unknown["last_known_status"] == "running" and not unknown["observation"]["available"]


async def test_detached_remote_interview_unadmitted_connection_does_not_start_preflight(service):
    await connect(service)
    state = deepcopy(service.guide.state)
    async def forbidden(*args, **kwargs):
        raise AssertionError("Interview evidence must remain detached")
    service.guide.get = forbidden
    service.browser_access.reconcile = forbidden
    service.last_observed.clear()
    before = len(service.transport.calls)
    result = await service.guide_status(state["id"], state=state)
    assert result["status"] == "configuration-required" and result["owned_package_verified"]
    assert not result["observation_fresh"] and len(service.transport.calls) == before


async def test_detached_remote_interview_changed_or_tampered_source_is_not_current(service):
    await connect(service)
    state = deepcopy(service.guide.state)
    harmless = {**state, "version": 5}
    result = await service.guide_status(state["id"], state=harmless)
    assert result["owned_package_verified"] and result["source_binding_current"]
    assert result["guide_version"] == 4 and result["current_guide_version"] == 5
    from launchkit.deployment_evidence import project_remote_deployment_evidence
    assert project_remote_deployment_evidence(harmless, result)["current"]
    changed_plan = deepcopy(harmless)
    changed_plan["proposal"]["project"]["name"] += " changed"
    for changed in (changed_plan, {**harmless, "brief": {"facts": {"outcome": "A changed required outcome"}}}):
        result = await service.guide_status(state["id"], state=changed)
        assert result["owned_package_verified"] and not result["source_binding_current"]
        assert result["guide_version"] == 4 and result["current_guide_version"] is None
        assert not project_remote_deployment_evidence(changed, result)["current"]
    with pytest.raises(DraftConflict, match="guide changed"):
        service.guide.state["version"] = 5
        await service.accepted(state["id"], 4, service.implementation.identity)
    directory = service.directory(state["id"])
    (directory / "controller.py").write_bytes(b"tampered")
    before = len(service.transport.calls)
    with pytest.raises(DraftConflict, match="package or controller changed"):
        await service.guide_status(state["id"], state=state)
    assert len(service.transport.calls) == before


@pytest.mark.parametrize("cloud_status", ["removing", "removal-required", "removed"])
async def test_remote_interview_cloud_removal_keeps_saved_custody_without_ssh_or_browser_changes(service, cloud_status):
    receipt = await connect(service)
    await install(service, receipt)
    directory = service.directory(service.guide.identity)
    update_receipt(directory, target="gcp", status="running", last_known_status="running", ready=True)
    async def removal(endpoint): return cloud_status
    async def forbidden(*args, **kwargs): raise AssertionError("No browser or guide mutation during removal")
    service.cloud_removal_status = removal
    service.guide.get = forbidden
    service.browser_access.reconcile = forbidden
    before = len(service.transport.calls)
    result = await service.guide_status(service.guide.identity, state=deepcopy(service.guide.state))
    assert result["owned_package_verified"] and result["cloud_status"] == cloud_status
    assert not result["observation"]["available"] and not result["ready"]
    assert len(service.transport.calls) == before


async def test_remote_interview_unknown_cloud_observer_retains_verified_installation_but_no_use_claim(service):
    from launchkit.deployment_evidence import project_remote_deployment_evidence
    await connect(service)
    directory = service.directory(service.guide.identity)
    update_receipt(directory, target="gcp", remote_admission_requested=True, admitted=True,
        status="running", last_known_status="running", ready=True, observation={"available": True})
    async def unavailable(endpoint): raise OSError("private-cloud-host-path-secret")
    service.cloud_removal_status = unavailable
    state = deepcopy(service.guide.state)
    before = len(service.transport.calls)
    receipt = await service.guide_status(state["id"], state=state)
    evidence = project_remote_deployment_evidence(state, receipt)
    assert evidence["saved_installation"] and evidence["owned_source_verified"]
    assert evidence["installation_admitted"] and evidence["last_observed_status"] == "running"
    assert not evidence["observation_available"] and evidence["status"] is None
    assert not evidence["native_runtime_available"] and evidence["cloud_status"] is None
    assert evidence["suggested_actions"] == ["Manage remote destination", "Recheck remote status"]
    assert "private-cloud-host-path-secret" not in str(evidence)
    assert len(service.transport.calls) == before
    with pytest.raises(DraftConflict, match="cloud removal state"):
        await service.status(state["id"])


async def test_preflight_is_read_only_and_pins_source_package_host_before_private_admission(service):
    receipt = await connect(service)
    assert receipt["status"] == "configuration-required" and receipt["can_install"]
    assert receipt["destination"] == "/home/test/src/team"
    assert receipt["host_key_fingerprint"] == service.transport.key
    assert receipt["accepted_source_digest"] != receipt["transferred_source_digest"] != receipt["package_sha256"]
    assert all("settings" not in call["payload"] for call in service.transport.calls)
    accepted = await install(service, receipt)
    assert accepted["status"] == "installing" and not accepted["can_install"] and not accepted["can_stop"]
    mutation = service.transport.calls[-1]
    assert mutation["pin"] == receipt["host_key_fingerprint"]
    assert mutation["payload"]["expected_host"] == {key: service.transport.host[key] for key in ("uid", "home", "destination")}
    assert mutation["payload"]["settings"]["OPENAI_API_KEY"] == "private-host-test-value"
    assert "private-host-test-value" not in json.dumps(accepted)
    for path in service.root.rglob("*"):
        if path.is_file():
            assert b"private-host-test-value" not in path.read_bytes()
            assert path.stat().st_mode & 0o077 == 0


async def test_initial_console_port_is_retained_as_nonsecret_intent_with_exact_admission_binding(service):
    receipt = await connect(service)
    directory = service.directory(service.guide.identity)
    before = {name: (directory / name).read_bytes() for name in ("package.zip", "controller.py")}
    intent_at_admission = []

    async def observe_admission(payload):
        if payload["action"] == "install":
            saved = read_receipt(directory)
            intent_at_admission.append((saved["requested_console_port"],
                saved["pending_operation"]["id"], saved["pending_operation"]["action"]))

    service.transport.hook = observe_admission
    accepted = await install(service, receipt, MOBKIT_PORT="24173")
    mutation = service.transport.calls[-1]["payload"]
    assert mutation["action"] == "install" and mutation["settings"]["MOBKIT_PORT"] == "24173"
    assert accepted["requested_console_port"] == 24173
    saved = read_receipt(directory)
    assert type(saved["requested_console_port"]) is int and saved["requested_console_port"] == 24173
    assert saved["install_operation_id"] == mutation["operation_id"]
    assert intent_at_admission == [(24173, mutation["operation_id"], "install")]
    assert all(mutation["binding"][key] == receipt[key] for key in mutation["binding"])
    assert mutation["expected_host"] == {key: service.transport.host[key] for key in ("uid", "home", "destination")}
    assert before == {name: (directory / name).read_bytes() for name in before}
    assert not (directory / ".env").exists()
    assert "private-host-test-value" not in json.dumps(accepted)
    assert all(b"private-host-test-value" not in path.read_bytes() for path in directory.rglob("*") if path.is_file())


@pytest.mark.parametrize("changed,key", [
    ({"CONSOLE_USER_ID": "private principal-marker"}, "CONSOLE_USER_ID"),
    ({"CONSOLE_LOGIN_PASSWORD": "tiny-marker"}, "CONSOLE_LOGIN_PASSWORD"),
])
async def test_console_intrinsic_rules_reject_before_ssh_admission_or_custody_mutation(service, changed, key):
    service.implementation.extra_files["application/console.json"] = '{"authentication":"local-password"}'
    service.implementation.application_configuration = {"fields": list(CONSOLE_FIELDS)}
    receipt = await connect(service)
    assert receipt["console_scope"]["authentication"] == "local-password"
    directory = service.directory(service.guide.identity)
    before = {path.relative_to(directory): path.read_bytes() for path in directory.rglob("*") if path.is_file()}
    call_count = len(service.transport.calls)
    with pytest.raises(ValueError, match=key) as failure:
        await install(service, receipt, **{"CONSOLE_USER_ID": "owner@example.test",
            "CONSOLE_LOGIN_PASSWORD": "valid-password-123", **changed})
    assert "tiny-marker" not in str(failure.value) and "principal-marker" not in str(failure.value)
    assert len(service.transport.calls) == call_count
    assert before == {path.relative_to(directory): path.read_bytes() for path in directory.rglob("*") if path.is_file()}


async def test_console_remote_host_passes_exact_valid_password_without_local_storage(service):
    service.implementation.extra_files["application/console.json"] = '{"authentication":"local-password"}'
    service.implementation.application_configuration = {"fields": list(CONSOLE_FIELDS)}
    receipt = await connect(service)
    exact = " \tvalid private password\t "
    result = await install(service, receipt, CONSOLE_USER_ID="owner@example.test", CONSOLE_LOGIN_PASSWORD=exact)
    assert service.transport.calls[-1]["payload"]["settings"]["CONSOLE_LOGIN_PASSWORD"] == exact
    assert exact not in json.dumps(result)
    for path in service.root.rglob("*"):
        if path.is_file():
            assert exact.encode() not in path.read_bytes()


@pytest.mark.parametrize("change", ["guide", "acceptance", "source"])
async def test_changed_guide_acceptance_or_source_blocks_private_admission(service, change):
    receipt = await connect(service)
    if change == "guide": service.guide.state["version"] += 1
    if change == "acceptance": service.implementation.accepted = False
    if change == "source": service.implementation.digest = "b" * 64
    with pytest.raises((ValueError, DraftConflict)):
        await install(service, receipt, MOBKIT_PORT="24173")
    assert not any(call["payload"]["action"] == "install" for call in service.transport.calls)
    assert "requested_console_port" not in read_receipt(service.directory(service.guide.identity))


@pytest.mark.parametrize("change", ["uid", "home", "destination", "key", "platform", "python"])
async def test_destination_drift_before_install_never_sends_private_values(service, change):
    receipt = await connect(service)
    if change == "key": service.transport.key = "SHA256:" + "B" * 43
    elif change == "uid": service.transport.host[change] = 1001
    elif change == "platform": service.transport.host[change] = "darwin"
    elif change == "python": service.transport.host[change] = "3.10.9"
    else: service.transport.host[change] += "/changed"
    with pytest.raises(DraftConflict):
        await install(service, receipt)
    assert not any("settings" in call["payload"] for call in service.transport.calls)


async def test_lost_install_response_preserves_custody_without_reposting_private_values(service):
    receipt = await connect(service)
    service.transport.fail = "install"
    uncertain = await install(service, receipt, MOBKIT_PORT="24173")
    assert uncertain["status"] == "observation-unavailable" and uncertain["last_known_status"] == "installing"
    assert not uncertain["can_install"] and uncertain["remote_admission_requested"]
    assert uncertain["requested_console_port"] == 24173
    again = await install(service, receipt)
    assert again["status"] == "not-installed" and again["can_install"]
    assert again["requested_console_port"] == 24173
    assert len([call for call in service.transport.calls if call["payload"]["action"] == "install"]) == 1
    original_id = read_receipt(service.directory(service.guide.identity))["install_operation_id"]
    service.transport.fail = None
    await install(service, receipt, OPENAI_API_KEY="reentered-private-value")
    mutations = [call["payload"] for call in service.transport.calls if call["payload"]["action"] == "install"]
    assert len(mutations) == 2 and all(item["operation_id"] == original_id for item in mutations)
    assert mutations[-1]["settings"]["OPENAI_API_KEY"] == "reentered-private-value"
    assert mutations[-1]["settings"]["MOBKIT_PORT"] == "24173"
    assert read_receipt(service.directory(service.guide.identity))["requested_console_port"] == 24173


@pytest.mark.parametrize("retained", [True, 24173.0, "24173", 1023, 65536, None])
async def test_initial_not_installed_retry_refuses_invalid_retained_console_port_without_admission(service, retained):
    receipt = await connect(service)
    service.transport.fail = "install"
    await install(service, receipt, MOBKIT_PORT="24173")
    observed = await install(service, receipt)
    assert observed["status"] == "not-installed" and observed["can_install"]
    directory = service.directory(service.guide.identity)
    update_receipt(directory, requested_console_port=retained)
    before = {path.relative_to(directory): path.read_bytes() for path in directory.rglob("*") if path.is_file()}
    call_count = len(service.transport.calls)
    with pytest.raises(ValueError, match="Console port"):
        await install(service, observed, MOBKIT_PORT="25173")
    assert len(service.transport.calls) == call_count
    assert before == {path.relative_to(directory): path.read_bytes() for path in directory.rglob("*") if path.is_file()}


@pytest.mark.parametrize("override,expected", [("", 24173), ("25173", 25173)])
async def test_initial_not_installed_retry_uses_retained_port_or_explicit_override(service, override, expected):
    receipt = await connect(service)
    service.transport.fail = "install"
    await install(service, receipt, MOBKIT_PORT="24173")
    observed = await install(service, receipt)
    service.transport.fail = None
    accepted = await install(service, observed, MOBKIT_PORT=override)
    mutation = service.transport.calls[-1]["payload"]
    assert mutation["settings"]["MOBKIT_PORT"] == str(expected)
    assert accepted["requested_console_port"] == expected


async def test_old_source_stays_visible_and_stoppable_without_restart_or_new_target(service):
    receipt = await connect(service)
    await install(service, receipt)
    observed = await running(service, receipt)
    assert observed["ready"] and observed["can_stop"]
    service.guide.state["version"] += 1
    service.guide.state["brief"] = {"purpose": "Changed application requirements"}
    older = await service.status(service.guide.identity)
    assert not older["source_binding_current"] and older["can_stop"] and not older["can_restart"]
    with pytest.raises(DraftConflict):
        await service.action(service.guide.identity, receipt["deployment_id"], "restart")
    stopped = await service.action(service.guide.identity, receipt["deployment_id"], "stop")
    assert stopped["status"] == "stopped" and not stopped["can_restart"]


async def test_owned_running_source_drift_is_visible_without_claiming_verified_readiness(service):
    receipt = await connect(service)
    await install(service, receipt)
    await running(service, receipt)
    service.transport.remote.update(source_current=False, ready=False)
    changed = await service.status(service.guide.identity, force=True)
    assert changed["status"] == "running" and not changed["ready"] and changed["can_stop"]
    assert not changed["can_restart"]


async def test_lost_control_response_reuses_operation_id_and_newer_pending_control_is_not_overwritten(service):
    receipt = await connect(service)
    await install(service, receipt)
    await running(service, receipt)
    service.transport.fail = "stop"
    uncertain = await service.action(service.guide.identity, receipt["deployment_id"], "stop")
    pending = uncertain["pending_operation"]
    assert pending["action"] == "stop" and uncertain["status"] == "observation-unavailable"
    # The status is a valid observation of the prior remote operation. It cannot
    # erase custody of the stop whose response was lost before admission.
    old = await service.status(service.guide.identity, force=True)
    assert old["pending_operation"] == pending and old["status"] == "running"
    service.transport.fail = None
    final = await service.action(service.guide.identity, receipt["deployment_id"], "stop")
    stops = [call["payload"] for call in service.transport.calls if call["payload"]["action"] == "stop"]
    assert len(stops) == 2 and all(call["operation_id"] == pending["id"] for call in stops)
    assert final["status"] == "stopped" and final["pending_operation"] is None


async def test_delayed_read_and_install_are_serialized_per_owned_destination(service):
    receipt = await connect(service)
    entered, release = asyncio.Event(), asyncio.Event()
    async def delay(payload):
        if payload["action"] == "preflight" and not entered.is_set():
            entered.set()
            await release.wait()
    service.transport.hook = delay
    old_read = asyncio.create_task(service.status(service.guide.identity, force=True))
    await entered.wait()
    mutation = asyncio.create_task(install(service, receipt))
    await asyncio.sleep(0)
    assert not mutation.done()
    release.set()
    await old_read
    accepted = await mutation
    final = read_receipt(service.directory(service.guide.identity))
    assert accepted["status"] == final["status"] == "installing" and final["remote_admission_requested"]


async def test_changed_frozen_controller_or_symlink_record_blocks_before_ssh(service):
    receipt = await connect(service)
    directory = service.directory(service.guide.identity)
    before = len(service.transport.calls)
    (directory / "controller.py").write_text("# changed controller\n")
    with pytest.raises(DraftConflict): await install(service, receipt)
    assert len(service.transport.calls) == before


async def test_fifo_private_record_is_refused_without_blocking_or_remote_admission(service):
    await connect(service)
    directory = service.directory(service.guide.identity)
    (directory / "controller.py").unlink()
    os.mkfifo(directory / "controller.py", 0o600)
    before = len(service.transport.calls)
    with pytest.raises(DraftConflict):
        service.stored(service.guide.identity)
    assert len(service.transport.calls) == before


async def test_failed_observation_keeps_evidence_but_must_match_the_submitted_operation(service):
    receipt = await connect(service)
    directory = service.directory(service.guide.identity)
    expected = str(uuid.uuid4())
    evidence = {"status": "failed", "operation_id": str(uuid.uuid4()), "worker_live": False, "cleanup_confirmed": True,
                "error": {"code": "operation-failed", "message": "The operation needs attention."}}
    with pytest.raises(DraftConflict):
        await service.adopt(directory, read_receipt(directory), evidence, expected_operation=expected)
    evidence["operation_id"] = expected
    await service.adopt(directory, read_receipt(directory), evidence, expected_operation=expected)
    result = read_receipt(directory)
    assert result["status"] == "failed" and result["remote_error_code"] == "operation-failed"
    assert result["worker_live"] is False and result["observation"]["available"] is True
    evidence["cleanup_confirmed"] = False
    await service.adopt(directory, result, evidence, expected_operation=expected)
    assert read_receipt(directory)["status"] == "recovery-needed"
    evidence.update(cleanup_confirmed=True, worker_live=True, ready=True, source_current=True)
    await service.adopt(directory, read_receipt(directory), evidence, expected_operation=expected)
    observed = read_receipt(directory)
    assert observed["status"] == "recovery-needed" and observed["ready"] is False
    evidence.pop("worker_live")
    await service.adopt(directory, observed, evidence, expected_operation=expected)
    assert read_receipt(directory)["status"] == "recovery-needed"
    evidence["worker_live"] = False
    await service.adopt(directory, read_receipt(directory), evidence, expected_operation=expected)
    observed = read_receipt(directory)
    assert observed["status"] == "failed" and observed["ready"] is False


def test_transfer_manifest_has_distinct_digest_and_excludes_private_content():
    source = {"app.py": (b"accepted source", 0o644), ".env": (b"private", 0o600),
              "data/notes": (b"private notes", 0o600), "SHA256SUMS": (b"obsolete", 0o644)}
    content, digest = source_archive(source)
    with tarfile.open(fileobj=io.BytesIO(content), mode="r:gz") as archive:
        assert set(archive.getnames()) == {"app.py", "SHA256SUMS"}
        expected = hashlib.sha256(b"accepted source").hexdigest()
        assert archive.extractfile("SHA256SUMS").read() == (expected + "  app.py\n").encode()
    assert digest == hashlib.sha256(b"app.py\0" + bytes.fromhex(expected)).hexdigest()


def test_repeated_transfer_of_same_accepted_source_keeps_exact_archive_custody(monkeypatch):
    import gzip
    files = {"app.py": (b"accepted source", 0o644)}
    monkeypatch.setattr(gzip.time, "time", lambda: 100)
    first, first_digest = source_archive(files)
    monkeypatch.setattr(gzip.time, "time", lambda: 200)
    repeated, repeated_digest = source_archive(files)
    assert first == repeated and first_digest == repeated_digest
    assert hashlib.sha256(first).hexdigest() == hashlib.sha256(repeated).hexdigest()


def test_remote_api_validation_never_echoes_private_keys_values_or_extra_members():
    from launchkit.server import app
    client = TestClient(app)
    for body in [{"settings": {"private-key-name": 42}}, {"private-extra-key-name": "private-extra-value"}]:
        response = client.post("/api/guide/owned/remote-deployment/install", json={
            "version": 4, "build_id": "build", "deployment_id": "installation", **body})
        assert response.status_code == 422
        assert "private-key-name" not in response.text and "private-extra" not in response.text
        assert '"input"' not in response.text and '"ctx"' not in response.text


async def test_remote_shutdown_closes_observation_without_claiming_remote_stop(service):
    receipt = await connect(service)
    await install(service, receipt)
    before = read_receipt(service.directory(service.guide.identity))
    await service.shutdown()
    after = read_receipt(service.directory(service.guide.identity))
    assert before == after and after["status"] == "installing"
    assert not any(call["payload"]["action"] == "stop" for call in service.transport.calls)


async def terminal_failed(service, **settings):
    receipt = await connect(service)
    await install(service, receipt, **settings)
    operation = read_receipt(service.directory(service.guide.identity))["install_operation_id"]
    service.transport.remote = {"status": "failed", "admitted": True, "operation_id": operation,
        "worker_live": False, "cleanup_confirmed": True, "source_current": True,
        "service_observed": True, "service_owned": False, "service": {"status": "not-installed"},
        "retryable": True, "retry_of_operation_id": operation, "failure_phase": "dependencies",
        "error": {"code": "operation-failed", "message": "The dependency phase failed."}}
    return await service.status(service.guide.identity, force=True)


async def retry_install(service, receipt, **settings):
    return await service.retry_install(service.guide.identity, 4, service.implementation.identity,
        receipt["deployment_id"], receipt["retry_of_operation_id"],
        {"OPENAI_API_KEY": "fresh-private-retry-value", **settings})


async def test_terminal_failure_retry_uses_new_attempt_and_preserves_installed_source(service):
    receipt = await terminal_failed(service)
    assert receipt["can_retry_install"] and receipt["fields"] and not receipt["can_install"]
    assert receipt["requested_console_port"] == 18088
    accepted = await retry_install(service, receipt)
    mutation = service.transport.calls[-1]
    body = mutation["payload"]
    assert body["action"] == "retry-install" and "archive" not in body
    assert body["operation_id"] != receipt["operation_id"]
    assert body["previous_operation_id"] == receipt["operation_id"]
    assert body["settings"]["OPENAI_API_KEY"] == "fresh-private-retry-value"
    assert "MOBKIT_PORT" not in body["settings"]
    assert accepted["requested_console_port"] == receipt["requested_console_port"]
    assert mutation["pin"] == receipt["host_key_fingerprint"]
    assert accepted["status"] == "installing" and not accepted["can_retry_install"] and accepted["fields"] == []
    assert accepted["previous_operation_id"] == receipt["operation_id"]
    assert not accepted["retryable"] and accepted["retry_of_operation_id"] is None
    for path in service.root.rglob("*"):
        if path.is_file():
            assert b"fresh-private-retry-value" not in path.read_bytes()


@pytest.mark.parametrize("legacy", [False, True])
@pytest.mark.parametrize("override", [None, ""])
async def test_failed_retry_omits_synthesized_port_for_guest_private_configuration_authority(service, legacy, override):
    receipt = await terminal_failed(service, MOBKIT_PORT="24173")
    directory = service.directory(service.guide.identity)
    if legacy:
        saved = read_receipt(directory)
        saved.pop("requested_console_port")
        write_private(directory / "receipt.json", saved)
    before = {name: (directory / name).read_bytes() for name in ("package.zip", "controller.py")}
    changes = {} if override is None else {"MOBKIT_PORT": override}
    accepted = await retry_install(service, receipt, **changes)
    mutation = service.transport.calls[-1]["payload"]
    assert mutation["action"] == "retry-install" and "MOBKIT_PORT" not in mutation["settings"]
    assert mutation["previous_operation_id"] == receipt["operation_id"]
    assert mutation["binding"]["package_sha256"] == receipt["package_sha256"]
    assert before == {name: (directory / name).read_bytes() for name in before}
    if legacy:
        assert "requested_console_port" not in accepted and "requested_console_port" not in read_receipt(directory)
    else:
        assert accepted["requested_console_port"] == 24173
    assert all(b"fresh-private-retry-value" not in path.read_bytes() for path in directory.rglob("*") if path.is_file())


@pytest.mark.parametrize("legacy", [False, True])
async def test_failed_retry_explicit_console_port_updates_nonsecret_intent_without_changing_source(service, legacy):
    receipt = await terminal_failed(service, MOBKIT_PORT="24173")
    directory = service.directory(service.guide.identity)
    if legacy:
        saved = read_receipt(directory)
        saved.pop("requested_console_port")
        write_private(directory / "receipt.json", saved)
    before = {name: (directory / name).read_bytes() for name in ("package.zip", "controller.py")}
    accepted = await retry_install(service, receipt, MOBKIT_PORT="25173")
    mutation = service.transport.calls[-1]["payload"]
    assert mutation["settings"]["MOBKIT_PORT"] == "25173"
    assert mutation["previous_operation_id"] == receipt["operation_id"]
    assert accepted["requested_console_port"] == 25173
    assert read_receipt(directory)["requested_console_port"] == 25173
    assert before == {name: (directory / name).read_bytes() for name in before}
    assert all(b"fresh-private-retry-value" not in path.read_bytes() for path in directory.rglob("*") if path.is_file())


@pytest.mark.parametrize("change", ["worker", "cleanup", "source", "service-running", "service-unobserved",
    "service-foreign", "capability", "predecessor", "pending-stop"])
async def test_retry_requires_current_joined_failure_cleanup_source_and_service_evidence(service, change):
    receipt = await terminal_failed(service)
    remote = service.transport.remote
    if change == "worker": remote["worker_live"] = True
    if change == "cleanup": remote["cleanup_confirmed"] = False
    if change == "source": remote["source_current"] = False
    if change == "service-running": remote["service"] = {"status": "running"}
    if change == "service-unobserved": remote["service_observed"] = False
    if change == "service-foreign": remote["service"] = {"status": "inactive"}
    if change == "capability": remote["retryable"] = False
    if change == "predecessor": remote["retry_of_operation_id"] = str(uuid.uuid4())
    if change == "pending-stop":
        update_receipt(service.directory(service.guide.identity), pending_operation={"id": str(uuid.uuid4()), "action": "stop"})
    with pytest.raises(DraftConflict):
        await retry_install(service, receipt)
    assert not any(call["payload"]["action"] == "retry-install" for call in service.transport.calls)
    observed = await service.status(service.guide.identity, force=True)
    assert not observed["can_retry_install"] and observed["fields"] == []


async def test_lost_retry_before_admission_deliberately_reuses_same_attempt_uuid(service):
    receipt = await terminal_failed(service)
    service.transport.fail = "retry-install"
    uncertain = await retry_install(service, receipt)
    assert uncertain["status"] == "observation-unavailable" and not uncertain["can_retry_install"]
    pending = uncertain["pending_operation"]
    assert pending["action"] == "retry-install" and pending["previous_operation_id"] == receipt["operation_id"]
    observed = await service.status(service.guide.identity, force=True)
    assert observed["can_retry_install"] and observed["pending_operation"] == pending
    service.transport.fail = None
    final = await retry_install(service, observed, OPENAI_API_KEY="deliberately-reentered-value")
    retries = [call["payload"] for call in service.transport.calls if call["payload"]["action"] == "retry-install"]
    assert len(retries) == 2 and all(call["operation_id"] == pending["id"] for call in retries)
    assert retries[-1]["settings"]["OPENAI_API_KEY"] == "deliberately-reentered-value"
    assert final["status"] == "installing"


async def test_lost_retry_after_admission_observes_new_attempt_without_resending_settings(service):
    receipt = await terminal_failed(service)
    async def admitted_but_lost(body):
        if body["action"] == "retry-install":
            service.transport.remote = {"status": "installing", "admitted": True,
                "operation_id": body["operation_id"], "previous_operation_id": body["previous_operation_id"],
                "worker_live": True, "source_current": True, "ready": False}
            raise SshTransportError("timeout")
    service.transport.hook = admitted_but_lost
    first = await retry_install(service, receipt)
    assert first["status"] == "observation-unavailable"
    again = await retry_install(service, receipt, OPENAI_API_KEY="must-not-be-resent")
    assert again["status"] == "installing" and again["operation_id"] == first["pending_operation"]["id"]
    retries = [call["payload"] for call in service.transport.calls if call["payload"]["action"] == "retry-install"]
    assert len(retries) == 1 and "must-not-be-resent" not in json.dumps(service.transport.calls)


@pytest.mark.parametrize("change", ["guide", "acceptance", "source", "uid", "home", "destination", "key", "python", "platform", "preflight-connection"])
async def test_retry_rechecks_source_and_destination_before_sending_private_values(service, change):
    receipt = await terminal_failed(service)
    if change == "guide": service.guide.state["version"] += 1
    elif change == "acceptance": service.implementation.accepted = False
    elif change == "source": service.implementation.digest = "b" * 64
    elif change == "uid": service.transport.host[change] += 1
    elif change == "key": service.transport.key = "SHA256:" + "B" * 43
    elif change == "python": service.transport.host[change] = "3.10.1"
    elif change == "platform": service.transport.host[change] = "darwin"
    elif change == "preflight-connection": service.transport.fail = "preflight"
    else: service.transport.host[change] += "/changed"
    if change == "preflight-connection":
        observed = await retry_install(service, receipt)
        assert observed["status"] == "observation-unavailable" and not observed["can_retry_install"]
    else:
        with pytest.raises((DraftConflict, ValueError)):
            await retry_install(service, receipt)
    assert not any(call["payload"]["action"] == "retry-install" for call in service.transport.calls)


async def test_retry_of_new_terminal_attempt_gets_fresh_uuid_and_old_post_is_observation_only(service):
    receipt = await terminal_failed(service)
    first = await retry_install(service, receipt)
    first_id = first["operation_id"]
    service.transport.remote = {**service.transport.remote, "status": "failed", "worker_live": False,
        "cleanup_confirmed": True, "service_observed": True, "service_owned": False,
        "service": {"status": "not-installed"}, "retryable": True, "retry_of_operation_id": first_id}
    # Repeating the old POST only observes its admitted retry, even if that retry failed.
    duplicate = await retry_install(service, receipt)
    assert duplicate["operation_id"] == first_id and duplicate["can_retry_install"]
    second = await retry_install(service, duplicate)
    assert second["operation_id"] != first_id and second["previous_operation_id"] == first_id
    assert len([call for call in service.transport.calls if call["payload"]["action"] == "retry-install"]) == 2


async def test_retry_predecessor_must_be_exact_and_retry_capability_does_not_survive_new_observation(service):
    receipt = await terminal_failed(service)
    with pytest.raises(ValueError):
        await service.retry_install(service.guide.identity, 4, service.implementation.identity,
            receipt["deployment_id"], "not-an-attempt", {"OPENAI_API_KEY": "never-sent"})
    service.transport.remote.pop("retryable")
    observed = await service.status(service.guide.identity, force=True)
    assert not observed["retryable"] and not observed["can_retry_install"] and observed["fields"] == []


def test_remote_retry_api_validation_redacts_private_keys_values_and_extra_members():
    from launchkit.server import app
    client = TestClient(app)
    for changes in [{"settings": {"retry-private-key": 42}}, {"extra-retry-private-key": "extra-private-value"},
                    {"previous_operation_id": "malformed-private-canary"}]:
        response = client.post("/api/guide/owned/remote-deployment/retry", json={"version": 4,
            "build_id": "build", "deployment_id": "installation", "previous_operation_id": str(uuid.uuid4()), **changes})
        assert response.status_code == 422
        assert "retry-private-key" not in response.text and "private-canary" not in response.text
        assert "extra-private-value" not in response.text and '"input"' not in response.text and '"ctx"' not in response.text


@pytest.mark.parametrize("quiescent", [True, False, None])
async def test_owned_inactive_service_retry_needs_explicit_process_quiescence(service, quiescent):
    await terminal_failed(service)
    service.transport.remote.update(service_owned=True,
        service={"status": "inactive", "quiescent": quiescent})
    observed = await service.status(service.guide.identity, force=True)
    assert observed["can_retry_install"] is (quiescent is True)
    if quiescent is True:
        assert (await retry_install(service, observed))["status"] == "installing"
    else:
        with pytest.raises(DraftConflict):
            await retry_install(service, observed)


def test_retry_api_routes_exact_current_binding_and_predecessor(monkeypatch):
    from unittest.mock import AsyncMock
    from launchkit import server
    result = {"deployment_id": str(uuid.uuid4()), "status": "installing"}
    retry = AsyncMock(return_value=result)
    monkeypatch.setattr(server.remote_deployment, "retry_install", retry)
    identity, build, previous = str(uuid.uuid4()), str(uuid.uuid4()), str(uuid.uuid4())
    response = TestClient(server.app).post(f"/api/guide/{identity}/remote-deployment/retry", json={
        "version": 4, "build_id": build, "deployment_id": result["deployment_id"],
        "previous_operation_id": previous, "settings": {"OPENAI_API_KEY": "api-retry-private-canary"}})
    assert response.status_code == 202 and response.json() == result
    retry.assert_awaited_once_with(identity, 4, build, result["deployment_id"], previous,
        {"OPENAI_API_KEY": "api-retry-private-canary"})
    assert "api-retry-private-canary" not in response.text


@pytest.mark.parametrize("pending", [False, 0, "", [], "invalid-operation"])
async def test_malformed_pending_custody_never_grants_a_retry(service, pending):
    await terminal_failed(service)
    update_receipt(service.directory(service.guide.identity), pending_operation=pending)
    observed = await service.status(service.guide.identity, force=True)
    assert not observed["can_retry_install"] and observed["fields"] == []


@pytest.mark.parametrize("stage", ["destination-check", "install", "retry-install"])
async def test_every_destination_check_uses_only_granted_frozen_package_commands(service, stage):
    service.implementation.mcp_connections = [
        {"id": "first", "transport": "stdio", "command": "npx"},
        {"id": "second", "transport": "stdio", "command": "npx"},
        {"id": "package", "transport": "stdio", "command": ".venv/bin/custom-mcp"},
        {"id": "unused", "transport": "stdio", "command": "unavailable-unused"},
        {"id": "http", "transport": "http", "url": "https://example.test/mcp"},
    ]
    service.implementation.mcp_grants = [{"connection_id": key} for key in ["first", "second", "package", "http"]]
    receipt = await terminal_failed(service) if stage == "retry-install" else await connect(service)
    if stage == "install":
        await install(service, receipt)
    elif stage == "retry-install":
        await retry_install(service, receipt)
    probes = [item["payload"] for item in service.transport.calls if item["payload"]["action"] == "preflight"]
    assert len(probes) >= (1 if stage == "destination-check" else 2)
    assert all(item["mcp_prerequisites"] == ["npx"] and
               item["mcp_deferred_prerequisites"] == ["package"] for item in probes)
    assert all("settings" not in item for item in probes)


async def test_missing_granted_descriptor_fails_before_ssh_without_private_admission(service):
    service.implementation.mcp_grants = [{"connection_id": "missing"}]
    receipt = await connect(service)
    assert receipt["status"] == "observation-unavailable" and not receipt["can_install"]
    assert service.transport.calls == [] and receipt["fields"] == []


async def test_transcript_only_revision_preserves_installed_status_and_owned_restart(service):
    receipt = await connect(service)
    await install(service, receipt)
    await running(service, receipt)
    directory = service.directory(service.guide.identity)
    original = read_receipt(directory)
    protected = {name: (directory / name).read_bytes() for name in ("package.zip", "controller.py")}
    service.guide.state["version"] = 5
    current = await service.status(service.guide.identity, force=True)
    assert current["source_binding_current"] and current["current_guide_version"] == 5
    assert current["guide_version"] == current["build_guide_version"] == 4
    assert current["can_stop"] and current["ready"]
    stopped = await service.action(service.guide.identity, receipt["deployment_id"], "stop")
    assert stopped["status"] == "stopped" and stopped["can_restart"]
    restarted = await service.action(service.guide.identity, receipt["deployment_id"], "restart")
    assert restarted["status"] == "running" and restarted["source_binding_current"]
    assert restarted["current_guide_version"] == 5 and restarted["guide_version"] == 4
    controls = [call["payload"] for call in service.transport.calls if call["payload"]["action"] in {"stop", "restart"}]
    assert [body["action"] for body in controls] == ["stop", "restart"]
    assert controls[0]["operation_id"] != controls[1]["operation_id"]
    assert controls[0]["binding"] == controls[1]["binding"]
    assert controls[0]["binding"]["guide_version"] == 4
    assert all("request_version" not in body for body in controls)
    saved = read_receipt(directory)
    assert saved["install_operation_id"] == original["install_operation_id"]
    assert all(saved[key] == original[key] for key in ("guide_version", "build_guide_version", "build_id",
        "package_sha256", "controller_sha256", "transferred_source_digest", "endpoint", "host", "destination"))
    assert protected == {name: (directory / name).read_bytes() for name in protected}


async def test_transcript_only_revision_can_admit_exact_saved_private_setup_but_not_old_post(service):
    receipt = await connect(service)
    service.guide.state["version"] = 5
    before = len(service.transport.calls)
    with pytest.raises(DraftConflict, match="guide changed"):
        await service.install(service.guide.identity, 4, service.implementation.identity, receipt["deployment_id"],
            {"OPENAI_API_KEY": "stale-private-canary"})
    assert len(service.transport.calls) == before
    accepted = await service.install(service.guide.identity, 5, service.implementation.identity, receipt["deployment_id"],
        {"OPENAI_API_KEY": "current-private-canary", "MOBKIT_PORT": "24173"})
    body = service.transport.calls[-1]["payload"]
    assert body["action"] == "install" and body["binding"]["guide_version"] == 4
    assert body["settings"]["OPENAI_API_KEY"] == "current-private-canary"
    assert accepted["guide_version"] == accepted["build_guide_version"] == 4
    assert accepted["current_guide_version"] == 5 and accepted["requested_console_port"] == 24173
    assert "request_version" not in body
    for path in service.root.rglob("*"):
        if path.is_file():
            assert b"current-private-canary" not in path.read_bytes()
            assert b"stale-private-canary" not in path.read_bytes()


async def test_transcript_only_revision_retries_exact_failed_attempt_without_changing_original_binding(service):
    receipt = await terminal_failed(service, MOBKIT_PORT="24173")
    service.guide.state["version"] = 5
    current = await service.status(service.guide.identity)
    assert current["can_retry_install"] and current["current_guide_version"] == 5
    accepted = await service.retry_install(service.guide.identity, 5, service.implementation.identity,
        receipt["deployment_id"], receipt["operation_id"], {"OPENAI_API_KEY": "current-retry-canary"})
    body = service.transport.calls[-1]["payload"]
    assert body["action"] == "retry-install" and body["previous_operation_id"] == receipt["operation_id"]
    assert body["operation_id"] != receipt["operation_id"] and body["binding"]["guide_version"] == 4
    assert "MOBKIT_PORT" not in body["settings"] and "request_version" not in body
    assert accepted["guide_version"] == accepted["build_guide_version"] == 4
    assert accepted["current_guide_version"] == 5 and accepted["requested_console_port"] == 24173


@pytest.mark.parametrize("change", ["blueprint", "brief", "acceptance", "source"])
async def test_changed_application_after_chat_cannot_inherit_current_remote_installation(service, change):
    receipt = await connect(service)
    await install(service, receipt)
    await running(service, receipt)
    service.guide.state["version"] = 5
    if change == "blueprint": service.guide.state["proposal"]["project"]["name"] += " changed"
    elif change == "brief": service.guide.state["brief"] = {"purpose": "Different application"}
    elif change == "acceptance": service.implementation.accepted = False
    else: service.implementation.digest = "b" * 64
    observed = await service.status(service.guide.identity)
    assert not observed["source_binding_current"] and observed["current_guide_version"] is None
    assert observed["can_stop"] and not observed["can_restart"]
    before = len(service.transport.calls)
    with pytest.raises(DraftConflict):
        await service.action(service.guide.identity, receipt["deployment_id"], "restart")
    assert len(service.transport.calls) == before


@pytest.mark.parametrize("action", ["install", "retry-install"])
async def test_current_private_post_is_refused_if_transcript_changes_during_destination_check(service, action):
    receipt = await connect(service) if action == "install" else await terminal_failed(service)
    service.guide.state["version"] = 5
    async def changed(payload):
        if payload["action"] == "preflight": service.guide.state["version"] = 6
    service.transport.hook = changed
    before = len(service.transport.calls)
    with pytest.raises(DraftConflict, match="guide changed"):
        if action == "install":
            await service.install(service.guide.identity, 5, service.implementation.identity,
                receipt["deployment_id"], {"OPENAI_API_KEY": "late-private-canary"})
        else:
            await service.retry_install(service.guide.identity, 5, service.implementation.identity,
                receipt["deployment_id"], receipt["operation_id"], {"OPENAI_API_KEY": "late-private-canary"})
    calls = service.transport.calls[before:]
    assert calls and not any(call["payload"]["action"] in {"install", "retry-install"} for call in calls)
    saved = read_receipt(service.directory(service.guide.identity))
    assert saved.get("pending_operation") is None and saved["guide_version"] == 4
    for path in service.root.rglob("*"):
        if path.is_file(): assert b"late-private-canary" not in path.read_bytes()


async def test_restart_rechecks_current_source_after_awaited_browser_cleanup(service):
    receipt = await connect(service)
    await install(service, receipt)
    await running(service, receipt)
    await service.action(service.guide.identity, receipt["deployment_id"], "stop")
    service.guide.state["version"] = 5
    original = service.browser_access.revoke
    async def changed(*args, **kwargs):
        value = await original(*args, **kwargs)
        service.guide.state["version"] = 6
        return value
    service.browser_access.revoke = changed
    before = len(service.transport.calls)
    with pytest.raises(DraftConflict, match="guide changed"):
        await service.action(service.guide.identity, receipt["deployment_id"], "restart")
    assert len(service.transport.calls) == before
    saved = read_receipt(service.directory(service.guide.identity))
    assert saved["status"] == "stopped" and saved.get("pending_operation") is None


async def test_connect_after_chat_retains_exact_build_version_and_later_current_snapshot(service):
    service.guide.state["version"] = 5
    receipt = await service.connect(service.guide.identity, 5, service.implementation.identity,
        "owned.example", "test", 22, "~/src/team")
    await asyncio.gather(*list(service.tasks.values()))
    directory = service.directory(service.guide.identity)
    saved = read_receipt(directory)
    assert saved["guide_version"] == 5 and saved["build_guide_version"] == 4
    service.guide.state["version"] = 6
    observed = await service.status(service.guide.identity)
    interview = await service.guide_status(service.guide.identity, state=deepcopy(service.guide.state))
    assert observed["source_binding_current"] and observed["current_guide_version"] == 6 and observed["can_install"]
    assert interview["owned_package_verified"] and interview["source_binding_current"]
    assert interview["guide_version"] == 5 and interview["current_guide_version"] == 6
    assert read_receipt(directory) == saved


@pytest.mark.parametrize("provenance", [None, True, 3, 5])
async def test_tampered_saved_build_provenance_never_admits_remote_observation(service, provenance):
    await connect(service)
    directory = service.directory(service.guide.identity)
    update_receipt(directory, build_guide_version=provenance)
    before = len(service.transport.calls)
    with pytest.raises(DraftConflict): await service.status(service.guide.identity, force=True)
    assert len(service.transport.calls) == before


async def test_legacy_receipt_uses_exact_original_version_without_rewriting_private_record(service):
    await connect(service)
    directory = service.directory(service.guide.identity)
    saved = read_receipt(directory)
    saved.pop("build_guide_version")
    write_private(directory / "receipt.json", saved)
    original = (directory / "receipt.json").read_bytes()
    service.guide.state["version"] = 5
    observed = await service.status(service.guide.identity)
    assert observed["source_binding_current"] and observed["current_guide_version"] == 5
    assert observed["guide_version"] == 4 and "build_guide_version" not in observed
    assert (directory / "receipt.json").read_bytes() == original


@pytest.mark.parametrize("drift", [None, "transport", "vm", "owner", "transport-replaced", "other-build"])
async def test_private_product_gcp_staging_uses_original_iap_owner_and_cloud_identity(service, monkeypatch, drift):
    from test_gcp_access import endpoint as cloud_endpoint
    from launchkit import remote_product_destination_owner as product_source
    from launchkit.artifact_binding import proposal_hash
    operation = str(uuid.uuid4())
    endpoint = cloud_endpoint(operation)
    proposal = service.guide.state["proposal"]
    proposal["target"] = "gcp"
    proposal["gcp"].update(project=endpoint["project"], zone=endpoint["zone"])
    endpoint["instance"] = proposal["project"]["slug"]
    cloud = {"status": "vm-ready", "vm_verified": True, "guide_id": service.guide.identity,
        "operation_id": operation, "endpoint": deepcopy(endpoint),
        "destination": {key: endpoint[key] for key in ("project", "zone", "instance")},
        "binding": {"guide_id": service.guide.identity, "guide_version": 4,
            "build_id": service.implementation.identity, "blueprint_hash": proposal_hash(proposal),
            "source_digest": service.implementation.digest, "package_sha256": "d" * 64}}
    if drift == "other-build": cloud["binding"]["build_id"] = str(uuid.uuid4())
    observations, staging, current = [], [], [True]
    class CloudTransport:
        async def verify(self, value):
            observations.append(deepcopy(value))
            if drift == "owner": current[0] = False
            if drift == "transport-replaced": service.cloud_transport = CloudTransport()
            returned = deepcopy(value)
            if drift == "vm": returned["instance_id"] = "741103"
            return returned
    original = CloudTransport()
    service.cloud_transport = original
    async def accepted_source(implementation, identity, **kwargs):
        assert implementation is service.implementation and identity == service.guide.identity
        staging.append(kwargs)
        return {"status": "source-only-fixture"}
    monkeypatch.setattr(product_source, "stage_product_destination_source", accepted_source)
    arguments = {"build_id": service.implementation.identity, "source_digest": service.implementation.digest,
        "guide_version": 4, "deployment_id": str(uuid.uuid4()), "custody": object(),
        "transaction_directory": service.root / "inert-intake", "destination_host": {},
        "transport": CloudTransport() if drift == "transport" else original,
        "endpoint": endpoint, "cloud": cloud, "expected_fingerprint": "inert", "require_current": lambda: current[0]}
    if drift:
        with pytest.raises((DraftConflict, ValueError)):
            await service.stage_product_source(service.guide.identity, **arguments)
        assert not staging and not service.root.exists()
        assert observations == ([] if drift in {"transport", "other-build"} else [endpoint])
        return
    result = await service.stage_product_source(service.guide.identity, **arguments)
    assert result["status"] == "source-only-fixture" and observations == [endpoint]
    assert len(staging) == 1
    assert staging[0]["source_disposition"] == {"target": "gcp", "binding": cloud["binding"], **{
        key: endpoint[key] for key in ("project", "zone", "instance", "instance_id", "operation_id")}}
    assert staging[0]["endpoint"] == endpoint and staging[0]["transport"] is original
    assert 0 < staging[0]["timeout"] <= 600
    assert not service.root.exists()


@pytest.mark.parametrize("drift", [None, "package", "destination", "future-creation"])
async def test_cloud_connection_keeps_creation_build_and_connect_versions_separate(service, drift):
    from test_gcp_access import endpoint as cloud_endpoint
    from launchkit.artifact_binding import proposal_hash
    service.guide.state["proposal"]["target"] = "gcp"
    service.guide.state["proposal"]["gcp"].update(project="mobkit-cloud-project", zone="europe-west1-b")
    service.implementation.original = deepcopy(service.guide.state)
    selected = await service.accepted(service.guide.identity, 4, service.implementation.identity, target="gcp")
    operation = str(uuid.uuid4())
    endpoint = cloud_endpoint(operation)
    endpoint["instance"] = "host-test-team"
    cloud = {"status": "vm-ready", "vm_verified": True, "guide_id": service.guide.identity,
        "operation_id": operation, "endpoint": endpoint,
        "destination": {key: endpoint[key] for key in ("project", "zone", "instance")},
        "binding": {"guide_id": service.guide.identity, "guide_version": 4,
            "build_id": service.implementation.identity, "blueprint_hash": proposal_hash(service.guide.state["proposal"]),
            "source_digest": service.implementation.digest, "package_sha256": hashlib.sha256(selected["payload"]).hexdigest()}}
    class CloudTransport(Transport):
        async def verify(self, value): return deepcopy(value)
    service.cloud_transport = CloudTransport()
    service.guide.state["version"] = 5
    if drift == "package": cloud["binding"]["package_sha256"] = "b" * 64
    elif drift == "destination":
        cloud["endpoint"]["project"] = "other-cloud-project"
        cloud["destination"]["project"] = "other-cloud-project"
    elif drift == "future-creation": cloud["binding"]["guide_version"] = 6
    if drift:
        with pytest.raises(DraftConflict):
            await service.connect_cloud(service.guide.identity, 5, service.implementation.identity, cloud)
        assert not service.cloud_transport.calls and not service.root.exists()
        return
    receipt = await service.connect_cloud(service.guide.identity, 5, service.implementation.identity, cloud)
    await asyncio.gather(*list(service.tasks.values()))
    assert receipt["guide_version"] == 5 and receipt["build_guide_version"] == 4
    assert cloud["binding"]["guide_version"] == 4 and receipt["cloud_operation_id"] == operation
    service.guide.state["version"] = 6
    observed = await service.status(service.guide.identity)
    assert observed["source_binding_current"] and observed["current_guide_version"] == 6
    assert observed["guide_version"] == 5 and observed["build_guide_version"] == 4
    assert observed["endpoint"] == endpoint and observed["cloud_operation_id"] == operation


@pytest.mark.parametrize("error_code", ["timeout", "configuration_unavailable"])
async def test_initial_cloud_observation_failure_guides_same_vm_recheck_without_admission(service, error_code):
    from test_gcp_access import endpoint as cloud_endpoint
    from launchkit.artifact_binding import proposal_hash
    service.guide.state["proposal"]["target"] = "gcp"
    service.guide.state["proposal"]["gcp"].update(project="mobkit-cloud-project", zone="europe-west1-b")
    service.implementation.original = deepcopy(service.guide.state)
    selected = await service.accepted(service.guide.identity, 4, service.implementation.identity, target="gcp")
    operation = str(uuid.uuid4())
    endpoint = cloud_endpoint(operation)
    endpoint["instance"] = "host-test-team"
    cloud = {"status": "vm-ready", "vm_verified": True, "guide_id": service.guide.identity,
        "operation_id": operation, "endpoint": endpoint,
        "destination": {key: endpoint[key] for key in ("project", "zone", "instance")},
        "binding": {"guide_id": service.guide.identity, "guide_version": 4,
            "build_id": service.implementation.identity, "blueprint_hash": proposal_hash(service.guide.state["proposal"]),
            "source_digest": service.implementation.digest, "package_sha256": hashlib.sha256(selected["payload"]).hexdigest()}}
    class CloudTransport(Transport):
        async def verify(self, value): return deepcopy(value)
    service.cloud_transport = CloudTransport()
    async def fail_observation(payload):
        assert payload["action"] == "preflight"
        raise SshTransportError(error_code)
    service.cloud_transport.hook = fail_observation
    initial = await service.connect_cloud(service.guide.identity, 4, service.implementation.identity, cloud)
    await asyncio.gather(*list(service.tasks.values()))
    directory = service.directory(service.guide.identity)
    package = (directory / "package.zip").read_bytes()
    failed = await service.current_public(read_receipt(directory))
    assert failed["status"] == "observation-unavailable" and failed["observation"] == {"available": False}
    assert not failed["can_install"] and not failed["can_stop"] and not failed["can_restart"] and failed["fields"] == []
    if error_code == "configuration_unavailable":
        assert failed["detail"] == str(SshTransportError(error_code))
        assert "guest preparation" not in failed["detail"]
    else:
        assert "guest preparation may still be running" in failed["detail"]
        assert "Recheck remote status for this same VM" in failed["detail"]
        assert "This observation does not identify the cause." in failed["detail"]
    repeated = await service.status(service.guide.identity, force=True)
    assert repeated["detail"] == failed["detail"]
    assert repeated["endpoint"] == endpoint and repeated["cloud_operation_id"] == operation
    assert repeated["deployment_id"] == initial["deployment_id"] and repeated["preflight"] is None
    service.cloud_transport.hook = None
    recovered = await service.status(service.guide.identity, force=True)
    assert recovered["status"] == "configuration-required" and recovered["observation"]["available"]
    assert recovered["endpoint"] == endpoint and recovered["cloud_operation_id"] == operation
    assert recovered["deployment_id"] == initial["deployment_id"] and recovered["can_install"]
    assert "guest preparation" not in recovered["detail"]
    saved = read_receipt(directory)
    assert saved["remote_admission_requested"] is False and "install_operation_id" not in saved
    assert (directory / "package.zip").read_bytes() == package
    assert [call["payload"]["action"] for call in service.cloud_transport.calls] == ["preflight"] * 3
    assert service.transport.calls == []


@pytest.mark.parametrize("target_flag_only", [False, True])
async def test_initial_ordinary_observation_failure_does_not_infer_cloud_from_target_flag(service, target_flag_only):
    service.transport.fail = "preflight"
    initial = await connect(service)
    directory = service.directory(service.guide.identity)
    saved = read_receipt(directory)
    if target_flag_only:
        update_receipt(directory, target="gcp")
    service.unavailable(directory, error=SshTransportError("timeout"))
    failed = read_receipt(directory)
    assert failed["detail"] == initial["detail"]
    assert "guest preparation" not in failed["detail"]
    assert failed["endpoint"] == saved["endpoint"] and failed["remote_admission_requested"] is False
    assert "cloud_operation_id" not in failed and "install_operation_id" not in failed
    assert failed["observation"] == {"available": False} and failed["preflight"] is None
    assert all(call["payload"]["action"] == "preflight" for call in service.transport.calls)


async def test_remote_interview_evidence_leaves_the_event_loop_free(service, monkeypatch):
    from launchkit import remote_deployment
    await connect(service)
    state = deepcopy(service.guide.state)
    original = remote_deployment.package_files
    def slowed(payload):
        time.sleep(1.0)  # A large package on a small host.
        return original(payload)
    monkeypatch.setattr(remote_deployment, "package_files", slowed)
    stalls, done = [], asyncio.Event()
    async def other_request():
        while not done.is_set():
            started = time.monotonic()
            await asyncio.sleep(0.01)
            stalls.append(time.monotonic() - started)
    probe = asyncio.create_task(other_request())
    await asyncio.sleep(0)  # The other request is waiting on the loop.
    try:
        result = await service.guide_status(state["id"], state=state)
    finally:
        done.set()
        await probe
    assert result["status"] == "configuration-required" and result["owned_package_verified"]
    assert stalls and max(stalls) < 0.5


def slow_package_reads(monkeypatch):
    from launchkit import remote_deployment
    for name in ("package_files", "source_archive"):
        def slowed(*args, original=getattr(remote_deployment, name), **kwargs):
            time.sleep(1.0)  # A large package on a small host.
            return original(*args, **kwargs)
        monkeypatch.setattr(remote_deployment, name, slowed)


async def longest_loop_gap(operation):
    stalls, done = [], asyncio.Event()
    async def other_request():
        while not done.is_set():
            started = time.monotonic()
            await asyncio.sleep(0.01)
            stalls.append(time.monotonic() - started)
    probe = asyncio.create_task(other_request())
    await asyncio.sleep(0)  # The other request is waiting on the loop.
    try:
        result = await operation
    finally:
        done.set()
        await probe
    return result, max(stalls)


async def test_remote_connect_leaves_the_event_loop_free(service, monkeypatch):
    slow_package_reads(monkeypatch)
    connected, gap = await longest_loop_gap(connect(service))
    assert connected["status"] == "configuration-required"
    assert gap < 0.5


async def test_remote_install_leaves_the_event_loop_free(service, monkeypatch):
    receipt = await connect(service)
    slow_package_reads(monkeypatch)
    async def installed():
        value = await install(service, receipt)
        await asyncio.gather(*list(service.tasks.values()))
        return value
    accepted, gap = await longest_loop_gap(installed())
    assert accepted["status"] == "installing" and not accepted["can_install"]
    assert gap < 0.5


async def test_remote_status_observation_leaves_the_event_loop_free(service, monkeypatch):
    receipt = await connect(service)
    await install(service, receipt)
    await asyncio.gather(*list(service.tasks.values()))
    slow_package_reads(monkeypatch)
    observed, gap = await longest_loop_gap(running(service, receipt))
    assert observed["status"] == "running" and observed["ready"]
    assert gap < 0.5


def slow_package_hashing(monkeypatch):
    from launchkit import remote_deployment
    class Hashing:
        def __getattr__(self, name):
            return getattr(hashlib, name)
        def sha256(self, data=b""):
            if bytes(data[:4]) == b"PK\x03\x04":
                time.sleep(1.0)  # Hashing a large package on a small host.
            return hashlib.sha256(data)
    monkeypatch.setattr(remote_deployment, "hashlib", Hashing())


async def test_remote_connect_hashes_the_package_off_the_event_loop(service, monkeypatch):
    slow_package_hashing(monkeypatch)
    connected, gap = await longest_loop_gap(connect(service))
    assert connected["status"] == "configuration-required"
    assert gap < 0.5


async def test_remote_install_hashes_the_package_off_the_event_loop(service, monkeypatch):
    receipt = await connect(service)
    slow_package_hashing(monkeypatch)
    async def installed():
        value = await install(service, receipt)
        await asyncio.gather(*list(service.tasks.values()))
        return value
    accepted, gap = await longest_loop_gap(installed())
    assert accepted["status"] == "installing" and not accepted["can_install"]
    assert gap < 0.5


async def test_remote_status_hashes_the_package_off_the_event_loop(service, monkeypatch):
    receipt = await connect(service)
    await install(service, receipt)
    await asyncio.gather(*list(service.tasks.values()))
    slow_package_hashing(monkeypatch)
    observed, gap = await longest_loop_gap(running(service, receipt))
    assert observed["status"] == "running" and observed["ready"]
    assert gap < 0.5


async def test_remote_stop_and_restart_hash_the_package_off_the_event_loop(service, monkeypatch):
    receipt = await connect(service)
    await install(service, receipt)
    await asyncio.gather(*list(service.tasks.values()))
    await running(service, receipt)
    slow_package_hashing(monkeypatch)
    async def stopped_and_restarted():
        stopped = await service.action(service.guide.identity, receipt["deployment_id"], "stop")
        return stopped, await service.action(service.guide.identity, receipt["deployment_id"], "restart")
    (stopped, restarted), gap = await longest_loop_gap(stopped_and_restarted())
    assert stopped["status"] == "stopped" and restarted["status"] == "running"
    assert gap < 0.5


def slow_package_writes(monkeypatch):
    from launchkit import remote_deployment
    original = remote_deployment.write_bytes
    def slowed(path, content):
        if path.name == "package.zip":
            time.sleep(1.0)  # Writing and syncing a large package on a small host.
        return original(path, content)
    monkeypatch.setattr(remote_deployment, "write_bytes", slowed)


async def test_remote_connect_writes_the_package_off_the_event_loop(service, monkeypatch):
    slow_package_writes(monkeypatch)
    connected, gap = await longest_loop_gap(connect(service))
    saved = (service.directory(service.guide.identity) / "package.zip").read_bytes()
    assert connected["status"] == "configuration-required"
    assert hashlib.sha256(saved).hexdigest() == connected["package_sha256"]
    assert gap < 0.5


async def test_cancelled_connect_leaves_nothing_that_blocks_cloud_removal(service, monkeypatch):
    import threading
    from launchkit import remote_deployment
    entered, release = threading.Event(), threading.Event()
    original = remote_deployment.source_archive
    def held(files):
        entered.set()
        release.wait(30)
        return original(files)
    monkeypatch.setattr(remote_deployment, "source_archive", held)
    task = asyncio.create_task(service.connect(service.guide.identity, 4, service.implementation.identity,
        "owned.example", "test", 22, "~/src/team"))
    try:
        assert await asyncio.to_thread(entered.wait, 30)
        task.cancel()
    finally:
        release.set()
    with pytest.raises(asyncio.CancelledError):
        await task
    assert await prepare_removal(service) is True
    assert installation_entries(service) == ([], [])


async def prepare_removal(service):
    return await service.prepare_cloud_removal({"guide_id": service.guide.identity, "operation_id": str(uuid.uuid4())})


def installation_entries(service):
    """Names in the guide's installation inventory and in the connect staging area."""
    parent, staging = service.root / service.guide.identity, service.root / ".connecting"
    return (sorted(path.name for path in parent.iterdir()) if parent.exists() else [],
            sorted(path.name for path in staging.iterdir()) if staging.exists() else [])


async def test_refused_connect_leaves_nothing_that_blocks_cloud_removal(service, monkeypatch):
    from launchkit import remote_deployment
    monkeypatch.setattr(remote_deployment, "MAX_CONTROLLER", 1)
    with pytest.raises(DraftConflict, match="controller exceeded its original bound"):
        await service.connect(service.guide.identity, 4, service.implementation.identity,
            "owned.example", "test", 22, "~/src/team")
    assert await prepare_removal(service) is True
    assert installation_entries(service) == ([], [])


async def test_cloud_removal_still_refuses_an_installation_with_an_unreadable_receipt(service):
    await connect(service)
    (service.directory(service.guide.identity) / "receipt.json").write_text("not a receipt")
    with pytest.raises(ValueError):
        await prepare_removal(service)


async def test_cloud_removal_still_refuses_an_installation_directory_without_a_receipt(service):
    await connect(service)
    (service.root / service.guide.identity / str(uuid.uuid4())).mkdir(mode=0o700)
    with pytest.raises(FileNotFoundError):
        await prepare_removal(service)


async def test_connect_sweeps_an_abandoned_staging_copy_and_nothing_else(service, tmp_path):
    staging = service.root / ".connecting"
    staging.mkdir(parents=True, mode=0o700)
    abandoned = staging / str(uuid.uuid4())
    abandoned.mkdir(mode=0o700)
    (abandoned / "package.zip").write_bytes(b"partial copy")
    (staging / (abandoned.name + ".lock")).write_bytes(b"")
    foreign = staging / "not-a-connect"
    foreign.mkdir(mode=0o700)
    elsewhere = tmp_path / "elsewhere"
    elsewhere.mkdir(mode=0o700)
    (elsewhere / "keep").write_bytes(b"keep")
    linked = staging / str(uuid.uuid4())
    linked.symlink_to(elsewhere, target_is_directory=True)
    receipt = await connect(service)
    assert receipt["status"] == "configuration-required"
    assert not abandoned.exists() and not (staging / (abandoned.name + ".lock")).exists()
    assert foreign.is_dir() and linked.is_symlink() and (elsewhere / "keep").read_bytes() == b"keep"
    assert installation_entries(service)[1] == sorted([foreign.name, linked.name])
    assert await prepare_removal(service) is True


async def test_connect_keeps_a_staging_copy_whose_lock_is_held(service):
    import fcntl
    staging = service.root / ".connecting"
    staging.mkdir(parents=True, mode=0o700)
    live = staging / str(uuid.uuid4())
    live.mkdir(mode=0o700)
    (live / "package.zip").write_bytes(b"copy in progress")
    descriptor = os.open(staging / (live.name + ".lock"), os.O_CREAT | os.O_RDWR, 0o600)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        await connect(service)
        assert (live / "package.zip").read_bytes() == b"copy in progress"
    finally:
        os.close(descriptor)


async def test_connect_names_an_installation_only_after_its_record_is_in_place(service, monkeypatch):
    from launchkit import remote_deployment
    original, seen = remote_deployment.write_private, []
    def witnessed(path, value):
        if path.name == "latest.json":
            seen.append(((path.parent / value["deployment_id"] / "receipt.json").is_file(),
                (service.root / ".connecting" / value["deployment_id"]).exists()))
        return original(path, value)
    monkeypatch.setattr(remote_deployment, "write_private", witnessed)
    receipt = await connect(service)
    assert seen == [(True, False)]
    assert installation_entries(service) == (sorted([receipt["deployment_id"], "latest.json"]), [])


async def test_connect_never_replaces_an_existing_installation_directory(service, monkeypatch):
    from types import SimpleNamespace
    from launchkit import remote_deployment
    taken = uuid.uuid4()
    existing = service.root / service.guide.identity / str(taken)
    existing.mkdir(parents=True, mode=0o700)
    issued = iter([taken])
    monkeypatch.setattr(remote_deployment, "uuid", SimpleNamespace(UUID=uuid.UUID,
        uuid4=lambda: next(issued, None) or uuid.uuid4()))
    with pytest.raises(FileExistsError):
        await service.connect(service.guide.identity, 4, service.implementation.identity,
            "owned.example", "test", 22, "~/src/team")
    assert existing.is_dir() and not any(existing.iterdir())
    assert installation_entries(service)[1] == []


def test_a_staging_claim_whose_name_was_swept_before_its_lock_is_refused(tmp_path, monkeypatch):
    import fcntl
    from launchkit import remote_deployment
    root = tmp_path / ".connecting"
    root.mkdir(mode=0o700)
    identity = str(uuid.uuid4())
    lock_path, staging = root / (identity + ".lock"), root / identity
    real_flock, interleaved = fcntl.flock, []
    def sweep_first(descriptor, operation):
        if not interleaved:
            interleaved.append(True)
            # Another process's sweep claims the new name between this open and
            # this lock, finds no staging copy and unlinks the lock file.
            claim = remote_deployment._staging_lock(lock_path)
            assert claim is not None
            remote_deployment._end_staging(staging, claim)
        return real_flock(descriptor, operation)
    monkeypatch.setattr(remote_deployment.fcntl, "flock", sweep_first)
    claim = remote_deployment._staging_lock(lock_path)
    monkeypatch.setattr(remote_deployment.fcntl, "flock", real_flock)
    try:
        assert interleaved and claim is None
    finally:
        if claim is not None:
            os.close(claim)
