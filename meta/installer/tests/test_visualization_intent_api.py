"""Exercise the real HTTP intent boundary without runtime or provider work."""
from copy import deepcopy
from types import SimpleNamespace
import asyncio

import pytest
from fastapi.testclient import TestClient


CONSUMER = "18017cba-330e-4282-9e1d-1f331d82cd19"
DECLARATION = {"consumer_id": CONSUMER, "intent_revision": 1,
               "setup_id": "setup-a", "stream_id": "setup-a:__overview__"}
INTENT = {**DECLARATION, "intent_token": "4881aaf1-8d35-4469-b45e-9206b4e4ab35"}


@pytest.fixture
def intent_api(monkeypatch):
    from launchkit import server

    calls = []

    class Operations:
        fail = False

        async def action(self, kind, **value):
            if self.fail:
                raise server.VisualizationIntentConflict("An internal scheduling detail.")
            calls.append((kind, deepcopy(value)))
            return {**value, "status": kind, "lease_seconds": 90, "expires_at": 1234567890}

        async def declare_intent(self, **value):
            return await self.action("declared", **value)

        async def release_intent(self, **value):
            return await self.action("released", **value)

        async def renew_intent(self, **value):
            return await self.action("renewed", **value)

        async def submit(self, service, blueprint, model, **value):
            await self.action("submitted", **value)
            return {"id": "f" * 64, "status": "queued"}

        def latest_saved(self, service, **value):
            calls.append(('latest', deepcopy(value)))
            return {'id': 'a' * 64, 'status': 'completed'}

        async def adopt_saved(self, service, **value):
            await self.action('adopted', **value)
            return {'id': value['publication']['operation_id'], 'status': 'completed'}

    operations = Operations()
    monkeypatch.setattr(server, "visualization_operations", operations)
    monkeypatch.setattr(server, "architect", SimpleNamespace(visualizer=object()))
    return TestClient(server.app), server, operations, calls


def payload():
    from launchkit.blueprint import starter_blueprint
    blueprint = starter_blueprint()
    return {"blueprint": blueprint.model_dump(mode="json"),
            "model": {"nodes": [{"id": identity, "kind": "agent"} for identity in blueprint.identities()],
                      "edges": [], "phases": []},
            "stream_id": DECLARATION["stream_id"], "setup_id": DECLARATION["setup_id"]}


def test_real_routes_forward_exact_declaration_release_and_renewal(intent_api):
    client, _, _, calls = intent_api
    assert client.post("/api/visualization-intents/declare", json=DECLARATION).status_code == 200
    assert client.post("/api/visualization-intents/release", json=DECLARATION).status_code == 200
    assert client.post("/api/visualization-intents/renew", json=INTENT).status_code == 200
    assert calls == [("declared", DECLARATION), ("released", {**DECLARATION, "intent_token": None}),
                     ("renewed", INTENT)]


def test_saved_selection_routes_forward_exact_scope_and_declared_owner_without_submit(intent_api):
    client, _, _, calls = intent_api
    scope = {'setup_id': DECLARATION['setup_id'], 'stream_id': DECLARATION['stream_id'],
             'blueprint_hash': 'b' * 64, 'model_hash': 'c' * 64}
    parent = {**scope, 'operation_id': 'a' * 64, 'code_hash': 'd' * 64}
    assert client.get('/api/visualizations/published/latest', params=scope).status_code == 200
    assert client.post('/api/visualizations/published/adopt', json={
        'publication': parent, 'consumer_intent': INTENT}).status_code == 200
    assert calls == [('latest', scope), ('adopted', {'publication': parent, 'consumer_intent': INTENT, 'require_latest': True})]


def test_saved_selection_requires_explicit_owner_and_rejects_retired_intent(intent_api):
    client, _, operations, calls = intent_api
    parent = {'setup_id': DECLARATION['setup_id'], 'stream_id': DECLARATION['stream_id'],
        'operation_id': 'a' * 64, 'code_hash': 'd' * 64, 'blueprint_hash': 'b' * 64, 'model_hash': 'c' * 64}
    assert client.post('/api/visualizations/published/adopt', json={'publication': parent}).status_code == 422
    operations.fail = True
    response = client.post('/api/visualizations/published/adopt', json={'publication': parent, 'consumer_intent': INTENT})
    assert response.status_code == 409 and response.json()['code'] == 'stale_visualization_intent'
    assert calls == [] and 'internal' not in response.text


def test_unsaved_preview_can_declare_and_submit_with_no_setup_id(intent_api):
    client, _, _, calls = intent_api
    unsaved = {**DECLARATION, "setup_id": None, "stream_id": "unsaved:__overview__"}
    intent = {**unsaved, "intent_token": INTENT["intent_token"]}
    assert client.post("/api/visualization-intents/declare", json=unsaved).status_code == 200
    response = client.post("/api/visualizations", json={**payload(), "setup_id": None,
        "stream_id": unsaved["stream_id"], "consumer_intent": intent})
    assert response.status_code == 202
    assert calls[0] == ("declared", unsaved)
    assert calls[1][1]["consumer_intent"] == intent


@pytest.mark.parametrize("route,body", [("declare", DECLARATION), ("release", INTENT), ("renew", INTENT)])
def test_intent_conflict_is_explicit_and_does_not_echo_internal_detail(intent_api, route, body):
    client, _, operations, calls = intent_api
    operations.fail = True
    response = client.post(f"/api/visualization-intents/{route}", json=body)
    assert response.status_code == 409
    assert response.json()["code"] == "stale_visualization_intent"
    assert "internal" not in response.text and calls == []


@pytest.mark.parametrize("change", [{"consumer_id": "shared-storage-id"}, {"intent_revision": True},
    {"intent_revision": 0}, {"intent_revision": 9007199254740992}, {"stream_id": " "},
    {"setup_id": ""}, {"native_request_key": "f" * 64}])
def test_invalid_or_unexpected_declaration_never_reaches_coordinator(intent_api, change):
    client, _, _, calls = intent_api
    assert client.post("/api/visualization-intents/declare", json={**DECLARATION, **change}).status_code == 422
    assert calls == []


def test_renew_requires_token_and_cross_origin_requests_are_rejected(intent_api):
    client, _, _, calls = intent_api
    assert client.post("/api/visualization-intents/renew", json=DECLARATION).status_code == 422
    response = client.post("/api/visualization-intents/declare", json=DECLARATION,
                           headers={"Origin": "https://other.example"})
    assert response.status_code == 403 and calls == []


def test_visualization_body_passes_scheduling_separately_and_preserves_legacy_call(intent_api):
    client, _, _, calls = intent_api
    factual = payload()
    managed = client.post("/api/visualizations", json={**factual, "consumer_intent": INTENT})
    legacy = client.post("/api/visualizations", json=factual)
    assert managed.status_code == legacy.status_code == 202
    assert calls[0] == ("submitted", {"feedback": "", "stream_id": factual["stream_id"],
        "setup_id": factual["setup_id"], "consumer_intent": INTENT})
    assert calls[1] == ("submitted", {"feedback": "", "stream_id": factual["stream_id"],
        "setup_id": factual["setup_id"]})


def test_stale_body_is_distinct_from_ordinary_correction_conflict(intent_api):
    client, _, operations, calls = intent_api
    operations.fail = True
    response = client.post("/api/visualizations", json={**payload(), "consumer_intent": INTENT})
    assert response.status_code == 409 and response.json()["code"] == "stale_visualization_intent"
    assert calls == []


def test_saved_visual_review_authorization_still_precedes_managed_correction(intent_api):
    client, _, _, calls = intent_api
    parent = {"operation_id": "a" * 64, "code_hash": "b" * 64,
              "blueprint_hash": "c" * 64, "model_hash": "d" * 64,
              "setup_id": DECLARATION["setup_id"], "stream_id": DECLARATION["stream_id"]}
    response = client.post("/api/visualizations", json={**payload(), "consumer_intent": INTENT,
        "feedback": "Correct the explanation.", "expected_parent": parent, "review_id": "e" * 64})
    assert response.status_code == 409
    assert "code" not in response.json() and calls == []


def test_exact_parent_restoration_is_an_admission_restriction_not_a_correction(intent_api):
    client, _, _, calls = intent_api
    parent = {"operation_id": "a" * 64, "code_hash": "b" * 64,
              "blueprint_hash": "c" * 64, "model_hash": "d" * 64,
              "setup_id": DECLARATION["setup_id"], "stream_id": DECLARATION["stream_id"]}
    request = {**payload(), "consumer_intent": INTENT, "replay_parent": parent}
    assert client.post('/api/visualizations', json=request).status_code == 202
    assert calls[-1][1]['replay_parent'] == parent
    before = deepcopy(calls)
    for correction in [{'expected_parent': parent}, {'review_id': 'e' * 64}]:
        assert client.post('/api/visualizations', json={**request, **correction}).status_code == 409
    assert calls == before


def test_authorized_visual_correction_keeps_exact_parent_and_consumer_binding(intent_api, monkeypatch):
    client, server, _, calls = intent_api
    parent = {"operation_id": "a" * 64, "code_hash": "b" * 64,
              "blueprint_hash": "c" * 64, "model_hash": "d" * 64,
              "setup_id": DECLARATION["setup_id"], "stream_id": DECLARATION["stream_id"]}
    feedback = "Explain the domain collaboration clearly."
    review_id = "e" * 64
    review = {"status": "reviewed", "correction_advisory":
        {"expected_parent": parent, "review_id": review_id, "feedback": feedback}}
    monkeypatch.setattr(server, "visualization_reviews", SimpleNamespace(get=lambda identity:
        review if identity == review_id else None))
    response = client.post("/api/visualizations", json={**payload(), "consumer_intent": INTENT,
        "feedback": feedback, "expected_parent": parent, "review_id": review_id})
    assert response.status_code == 202
    assert calls[0][1]["expected_parent"] == parent
    assert calls[0][1]["consumer_intent"] == INTENT
    assert calls[0][1]["feedback"] == feedback


@pytest.mark.asyncio
@pytest.mark.parametrize("setup_id", ["setup-a", None])
async def test_real_http_flow_shares_demand_and_fences_delayed_old_body(monkeypatch, tmp_path, setup_id):
    import httpx
    from launchkit import server
    from launchkit.projects import fingerprint
    from launchkit.visualization_operations import VisualizationOperations

    class Service:
        calls = []

        def request_binding(self, blueprint, model, feedback=""):
            return {"blueprint_hash": fingerprint(blueprint), "model_hash": fingerprint(model),
                    "native_request_key": fingerprint({"blueprint": blueprint, "model": model, "feedback": feedback})}

        async def generate(self, blueprint, model, *, feedback=""):
            self.calls.append(feedback)
            return {"code": "trusted fixture output", "summary": "HTTP fixture", "key": self.request_binding(blueprint, model, feedback)["native_request_key"]}

    service = Service()
    operations = VisualizationOperations(root=tmp_path / "operations")
    monkeypatch.setattr(server, "architect", SimpleNamespace(visualizer=service))
    monkeypatch.setattr(server, "visualization_operations", operations)
    await operations.generation_lock.acquire()

    def binding(ack):
        return {name: ack[name] for name in (*DECLARATION, "intent_token")}

    try:
        async with httpx.AsyncClient(transport=httpx.ASGITransport(app=server.app),
                base_url="http://testserver", trust_env=False) as client:
            declaration = {**DECLARATION, "setup_id": setup_id}
            first_response = await client.post("/api/visualization-intents/declare", json=declaration)
            assert first_response.status_code == 200
            first = first_response.json()
            other = {**declaration, "consumer_id": "4e3fd6a9-0d2c-4a52-8c18-9ca961d81360"}
            second = (await client.post("/api/visualization-intents/declare", json=other)).json()
            factual = {**payload(), "setup_id": setup_id}
            one = await client.post("/api/visualizations", json={**factual, "consumer_intent": binding(first)})
            two = await client.post("/api/visualizations", json={**factual, "consumer_intent": binding(second)})
            assert one.status_code == two.status_code == 202 and one.json()["id"] == two.json()["id"]

            changed = {**declaration, "intent_revision": 2, "stream_id": "setup-a:another-view"}
            newest = (await client.post("/api/visualization-intents/declare", json=changed)).json()
            await client.post("/api/visualization-intents/release", json=binding(first))
            delayed = await client.post("/api/visualizations", json={**factual, "consumer_intent": binding(first)})
            assert delayed.status_code == 409 and delayed.json()["code"] == "stale_visualization_intent"
            latest = await client.post("/api/visualizations", json={**factual, "stream_id": changed["stream_id"],
                "feedback": "Explain the current view.", "consumer_intent": binding(newest)})
            assert latest.status_code == 202 and latest.json()["id"] != one.json()["id"]
            assert service.calls == [] and len(operations.tasks) == 2

            await client.post("/api/visualization-intents/release", json=binding(second))
            operations.generation_lock.release()
            await asyncio.wait_for(asyncio.gather(*list(operations.tasks.values()), return_exceptions=True), 2)
            assert service.calls == ["Explain the current view."]
            current = await client.get("/api/visualizations/" + latest.json()["id"])
            assert current.status_code == 200 and current.json()["status"] == "completed"
    finally:
        if operations.generation_lock.locked():
            operations.generation_lock.release()
        await operations.shutdown()
