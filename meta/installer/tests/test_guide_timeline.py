import json
from types import SimpleNamespace

import pytest

from launchkit.guide_models import starter_state
from launchkit.guide_store import GuideStore
from launchkit.guide_timeline import GuideTimeline, legacy_binding, project_frame


def test_native_projection_hides_host_prompt_even_in_versioned_updates():
    binding = {"user_text": "Help our design team", "user_message_id": "user-1"}
    original = {"kind": "user_input", "payload": {"content": "Host prompt and schema", "origin": "launch-workbench"}}
    frame = {"kind": "frame_updated", "payload": {"frame": original}}
    projected = project_frame(frame, binding)
    assert projected["payload"]["frame"]["payload"]["content"] == binding["user_text"]
    assert original["payload"]["content"] == "Host prompt and schema"
    tool = {"kind": "tool_execution_completed", "payload": {"tool_name": "validate_blueprint", "result": {"valid": True}}}
    assert project_frame(tool, binding) == tool


def test_legacy_binding_requires_matching_setup_and_real_saved_user_message():
    message = {"id": "real-message", "role": "user", "text": "One job"}
    state = {"id": "setup-one", "messages": [message]}
    context = {"guide_id": state["id"], "current_user_message": message}
    frame = {"kind": "user_input", "cursor": "console:20", "interaction_id": "native-one",
             "payload": {"content": "Host instructions\nCurrent setup turn:\n" + json.dumps(context) + "\nMore instructions"}}
    assert legacy_binding(frame, state)["user_message_id"] == message["id"]
    assert legacy_binding(frame, {**state, "id": "setup-two"}) is None
    assert legacy_binding(frame, {**state, "messages": []}) is None


def test_attachment_projection_and_legacy_recovery_use_saved_guide_evidence():
    refs = [{'id': 'a' * 64, 'filename': 'notes [draft].txt', 'media_type': 'text/plain',
             'url': '/api/guide/setup-one/attachments/' + 'a' * 64},
            {'id': 'b' * 64, 'filename': 'night.png', 'media_type': 'image/png',
             'url': '/api/guide/setup-one/attachments/' + 'b' * 64}]
    message = {'id': 'user-one', 'role': 'user', 'text': 'Use this evidence', 'attachments': refs}
    binding = {'user_message_id': message['id'], 'user_text': message['text'], 'attachments': refs}
    original = {'kind': 'user_input', 'payload': {'content': [
        {'type': 'text', 'text': 'Private scaffold'}, {'type': 'image', 'source': 'blob', 'blob_id': 'native-only'}]}}
    projected = project_frame(original, binding)
    assert projected['payload']['content'][1] == {
        'type': 'image', 'media_type': 'image/png', 'url': refs[1]['url'], 'alt': 'night.png'}
    assert 'notes \\[draft\\].txt' in projected['payload']['content'][0]['text']
    assert 'blob_id' not in json.dumps(projected)
    assert original['payload']['content'][0]['text'] == 'Private scaffold'
    # The replay index takes references from the owned saved message, not a
    # similarly shaped foreign URL in the host prompt's current-message copy.
    context = {'guide_id': 'setup-one', 'current_user_message': {**message, 'attachments': [{'url': 'foreign'}]}}
    frame = {'kind': 'user_input', 'cursor': 'console:9', 'interaction_id': 'native-one', 'payload': {
        'content': [{'type': 'text', 'text': 'Current setup turn:\n' + json.dumps(context)},
                    {'type': 'image', 'source': 'blob', 'blob_id': 'native-only'}]}}
    recovered = legacy_binding(frame, {'id': 'setup-one', 'messages': [message]})
    assert recovered['attachments'] == refs
    refs[0]['filename'] = 'changed.txt'
    assert recovered['attachments'][0]['filename'] == 'notes [draft].txt'


@pytest.mark.asyncio
async def test_timeline_scopes_native_stream_and_replays_without_resending(tmp_path, monkeypatch):
    store = GuideStore(tmp_path)
    state = store.create(starter_state({}))
    state["messages"] += [{"id": "user-1", "role": "user", "text": "Our own context"},
                          {"id": "assistant-1", "role": "assistant", "text": "The saved reply"}]
    store.save(state, 1)
    starts = []
    async def start():
        starts.append(True)
    architect = SimpleNamespace(start=start, runtime=SimpleNamespace(rust_http_base_url="http://native.test"))
    timeline = GuideTimeline(store, architect)
    operation = {"request_id": "a" * 64, "status": "running", "started_at": "2026-10-04T09:00:00Z"}
    store.save_operation(state["id"], operation)
    await timeline.admitted({"guide_id": state["id"], "current_user_message": state["messages"][-2],
                             "operation_request_id": operation["request_id"], "private_scaffold": "Not public metadata"},
                            {"cursor": "console:7", "interaction_id": "our-turn"})
    frames = [{"cursor": "console:7", "interaction_id": "our-turn", "kind": "user_input", "payload": {"content": "Secret scaffold"}},
              {"cursor": "console:8", "interaction_id": "other-turn", "kind": "text_delta", "payload": {"delta": "Another setup"}},
              {"cursor": "console:9", "interaction_id": "our-turn", "kind": "reasoning_delta", "payload": {"delta": "Public provider summary"}}]
    async def page(client, after, limit=500):
        assert after == "console:6"
        return {"frames": frames, "next_cursor": "console:9", "latest_cursor": "console:9"}
    monkeypatch.setattr(timeline, "_page", page)
    result = await timeline.read(state["id"])
    assert [frame["cursor"] for frame in result["frames"]] == ["console:7", "console:9"]
    assert result["frames"][0]["payload"]["content"] == "Our own context"
    assert result["bindings"][0]["assistant_message_id"] == "assistant-1"
    assert result["bindings"][0]["operation_request_id"] == operation["request_id"]
    assert result["bindings"][0]["operation_started_at"] == operation["started_at"]
    assert "private_scaffold" not in result["bindings"][0]
    assert result["exhausted"]
    assert len(store.interactions(state["id"])) == 1
    # A replacement host restores the binding from app-owned receipts.
    restored = GuideTimeline(store, architect).store.interactions(state["id"])[0]
    assert restored["interaction_id"] == "our-turn"
    assert restored["operation_request_id"] == operation["request_id"]
    assert restored["operation_started_at"] == operation["started_at"]


@pytest.mark.asyncio
async def test_same_request_retry_retains_each_exact_owned_admission_attempt(tmp_path):
    store = GuideStore(tmp_path)
    state = store.create(starter_state({}))
    timeline = GuideTimeline(store, SimpleNamespace())
    context = {"guide_id": state["id"], "operation_request_id": "a" * 64,
               "current_user_message": {"id": "first-user", "text": "Keep the same request"}}
    operation = {"request_id": context["operation_request_id"], "status": "running",
                 "started_at": "2026-10-04T09:00:00Z"}
    store.save_operation(state["id"], operation)
    await timeline.admitted(context, {"cursor": "console:7", "interaction_id": "first-turn"})
    retried = {**operation, "started_at": "2026-10-04T09:05:00Z"}
    store.save_operation(state["id"], retried)
    await timeline.admitted({**context, "current_user_message": {"id": "retry-user", "text": "Keep the same request"}},
                            {"cursor": "console:40", "interaction_id": "retry-turn"})
    first, retry = store.interactions(state["id"])
    assert first["operation_request_id"] == retry["operation_request_id"] == operation["request_id"]
    assert first["operation_started_at"] == operation["started_at"]
    assert retry["operation_started_at"] == retried["started_at"]
    assert first["interaction_id"] == "first-turn" and retry["interaction_id"] == "retry-turn"


@pytest.mark.asyncio
@pytest.mark.parametrize("operation", [None, {"request_id": "b" * 64, "status": "running", "started_at": "other"},
    {"request_id": "a" * 64, "status": "completed", "started_at": "old"},
    {"request_id": "a" * 64, "status": "running", "started_at": None}])
async def test_admission_never_borrows_unknown_mismatched_or_finished_operation(tmp_path, operation):
    store = GuideStore(tmp_path)
    state = store.create(starter_state({}))
    if operation:
        store.save_operation(state["id"], operation)
    timeline = GuideTimeline(store, SimpleNamespace())
    await timeline.admitted({"guide_id": state["id"], "operation_request_id": "a" * 64,
                             "current_user_message": {"id": "user", "text": "Current request"}},
                            {"cursor": "console:7", "interaction_id": "our-turn"})
    binding = store.interactions(state["id"])[0]
    assert binding["operation_request_id"] == "a" * 64
    assert "operation_started_at" not in binding


@pytest.mark.asyncio
async def test_new_setup_does_not_boot_model_runtime_for_empty_history(tmp_path):
    store = GuideStore(tmp_path)
    state = store.create(starter_state({}))
    async def start():
        pytest.fail("No model runtime is needed for a blank setup")
    timeline = GuideTimeline(store, SimpleNamespace(start=start))
    result = await timeline.read(state["id"])
    assert not result["available"] and result["frames"] == []


@pytest.mark.asyncio
async def test_legacy_history_is_recovered_when_new_turn_already_has_binding(tmp_path, monkeypatch):
    store = GuideStore(tmp_path)
    state = store.create(starter_state({}))
    old_message = {"id": "old-user", "role": "user", "text": "Original intent"}
    new_message = {"id": "new-user", "role": "user", "text": "Refined intent"}
    state["messages"] += [old_message, new_message]
    store.save(state, 1)
    async def start():
        pass
    timeline = GuideTimeline(store, SimpleNamespace(start=start, runtime=SimpleNamespace(rust_http_base_url="http://native.test")))
    await timeline.admitted({"guide_id": state["id"], "current_user_message": new_message},
                            {"cursor": "console:20", "interaction_id": "new-turn"})
    old_frame = {"kind": "user_input", "cursor": "console:4", "interaction_id": "old-turn",
                 "payload": {"content": "Current setup turn:\n" + json.dumps({"guide_id": state["id"], "current_user_message": old_message})}}
    reads = []
    async def page(client, after, limit=500):
        reads.append(after)
        return {"frames": [old_frame], "next_cursor": "console:20", "latest_cursor": "console:20"}
    monkeypatch.setattr(timeline, "_page", page)
    result = await timeline.read(state["id"])
    assert {binding["interaction_id"] for binding in result["bindings"]} == {"old-turn", "new-turn"}
    assert reads == ["console:0", "console:3"]
    assert result["frames"][0]["payload"]["content"] == "Original intent"


@pytest.mark.asyncio
async def test_later_missing_receipt_recovers_after_previous_backfill(tmp_path, monkeypatch):
    store = GuideStore(tmp_path)
    state = store.create(starter_state({}))
    old_message = {"id": "old-user", "role": "user", "text": "Initial intent"}
    state["messages"].append(old_message)
    state = store.save(state, 1)
    async def start():
        pass
    timeline = GuideTimeline(store, SimpleNamespace(start=start, runtime=SimpleNamespace(rust_http_base_url="http://native.test")))
    def native_frame(message, cursor, interaction):
        return {"kind": "user_input", "cursor": cursor, "interaction_id": interaction,
                "payload": {"content": "Current setup turn:\n" + json.dumps({"guide_id": state["id"], "current_user_message": message})}}
    frames = [native_frame(old_message, "console:4", "old-turn")]
    async def page(client, after, limit=500):
        return {"frames": frames, "next_cursor": frames[-1]["cursor"], "latest_cursor": frames[-1]["cursor"]}
    monkeypatch.setattr(timeline, "_page", page)
    assert len((await timeline.read(state["id"]))["bindings"]) == 1
    new_message = {"id": "new-user", "role": "user", "text": "Later intent"}
    state["messages"].append(new_message)
    store.save(state, state["version"])
    frames.append(native_frame(new_message, "console:20", "new-turn"))
    result = await timeline.read(state["id"], after="console:4")
    assert {binding["interaction_id"] for binding in result["bindings"]} == {"old-turn", "new-turn"}
