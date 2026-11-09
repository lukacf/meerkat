"""Inert source regression for an editable unsupported deployment target."""
from copy import deepcopy

import pytest

from launchkit import compiler
from launchkit.projects import fingerprint
from test_guide import complete_brief, fact, guide_client, start
from test_meta_toolkit_initial_package import setup_spec
from test_meta_toolkit_runtime_selection import product, no_product_execution
from test_meta_toolkit_runtime_update import catalog_target
import test_meta_toolkit_runtime_selection as product_fixtures


@pytest.fixture(autouse=True)
def inert_isolation_probe(monkeypatch):
    """A guide turn reads the host's isolation probe, which starts a process.

    This file forbids processes, so the probe passed only after an earlier
    file had cached it. Select an inert sandbox path here, as other fixtures do.
    """
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: "/inert/sandbox")


@pytest.fixture
def selected_runtime(request, monkeypatch):
    platform = request.param
    monkeypatch.setattr(product_fixtures, "TARGET", platform)
    monkeypatch.setattr(product_fixtures, "PREFIX", "meerkat-connect-" + platform)
    monkeypatch.setattr(product_fixtures, "RESOURCE",
        "meerkat-connect-" + platform + "/libexec/meerkat-connect")
    return catalog_target(request.getfixturevalue("product"))


@pytest.mark.parametrize("selected_runtime, desired, corrected", [
    ("linux-x86_64", "mac", "linux"),
    ("macos-arm64", "linux", "mac"),
    ("macos-arm64", "gcp", "mac"),
], indirect=["selected_runtime"])
def test_saved_target_mismatch_remains_editable_through_normal_clarification(
        guide_client, selected_runtime, desired, corrected):
    client, agent, service = guide_client
    service.package_runtime_provider = lambda state: selected_runtime
    saved = start(client)
    proposal = setup_spec()
    proposal["target"] = desired
    if desired == "gcp":
        proposal["gcp"]["project"] = "fixture-cloud-project"
    saved.update(proposal=proposal, status="refining", stage="review", question=None,
        brief=complete_brief("request"), messages=[{
            "id": "request", "role": "user", "text": "Review my comparison team."}])
    saved = service.store.save(saved, saved["version"])
    before = service.store.get(saved["id"])
    service.store.save_operation(saved["id"], {
        "request_id": "failed-original-operation", "version": saved["version"] - 1,
        "status": "failed", "detail": "Original publication readback failed."})

    # Reading an incompatible desired plan cannot require silently rewriting it.
    response = client.get("/api/guide/" + saved["id"])
    assert response.status_code == 200, response.text
    shown = response.json()
    availability = shown["package_availability"]
    assert availability["available"] is False
    assert availability["status"] == "runtime-target-mismatch"
    assert availability["selected_local_target"] == corrected
    assert availability["blueprint_hash"] == fingerprint(proposal)
    assert shown["proposal"] == proposal
    assert shown["operation"]["status"] == "failed"
    assert service.store.get(saved["id"]) == before
    # Native source admission is still strict on the incompatible target.
    with pytest.raises(ValueError):
        compiler.generate_package(proposal, runtime_target=selected_runtime)

    def clarify(context, current):
        assert current == proposal
        assert context["package_availability"]["selected_local_target"] == corrected
        revised = deepcopy(current)
        revised["target"] = corrected
        brief = deepcopy(context["brief"])
        brief["facts"]["destination"] = fact("Run locally on " + corrected,
            context["current_user_message"]["id"])
        return {"guide": {"publication_mode": "refinement", "question": None,
            "message": "The deployment target is clarified.", "summary": "Target clarified.",
            "warnings": [], "brief": brief, "design_changed": True},
            "proposal": revised}

    agent.reply = clarify
    clarified = client.post("/api/guide/" + saved["id"] + "/respond", json={
        "version": shown["version"], "action": "chat",
        "text": "Run this application locally on " + corrected + "; preserve the team."})
    assert clarified.status_code == 200, clarified.text
    current = client.get("/api/guide/" + saved["id"]).json()
    assert current["proposal"]["target"] == corrected
    assert current["package_availability"]["available"] is True
    assert current["operation"]["status"] == "completed"
    assert len(agent.calls) == 1


@pytest.mark.parametrize("selected_runtime", ["linux-x86_64"], indirect=True)
def test_mismatch_projection_does_not_mask_changed_runtime_artifacts(
        guide_client, selected_runtime):
    client, agent, service = guide_client
    service.package_runtime_provider = lambda state: selected_runtime
    saved = start(client)
    proposal = setup_spec()
    proposal["target"] = "mac"
    saved.update(proposal=proposal)
    saved = service.store.save(saved, saved["version"])
    selected_runtime.assembly.path.write_bytes(b"{}\n")

    response = client.get("/api/guide/" + saved["id"])
    assert response.status_code == 200, response.text
    availability = response.json()["package_availability"]
    assert availability["available"] is False
    assert availability["status"] == "runtime-unavailable"
    assert "selected_local_target" not in availability
    assert response.json()["proposal"] == proposal
    assert agent.calls == []
    with pytest.raises(ValueError):
        compiler.generate_package(proposal, runtime_target=selected_runtime)
