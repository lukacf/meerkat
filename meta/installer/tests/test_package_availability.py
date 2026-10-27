"""New-package readiness is a host projection, separate from plan acceptance.

Only synthetic interviews, private temporary source workspaces and inert worker
substitutions are used. No model, SDK, native process or installed app executes.
"""
import asyncio
from copy import deepcopy
import hashlib
import json
import uuid

import pytest

from launchkit import compiler
from launchkit.blueprint import AgentClass, ToolGrants, starter_blueprint
from launchkit.guide import GuideService, implementation_action_context
from launchkit.guide_models import REVIEW_TOPICS, starter_state
from launchkit.guide_store import GuideStore
from launchkit.implementation import ImplementationService
from launchkit.implementation_workspace import ImplementationWorkspace
from launchkit.projects import fingerprint
from launchkit.templates.storage_setup import write_private


def plan(selected=True):
    value = starter_blueprint().model_dump(mode="json")
    if selected:
        value["agent_classes"].append(AgentClass(
            id="connection_researcher", name="Connection researcher",
            description="Research requested tool connections.",
            instructions="Research and prepare requested connections; return findings to the lead.",
            tools=ToolGrants(memory=False),
        ).model_dump(mode="json"))
        value["acquisition"] = {"helper_identity": "connection_researcher:main",
            "helper_profile": "exclusive_application_role", "helper_tools": "preparation"}
    return value


def brief():
    return {"facts": {topic: {"summary": f"The person supplied {topic} context.",
        "status": "confirmed", "source_message_ids": ["request"]} for topic in REVIEW_TOPICS},
        "requirements": []}


def state(selected=True):
    value = starter_state({"available": True, "provider": "openai"})
    value.update(proposal=plan(selected), brief=brief(), status="review", stage="review",
        question=None, messages=[{"id": "request", "role": "user", "text": "Prepare my requested connections."}])
    return value


async def test_guide_recomputes_bound_availability_and_never_persists_it(tmp_path, monkeypatch):
    monkeypatch.setattr("launchkit.guide.planner_capability", lambda: {"available": True})
    service = GuideService(GuideStore(tmp_path / "guides"), object())
    saved = service.store.create(state())
    monkeypatch.setattr(compiler, "generate_package", lambda *a, **k: pytest.fail("Guide reads must not compile packages."))
    shown = await service.get(saved["id"])
    expected = {"guide_id": saved["id"], "guide_version": saved["version"],
        "source": "proposal", "blueprint_hash": fingerprint(saved["proposal"]),
        "available": False, "status": "pending-integration", "reason": compiler.CONNECTION_SETUP_PENDING}
    assert shown.get("package_availability") == expected
    assert "package_availability" not in service.store.get(saved["id"])

    # A prior response, even if supplied as input, cannot become saved authority.
    changed = deepcopy(shown)
    changed["proposal"]["project"]["name"] = "Revised connection setup"
    changed["package_availability"].update(available=True, reason=None)
    revised = await service._save(changed, shown["version"])
    assert revised["package_availability"]["guide_version"] == shown["version"] + 1
    assert revised["package_availability"]["blueprint_hash"] == fingerprint(changed["proposal"])
    assert revised["package_availability"]["available"] is False
    assert "package_availability" not in service.store.get(saved["id"])

    accepted = await service.apply(saved["id"], revised["version"])
    assert accepted["proposal"] is None and accepted["status"] == "ready"
    assert accepted["package_availability"]["source"] == "blueprint"
    assert accepted["package_availability"]["guide_version"] == accepted["version"]
    assert compiler.CONNECTION_SETUP_PENDING in accepted["messages"][-1]["text"]
    assert "download it for your chosen computer" not in accepted["messages"][-1]["text"]
    assert "package_availability" not in service.store.get(saved["id"])

    # Keep-current clears only desired setup selection; current host readiness wins.
    accepted["blueprint"].pop("acquisition")
    available = await service._save(accepted, accepted["version"])
    assert available["package_availability"]["available"] is True
    assert available["package_availability"]["reason"] is None
    assert available["package_availability"]["blueprint_hash"] == fingerprint(available["blueprint"])


def test_preview_export_and_actions_share_current_guard(monkeypatch):
    selected = plan()
    ready = compiler.package_availability(selected)
    assert ready == {"available": False, "status": "pending-integration", "reason": compiler.CONNECTION_SETUP_PENDING}
    preview = compiler.package_preview(selected)
    assert preview["valid"] is True and preview["files"] == []
    assert preview["package_availability"] == ready
    with pytest.raises(ValueError) as refused:
        compiler.generate_package(selected)
    assert str(refused.value) == ready["reason"]
    guided = {"id": str(uuid.uuid4()), "version": 4, "proposal": selected, "brief": brief(),
        "package_availability": {"available": True, "reason": None}}
    evidence = {"lifecycle": {"availability": "available", "build_gate_clear": True,
        "status": "running", "stop_available": True}}
    actions = implementation_action_context(guided, evidence, {"available": True}, True)
    assert actions["build_available"] is False and actions["unavailable_reason"] == ready["reason"]
    assert actions["stop_available"] is True
    assert actions["package_availability"]["guide_version"] == guided["version"]
    assert "new generation with the current compiler" in actions["admission"]
    assert "Saved-source continuation is checked separately in the wizard" in actions["admission"]
    guided["proposal"].pop("acquisition")
    guided["package_availability"] = ready
    actions = implementation_action_context(guided, evidence, {"available": True}, True)
    assert actions["build_available"] is True and actions["unavailable_reason"] is None


class StaticGuide:
    def __init__(self, selected=True):
        self.state = {"id": str(uuid.uuid4()), "version": 3, "proposal": plan(selected),
            "brief": brief(), "messages": []}

    async def get(self, identity):
        assert identity == self.state["id"]
        return deepcopy(self.state)


class InertService(ImplementationService):
    async def _run(self, directory):
        await asyncio.Event().wait()


def host_ready(monkeypatch):
    monkeypatch.setattr("launchkit.planner.planner_capability", lambda: {"available": True})
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: "/synthetic/checker")


@pytest.mark.parametrize("previous_status", [None, "failed", "queued"])
async def test_refusal_precedes_history_reservation_or_interruption(tmp_path, monkeypatch, previous_status):
    host_ready(monkeypatch)
    guide = StaticGuide()
    service = InertService(tmp_path, guide)
    previous = None
    if previous_status:
        build = str(uuid.uuid4())
        previous = service._directory(guide.state["id"], build)
        previous.mkdir(parents=True)
        old = plan(False)
        ImplementationWorkspace(previous / "workspace", old).seed()
        write_private(previous / "input.json", {"blueprint": old, "brief": brief(), "request": "Earlier request"})
        write_private(previous / "receipt.json", {"build_id": build, "guide_id": guide.state["id"],
            "guide_version": 2, "status": previous_status, "cleanup_pending": False})
        write_private(previous.parent / "latest.json", {"build_id": build})
    snapshots = {str(p.relative_to(tmp_path)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in tmp_path.rglob("*") if p.is_file()}
    monkeypatch.setattr(ImplementationWorkspace, "seed", lambda *a, **k: pytest.fail("Refusal must precede workspace reservation/seeding."))

    class NoCheckerAdmission:
        async def status(self):
            pytest.fail("A package hold must precede new checker admission.")
    service.checker_setup = NoCheckerAdmission()
    with pytest.raises(ValueError) as refused:
        await service.submit(guide.state["id"], 3, "Build this plan", expected_selected_build_id=previous.name if previous else None)
    assert str(refused.value) == compiler.CONNECTION_SETUP_PENDING
    assert not service.tasks and not service.processes
    after = {str(p.relative_to(tmp_path)): hashlib.sha256(p.read_bytes()).hexdigest()
        for p in tmp_path.rglob("*") if p.is_file()}
    assert after == snapshots
    if previous is None:
        assert not service.root.exists()
    else:
        assert sorted(p.name for p in previous.parent.iterdir()) == sorted([previous.name, "latest.json"])


async def test_ordinary_build_still_admits_inert_worker(tmp_path, monkeypatch):
    host_ready(monkeypatch)
    guide = StaticGuide(False)
    service = InertService(tmp_path, guide)
    try:
        admitted = await service.submit(guide.state["id"], 3, expected_selected_build_id=None)
        assert admitted["status"] == "queued" and admitted["ready_to_package"] is False
        assert list(service.tasks) == [admitted["build_id"]] and not service.processes
        assert json.loads((service._directory(guide.state["id"]) / "input.json").read_text())["blueprint"] == guide.state["proposal"]
    finally:
        await service.shutdown()


async def test_existing_active_selected_build_is_observed_without_new_generation(tmp_path, monkeypatch):
    """Synthetic active-owner state tests dedup only, not native candidate custody."""
    host_ready(monkeypatch)
    guide = StaticGuide()
    service = InertService(tmp_path, guide)
    build = str(uuid.uuid4())
    directory = service._directory(guide.state["id"], build)
    directory.mkdir(parents=True)
    write_private(directory / "input.json", {"blueprint": guide.state["proposal"], "brief": brief(),
        "request": "", "refresh_framework": False, "preserve_design": False})
    write_private(directory / "receipt.json", {"build_id": build, "status": "running", "request": ""})
    write_private(directory.parent / "latest.json", {"build_id": build})
    service.tasks[build] = object()
    monkeypatch.setattr(service, "_retained_runtime_candidate", lambda *a: None)
    result = {"build_id": build, "status": "running", "ready_to_package": False}

    async def observed(identity, **kwargs):
        return deepcopy(result)
    monkeypatch.setattr(service, "status", observed)
    snapshots = {str(p.relative_to(tmp_path)): p.read_bytes() for p in tmp_path.rglob("*") if p.is_file()}
    assert await service.submit(guide.state["id"], 3, expected_selected_build_id=build) == result
    assert {str(p.relative_to(tmp_path)): p.read_bytes() for p in tmp_path.rglob("*") if p.is_file()} == snapshots
    assert service.tasks[build] is not None and not service.processes
