"""Offline native admission fixtures, not proof of actual specialist quality."""
import asyncio
from copy import deepcopy
import json
from types import SimpleNamespace

import httpx
import pytest

from launchkit import architect as module
from launchkit.blueprint import starter_blueprint
from launchkit.projects import fingerprint
from test_architect_startup_recovery import FakeRuntime, startup


class Response:
    status_code = 200

    def __init__(self, payload):
        self.payload = payload

    def raise_for_status(self):
        pass

    def json(self):
        return self.payload


@pytest.fixture
async def consultation(tmp_path, monkeypatch):
    service = module.ArchitectService(tmp_path)
    service.set_context(starter_blueprint().model_dump(mode="json"))
    service.guide_context = {"guide_id": "current-guide"}
    service.runtime = SimpleNamespace(rust_http_base_url="http://native.invalid")
    calls = []
    controller = SimpleNamespace(pages=[], post_error=None, accepted={
        "interaction_id": "consultation-turn", "cursor": "console:10"},
        get_entered=asyncio.Event(), hold=False)

    class Client:
        def __init__(self, **kwargs):
            assert kwargs == {"timeout": 20, "trust_env": False}

        async def __aenter__(self):
            return self

        async def __aexit__(self, *args):
            pass

        async def post(self, url, **kwargs):
            calls.append(("post", url, deepcopy(kwargs)))
            if controller.post_error:
                raise controller.post_error
            return Response(controller.accepted)

        async def get(self, url, **kwargs):
            calls.append(("get", url, deepcopy(kwargs)))
            controller.get_entered.set()
            if controller.hold:
                await asyncio.Event().wait()
            return Response({"frames": controller.pages.pop(0)})

    monkeypatch.setattr(httpx, "AsyncClient", Client)
    await service.message_lock.acquire()
    yield service, controller, calls
    if service.message_lock.locked():
        service.message_lock.release()


def completion(**changes):
    return {"kind": "interaction_complete", "identity": "connectors:main",
        "interaction_id": "consultation-turn", "cursor": "console:11",
        "source": {"kind": "console_event"}, "payload": {"result": "Actual completed findings"}, **changes}


@pytest.mark.asyncio
async def test_consultation_waits_for_exact_native_terminal_without_changing_design(consultation):
    service, controller, calls = consultation
    before = deepcopy((service.context, service.proposal, service.guide_context, service.guide_result, service.discovered))
    controller.pages = [[completion(interaction_id="another-turn"), completion(identity="reviewer:main"), completion(
        source={"kind": "session_history"}, payload={"result": "Intermediate prose", "message": {"stop_reason": "tool_use"}}),
        completion(source={"kind": "synthetic"}, payload={"reason": "steer_delivered"}),
        completion(kind="tool_execution_completed", payload={"result": "Tool receipt"}),
        completion(payload={"result": "Premature prose", "source_event_type": "text_complete"}), completion()]]
    question = "Inspect the scoped catalog; return useful limitations.\nExact quoted context: \"hello\" 🌌"
    result = await service.consult_setup_peer({"peer": "connectors", "question": question})
    assert result["status"] == "completed" and result["output"] == "Actual completed findings"
    assert result["interaction_id"] == "consultation-turn" and "authority changed" in result["scope"]
    assert [kind for kind, _, _ in calls] == ["post", "get"]
    sent = calls[0][2]["json"]
    assert sent["identity"] == "connectors:main" and sent["origin"] == "setup-consultation"
    assert sent["handling_mode"] == "queue"
    assert json.loads(sent["content"].split("\n\nQuestion:\n", 1)[1]) == question
    assert "final assistant answer" in sent["content"] and "not a peer request" in sent["content"]
    assert sent["idempotency_key"]
    assert calls[1][2]["params"] == {"identity": "connectors:main", "after": "console:10", "limit": 500}
    assert (service.context, service.proposal, service.guide_context, service.guide_result, service.discovered) == before
    assert service.consultation_pending is None


@pytest.mark.asyncio
@pytest.mark.parametrize("args", [
    {"peer": "architect", "question": "Question"}, {"peer": "visualizer", "question": "Question"},
    {"peer": [], "question": "Question"}, {"peer": "reviewer", "question": ""},
    {"peer": "reviewer", "question": " \n"}, {"peer": "reviewer", "question": "x" * 6001},
    {"peer": "reviewer", "question": None}, {"peer": "reviewer", "question": "Question", "authority": "all"}])
async def test_invalid_consultation_cannot_admit_work(consultation, args):
    service, _, calls = consultation
    with pytest.raises(ValueError):
        await service.consult_setup_peer(args)
    assert not calls and service.consultation_pending is None


@pytest.mark.asyncio
async def test_consultation_requires_current_guided_runtime(consultation):
    service, _, calls = consultation
    service.guide_context = None
    assert (await service.consult_setup_peer({"peer": "reviewer", "question": "Question"}))["status"] == "unavailable"
    service.guide_context = {"guide_id": "retained-after-turn"}
    service.message_lock.release()
    assert (await service.consult_setup_peer({"peer": "reviewer", "question": "Question"}))["status"] == "unavailable"
    assert not calls


@pytest.mark.asyncio
async def test_timeout_retains_admitted_receipt_and_does_not_resubmit(consultation, monkeypatch):
    service, controller, calls = consultation
    controller.hold = True
    timeout = asyncio.timeout
    budgets = []

    def bounded(seconds):
        budgets.append(seconds)
        return timeout(0.01)

    monkeypatch.setattr(module.asyncio, "timeout", bounded)
    result = await service.consult_setup_peer({"peer": "connectors", "question": "Question"})
    assert budgets == [110] and result["status"] == "pending" and result["admission"] == "admitted"
    assert result["interaction_id"] == "consultation-turn" and result["cursor"] == "console:10"
    assert result["question_hash"] == fingerprint("Question")
    result["interaction_id"] = "caller mutation"
    retry = await service.consult_setup_peer({"peer": "connectors", "question": "Question"})
    assert retry["interaction_id"] == "consultation-turn"
    assert sum(kind == "post" for kind, _, _ in calls) == 1


@pytest.mark.asyncio
async def test_pending_observation_completes_without_post_then_allows_a_new_question(consultation, monkeypatch):
    service, controller, calls = consultation
    controller.hold = True
    timeout = asyncio.timeout
    monkeypatch.setattr(module.asyncio, "timeout", lambda seconds: timeout(0.01))
    first = await service.consult_setup_peer({"peer": "connectors", "question": "Original question"})
    assert first["status"] == "pending"
    monkeypatch.setattr(module.asyncio, "timeout", timeout)
    controller.hold = False
    controller.pages = [[completion()]]
    observed = await service.consult_setup_peer({"peer": "reviewer", "question": "Different question"})
    assert observed["status"] == "completed" and observed["identity"] == "connectors:main"
    assert observed["question_hash"] == fingerprint("Original question")
    assert sum(kind == "post" for kind, _, _ in calls) == 1
    controller.accepted = {"interaction_id": "second-turn", "cursor": "console:20"}
    controller.pages = [[completion(identity="reviewer:main", interaction_id="second-turn")]]
    second = await service.consult_setup_peer({"peer": "reviewer", "question": "Different question"})
    assert second["identity"] == "reviewer:main" and second["question_hash"] == fingerprint("Different question")
    assert sum(kind == "post" for kind, _, _ in calls) == 2


@pytest.mark.asyncio
async def test_cancellation_and_nested_call_do_not_resubmit_or_claim_stopped_work(consultation):
    service, controller, calls = consultation
    controller.hold = True
    task = asyncio.create_task(service.consult_setup_peer({"peer": "connectors", "question": "Question"}))
    await controller.get_entered.wait()
    nested = await service.consult_setup_peer({"peer": "reviewer", "question": "Different question"})
    assert nested == {"status": "unavailable", "reason": "consultation_in_progress"}
    task.cancel()
    with pytest.raises(asyncio.CancelledError):
        await task
    pending = service.consultation_pending
    assert pending["status"] == "pending" and pending["admission"] == "admitted"
    assert not service.consultation_lock.locked()
    assert sum(kind == "post" for kind, _, _ in calls) == 1


@pytest.mark.asyncio
@pytest.mark.parametrize("error", ["lost-post", "missing-binding"])
async def test_unknown_admission_never_resends_or_exposes_error_content(consultation, error):
    service, controller, calls = consultation
    if error == "lost-post":
        controller.post_error = httpx.TransportError("PRIVATE_EXCEPTION_CANARY")
    else:
        controller.accepted = {"cursor": "console:10"}
    result = await service.consult_setup_peer({"peer": "reviewer", "question": "Question"})
    assert result["status"] == "pending" and result["admission"] == "unknown"
    assert "interaction_id" not in result and "PRIVATE_EXCEPTION_CANARY" not in str(result)
    assert await service.consult_setup_peer({"peer": "reviewer", "question": "Question"}) == result
    assert [kind for kind, _, _ in calls] == ["post"]


@pytest.mark.asyncio
@pytest.mark.parametrize("kind", ["interaction_failed", "run_failed", "message_delivery_failed"])
async def test_exact_native_failure_is_not_success_and_does_not_expose_payload(consultation, kind):
    service, controller, calls = consultation
    controller.pages = [[completion(interaction_id="other", kind=kind), completion(kind=kind,
        payload={"error": "PRIVATE_FAILURE_CANARY"})]]
    result = await service.consult_setup_peer({"peer": "connectors", "question": "Question"})
    assert result["status"] == "failed" and result["reason"] == "native_interaction_failed"
    assert "PRIVATE_FAILURE_CANARY" not in str(result) and "output" not in result
    assert service.consultation_pending is None and service.consultation_runtime is None
    assert sum(kind == "post" for kind, _, _ in calls) == 1
    controller.accepted = {"interaction_id": "later-turn", "cursor": "console:20"}
    controller.pages = [[completion(identity="reviewer:main", interaction_id="later-turn")]]
    later = await service.consult_setup_peer({"peer": "reviewer", "question": "Later question"})
    assert later["status"] == "completed" and later["interaction_id"] == "later-turn"
    assert later["identity"] == "reviewer:main" and later["question_hash"] == fingerprint("Later question")
    posts = [kwargs["json"] for method, _, kwargs in calls if method == "post"]
    assert len(posts) == 2 and posts[0]["idempotency_key"] != posts[1]["idempotency_key"]


@pytest.mark.asyncio
async def test_missing_terminal_output_does_not_use_identity_inspection(consultation):
    service, controller, _ = consultation
    controller.pages = [[completion(identity="reviewer:main", payload={})]]
    service.runtime.agent = lambda identity: pytest.fail("No session-wide inspection fallback is allowed")
    result = await service.consult_setup_peer({"peer": "reviewer", "question": "Question"})
    assert result["status"] == "inconclusive" and result["reason"] == "missing_completed_output"
    assert "output" not in result


@pytest.mark.asyncio
async def test_old_runtime_pending_binding_is_not_sent_to_a_new_owner(consultation):
    service, controller, calls = consultation
    controller.post_error = httpx.TransportError("Ambiguous admission")
    pending = await service.consult_setup_peer({"peer": "connectors", "question": "Question"})
    service.runtime = SimpleNamespace(rust_http_base_url="http://new-native.invalid")
    assert await service.consult_setup_peer({"peer": "reviewer", "question": "New question"}) == pending
    assert [kind for kind, _, _ in calls] == ["post"]


@pytest.mark.asyncio
async def test_consultation_registration_is_architect_only(startup):
    service, controller = startup
    registered = {}

    async def callbacks(builder):
        for profile in ("architect", "connectors", "reviewer", "visualizer"):
            tools = registered.setdefault(profile, {})
            def register(name, handler, **kwargs):
                tools[name] = (handler, kwargs)
            await builder.session_service_callback.build_agent(SimpleNamespace(profile_name=profile, register_tool=register))

    controller.callback_runner = callbacks
    controller.outcomes = [FakeRuntime()]
    await service.start()
    assert {profile for profile, tools in registered.items() if "consult_setup_peer" in tools} == {"architect"}
    handler, options = registered["architect"]["consult_setup_peer"]
    assert handler == service.consult_setup_peer
    schema = options["input_schema"]
    assert schema["properties"]["peer"]["enum"] == ["connectors", "reviewer"]
    assert schema["properties"]["question"]["maxLength"] == 6000 and schema["additionalProperties"] is False
    await service.shutdown()
