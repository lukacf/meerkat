"""Zonal OS Login overrides cannot inherit a project-only SSH approval."""
from urllib.parse import urlsplit
import pytest
from launchkit.gcp_preflight import GcpPreflightService, RestResult
from test_gcp_preflight import Local, Cli, ACCOUNT, PROJECT, ZONE, row
from test_gcp_guided_preflight import GuidedCloud

PATH = "/compute/v1/projects/" + PROJECT + "/zones/" + ZONE + "/instanceSettings"
def metadata(items):
    return {"kind": "compute#instance_settings", "zone": "https://www.googleapis.com/compute/v1/projects/" + PROJECT + "/zones/" + ZONE, "fingerprint": "synthetic-read-fingerprint", "metadata": {"items": items}}

class ZonalCloud(GuidedCloud):
    def __init__(self, result):
        super().__init__()
        self.zonal_result = result
    async def request(self, method, url, body, token):
        if urlsplit(url).path == PATH:
            assert method == "GET" and body is None and token == "transient-private-token"
            self.calls.append((method, url, body, token))
            return self.zonal_result
        return await super().request(method, url, body, token)

@pytest.mark.parametrize("value,status", [
    (RestResult(200, metadata({})), "checked"),
    (RestResult(200, metadata({"enable-oslogin": "FALSE"})), "checked"),
    (RestResult(200, metadata({"enable-oslogin": "TRUE"})), "blocked"),
    (RestResult(200, metadata({"enable-oslogin": "yes"})), "unknown"),
    (RestResult(200, metadata({"enable-oslogin": True})), "unknown"),
    (RestResult(200, metadata([])), "unknown"),
    (RestResult(200, {"metadata": {"items": {}}}), "unknown"),
    (RestResult(200, {**metadata({}), "zone": "projects/foreign-project/zones/" + ZONE}), "unknown"),
    (RestResult(403, None), "unknown"), (RestResult(404, None), "unknown"),
])
async def test_zonal_observation_controls_guided_creation_only(value, status):
    local, cloud = Local(), ZonalCloud(value)
    service = GcpPreflightService(local, command=Cli(), rest=cloud)
    result = await service.check_guided(local.state["id"], local.state["version"], local.build_id, ACCOUNT, PROJECT)
    assert row(result, "zonal-ssh-mode")["status"] == status
    assert result["guided_supported"] is (status == "checked")
    assert result["ready_for_plan"] is (status == "checked")
    assert result["provisioning_verified"] is False
    assert result["plan"] is None if status != "checked" else result["plan"] is not None
    cloud.calls.clear()
    ordinary = await service.check(local.state["id"], local.state["version"], local.build_id, ACCOUNT, PROJECT)
    assert ordinary["ready_for_plan"] is True
    assert not any(urlsplit(url).path == PATH for _, url, _, _ in cloud.calls)
