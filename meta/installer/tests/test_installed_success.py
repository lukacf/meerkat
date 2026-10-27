"""Durable human reports have no builder, runtime or private-domain authority."""
from copy import deepcopy
import hashlib
import json
import os
from pathlib import Path
import uuid
from unittest.mock import AsyncMock
import fcntl

import httpx
import pytest

from launchkit import installed_feedback
from launchkit.deployment import read_receipt, update_receipt
from launchkit.projects import DraftConflict
from launchkit.remote_deployment import update_receipt as remote_update
from test_installed_feedback import prepared as prepare_local, PRIVATE
from test_remote_installed_feedback import prepared as prepare_remote


TASK = "Read my saved plans and show the useful result."
NOT_ADMITTED_DETAIL = "The setup changed or is still being revised. Reload before reporting a result."


async def prepared(tmp_path, kind):
    if kind == "local":
        service, directory, destination = await prepare_local(tmp_path)
    else:
        service, directory = await prepare_remote(tmp_path)
        destination = None
    return service, directory, destination


def body(service, directory, *, task=TASK):
    receipt = json.loads((directory / "receipt.json").read_bytes())
    source = receipt.get("accepted_source_digest")
    if source is None:
        _, files = installed_feedback.checked_package(directory, "package.zip", receipt["package_sha256"])
        source = json.loads(files["implementation-report.json"][0])["verification"]["digest"]
    return {"version": service.guide.state["version"], "deployment_id": receipt["deployment_id"],
        "build_id": receipt["build_id"], "source_digest": source, "package_sha256": receipt["package_sha256"],
        "request_id": str(uuid.uuid4()), "task": task}


async def save(service, value):
    return await service.record_success(service.guide.identity, **value)


def public_bytes(path):
    return {str(item.relative_to(path)): item.read_bytes() for item in path.rglob("*")
            if item.is_file() and "human-success-reports" not in item.relative_to(path).parts}


def report_bytes(path):
    return {item.name: item.read_bytes() for item in path.glob("*.json")}


@pytest.mark.parametrize("kind", ["local", "remote"])
@pytest.mark.parametrize("history", ["absent-directory", "empty-directory", "previous-report"])
@pytest.mark.parametrize("change", ["guide-revision", "guide-operation"])
async def test_never_delivered_stale_success_has_exact_absence_witness(tmp_path, kind, history, change):
    service, directory, destination = await prepared(tmp_path, kind)
    value = body(service, directory)
    root = directory / "human-success-reports"
    if history == "empty-directory":
        root.mkdir(mode=0o700)
    elif history == "previous-report":
        await save(service, body(service, directory, task="An earlier completed job."))
    before = report_bytes(root)
    custody = (directory / "receipt.json").read_bytes()
    installed = public_bytes(destination) if destination else None
    calls = deepcopy(service.transport.calls) if kind == "remote" else None
    if change == "guide-revision":
        service.guide.state["version"] += 1
    else:
        service.guide.state["operation"] = {"status": "running"}
    with pytest.raises(installed_feedback.InstalledSuccessNotAdmitted) as caught:
        await save(service, value)
    assert isinstance(caught.value, DraftConflict)
    assert caught.value.code == "installed_success_not_admitted" and caught.value.status_code == 409
    assert str(caught.value) == NOT_ADMITTED_DETAIL
    assert not (root / (value["request_id"] + ".json")).exists()
    assert report_bytes(root) == before and (directory / "receipt.json").read_bytes() == custody
    if history == "absent-directory":
        assert not root.exists()
    if destination:
        assert public_bytes(destination) == installed
    else:
        assert service.transport.calls == calls
    service.implementation.submit.assert_not_awaited()


@pytest.mark.parametrize("kind", ["local", "remote"])
@pytest.mark.parametrize("entry", ["directory", "pointer", "request"])
@pytest.mark.parametrize("failure_type", [PermissionError, OSError])
async def test_unknown_success_entry_presence_never_becomes_absence_witness(tmp_path, kind, entry, failure_type, monkeypatch):
    service, directory, _ = await prepared(tmp_path, kind)
    value = body(service, directory)
    root = directory / "human-success-reports"
    if entry != "directory":
        await save(service, body(service, directory, task="An earlier completed job."))
    target = root if entry == "directory" else root / ("latest.json" if entry == "pointer" else value["request_id"] + ".json")
    before = report_bytes(root)
    service.guide.state["version"] += 1
    original = Path.lstat
    def unavailable(path, *args, **kwargs):
        if path == target:
            raise failure_type("private-entry-error-canary")
        return original(path, *args, **kwargs)
    monkeypatch.setattr(Path, "lstat", unavailable)
    with pytest.raises(DraftConflict) as caught:
        await save(service, value)
    assert not isinstance(caught.value, installed_feedback.InstalledSuccessNotAdmitted)
    assert "private-entry-error-canary" not in str(caught.value)
    monkeypatch.undo()
    assert report_bytes(root) == before
    assert not (root / (value["request_id"] + ".json")).exists()
    service.implementation.submit.assert_not_awaited()


@pytest.mark.parametrize("kind", ["local", "remote"])
@pytest.mark.parametrize("corruption", ["directory-symlink", "directory-mode", "request-json", "request-symlink", "request-hardlink"])
async def test_unverified_report_root_or_exact_uuid_never_has_absence_witness(tmp_path, kind, corruption):
    service, directory, _ = await prepared(tmp_path, kind)
    value = body(service, directory)
    await save(service, value)
    root = directory / "human-success-reports"
    record = root / (value["request_id"] + ".json")
    if corruption == "directory-symlink":
        retained = directory / "retained-success-reports"
        root.rename(retained)
        root.symlink_to(retained, target_is_directory=True)
    elif corruption == "directory-mode":
        root.chmod(0o755)
    else:
        (root / "latest.json").unlink()
        if corruption == "request-json":
            record.write_text("not JSON private-record-canary")
        elif corruption == "request-symlink":
            retained = root / "retained.json"
            record.rename(retained)
            record.symlink_to(retained)
        else:
            os.link(record, root / "retained.json")
    before = report_bytes(root)
    service.guide.state["version"] += 1
    with pytest.raises(DraftConflict) as caught:
        await save(service, value)
    assert not isinstance(caught.value, installed_feedback.InstalledSuccessNotAdmitted)
    assert "private-record-canary" not in str(caught.value)
    assert report_bytes(root) == before
    service.implementation.submit.assert_not_awaited()


@pytest.mark.parametrize("kind", ["local", "remote"])
async def test_report_records_exact_task_and_source_without_candidate_or_runtime_or_private_data(tmp_path, kind, monkeypatch):
    service, directory, destination = await prepared(tmp_path, kind)
    value = body(service, directory)
    before, installed = public_bytes(service.root), public_bytes(destination) if destination else None
    calls = deepcopy(service.transport.calls) if kind == "remote" else None
    service.implementation.status = AsyncMock(side_effect=AssertionError("no candidate lookup"))
    service.implementation.submit = AsyncMock(side_effect=AssertionError("no build submission"))
    if kind == "local":
        service.guide_status = AsyncMock(side_effect=AssertionError("no lifecycle observation"))
        service.status = AsyncMock(side_effect=AssertionError("no OS status"))
        service.service_manager.command = AsyncMock(side_effect=AssertionError("no OS command"))
    original = Path.open
    def deny_domain(path, *args, **kwargs):
        if path.name == ".env" or "data" in path.parts or "workspace" in path.parts:
            raise AssertionError("never read private application data")
        return original(path, *args, **kwargs)
    monkeypatch.setattr(Path, "open", deny_domain)
    report = await save(service, value)
    monkeypatch.undo()
    assert report == {**value, "guide_id": service.guide.identity, "recorded_at": report["recorded_at"],
        "scope": "human-reported-installed-success", "outcome": "worked", "application_outcomes_verified": False}
    assert report["task"] == TASK and PRIVATE not in json.dumps(report)
    assert public_bytes(service.root) == before
    if destination:
        assert public_bytes(destination) == installed
    else:
        assert service.transport.calls == calls
    service.implementation.status.assert_not_awaited()
    service.implementation.submit.assert_not_awaited()
    root = directory / "human-success-reports"
    assert root.stat().st_mode & 0o777 == 0o700
    assert all(path.stat().st_mode & 0o777 == 0o600 for path in root.iterdir())
    assert all(path.stat().st_nlink == 1 for path in root.iterdir())


@pytest.mark.parametrize("kind", ["local", "remote"])
async def test_exact_replay_survives_guide_revision_and_conflict_does_not_leak_previous_task(tmp_path, kind):
    service, directory, _ = await prepared(tmp_path, kind)
    value = body(service, directory, task="Private human wording for this task.")
    original = await save(service, value)
    before = public_bytes(directory / "human-success-reports")
    service.guide.state["version"] += 1
    assert await save(service, value) == original
    assert public_bytes(directory / "human-success-reports") == before
    with pytest.raises(DraftConflict, match="already used") as error:
        await save(service, {**value, "task": "A different task."})
    assert original["task"] not in str(error.value)
    assert not isinstance(error.value, installed_feedback.InstalledSuccessNotAdmitted)
    with pytest.raises(installed_feedback.InstalledSuccessNotAdmitted, match="setup changed"):
        await save(service, {**value, "request_id": str(uuid.uuid4())})


@pytest.mark.parametrize("kind", ["local", "remote"])
@pytest.mark.parametrize("unavailable", [LookupError("missing"), OSError("unreadable"), ValueError("bad candidate")])
async def test_success_and_readback_remain_available_without_a_current_candidate(tmp_path, kind, unavailable):
    service, directory, _ = await prepared(tmp_path, kind)
    report = await save(service, body(service, directory))
    service.implementation.status = AsyncMock(side_effect=unavailable)
    service.guide.state["version"] += 1
    result = await service.use(service.guide.identity)
    assert result["success_report"] == report
    assert result["success_report"]["version"] == result["version"] - 1
    assert result["continuation"]["available"] is False
    assert result["application_outcomes_verified"] is False
    service.implementation.submit.assert_not_awaited()


@pytest.mark.parametrize("kind", ["local", "remote"])
@pytest.mark.parametrize("field,value", [("version", 99), ("deployment_id", str(uuid.uuid4())),
    ("build_id", "different-build"), ("source_digest", "b" * 64), ("package_sha256", "b" * 64)])
async def test_new_reports_reject_stale_guide_installation_and_source_without_a_builder(tmp_path, kind, field, value):
    service, directory, _ = await prepared(tmp_path, kind)
    with pytest.raises(DraftConflict) as caught:
        await save(service, {**body(service, directory), field: value})
    assert isinstance(caught.value, installed_feedback.InstalledSuccessNotAdmitted) is (field == "version")
    assert not (directory / "human-success-reports").exists()
    service.implementation.submit.assert_not_awaited()


@pytest.mark.parametrize("kind", ["local", "remote"])
@pytest.mark.parametrize("changes", [{"status": "starting"}, {"status": "recovery-needed"},
    {"status": "running", "phase": "update", "update": {"status": "applying"}}])
async def test_mutating_installation_blocks_new_reports_and_replays(tmp_path, kind, changes):
    service, directory, _ = await prepared(tmp_path, kind)
    value = body(service, directory)
    await save(service, value)
    (remote_update if kind == "remote" else update_receipt)(directory, **changes)
    before = public_bytes(directory / "human-success-reports")
    with pytest.raises(DraftConflict) as caught:
        await save(service, value)
    assert not isinstance(caught.value, installed_feedback.InstalledSuccessNotAdmitted)
    assert public_bytes(directory / "human-success-reports") == before


@pytest.mark.parametrize("kind", ["local", "remote"])
@pytest.mark.parametrize("field,value", [("version", True), ("request_id", "../escape"),
    ("request_id", str(uuid.uuid4()).upper()), ("task", " "), ("task", "x" * 3001),
    ("task", "hidden\x00value"), ("task", "\ud800"), ("source_digest", "A" * 64)])
async def test_report_input_is_strict_and_bounded(tmp_path, kind, field, value):
    service, directory, _ = await prepared(tmp_path, kind)
    with pytest.raises(ValueError):
        await save(service, {**body(service, directory), field: value})
    assert not (directory / "human-success-reports").exists()


@pytest.mark.parametrize("kind", ["local", "remote"])
@pytest.mark.parametrize("prior", [False, True])
@pytest.mark.parametrize("failure_type", [DraftConflict, ValueError, OSError])
async def test_crash_before_pointer_is_repaired_by_exact_retry_without_replacing_a_newer_report(tmp_path, kind, prior, failure_type, monkeypatch):
    service, directory, _ = await prepared(tmp_path, kind)
    if prior:
        await save(service, body(service, directory, task="An earlier completed job."))
    value = body(service, directory)
    original = installed_feedback._write_report
    def pointer_fails(path, report, **kwargs):
        if path.name == "latest.json":
            raise failure_type("controlled failed pointer write")
        return original(path, report, **kwargs)
    monkeypatch.setattr(installed_feedback, "_write_report", pointer_fails)
    with pytest.raises(failure_type) as caught:
        await save(service, value)
    assert not isinstance(caught.value, installed_feedback.InstalledSuccessNotAdmitted)
    assert (directory / "human-success-reports" / (value["request_id"] + ".json")).exists()
    monkeypatch.setattr(installed_feedback, "_write_report", original)
    service.guide.state["version"] += 1
    recovered = await save(service, value)
    assert (await service.use(service.guide.identity))["success_report"] == recovered
    newer = await save(service, body(service, directory, task="A newer completed job."))
    assert await save(service, value) == recovered
    assert (await service.use(service.guide.identity))["success_report"] == newer


@pytest.mark.parametrize("kind", ["local", "remote"])
@pytest.mark.parametrize("corruption", ["bad-json", "oversized", "symlink", "hardlink", "binding"])
async def test_corrupt_or_unowned_success_history_is_preserved_and_ordinary_use_has_safe_fallback(tmp_path, kind, corruption):
    service, directory, _ = await prepared(tmp_path, kind)
    value = body(service, directory)
    await save(service, value)
    root = directory / "human-success-reports"
    pointer = root / "latest.json"
    if corruption == "bad-json":
        pointer.write_text("not JSON private-test-canary")
    elif corruption == "oversized":
        pointer.write_bytes(b" " * 4097)
    elif corruption == "symlink":
        original = pointer.read_bytes()
        pointer.unlink()
        outsider = tmp_path / "outside.json"
        outsider.write_bytes(original)
        pointer.symlink_to(outsider)
    elif corruption == "hardlink":
        os.link(pointer, tmp_path / "other-link.json")
    else:
        record = root / (value["request_id"] + ".json")
        saved = json.loads(record.read_bytes())
        saved["binding"]["target"] = "fabricated"
        record.write_bytes(installed_feedback._encoded(saved))
        pointer.write_bytes(installed_feedback._encoded({"request_id": value["request_id"],
            "report_sha256": installed_feedback._sha(saved)}))
    before = pointer.read_bytes()
    service.guide.state["version"] += 1
    with pytest.raises(DraftConflict) as error:
        await save(service, {**value, "request_id": str(uuid.uuid4())})
    assert not isinstance(error.value, installed_feedback.InstalledSuccessNotAdmitted)
    assert "private-test-canary" not in str(error.value)
    assert pointer.read_bytes() == before
    result = await service.use(service.guide.identity)
    assert result["success_report"] is None and result["success_report_detail"]
    assert "private-test-canary" not in result["success_report_detail"]


@pytest.mark.parametrize("kind", ["local", "remote"])
async def test_existing_operation_lock_returns_immediate_retry_conflict_without_modifying_lock(tmp_path, kind):
    service, directory, _ = await prepared(tmp_path, kind)
    value = body(service, directory)
    descriptor = os.open(directory / "journal.lock", os.O_RDWR)
    try:
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
        with pytest.raises(DraftConflict, match="being updated") as caught:
            await save(service, value)
        assert not isinstance(caught.value, installed_feedback.InstalledSuccessNotAdmitted)
        assert not (directory / "human-success-reports").exists()
        # Our independent descriptor still owns its lock after the rejection.
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
    finally:
        os.close(descriptor)


async def test_changed_installed_source_and_linked_owner_cannot_accept_replayed_success(tmp_path):
    service, directory, destination = await prepared(tmp_path, "local")
    value = body(service, directory)
    await save(service, value)
    original = (destination / "app.py").read_bytes()
    (destination / "app.py").write_bytes(b"different installed source\n")
    with pytest.raises(DraftConflict, match="could not be verified") as caught:
        await save(service, value)
    assert not isinstance(caught.value, installed_feedback.InstalledSuccessNotAdmitted)
    (destination / "app.py").write_bytes(original)
    os.link(destination / ".mobkit/launch-deployment-owner.json", tmp_path / "owner-hardlink")
    with pytest.raises(DraftConflict, match="could not be verified") as caught:
        await save(service, value)
    assert not isinstance(caught.value, installed_feedback.InstalledSuccessNotAdmitted)


async def test_remote_observation_unavailable_does_not_require_ssh_to_report_a_human_result(tmp_path):
    service, directory, _ = await prepared(tmp_path, "remote")
    value = body(service, directory)
    remote_update(directory, status="observation-unavailable", last_known_status="stopped", observation={"available": False})
    service.transport.request = AsyncMock(side_effect=AssertionError("no SSH admission"))
    report = await save(service, value)
    use = await service.use(service.guide.identity)
    assert use["success_report"] == report and use["success_reporting_available"] is True
    service.transport.request.assert_not_awaited()


@pytest.mark.parametrize("field", ["remote_admission_requested", "admitted"])
@pytest.mark.parametrize("flag", [False, None, "unknown", "true", 1, "absent"])
async def test_remote_success_and_exact_replay_require_literal_saved_historic_admission(tmp_path, field, flag):
    service, directory, _ = await prepared(tmp_path, "remote")
    value = body(service, directory)
    await save(service, value)
    if flag == "absent":
        receipt = json.loads((directory / "receipt.json").read_bytes())
        receipt.pop(field)
        from launchkit.remote_deployment import write_private
        write_private(directory / "receipt.json", receipt)
    else:
        remote_update(directory, **{field: flag})
    before = public_bytes(directory / "human-success-reports")
    for request in (value, {**value, "request_id": str(uuid.uuid4())}):
        with pytest.raises(DraftConflict, match="Install this owned application") as caught:
            await save(service, request)
        assert not isinstance(caught.value, installed_feedback.InstalledSuccessNotAdmitted)
    assert public_bytes(directory / "human-success-reports") == before
    use = await service.use(service.guide.identity)
    assert use["success_reporting_available"] is False and use["success_reporting_detail"]
    service.implementation.submit.assert_not_awaited()


@pytest.mark.parametrize("kind", ["local", "remote"])
@pytest.mark.parametrize("change", ["same-version-brief", "mutating-installation"])
async def test_final_join_catches_source_or_guide_changes_during_awaits(tmp_path, kind, change):
    service, directory, _ = await prepared(tmp_path, kind)
    value = body(service, directory)
    original = service.guide.get
    reads = 0
    async def changing(identity):
        nonlocal reads
        reads += 1
        if reads == 2:
            if change == "same-version-brief":
                service.guide.state["proposal"]["project"]["name"] = "Same-version changed meaning"
            else:
                (remote_update if kind == "remote" else update_receipt)(directory, status="starting")
        return await original(identity)
    service.guide.get = changing
    with pytest.raises(DraftConflict) as caught:
        await save(service, value)
    assert not isinstance(caught.value, installed_feedback.InstalledSuccessNotAdmitted)
    assert not (directory / "human-success-reports").exists()
    service.implementation.submit.assert_not_awaited()


async def test_local_report_is_not_reused_after_its_owned_destination_is_relocated(tmp_path):
    import shutil
    service, directory, destination = await prepared(tmp_path, "local")
    value = body(service, directory)
    await save(service, value)
    moved = tmp_path / "relocated-application"
    shutil.copytree(destination, moved)
    receipt = update_receipt(directory, destination=str(moved))
    marker = moved / ".mobkit/launch-deployment-owner.json"
    owned = json.loads(marker.read_bytes())
    owned["destination"] = str(moved)
    marker.write_text(json.dumps(owned))
    with pytest.raises(DraftConflict, match="already used") as caught:
        await save(service, value)
    assert not isinstance(caught.value, installed_feedback.InstalledSuccessNotAdmitted)
    result = await service.use(service.guide.identity)
    assert result["success_report"] is None
    assert read_receipt(directory) == receipt


async def post_success(service, kind, value):
    from launchkit import server
    route = "deployment" if kind == "local" else "remote-deployment"
    async with httpx.AsyncClient(transport=httpx.ASGITransport(app=server.app), base_url="http://testserver") as client:
        return await client.post(f"/api/guide/{service.guide.identity}/{route}/use/success", json=value)


@pytest.mark.parametrize("kind", ["local", "remote"])
@pytest.mark.parametrize("history", ["absent-directory", "previous-report"])
async def test_real_success_http_routes_map_verified_absence_and_allow_a_fresh_report(tmp_path, kind, history, monkeypatch):
    from launchkit import server
    service, directory, _ = await prepared(tmp_path, kind)
    value = body(service, directory)
    root = directory / "human-success-reports"
    if history == "previous-report":
        await save(service, body(service, directory, task="An earlier completed job."))
    before = report_bytes(root)
    calls = deepcopy(service.transport.calls) if kind == "remote" else None
    service.guide.state["version"] += 1
    monkeypatch.setattr(server, "deployment" if kind == "local" else "remote_deployment", service)
    response = await post_success(service, kind, value)
    assert response.status_code == 409
    assert response.json() == {"detail": NOT_ADMITTED_DETAIL, "code": "installed_success_not_admitted"}
    assert response.headers["cache-control"] == "no-store"
    assert response.headers["referrer-policy"] == "no-referrer"
    assert report_bytes(root) == before and not (root / (value["request_id"] + ".json")).exists()
    if history == "absent-directory":
        assert not root.exists()
    fresh = {**value, "version": service.guide.state["version"], "request_id": str(uuid.uuid4())}
    accepted = await post_success(service, kind, fresh)
    assert accepted.status_code == 200
    assert accepted.json() == {**fresh, "guide_id": service.guide.identity,
        "recorded_at": accepted.json()["recorded_at"], "scope": installed_feedback.SUCCESS_SCOPE,
        "outcome": "worked", "application_outcomes_verified": False}
    assert not (root / (value["request_id"] + ".json")).exists()
    service.implementation.submit.assert_not_awaited()
    if kind == "remote":
        assert service.transport.calls == calls


@pytest.mark.parametrize("kind", ["local", "remote"])
async def test_real_success_http_replay_survives_revision_but_uuid_conflict_has_no_witness(tmp_path, kind, monkeypatch):
    from launchkit import server
    service, directory, _ = await prepared(tmp_path, kind)
    value = body(service, directory)
    original = await save(service, value)
    before = report_bytes(directory / "human-success-reports")
    service.guide.state["version"] += 1
    monkeypatch.setattr(server, "deployment" if kind == "local" else "remote_deployment", service)
    response = await post_success(service, kind, value)
    assert response.status_code == 200 and response.json() == original
    conflict = await post_success(service, kind, {**value, "task": "A different completed job."})
    assert conflict.status_code == 409
    assert conflict.json() == {"detail": "This success-report request ID was already used for a different report."}
    assert report_bytes(directory / "human-success-reports") == before
    service.implementation.submit.assert_not_awaited()


@pytest.mark.parametrize("kind", ["local", "remote"])
@pytest.mark.parametrize("refusal", ["source-custody", "corrupt-history", "active-lifecycle-lock"])
async def test_real_success_http_guard_failures_are_uncoded_even_for_an_absent_stale_request(tmp_path, kind, refusal, monkeypatch):
    from launchkit import server
    service, directory, _ = await prepared(tmp_path, kind)
    value = body(service, directory)
    root = directory / "human-success-reports"
    descriptor = None
    if refusal == "source-custody":
        value["source_digest"] = "b" * 64
    elif refusal == "corrupt-history":
        await save(service, body(service, directory, task="An earlier completed job."))
        (root / "latest.json").write_text("not JSON private-history-canary")
    else:
        descriptor = os.open(directory / "journal.lock", os.O_RDWR)
        fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
    before = report_bytes(root)
    service.guide.state["version"] += 1
    monkeypatch.setattr(server, "deployment" if kind == "local" else "remote_deployment", service)
    try:
        response = await post_success(service, kind, value)
        assert response.status_code == 409 and set(response.json()) == {"detail"}
        assert "private-history-canary" not in response.text
        assert report_bytes(root) == before and not (root / (value["request_id"] + ".json")).exists()
        if descriptor is not None:
            fcntl.flock(descriptor, fcntl.LOCK_EX | fcntl.LOCK_NB)
    finally:
        if descriptor is not None:
            os.close(descriptor)
    service.implementation.submit.assert_not_awaited()


@pytest.mark.parametrize("kind", ["local", "remote"])
@pytest.mark.parametrize("failure_type,status", [(DraftConflict, 409), (ValueError, 422), (OSError, 502)])
async def test_real_success_http_failure_after_record_publication_has_no_witness_and_exact_replay_recovers(tmp_path, kind, failure_type, status, monkeypatch):
    from launchkit import server
    service, directory, _ = await prepared(tmp_path, kind)
    value = body(service, directory)
    root = directory / "human-success-reports"
    original = installed_feedback._write_report
    def pointer_fails(path, report, **kwargs):
        if path.name == "latest.json":
            raise failure_type("The report pointer could not be published.")
        return original(path, report, **kwargs)
    monkeypatch.setattr(installed_feedback, "_write_report", pointer_fails)
    monkeypatch.setattr(server, "deployment" if kind == "local" else "remote_deployment", service)
    response = await post_success(service, kind, value)
    assert response.status_code == status and set(response.json()) == {"detail"}
    record = root / (value["request_id"] + ".json")
    published = record.read_bytes()
    assert not (root / "latest.json").exists()
    monkeypatch.setattr(installed_feedback, "_write_report", original)
    service.guide.state["version"] += 1
    replay = await post_success(service, kind, value)
    assert replay.status_code == 200
    assert replay.json() == json.loads(published)["report"] and record.read_bytes() == published
    assert (await service.use(service.guide.identity))["success_report"] == replay.json()
    service.implementation.submit.assert_not_awaited()


@pytest.mark.parametrize("kind", ["local", "remote"])
@pytest.mark.parametrize("boundary", ["link", "fsync"])
async def test_failure_after_link_or_record_fsync_has_no_absence_witness(tmp_path, kind, boundary, monkeypatch):
    service, directory, _ = await prepared(tmp_path, kind)
    value = body(service, directory)
    root = directory / "human-success-reports"
    root.mkdir(mode=0o700)
    record = root / (value["request_id"] + ".json")
    if boundary == "link":
        original = installed_feedback.os.link
        def linked_then_fails(source, target, **kwargs):
            original(source, target, **kwargs)
            if target == record:
                raise OSError("controlled post-link failure")
        monkeypatch.setattr(installed_feedback.os, "link", linked_then_fails)
    else:
        original = installed_feedback._fsync_directory
        def synced_then_fails(path):
            original(path)
            if path == root:
                raise OSError("controlled record fsync failure")
        monkeypatch.setattr(installed_feedback, "_fsync_directory", synced_then_fails)
    with pytest.raises(OSError) as caught:
        await save(service, value)
    assert not isinstance(caught.value, installed_feedback.InstalledSuccessNotAdmitted)
    published = record.read_bytes()
    assert record.stat().st_nlink == 1 and not (root / "latest.json").exists()
    monkeypatch.undo()
    service.guide.state["version"] += 1
    recovered = await save(service, value)
    assert recovered == json.loads(published)["report"] and record.read_bytes() == published
    service.implementation.submit.assert_not_awaited()
