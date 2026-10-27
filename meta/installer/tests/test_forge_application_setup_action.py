"""Inert command custody controls for the existing selected application owner.

The host selection and startup admission are stubs. No SDK, native, process,
installation, credentials or runtime are invoked by these controls.
"""
import asyncio
import json
from types import SimpleNamespace
import uuid

import pytest

from launchkit.deployment import DeploymentService
from launchkit.projects import DraftConflict


@pytest.fixture
def owned(tmp_path, monkeypatch):
    guide_id, deployment_id = str(uuid.uuid4()), str(uuid.uuid4())
    owner = DeploymentService(tmp_path, SimpleNamespace(identity=guide_id), None,
        service_manager=object())
    directory = owner._directory(guide_id, deployment_id)
    directory.mkdir(parents=True, mode=0o700)
    receipt = {"guide_id": guide_id, "deployment_id": deployment_id, "status": "stopped"}
    (directory / "receipt.json").write_text(json.dumps(receipt))
    calls = []
    selected = {"kind": "restore", "inert_selection": "original retained input"}

    def selection(guide, deployment):
        assert (guide, deployment) == (guide_id, deployment_id)
        assert owner.lock.locked()
        calls.append("selection")
        return dict(selected)

    async def start(guide, deployment, *, application_startup):
        assert (guide, deployment) == (guide_id, deployment_id)
        assert owner.lock.locked()
        assert application_startup == selected
        calls.append("startup")
        return {**receipt, "status": "queued"}

    async def forbidden(*args, **kwargs):
        pytest.fail("Finish setup must reuse the already locked original startup admission.")

    owner.application_setup_selection = selection
    monkeypatch.setattr(owner, "_start_application_locked", start)
    monkeypatch.setattr(owner, "start_application", forbidden)
    return SimpleNamespace(owner=owner, guide_id=guide_id, deployment_id=deployment_id,
        calls=calls, directory=directory)


def binding(selected, request_id):
    return {"action_request_id": request_id, "guide_id": selected.guide_id,
        "deployment_id": selected.deployment_id, "action": "finish-setup", "update_id": None}


async def finish(selected, request_id, **fields):
    return await selected.owner.action(selected.guide_id, selected.deployment_id,
        "finish-setup", action_request_id=request_id, **fields)


async def test_lost_success_reply_does_not_select_or_start_twice(owned):
    request_id = str(uuid.uuid4())
    result = await finish(owned, request_id)
    assert result["status"] == "queued"
    assert result["action_request"]["phase"] == "settled"
    assert result["action_request"]["outcome"] == "returned"
    repeated = await finish(owned, request_id)
    assert repeated["action_request"] == result["action_request"]
    assert owned.calls == ["selection", "startup"]


@pytest.mark.parametrize("phase", ["pending", "unknown"])
async def test_original_uncertain_request_returns_before_host_selection(owned, phase):
    request_id = str(uuid.uuid4())
    value, claimed = owned.owner._claim_action_request(binding(owned, request_id))
    assert claimed
    if phase == "unknown":
        value["phase"] = phase
        owned.owner._action_request_path(request_id).write_text(json.dumps(value))
    result = await finish(owned, request_id)
    assert result["action_request"]["phase"] == phase
    assert owned.calls == []


@pytest.mark.parametrize("changed", ["guide_id", "deployment_id", "action", "update_id"])
async def test_request_binding_collision_refuses_before_host_selection(owned, changed):
    request_id = str(uuid.uuid4())
    original = binding(owned, request_id)
    original[changed] = str(uuid.uuid4()) if changed != "action" else "restart"
    owned.owner._claim_action_request(original)
    with pytest.raises(DraftConflict, match="different deployment action"):
        await finish(owned, request_id)
    assert owned.calls == []


@pytest.mark.parametrize("fields", [{"action_request_id": None}, {"update_id": "unselected-update"}])
async def test_missing_command_identity_or_update_substitution_refuses_before_selection(owned, fields):
    options = {"action_request_id": str(uuid.uuid4()), **fields}
    with pytest.raises(ValueError, match="exact action request UUID"):
        await owned.owner.action(owned.guide_id, owned.deployment_id, "finish-setup", **options)
    assert owned.calls == []


async def test_selection_failure_settles_existing_request_without_replaying(owned):
    request_id = str(uuid.uuid4())
    def unavailable(*args):
        owned.calls.append("unavailable")
        raise DraftConflict("The original selected inputs are unavailable.")
    owned.owner.application_setup_selection = unavailable
    with pytest.raises(DraftConflict, match="unavailable"):
        await finish(owned, request_id)
    repeated = await finish(owned, request_id)
    assert repeated["action_request"]["phase"] == "settled"
    assert repeated["action_request"]["outcome"] == "error"
    assert owned.calls == ["unavailable"]


async def test_cancelled_original_admission_remains_pending_without_replay(owned, monkeypatch):
    request_id = str(uuid.uuid4())
    async def cancelled(*args, **kwargs):
        owned.calls.append("cancelled")
        raise asyncio.CancelledError
    monkeypatch.setattr(owned.owner, "_start_application_locked", cancelled)
    with pytest.raises(asyncio.CancelledError):
        await finish(owned, request_id)
    repeated = await finish(owned, request_id)
    assert repeated["action_request"]["phase"] == "pending"
    assert owned.calls == ["selection", "cancelled"]
