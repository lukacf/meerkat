"""meta-forge journeys on Forge's own guide and installation owners, with fakes for agents and workers.

The guide is the real GuideService, GuideStore and GuideOperations on a
temporary state directory, answered by the SetupAgent fixture of
test_guide.py. Installation is the real DeploymentService with the inert
worker of test_deployment.py, which admits no subprocess. Each journey runs in
one event loop, as one Forge server process would.
"""
import asyncio
import json
import socket
import sys
from types import SimpleNamespace
from unittest.mock import AsyncMock

import pytest

from cli_support import Recording, Terminal, cli
from launchkit import server
from launchkit.deployment import materialize, read_receipt
from launchkit.guide import GuideService
from launchkit.guide_operations import GuideOperations
from launchkit.guide_store import GuideStore
from launchkit.implementation import NoApplicationBuild
from test_deployment import Service
from test_deployment_native_team import NoImplementation
from test_guide import SetupAgent, complete_brief

HOST = "mac" if sys.platform == "darwin" else "linux"
SAVED_KEY = "journey-saved-provider-key-not-a-real-key"


def review_reply(context, blueprint):
    blueprint["project"]["name"] = "Heat-pump comparison"
    blueprint["target"] = HOST
    return {"guide": {"question": None, "message": "Your comparison team is ready to review.",
                      "brief": complete_brief(context["current_user_message"]["id"]), "design_changed": True,
                      "summary": "One comparison team.", "warnings": []}, "proposal": blueprint}


def free_port():
    with socket.socket() as probe:
        probe.bind(("127.0.0.1", 0))
        return probe.getsockname()[1]


@pytest.fixture
def journey(tmp_path, monkeypatch):
    from launchkit import guide as guide_module
    monkeypatch.setattr(guide_module, "planner_capability",
                        lambda: {"available": True, "provider": "openai", "reason": "Ready"})
    agent = SetupAgent()
    agent.reply = review_reply
    guide = GuideService(GuideStore(tmp_path / "state"), agent)

    def no_application(_state):
        raise NoApplicationBuild("No application build")

    guide.implementation_evidence_provider = no_application
    operations = GuideOperations()
    installs = Service(tmp_path / "state", guide, NoImplementation(), operations)
    installs.service_manager = SimpleNamespace(capability=AsyncMock(return_value={"available": False}))
    monkeypatch.setattr(server, "guide", guide)
    monkeypatch.setattr(server, "guide_operations", operations)
    monkeypatch.setattr(server, "deployment", installs)
    monkeypatch.setattr(server, "architect", SimpleNamespace(guide_context=None, proposal=None, guide_result=None,
                                                             message_lock=asyncio.Lock()))
    monkeypatch.setattr(server, "cloud_capability", lambda: {"available": False, "project": None, "reason": "test"})
    # Forge's own saved provider key lives in the server's environment only; the CLI's environment has none.
    monkeypatch.setenv("OPENAI_API_KEY", SAVED_KEY)
    yield SimpleNamespace(agent=agent, guide=guide, installs=installs, root=tmp_path, transport=Recording(server.app))
    for task in list(installs.tasks.values()):
        task.cancel()


async def run(journey, *argv, **options):
    return await cli(list(argv), transport=journey.transport, **options)


async def accepted_setup(journey):
    created = await run(journey, "setup", "new", "--json")
    assert created.code == 0, created.text
    identity = created.document["setup"]
    sent = await run(journey, "setup", "send", identity, "--text",
                     "Compare two heat-pump proposals with source references", "--wait", "--json")
    assert sent.code == 0, sent.text
    assert sent.document["result"]["status"] == "review" and sent.document["result"]["proposal"]
    review = await run(journey, "plan", "accept", identity, "--json")
    assert review.code == 84 and review.document["error"]["code"] == "approval_required"
    accepted = await run(journey, "plan", "accept", identity, "--approve", review.document["digest"], "--json")
    assert accepted.code == 0, accepted.text
    assert accepted.document["result"]["status"] == "ready"
    return identity, accepted.document["result"]["version"]


async def test_design_accept_install_operate_and_recover_a_lost_action(journey):
    identity, version = await accepted_setup(journey)
    assert len(journey.agent.calls) == 1
    folder = journey.root / "team"
    port = free_port()
    argv = ["install", identity, "--team", "--folder", str(folder), "--port", str(port), "--reuse-forge-keys"]
    review = await run(journey, *argv, "--json")
    assert review.code == 84, review.text
    assert review.document["review"]["what"]["agents"] and review.document["review"]["version"] == version
    installed = await run(journey, *argv, "--approve", review.document["digest"], "--no-wait", "--json")
    assert installed.code == 0, installed.text
    receipt = installed.document["result"]
    assert receipt["source_kind"] == "native-team" and receipt["destination"] == str(folder)
    directory = journey.installs._directory(identity)
    # Forge, not the command, copied its saved key into the installation's private settings.
    assert (folder / ".env").read_text().count(SAVED_KEY) == 1
    assert (folder / ".env").stat().st_mode & 0o777 == 0o600
    materialize(directory)

    journey.transport.lose.add(("POST", f"/api/guide/{identity}/deployment/action"))
    stopped = await run(journey, "installation", "stop", identity, "--no-wait", "--json")
    assert (stopped.code, stopped.document["error"]["code"]) == (83, "outcome_unknown")
    request = stopped.document["request"]
    record = await run(journey, "installation", "request", identity, request, "--wait", "--json")
    assert record.code == 0, record.text
    assert record.document["result"]["action_request"]["phase"] == "settled"
    assert record.document["result"]["action_request"]["action"] == "stop"
    status = await run(journey, "installation", "status", identity, "--json")
    assert status.code == 0 and status.document["result"]["status"] == "stopped"
    assert read_receipt(directory)["status"] == "stopped"

    settings = await run(journey, "installation", "settings", "show", identity, "--json")
    assert settings.code == 0, settings.text
    fields = {field["key"]: field for field in settings.document["result"]["fields"]}
    assert fields["OPENAI_API_KEY"]["configured"] is True
    assert SAVED_KEY not in settings.text

    start = await run(journey, "installation", "start", identity, "--json")
    assert start.code == 84 and start.document["review"]["current_status"] == "stopped"
    started = await run(journey, "installation", "start", identity, "--approve", start.document["digest"],
                        "--no-wait", "--json")
    assert started.code == 0, started.text
    again = await run(journey, "installation", "request", identity, started.document["request"], "--json")
    assert again.code == 0 and again.document["result"]["action_request"]["action"] == "restart"
    assert len(journey.agent.calls) == 1


async def test_failed_turn_is_resent_with_setup_retry(journey):
    created = await run(journey, "setup", "new", "--json")
    identity = created.document["setup"]
    journey.agent.fail = True
    failed = await run(journey, "setup", "send", identity, "--text", "Compare two heat-pump proposals", "--wait", "--json")
    assert (failed.code, failed.document["error"]["code"]) == (1, "operation_failed")
    assert "Provider failed" not in failed.text
    journey.agent.fail = False
    retried = await run(journey, "setup", "retry", identity, "--wait", "--json")
    assert retried.code == 0, retried.text
    assert retried.document["result"]["status"] == "review"
    assert [call["guide_context"]["user_message"] for call in journey.agent.calls] == [
        "Compare two heat-pump proposals"] * 2


async def test_an_interactive_chat_designs_and_accepts(journey):
    script = Terminal("Compare two heat-pump proposals with source references\n/plan\n/accept\ny\n/quit\n")
    result = await run(journey, "chat", "--new", stdin=script, terminal=True)
    assert result.code == 0, result.text
    assert "Guide: Your comparison team is ready to review." in result.stdout
    assert "Plan (proposed" in result.stdout and "Plan accepted (version" in result.stdout
    assert "Review: accept plan version" in result.stderr and "Accept this plan?" in result.stderr
    (setup,) = journey.guide.store.list()
    assert setup["status"] == "ready"


async def test_chat_refuses_without_a_terminal(journey):
    result = await run(journey, "chat", "--new")
    assert result.code == 2 and "setup send" in result.text
    assert journey.guide.store.list() == []


async def test_a_lost_setup_creation_is_repeated_without_a_second_setup(journey):
    request = "cd20e4ba-618c-42e2-b40a-07d1628f1a53"
    journey.transport.lose.add(("POST", "/api/guide/start"))
    lost = await run(journey, "setup", "new", "--request-id", request, "--json")
    assert (lost.code, lost.document["request"]) == (83, request)
    again = await run(journey, "setup", "new", "--request-id", request, "--json")
    assert again.code == 0
    assert [item["id"] for item in journey.guide.store.list()] == [again.document["setup"]]
    assert json.dumps(again.document).count(request) == 1
