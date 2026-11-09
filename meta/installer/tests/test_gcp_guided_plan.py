"""The novice creation plan describes the source-owned action it admits."""
import json
from launchkit.gcp_preflight import GcpPreflightService
from test_gcp_preflight import Local, Cli, ACCOUNT, PROJECT
from test_gcp_guided_preflight import GuidedCloud

async def test_guided_plan_separates_vm_creation_from_private_application_installation():
    local = Local()
    service = GcpPreflightService(local, command=Cli(), rest=GuidedCloud())
    value = await service.check_guided(local.state["id"], local.state["version"], local.build_id, ACCOUNT, PROJECT)
    plan = value["plan"]
    assert value["guided_supported"] is True and value["ready_for_plan"] is True
    assert plan["commands"] == []
    assert "private setup form" in json.dumps(plan["access"])
    assert "VM terminal" not in json.dumps(plan)
    assert "separate installation action" in json.dumps(plan["mutations"])
    assert "Python" in json.dumps(plan["mutations"])
    assert "local knowledge service" in json.dumps(plan["limitations"])
    assert value["provisioning_verified"] is False
    assert value["guided_access"]["private_settings_on_host_form"] is True
    ordinary = await service.check(local.state["id"], local.state["version"], local.build_id, ACCOUNT, PROJECT)
    assert "VM terminal" in json.dumps(ordinary["plan"]["access"])
    assert ordinary["plan"]["commands"]
