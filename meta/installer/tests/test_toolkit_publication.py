"""Exact Toolkit handoff through real proposal checking and package compilation."""
import copy
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest

from launchkit.architect import ArchitectService
from launchkit.blueprint import Connection, starter_blueprint
from launchkit.toolkit_setup import ToolkitSetupUnavailable


@pytest.fixture
def publication(tmp_path):
    service = ArchitectService(tmp_path)
    service.set_context(starter_blueprint().model_dump(mode="json"))
    service.guide_context = {"action": "chat"}
    service.reference_read = True
    service.start = AsyncMock(side_effect=AssertionError("Offline publication cannot start native runtime"))
    connection = Connection(id="reader", name="Selected reader", transport="stdio",
        command="/fictional/installed/runtime/bin/python", args=["-I", "-B", "-m", "fixture_reader"],
        tool_evidence={"source_url": "toolkit-setup:agno/fixture-reader?contents_sha256=" + "b" * 64,
            "sha256": "c" * 64, "retrieved_at": "2026-10-04T12:00:00+00:00",
            "tool_names": ["read_article", "search_article"]}).model_dump(mode="json")
    owner_result = {"connection": connection, "provenance": {"job_id": "a" * 32,
        "scope": "software_only", "portability": "current_host_only"}}
    calls = []
    async def handoff(arguments):
        calls.append(copy.deepcopy(arguments))
        return copy.deepcopy(owner_result)
    service.toolkit_setup = SimpleNamespace(handoff=handoff)
    return SimpleNamespace(service=service, owner_result=owner_result, calls=calls,
        arguments={"job_id": "a" * 32, "connection_id": "reader", "name": "Selected reader"})


def candidate_for(service, connection, names=None):
    candidate = copy.deepcopy(service.context)
    candidate["connections"] = [copy.deepcopy(connection)]
    if names is not None:
        candidate["agent_classes"][0]["tools"]["mcp"] = [{"connection_id": "reader", "tool_names": names}]
    return candidate


@pytest.mark.asyncio
async def test_owned_handoff_is_retained_and_enters_real_checked_proposal_with_evidenced_names(publication):
    service = publication.service
    before = copy.deepcopy(service.context)
    result = await service.inspect_toolkit_installation(publication.arguments)
    assert publication.calls == [publication.arguments]
    assert service.discovered["reader"] == result["connection"]
    candidate = candidate_for(service, result["connection"], ["read_article"])
    checked, checks = service._check_candidate({"blueprint": candidate})
    assert checks["summary"]["connections"] == 1 and checks["summary"]["files"] > 0
    assert any(check["label"] == "Native runtime" for check in checks["checks"])
    assert checked["agent_classes"][0]["tools"]["mcp"] == [{"connection_id": "reader", "tool_names": ["read_article"]}]
    proposed = await service.propose({"blueprint": candidate, "summary": "Use the checked reader for the selected operation."})
    assert proposed["valid"] is True and service.proposal["blueprint"] == checked
    assert service.context == before and service.runtime is None
    service.start.assert_not_called()
    result["connection"]["args"].append("invented")
    assert service.discovered["reader"]["args"] == publication.owner_result["connection"]["args"]


@pytest.mark.asyncio
@pytest.mark.parametrize("changed", ["command", "args"])
async def test_rewritten_installed_launch_cannot_enter_proposal(publication, changed):
    service = publication.service
    result = await service.inspect_toolkit_installation(publication.arguments)
    candidate = candidate_for(service, result["connection"], ["read_article"])
    if changed == "command": candidate["connections"][0]["command"] = "/fictional/other/python"
    else: candidate["connections"][0]["args"].append("--invented-switch")
    with pytest.raises(ValueError, match="not retrieved"):
        service._check_candidate({"blueprint": candidate})
    assert not (await service.propose({"blueprint": candidate}))["valid"]
    assert service.proposal is None and not (service.root / "proposal.json").exists()
    assert service.discovered["reader"] == publication.owner_result["connection"]


@pytest.mark.asyncio
@pytest.mark.parametrize("names, forged", [(["invented_tool"], False), ([], False), (["write_admin"], False), (["write_admin"], True)])
async def test_all_invented_or_unevidenced_tools_are_rejected_even_with_candidate_forgery(publication, names, forged):
    service = publication.service
    result = await service.inspect_toolkit_installation(publication.arguments)
    candidate = candidate_for(service, result["connection"], names)
    if forged:
        candidate["connections"][0]["tool_evidence"]["tool_names"].append("write_admin")
    with pytest.raises(ValueError, match="publisher-backed exact tool names"):
        service._check_candidate({"blueprint": candidate})
    assert not (await service.propose({"blueprint": candidate}))["valid"] and service.proposal is None
    assert service.discovered["reader"]["tool_evidence"]["tool_names"] == ["read_article", "search_article"]


@pytest.mark.asyncio
async def test_no_tool_evidence_allows_ungranted_connection_but_does_not_allow_new_grants(publication):
    service = publication.service
    publication.owner_result["connection"]["tool_evidence"] = None
    result = await service.inspect_toolkit_installation(publication.arguments)
    ungranted = candidate_for(service, result["connection"])
    checked, _ = service._check_candidate({"blueprint": ungranted})
    assert all(not role["tools"]["mcp"] for role in checked["agent_classes"])
    with pytest.raises(ValueError, match="publisher-backed exact tool names"):
        service._check_candidate({"blueprint": candidate_for(service, result["connection"], ["read_article"])})


@pytest.mark.asyncio
async def test_colliding_discovered_identity_keeps_original_launch_and_refuses_handoff(publication):
    service = publication.service
    original = copy.deepcopy(publication.owner_result["connection"])
    original["args"].append("--other-existing-launch")
    service.discovered["reader"] = original
    before = copy.deepcopy(service.discovered)
    with pytest.raises(ValueError, match="already names another discovered launch"):
        await service.inspect_toolkit_installation(publication.arguments)
    assert service.discovered == before and service.proposal is None


@pytest.mark.asyncio
async def test_failed_owner_handoff_does_not_record_or_replace_discovery(publication):
    service = publication.service
    service.discovered["existing"] = {"id": "existing", "name": "Existing", "transport": "stdio", "command": "existing", "args": []}
    before = copy.deepcopy(service.discovered)
    service.toolkit_setup.handoff = AsyncMock(side_effect=ToolkitSetupUnavailable("The selected job could not be confirmed."))
    with pytest.raises(ToolkitSetupUnavailable):
        await service.inspect_toolkit_installation(publication.arguments)
    assert service.discovered == before and service.proposal is None
    service.start.assert_not_called()
