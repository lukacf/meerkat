"""Source and observation bindings for inert application-trial snapshots."""
import copy
import hashlib
import json
from types import SimpleNamespace
import uuid

import pytest

from launchkit.application_preview import sanitize_application_preview
from launchkit.artifact_binding import proposal_hash
from launchkit.blueprint import starter_blueprint
from launchkit.implementation import read_receipt, update_receipt
from launchkit.projects import fingerprint
from launchkit.rehearsal import RehearsalService, application_preview_projection
from launchkit.templates.storage_setup import write_private


@pytest.fixture
def bound_preview():
    guide_id, trial_id, build_id = (str(uuid.uuid4()) for _ in range(3))
    blueprint = starter_blueprint().model_dump(mode="json")
    brief = {"requirements": [{"id": "collection", "text": "Keep the fictional collection."}]}
    source = {"build_id": build_id, "source_digest": "a" * 64, "deployment": {"agents": []}}
    state = {"id": guide_id, "version": 3, "proposal": blueprint, "brief": brief}
    frozen = {"guide_id": guide_id, "guide_version": 3, "blueprint": blueprint,
              "brief": brief, "mode": "application", "application_source": source}
    preview = {**sanitize_application_preview({"title": "Sample collection", "html": "<main>One fictional item</main>"}),
        "trial_id": trial_id, "build_id": build_id, "source_digest": source["source_digest"],
        "guide_version": 3, "observation_revision": 3}
    receipt = {"guide_id": guide_id, "guide_version": 3, "rehearsal_id": trial_id,
        "blueprint_hash": proposal_hash(blueprint), "brief_hash": fingerprint(brief),
        "mode": "application", "status": "completed", "current": True,
        "worker_stop_confirmed": True, "application_stop_confirmed": True,
        "application_source": source, "application_observation_revision": 3,
        "application_observations": [{"kind": "callback_returned", "observation_revision": 3,
                                      "result": {"count": 1}}], "application_preview": preview}
    return receipt, frozen, state


def test_projection_retains_only_inert_exact_bound_snapshot_without_promoting_outcomes(bound_preview):
    receipt, frozen, state = bound_preview
    receipt["application_preview"].update(private_path="PRIVATE_CANARY", settings={"SECRET": "PRIVATE_CANARY"})
    before = copy.deepcopy((receipt, frozen, state))
    value = application_preview_projection(receipt, frozen, state)
    assert value["html"] == receipt["application_preview"]["html"]
    assert value["document_sha256"] == hashlib.sha256(value["html"].encode()).hexdigest()
    assert value["observation_revision"] == 3 and value["live_ready"] is False
    assert value["scope"] == "offline-application-preview" and "PRIVATE_CANARY" not in json.dumps(value)
    assert "connected" not in value and "application_outcomes_verified" not in value
    assert (receipt, frozen, state) == before


@pytest.mark.parametrize("changed", ["trial", "build", "digest", "version", "bool_version", "revision", "bool_revision",
                                     "current", "mode", "frozen_source", "proposal", "brief", "provenance"])
def test_projection_rejects_wrong_trial_source_revision_or_design(bound_preview, changed):
    receipt, frozen, state = copy.deepcopy(bound_preview)
    preview = receipt["application_preview"]
    if changed == "trial": preview["trial_id"] = str(uuid.uuid4())
    elif changed == "build": preview["build_id"] = str(uuid.uuid4())
    elif changed == "digest": preview["source_digest"] = "b" * 64
    elif changed == "version": preview["guide_version"] = 2
    elif changed == "bool_version": preview["guide_version"] = True
    elif changed == "revision": receipt["application_observation_revision"] = 4
    elif changed == "bool_revision": receipt["application_observation_revision"] = True
    elif changed == "current": receipt["current"] = False
    elif changed == "mode": receipt["mode"] = "design"
    elif changed == "frozen_source": frozen["application_source"] = {**frozen["application_source"], "build_id": "other"}
    elif changed == "proposal": state["proposal"] = {**state["proposal"], "description": "A different requested design"}
    elif changed == "brief": state["brief"] = {"requirements": [{"id": "different"}]}
    else: frozen.pop("guide_version")
    assert application_preview_projection(receipt, frozen, state) is None


@pytest.mark.parametrize("status", ["unsupported", "failed"])
def test_unavailable_preview_does_not_expose_authored_errors_or_claim_cleanup(bound_preview, status):
    receipt, frozen, state = bound_preview
    bindings = {key: receipt["application_preview"][key]
        for key in ("trial_id", "build_id", "source_digest", "guide_version", "observation_revision")}
    receipt["application_preview"] = {**bindings, "status": status, "scope": "offline-application-preview",
        "sanitization": "inert-html-v1", "live_ready": False, "html": None, "title": "PRIVATE_CANARY",
        "detail": "/private/PRIVATE_CANARY", "process_stop_confirmed": True}
    value = application_preview_projection(receipt, frozen, state)
    assert value["status"] == status and value["html"] is None and value["title"] is None
    assert "PRIVATE_CANARY" not in json.dumps(value) and "process_stop_confirmed" not in value
    assert value["live_ready"] is False


@pytest.mark.parametrize("field,value", [("html", ""), ("html", "<main>Unexpected document</main>"),
                                         ("document_sha256", "a" * 64), ("live_ready", True),
                                         ("scope", "production"), ("sanitization", "unverified")])
def test_unavailable_copy_normalization_cannot_erase_a_document_or_safety_marker(bound_preview, field, value):
    receipt, frozen, state = bound_preview
    bindings = {key: receipt["application_preview"][key]
        for key in ("trial_id", "build_id", "source_digest", "guide_version", "observation_revision")}
    receipt["application_preview"] = {**bindings, "status": "failed", "scope": "offline-application-preview",
        "sanitization": "inert-html-v1", "live_ready": False, "html": None, "title": "PRIVATE_CANARY",
        "detail": "/private/PRIVATE_CANARY", field: value}
    assert application_preview_projection(receipt, frozen, state) is None


def test_rehashed_active_document_is_rejected_instead_of_relying_on_digest(bound_preview):
    receipt, frozen, state = bound_preview
    preview = receipt["application_preview"]
    preview["html"] = preview["html"].replace("</body>", "<script>alert('PRIVATE_CANARY')</script></body>")
    preview["document_sha256"] = hashlib.sha256(preview["html"].encode()).hexdigest()
    assert application_preview_projection(receipt, frozen, state) is None


def service_fixture(tmp_path, bound_preview, status):
    receipt, frozen, state = bound_preview
    async def get(identity):
        assert identity == state["id"]
        return copy.deepcopy(state)
    service = RehearsalService(tmp_path, SimpleNamespace(get=get), implementation=SimpleNamespace(status=status))
    directory = service._directory(state["id"], receipt["rehearsal_id"])
    directory.mkdir(parents=True)
    write_private(directory / "input.json", frozen)
    write_private(directory / "receipt.json", receipt)
    return service, directory, state


def current_build(receipt, state):
    return {"guide_id": state["id"], "guide_version": receipt["guide_version"], "current": True,
        "current_guide_version": state["version"], "build_id": receipt["application_source"]["build_id"],
        "source_digest": receipt["application_source"]["source_digest"], "status": "completed", "source_verified": True}


async def test_service_hides_snapshot_when_exact_checked_build_changes(tmp_path, bound_preview):
    receipt, frozen, state = bound_preview
    async def status(identity, *, state):
        return {**current_build(receipt, state), "source_digest": "b" * 64}
    service, directory, state = service_fixture(tmp_path, bound_preview, status)
    before = (directory / "receipt.json").read_bytes()
    result = await service.status(state["id"], directory.name)
    assert result["application_preview"] is None and result["current"] is False
    assert result["application_observations"] == receipt["application_observations"]
    assert result["live_ready"] is False and result["connected"] is False
    assert (directory / "receipt.json").read_bytes() == before


async def test_service_projects_one_atomic_observation_snapshot_across_awaited_source_read(tmp_path, bound_preview):
    receipt, frozen, state = bound_preview
    changed = False
    async def status(identity, *, state):
        nonlocal changed
        if not changed:
            changed = True
            later = {**sanitize_application_preview({"html": "<main>Two fictional items</main>"}),
                **{key: receipt["application_preview"][key]
                   for key in ("trial_id", "build_id", "source_digest", "guide_version")}, "observation_revision": 5}
            update_receipt(directory, application_observation_revision=5, application_preview=later,
                application_observations=[{"kind": "callback_returned", "observation_revision": 5, "result": {"count": 2}}])
        return current_build(receipt, state)
    service, directory, state = service_fixture(tmp_path, bound_preview, status)
    first = await service.status(state["id"], directory.name)
    assert first["application_observation_revision"] == first["application_preview"]["observation_revision"] == 3
    assert first["application_observations"][0]["result"]["count"] == 1
    second = await service.status(state["id"], directory.name)
    assert second["application_observation_revision"] == second["application_preview"]["observation_revision"] == 5
    assert second["application_observations"][0]["result"]["count"] == 2
    assert "Two fictional items" in second["application_preview"]["html"]
    assert read_receipt(directory)["application_observation_revision"] == 5
