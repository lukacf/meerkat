import copy
import importlib.util
import json
import os
import subprocess
import sys
from pathlib import Path

import pytest

from launchkit.guide import GuideService, compact_guidance_context
from launchkit.guide_store import GuideStore
from launchkit.organization import load_organization_plugin, organization_snapshot
from test_guide import guide_client

EXAMPLE = Path(__file__).parents[1] / "examples/workplace-starters/workplace_starters.py"


def example():
    spec = importlib.util.spec_from_file_location("example_starters", EXAMPLE)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.organization()


def test_selected_external_distribution_is_loaded_through_its_entry_point(tmp_path, monkeypatch):
    # Use real importlib distribution discovery, not a mock of the plugin loader.
    (tmp_path / "external_starters.py").write_text(
        "def contribution():\n    return " + repr(example()) + "\n"
    )
    metadata = tmp_path / "external_starters-1.0.dist-info"
    metadata.mkdir()
    (metadata / "METADATA").write_text("Name: external-starters\nVersion: 1.0\n")
    (metadata / "entry_points.txt").write_text(
        "[meerkat_forge.organizations.v1]\nexternal = external_starters:contribution\n"
    )
    monkeypatch.syspath_prepend(str(tmp_path))
    monkeypatch.setenv("META_FORGE_ORGANIZATION", "external")
    assert load_organization_plugin() == organization_snapshot(example())


def test_no_selection_loads_no_plugins(monkeypatch):
    monkeypatch.delenv("META_FORGE_ORGANIZATION", raising=False)
    monkeypatch.setattr("launchkit.organization.entry_points", lambda **kw: pytest.fail("No discovery needed"))
    assert load_organization_plugin() is None


@pytest.mark.parametrize("selection", ["", "missing"])
def test_explicit_invalid_selection_does_not_fall_back(selection):
    with pytest.raises(ValueError):
        load_organization_plugin(selection)


def test_ambiguous_plugin_is_not_executed(monkeypatch):
    monkeypatch.setattr("launchkit.organization.entry_points", lambda **kw: [object(), object()])
    with pytest.raises(ValueError, match="exactly once"):
        load_organization_plugin("ambiguous")


def test_invalid_plugin_values_are_not_in_the_launcher_error(monkeypatch):
    class Entry:
        def load(self):
            return lambda: {"password": "not-for-a-log"}
    monkeypatch.setattr("launchkit.organization.entry_points", lambda **kw: [Entry()])
    with pytest.raises(ValueError) as error:
        load_organization_plugin("invalid")
    assert "not-for-a-log" not in str(error.value)
    assert error.value.__suppress_context__


def test_plugin_cannot_supply_execution_or_credential_fields():
    for extra in [{"credentials": {"secret": "value"}}, {"agents": []}, {"deployment": {"command": "run"}}]:
        with pytest.raises(ValueError):
            organization_snapshot(example() | extra)
    invalid = example()
    invalid["starting_examples"][0]["grants"] = ["all"]
    with pytest.raises(ValueError):
        organization_snapshot(invalid)


def test_empty_examples_are_intentional_and_duplicate_ids_are_rejected():
    assert organization_snapshot(example() | {"starting_examples": []})["starting_examples"] == []
    invalid = example()
    invalid["starting_examples"].append(copy.deepcopy(invalid["starting_examples"][0]))
    with pytest.raises(ValueError):
        organization_snapshot(invalid)


@pytest.mark.parametrize("version", [True, 1.0, "1", 2])
def test_only_the_declared_integer_schema_version_is_accepted(version):
    with pytest.raises(ValueError):
        organization_snapshot(example() | {"schema_version": version})


@pytest.mark.asyncio
async def test_saved_setup_retains_organization_across_host_upgrade_and_recreation(tmp_path, monkeypatch):
    monkeypatch.setattr("launchkit.guide.planner_capability", lambda: {"available": False})
    first = example()
    service = GuideService(GuideStore(tmp_path), object(), organization=first)
    created = await service.create()
    first["starting_examples"].clear()
    assert created["organization"]["starting_examples"]
    assert created["brief"]["facts"] == {}
    assert created["proposal"] is None and created["blueprint"] is None
    second = example() | {"version": "2.0.0", "starting_examples": []}
    restarted = GuideService(GuideStore(tmp_path), object(), organization=second)
    assert restarted.store.get(created["id"])["organization"] == created["organization"]
    assert restarted.new()["organization"]["starting_examples"] == []
    assert "organization" not in GuideService(GuideStore(tmp_path), object()).new()
    new = restarted.new()
    new["organization"]["label"] = "User edit"
    assert restarted.new()["organization"]["label"] == "Example workplace"


def test_guidance_projection_retains_same_examples_without_promoting_them_to_facts():
    context = {"organization": organization_snapshot(example()), "brief": {"facts": {}, "requirements": []}}
    projected = compact_guidance_context(context)
    assert projected["organization"] == context["organization"]
    assert projected["brief"]["facts"] == {}
    projected["organization"]["starting_examples"].clear()
    assert context["organization"]["starting_examples"]


@pytest.mark.parametrize("previous", [None, "1.0"])
def test_explicit_context_adoption_keeps_accepted_plan_history_and_retry_identity(guide_client, monkeypatch, previous):
    from launchkit.blueprint import starter_blueprint
    from test_guide import start
    client, agent, service = guide_client
    state = start(client)
    state = service.store.get(state["id"])
    state.update(status="accepted", proposal=starter_blueprint().model_dump(mode="json"))
    if previous is not None:
        state["organization"] = organization_snapshot(example() | {"version": previous})
    saved = service.store.save(state, state["version"])
    service.organization = organization_snapshot(example() | {"version": "2.0",
        "guide_context": "  Use the workplace tool instructions as written.\nNo catalog is required.\n"})
    monkeypatch.setattr("launchkit.guide.planner_capability", lambda: {"available": False})
    visible = client.get(f"/api/guide/{saved['id']}").json()
    assert visible.get("organization") == saved.get("organization")
    assert visible["organization_update"]["context"] == service.organization
    assert service.store.get(saved["id"]) == saved  # GET does not adopt.
    request = {"version": saved["version"], "action": "organization",
        "organization_fingerprint": visible["organization_update"]["fingerprint"]}
    response = client.post(f"/api/guide/{saved['id']}/respond", json=request)
    assert response.status_code == 200, response.text
    current = service.store.get(saved["id"])
    assert current["organization"] == service.organization
    assert "organization_update" not in current
    assert current["version"] == saved["version"] + 1
    for key in ("brief", "proposal", "blueprint", "status", "answers", "fact_history"):
        assert current.get(key, []) == saved.get(key, [])
    assert current["messages"][:-1] == saved["messages"]
    assert current["messages"][-1]["action"] == "organization"
    assert client.post(f"/api/guide/{saved['id']}/respond", json=request).status_code == 200
    assert service.store.get(saved["id"]) == current
    assert "organization_update" not in client.get(f"/api/guide/{saved['id']}").json()
    assert agent.calls == []


@pytest.mark.parametrize("change", ["document", "removed", "guide-version"])
def test_context_adoption_rejects_a_changed_displayed_selection(guide_client, change):
    from test_guide import start
    client, agent, service = guide_client
    saved = start(client)
    service.organization = organization_snapshot(example())
    visible = client.get(f"/api/guide/{saved['id']}").json()
    if change == "document":
        service.organization["guide_context"] += " Changed after display."
    elif change == "removed":
        service.organization = None
    else:
        row = service.store.get(saved["id"])
        service.store.save(row, row["version"])
    before = service.store.get(saved["id"])
    response = client.post(f"/api/guide/{saved['id']}/respond", json={
        "version": saved["version"], "action": "organization",
        "organization_fingerprint": visible["organization_update"]["fingerprint"]})
    assert response.status_code == 409, response.text
    assert service.store.get(saved["id"]) == before and agent.calls == []


@pytest.mark.parametrize("extra", [{"text": "Also change my plan"}, {"selected": ["another"]},
    {"stage": "connections"}, {"organization": {"guide_context": "browser-owned"}},
    {"action": "chat"}, {"organization_fingerprint": None}])
def test_context_action_cannot_smuggle_document_or_other_guide_changes(guide_client, extra):
    from test_guide import start
    client, agent, service = guide_client
    saved = start(client)
    before = service.store.get(saved["id"])
    response = client.post(f"/api/guide/{saved['id']}/respond", json={
        "version": saved["version"], "action": "organization", "organization_fingerprint": "a" * 64, **extra})
    assert response.status_code == 422
    assert service.store.get(saved["id"]) == before and agent.calls == []


def test_selected_examples_reach_actual_guide_turn_and_survive_next_read(guide_client):
    from test_guide import answer, start
    client, agent, service = guide_client
    service.organization = organization_snapshot(example())
    opening = client.get('/api/guide/new').json()
    state = start(client)
    assert opening['organization'] == state['organization']
    assert agent.calls == []
    text = state['organization']['starting_examples'][0]['initial_brief']
    updated = answer(client, state, text=text)
    context = agent.calls[-1]['guide_context']
    assert context['organization'] == state['organization']
    assert compact_guidance_context(context)['organization'] == state['organization']
    assert context['current_user_message']['text'] == text
    assert 'not the user' in agent.calls[-1]['prompt']
    assert updated['organization'] == state['organization']
    service.organization = organization_snapshot(example() | {'version': '2.0', 'starting_examples': []})
    assert client.get(f"/api/guide/{state['id']}").json()['organization'] == state['organization']


def test_real_server_loads_selected_plugin_and_refuses_a_missing_one(tmp_path):
    (tmp_path / "server_starters.py").write_text(
        "def contribution():\n    return " + repr(example()) + "\n"
    )
    metadata = tmp_path / "server_starters-1.0.dist-info"
    metadata.mkdir()
    (metadata / "METADATA").write_text("Name: server-starters\nVersion: 1.0\n")
    (metadata / "entry_points.txt").write_text(
        "[meerkat_forge.organizations.v1]\nserver-test = server_starters:contribution\n"
    )
    source = str(Path(__file__).resolve().parents[1])
    env = {"PATH": os.environ.get("PATH", ""), "HOME": str(tmp_path),
           "PYTHONPATH": os.pathsep.join((str(tmp_path), source)),
           "PYTHONDONTWRITEBYTECODE": "1", "META_FORGE_HOME": str(tmp_path / "state"),
           "META_FORGE_ORGANIZATION": "server-test"}
    code = """
import json
from fastapi.testclient import TestClient
from launchkit.server import app
client = TestClient(app)
response = client.get('/api/guide/new')
assert response.status_code == 200
print(json.dumps(response.json()['organization']))
"""
    result = subprocess.run([sys.executable, "-c", code], env=env, cwd=tmp_path,
                            capture_output=True, text=True, timeout=30)
    assert result.returncode == 0, result.stderr
    assert json.loads(result.stdout) == organization_snapshot(example())
    env["META_FORGE_ORGANIZATION"] = "missing-server-plugin"
    refused = subprocess.run([sys.executable, "-c", code], env=env, cwd=tmp_path,
                             capture_output=True, text=True, timeout=30)
    assert refused.returncode != 0
    assert "must be installed exactly once" in refused.stderr
    assert "missing-server-plugin" not in refused.stderr


@pytest.mark.parametrize("adopt_update", [False, True])
def test_chat_keeps_request_identity_with_pending_or_adopted_organization(guide_client, adopt_update):
    from launchkit.projects import fingerprint
    from launchkit.server import GuideResponse
    from test_guide import start
    client, agent, service = guide_client
    service.organization = organization_snapshot(example() | {"version": "1.0"})
    saved = start(client)
    service.organization = organization_snapshot(example() | {"version": "2.0"})
    visible = client.get(f"/api/guide/{saved['id']}").json()
    assert visible["organization_update"]["fingerprint"] == fingerprint(service.organization)
    if adopt_update:
        adopted = client.post(f"/api/guide/{saved['id']}/respond", json={
            "version": saved["version"], "action": "organization",
            "organization_fingerprint": visible["organization_update"]["fingerprint"]})
        assert adopted.status_code == 200, adopted.text
        saved = service.store.get(saved["id"])
    assert agent.calls == []
    request = {"version": saved["version"], "action": "chat",
               "text": "Compare two heat-pump proposals and cite their source text."}
    response = client.post(f"/api/guide/{saved['id']}/respond", json=request)
    assert response.status_code == 200, response.text
    current = service.store.get(saved["id"])
    assert len(agent.calls) == 1
    context = agent.calls[0]["guide_context"]
    expected = fingerprint({"guide_id": saved["id"], **GuideResponse.model_validate(request).model_dump(mode="json")})
    assert context["operation_request_id"] == expected
    assert service.store.operation(saved["id"])["request_id"] == expected
    assert current["organization"] == saved["organization"] == context["organization"]
    assert current["messages"][:len(saved["messages"])] == saved["messages"]
    assert current["version"] == saved["version"] + 1
    replay = client.post(f"/api/guide/{saved['id']}/respond", json=request)
    assert replay.status_code == 200, replay.text
    assert len(agent.calls) == 1 and service.store.get(saved["id"]) == current
