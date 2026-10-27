"""Owner drafts remain frozen advisory input to an ordinary native author turn."""
import copy
import hashlib
import json
from types import SimpleNamespace

import pytest

from launchkit import architect as architect_module
from launchkit.visualizer import (
    MAX_CODE_LENGTH, VisualizerPending, VisualizerService, VisualizerUnavailable,
    visualization_request_message,
)
from test_visualizer import CODE, inputs, native_admission, native_completion, service


DRAFT = CODE.replace("duration:36", "duration:43") + "\r\n// Distinct caf\u00e9 composition.\r\n"


def draft_feedback(code=DRAFT):
    return "Refine this advisory composition. draft-sha256:" + hashlib.sha256(code.encode("utf-8")).hexdigest()


@pytest.mark.asyncio
async def test_exact_owner_source_is_saved_before_one_ordinary_native_send(service, monkeypatch, tmp_path):
    path = tmp_path / "owner-draft.js"
    path.write_bytes(DRAFT.encode("utf-8"))
    monkeypatch.setenv("MOBKIT_LAUNCH_ANIMATION_DRAFT", str(path))
    calls = native_completion(service, monkeypatch)
    delegate = architect_module._native_request
    frozen = []

    async def request(client, method, url, **kwargs):
        if method == "post":
            saved = json.loads((service.root / "pending.json").read_text())
            frozen.append(saved)
            assert saved["author_draft"] == {
                "code": DRAFT, "code_hash": hashlib.sha256(DRAFT.encode("utf-8")).hexdigest()}
            assert kwargs["json"]["content"] == visualization_request_message(saved)
            assert kwargs["json"]["idempotency_key"] == saved["idempotency_key"]
        return await delegate(client, method, url, **kwargs)

    monkeypatch.setattr(architect_module, "_native_request", request)
    feedback = draft_feedback()
    expected = service.request_binding(*inputs(), feedback=feedback)
    artifact = await service.generate(*inputs(), feedback=feedback)
    sends = [kwargs["json"] for method, _, kwargs in calls if method == "post"]
    assert len(sends) == len(frozen) == 1
    assert sends[0]["identity"] == frozen[0]["author_identity"]
    assert sends[0]["identity"].startswith("visualizer-request-")
    assert sends[0]["origin"] == "launch-visualizer"
    assert DRAFT not in sends[0]["content"]
    assert frozen[0]["author_draft_delivery"] == "inspection-v1"
    assert frozen[0]["author_draft"]["code_hash"] in sends[0]["content"]
    assert "view:author_draft" in sends[0]["content"] and "After context shortening" in sends[0]["content"]
    assert "composition and motion retained" in sends[0]["content"]
    assert "Do not recreate it as an unrelated layout" in sends[0]["content"]
    assert "not topology, a checked previous publication" in sends[0]["content"]
    assert "publication checks still apply" in sends[0]["content"]
    assert artifact["key"] == expected["native_request_key"]
    assert frozen[0]["blueprint"] == inputs()[0] and frozen[0]["model"] == inputs()[1]
    # The draft itself is not a publication; the native author publishes CODE.
    assert artifact["code"] == CODE and "author_draft" not in artifact
    path.write_bytes(b"Changed after publication")
    assert (await service.generate(*inputs(), feedback=feedback))["cached"] is True
    assert len([call for call in calls if call[0] == "post"]) == 1


@pytest.mark.asyncio
async def test_configured_file_is_ignored_without_explicit_draft_marker(service, monkeypatch, tmp_path):
    monkeypatch.setenv("MOBKIT_LAUNCH_ANIMATION_DRAFT", str(tmp_path / "missing-private.js"))
    calls = native_completion(service, monkeypatch)
    await service.generate(*inputs(), feedback="Keep the main labels readable.")
    content = calls[0][2]["json"]["content"]
    assert "ADVISORY ANIMATION DRAFT" not in content


@pytest.mark.asyncio
@pytest.mark.parametrize("invalid", ["missing", "mismatch", "oversized", "utf8", "duplicate", "malformed"])
async def test_requested_invalid_draft_refuses_before_admission_without_disclosure(
        service, monkeypatch, tmp_path, invalid):
    path = tmp_path / "private-owner-draft.js"
    raw = DRAFT.encode("utf-8")
    feedback = draft_feedback()
    if invalid == "mismatch":
        raw = b"Private replacement source"
    elif invalid == "oversized":
        raw = b"x" * (MAX_CODE_LENGTH + 1)
        feedback = draft_feedback(raw.decode())
    elif invalid == "utf8":
        raw = b"\xff" + raw
        feedback = "draft-sha256:" + hashlib.sha256(raw).hexdigest()
    elif invalid == "duplicate":
        feedback += " " + feedback.split()[-1]
    elif invalid == "malformed":
        feedback = "draft-sha256:private-invalid-digest"
    if invalid != "missing":
        path.write_bytes(raw)
    monkeypatch.setenv("MOBKIT_LAUNCH_ANIMATION_DRAFT", str(path))
    calls = native_completion(service, monkeypatch)
    admissions = []

    async def admitted():
        admissions.append(True)

    with pytest.raises(ValueError) as error:
        await service.generate_with_admission(*inputs(), feedback=feedback, on_admission=admitted)
    assert str(error.value) == "The requested animation draft is unavailable or does not match its hash."
    assert calls == admissions == [] and service.architect.starts == 0
    assert service.pending is None and not (service.root / "pending.json").exists()


@pytest.mark.asyncio
@pytest.mark.parametrize("local_change", ["replace", "delete", "new-path"])
async def test_response_loss_recovery_replays_frozen_source_and_same_key_without_rereading_file(
        service, monkeypatch, tmp_path, local_change):
    path = tmp_path / "owner-draft.js"
    path.write_bytes(DRAFT.encode("utf-8"))
    monkeypatch.setenv("MOBKIT_LAUNCH_ANIMATION_DRAFT", str(path))
    first_send = []

    async def response_lost(client, method, url, **kwargs):
        assert method == "post"
        first_send.append(copy.deepcopy(kwargs["json"]))
        raise TimeoutError

    monkeypatch.setattr(architect_module, "_native_request", response_lost)
    with pytest.raises(VisualizerPending):
        await service.generate(*inputs(), feedback=draft_feedback())
    saved = json.loads((service.root / "pending.json").read_text())
    assert saved["admission"] is None and saved["author_draft"]["code"] == DRAFT
    if local_change == "replace":
        path.write_bytes(b"Different bytes must never replace the saved native input.")
    elif local_change == "delete":
        path.unlink()
    else:
        monkeypatch.setenv("MOBKIT_LAUNCH_ANIMATION_DRAFT", str(tmp_path / "different-missing.js"))
    restored = VisualizerService(service.architect, timeout=3, poll_interval=.001)
    calls = native_completion(restored, monkeypatch)
    delegate = architect_module._native_request

    async def request(client, method, url, **kwargs):
        if method == "post":
            page = await restored.inspect({"view": "author_draft", "request_key": saved["key"]})
            assert page["code"] == DRAFT and page["code_hash"] == saved["author_draft"]["code_hash"]
        return await delegate(client, method, url, **kwargs)

    monkeypatch.setattr(architect_module, "_native_request", request)
    artifact = await restored.generate(*inputs(), feedback=draft_feedback())
    assert calls[0][0] == "post" and calls[0][2]["json"] == first_send[0]
    assert artifact["key"] == saved["key"] and artifact["code"] == CODE


@pytest.mark.asyncio
@pytest.mark.parametrize("mutation", ["code", "hash", "missing", "unrequested", "feedback-hash", "delivery"])
async def test_damaged_saved_draft_cannot_be_sent_or_obtain_checked_author_context(
        service, monkeypatch, tmp_path, mutation):
    path = tmp_path / "owner-draft.js"
    path.write_bytes(DRAFT.encode("utf-8"))
    monkeypatch.setenv("MOBKIT_LAUNCH_ANIMATION_DRAFT", str(path))

    async def response_lost(*args, **kwargs):
        raise TimeoutError

    monkeypatch.setattr(architect_module, "_native_request", response_lost)
    with pytest.raises(VisualizerPending):
        await service.generate(*inputs(), feedback=draft_feedback())
    job = copy.deepcopy(service.pending)
    if mutation == "code":
        job["author_draft"]["code"] = CODE
    elif mutation == "hash":
        job["author_draft"]["code_hash"] = "0" * 64
    elif mutation == "missing":
        job.pop("author_draft")
    elif mutation == "unrequested":
        job["revision_feedback"] = "No draft was requested."
    elif mutation == "feedback-hash":
        job["feedback_hash"] = "0" * 64
    else:
        job["author_draft_delivery"] = "unknown-format"
    with pytest.raises(ValueError, match="requested animation draft"):
        visualization_request_message(job)
    assert service._author_context_source(job) is None
    service.pending = job
    detail = await service.inspect({"view": "author_draft", "request_key": job["key"]})
    assert detail["available"] is False and "code" not in detail
    service._save_pending()
    before = (service.root / "pending.json").read_bytes()
    restored = VisualizerService(service.architect, timeout=3, poll_interval=.001)
    calls = native_completion(restored, monkeypatch)
    starts = service.architect.starts
    with pytest.raises(VisualizerUnavailable, match="saved visualization request"):
        await restored.generate(*inputs(), feedback=draft_feedback())
    assert calls == [] and service.architect.starts == starts
    assert (service.root / "pending.json").read_bytes() == before


@pytest.mark.asyncio
async def test_draft_line_pages_reassemble_exact_utf8_without_claiming_context_or_publication(
        service, monkeypatch, tmp_path):
    source = DRAFT + "".join(f"// source line {i}\r\n" for i in range(450)) + "// Final line without newline"
    path = tmp_path / "owner-draft.js"
    path.write_bytes(source.encode("utf-8"))
    monkeypatch.setenv("MOBKIT_LAUNCH_ANIMATION_DRAFT", str(path))

    async def response_lost(*args, **kwargs):
        raise TimeoutError

    monkeypatch.setattr(architect_module, "_native_request", response_lost)
    with pytest.raises(VisualizerPending):
        await service.generate(*inputs(), feedback=draft_feedback(source))
    before = copy.deepcopy(service.pending)
    path.unlink()
    pages, offset = [], 0
    while offset is not None:
        page = await service.inspect({"view": "author_draft", "request_key": before["key"],
                                      "offset": offset, "limit": 200})
        assert page["available"] is True and page["offset"] == offset
        assert page["offset_unit"] == "source_lines"
        assert page["total"] == len(source.splitlines(keepends=True))
        assert page["byte_length"] == len(source.encode("utf-8"))
        assert page["code_hash"] == hashlib.sha256(source.encode("utf-8")).hexdigest()
        pages.append(page["code"])
        offset = page["next_offset"]
    assert len(pages) == 3 and "".join(pages).encode("utf-8") == source.encode("utf-8")
    assert service.pending["author_draft_read"] == {"request_key": before["key"],
        "code_hash": before["author_draft"]["code_hash"], "ranges": [[0, len(source.splitlines(keepends=True))]]}
    assert {key: value for key, value in service.pending.items() if key != "author_draft_read"} == before
    after_reads = copy.deepcopy(service.pending)
    result = await service.publish({"request_key": before["key"], "code": CODE, "summary": "Still needs checked context."})
    assert result["valid"] is False and service.pending == after_reads
    overview = await service.inspect({"view": "overview", "request_key": before["key"]})
    assert overview["author_draft"]["code_hash"] == before["author_draft"]["code_hash"]
    assert "code" not in overview["author_draft"] and source not in json.dumps(overview)


@pytest.mark.asyncio
async def test_draft_reads_require_current_key_and_global_page_bounds(service, monkeypatch, tmp_path):
    path = tmp_path / "owner-draft.js"
    path.write_bytes(DRAFT.encode("utf-8"))
    monkeypatch.setenv("MOBKIT_LAUNCH_ANIMATION_DRAFT", str(path))

    async def response_lost(*args, **kwargs):
        raise TimeoutError

    monkeypatch.setattr(architect_module, "_native_request", response_lost)
    with pytest.raises(VisualizerPending):
        await service.generate(*inputs(), feedback=draft_feedback())
    job = copy.deepcopy(service.pending)
    for args in ({}, {"request_key": "previous-key"}, {"request_key": job["key"], "offset": -1},
                 {"request_key": job["key"], "limit": 201}, {"request_key": job["key"], "limit": True},
                 {"request_key": job["key"], "offset": len(DRAFT.splitlines(keepends=True)) + 1}):
        result = await service.inspect({"view": "author_draft", **args})
        assert result["available"] is False and "code" not in result
    assert service.pending == job
    service.pending.pop("author_draft")
    service.pending.pop("author_draft_delivery")
    service.pending["revision_feedback"] = ""
    service.pending.pop("feedback_hash")
    service.pending["key"] = service.request_binding(*inputs())["native_request_key"]
    result = await service.inspect({"view": "author_draft", "request_key": service.pending["key"]})
    assert result["available"] is False and "No owner-supplied" in result["issue"] and "code" not in result


@pytest.mark.asyncio
async def test_preupgrade_lost_response_keeps_original_inline_admission_body_and_new_retrieval(
        service, monkeypatch, tmp_path):
    path = tmp_path / "owner-draft.js"
    path.write_bytes(DRAFT.encode("utf-8"))
    monkeypatch.setenv("MOBKIT_LAUNCH_ANIMATION_DRAFT", str(path))

    async def response_lost(*args, **kwargs):
        raise TimeoutError

    monkeypatch.setattr(architect_module, "_native_request", response_lost)
    with pytest.raises(VisualizerPending):
        await service.generate(*inputs(), feedback=draft_feedback())
    # A saved job made before compact retrieval was introduced has no marker.
    service.pending.pop("author_draft_delivery")
    legacy = copy.deepcopy(service.pending)
    original_body = visualization_request_message(legacy)
    assert original_body.endswith("BEGIN ADVISORY ANIMATION DRAFT\n" + DRAFT + "\nEND ADVISORY ANIMATION DRAFT")
    assert "view:author_draft" not in original_body
    service._save_pending()
    path.unlink()
    restored = VisualizerService(service.architect, timeout=3, poll_interval=.001)
    calls = native_completion(restored, monkeypatch)
    restored._load()
    page = await restored.inspect({"view": "author_draft", "request_key": legacy["key"]})
    assert page["code"] == DRAFT
    assert {key: value for key, value in restored.pending.items() if key != "author_draft_read"} == legacy
    await restored.generate(*inputs(), feedback=draft_feedback())
    assert calls[0][0] == "post"
    assert calls[0][2]["json"]["content"] == original_body
    assert calls[0][2]["json"]["idempotency_key"] == legacy["idempotency_key"]


@pytest.mark.asyncio
async def test_draft_retrieval_tool_is_available_to_fresh_and_resumed_native_build_callbacks(
        service, monkeypatch, tmp_path):
    from meerkat_meta.agent_builder import CallbackDispatcher

    path = tmp_path / "owner-draft.js"
    path.write_bytes(DRAFT.encode("utf-8"))
    monkeypatch.setenv("MOBKIT_LAUNCH_ANIMATION_DRAFT", str(path))

    async def response_lost(*args, **kwargs):
        raise TimeoutError

    monkeypatch.setattr(architect_module, "_native_request", response_lost)
    with pytest.raises(VisualizerPending):
        await service.generate(*inputs(), feedback=draft_feedback())
    dispatcher = CallbackDispatcher()

    class SessionBuilder:
        async def build_agent(self, draft):
            service.register_tools(draft)

    dispatcher.register_builder(SessionBuilder())
    before = copy.deepcopy(service.pending)
    for scope, resume in (("fresh-draft", None), ("restored-draft", before["author_session_id"])):
        options = {"scope_id": scope, "profile_name": "visualizer",
                   "session_id": before["author_session_id"],
                   "labels": {"visualization_author_identity": before["author_identity"],
                              "visualization_request_key": before["key"]}}
        if resume:
            options["resume_session_id"] = resume
        result = await dispatcher.handle_callback("callback/build_agent", {"options": options})
        definition = next(tool for tool in result["tools"] if tool["name"] == "inspect_visualization")
        assert "author_draft" in definition["input_schema"]["properties"]["view"]["enum"]
        publication = next(tool for tool in result["tools"] if tool["name"] == "publish_visualization")
        assert publication["input_schema"]["properties"]["author_draft_code_hash"]["type"] == ["string", "null"]
        page = (await dispatcher.handle_callback("callback/call_tool", {"scope_id": scope,
            "tool": "inspect_visualization", "arguments": {"view": "author_draft", "request_key": before["key"],
                "offset": 1, "limit": 2}}))["content"]
        assert page["code"] == "".join(DRAFT.splitlines(keepends=True)[1:3])
        assert page["code_hash"] == before["author_draft"]["code_hash"]
    assert service.pending["author_draft_read"]["ranges"] == [[1, min(3, len(DRAFT.splitlines(keepends=True)))]]
    assert {key: value for key, value in service.pending.items() if key != "author_draft_read"} == before


@pytest.mark.asyncio
async def test_native_author_can_publish_exact_frozen_draft_by_hash_after_complete_checked_reads(
        service, monkeypatch, tmp_path):
    path = tmp_path / "owner-draft.js"
    path.write_bytes(DRAFT.encode("utf-8"))
    monkeypatch.setenv("MOBKIT_LAUNCH_ANIMATION_DRAFT", str(path))
    calls = []

    async def request(client, method, url, **kwargs):
        calls.append((method, copy.deepcopy(kwargs)))
        if method == "post":
            return SimpleNamespace(json=lambda: native_admission(service, "native-visualization"))
        job = service.pending
        await service.inspect({"view": "overview", "request_key": job["key"], "include_author_references": True})
        await service.inspect({"view": "author_draft", "request_key": job["key"]})
        path.write_bytes(b"Changed current file must not enter publication.")
        result = await service.publish({"request_key": job["key"], "author_draft_code_hash": job["author_draft"]["code_hash"],
                                        "code": None, "base_code_hash": None, "edits": None,
                                        "summary": "Verified supplied composition retained unchanged."})
        assert result["valid"] is True
        return SimpleNamespace(json=lambda: {"frames": [
            {"kind": "tool_execution_completed", "interaction_id": "native-visualization", "cursor": 2,
             "payload": {"name": "publish_visualization", "result": result}},
            {"kind": "interaction_complete", "interaction_id": "native-visualization", "cursor": 3}]})

    monkeypatch.setattr(architect_module, "_native_request", request)
    artifact = await service.generate(*inputs(), feedback=draft_feedback())
    digest = hashlib.sha256(DRAFT.encode("utf-8")).hexdigest()
    assert [method for method, _ in calls] == ["post", "get"]
    assert DRAFT not in calls[0][1]["json"]["content"]
    assert "author_draft_code_hash" in calls[0][1]["json"]["content"]
    assert artifact["code"] == DRAFT and artifact["code_hash"] == digest
    assert artifact["author_draft_code_hash"] == digest
    assert artifact["publication_receipt"]["author_draft_code_hash"] == digest
    assert artifact["publication_receipt"]["authority"] == "native_publish_acknowledgment"
    assert service.pending is None
    assert (await service.generate(*inputs(), feedback=draft_feedback()))["cached"] is True
    assert len(calls) == 2
    cached_path = service.root / f"{artifact['key']}.json"
    damaged = json.loads(cached_path.read_text())
    damaged["author_draft_code_hash"] = "0" * 64
    cached_path.write_text(json.dumps(damaged))
    assert service._cache(artifact["key"]) is None


@pytest.mark.asyncio
async def test_reference_publication_requires_all_current_source_lines_and_retains_recovery_coverage(
        service, monkeypatch, tmp_path):
    path = tmp_path / "owner-draft.js"
    path.write_bytes(DRAFT.encode("utf-8"))
    monkeypatch.setenv("MOBKIT_LAUNCH_ANIMATION_DRAFT", str(path))

    async def response_lost(*args, **kwargs):
        raise TimeoutError

    monkeypatch.setattr(architect_module, "_native_request", response_lost)
    with pytest.raises(VisualizerPending):
        await service.generate(*inputs(), feedback=draft_feedback())
    job = service.pending
    args = {"request_key": job["key"], "author_draft_code_hash": job["author_draft"]["code_hash"], "summary": "Retained source."}
    # The body was not returned by initial admission or the compact overview.
    await service.inspect({"view": "overview", "request_key": job["key"], "include_author_references": True})
    assert "author_draft_read" not in job
    result = await service.publish(args)
    assert result["valid"] is False and "every frozen draft source line" in result["issue"]
    total = len(DRAFT.splitlines(keepends=True))
    await service.inspect({"view": "author_draft", "request_key": job["key"], "offset": 1, "limit": total})
    # Returning the final page or rereading it does not cover a missing first line.
    await service.inspect({"view": "author_draft", "request_key": job["key"], "offset": total - 1, "limit": 1})
    assert job["author_draft_read"]["ranges"] == [[1, total]]
    result = await service.publish(args)
    assert result["valid"] is False and job["publication"] is None
    restored = VisualizerService(service.architect, timeout=3, poll_interval=.001)
    path.unlink()
    restored._load()
    await restored.inspect({"view": "author_draft", "request_key": job["key"], "offset": 0, "limit": 2})
    assert restored.pending["author_draft_read"]["ranges"] == [[0, total]]
    for override in ({"request_key": "old-request"}, {"author_draft_code_hash": "0" * 64},
                     {"author_draft_code_hash": True}, {"code": CODE}, {"base_code_hash": "0" * 64},
                     {"edits": []}):
        rejected = await restored.publish({**args, **override})
        assert rejected["valid"] is False and restored.pending["publication"] is None
    result = await restored.publish(args)
    assert result["valid"] is True and restored.pending["publication"]["code"] == DRAFT
    assert restored._publication_is_bound(restored.pending)
    damaged = copy.deepcopy(restored.pending)
    damaged["author_draft_read"]["code_hash"] = "0" * 64
    assert not restored._publication_is_bound(damaged)
    damaged = copy.deepcopy(restored.pending)
    damaged["publication_receipt"]["author_draft_code_hash"] = "0" * 64
    assert not restored._publication_is_bound(damaged)


@pytest.mark.asyncio
async def test_returned_source_alone_does_not_authorize_hash_publication(service, monkeypatch, tmp_path):
    path = tmp_path / "owner-draft.js"
    path.write_bytes(DRAFT.encode("utf-8"))
    monkeypatch.setenv("MOBKIT_LAUNCH_ANIMATION_DRAFT", str(path))

    async def response_lost(*args, **kwargs):
        raise TimeoutError

    monkeypatch.setattr(architect_module, "_native_request", response_lost)
    with pytest.raises(VisualizerPending):
        await service.generate(*inputs(), feedback=draft_feedback())
    job = service.pending
    await service.inspect({"view": "author_draft", "request_key": job["key"]})
    result = await service.publish({"request_key": job["key"], "author_draft_code_hash": job["author_draft"]["code_hash"],
                                    "summary": "Source alone is insufficient."})
    assert result["valid"] is False and "both original reference images" in result["issue"]
    assert job["author_context_read"] is False and job["publication"] is None
