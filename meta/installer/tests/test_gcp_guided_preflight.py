"""Cloud-free checks for the explicit guided metadata-SSH extension."""
import asyncio
import hashlib
import json
from urllib.parse import urlsplit

import pytest

from launchkit.gcp_preflight import GcpPreflightService, RestResult
from launchkit.projects import DraftConflict
from test_gcp_preflight import ACCOUNT, PROJECT, NUMBER, ZONE, Local, Cli, Cloud, row, slow_cloud_package_work
from test_remote_deployment import longest_loop_gap

GUEST_PERMISSION = "compute.instances.getGuestAttributes"
POLICIES = (
    "disableGuestAttributesAccess",
    "managed.disableGuestAttributesAccess",
    "managed.requireOsLogin",
)


def policy_path(name):
    return "/v2/projects/" + NUMBER + "/policies/compute." + name + ":getEffectivePolicy"


def policy(name, rules=None):
    return {"name": "projects/" + NUMBER + "/policies/compute." + name,
            "spec": {"rules": [{"enforce": False}] if rules is None else rules}}


ZONAL_METADATA_PATH = "/compute/v1/projects/" + PROJECT + "/zones/" + ZONE + "/instanceSettings"


class GuidedCloud(Cloud):
    async def request(self, method, url, body, token):
        path = urlsplit(url).path
        if path == ZONAL_METADATA_PATH:
            assert method == "GET" and body is None and token == "transient-private-token"
            self.calls.append((method, url, body, token))
            return self.overrides.get(path, RestResult(200, {"kind": "compute#instance_settings",
                "zone": "https://www.googleapis.com/compute/v1/projects/" + PROJECT + "/zones/" + ZONE,
                "fingerprint": "synthetic-read-fingerprint", "metadata": {"items": {}}}))
        if body == {"permissions": [GUEST_PERMISSION]}:
            assert method == "POST" and token == "transient-private-token"
            self.calls.append((method, url, body, token))
            return self.overrides.get("guest-permission", RestResult(200, {"permissions": [GUEST_PERMISSION]}))
        if path in {policy_path(name) for name in POLICIES}:
            assert method == "GET" and body is None and token == "transient-private-token"
            self.calls.append((method, url, body, token))
            name = next(name for name in POLICIES if path == policy_path(name))
            return self.overrides.get(path, RestResult(200, policy(name)))
        return await super().request(method, url, body, token)


@pytest.fixture
def guided():
    local, cli, cloud = Local(), Cli(), GuidedCloud()
    return GcpPreflightService(local, command=cli, rest=cloud), local, cli, cloud


async def check(harness, **changes):
    service, local, _, _ = harness
    return await service.check_guided(local.state["id"], local.state["version"], local.build_id,
                                     changes.get("account", ACCOUNT), changes.get("project", PROJECT))


async def test_legacy_check_preserves_ordinary_response_and_observations(guided):
    service, local, _, cloud = guided
    result = await service.check(local.state["id"], 7, local.build_id, ACCOUNT, PROJECT)
    assert result["ready_for_plan"] is True
    assert "guided_supported" not in result and "guided_access" not in result
    assert not any(body == {"permissions": [GUEST_PERMISSION]} for _, _, body, _ in cloud.calls)
    assert not any(urlsplit(url).path in {policy_path(name) for name in POLICIES} for _, url, _, _ in cloud.calls)
    assert result["provisioning_verified"] is False
    assert not any(urlsplit(url).path == ZONAL_METADATA_PATH for _, url, _, _ in cloud.calls)


async def test_guided_positive_requires_actual_accepted_package_and_all_access_policy_checks(guided):
    result = await check(guided)
    service, local, cli, cloud = guided
    assert result["guided_supported"] is True and result["ready_for_plan"] is True
    assert result["provisioning_verified"] is False
    assert result["binding"]["package_sha256"] == hashlib.sha256(local.package_bytes).hexdigest()
    assert result["binding"]["source_digest"] == local.digest
    assert row(result, "guest-key-permission")["status"] == "checked"
    assert row(result, "zonal-ssh-mode")["status"] == "checked"
    assert sum(urlsplit(url).path == ZONAL_METADATA_PATH for _, url, _, _ in cloud.calls) == 1
    assert all(row(result, "policy-" + name)["status"] == "checked" for name in POLICIES)
    assert result["guided_access"]["mode"] == "metadata-ssh-guest-host-key"
    assert result["guided_access"]["user"] == "mobkit"
    assert all(method == "GET" or url.endswith(":testIamPermissions") for method, url, _, _ in cloud.calls)
    assert all(call[0] == "auth" or call[:2] == ["config", "list"] for call, _ in cli.calls)
    assert "transient-private-token" not in json.dumps(result)
    assert service._active is None


@pytest.mark.parametrize("kind", ["compute#instanceSettings", "compute#instance_settings"])
@pytest.mark.parametrize("metadata_fields", [{}, {"metadata": {}}, {"metadata": {"items": {}}}])
async def test_exact_zonal_settings_allow_known_kinds_and_omitted_empty_defaults(guided, kind, metadata_fields):
    guided[3].overrides[ZONAL_METADATA_PATH] = RestResult(200, {
        "kind": kind,
        "zone": "https://www.googleapis.com/compute/v1/projects/" + PROJECT + "/zones/" + ZONE,
        "fingerprint": "synthetic-read-fingerprint", **metadata_fields})
    result = await check(guided)
    assert row(result, "zonal-ssh-mode")["status"] == "checked"
    assert result["guided_supported"] is True and result["ready_for_plan"] is True
    assert row(result, "effective-account")["status"] == "checked"
    assert all(row(result, "policy-" + name)["status"] == "checked" for name in POLICIES)


@pytest.mark.parametrize("change,status", [
    ({"kind": "compute#wrongSettings"}, "unknown"),
    ({"kind": None}, "unknown"),
    ({"zone": "https://www.googleapis.com/compute/v1/projects/" + PROJECT + "/zones/other-zone"}, "unknown"),
    ({"zone": "https://www.googleapis.com/compute/v1/projects/foreign-project/zones/" + ZONE}, "unknown"),
    ({"fingerprint": ""}, "unknown"),
    ({"fingerprint": None}, "unknown"),
    ({"metadata": None}, "unknown"),
    ({"metadata": []}, "unknown"),
    ({"metadata": False}, "unknown"),
    ({"metadata": "private-malformed-metadata"}, "unknown"),
    ({"metadata": {"items": None}}, "unknown"),
    ({"metadata": {"items": []}}, "unknown"),
    ({"metadata": {"items": False}}, "unknown"),
    ({"metadata": {"items": {"enable-oslogin": True}}}, "unknown"),
    ({"metadata": {"items": {"enable-oslogin": "private-unsupported-login"}}}, "unknown"),
    ({"metadata": {"items": {"enable-oslogin": "true"}}}, "blocked"),
    ({"metadata": {"items": {"enable-oslogin": " TRUE "}}}, "blocked"),
])
async def test_zonal_default_support_preserves_resource_and_metadata_refusals(guided, change, status):
    value = {"kind": "compute#instanceSettings",
        "zone": "https://www.googleapis.com/compute/v1/projects/" + PROJECT + "/zones/" + ZONE,
        "fingerprint": "synthetic-read-fingerprint", **change}
    guided[3].overrides[ZONAL_METADATA_PATH] = RestResult(200, value)
    result = await check(guided)
    assert row(result, "zonal-ssh-mode")["status"] == status
    assert result["guided_supported"] is False and result["ready_for_plan"] is False and result["plan"] is None
    assert "private-malformed" not in json.dumps(result) and "private-unsupported" not in json.dumps(result)


@pytest.mark.parametrize("status", [403, 404])
async def test_denied_zonal_resource_does_not_turn_missing_metadata_into_empty_defaults(guided, status):
    guided[3].overrides[ZONAL_METADATA_PATH] = RestResult(status, {
        "kind": "compute#instanceSettings",
        "zone": "https://www.googleapis.com/compute/v1/projects/" + PROJECT + "/zones/" + ZONE,
        "fingerprint": "synthetic-read-fingerprint"})
    result = await check(guided)
    assert row(result, "zonal-ssh-mode")["status"] == "unknown"
    assert result["guided_supported"] is False and result["plan"] is None


@pytest.mark.parametrize("name", POLICIES)
@pytest.mark.parametrize("rules,status", [
    ([{"enforce": True}], "blocked"),
    ([{"enforce": False, "condition": {"expression": "resource.matchTag('env','approved')"}}], "unknown"),
    ([{"enforce": "false"}], "unknown"),
    ([], "unknown"),
    ([{"enforce": False}, {"enforce": False}], "unknown"),
])
async def test_guided_denied_conditional_or_malformed_policy_never_allows_plan(guided, name, rules, status):
    guided[3].overrides[policy_path(name)] = RestResult(200, policy(name, rules))
    result = await check(guided)
    assert row(result, "policy-" + name)["status"] == status
    assert result["guided_supported"] is False and result["ready_for_plan"] is False
    assert result["plan"] is None and result["provisioning_verified"] is False


@pytest.mark.parametrize("name", POLICIES)
@pytest.mark.parametrize("value", [RestResult(403, None), RestResult(404, None), RestResult(200, {"name": "foreign", "spec": {"rules": [{"enforce": False}]}})])
async def test_missing_or_foreign_effective_policy_is_unknown(guided, name, value):
    guided[3].overrides[policy_path(name)] = value
    result = await check(guided)
    assert row(result, "policy-" + name)["status"] == "unknown"
    assert result["guided_supported"] is False and result["plan"] is None


@pytest.mark.parametrize("value,status", [
    (RestResult(200, {"permissions": []}), "blocked"),
    (RestResult(403, None), "unknown"),
    (RestResult(200, {"permissions": [GUEST_PERMISSION, GUEST_PERMISSION]}), "unknown"),
    (RestResult(200, {"permissions": ["foreign.permission"]}), "unknown"),
])
async def test_guest_key_permission_cannot_be_inferred_or_duplicated(guided, value, status):
    guided[3].overrides["guest-permission"] = value
    result = await check(guided)
    assert row(result, "guest-key-permission")["status"] == status
    assert result["guided_supported"] is False and result["ready_for_plan"] is False


@pytest.mark.parametrize("change", ["deployer", "helper", "project", "acceptance", "brief", "source"])
async def test_package_or_source_mismatch_does_not_observe_cloud_or_gain_guided_support(guided, change):
    _, local, cli, cloud = guided
    if change in {"deployer", "helper"}:
        name = "deploy/deploy-gcp.sh" if change == "deployer" else "remote_operations.py"
        local.files[name] += b"\n# accepted custom cloud behavior\n"
        local.package_bytes = local.archive()
        result = await check(guided)
        assert result["guided_supported"] is False and result["plan"] is None
    elif change == "project":
        result = await check(guided, project="different-cloud-project")
        assert result["guided_supported"] is False and result["plan"] is None
    else:
        if change == "acceptance": local.accepted = False
        elif change == "brief": local.state["brief"]["outcome"] = "Another continuing job"
        else: local.digest = "f" * 64
        with pytest.raises((ValueError, DraftConflict)):
            await check(guided)
    assert cli.calls == [] and cloud.calls == []


@pytest.mark.parametrize("change", ["version", "source", "proposal"])
async def test_current_source_change_while_observing_discards_guided_completion(guided, change):
    service, local, _, cloud = guided
    cloud.wait = asyncio.Event()
    task = asyncio.create_task(check(guided))
    await cloud.entered.wait()
    if change == "version": local.state["version"] += 1
    elif change == "source": local.digest = "e" * 64
    else: local.state["proposal"]["project"]["name"] = "Changed cloud job"
    cloud.wait.set()
    with pytest.raises(DraftConflict):
        await task
    assert service._active is None


@pytest.mark.parametrize("name", POLICIES)
@pytest.mark.parametrize("spec", [None, [], "malformed-policy"])
async def test_bad_policy_envelope_is_controlled_unknown(guided, name, spec):
    guided[3].overrides[policy_path(name)] = RestResult(200, {
        "name": "projects/" + NUMBER + "/policies/compute." + name, "spec": spec})
    result = await check(guided)
    assert row(result, "policy-" + name)["status"] == "unknown"
    assert result["guided_supported"] is False and result["plan"] is None


async def test_guided_cloud_check_leaves_the_event_loop_free(guided, monkeypatch):
    from launchkit import gcp_preflight
    slow_cloud_package_work(monkeypatch, gcp_preflight)
    result, gap = await longest_loop_gap(check(guided))
    assert result["guided_supported"] is True and result["ready_for_plan"] is True
    assert gap < 0.5
