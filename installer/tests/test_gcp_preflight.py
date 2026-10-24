"""Cloud-free evidence of package, principal, bounded observation and custody."""
import asyncio
from copy import deepcopy
import hashlib
import io
import json
import os
from pathlib import Path
import signal
import sys
import time
from urllib.parse import parse_qs, urlsplit
import uuid
import zipfile

import httpx
import pytest

from launchkit.artifact_binding import proposal_hash
from launchkit.blueprint import starter_blueprint
from launchkit.compiler import generate_package
from launchkit.gcp_preflight import (CommandResult, GcloudReadOnly, GoogleReadOnlyRest,
    GcpPreflightService, IAP_PERMISSION, MAX_ERROR, MAX_OUTPUT, PERMISSIONS,
    ProbeUnavailable, RestResult, _environment)
from launchkit.projects import DraftConflict, fingerprint
from test_remote_deployment import longest_loop_gap

ACCOUNT = "operator@example.com"
OTHER = "another@example.com"
PROJECT = "mobkit-preflight-project"
NUMBER = "123456789012"
ZONE = "europe-west1-b"
BASE = "https://compute.googleapis.com/compute/v1/projects/" + PROJECT


class Local:
    def __init__(self):
        proposal = starter_blueprint().model_dump(mode="json")
        proposal.update(target="gcp", gcp={"project": PROJECT, "region": "europe-west1", "zone": ZONE,
            "machine_type": "e2-standard-2", "disk_gb": 30})
        self.state = {"id": str(uuid.uuid4()), "version": 7, "proposal": proposal, "brief": {"outcome": "Useful operations"}}
        self.implementation = self
        self.build_id = str(uuid.uuid4())
        self.digest = hashlib.sha256(b"independently accepted source").hexdigest()
        self.accepted = True
        self.files = {item.path: item.content.encode() for item in generate_package(proposal)}
        self.files["implementation-report.json"] = json.dumps({"build_id": self.build_id,
            "guide_version": self.state["version"], "blueprint_hash": proposal_hash(proposal),
            "brief_hash": fingerprint(self.state["brief"]), "verification": {"digest": self.digest}}).encode()
        self.package_bytes = self.archive()

    def archive(self):
        output = io.BytesIO()
        with zipfile.ZipFile(output, "w") as archive:
            for name, value in self.files.items():
                archive.writestr(name, value)
        return output.getvalue()

    async def _guide(self, identity):
        assert identity == self.state["id"]
        return deepcopy(self.state)

    async def status(self, identity):
        return {"guide_id": identity, "guide_version": 7, "current_guide_version": self.state["version"],
            "build_id": self.build_id, "source_digest": self.digest, "current": True,
            "ready_to_package": True, "acceptance_current": self.accepted, "acceptance_trusted": self.accepted,
            "acceptance": {"status": "pass"} if self.accepted else None}

    async def package(self, identity, build_id):
        assert identity == self.state["id"] and build_id == self.build_id
        return self.package_bytes, "atlas-operations"


class Cli:
    available = True
    cleanup_pending = False
    def __init__(self):
        self.calls = []
        self.profile = {}
        self.accounts = [{"account": ACCOUNT, "status": "ACTIVE"}, {"account": OTHER, "status": ""}]
        self.failure = None
        self.wait = None
        self.entered = asyncio.Event()
        self.cleaned = asyncio.Event()

    async def command(self, args, *, secret=False):
        self.calls.append((args, secret))
        self.entered.set()
        if self.wait is not None:
            try:
                await self.wait.wait()
            finally:
                self.cleaned.set()
        if self.failure:
            return self.failure
        if args[:2] == ["config", "list"]:
            value = self.profile
        elif args[:2] == ["auth", "list"]:
            value = self.accounts
        elif args[:2] == ["auth", "print-access-token"]:
            assert secret
            return CommandResult(0, "transient-private-token")
        else:
            raise AssertionError("Unexpected or mutating CLI command: " + repr(args))
        return CommandResult(0, json.dumps(value))


class Cloud:
    def __init__(self):
        self.calls = []
        self.overrides = {}
        self.wait = None
        self.entered = asyncio.Event()

    async def request(self, method, url, body, token):
        assert token == "transient-private-token"
        self.calls.append((method, url, body, token))
        path = urlsplit(url).path
        self.entered.set()
        if self.wait:
            await self.wait.wait()
        if path in self.overrides:
            return self.overrides[path]
        if path == "/v1/userinfo":
            value = {"email": ACCOUNT, "email_verified": True, "sub": "private-other-identity-field"}
        elif path == "/v1/projects":
            value = {"projects": [{"projectId": PROJECT, "name": "Operations project", "lifecycleState": "ACTIVE"}]}
        elif path == "/v1/projects/" + PROJECT:
            value = {"projectId": PROJECT, "projectNumber": NUMBER, "lifecycleState": "ACTIVE"}
        elif path.endswith("/billingInfo"):
            value = {"projectId": PROJECT, "billingEnabled": True, "billingAccountName": "private-billing-account"}
        elif path.endswith("/services:batchGet"):
            value = {"services": [{"name": "projects/" + NUMBER + "/services/" + name, "state": "ENABLED"} for name in ("compute.googleapis.com", "iap.googleapis.com")]}
        elif path.endswith("/iap_tunnel:testIamPermissions"):
            value = {"permissions": [IAP_PERMISSION]}
        elif path.endswith(":testIamPermissions"):
            value = {"permissions": list(PERMISSIONS)}
        elif path.endswith("/machineTypes/e2-standard-2"):
            value = {"name": "e2-standard-2", "zone": ZONE, "guestCpus": 2, "memoryMb": 8192}
        elif path.endswith("/zones/" + ZONE):
            value = {"name": ZONE, "region": BASE + "/regions/europe-west1", "status": "UP"}
        elif path.endswith("/global/networks/default"):
            value = {"name": "default", "selfLink": BASE + "/global/networks/default", "autoCreateSubnetworks": True,
                "subnetworks": [BASE + "/regions/europe-west1/subnetworks/default"]}
        elif path.endswith("/instances"):
            value = {"kind": "compute#instanceList", "selfLink": BASE + "/zones/" + ZONE + "/instances"}
        elif path.endswith("/global/firewalls"):
            value = {"kind": "compute#firewallList", "selfLink": BASE + "/global/firewalls"}
        elif path.endswith("/regions/europe-west1"):
            value = {"name": "europe-west1", "quotas": [{"metric": metric, "limit": limit, "usage": 1}
                for metric, limit in (("CPUS", 100), ("INSTANCES", 10), ("SSD_TOTAL_GB", 1000), ("DISKS_TOTAL_GB", 0), ("IN_USE_ADDRESSES", 10))]}
            # Standard PD quota is irrelevant to this pd-balanced plan.
            value["quotas"][3]["usage"] = 0
        elif path.endswith("compute.vmExternalIpAccess:getEffectivePolicy"):
            value = {"name": "projects/" + NUMBER + "/policies/compute.vmExternalIpAccess", "spec": {"rules": [{"allowAll": True}]}}
        elif path.endswith("compute.requireOsLogin:getEffectivePolicy"):
            value = {"name": "projects/" + NUMBER + "/policies/compute.requireOsLogin", "spec": {"rules": [{"enforce": False}]}}
        elif path == "/compute/v1/projects/" + PROJECT:
            value = {"name": PROJECT, "commonInstanceMetadata": {"items": []}, "quotas": [{"metric": "FIREWALLS", "limit": 100, "usage": 1}]}
        else:
            raise AssertionError("Unexpected cloud observation: " + repr((method, url, body)))
        return RestResult(200, value)


@pytest.fixture
def harness():
    local, cli, cloud = Local(), Cli(), Cloud()
    service = GcpPreflightService(local, command=cli, rest=cloud)
    return service, local, cli, cloud


async def checked(harness, **changes):
    service, local, _, _ = harness
    return await service.check(local.state["id"], local.state["version"], local.build_id,
        changes.get("account", ACCOUNT), changes.get("project", PROJECT))


def row(result, identity):
    return next(item for item in result["checks"] if item["id"] == identity)


async def test_supported_package_plan_binds_all_bytes_destination_and_verified_token(harness):
    result = await checked(harness)
    service, local, cli, cloud = harness
    assert result["protocol"] == 1 and result["ready_for_plan"] is True
    assert result["provisioning_verified"] is False
    assert result["binding"] == {"guide_id": local.state["id"], "guide_version": 7, "build_id": local.build_id,
        "blueprint_hash": proposal_hash(local.state["proposal"]), "source_digest": local.digest,
        "package_sha256": hashlib.sha256(local.package_bytes).hexdigest()}
    assert result["plan"]["destination"] == result["destination"]
    assert result["destination"] == {"project": PROJECT, "zone": ZONE, "instance": "atlas-operations",
        "machine_type": "e2-standard-2", "disk_gb": 30, "console_port": local.state["proposal"]["advanced"]["port"]}
    assert all("--account=" + ACCOUNT in call for call, _ in cli.calls)
    assert all(call[0] == "auth" or call[:2] == ["config", "list"] for call, _ in cli.calls)
    assert all(method == "GET" or url.endswith(":testIamPermissions") for method, url, _, _ in cloud.calls)
    assert all(token == "transient-private-token" for _, _, _, token in cloud.calls)
    assert row(result, "live-capacity")["required"] is False
    assert row(result, "network-access")["status"] == "unknown"
    assert "external IPv4" in str(result["plan"]) and "No VM service account" in str(result["plan"])
    assert "env -i" in result["plan"]["commands"][0] and "CLOUDSDK_CORE_ACCOUNT=" + ACCOUNT in result["plan"]["commands"][0]
    assert not any(value in json.dumps(result) for value in ("transient-private-token", "private-billing-account", "private-other-identity-field"))
    assert service._active is None


async def test_options_positive_empty_accounts_is_distinct_from_unavailable_listing(harness):
    service, local, cli, _ = harness
    cli.accounts = []
    result = await service.options(local.state["id"], local.build_id)
    assert result["accounts"] == [] and row(result, "account")["status"] == "blocked"
    assert row(result, "account-binding")["status"] == "checked"
    assert cli.calls[0][0][:3] == ["config", "list", "--format=json"]
    cli.failure = CommandResult(1, "private-error-canary")
    unknown = await service.options(local.state["id"], local.build_id)
    assert unknown["accounts"] == [] and row(unknown, "cli")["status"] == "unknown"
    assert "private-error-canary" not in str(unknown)


async def test_default_configuration_uses_complete_object_instead_of_unset_projection(harness):
    service, local, cli, cloud = harness
    cli.profile = {"core": {"account": ACCOUNT, "project": PROJECT},
        "compute": {"region": "europe-west1"}, "proxy": {"password": "private-config-canary"}}
    original = cli.command
    async def observed_default(args, *, secret=False):
        result = await original(args, secret=secret)
        if "--format=json(auth,core.disable_ssl_validation,api_endpoint_overrides)" in args:
            return CommandResult(0, "null\n")
        return result
    cli.command = observed_default
    result = await service.options(local.state["id"], local.build_id)
    assert result["accounts"] == [{"id": ACCOUNT, "label": ACCOUNT, "active": True},
        {"id": OTHER, "label": OTHER, "active": False}]
    assert row(result, "account-binding")["status"] == "checked"
    assert cli.calls[0][0] == ["config", "list", "--format=json", "--quiet", "--verbosity=error", "--no-log-http"]
    assert "private-config-canary" not in json.dumps(result)
    assert cloud.calls == []


@pytest.mark.parametrize("profile", [None, [], "private-malformed-config", False, 0,
    *({section: value} for section in ("auth", "core", "api_endpoint_overrides")
      for value in (None, [], "private-malformed-section", False, 0))])
async def test_malformed_complete_configuration_remains_unknown_and_private(harness, profile):
    service, local, cli, cloud = harness
    cli.profile = profile
    result = await service.options(local.state["id"], local.build_id)
    assert result["accounts"] == [] and row(result, "cli")["status"] == "unknown"
    assert len(cli.calls) == 1 and cli.calls[0][0][:3] == ["config", "list", "--format=json"]
    assert "private-malformed" not in json.dumps(result)
    assert cloud.calls == []


@pytest.mark.parametrize("profile", [{}, {"auth": {}, "core": {}, "api_endpoint_overrides": {}},
    {"auth": {"disable_credentials": False, "disable_ssl_validation": "false"},
     "core": {"disable_ssl_validation": "False"}, "api_endpoint_overrides": {}}])
async def test_explicit_empty_or_false_override_configuration_keeps_account_choices(harness, profile):
    service, local, cli, cloud = harness
    cli.profile = profile
    result = await service.options(local.state["id"], local.build_id)
    assert len(result["accounts"]) == 2 and row(result, "account-binding")["status"] == "checked"
    assert cloud.calls == []


@pytest.mark.parametrize("key", ["impersonate_service_account", "credential_file_override", "access_token_file", "access_token"])
@pytest.mark.parametrize("value", ["false", "False", "private-credential-value"])
async def test_nonempty_credential_strings_are_overrides_even_when_spelled_false(harness, key, value):
    service, local, cli, cloud = harness
    cli.profile = {"auth": {key: value}}
    result = await service.options(local.state["id"], local.build_id)
    assert result["accounts"] == [] and row(result, "account-binding")["status"] == "blocked"
    assert len(cli.calls) == 1 and cloud.calls == []
    assert "private-credential-value" not in json.dumps(result)


@pytest.mark.parametrize("key", ["impersonate_service_account", "credential_file_override", "access_token_file", "access_token"])
@pytest.mark.parametrize("value", [None, False, True, 0, 0.0, 1, [], {"private-leaf-canary": "value"}])
async def test_present_nonstring_credentials_remain_unknown_and_private(harness, key, value):
    service, local, cli, cloud = harness
    cli.profile = {"auth": {key: value}}
    result = await service.options(local.state["id"], local.build_id)
    assert result["accounts"] == [] and row(result, "cli")["status"] == "unknown"
    assert len(cli.calls) == 1 and cloud.calls == []
    assert "private-leaf-canary" not in json.dumps(result)


@pytest.mark.parametrize("key", ["impersonate_service_account", "credential_file_override", "access_token_file", "access_token"])
async def test_empty_credential_strings_keep_account_choices(harness, key):
    service, local, cli, cloud = harness
    cli.profile = {"auth": {key: ""}}
    result = await service.options(local.state["id"], local.build_id)
    assert len(result["accounts"]) == 2 and row(result, "account-binding")["status"] == "checked"
    assert len(cli.calls) == 2 and cloud.calls == []


@pytest.mark.parametrize("section,key", [("auth", "disable_credentials"), ("auth", "disable_ssl_validation"), ("core", "disable_ssl_validation")])
@pytest.mark.parametrize("value", [False, "false", "False"])
async def test_recognized_false_boolean_flags_keep_account_choices(harness, section, key, value):
    service, local, cli, cloud = harness
    cli.profile = {section: {key: value}}
    result = await service.options(local.state["id"], local.build_id)
    assert len(result["accounts"]) == 2 and row(result, "account-binding")["status"] == "checked"
    assert len(cli.calls) == 2 and cloud.calls == []


@pytest.mark.parametrize("section,key", [("auth", "disable_credentials"), ("auth", "disable_ssl_validation"), ("core", "disable_ssl_validation")])
@pytest.mark.parametrize("value", [True, "true", "True"])
async def test_recognized_true_boolean_flags_block_account_binding(harness, section, key, value):
    service, local, cli, cloud = harness
    cli.profile = {section: {key: value}}
    result = await service.options(local.state["id"], local.build_id)
    assert result["accounts"] == [] and row(result, "account-binding")["status"] == "blocked"
    assert len(cli.calls) == 1 and cloud.calls == []


@pytest.mark.parametrize("section,key", [("auth", "disable_credentials"), ("auth", "disable_ssl_validation"), ("core", "disable_ssl_validation")])
@pytest.mark.parametrize("value", [None, 0, 0.0, 1, [], {"private-leaf-canary": "value"}, "", "private-unsupported-flag"])
async def test_unrecognized_boolean_leaves_remain_unknown_and_private(harness, section, key, value):
    service, local, cli, cloud = harness
    cli.profile = {section: {key: value}}
    result = await service.options(local.state["id"], local.build_id)
    assert result["accounts"] == [] and row(result, "cli")["status"] == "unknown"
    assert len(cli.calls) == 1 and cloud.calls == []
    assert not any(value in json.dumps(result) for value in ("private-leaf-canary", "private-unsupported-flag"))


async def test_cli_installed_after_launch_start_needs_restart_guidance(harness, tmp_path, monkeypatch):
    service, local, _, cloud = harness
    monkeypatch.setattr("launchkit.gcp_preflight.shutil.which", lambda *args, **kwargs: None)
    monkeypatch.setattr(Path, "home", lambda: tmp_path)
    service.command = GcloudReadOnly()
    missing = await service.options(local.state["id"], local.build_id)

    # Discovery is startup-bound, including the server's cloud transport wiring.
    # A page retry after installation must retain an actionable restart step.
    monkeypatch.setattr("launchkit.gcp_preflight.shutil.which", lambda *args, **kwargs: str(tmp_path / "gcloud"))
    assert GcloudReadOnly().available is True
    retried = await service.options(local.state["id"], local.build_id)
    for result in (missing, retried):
        assert result["cli"]["available"] is False and result["accounts"] == []
        assert "when Forge started" in result["cli"]["detail"]
        assert "restart Forge and reopen this saved setup" in row(result, "cli")["action"]
    assert cloud.calls == []


async def test_project_list_is_bounded_and_manual_exact_project_is_independently_checked(harness):
    service, local, _, cloud = harness
    cloud.overrides["/v1/projects"] = RestResult(200, {"projects": [{"projectId": "project-" + str(i).zfill(4), "name": "Project " + str(i), "lifecycleState": "ACTIVE"} for i in range(100)], "nextPageToken": "private-next-page-token"})
    result = await service.projects(local.state["id"], 7, local.build_id, ACCOUNT)
    assert len(result["projects"]) == 100 and result["truncated"] is True
    assert "private-next-page-token" not in str(result)
    assert (await checked(harness))["ready_for_plan"] is True


async def test_service_usage_projects_only_exact_names_and_states_within_existing_bounds(harness):
    cloud = harness[3]
    original = cloud.request
    async def projected_only(method, url, body, token):
        if urlsplit(url).path == "/v1/projects/" + NUMBER + "/services:batchGet":
            assert method == "GET" and body is None and token == "transient-private-token"
            if parse_qs(urlsplit(url).query).get("fields") != ["services(name,state)"]:
                raise ProbeUnavailable("private-unfiltered-service-details-exceed-bounds")
        return await original(method, url, body, token)
    cloud.request = projected_only
    result = await checked(harness)
    assert row(result, "apis")["status"] == "checked" and result["ready_for_plan"] is True
    service_calls = [call for call in cloud.calls if urlsplit(call[1]).path.endswith("/services:batchGet")]
    assert len(service_calls) == 1
    assert parse_qs(urlsplit(service_calls[0][1]).query) == {
        "names": ["projects/" + NUMBER + "/services/compute.googleapis.com",
                  "projects/" + NUMBER + "/services/iap.googleapis.com"],
        "fields": ["services(name,state)"],
    }
    assert not any("fields=" in url for _, url, _, _ in cloud.calls if not urlsplit(url).path.endswith("/services:batchGet"))
    assert "private-unfiltered" not in json.dumps(result)


@pytest.mark.parametrize("services,status", [
    ([{"name": "projects/" + NUMBER + "/services/compute.googleapis.com", "state": "ENABLED"}] * 2, "unknown"),
    ([{"name": "projects/other/services/compute.googleapis.com", "state": "ENABLED"},
      {"name": "projects/" + NUMBER + "/services/iap.googleapis.com", "state": "ENABLED"}], "unknown"),
    ([None, {"name": "projects/" + NUMBER + "/services/iap.googleapis.com", "state": "ENABLED"}], "unknown"),
    ([{"name": "projects/" + NUMBER + "/services/compute.googleapis.com", "state": "enabled"},
      {"name": "projects/" + NUMBER + "/services/iap.googleapis.com", "state": "ENABLED"}], "unknown"),
    ([{"name": "projects/" + NUMBER + "/services/compute.googleapis.com", "state": "DISABLED"},
      {"name": "projects/" + NUMBER + "/services/iap.googleapis.com", "state": "ENABLED"}], "blocked"),
])
async def test_projected_service_usage_keeps_exact_row_name_and_state_checks(harness, services, status):
    harness[3].overrides["/v1/projects/" + NUMBER + "/services:batchGet"] = RestResult(200, {"services": services})
    result = await checked(harness)
    assert row(result, "apis")["status"] == status
    assert result["ready_for_plan"] is False and result["plan"] is None


@pytest.mark.parametrize("path,override,identity,status", [
    ("/v1/userinfo", RestResult(200, {"email": OTHER, "email_verified": True}), "effective-account", "blocked"),
    ("/v1/userinfo", RestResult(200, {"email": ACCOUNT, "email_verified": False}), "effective-account", "unknown"),
    ("/v1/userinfo", RestResult(403, None), "effective-account", "unknown"),
    ("/v1/projects/" + PROJECT + "/billingInfo", RestResult(200, {"projectId": PROJECT, "billingEnabled": False}), "billing", "blocked"),
    ("/v1/projects/" + PROJECT + "/billingInfo", RestResult(403, {"billingEnabled": False}), "billing", "unknown"),
    ("/v1/projects/" + PROJECT + "/billingInfo", RestResult(200, {"projectId": PROJECT, "billingEnabled": "true"}), "billing", "unknown"),
    ("/v1/projects/" + NUMBER + "/services:batchGet", RestResult(200, {"services": []}), "apis", "unknown"),
    ("/v1/projects/" + NUMBER + "/iap_tunnel:testIamPermissions", RestResult(200, {}), "iap-permission", "unknown"),
    ("/v1/projects/" + PROJECT + ":testIamPermissions", RestResult(200, {"permissions": [v for v in PERMISSIONS if v != "serviceusage.services.enable"]}), "permissions", "blocked"),
    ("/v2/projects/" + NUMBER + "/policies/compute.vmExternalIpAccess:getEffectivePolicy", RestResult(404, None), "policy-vmExternalIpAccess", "unknown"),
    ("/v2/projects/" + NUMBER + "/policies/compute.vmExternalIpAccess:getEffectivePolicy", RestResult(200, {"name": "projects/" + NUMBER + "/policies/compute.vmExternalIpAccess", "spec": {"rules": [{"denyAll": True}]}}), "policy-vmExternalIpAccess", "blocked"),
])
async def test_unknown_and_denied_preconditions_never_become_readiness(harness, path, override, identity, status):
    harness[3].overrides[path] = override
    result = await checked(harness)
    assert row(result, identity)["status"] == status
    assert result["ready_for_plan"] is False and result["plan"] is None


@pytest.mark.parametrize("permission", ["compute.networks.updatePolicy", "compute.firewalls.get"])
async def test_firewall_setup_checks_network_policy_and_recovery_read_permissions(harness, permission):
    # The firewall insert also authorizes its network field. Being able to
    # create firewall resources and read the VPC does not prove that right,
    # or the rule read needed when resuming after VM creation failed.
    # https://docs.cloud.google.com/compute/docs/reference/rest/v1/firewalls/insert
    # https://docs.cloud.google.com/compute/docs/reference/rest/v1/firewalls/get
    cloud = harness[3]
    path = "/v1/projects/" + PROJECT + ":testIamPermissions"
    cloud.overrides[path] = RestResult(200, {
        "permissions": [value for value in PERMISSIONS if value != permission],
    })
    result = await checked(harness)
    requests = [body for _, url, body, _ in cloud.calls if urlsplit(url).path == path]
    assert any("compute.firewalls.create" in body["permissions"]
        and permission in body["permissions"] for body in requests)
    assert "compute.firewalls.create" in cloud.overrides[path].value["permissions"]
    assert row(result, "network")["status"] == "checked"
    assert row(result, "permissions")["status"] == "blocked"
    assert permission in row(result, "permissions")["detail"]
    assert result["ready_for_plan"] is False and result["plan"] is None


@pytest.mark.parametrize("override,status", [
    (RestResult(403, {"kind": "compute#instanceList", "items": []}), "unknown"),
    (RestResult(200, {}), "unknown"),
    (RestResult(200, {"kind": "compute#instanceList", "selfLink": BASE + "/zones/other-zone/instances"}), "unknown"),
    (RestResult(200, {"kind": "compute#wrongList", "selfLink": BASE + "/zones/" + ZONE + "/instances"}), "unknown"),
    (RestResult(200, {"kind": "compute#instanceList", "selfLink": BASE + "/zones/" + ZONE + "/instances", "warning": {"code": "UNREACHABLE"}}), "unknown"),
    (RestResult(200, {"kind": "compute#instanceList", "selfLink": BASE + "/zones/" + ZONE + "/instances", "nextPageToken": "more"}), "unknown"),
    (RestResult(200, {"kind": "compute#instanceList", "selfLink": BASE + "/zones/" + ZONE + "/instances", "items": [{"name": "atlas-operations"}]}), "blocked"),
    (RestResult(200, {"kind": "compute#instanceList", "selfLink": BASE + "/zones/" + ZONE + "/instances", "warning": {"code": "NO_RESULTS_ON_PAGE"}}), "checked"),
])
async def test_absence_requires_successful_exact_scope_complete_collection(harness, override, status):
    harness[3].overrides["/compute/v1/projects/" + PROJECT + "/zones/" + ZONE + "/instances"] = override
    result = await checked(harness)
    assert row(result, "instance-collision")["status"] == status
    assert result["ready_for_plan"] is (status == "checked")


async def test_balanced_disk_checks_ssd_quota_even_when_standard_disk_headroom_is_large(harness):
    harness[3].overrides["/compute/v1/projects/" + PROJECT + "/regions/europe-west1"] = RestResult(200, {"name": "europe-west1", "quotas": [{"metric": metric, "limit": limit, "usage": 0} for metric, limit in (("CPUS", 100), ("INSTANCES", 100), ("DISKS_TOTAL_GB", 10000), ("SSD_TOTAL_GB", 0), ("IN_USE_ADDRESSES", 100))]})
    result = await checked(harness)
    assert row(result, "regional-quota")["status"] == "blocked" and "SSD_TOTAL_GB" in row(result, "regional-quota")["detail"]
    assert result["plan"] is None


@pytest.mark.parametrize("changed", ["script", "tunnel", "remote-helper", "definition", "source-digest", "brief", "unaccepted"])
async def test_package_cannot_borrow_fixed_cloud_plan_after_source_or_scope_change(harness, changed):
    _, local, cli, cloud = harness
    if changed in {"script", "tunnel", "remote-helper"}:
        name = {"script": "deploy/deploy-gcp.sh", "tunnel": "deploy/tunnel-gcp.sh", "remote-helper": "remote_operations.py"}[changed]
        local.files[name] += b"\n# accepted custom behavior\n"
        local.package_bytes = local.archive()
        result = await checked(harness)
        assert row(result, "package-deployer")["status"] == "blocked" and result["plan"] is None
        assert cli.calls == [] and cloud.calls == []
    else:
        if changed == "definition":
            value = json.loads(local.files["deployment.json"])
            value["port"] += 1
            local.files["deployment.json"] = json.dumps(value).encode()
            local.package_bytes = local.archive()
        elif changed == "source-digest": local.digest = "f" * 64
        elif changed == "brief": local.state["brief"]["outcome"] = "Completely different application"
        else: local.accepted = False
        with pytest.raises((ValueError, DraftConflict)):
            await checked(harness)
        assert cli.calls == [] and cloud.calls == []


def replace_package_evidence(local, location, value):
    name = "deployment.json" if location in {"definition", "knowledge"} else "implementation-report.json"
    if location in {"verification", "knowledge"}:
        document = json.loads(local.files[name])
        document[location] = value
        value = document
    local.files[name] = json.dumps(value).encode()
    local.package_bytes = local.archive()


@pytest.mark.parametrize("location", ["definition", "report", "verification"])
@pytest.mark.parametrize("value", [None, [], "private-malformed-evidence", False, 7, 1.5])
async def test_nonobject_package_evidence_refuses_before_probe_and_allows_repair(harness, location, value):
    service, local, cli, cloud = harness
    accepted_package = local.package_bytes
    replace_package_evidence(local, location, value)
    with pytest.raises(DraftConflict, match=r"^The accepted cloud package has invalid deployment evidence\.$"):
        await checked(harness)
    assert cli.calls == [] and cloud.calls == []
    assert service._active is None

    # Restore the same accepted bytes, then use the public operation again.
    local.package_bytes = accepted_package
    result = await checked(harness)
    assert result["ready_for_plan"] is True and result["provisioning_verified"] is False
    assert result["binding"]["package_sha256"] == hashlib.sha256(accepted_package).hexdigest()
    assert service._active is None


@pytest.mark.parametrize("location", ["definition", "report", "verification"])
async def test_package_evidence_becoming_nonobject_during_probe_refuses_result(harness, location):
    service, local, cli, cloud = harness
    accepted_package = local.package_bytes
    request = cloud.request
    async def changing(method, url, body, token):
        result = await request(method, url, body, token)
        if urlsplit(url).path == "/v1/userinfo":
            replace_package_evidence(local, location, None)
        return result
    cloud.request = changing
    with pytest.raises(DraftConflict, match=r"^The accepted cloud package has invalid deployment evidence\.$"):
        await checked(harness)
    assert cli.calls and cloud.calls
    assert service._active is None

    cloud.request = request
    local.package_bytes = accepted_package
    result = await checked(harness)
    assert result["ready_for_plan"] is True and result["provisioning_verified"] is False
    assert result["binding"]["package_sha256"] == hashlib.sha256(accepted_package).hexdigest()
    assert service._active is None


@pytest.mark.parametrize("missing", [False, True])
async def test_empty_or_missing_package_verification_keeps_existing_mismatch_refusal(harness, missing):
    service, local, cli, cloud = harness
    report = json.loads(local.files["implementation-report.json"])
    if missing:
        del report["verification"]
    else:
        report["verification"] = {}
    replace_package_evidence(local, "report", report)
    with pytest.raises(DraftConflict, match=r"^The accepted package differs from the current proposal or checked source\. Rebuild before cloud preparation\.$"):
        await checked(harness)
    assert cli.calls == [] and cloud.calls == []
    assert service._active is None


@pytest.mark.parametrize("guided", [False, True])
@pytest.mark.parametrize("value", [False, [], ["local"], "private-knowledge-shape", 0, 1.5, True])
async def test_nonobject_knowledge_refuses_before_cloud_probe_and_allows_repair(harness, guided, value):
    service, local, cli, cloud = harness
    accepted_package = local.package_bytes
    replace_package_evidence(local, "knowledge", value)
    check = service.check_guided if guided else service.check
    with pytest.raises(DraftConflict, match=r"^The accepted cloud package has invalid deployment evidence\.$"):
        await check(local.state["id"], local.state["version"], local.build_id, ACCOUNT, PROJECT)
    assert cli.calls == [] and cloud.calls == []
    assert service._active is None
    local.package_bytes = accepted_package
    result = await checked(harness)
    assert result["ready_for_plan"] is True and result["provisioning_verified"] is False
    assert result["binding"]["package_sha256"] == hashlib.sha256(accepted_package).hexdigest()
    assert service._active is None


@pytest.mark.parametrize("value", ["missing", None, {}])
async def test_legacy_optional_knowledge_keeps_the_base_cloud_plan(harness, value):
    service, local, _, _ = harness
    if value == "missing":
        definition = json.loads(local.files["deployment.json"])
        del definition["knowledge"]
        replace_package_evidence(local, "definition", definition)
    else:
        replace_package_evidence(local, "knowledge", value)
    result = await checked(harness)
    assert result["ready_for_plan"] is True and result["provisioning_verified"] is False
    assert result["binding"]["package_sha256"] == hashlib.sha256(local.package_bytes).hexdigest()
    assert service._active is None


@pytest.mark.parametrize("value", [["local"], "private-knowledge-shape", 1.5])
async def test_knowledge_becoming_nonobject_during_probe_refuses_the_plan(harness, value):
    service, local, cli, cloud = harness
    accepted_package = local.package_bytes
    request = cloud.request
    async def changing(method, url, body, token):
        result = await request(method, url, body, token)
        if urlsplit(url).path == "/v1/userinfo":
            replace_package_evidence(local, "knowledge", value)
        return result
    cloud.request = changing
    with pytest.raises(DraftConflict, match=r"^The accepted cloud package has invalid deployment evidence\.$"):
        await checked(harness)
    assert cli.calls and cloud.calls
    assert service._active is None
    cloud.request = request
    local.package_bytes = accepted_package
    result = await checked(harness)
    assert result["ready_for_plan"] is True and result["provisioning_verified"] is False
    assert result["binding"]["package_sha256"] == hashlib.sha256(accepted_package).hexdigest()
    assert service._active is None


@pytest.mark.parametrize("location", ["report", "acceptance"])
async def test_malformed_package_brief_binding_refuses_before_cloud_observation(harness, location):
    _, local, cli, cloud = harness
    report = json.loads(local.files["implementation-report.json"])
    if location == "report":
        report["brief_hash"] = "malformed"
    else:
        del report["brief_hash"]
        report["acceptance"] = {"brief_digest": "malformed"}
    local.files["implementation-report.json"] = json.dumps(report).encode()
    local.package_bytes = local.archive()
    with pytest.raises(DraftConflict, match="^The accepted package differs from the current proposal or checked source\\."):
        await checked(harness)
    assert cli.calls == [] and cloud.calls == []


@pytest.mark.parametrize("tampered", [False, True])
async def test_package_accepted_by_forge_094_keeps_its_destination_helper(harness, tampered):
    # Forge 0.9.4 rendered this exact destination helper into the packages it
    # accepted. It is recognized by its digest and never rewritten.
    from launchkit import gcp_preflight
    _, local, cli, cloud = harness
    retained = (Path(__file__).parent / "fixtures/retained_destination_helper/remote_operations_094.py").read_bytes()
    assert hashlib.sha256(retained).hexdigest() in gcp_preflight.RETAINED_DESTINATION_HELPER_SHA256
    local.files["remote_operations.py"] = retained + (b"\n# unrelated host modification\n" if tampered else b"")
    local.package_bytes = local.archive()
    result = await checked(harness)
    assert row(result, "package-deployer")["status"] == ("blocked" if tampered else "checked")


@pytest.mark.parametrize('retained', [False, True])
@pytest.mark.parametrize('tampered', [False, True])
async def test_selected_knowledge_export_matches_fixed_cloud_adapter_including_retained_packages(harness, monkeypatch, retained, tampered):
    from launchkit import gcp_bootstrap
    _, local, cli, cloud = harness
    proposal = local.state['proposal']
    proposal['knowledge']['mode'] = 'local'
    proposal['gcp'].update(machine_type='e2-standard-8', disk_gb=100)
    cloud.overrides['/compute/v1/projects/' + PROJECT + '/zones/' + ZONE + '/machineTypes/e2-standard-8'] = RestResult(
        200, {'name': 'e2-standard-8', 'zone': ZONE, 'guestCpus': 8, 'memoryMb': 32768})
    with monkeypatch.context() as patch:
        if retained:
            patch.setattr(gcp_bootstrap, 'render_manual_prerequisites', lambda deploy, definition: deploy)
        local.files = {item.path: item.content.encode() for item in generate_package(proposal)}
    local.files['implementation-report.json'] = json.dumps({'build_id': local.build_id,
        'guide_version': local.state['version'], 'blueprint_hash': proposal_hash(proposal),
        'brief_hash': fingerprint(local.state['brief']), 'verification': {'digest': local.digest}}).encode()
    if tampered:
        local.files['deploy/deploy-gcp.sh'] += b'\n# unrelated host modification\n'
    local.package_bytes = local.archive()
    result = await checked(harness)
    assert row(result, 'package-deployer')['status'] == ('blocked' if tampered else 'checked')
    if tampered:
        assert result['plan'] is None and cli.calls == [] and cloud.calls == []
    else:
        assert result['plan'] is not None


async def test_selected_project_cannot_redirect_accepted_package(harness):
    result = await checked(harness, project="another-cloud-project")
    assert result["destination"]["project"] == PROJECT
    assert row(result, "project-match")["status"] == "blocked" and result["plan"] is None
    assert harness[2].calls == [] and harness[3].calls == []


@pytest.mark.parametrize("profile", [{"auth": {"impersonate_service_account": "private-principal"}},
    {"auth": {"access_token_file": "/private/token-path"}}, {"auth": {"credential_file_override": "/private/credentials"}},
    {"auth": {"access_token": "private-access-token"}}, {"auth": {"disable_credentials": True}},
    {"auth": {"disable_ssl_validation": True}}, {"auth": {"disable_ssl_validation": "true"}},
    {"api_endpoint_overrides": {"compute": "private-endpoint"}}, {"core": {"disable_ssl_validation": "true"}}])
async def test_config_credential_overrides_do_not_assert_selected_account(harness, profile):
    harness[2].profile = profile
    result = await checked(harness)
    assert row(result, "account-binding")["status"] == "blocked" and result["plan"] is None
    assert harness[3].calls == []
    assert not any(value in str(result) for value in ("private-principal", "/private/token-path", "/private/credentials", "private-access-token", "private-endpoint"))


@pytest.mark.parametrize("mutation", ["version", "build", "source", "account-reversal", "project-reversal"])
async def test_stale_completion_and_selection_reversal_discard_old_result(harness, mutation):
    service, local, _, cloud = harness
    cloud.wait = asyncio.Event()
    task = asyncio.create_task(checked(harness))
    await cloud.entered.wait()
    if mutation == "version": local.state["version"] += 1
    elif mutation == "build": local.build_id = str(uuid.uuid4())
    elif mutation == "source": local.digest = "e" * 64
    else:
        first = {"account": OTHER} if mutation == "account-reversal" else {"project": "another-cloud-project"}
        with pytest.raises(DraftConflict, match="still finishing"):
            await checked(harness, **first)
        with pytest.raises(DraftConflict, match="still finishing"):
            await checked(harness)
    cloud.wait.set()
    with pytest.raises(DraftConflict):
        await task
    assert service._active is None


async def test_shutdown_awaits_owned_probe_cleanup_and_unresolved_custody_stays_admitted(harness):
    service, local, cli, _ = harness
    cli.wait = asyncio.Event()
    task = asyncio.create_task(service.options(local.state["id"], local.build_id))
    await cli.entered.wait()
    await service.shutdown()
    assert task.cancelled() and cli.cleaned.is_set() and service._active is None
    fresh = GcpPreflightService(local, command=cli, rest=harness[3])
    cli.wait = None
    cli.cleanup_pending = True
    with pytest.raises(DraftConflict, match="still finishing"):
        await fresh.options(local.state["id"], local.build_id)
    with pytest.raises(DraftConflict, match="not positively stopped"):
        await fresh.shutdown()


def fake_cli(tmp_path, source):
    path = tmp_path / "gcloud"
    path.write_text("#!" + sys.executable + "\n" + source)
    path.chmod(0o755)
    return str(path)


async def test_default_runner_normal_completion_has_no_retained_process_or_group(tmp_path):
    runner = GcloudReadOnly(fake_cli(tmp_path, "print('{}')\n"))
    assert (await runner.command(["auth", "list"])).output == "{}\n"
    assert runner.cleanup_pending is False and runner._pending_groups == set() and runner._pending_processes == {}


@pytest.mark.parametrize("source,timeout", [("import sys\nsys.stdout.write('x' * 400000)\n", 1),
    ("import sys\nsys.stderr.write('private-error' * 3000)\n", 1),
    ("import time\ntime.sleep(60)\n", 0.05)])
async def test_default_runner_bounds_stdout_stderr_timeout_and_cleans_group(tmp_path, source, timeout):
    runner = GcloudReadOnly(fake_cli(tmp_path, source), timeout=timeout)
    before = time.monotonic()
    with pytest.raises(ProbeUnavailable):
        await runner.command([])
    assert time.monotonic() - before < 5
    assert runner.cleanup_pending is False and runner._pending_processes == {}


async def test_default_runner_cancellation_cleans_exact_owned_group(tmp_path):
    pidfile = tmp_path / "owned-pid"
    source = f"import os, pathlib, time\npathlib.Path({str(pidfile)!r}).write_text(str(os.getpid()))\ntime.sleep(60)\n"
    runner = GcloudReadOnly(fake_cli(tmp_path, source))
    task = asyncio.create_task(runner.command([]))
    while not pidfile.exists(): await asyncio.sleep(0.01)
    group = int(pidfile.read_text())
    task.cancel()
    with pytest.raises(asyncio.CancelledError): await task
    with pytest.raises(ProcessLookupError): os.killpg(group, 0)
    assert runner.cleanup_pending is False


async def test_exited_parent_does_not_leave_pipe_holding_child_alive(tmp_path):
    childfile = tmp_path / "child-pid"
    source = f"import pathlib, subprocess, sys\nchild = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(60)'])\npathlib.Path({str(childfile)!r}).write_text(str(child.pid))\n"
    runner = GcloudReadOnly(fake_cli(tmp_path, source), timeout=0.1)
    with pytest.raises(ProbeUnavailable): await runner.command([])
    child = int(childfile.read_text())
    # Absence of the owned group is independent of the CLI parent's exit.
    assert runner.cleanup_pending is False and runner._pending_processes == {}
    with pytest.raises(ProcessLookupError): os.kill(child, 0)


async def test_unresolved_group_cleanup_is_bounded_and_keeps_ledger(tmp_path, monkeypatch):
    runner = GcloudReadOnly(fake_cli(tmp_path, "print('{}')\n"))
    real_killpg = os.killpg
    def unknown(group, sig):
        if sig == 0: raise PermissionError("controlled unknown absence")
        return real_killpg(group, sig)
    monkeypatch.setattr(os, "killpg", unknown)
    started = time.monotonic()
    with pytest.raises(ProbeUnavailable, match="not positively stopped"):
        await runner.command([])
    assert time.monotonic() - started < 5 and runner.cleanup_pending is True
    monkeypatch.setattr(os, "killpg", real_killpg)
    assert runner.cleanup_pending is False and runner._pending_processes == {}


def test_cli_discovery_supports_normal_path_and_environment_excludes_credential_overrides(tmp_path, monkeypatch):
    executable = fake_cli(tmp_path, "print('{}')\n")
    monkeypatch.setenv("PATH", str(tmp_path))
    monkeypatch.setenv("CLOUDSDK_AUTH_ACCESS_TOKEN", "private-token")
    monkeypatch.setenv("CLOUDSDK_API_ENDPOINT_OVERRIDES_COMPUTE", "https://private.example")
    monkeypatch.setenv("GOOGLE_APPLICATION_CREDENTIALS", "/private/key")
    runner = GcloudReadOnly()
    assert runner.executable == executable and Path(runner.executable).is_absolute()
    assert not any(name in _environment() for name in ("CLOUDSDK_AUTH_ACCESS_TOKEN", "GOOGLE_APPLICATION_CREDENTIALS", "CLOUDSDK_API_ENDPOINT_OVERRIDES_COMPUTE"))


class Trickle(httpx.AsyncByteStream):
    def __init__(self): self.closed = False
    async def __aiter__(self):
        for _ in range(20):
            await asyncio.sleep(0.02)
            yield b" "
    async def aclose(self): self.closed = True


async def test_default_rest_rejects_redirect_without_forwarding_bearer():
    seen = []
    def transport(req):
        seen.append(req)
        return httpx.Response(302, headers={"Location": "https://other.example/"})
    adapter = GoogleReadOnlyRest(transport=httpx.MockTransport(transport))
    result = await adapter.request("GET", "https://openidconnect.googleapis.com/v1/userinfo", None, "private-token")
    assert result.status == 302 and result.value is None and len(seen) == 1
    assert seen[0].url.host == "openidconnect.googleapis.com"


async def test_default_rest_total_timeout_closes_trickling_stream():
    stream = Trickle()
    adapter = GoogleReadOnlyRest(transport=httpx.MockTransport(lambda _: httpx.Response(200, stream=stream)), timeout=0.06)
    started = time.monotonic()
    with pytest.raises(ProbeUnavailable):
        await adapter.request("GET", "https://openidconnect.googleapis.com/v1/userinfo", None, "private-token")
    assert time.monotonic() - started < 0.3 and stream.closed


async def test_default_rest_bounds_body_and_never_allows_mutation_post():
    adapter = GoogleReadOnlyRest(transport=httpx.MockTransport(lambda _: httpx.Response(200, content=b"x" * (MAX_OUTPUT + 1))))
    with pytest.raises(ProbeUnavailable, match="output bound"):
        await adapter.request("GET", "https://openidconnect.googleapis.com/v1/userinfo", None, "private-token")
    with pytest.raises(ProbeUnavailable, match="permission observations"):
        await adapter.request("POST", "https://compute.googleapis.com/compute/v1/projects/example/zones/zone/instances", {}, "private-token")


async def test_failed_signal_has_bounded_observation_and_retains_exact_owned_group(tmp_path, monkeypatch):
    pidfile = tmp_path / "signal-failure-owned-pid"
    runner = GcloudReadOnly(fake_cli(tmp_path, f"import os, pathlib, time\npathlib.Path({str(pidfile)!r}).write_text(str(os.getpid()))\ntime.sleep(60)\n"), timeout=0.1)
    real_killpg = os.killpg
    def denied(group, sig):
        if sig == signal.SIGKILL:
            raise PermissionError("controlled owned signal failure")
        return real_killpg(group, sig)
    monkeypatch.setattr(os, "killpg", denied)
    started = time.monotonic()
    try:
        with pytest.raises(ProbeUnavailable, match="not positively stopped"):
            await runner.command([])
        assert time.monotonic() - started < 5
        group = int(pidfile.read_text())
        assert runner.cleanup_pending and group in runner._pending_groups
        assert group in runner._pending_processes
    finally:
        # The fixture, rather than the product observer, removes its retained
        # owned process after restoring the controlled signal failure.
        monkeypatch.setattr(os, "killpg", real_killpg)
        for group in tuple(runner._pending_groups):
            try: real_killpg(group, signal.SIGKILL)
            except ProcessLookupError: pass
            await asyncio.wait_for(runner._pending_processes[group][1], 2)
    assert runner.cleanup_pending is False and runner._pending_processes == {}


async def test_later_cli_configuration_cannot_replace_cloud_token_and_prevents_manual_plan(harness):
    service, local, cli, cloud = harness
    original = cloud.request
    async def changing(method, url, body, token):
        result = await original(method, url, body, token)
        if urlsplit(url).path == "/v1/userinfo":
            cli.profile = {"auth": {"impersonate_service_account": "other-principal-after-token"}}
        return result
    cloud.request = changing
    result = await checked(harness)
    assert result["ready_for_plan"] is False and result["plan"] is None
    assert row(result, "effective-account")["status"] == "checked"
    assert row(result, "account-current")["status"] == "blocked"
    assert all(token == "transient-private-token" for _, _, _, token in cloud.calls)
    assert len(cli.calls) == 4 and cli.calls[-1][0][:2] == ["config", "list"]


async def test_plan_pins_observed_cli_home_config_and_executable_across_terminal_context(harness, monkeypatch):
    service, _, cli, _ = harness
    cli.executable = "/observed-home/google-cloud-sdk/bin/gcloud"
    service._context = {"HOME": "/observed-home", "CLOUDSDK_CONFIG": "/observed-config-dir"}
    monkeypatch.setenv("HOME", "/other-terminal-home")
    monkeypatch.setenv("CLOUDSDK_CONFIG", "/other-terminal-config")
    monkeypatch.setenv("PATH", "/other-terminal-cli-bin")
    result = await checked(harness)
    assert result["ready_for_plan"] is True
    command = result["plan"]["commands"][0]
    assert "HOME=/observed-home" in command
    assert "CLOUDSDK_CONFIG=/observed-config-dir" in command
    assert "PATH=/observed-home/google-cloud-sdk/bin:" in command
    assert "$HOME" not in command and "$PATH" not in command and "${CLOUDSDK_CONFIG" not in command
    assert "/other-terminal" not in command


async def test_default_runner_pins_sanitized_context_before_later_environment_changes(tmp_path, monkeypatch):
    home = str(tmp_path / "observed-home")
    config = str(tmp_path / "observed-config")
    monkeypatch.setenv("HOME", home)
    monkeypatch.setenv("CLOUDSDK_CONFIG", config)
    runner = GcloudReadOnly(fake_cli(tmp_path, "import json, os\nprint(json.dumps({name: os.environ.get(name) for name in ['HOME', 'CLOUDSDK_CONFIG', 'CLOUDSDK_AUTH_ACCESS_TOKEN', 'CLOUDSDK_API_ENDPOINT_OVERRIDES_COMPUTE']}))\n"))
    monkeypatch.setenv("HOME", "/another-home")
    monkeypatch.setenv("CLOUDSDK_CONFIG", "/another-config")
    monkeypatch.setenv("CLOUDSDK_AUTH_ACCESS_TOKEN", "private-token")
    monkeypatch.setenv("CLOUDSDK_API_ENDPOINT_OVERRIDES_COMPUTE", "private-endpoint")
    result = json.loads((await runner.command([])).output)
    assert result == {"HOME": home, "CLOUDSDK_CONFIG": config, "CLOUDSDK_AUTH_ACCESS_TOKEN": None, "CLOUDSDK_API_ENDPOINT_OVERRIDES_COMPUTE": None}
    assert runner.cleanup_pending is False


async def test_repeated_cancellation_during_spawn_acquisition_retains_owned_process_and_admission(monkeypatch):
    entered, acquire, exited = asyncio.Event(), asyncio.Event(), asyncio.Event()
    observed = {"spawn_cancelled": False, "alive": True, "signals": []}
    class Process:
        pid = 987654321
        returncode = None
        stdout = asyncio.StreamReader()
        stderr = asyncio.StreamReader()
        async def wait(self):
            await exited.wait()
            return self.returncode
    process = Process()
    async def controlled_spawn(*args, **kwargs):
        entered.set()
        try:
            await acquire.wait()
            return process
        except asyncio.CancelledError:
            observed["spawn_cancelled"] = True
            raise
    def controlled_group(group, sig):
        assert group == process.pid
        if sig == signal.SIGKILL:
            observed["signals"].append(sig)
            observed["alive"] = False
            process.returncode = -signal.SIGKILL
            process.stdout.feed_eof()
            process.stderr.feed_eof()
            exited.set()
        elif sig == 0:
            if not observed["alive"]:
                raise ProcessLookupError()
        else:
            raise AssertionError("Unexpected signal")
    monkeypatch.setattr(asyncio, "create_subprocess_exec", controlled_spawn)
    monkeypatch.setattr(os, "killpg", controlled_group)
    local = Local()
    runner = GcloudReadOnly(executable="/controlled-not-executed/gcloud")
    service = GcpPreflightService(local, command=runner, rest=Cloud())
    task = asyncio.create_task(service.options(local.state["id"], local.build_id))
    await asyncio.wait_for(entered.wait(), 30)
    task.cancel()
    await asyncio.sleep(0)
    task.cancel()
    await asyncio.sleep(0)
    await asyncio.sleep(0)
    try:
        assert not observed["spawn_cancelled"], "Observer cancellation cancelled process ownership acquisition"
        assert not task.done(), "The observer released custody before acquiring the process"
        assert service._active is task
        with pytest.raises(DraftConflict, match="still finishing"):
            await service.options(local.state["id"], local.build_id)
    finally:
        acquire.set()
        try:
            await asyncio.wait_for(task, 1)
        except asyncio.CancelledError:
            pass
    assert observed["signals"] == [signal.SIGKILL]
    assert observed["alive"] is False
    assert runner.cleanup_pending is False and runner._pending_processes == {}
    assert service._active is None


def slow_cloud_package_work(monkeypatch, module):
    original_files, original_hashlib = module.package_files, module.hashlib
    def unpacked(payload):
        time.sleep(1.0)  # Unpacking a large package on a small host.
        return original_files(payload)
    class Hashing:
        def __getattr__(self, name):
            return getattr(original_hashlib, name)
        def sha256(self, data=b""):
            if bytes(data[:4]) == b"PK\x03\x04":
                time.sleep(1.0)  # Hashing it.
            return original_hashlib.sha256(data)
    monkeypatch.setattr(module, "package_files", unpacked)
    monkeypatch.setattr(module, "hashlib", Hashing())


async def test_cloud_account_options_leave_the_event_loop_free(harness, monkeypatch):
    from launchkit import gcp_preflight
    service, local, _, _ = harness
    slow_cloud_package_work(monkeypatch, gcp_preflight)
    result, gap = await longest_loop_gap(service.options(local.state["id"], local.build_id))
    assert result["accounts"] and result["cli"]["status"] == "checked"
    assert gap < 0.5


async def test_cloud_project_list_leaves_the_event_loop_free(harness, monkeypatch):
    from launchkit import gcp_preflight
    service, local, _, _ = harness
    slow_cloud_package_work(monkeypatch, gcp_preflight)
    result, gap = await longest_loop_gap(service.projects(local.state["id"], 7, local.build_id, ACCOUNT))
    assert len(result["projects"]) == 1 and result["truncated"] is False
    assert gap < 0.5


async def test_cloud_project_check_leaves_the_event_loop_free(harness, monkeypatch):
    from launchkit import gcp_preflight
    slow_cloud_package_work(monkeypatch, gcp_preflight)
    result, gap = await longest_loop_gap(checked(harness))
    assert result["ready_for_plan"] is True
    assert gap < 0.5
