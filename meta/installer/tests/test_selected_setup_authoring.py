"""Selected source authoring evidence with inert products and a controlled author.

No model, SDK, installed package or native program executes. The normal guide
route still calls the actual inspection, reference and atomic publication tools.
"""
from copy import deepcopy
from unittest.mock import AsyncMock

import pytest

from launchkit import compiler
from launchkit.architect import ArchitectService
from launchkit.blueprint import Blueprint
from launchkit.organization import organization_snapshot
from launchkit.projects import fingerprint
from test_guide import complete_brief, guide_client, start
from test_guided_edit_publication import activate, context, edit_args
from test_guide_runtime_platform_recovery import selected_runtime
from test_meta_toolkit_initial_package import setup_spec, setup_with_classes_spec
from test_meta_toolkit_runtime_selection import product, no_product_execution
from test_meta_toolkit_runtime_update import catalog_target


@pytest.fixture(autouse=True)
def inert_isolation_probe(monkeypatch):
    """A guide turn reads the host's isolation probe, which starts a process.

    This file forbids processes, so the probe passed only after an earlier
    file had cached it. Select an inert sandbox path here, as other fixtures do.
    """
    monkeypatch.setattr("launchkit.implementation_workspace._sandbox_executable", lambda: "/inert/sandbox")


def handoff_guidance(authoring):
    return next(row for row in authoring["requirements"]
        if "find_and_install user_context" in row)


def saved_plan(client, service, value):
    saved = start(client)
    saved.update(proposal=value, status="refining", stage="review", question=None,
        brief=complete_brief("request"), messages=[{
            "id": "request", "role": "user", "text": "Enable connection discovery for the request owner."}])
    return service.store.save(saved, saved["version"])


@pytest.mark.parametrize("selected_runtime", ["linux-x86_64", "macos-arm64"], indirect=True)
@pytest.mark.parametrize("declared_classes", [False, True])
@pytest.mark.parametrize("helper_selected", [False, True])
def test_normal_clarification_authors_supported_caller_from_host_contract(
        guide_client, selected_runtime, declared_classes, helper_selected, tmp_path, monkeypatch):
    client, agent, service = guide_client
    service.package_runtime_provider = lambda state: selected_runtime
    document = ("  Incident history is available through our workplace tools.\n"
        "Use the setup instructions at https://tools.example.test/start.\n"
        "Let Connect resolve the tool; retain this original wording.\n")
    service.organization = organization_snapshot({"schema_version": 1,
        "id": "workplace", "version": "1", "label": "Workplace",
        "guide_context": document, "starting_examples": []})
    plan = setup_with_classes_spec() if declared_classes else setup_spec()
    plan["target"] = "linux" if selected_runtime.target == "linux-x86_64" else "mac"
    plan["agent_classes"][0]["tools"].pop("rust_bundles")
    desired_helper = deepcopy(plan["agent_classes"][1])
    desired_acquisition = deepcopy(plan["acquisition"])
    if not helper_selected:
        plan["agent_classes"].pop(1)
        plan["acquisition"] = None
    saved = saved_plan(client, service, plan)
    before = service.store.get(saved["id"])
    author = ArchitectService(tmp_path / "controlled-author")
    monkeypatch.setattr(author, "model_choices", AsyncMock(return_value={"status": "unavailable"}))
    observed = {}

    async def author_turn(prompt, blueprint, *, guide_context):
        author.set_context(blueprint)
        activate(author, guide_context)
        inspected = await author.inspect({})
        assert inspected["schema"] is None
        assert inspected["guided_setup"]["organization"]["guide_context"] == document
        availability = inspected["guided_setup"]["package_availability"]
        assert availability["blueprint_hash"] == fingerprint(blueprint)
        # An ordinary package can be ready even though setup has not been
        # selected. The same recipe must be discoverable before that choice.
        assert availability["available"] is (not helper_selected)
        requirements = availability["source_authoring"]
        assert requirements["scope"] == "selected_runtime_source_authoring"
        assert requirements["candidate_supported"] is (not helper_selected)
        assert requirements["caller_bundle"]["operation"] == "find_and_install"
        # The handoff recipe must survive selection of a helper, including a
        # helper whose caller has not yet been repaired by this guide turn.
        handoff = handoff_guidance(requirements)
        assert "intact" in handoff and "Connect setup agent" in handoff
        assert "No organization catalog, document parser" in handoff
        schema = (await author.inspect({"include_schema": True}))["schema"]
        assert "rust_bundles" in schema["$defs"]["ToolGrants"]["properties"]
        for topic in ("all", "capabilities"):
            reference = await author.read_design_reference({"topic": topic})
            assert reference["package_availability"] == availability
            reference["package_availability"]["source_authoring"]["caller_bundle"]["value"] = "changed-return-value"
            assert guide_context["package_availability"] == availability
        caller = requirements["caller_bundle"]
        assert caller["field"] == "agent_classes[].tools.rust_bundles"
        edits = [{"op": "add", "path": "/blueprint/agent_classes/0/tools/rust_bundles",
            "value": [caller["value"]]}]
        if helper_selected:
            for key, value in requirements["acquisition"].items():
                edits.append({"op": "replace", "path": "/blueprint/acquisition/" + key, "value": value})
        else:
            # This controlled author chooses the role through the real edit
            # tool. It does not prove that an LLM makes the same design choice.
            edits.extend([
                {"op": "add", "path": "/blueprint/agent_classes/1", "value": desired_helper},
                {"op": "add", "path": "/blueprint/acquisition",
                    "value": {**desired_acquisition, **requirements["acquisition"]}},
            ])
        result = await author.publish_guide_edit(edit_args(author, edits))
        assert result["valid"], result
        assert result["checks"][0]["status"] == "not-evaluated"
        assert result["checks"][0]["scope"] == "unselected_runtime_preview"
        assert "not ready in this Forge build" not in str(result)
        observed.update(requirements=deepcopy(requirements), result=deepcopy(result))
        return {"guide": author.guide_result, "proposal": author.proposal["blueprint"]}

    monkeypatch.setattr(agent, "message", author_turn)
    response = client.post("/api/guide/" + saved["id"] + "/respond", json={
        "version": saved["version"], "action": "chat",
        "text": "Let the request owner start the supported connection setup; preserve the team."})
    assert response.status_code == 200, response.text
    shown = client.get("/api/guide/" + saved["id"]).json()
    assert shown["package_availability"]["available"] is True
    assert shown["package_availability"]["source_authoring"]["candidate_supported"] is True
    assert shown["organization"]["guide_context"] == document
    assert shown["organization"] == before["organization"]
    # This is the now-supported helper/caller plan after real atomic publication.
    assert handoff_guidance(observed["requirements"]) == handoff_guidance(
        shown["package_availability"]["source_authoring"])
    assert shown["package_availability"]["blueprint_hash"] == fingerprint(shown["proposal"])
    assert shown["proposal"]["agent_classes"][0]["tools"]["rust_bundles"] == [
        observed["requirements"]["caller_bundle"]["value"]]
    expected = deepcopy(plan)
    expected["agent_classes"][0]["tools"]["rust_bundles"] = shown["proposal"]["agent_classes"][0]["tools"]["rust_bundles"]
    if not helper_selected:
        expected["agent_classes"].insert(1, desired_helper)
        expected["acquisition"] = {**desired_acquisition, **observed["requirements"]["acquisition"]}
    assert shown["proposal"] == Blueprint.model_validate(expected).model_dump(mode="json")
    assert shown["blueprint"] == before["blueprint"]
    assert shown["proposal"]["agent_classes"][1]["instances"] == 1
    if declared_classes:
        project = next(row for row in shown["proposal"]["agent_classes"] if row["id"] == "project")
        assert project["instances"] == 0 and project["members"] == []
    assert before["proposal"] == plan
    assert "package_availability" not in service.store.get(saved["id"])
    # Recompute source support with the actual compiler and original target;
    # neither the tool result nor old guide readiness authorizes this candidate.
    assert compiler.package_availability(shown["proposal"], runtime_target=selected_runtime)["available"] is True


@pytest.mark.parametrize("defect", ["caller-missing", "helper-only", "other-bundle", "helper-profile", "helper-tools", "future-only-caller"])
def test_changed_candidate_cannot_reuse_previous_ready_authoring(product, defect, tmp_path):
    target = catalog_target(product)
    previous = setup_spec()
    ready = compiler.package_availability(previous, runtime_target=target)
    assert ready["available"] is True
    changed = deepcopy(previous)
    if defect in {"caller-missing", "helper-only"}:
        changed["agent_classes"][0]["tools"].pop("rust_bundles")
        if defect == "helper-only":
            changed["agent_classes"][1]["tools"]["rust_bundles"] = [ready["source_authoring"]["caller_bundle"]["value"]]
    elif defect == "other-bundle":
        changed["agent_classes"][0]["tools"]["rust_bundles"].append("unselected.bundle")
    elif defect == "helper-profile": changed["acquisition"]["helper_profile"] = "exclusive_application_role"
    elif defect == "helper-tools": changed["acquisition"]["helper_tools"] = "preparation"
    else:
        # A future class may carry desired setup tools, but cannot replace
        # the actual initial caller required by the selected composition.
        future = {**deepcopy(changed["agent_classes"][0]), "id": "future", "instances": 0, "members": []}
        changed["agent_classes"][0]["tools"].pop("rust_bundles")
        changed["agent_classes"].append(future)
    author = ArchitectService(tmp_path)
    author.set_context(previous)
    author.guide_context = context(previous, package_availability=ready)
    _, preview = author._compile_candidate({"blueprint": changed})
    assert preview["checks"][0]["status"] == "not-evaluated"
    current = compiler.package_availability(changed, runtime_target=target)
    assert current["available"] is False
    assert current["source_authoring"]["candidate_supported"] is False
    assert ready["available"] is True
    with pytest.raises(ValueError):
        compiler.generate_package(changed, runtime_target=target)


def test_no_selected_artifacts_means_no_selected_authoring_contract(guide_client, product):
    client, agent, service = guide_client
    target = catalog_target(product)
    plan = setup_spec()
    saved = saved_plan(client, service, plan)
    unselected = client.get("/api/guide/" + saved["id"]).json()["package_availability"]
    assert unselected["available"] is False and "source_authoring" not in unselected
    service.package_runtime_provider = lambda state: target
    checked = client.get("/api/guide/" + saved["id"]).json()["package_availability"]
    assert checked["available"] is True and "source_authoring" in checked
    target.assembly.path.write_bytes(b"{}\n")
    corrupted = client.get("/api/guide/" + saved["id"]).json()["package_availability"]
    assert corrupted["available"] is False and corrupted["status"] == "runtime-unavailable"
    assert "source_authoring" not in corrupted
    assert service.store.get(saved["id"])["proposal"] == plan
    assert agent.calls == []


def test_authoring_contract_cannot_be_supplied_as_runtime_selection(product):
    target = catalog_target(product)
    plan = setup_spec()
    contract = compiler.package_availability(plan, runtime_target=target)["source_authoring"]
    untrusted = compiler.package_availability(plan, runtime_target=contract)
    assert untrusted["available"] is False and "source_authoring" not in untrusted
    with pytest.raises(ValueError):
        compiler.generate_package(plan, runtime_target=contract)
