from meta_trial_fixtures import selected, meta_trial_runtime
"""Offline host regressions for distinct workspace and application input scopes."""
import asyncio
import json
from pathlib import Path
from types import SimpleNamespace

import pytest

from launchkit.blueprint import Blueprint, starter_blueprint
from launchkit.compiler import _runtime
from launchkit.implementation import read_receipt
from launchkit.rehearsal_worker import FixtureTools
from launchkit.templates.storage_setup import write_private

pytestmark = pytest.mark.asyncio


class Draft:
    def __init__(self, profile=None):
        self.profile_name = profile
        self.resume_session_id = None
        self.additional_instructions = []
        self.callbacks = {}
        self.descriptions = {}

    def register_tool(self, name, handler, **metadata):
        self.callbacks[name] = handler
        self.descriptions[name] = metadata["description"]


async def test_empty_workspace_does_not_hide_registered_application_callbacks(tmp_path):
    tools = FixtureTools(tmp_path, {}, [])
    draft = Draft()
    tools.register(draft, {"tools": {"workspace": True, "notes": "none"}},
        {"scope": "application trial", "available": ["sandbox application:read_inputs"]},
        application_tools=[{"name": "read_inputs"}])

    visible = await draft.callbacks["rehearsal_capabilities"]({})
    assert visible["scope"] == "application trial"
    assert visible["fixture_files"] == []
    assert visible["fixture_workspace"]["granted"] is True
    assert visible["fixture_workspace"]["files"] == []
    assert "excludes application JSON inputs" in visible["fixture_workspace"]["scope"]
    assert visible["application_callbacks"] == ["read_inputs"]
    assert "sandbox application:read_inputs" in visible["available"]
    assert draft.callbacks["files_list"]({})["entries"] == []
    assert draft.callbacks["files_read"]({"path": "unknown"})["ok"] is False
    assert (await draft.callbacks["rehearsal_capabilities"]({}))["application_callbacks"] == ["read_inputs"]
    assert "empty workspace does not establish" in draft.descriptions["rehearsal_capabilities"].lower()


async def test_ungranted_workspace_discloses_no_workspace_inventory_or_contents(tmp_path):
    tools = FixtureTools(tmp_path, {"private-name.txt": "private contents"}, [])
    draft = Draft()
    tools.register(draft, {"tools": {"workspace": False, "notes": "none"}},
        {"available": ["native peer comms"]})
    visible = await draft.callbacks["rehearsal_capabilities"]({})
    assert visible["fixture_files"] == []
    assert visible["fixture_workspace"]["granted"] is False
    assert visible["fixture_workspace"]["files"] == []
    assert visible["application_callbacks"] == []
    assert "files_list" not in draft.callbacks and "files_read" not in draft.callbacks
    assert "private-name" not in json.dumps(visible)
    assert "private contents" not in json.dumps(visible)


async def test_capability_results_are_detached_and_empty_catalog_stays_unavailable(tmp_path):
    tools = FixtureTools(tmp_path, {"available.txt": "workspace value"}, [])
    draft = Draft()
    capabilities = {"available": ["fixture workspace (read only)"], "masked": ["application adapters"]}
    tools.register(draft, {"tools": {"workspace": True, "notes": "none"}}, capabilities)
    first = await draft.callbacks["rehearsal_capabilities"]({})
    assert first["fixture_files"] == first["fixture_workspace"]["files"] == ["available.txt"]
    assert first["application_callbacks"] == []
    first["fixture_files"].append("invented.txt")
    first["fixture_workspace"]["files"].clear()
    first["application_callbacks"].append("invented_callback")
    first["available"].append("invented availability")
    second = await draft.callbacks["rehearsal_capabilities"]({})
    assert second["fixture_files"] == second["fixture_workspace"]["files"] == ["available.txt"]
    assert second["application_callbacks"] == []
    assert second["masked"] == ["application adapters"]
    assert capabilities["available"] == ["fixture workspace (read only)"]
    assert tools.files_read({"path": "available.txt"})["content"] == "workspace value"


@pytest.mark.parametrize("workspace_granted", [False, True])
async def test_worker_projects_only_each_identity_actual_callbacks_without_application_input_dump(
        tmp_path, monkeypatch, workspace_granted):
    """Exercise the real host builder with SDK, transport and app fully replaced offline."""
    import httpx
    import meerkat_meta
    from meerkat_meta.identity_first_models import DurableAgentSpec, ManagedPeerEdge
    import launchkit.rehearsal_worker as worker

    raw = starter_blueprint().model_dump(mode="json")
    for role in raw["agent_classes"]:
        role["tools"]["workspace"] = workspace_granted
    spec = Blueprint.model_validate(raw)
    rows = _runtime(spec)["agents"]
    owner = rows[0]["identity"]
    write_private(tmp_path / "input.json", {"blueprint": raw, "text": "Use the supplied application record.",
        "flow_id": None, "params": None, "fixtures": {"files": {}, "notes": []},
        "mode": "application", "scope": "application trial",
        "application_source": {"source_digest": "checked", "deployment": {}},
        "application_fixtures": {"private-owner-record": {"value": "OWNER_INPUT_SENTINEL"}}})
    write_private(tmp_path / "receipt.json", {"status": "queued"})
    drafts, traces, applications, policies = [], [], [], []
    classes = {row.id: row for row in spec.agent_classes}
    policy = {"roster": [DurableAgentSpec(identity=row["identity"], profile=row["profile"],
        display_name="Trial " + row["name"], addressability="addressable", runtime_mode_override="turn_driven",
        labels={"class": row["classId"], "role": row["role"], "console_group": classes[row["classId"]].name,
                "project": "sample context"}, context={"owner": row["identity"]},
        additional_instructions=["Use this application's agreed context."]).to_dict() for row in rows],
        "edges": [ManagedPeerEdge(a=owner, b=rows[1]["identity"]).to_dict()]}

    class OfflineApplication:
        def __init__(self, directory, source, digest, deployment, fixtures):
            self.fixtures, self.stopped = fixtures, False
            applications.append(self)
        async def start(self):
            return [{"identity": row["identity"], "tools": ([{"name": "read_owner_record",
                "description": "Read an authorized record discovered through this scoped callback.",
                "schema": {"type": "object"}}] if row["identity"] == owner else [])} for row in rows]
        async def call(self, identity, name, args):
            assert identity == owner and name == "read_owner_record"
            return self.fixtures["private-owner-record"]
        def native_policy(self):
            return json.loads(json.dumps(policy))
        def status(self):
            return {"available": True, "running": not self.stopped}
        def observations(self):
            return []
        async def stop(self):
            self.stopped = True
            return True

    class Runtime:
        rust_http_base_url = "http://offline-test-transport"
        async def identity_bootstrap_status(self):
            assert policies == [policy]
            return SimpleNamespace(complete=True, ready=True, counts=SimpleNamespace(broken=0))
        async def shutdown(self):
            pass

    class Builder:
        def __getattr__(self, name):
            def configure(*args, **kwargs):
                if name in {"roster", "session_service", "topology_provider"}:
                    setattr(self, name + "_value", args[0])
                return self
            return configure
        async def build(self):
            roster = await self.roster_value.roster(None)
            edges = await self.topology_provider_value.compute_edges([member.identity for member in roster], None)
            policies.append({"roster": [member.to_dict() for member in roster], "edges": [edge.to_dict() for edge in edges]})
            for member in roster:
                draft = Draft(member.profile)
                await self.session_service_value.build_agent(draft)
                drafts.append((member.identity, draft))
            return Runtime()

    class Client:
        def __init__(self, **kwargs):
            assert kwargs["trust_env"] is False
        async def __aenter__(self):
            return self
        async def __aexit__(self, *args):
            pass

    async def capture(trace, client, base):
        traces.append(trace)
        trace.ready.set()
        await asyncio.Event().wait()

    async def request(client, method, url, **values):
        traces[0].record({"type": "console_frame", "frame": {"id": "offline-terminal",
            "cursor": "console:1", "identity": spec.topology.entrypoint, "timestamp_ms": 1,
            "kind": "interaction_complete", "interaction_id": "offline-admission",
            "payload": {}, "frame_version": 1}})
        return SimpleNamespace(json=lambda: {"interaction_id": "offline-admission", "cursor": "console:1"})

    async def prepared(root, build):
        pass

    monkeypatch.setattr("launchkit.application_trial.SandboxApplicationTrial", OfflineApplication)
    monkeypatch.setattr(meerkat_meta.Meta, "builder", lambda: Builder())
    monkeypatch.setattr(worker, "QUIET_SECONDS", 0)
    monkeypatch.setattr(worker.NativeTrace, "capture", capture)
    monkeypatch.setattr(worker, "_native_request", request)
    monkeypatch.setattr(httpx, "AsyncClient", Client)

    await worker.execute(tmp_path, asyncio.Event())
    assert read_receipt(tmp_path)["status"] == "completed"
    assert policies == [policy]
    assert read_receipt(tmp_path)["edges"] == policy["edges"]
    assert [row["name"] for row in read_receipt(tmp_path)["identities"]] == [row["display_name"] for row in policy["roster"]]
    assert applications[0].stopped is True
    for identity, draft in drafts:
        visible = await draft.callbacks["rehearsal_capabilities"]({})
        expected = ["read_owner_record"] if identity == owner else []
        assert visible["application_callbacks"] == expected
        assert visible["fixture_files"] == visible["fixture_workspace"]["files"] == []
        assert visible["fixture_workspace"]["granted"] is workspace_granted
        assert ("read_owner_record" in draft.callbacks) is (identity == owner)
        assert "OWNER_INPUT_SENTINEL" not in json.dumps(visible)
        assert "private-owner-record" not in json.dumps(visible)
        instructions = " ".join(draft.additional_instructions)
        assert "An empty workspace does not establish missing application inputs" in instructions
        assert "OWNER_INPUT_SENTINEL" not in instructions
        if identity == owner:
            assert await draft.callbacks["read_owner_record"]({}) == {"value": "OWNER_INPUT_SENTINEL"}
