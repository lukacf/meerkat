"""Installed observations improve a checked candidate without operating a runtime."""
import asyncio
import hashlib
import json
import uuid
from unittest.mock import AsyncMock

import pytest
from fastapi.testclient import TestClient

from launchkit import server
from launchkit.deployment import materialize, read_receipt, update_receipt
from launchkit.installed_feedback import InstalledFeedbackNotAdmitted, InstalledFeedbackConflictNotAdmitted
from launchkit.projects import DraftConflict
from test_deployment import Guide, Implementation, Service, submit


DIGEST = "a" * 64
PRIVATE = "private-test-value-never-harvested"


class CheckedImplementation(Implementation):
    async def status(self, identity, *, state=None):
        result = await super().status(identity)
        result.update(source_verified=True)
        return result


async def prepared(tmp_path, *, extra_files=None):
    guide = Guide()
    implementation = CheckedImplementation(guide)
    implementation.digest = DIGEST
    implementation.definition.update(name="Installed Household", description="The exact installed purpose.",
        entrypoint="lead:main", agents=[{"identity": "lead:main", "name": "Installed Companion"}])
    implementation.extra_files.update(extra_files or {})
    service = Service(tmp_path / "private-server", guide, implementation)
    destination = tmp_path / "installed-app"
    await submit(service, destination, settings={"OPENAI_API_KEY": PRIVATE})
    directory = service._directory(guide.identity)
    materialize(directory)
    await service.shutdown()
    update_receipt(directory, status="stopped")
    (destination / "data").mkdir()
    (destination / "data" / "records.json").write_text(json.dumps({"private-domain": PRIVATE}))
    implementation.submit = AsyncMock(return_value={"build_id": "new-candidate", "status": "queued"})
    return service, directory, destination


async def feedback_body(service):
    value = await service.use(service.guide.identity)
    return {"version": value["version"], **{key: value[key] for key in
        ("deployment_id", "build_id", "source_digest", "package_sha256")},
        "continuation_build_id": value["continuation"]["build_id"],
        "continuation_source_digest": value["continuation"]["source_digest"],
        "request": "Make the first job result easier to understand.", "outcome": "partly",
        "task": "Organize my test collection."}


def evidence_from(request):
    return json.loads(request.split("additional instructions:\n", 1)[1])


async def test_installed_use_reads_the_owned_package_not_a_newer_guide_or_private_files(tmp_path):
    service, _, destination = await prepared(tmp_path)
    service.guide.state["proposal"]["project"]["name"] = "New Guide Title"
    result = await service.use(service.guide.identity)
    assert result["project"] == {"name": "Installed Household", "description": "The exact installed purpose."}
    assert result["entrypoint"] == {"identity": "lead:main", "name": "Installed Companion"}
    assert result["source_digest"] == DIGEST and result["scope"] == "verified-installed-source"
    assert result["version"] == service.guide.state["version"]
    assert result["current"] is False and result["current_guide_version"] is None
    assert result["continuation"]["requires_acknowledgement"] is True
    assert result["application_outcomes_verified"] is False and result["console_url"] is None
    assert PRIVATE not in json.dumps(result) and str(destination) not in json.dumps(result)


async def test_first_jobs_come_from_exact_installed_archive_not_current_candidate_or_offline_trials(tmp_path):
    task = {"label": "Read my saved plans", "request": "Show my saved plans without changing them."}
    content = json.dumps({"tasks": [task]})
    service, directory, destination = await prepared(tmp_path, extra_files={
        "application/first-use.json": content,
        "application/trial.json": json.dumps({"cases": [{"label": "Synthetic approval",
            "request": "Use a synthetic confirmation.", "fixtures": {"principal": PRIVATE}}]}),
        "application/unused.py": "raise AssertionError('never execute installed source')\n"})
    before = {str(path.relative_to(destination)): path.read_bytes()
              for path in destination.rglob("*") if path.is_file()}
    receipt = read_receipt(directory)
    service.implementation.identity, service.implementation.digest = str(uuid.uuid4()), "b" * 64
    service.implementation.extra_files["application/first-use.json"] = json.dumps({"tasks": [
        {"label": "New uninstalled behavior", "request": "Use the newer candidate's new behavior."}]})
    service.guide.state["version"] += 1
    result = await service.use(service.guide.identity)
    assert result["first_use"]["tasks"] == [task]
    assert result["first_use"]["metadata_sha256"] == hashlib.sha256(content.encode()).hexdigest()
    assert result["first_use"]["scope"] == "authored-first-use-suggestions"
    assert result["build_id"] == receipt["build_id"] and result["source_digest"] == DIGEST
    assert result["version"] == 5 and result["guide_version"] == 4
    assert result["continuation"]["requires_acknowledgement"] is True
    assert result["application_outcomes_verified"] is False and result["console_url"] is None
    assert PRIVATE not in json.dumps(result) and "New uninstalled behavior" not in json.dumps(result)
    assert read_receipt(directory) == receipt
    assert before == {str(path.relative_to(destination)): path.read_bytes()
                      for path in destination.rglob("*") if path.is_file()}


@pytest.mark.parametrize("content", [None, '{"tasks":[{"label":"No","request":"Job","fixtures":{}}]}',
                                      '{"tasks":[{"label":"No","request":"Job","verified":true}]}'])
async def test_absent_or_invalid_first_jobs_leave_normal_installed_use_and_feedback_available(tmp_path, content):
    extra = {"application/trial.json": json.dumps({"cases": [{"label": "Offline only",
        "request": "Fake a confirmation.", "fixtures": {"private-principal": PRIVATE}}]})}
    if content is not None:
        extra["application/first-use.json"] = content
    service, directory, _ = await prepared(tmp_path, extra_files=extra)
    saved = read_receipt(directory)
    result = await service.use(service.guide.identity)
    assert result["first_use"]["tasks"] == [] and result["first_use"]["detail"]
    assert result["entrypoint"] == {"identity": "lead:main", "name": "Installed Companion"}
    assert result["continuation"]["available"] is True and PRIVATE not in json.dumps(result)
    await service.feedback(service.guide.identity, **await feedback_body(service))
    service.implementation.submit.assert_awaited_once()
    assert read_receipt(directory) == saved


async def test_modified_installed_catalog_or_changing_guide_cannot_publish_stale_first_jobs(tmp_path):
    content = json.dumps({"tasks": [{"label": "Read", "request": "Read my saved notes."}]})
    service, _, destination = await prepared(tmp_path, extra_files={"application/first-use.json": content})
    original = service.implementation.status
    async def changes_guide(*args, **kwargs):
        checked = await original(*args, **kwargs)
        service.guide.state["version"] += 1
        return checked
    service.implementation.status = changes_guide
    with pytest.raises(DraftConflict, match="setup changed"):
        await service.use(service.guide.identity)
    service.implementation.status = original
    (destination / "application/first-use.json").write_text('{"tasks": []}')
    with pytest.raises(DraftConflict, match="could not be verified"):
        await service.use(service.guide.identity)


async def test_current_install_feedback_uses_the_existing_source_fence_and_changes_no_installed_bytes(tmp_path):
    service, directory, destination = await prepared(tmp_path)
    body = await feedback_body(service)
    before = {str(path.relative_to(destination)): path.read_bytes()
              for path in destination.rglob("*") if path.is_file()}
    saved_receipt = read_receipt(directory)
    result = await service.feedback(service.guide.identity, **body)
    assert result == {"build_id": "new-candidate", "status": "queued"}
    args, kwargs = service.implementation.submit.await_args
    assert args[:2] == (service.guide.identity, body["version"])
    assert kwargs == {"expected_previous_build_id": body["continuation_build_id"],
                      "expected_source_digest": body["continuation_source_digest"],
                      "expected_framework_build_id": body["build_id"],
                      "expected_framework_source_digest": body["source_digest"]}
    evidence = evidence_from(args[2])
    assert evidence["reported_outcome"] == "partly" and evidence["reported_task"] == body["task"]
    assert evidence["older_installed_source"] is False
    assert "not verified native activity or business outcomes" in evidence["scope"]
    assert "fresh independent acceptance" in args[2] and body["request"] in args[2]
    assert PRIVATE not in args[2] and str(destination) not in args[2]
    assert read_receipt(directory) == saved_receipt
    assert before == {str(path.relative_to(destination)): path.read_bytes()
                      for path in destination.rglob("*") if path.is_file()}


async def test_older_install_requires_explicit_newer_candidate_and_retains_the_failed_checked_basis(tmp_path):
    service, directory, _ = await prepared(tmp_path)
    installed = read_receipt(directory)
    newer_id, newer_digest = str(uuid.uuid4()), "b" * 64
    service.implementation.identity, service.implementation.digest = newer_id, newer_digest
    checked = await service.implementation.status(service.guide.identity)
    checked.update(status="failed", ready_to_package=False, acceptance_current=False,
                   acceptance_trusted=False, acceptance=None)
    service.implementation.status = AsyncMock(return_value=checked)
    body = await feedback_body(service)
    assert body["build_id"] == installed["build_id"]
    assert body["continuation_build_id"] == newer_id
    with pytest.raises(DraftConflict, match="Explicitly choose"):
        await service.feedback(service.guide.identity, **body)
    service.implementation.submit.assert_not_awaited()
    await service.feedback(service.guide.identity, **body, continue_current_candidate=True)
    args, kwargs = service.implementation.submit.await_args
    assert kwargs == {"expected_previous_build_id": newer_id, "expected_source_digest": newer_digest,
                      "expected_framework_build_id": installed["build_id"],
                      "expected_framework_source_digest": body["source_digest"]}
    evidence = evidence_from(args[2])
    assert evidence["build_id"] == installed["build_id"] and evidence["source_digest"] == DIGEST
    assert evidence["older_installed_source"] is True and evidence["continuation_build_id"] == newer_id
    assert "Do not replace this candidate with the older installed source" in args[2]
    assert read_receipt(directory) == installed


async def test_changed_design_feedback_keeps_current_candidate_without_old_framework_override(tmp_path):
    service, _, _ = await prepared(tmp_path)
    service.guide.state["proposal"]["project"]["name"] = "New requested scope"
    body = await feedback_body(service)
    await service.feedback(service.guide.identity, **body, continue_current_candidate=True)
    _, kwargs = service.implementation.submit.await_args
    assert kwargs == {"expected_previous_build_id": body["continuation_build_id"],
                      "expected_source_digest": body["continuation_source_digest"]}


@pytest.mark.parametrize("field,value", [("deployment_id", str(uuid.uuid4())),
    ("build_id", str(uuid.uuid4())), ("source_digest", "b" * 64), ("package_sha256", "b" * 64),
    ("continuation_build_id", str(uuid.uuid4())), ("continuation_source_digest", "b" * 64),
    ("version", 99)])
async def test_stale_install_or_candidate_tokens_do_not_admit_a_builder(tmp_path, field, value):
    service, _, _ = await prepared(tmp_path)
    body = {**await feedback_body(service), field: value}
    with pytest.raises(DraftConflict):
        await service.feedback(service.guide.identity, **body)
    service.implementation.submit.assert_not_awaited()


async def test_modified_or_unowned_installed_source_does_not_expose_metadata_or_admit_feedback(tmp_path):
    service, _, destination = await prepared(tmp_path)
    body = await feedback_body(service)
    (destination / "app.py").write_text("print('different source')\n")
    with pytest.raises(DraftConflict, match="could not be verified"):
        await service.use(service.guide.identity)
    with pytest.raises(DraftConflict, match="could not be verified"):
        await service.feedback(service.guide.identity, **body)
    service.implementation.submit.assert_not_awaited()


@pytest.mark.parametrize("changes", [{"status": "starting"},
    {"status": "running", "phase": "update"},
    {"status": "stopped", "update": {"status": "recovery-needed"}}])
async def test_active_mutation_blocks_feedback_without_operating_the_installation(tmp_path, changes):
    service, directory, _ = await prepared(tmp_path)
    body = await feedback_body(service)
    # Avoid constructing an unrelated update transaction: these detached host
    # observations exercise the same mutation admission gate without a worker.
    receipt = await service.guide_status(service.guide.identity, state=service.guide.state)
    receipt.update(changes)
    service.guide_status = AsyncMock(return_value=receipt)
    assert (await service.use(service.guide.identity))["continuation"]["available"] is False
    with pytest.raises(DraftConflict, match="Finish installation"):
        await service.feedback(service.guide.identity, **body)
    service.implementation.submit.assert_not_awaited()


@pytest.mark.parametrize("changes", [{"current": False}, {"source_verified": False}, {"status": "running"}])
async def test_unchecked_stale_or_active_candidate_is_not_a_repair_basis(tmp_path, changes):
    service, _, _ = await prepared(tmp_path)
    body = await feedback_body(service)
    checked = await service.implementation.status(service.guide.identity)
    service.implementation.status = AsyncMock(return_value={**checked, **changes})
    assert (await service.use(service.guide.identity))["continuation"]["available"] is False
    with pytest.raises(DraftConflict, match="candidate changed"):
        await service.feedback(service.guide.identity, **body)
    service.implementation.submit.assert_not_awaited()


async def test_human_success_report_still_requires_an_explicit_improvement_and_cannot_supply_native_evidence(tmp_path):
    service, _, _ = await prepared(tmp_path)
    body = {**await feedback_body(service), "outcome": "worked", "request": "  "}
    with pytest.raises(ValueError, match="requested improvement"):
        await service.feedback(service.guide.identity, **body)
    service.implementation.submit.assert_not_awaited()
    body["request"] = "Make it easier to read."
    await service.feedback(service.guide.identity, **body)
    report = evidence_from(service.implementation.submit.await_args.args[2])
    assert report["reported_outcome"] == "worked"
    assert not {"native_frames", "conversation_id", "interaction_id", "application_outcomes_verified"} & report.keys()


async def test_deployment_lease_is_held_through_candidate_admission_but_no_install_guard_is_invented(tmp_path):
    service, _, _ = await prepared(tmp_path)
    body = await feedback_body(service)
    reached, release = asyncio.Event(), asyncio.Event()
    async def pending(*args, **kwargs):
        assert service.lock.locked()
        reached.set()
        await release.wait()
        return {"build_id": "new"}
    service.implementation.submit = AsyncMock(side_effect=pending)
    task = asyncio.create_task(service.feedback(service.guide.identity, **body))
    await reached.wait()
    assert service.lock.locked()
    release.set()
    assert await task == {"build_id": "new"}
    assert not service.lock.locked()
    assert set(service.implementation.submit.await_args.kwargs) == {
        "expected_previous_build_id", "expected_source_digest",
        "expected_framework_build_id", "expected_framework_source_digest"}


@pytest.mark.parametrize("detail", [
    "The application changed since this trial. Try the current build before asking for its repair.",
    "The application changed while preparing this build. Try its current checked source before repairing it.",
    "The setup changed while preparing this build. Reload before starting the application builder."])
async def test_final_submit_conflicts_remain_conflicts_with_truthful_installed_feedback_copy(tmp_path, detail):
    service, _, _ = await prepared(tmp_path)
    body = await feedback_body(service)
    service.implementation.submit.side_effect = DraftConflict(detail)
    with pytest.raises(DraftConflict) as caught:
        await service.feedback(service.guide.identity, **body)
    assert "trial" not in str(caught.value)
    assert not isinstance(caught.value, InstalledFeedbackNotAdmitted)
    assert service.implementation.submit.await_count == 1
    assert not service.lock.locked()
    if detail.startswith("The setup"):
        assert str(caught.value) == detail


def api_body():
    return {"version": 4, "deployment_id": "installed", "build_id": "build", "source_digest": DIGEST,
        "package_sha256": "b" * 64, "continuation_build_id": "current", "continuation_source_digest": "c" * 64,
        "continue_current_candidate": True, "outcome": "blocked", "request": "I could not see the result.",
        "task": "Organize my test collection."}


def test_api_forwards_exact_installed_and_continuation_bindings(monkeypatch):
    deployment = type("Endpoint", (), {})()
    deployment.use = AsyncMock(return_value={"build_id": "installed", "application_outcomes_verified": False})
    deployment.feedback = AsyncMock(return_value={"build_id": "new", "status": "queued"})
    monkeypatch.setattr(server, "deployment", deployment)
    client = TestClient(server.app)
    path = "/api/guide/isolated/deployment"
    assert client.get(path + "/use").json()["application_outcomes_verified"] is False
    deployment.use.assert_awaited_once_with("isolated")
    body = api_body()
    response = client.post(path + "/feedback", json=body)
    assert response.status_code == 202 and response.json() == {"build_id": "new", "status": "queued"}
    deployment.feedback.assert_awaited_once_with("isolated", 4, "installed", "build", DIGEST, "b" * 64,
        "current", "c" * 64, request=body["request"], outcome="blocked", task=body["task"], continue_current_candidate=True)
    assert response.headers["cache-control"] == "no-store"


@pytest.mark.parametrize("change", [{"request": {"private": PRIVATE}}, {"request": PRIVATE * 300},
    {"task": PRIVATE * 200}, {"task": [PRIVATE]}, {"source_digest": PRIVATE},
    {"continue_current_candidate": "true"}, {"version": "4"}, {"outcome": "verified"},
    {"native_frames": [{"text": PRIVATE}]}, {"expected_framework_build_id": "unowned-baseline"}])
def test_api_rejects_private_invalid_feedback_without_echoing_input_or_context(monkeypatch, change):
    deployment = type("Endpoint", (), {})()
    deployment.feedback = AsyncMock()
    monkeypatch.setattr(server, "deployment", deployment)
    response = TestClient(server.app).post("/api/guide/isolated/deployment/feedback", json={**api_body(), **change})
    assert response.status_code == 422 and PRIVATE not in response.text
    assert all("input" not in row and "ctx" not in row for row in response.json()["detail"])
    deployment.feedback.assert_not_awaited()


def test_feedback_conflict_remains_actionable_without_admitting_an_automatic_retry(monkeypatch):
    deployment = type("Endpoint", (), {})()
    deployment.feedback = AsyncMock(side_effect=DraftConflict("Choose the newer checked candidate explicitly."))
    monkeypatch.setattr(server, "deployment", deployment)
    response = TestClient(server.app).post("/api/guide/isolated/deployment/feedback", json=api_body())
    assert response.status_code == 409 and response.json()["detail"] == "Choose the newer checked candidate explicitly."
    assert "code" not in response.json()
    assert deployment.feedback.await_count == 1


@pytest.mark.parametrize("case,status", [("request", 422), ("predecessor", 409), ("operation", 409)])
async def test_pre_admission_refusal_has_an_explicit_witness_and_never_calls_submit(tmp_path, case, status):
    service, directory, destination = await prepared(tmp_path)
    body = await feedback_body(service)
    if case == "request":
        body["request"] = " "
    elif case == "predecessor":
        body["continuation_source_digest"] = "b" * 64
    else:
        service.guide.state["operation"] = {"status": "running"}
    before = {str(path.relative_to(destination)): path.read_bytes()
              for path in destination.rglob("*") if path.is_file()}
    receipt_before = read_receipt(directory)
    with pytest.raises(InstalledFeedbackNotAdmitted) as caught:
        await service.feedback(service.guide.identity, **body)
    assert caught.value.status_code == status and caught.value.code == "installed_feedback_not_admitted"
    assert isinstance(caught.value, DraftConflict) is (status == 409)
    assert PRIVATE not in str(caught.value)
    service.implementation.submit.assert_not_called()
    assert not service.lock.locked() and read_receipt(directory) == receipt_before
    assert before == {str(path.relative_to(destination)): path.read_bytes()
                      for path in destination.rglob("*") if path.is_file()}


async def test_owner_prerequisite_refusal_reaches_coded_feedback_server_result(tmp_path, monkeypatch):
    from launchkit.implementation import ImplementationPrerequisiteNotAdmitted

    service, directory, destination = await prepared(tmp_path)
    body = await feedback_body(service)
    before = {str(path.relative_to(destination)): path.read_bytes()
              for path in destination.rglob("*") if path.is_file()}
    receipt_before = read_receipt(directory)
    service.implementation.submit.side_effect = ImplementationPrerequisiteNotAdmitted("Prepare app checks before sending again.")
    monkeypatch.setattr(server, "deployment", service)
    response = await server.installed_application_feedback(service.guide.identity, server.InstalledFeedbackRequest(**body))
    assert response.status_code == 422
    assert json.loads(response.body) == {"detail": "Prepare app checks before sending again.",
        "code": "installed_feedback_not_admitted"}
    service.implementation.submit.assert_awaited_once()
    assert not service.lock.locked() and read_receipt(directory) == receipt_before
    assert before == {str(path.relative_to(destination)): path.read_bytes()
                      for path in destination.rglob("*") if path.is_file()}


@pytest.mark.parametrize("error_type,status", [(DraftConflict, 409), (ValueError, 422), (server.ArchitectUnavailable, 503)])
async def test_submit_then_status_failure_keeps_admission_uncertain_and_exact_predecessor(tmp_path, error_type, status):
    from fastapi import HTTPException

    service, directory, _ = await prepared(tmp_path)
    body = await feedback_body(service)
    original = read_receipt(directory)
    admitted = str(uuid.uuid4())
    async def publish_then_fail(*args, **kwargs):
        assert args[:2] == (service.guide.identity, body["version"])
        assert service.lock.locked()
        assert kwargs == {"expected_previous_build_id": body["continuation_build_id"],
                          "expected_source_digest": body["continuation_source_digest"],
                          "expected_framework_build_id": body["build_id"],
                          "expected_framework_source_digest": body["source_digest"]}
        service.implementation.identity = admitted
        raise error_type("The newly published candidate status could not be read.")
    service.implementation.submit.side_effect = publish_then_fail
    with pytest.raises(HTTPException) as caught:
        await server.guide_result(service.feedback(service.guide.identity, **body))
    assert caught.value.status_code == status
    assert caught.value.detail == "The newly published candidate status could not be read."
    service.implementation.submit.assert_awaited_once()
    assert service.implementation.identity == admitted and read_receipt(directory) == original
    assert not service.lock.locked()


@pytest.mark.parametrize("error_type,status", [
    (InstalledFeedbackConflictNotAdmitted, 409), (InstalledFeedbackNotAdmitted, 422)])
def test_feedback_api_preserves_typed_pre_admission_status_and_detail(monkeypatch, error_type, status):
    deployment = type("Endpoint", (), {})()
    detail = "Review this exact installed source before sending feedback."
    deployment.feedback = AsyncMock(side_effect=error_type(detail))
    monkeypatch.setattr(server, "deployment", deployment)
    response = TestClient(server.app).post("/api/guide/isolated/deployment/feedback", json=api_body())
    assert response.status_code == status
    assert response.json() == {"detail": detail, "code": "installed_feedback_not_admitted"}
    assert response.headers["cache-control"] == "no-store"
    deployment.feedback.assert_awaited_once()


@pytest.mark.parametrize("error_type,status", [(DraftConflict, 409), (ValueError, 422), (server.ArchitectUnavailable, 503)])
def test_feedback_api_never_infers_a_rejection_witness_from_http_status(monkeypatch, error_type, status):
    deployment = type("Endpoint", (), {})()
    detail = "Candidate publication was followed by an unavailable status."
    deployment.feedback = AsyncMock(side_effect=error_type(detail))
    monkeypatch.setattr(server, "deployment", deployment)
    response = TestClient(server.app).post("/api/guide/isolated/deployment/feedback", json=api_body())
    assert response.status_code == status and response.json() == {"detail": detail}
    deployment.feedback.assert_awaited_once()
