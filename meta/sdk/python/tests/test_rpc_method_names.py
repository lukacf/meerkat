"""Regression tests: verify Python SDK methods call the correct RPC names.

These tests mock the transport layer and verify that each SDK method sends
the expected RPC method name. This catches mismatches between the Python SDK
and the Rust RPC dispatch table.
"""

import asyncio
import json
from unittest.mock import AsyncMock, MagicMock

import pytest
from meerkat_meta.errors import NotConnectedError, RpcError


def make_mock_mob_handle(rpc_responses=None):
    """Create a MobHandle with a mocked RPC transport.

    ``handle._runtime.transport_timeouts`` records each call's per-call
    transport wait (``None`` is the transport's default).
    """
    from meerkat_meta.runtime import MobHandle

    runtime = MagicMock()
    calls = []
    runtime.transport_timeouts = []

    async def mock_rpc(method, params=None, *, transport_timeout=None):
        calls.append((method, params))
        runtime.transport_timeouts.append(transport_timeout)
        if rpc_responses and method in rpc_responses:
            return rpc_responses[method]
        return {}

    runtime._rpc = mock_rpc
    handle = MobHandle.__new__(MobHandle)
    handle._runtime = runtime
    return handle, calls


def make_http_mob_handle():
    from meerkat_meta.runtime import MobHandle

    runtime = MagicMock()
    runtime.rust_http_base_url = "http://127.0.0.1:8765"
    handle = MobHandle.__new__(MobHandle)
    handle._runtime = runtime
    return handle


class FakeHttpResponse:
    def __init__(self, payload):
        self.payload = payload

    def __enter__(self):
        return self

    def __exit__(self, exc_type, exc, tb):
        return False

    def read(self):
        return json.dumps(self.payload).encode("utf-8")


@pytest.mark.asyncio
async def test_attach_session_rpc_name():
    """P1 regression: attach_session must call meta/attach_existing_session."""
    handle, calls = make_mock_mob_handle({
        "meta/attach_existing_session": {
            "status": "active",
            "tokens_used": 0,
            "is_final": False,
        }
    })
    await handle.attach_session("worker", "w1", "sid_abc123")
    assert calls[0][0] == "meta/attach_existing_session"


@pytest.mark.asyncio
@pytest.mark.parametrize(
    ("decision", "outcome", "next_pending_id"),
    [
        ("approve", "allowed", None),
        ("reject", "safe_draft", None),
        ("escalate", "pending_approval", "gate-pending-000002"),
    ],
)
async def test_gating_decide_preserves_owner_result_without_followup(
    decision, outcome, next_pending_id
):
    handle, calls = make_mock_mob_handle({
        "meta/gating/decide": {
            "pending_id": "gate-pending-000001",
            "action_id": "gate-action-000001",
            "approver_id": "human-reviewer",
            "decision": decision,
            "outcome": outcome,
            "decided_at_ms": 1720000000123,
            "reason": "Recorded human decision",
            "next_pending_id": next_pending_id,
        }
    })

    result = await handle.gating_decide(
        "gate-pending-000001",
        decision,
        "human-reviewer",
        reason="Recorded human decision",
    )

    # Even an escalation successor is an observation, not an automatic request.
    assert calls == [
        (
            "meta/gating/decide",
            {
                "pending_id": "gate-pending-000001",
                "decision": decision,
                "approver_id": "human-reviewer",
                "reason": "Recorded human decision",
            },
        )
    ]
    assert result.pending_id == "gate-pending-000001"
    assert result.action_id == "gate-action-000001"
    assert result.approver_id == "human-reviewer"
    assert result.decision == decision
    assert result.outcome == outcome
    assert result.decided_at_ms == 1720000000123
    assert result.reason == "Recorded human decision"
    assert result.next_pending_id == next_pending_id


@pytest.mark.asyncio
async def test_agent_memory_rpc_names_and_params():
    """Agent memory helpers must hit the first-class durable memory RPCs."""
    handle, calls = make_mock_mob_handle({
        "meta/agent_memory/remember": {
            "memory_id": "mem-1",
            "title": "Release checklist",
            "body": "Checklist runs before release planning.",
            "tags": ["calendar", "team"],
            "created_at_ms": 10,
            "updated_at_ms": 20,
        },
        "meta/agent_memory/recall": {
            "records": [{
                "memory_id": "mem-1",
                "title": "Release checklist",
                "body": "Checklist runs before release planning.",
                "tags": ["calendar", "team"],
                "created_at_ms": 10,
                "updated_at_ms": 20,
            }],
        },
        "meta/agent_memory/forget": {
            "memory_id": "mem-1",
            "deleted": True,
        },
    })

    remembered = await handle.remember_agent_memory(
        "identity:luka",
        realm="example",
        title="Release checklist",
        body="Checklist runs before release planning.",
        tags=["team", "calendar"],
    )
    recalled = await handle.recall_agent_memory(
        "identity:luka",
        realm="example",
        selection="contextual",
        query_text="Where is the checklist?",
        query_terms=["checklist"],
        max_entries=4,
    )
    forgotten = await handle.forget_agent_memory(
        "identity:luka", "mem-1", realm="example"
    )

    assert calls == [
        (
            "meta/agent_memory/remember",
            {
                "identity": "identity:luka",
                "realm": "example",
                "title": "Release checklist",
                "body": "Checklist runs before release planning.",
                "tags": ["team", "calendar"],
            },
        ),
        (
            "meta/agent_memory/recall",
            {
                "identity": "identity:luka",
                "realm": "example",
                "selection": "contextual",
                "query_text": "Where is the checklist?",
                "query_terms": ["checklist"],
                "max_entries": 4,
            },
        ),
        (
            "meta/agent_memory/forget",
            {
                "identity": "identity:luka",
                "memory_id": "mem-1",
                "realm": "example",
            },
        ),
    ]
    assert remembered.memory_id == "mem-1"
    assert recalled[0].memory_id == "mem-1"
    assert forgotten.deleted is True


@pytest.mark.asyncio
async def test_agent_memory_update_and_manifest_rpc_names_and_params():
    """update/manifest must hit the v2 durable-memory RPCs with exact params."""
    handle, calls = make_mock_mob_handle({
        "meta/agent_memory/update": {
            "memory_id": "mem-2",
            "supersedes": "mem-1",
        },
        "meta/agent_memory/manifest": {
            "records": [{
                "id": "mem-2",
                "kind": "fact",
                "title": "Release checklist",
                "description": "When planning the team calendar",
                "age_days": 3,
                "rank": 1,
            }],
        },
    })

    updated = await handle.update_agent_memory(
        "identity:luka",
        "mem-1",
        realm="example",
        title="Release checklist",
        body="Checklist moved to 15:30.",
        tags=["team"],
    )
    manifest = await handle.manifest_agent_memory(
        "identity:luka",
        realm="example",
        tier="working_set",
        k=4,
    )

    assert calls == [
        (
            "meta/agent_memory/update",
            {
                "identity": "identity:luka",
                "memory_id": "mem-1",
                "title": "Release checklist",
                "body": "Checklist moved to 15:30.",
                "realm": "example",
                "tags": ["team"],
            },
        ),
        (
            "meta/agent_memory/manifest",
            {
                "identity": "identity:luka",
                "realm": "example",
                "tier": "working_set",
                "k": 4,
            },
        ),
    ]
    assert updated.memory_id == "mem-2"
    assert updated.supersedes == "mem-1"
    assert manifest[0].id == "mem-2"
    assert manifest[0].kind == "fact"
    assert manifest[0].rank == 1
    assert manifest[0].age_days == 3


@pytest.mark.asyncio
async def test_agent_memory_manifest_optional_params_omitted():
    """Optional tier/k/realm must not leak into params when not provided,
    and a rank-less manifest row parses with rank=None."""
    handle, calls = make_mock_mob_handle({
        "meta/agent_memory/manifest": {
            "records": [{
                "id": "mem-3",
                "kind": "gotcha",
                "title": "Unranked",
                "age_days": 0,
            }],
        },
    })

    manifest = await handle.manifest_agent_memory("identity:luka")

    assert calls[0][0] == "meta/agent_memory/manifest"
    assert calls[0][1] == {"identity": "identity:luka"}
    assert manifest[0].rank is None
    assert manifest[0].description == ""


@pytest.mark.asyncio
async def test_mobpack_editor_catalog_rpc_names():
    handle, calls = make_mock_mob_handle({
        "meta/tools/catalog": {
            "schema_version": "mobpack.editor.v1",
            "runtime_backed": False,
            "source": "meta/tool-config",
            "authoring_provider": {"id": "standalone_authoring"},
            "tool_catalog": [{"id": "shell"}],
        },
        "meta/skills/catalog": {
            "schema_version": "mobpack.editor.v1",
            "runtime_backed": False,
            "source": "meta/authoring-skill-realms",
            "authoring_provider": {"id": "standalone_authoring"},
            "skill_realms": [{"id": "meta/authoring"}],
        },
        "meta/agent_definitions/list": {
            "schema_version": "mobpack.editor.v1",
            "runtime_backed": False,
            "source": "meta/authoring-agent-definitions",
            "authoring_provider": {"id": "standalone_authoring"},
            "agent_definitions": [{"id": "authoring:reviewer"}],
        },
        "meta/mobpacks/templates": {
            "schema_version": "mobpack.editor.v1",
            "source": "meta/mobpack-templates",
            "authoring_provider": {"id": "standalone_authoring"},
            "blank_mobpack": {"document": {}},
            "sample_mobpacks": [{"id": "sample"}],
            "sample_agent_definitions": [{"id": "sample:reviewer"}],
            "templates": {"blank_mobpack": {"document": {}}},
        },
        "meta/mobpacks/catalogs": {
            "schema_version": "mobpack.editor.v1",
            "runtime_backed": False,
            "authoring_provider": {"id": "standalone_authoring"},
            "sources": {"tools": "meta/tools/catalog"},
            "templates": {},
            "tool_catalog": [{"id": "shell"}],
            "skill_realms": [{"id": "meta/authoring"}],
            "blank_mobpack": {"document": {}},
            "sample_mobpacks": [{"id": "sample"}],
            "agent_definitions": [{"id": "authoring:reviewer"}],
            "sample_agent_definitions": [{"id": "sample:reviewer"}],
            "models": [],
            "provider_defaults": [],
        },
    })

    assert (await handle.tools_catalog()).tool_catalog == [{"id": "shell"}]
    assert (await handle.skills_catalog()).skill_realms == [{"id": "meta/authoring"}]
    assert (await handle.agent_definitions()).agent_definitions == [{"id": "authoring:reviewer"}]
    assert (await handle.mobpack_templates()).sample_mobpacks == [{"id": "sample"}]
    assert (await handle.mobpack_catalogs()).sources == {"tools": "meta/tools/catalog"}
    assert [call[0] for call in calls] == [
        "meta/tools/catalog",
        "meta/skills/catalog",
        "meta/agent_definitions/list",
        "meta/mobpacks/templates",
        "meta/mobpacks/catalogs",
    ]


def _mobpack_validation_payload(ok=True):
    return {
        "ok": ok,
        "diagnostics": [],
        "display_rows": [{"kind": "ok", "glyph": "✓", "head": "valid", "sub": "", "meta": ""}],
        "mob_id": "demo",
        "flow_ids": ["flow_a"],
        "validation_source": "meta/mobpacks/validate",
        "deploy_command": "rkat mob deploy",
    }


def _mobpack_draft_row_payload(
    draft_id="f_demo", revision=1, can_undo=False, can_redo=False
):
    return {
        "id": draft_id,
        "name": "Demo",
        "version": "mobpack.editor.v1",
        "stage": "draft",
        "trigger": "Meta authoring draft",
        "source": "meta/mobpacks/create",
        "revision": revision,
        "etag": f"{draft_id}:{revision}",
        "updated_at_unix_ms": 1700000000000,
        "document": {"mob_id": "demo"},
        "validation": {"ok": True},
        "can_undo": can_undo,
        "can_redo": can_redo,
    }


@pytest.mark.asyncio
async def test_mobpack_authoring_rpc_names_and_params():
    """Every mobpack authoring wrapper must hit its exact RPC name with
    snake_case params and parse the typed result."""
    document = {"mob_id": "demo", "members": []}
    handle, calls = make_mock_mob_handle({
        "meta/mobpacks/validate": _mobpack_validation_payload(),
        "meta/mobpacks/source": {
            "filename": "demo.mobpack",
            "media_type": "application/vnd.meerkat.mobpack",
            "mob_toml": "[mob]\nid = \"demo\"\n",
            "source_files": [{
                "path": "mob.toml",
                "media_type": "text/x-toml",
                "size_bytes": 24,
                "content_base64": "W21vYl0=",
                "sha256": "abc",
            }],
            "validation": _mobpack_validation_payload(),
            "source": "meta/mobpacks/source",
        },
        "meta/mobpacks/export": {
            "filename": "demo.mobpack",
            "media_type": "application/vnd.meerkat.mobpack",
            "content_base64": "UEsDBA==",
            "mob_toml": "[mob]\nid = \"demo\"\n",
            "source_files": [],
            "validation": _mobpack_validation_payload(),
        },
        "meta/mobpacks/import": {
            "document": {"mob_id": "demo"},
            "validation": _mobpack_validation_payload(),
            "source": "meta/mobpacks/import:mob.toml",
            "source_label": "demo.toml",
            "source_media_type": "text/x-toml",
        },
        "meta/mobpacks/list": {
            "source": "meta/mobpacks/list",
            "store_path": "/tmp/drafts.json",
            "runtime_backed": False,
            "rows": [_mobpack_draft_row_payload()],
        },
        "meta/mobpacks/get": {
            "source": "meta/mobpacks/get",
            "store_path": "/tmp/drafts.json",
            "runtime_backed": False,
            "row": _mobpack_draft_row_payload(),
        },
        "meta/mobpacks/create": {
            "source": "meta/mobpacks/create",
            "store_path": "/tmp/drafts.json",
            "row": _mobpack_draft_row_payload(),
            "rows": [_mobpack_draft_row_payload()],
        },
        "meta/mobpacks/save": {
            "source": "meta/mobpacks/save",
            "store_path": "/tmp/drafts.json",
            "row": _mobpack_draft_row_payload(revision=2),
            "rows": [_mobpack_draft_row_payload(revision=2)],
        },
        "meta/mobpacks/undo": {
            "source": "meta/mobpacks/undo",
            "store_path": "/tmp/drafts.json",
            "stepped": True,
            "row": _mobpack_draft_row_payload(revision=3, can_undo=False, can_redo=True),
            "rows": [_mobpack_draft_row_payload(revision=3, can_undo=False, can_redo=True)],
        },
        "meta/mobpacks/redo": {
            "source": "meta/mobpacks/redo",
            "store_path": "/tmp/drafts.json",
            "stepped": True,
            "row": _mobpack_draft_row_payload(revision=4, can_undo=True, can_redo=False),
            "rows": [_mobpack_draft_row_payload(revision=4, can_undo=True, can_redo=False)],
        },
        "meta/mobpacks/delete": {
            "source": "meta/mobpacks/delete",
            "store_path": "/tmp/drafts.json",
            "id": "f_demo",
            "deleted": True,
            "rows": [],
        },
        "meta/mobpacks/apply_operation": {
            "source": "meta/mobpacks/apply_operation",
            "operation": "add_member",
            "ok": True,
            "document": {"mob_id": "demo", "members": [{"id": "reviewer"}]},
            "selection": {"kind": "agent", "id": "reviewer"},
            "validation": _mobpack_validation_payload(),
        },
        "meta/mobpacks/deploy_command": {
            "command": "rkat mob deploy demo.mobpack",
            "argv": ["rkat", "mob", "deploy", "demo.mobpack"],
            "deploy_command": "rkat mob deploy",
            "filename": "demo.mobpack",
            "validation": _mobpack_validation_payload(),
            "source": "meerkat_meta::mobpack::deploy_argv",
        },
        "meta/mobpacks/deploy": {
            "filename": "demo.mobpack",
            "pack_path": "/tmp/demo.mobpack",
            "pack_sha256": "deadbeef",
            "command": "rkat mob deploy /tmp/demo.mobpack",
            "argv": ["rkat", "mob", "deploy", "/tmp/demo.mobpack"],
            "plan_trace": [{"step": "validate"}],
            "executed": False,
            "success": False,
            "validation": _mobpack_validation_payload(),
            "display_rows": [],
        },
    })

    validation = await handle.mobpack_validate(document, rkat_validate=True)
    assert validation.ok is True
    assert validation.deploy_command == "rkat mob deploy"
    assert validation.display_rows[0].kind == "ok"
    assert validation.flow_ids == ["flow_a"]

    source = await handle.mobpack_source(document)
    assert source.mob_toml.startswith("[mob]")
    assert source.source_files[0].path == "mob.toml"
    assert source.validation.ok is True

    export = await handle.mobpack_export(document)
    assert export.content_base64 == "UEsDBA=="
    assert export.filename == "demo.mobpack"

    imported = await handle.mobpack_import(
        mob_toml="[mob]\nid = \"demo\"\n", source_name="demo.toml"
    )
    assert imported.document == {"mob_id": "demo"}
    assert imported.source == "meta/mobpacks/import:mob.toml"

    listed = await handle.mobpack_list()
    assert listed.rows[0].id == "f_demo"
    assert listed.rows[0].revision == 1
    assert listed.store_path == "/tmp/drafts.json"

    got = await handle.mobpack_get("f_demo")
    assert got.row.etag == "f_demo:1"
    assert got.row.stage == "draft"

    created = await handle.mobpack_create(template="blank", name="Demo")
    assert created.row.id == "f_demo"

    saved = await handle.mobpack_save(
        "f_demo",
        document,
        stage="draft",
        expected_revision=1,
        expected_etag="f_demo:1",
    )
    assert saved.row.revision == 2

    undone = await handle.mobpack_undo(
        "f_demo", expected_revision=2, expected_etag="f_demo:2"
    )
    assert undone.stepped is True
    assert undone.reason is None
    assert undone.row.revision == 3
    assert undone.row.can_undo is False
    assert undone.row.can_redo is True

    redone = await handle.mobpack_redo("f_demo")
    assert redone.stepped is True
    assert redone.row.revision == 4
    assert redone.row.can_undo is True
    assert redone.row.can_redo is False

    deleted = await handle.mobpack_delete("f_demo", expected_revision=2)
    assert deleted.deleted is True
    assert deleted.id == "f_demo"

    applied = await handle.mobpack_apply_operation(
        document,
        {"type": "add_member", "member": {"id": "reviewer"}},
        expected_catalog_snapshot_id="snap-1",
    )
    assert applied.ok is True
    assert applied.selection == {"kind": "agent", "id": "reviewer"}
    assert applied.validation.ok is True

    deploy_command = await handle.mobpack_deploy_command(document)
    assert deploy_command.argv == ["rkat", "mob", "deploy", "demo.mobpack"]

    deployed = await handle.mobpack_deploy(document, execute=False)
    assert deployed.executed is False
    assert deployed.pack_sha256 == "deadbeef"

    assert [call[0] for call in calls] == [
        "meta/mobpacks/validate",
        "meta/mobpacks/source",
        "meta/mobpacks/export",
        "meta/mobpacks/import",
        "meta/mobpacks/list",
        "meta/mobpacks/get",
        "meta/mobpacks/create",
        "meta/mobpacks/save",
        "meta/mobpacks/undo",
        "meta/mobpacks/redo",
        "meta/mobpacks/delete",
        "meta/mobpacks/apply_operation",
        "meta/mobpacks/deploy_command",
        "meta/mobpacks/deploy",
    ]
    assert calls[0][1] == {"document": document, "rkat_validate": True}
    assert calls[1][1] == {"document": document}
    assert calls[2][1] == {"document": document}
    assert calls[3][1] == {
        "mob_toml": "[mob]\nid = \"demo\"\n",
        "source_name": "demo.toml",
    }
    assert calls[4][1] == {}
    assert calls[5][1] == {"id": "f_demo"}
    assert calls[6][1] == {"template": "blank", "name": "Demo"}
    assert calls[7][1] == {
        "id": "f_demo",
        "document": document,
        "stage": "draft",
        "expected_revision": 1,
        "expected_etag": "f_demo:1",
    }
    assert calls[8][1] == {
        "id": "f_demo",
        "expected_revision": 2,
        "expected_etag": "f_demo:2",
    }
    assert calls[9][1] == {"id": "f_demo"}
    assert calls[10][1] == {"id": "f_demo", "expected_revision": 2}
    assert calls[11][1] == {
        "document": document,
        "operation": {"type": "add_member", "member": {"id": "reviewer"}},
        "expected_catalog_snapshot_id": "snap-1",
    }
    assert calls[12][1] == {"document": document}
    assert calls[13][1] == {"document": document, "execute": False}


@pytest.mark.asyncio
async def test_mobpack_authoring_optional_params_omitted():
    """Optional guards must not leak into params when not provided."""
    handle, calls = make_mock_mob_handle({
        "meta/mobpacks/validate": _mobpack_validation_payload(),
        "meta/mobpacks/delete": {
            "source": "meta/mobpacks/delete",
            "store_path": "/tmp/drafts.json",
            "id": "f_demo",
            "deleted": False,
            "rows": [],
        },
    })

    await handle.mobpack_validate({"mob_id": "demo"})
    await handle.mobpack_delete("f_demo")

    assert calls[0][1] == {"document": {"mob_id": "demo"}}
    assert calls[1][1] == {"id": "f_demo"}


@pytest.mark.asyncio
async def test_mobpack_undo_not_stepped_carries_reason():
    """When the history is empty, undo reports stepped=False with a reason
    and leaves the draft row untouched."""
    handle, calls = make_mock_mob_handle({
        "meta/mobpacks/undo": {
            "source": "meta/mobpacks/undo",
            "store_path": "/tmp/drafts.json",
            "stepped": False,
            "reason": "nothing to undo",
            "row": _mobpack_draft_row_payload(revision=1),
            "rows": [_mobpack_draft_row_payload(revision=1)],
        },
    })

    result = await handle.mobpack_undo("f_demo")
    assert calls[0][0] == "meta/mobpacks/undo"
    assert calls[0][1] == {"id": "f_demo"}
    assert result.stepped is False
    assert result.reason == "nothing to undo"
    assert result.row.revision == 1
    assert result.row.can_undo is False


@pytest.mark.asyncio
async def test_send_with_attachments_uses_multipart(monkeypatch):
    from meerkat_meta import runtime as runtime_module

    captured = {}

    def fake_urlopen(req, timeout=60):
        captured["url"] = req.full_url
        body = req.data.decode("utf-8", errors="replace")
        captured["body"] = body
        assert 'name="payload"' in body
        assert 'name="file:upload-1"' in body
        assert '"method": "meta/send_message"' in body
        assert '"type": "image_upload"' in body
        return FakeHttpResponse({
            "jsonrpc": "2.0",
            "id": "test",
            "result": {"accepted": True, "member_id": "m-1", "session_id": "s-1"},
        })

    monkeypatch.setattr(runtime_module.urllib_request, "urlopen", fake_urlopen)
    handle = make_http_mob_handle()
    result = await handle.send("m-1", "look", attachments=[b"png"])
    assert captured["url"] == "http://127.0.0.1:8765/console/rpc/multipart"
    assert result.accepted is True
    assert result.session_id == "s-1"


@pytest.mark.asyncio
async def test_memory_query_uses_assertion_filter_params():
    handle, calls = make_mock_mob_handle({
        "meta/memory/query": {
            "assertions": [],
            "conflicts": [],
        }
    })

    await handle.memory_query({
        "entity": "identity:ops",
        "topic": "deployment",
        "store": "knowledge_graph",
    })

    assert calls[0][0] == "meta/memory/query"
    assert calls[0][1] == {
        "entity": "identity:ops",
        "topic": "deployment",
        "store": "knowledge_graph",
    }


@pytest.mark.asyncio
async def test_memory_query_keeps_legacy_string_wire_shape():
    handle, calls = make_mock_mob_handle({
        "meta/memory/query": {
            "assertions": [],
            "conflicts": [],
        }
    })

    await handle.memory_query("legacy search text", store="knowledge_graph")

    assert calls[0][0] == "meta/memory/query"
    assert calls[0][1] == {
        "query": "legacy search text",
        "store": "knowledge_graph",
    }


@pytest.mark.asyncio
async def test_send_with_structured_content_and_attachment_forwards_mode(monkeypatch):
    from meerkat_meta import runtime as runtime_module

    def fake_urlopen(req, timeout=60):
        body = req.data.decode("utf-8", errors="replace")
        assert '"handling_mode": "steer"' in body
        assert '"type": "text"' in body
        assert '"type": "image_upload"' in body
        assert '"media_type": "image/jpeg"' in body
        return FakeHttpResponse({
            "jsonrpc": "2.0",
            "id": "test",
            "result": {"accepted": True, "member_id": "m-1", "session_id": "s-2"},
        })

    monkeypatch.setattr(runtime_module.urllib_request, "urlopen", fake_urlopen)
    handle = make_http_mob_handle()
    result = await handle.send(
        "m-1",
        content=[{"type": "text", "text": "hello"}],
        attachments=[(b"jpg", "image/jpeg", "photo.jpg")],
        handling_mode="steer",
    )
    assert result.session_id == "s-2"


@pytest.mark.asyncio
async def test_send_attachment_requires_http_base():
    from meerkat_meta.runtime import MobHandle

    runtime = MagicMock()
    runtime.rust_http_base_url = None
    handle = MobHandle.__new__(MobHandle)
    handle._runtime = runtime
    with pytest.raises(NotConnectedError):
        await handle.send("m-1", "x", attachments=[b"png"])


@pytest.mark.asyncio
async def test_upload_blob_uses_multipart(monkeypatch):
    from meerkat_meta import runtime as runtime_module

    def fake_urlopen(req, timeout=60):
        body = req.data.decode("utf-8", errors="replace")
        assert '"method": "meta/blob/upload"' in body
        assert '"media_type": "image/png"' in body
        return FakeHttpResponse({
            "jsonrpc": "2.0",
            "id": "test",
            "result": {"blob_id": "sha256:abc", "media_type": "image/png", "size": 3},
        })

    monkeypatch.setattr(runtime_module.urllib_request, "urlopen", fake_urlopen)
    handle = make_http_mob_handle()
    result = await handle.upload_blob(b"png", media_type="image/png", filename="a.png")
    assert result["blob_id"] == "sha256:abc"
    assert result["size"] == 3


@pytest.mark.asyncio
async def test_upload_blob_raises_rpc_error(monkeypatch):
    from meerkat_meta import runtime as runtime_module

    def fake_urlopen(req, timeout=60):
        return FakeHttpResponse({
            "jsonrpc": "2.0",
            "id": "test",
            "error": {"code": -32602, "message": "bad upload", "data": {"reason": "unit"}},
        })

    monkeypatch.setattr(runtime_module.urllib_request, "urlopen", fake_urlopen)
    handle = make_http_mob_handle()
    with pytest.raises(RpcError) as exc:
        await handle.upload_blob(b"png", media_type="image/png")
    assert exc.value.code == -32602
    assert exc.value.method == "meta/blob/upload"


@pytest.mark.asyncio
async def test_upload_blob_requires_http_base():
    from meerkat_meta.runtime import MobHandle

    runtime = MagicMock()
    runtime.rust_http_base_url = None
    handle = MobHandle.__new__(MobHandle)
    handle._runtime = runtime
    with pytest.raises(NotConnectedError):
        await handle.upload_blob(b"png", media_type="image/png")


@pytest.mark.asyncio
async def test_collect_completed_parses_wrapped_response():
    """P2 regression: collect_completed response is {"completed": [...]}, not bare list."""
    handle, calls = make_mock_mob_handle({
        "meta/collect_completed": {
            "completed": [
                {
                    "member_id": "w1",
                    "snapshot": {
                        "status": "completed",
                        "tokens_used": 100,
                        "is_final": True,
                    },
                }
            ]
        }
    })
    result = await handle.collect_completed()
    assert calls[0][0] == "meta/collect_completed"
    assert len(result) == 1
    assert result[0][0] == "w1"
    assert result[0][1].status == "completed"
    assert result[0][1].is_final is True


@pytest.mark.asyncio
async def test_collect_completed_empty():
    """collect_completed returns empty list when no members are terminal."""
    handle, calls = make_mock_mob_handle({
        "meta/collect_completed": {"completed": []}
    })
    result = await handle.collect_completed()
    assert result == []


@pytest.mark.asyncio
async def test_member_status_rpc_name():
    handle, calls = make_mock_mob_handle({
        "meta/member_status": {
            "status": "active",
            "tokens_used": 42,
            "is_final": False,
        }
    })
    result = await handle.member_status("w1")
    assert calls[0][0] == "meta/member_status"
    assert result.tokens_used == 42


@pytest.mark.asyncio
async def test_storage_doctor_rpc_name_and_params():
    handle, calls = make_mock_mob_handle({
        "meta/storage/doctor": {
            "state_dir": "/var/lib/meta/state",
            "diagnosis": {"findings": [], "inventory": []},
            "storage": None,
        }
    })
    result = await handle.storage_doctor(state_dir="/var/lib/meta/state")
    assert calls[0][0] == "meta/storage/doctor"
    assert calls[0][1] == {"state_dir": "/var/lib/meta/state"}
    assert result["diagnosis"] == {"findings": [], "inventory": []}

    await handle.storage_doctor(
        state_dir="/var/lib/meta/state", identity="domain:security"
    )
    assert calls[1][1] == {
        "state_dir": "/var/lib/meta/state",
        "identity": "domain:security",
    }

    # Omitted state_dir stays off the wire (the gateway answers with the
    # typed capability-unavailable error).
    await handle.storage_doctor()
    assert calls[2][1] == {}


@pytest.mark.asyncio
async def test_identity_resolved_tools_rpc_name():
    handle, calls = make_mock_mob_handle({
        "meta/identity/resolved_tools": {
            "identity": "domain:security",
            "session_id": "sid-1",
            "tools": ["shell", "send_message"],
        }
    })
    result = await handle.identity_resolved_tools("domain:security")
    assert calls[0][0] == "meta/identity/resolved_tools"
    assert calls[0][1] == {"identity": "domain:security"}
    assert result == ["shell", "send_message"]


@pytest.mark.asyncio
async def test_identity_resolved_tools_detail_rpc_name():
    handle, calls = make_mock_mob_handle({
        "meta/identity/resolved_tools": {
            "identity": "domain:security",
            "session_id": "sid-1",
            "tools": ["shell"],
        }
    })
    result = await handle.identity_resolved_tools_detail("domain:security")
    assert calls[0][0] == "meta/identity/resolved_tools"
    assert result.identity == "domain:security"
    assert result.session_id == "sid-1"
    assert result.tools == ["shell"]


@pytest.mark.asyncio
async def test_identity_routing_status_rpc_name():
    handle, calls = make_mock_mob_handle({
        "meta/identity/routing_status": {
            "identity": "domain:security",
            "session_id": "sid-1",
            "baseline_model": "gpt-5.6-sol",
            "effective_model": "gpt-5.6-sol",
            "session_provider": "openai",
        }
    })
    result = await handle.identity_routing_status("domain:security")
    assert calls[0][0] == "meta/identity/routing_status"
    assert calls[0][1] == {"identity": "domain:security"}
    assert result.identity == "domain:security"
    assert result.session_id == "sid-1"
    assert result.baseline_model == "gpt-5.6-sol"
    assert result.effective_model == "gpt-5.6-sol"
    assert result.session_provider == "openai"
    # The optional override summaries are absent on the wire, not empty dicts.
    assert result.active_turn_override is None
    assert result.pending_switch_turn is None


@pytest.mark.asyncio
async def test_identity_routing_status_absent_provider_stays_none():
    """An ABSENT session_provider must decode to None, never to a string.

    session_provider is Option + skip_serializing_if upstream, so a
    pre-hydration session simply omits the key. Coercing that to "" or "None"
    would make a caller's provider comparison silently pass against a value
    that was never read.
    """
    handle, _calls = make_mock_mob_handle({
        "meta/identity/routing_status": {
            "identity": "domain:security",
            "session_id": "sid-1",
            "baseline_model": "claude-fable-5",
            "effective_model": "claude-fable-5",
        }
    })
    result = await handle.identity_routing_status("domain:security")
    assert result.session_provider is None
    assert result.baseline_model == "claude-fable-5"


@pytest.mark.asyncio
async def test_force_cancel_member_rpc_name():
    handle, calls = make_mock_mob_handle()
    await handle.force_cancel_member("w1")
    assert calls[0][0] == "meta/force_cancel_member"


@pytest.mark.asyncio
async def test_stop_member_run_rpc_name_and_receipt():
    receipt = {"outcome": "not_current", "run_id": "r-1", "current_run_id": "r-2"}
    handle, calls = make_mock_mob_handle(
        {"meta/stop_member_run": {"member_id": "w1", "receipt": receipt}}
    )
    result = await handle.stop_member_run("w1", "r-1", reason="stop")
    assert calls[0][0] == "meta/stop_member_run"
    assert calls[0][1] == {"member_id": "w1", "run_id": "r-1", "reason": "stop"}
    assert result.outcome == "not_current"
    assert result.run_id == "r-1"
    assert result.current_run_id == "r-2"


@pytest.mark.asyncio
async def test_stop_member_run_parses_stopped_contributors():
    receipt = {
        "outcome": "stopped",
        "run_id": "r-1",
        "contributors": [
            {"input_id": "i-1", "completion": "cancelled", "terminal": "cancelled"},
            {"input_id": "i-2", "completion": "runtime_terminated", "terminal": "cancelled"},
        ],
    }
    handle, _calls = make_mock_mob_handle(
        {"meta/stop_member_run": {"member_id": "w1", "receipt": receipt}}
    )
    result = await handle.stop_member_run("w1", "r-1", reason="stop")
    assert result.outcome == "stopped"
    assert [row.input_id for row in result.contributors] == ["i-1", "i-2"]
    assert all(row.terminal == "cancelled" for row in result.contributors)


@pytest.mark.asyncio
async def test_stop_member_run_rejects_a_malformed_receipt():
    handle, _calls = make_mock_mob_handle(
        {"meta/stop_member_run": {"member_id": "w1", "receipt": {"outcome": "?"}}}
    )
    with pytest.raises(ValueError):
        await handle.stop_member_run("w1", "r-1", reason="stop")


@pytest.mark.asyncio
async def test_spawn_helper_rpc_name():
    handle, calls = make_mock_mob_handle({
        "meta/spawn_helper": {
            "output": "done",
            "tokens_used": 10,
            "session_id": "sess-1",
        }
    })
    result = await handle.spawn_helper(
        "h1",
        "do stuff",
        result_label="helper-result",
        max_text_bytes=65536,
        role="worker",
    )
    assert calls[0][0] == "meta/spawn_helper"
    assert calls[0][1]["agent_identity"] == "h1"
    assert calls[0][1]["task"] == "do stuff"
    assert calls[0][1]["result_label"] == "helper-result"
    assert calls[0][1]["max_text_bytes"] == 65536
    assert calls[0][1]["options"]["role"] == "worker"
    assert result.output == "done"
    assert result.session_id == "sess-1"


@pytest.mark.asyncio
async def test_fork_helper_rpc_name_and_params():
    handle, calls = make_mock_mob_handle({
        "meta/fork_helper": {"output": "forked result", "tokens_used": 5}
    })
    result = await handle.fork_helper(
        "lead",
        "fork-1",
        "review this",
        result_label="fork-result",
        max_text_bytes=32768,
        fork_context={"type": "last_messages", "count": 10},
        runtime_mode="turn_driven",
    )
    assert calls[0][0] == "meta/fork_helper"
    assert calls[0][1]["source_member_id"] == "lead"
    assert calls[0][1]["result_label"] == "fork-result"
    assert calls[0][1]["max_text_bytes"] == 32768
    assert calls[0][1]["fork_context"] == {"type": "last_messages", "count": 10}
    assert calls[0][1]["options"]["runtime_mode"] == "turn_driven"


@pytest.mark.asyncio
async def test_helper_transport_timeout_sets_only_the_client_wait():
    """``transport_timeout`` reaches the RPC's transport wait and nothing
    else: the request params are the same with and without it, and an omitted
    value leaves the transport's default wait."""
    handle, calls = make_mock_mob_handle()
    spawn = dict(result_label="helper-result", max_text_bytes=65536, role="worker")
    fork = dict(
        result_label="fork-result",
        max_text_bytes=32768,
        fork_context={"type": "last_messages", "count": 10},
    )

    await handle.spawn_helper("h1", "do stuff", **spawn)
    await handle.spawn_helper("h1", "do stuff", **spawn, transport_timeout=180.0)
    await handle.fork_helper("lead", "fork-1", "review this", **fork)
    await handle.fork_helper(
        "lead", "fork-1", "review this", **fork, transport_timeout=180.0
    )

    assert [method for method, _ in calls] == [
        "meta/spawn_helper",
        "meta/spawn_helper",
        "meta/fork_helper",
        "meta/fork_helper",
    ]
    assert calls[1][1] == calls[0][1]
    assert calls[3][1] == calls[2][1]
    assert handle._runtime.transport_timeouts == [None, 180.0, None, 180.0]


@pytest.mark.asyncio
async def test_cancel_flow_rpc_name():
    handle, calls = make_mock_mob_handle()
    await handle.cancel_flow("run_123")
    assert calls[0][0] == "meta/cancel_flow"


@pytest.mark.asyncio
async def test_flow_status_rpc_name():
    handle, calls = make_mock_mob_handle({
        "meta/flow_status": {
            "run_id": "r1",
            "mob_id": "m1",
            "flow_id": "f1",
            "status": "running",
            "step_ledger": [],
            "failure_ledger": [],
        }
    })
    result = await handle.flow_status("r1")
    assert calls[0][0] == "meta/flow_status"
    assert result.status == "running"


@pytest.mark.asyncio
async def test_list_flows_rpc_name():
    """list_flows must call meta/list_flows and parse the {flows: [...]} envelope."""
    handle, calls = make_mock_mob_handle({
        "meta/list_flows": {"flows": ["demo", "pipeline"]}
    })
    result = await handle.list_flows()
    assert calls[0][0] == "meta/list_flows"
    assert calls[0][1] is None
    assert result == ["demo", "pipeline"]


@pytest.mark.asyncio
async def test_run_flow_rpc_name_and_params():
    """run_flow must call meta/run_flow with flow_id+params and return run_id."""
    handle, calls = make_mock_mob_handle({
        "meta/run_flow": {"run_id": "run_abc123"}
    })
    run_id = await handle.run_flow("demo", {"choice": "a"})
    assert calls[0][0] == "meta/run_flow"
    assert calls[0][1] == {"flow_id": "demo", "params": {"choice": "a"}}
    assert run_id == "run_abc123"


@pytest.mark.asyncio
async def test_list_runs_rpc_name_no_filter():
    """list_runs must call meta/list_runs with no flow_id and parse the full ledger."""
    handle, calls = make_mock_mob_handle({
        "meta/list_runs": {
            "runs": [
                {
                    "run_id": "r-1",
                    "mob_id": "m-1",
                    "flow_id": "alpha",
                    "status": "completed",
                    "flow_state": {"phase": "done"},
                    "activation_params": {"k": "v"},
                    "created_at": "2026-04-29T12:00:00Z",
                    "completed_at": "2026-04-29T12:00:05Z",
                    "step_ledger": [
                        {
                            "step_id": "s1",
                            "agent_identity": "lead",
                            "status": "completed",
                            "output": {"text": "ok"},
                            "timestamp": "2026-04-29T12:00:01Z",
                        }
                    ],
                    "failure_ledger": [],
                    "frames": {
                        "frame-1": {"kernel_state": {"opaque": True}},
                    },
                    "loops": {
                        "loop-1": {"kernel_state": {}},
                    },
                    "loop_iteration_ledger": [
                        {
                            "loop_instance_id": "loop-1",
                            "iteration": 0,
                            "frame_id": "frame-2",
                        }
                    ],
                    "schema_version": 4,
                    "root_step_outputs": {"s1": {"text": "ok"}},
                    "loop_iteration_outputs": {"loop-1": []},
                }
            ]
        }
    })
    runs = await handle.list_runs()
    assert calls[0][0] == "meta/list_runs"
    assert calls[0][1] == {}
    assert len(runs) == 1
    run = runs[0]
    assert run.run_id == "r-1"
    assert run.flow_id == "alpha"
    assert run.status.value == "completed"
    assert run.activation_params == {"k": "v"}
    assert run.completed_at == "2026-04-29T12:00:05Z"
    assert len(run.step_ledger) == 1
    assert run.step_ledger[0].step_id == "s1"
    # frames / loops are MAPS keyed by id, not arrays.
    assert "frame-1" in run.frames
    assert "loop-1" in run.loops
    assert run.loop_iteration_ledger[0].iteration == 0
    assert run.schema_version == 4


@pytest.mark.asyncio
async def test_list_runs_rpc_name_with_flow_id_filter():
    """list_runs(flow_id=...) must forward flow_id."""
    handle, calls = make_mock_mob_handle({"meta/list_runs": {"runs": []}})
    runs = await handle.list_runs(flow_id="alpha")
    assert calls[0][0] == "meta/list_runs"
    assert calls[0][1] == {"flow_id": "alpha"}
    assert runs == []


@pytest.mark.asyncio
async def test_query_mob_events_stale_raises_typed_error():
    """A -32010 RpcError must be reified into MobEventsStaleError carrying both cursors."""
    from meerkat_meta.errors import MobEventsStaleError, RpcError

    async def stale_rpc(method, params=None):
        raise RpcError(
            code=-32010,
            message="stale mob event cursor: requested 999, latest 42",
            request_id="rid",
            method=method,
            data={"after_cursor": 999, "latest_cursor": 42},
        )

    from meerkat_meta.runtime import MobHandle

    runtime = MagicMock()
    runtime._rpc = stale_rpc
    handle = MobHandle.__new__(MobHandle)
    handle._runtime = runtime

    with pytest.raises(MobEventsStaleError) as info:
        await handle.query_mob_events({"after_seq": 999})

    assert info.value.after_cursor == 999
    assert info.value.latest_cursor == 42
    assert info.value.code == -32010


def test_console_timeline_replay_unavailable_uses_distinct_typed_error():
    """Console timeline replay gaps must not be reified as MobEventsStaleError."""
    from meerkat_meta.errors import ConsoleTimelineReplayUnavailableError
    from meerkat_meta.runtime import _rpc_error_from_payload

    err = _rpc_error_from_payload(
        {
            "code": -32013,
            "message": "query_timeline failed: replay unavailable",
            "data": {
                "error": "replay_unavailable",
                "stream": "timeline",
                "requested_cursor": "console:500",
                "latest_cursor": "console:42",
            },
        },
        request_id="rid",
        method="meta/console/query_timeline",
    )

    assert isinstance(err, ConsoleTimelineReplayUnavailableError)
    assert err.code == -32013
    assert err.method == "meta/console/query_timeline"
    assert err.data["requested_cursor"] == "console:500"
    assert err.data["latest_cursor"] == "console:42"


@pytest.mark.asyncio
async def test_wait_ready_rpc_name():
    """wait_ready must call meta/wait_ready and forward timeout in ms."""
    handle, calls = make_mock_mob_handle({
        "meta/wait_ready": {"ready": [], "timeout": False}
    })
    result = await handle.wait_ready(timeout=2.5)
    assert calls[0][0] == "meta/wait_ready"
    assert calls[0][1] == {"timeout_ms": 2500}
    assert result == {"ready": [], "timeout": False}



@pytest.mark.asyncio
async def test_peer_pubkey_rpc_name():
    """peer_pubkey must call meta/peer_pubkey and unwrap pubkey_b64."""
    handle, calls = make_mock_mob_handle({
        "meta/peer_pubkey": {"pubkey_b64": "AAAA"}
    })
    result = await handle.peer_pubkey()
    assert calls[0][0] == "meta/peer_pubkey"
    assert result == "AAAA"


@pytest.mark.asyncio
async def test_session_store_bigquery_rpc_name():
    handle, calls = make_mock_mob_handle({
        "meta/session_store/bigquery": {"rows": 1}
    })

    result = await handle.session_store_bigquery(operation="probe")

    assert calls[0][0] == "meta/session_store/bigquery"
    assert calls[0][1] == {"operation": "probe"}
    assert result == {"rows": 1}


@pytest.mark.asyncio
async def test_wire_local_forwards_optional_pubkey():
    """wire_local must forward remote_pubkey_b64 when provided."""
    handle, calls = make_mock_mob_handle()
    await handle.wire_local(
        "alice",
        "remote-name",
        "00000000-0000-4000-8000-000000000001",
        "tcp://10.0.0.2:9001",
        remote_pubkey_b64="KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio=",
    )
    assert calls[0][0] == "meta/cross_mob/wire_local"
    params = calls[0][1]
    assert params["remote_pubkey_b64"] == "KioqKioqKioqKioqKioqKioqKioqKioqKioqKioqKio="
    assert params["remote_address"] == "tcp://10.0.0.2:9001"


def _wg_item(**overrides):
    payload = {
        "id": "item-1",
        "realm_id": "realm-1",
        "namespace": "default",
        "title": "Ship it",
        "status": "open",
        "completion_policy": {"kind": "self_attest"},
        "priority": "medium",
        "labels": [],
        "machine_state": {"lifecycle_phase": "open", "revision": 1},
        "revision": 1,
        "created_at": "2026-07-01T00:00:00Z",
        "updated_at": "2026-07-01T00:00:00Z",
        "external_refs": [],
        "evidence_refs": [],
    }
    payload.update(overrides)
    return payload


def _wg_binding(**overrides):
    payload = {
        "binding_id": "binding-1",
        "work_ref": {"item_id": "item-1", "realm_id": "realm-1", "namespace": "default"},
        "target": {"kind": "session", "session_id": "sess-1"},
        "mode": "pursue",
        "status": {"state": "active"},
        "machine_state": {"lifecycle_phase": "active", "revision": 1},
        "delegated_authority": "add_evidence",
        "projection_policy": {"max_text_chars": 4096, "include_parent_context": True},
        "created_at": "2026-07-01T00:00:00Z",
        "updated_at": "2026-07-01T00:00:00Z",
    }
    payload.update(overrides)
    return payload


@pytest.mark.asyncio
async def test_workgraph_rpc_names_and_params():
    """Every WorkGraph wrapper must hit its exact RPC name with snake_case
    params and parse the typed result (docs/design/workgraph-wire-contract.md)."""
    handle, calls = make_mock_mob_handle({
        "meta/workgraph/snapshot": {
            "realm_id": "realm-1",
            "namespace": "default",
            "all_namespaces": False,
            "captured_at": "2026-07-08T00:00:00Z",
            "event_high_water_mark": 5,
            "items": [_wg_item()],
            "edges": [],
            "attention": [_wg_binding()],
            "ready_item_ids": ["item-1"],
        },
        "meta/workgraph/list": {"items": [_wg_item()]},
        "meta/workgraph/get": {"item": _wg_item()},
        "meta/workgraph/ready": {"items": [_wg_item()]},
        "meta/workgraph/events": {
            "events": [{
                "seq": 1,
                "realm_id": "realm-1",
                "namespace": "default",
                "item_id": "item-1",
                "kind": "created",
                "at": "2026-07-01T00:00:00Z",
                "payload": {},
            }]
        },
        "meta/workgraph/attention/list": {"attention": [_wg_binding()]},
        "meta/workgraph/goal/status": {"item": _wg_item(), "attention": _wg_binding()},
        "meta/workgraph/create": {"item": _wg_item()},
        "meta/workgraph/update": {"item": _wg_item(revision=2)},
        "meta/workgraph/claim": {"item": _wg_item(revision=2)},
        "meta/workgraph/release": {"item": _wg_item(revision=3)},
        "meta/workgraph/close": {"item": _wg_item(status="completed", revision=4)},
        "meta/workgraph/block": {"item": _wg_item(status="blocked", revision=2)},
        "meta/workgraph/link": {
            "edge": {
                "realm_id": "realm-1",
                "namespace": "default",
                "kind": "blocks",
                "from_id": "item-1",
                "to_id": "item-2",
                "created_at": "2026-07-01T00:00:00Z",
            }
        },
        "meta/workgraph/evidence/add": {"item": _wg_item(revision=2)},
        "meta/workgraph/policy/escalate": {"item": _wg_item(revision=2)},
        "meta/workgraph/goal/create": {"item": _wg_item(), "attention": _wg_binding()},
        "meta/workgraph/goal/confirm": {
            "item": _wg_item(status="completed"),
            "attention": _wg_binding(),
        },
        "meta/workgraph/goal/request_close": {
            "item": _wg_item(status="completed"),
            "attention": _wg_binding(),
        },
        "meta/workgraph/attention/pause": {
            "attention": _wg_binding(status={"state": "paused", "until": None})
        },
        "meta/workgraph/attention/resume": {"attention": _wg_binding()},
        "meta/workgraph/attention/reassign": {
            "previous": _wg_binding(binding_id="binding-1"),
            "attention": _wg_binding(binding_id="binding-2"),
        },
        "meta/workgraph/attention/prune": {"pruned": 3},
    })

    snapshot = await handle.workgraph_snapshot(namespace="default")
    assert snapshot.items[0].id == "item-1"
    assert snapshot.attention[0].binding_id == "binding-1"

    listed = await handle.workgraph_list(statuses=["open"])
    assert listed.items[0].id == "item-1"

    got = await handle.workgraph_get("item-1")
    assert got.id == "item-1"

    ready = await handle.workgraph_ready(labels=["backend"])
    assert ready.items[0].id == "item-1"

    events = await handle.workgraph_events(after_seq=0)
    assert events[0].seq == 1
    assert events[0].kind == "created"

    attention = await handle.workgraph_attention_list(status="active")
    assert attention[0].binding_id == "binding-1"

    goal_status = await handle.workgraph_goal_status("binding-1")
    assert goal_status.item.id == "item-1"
    assert goal_status.attention.binding_id == "binding-1"

    created = await handle.workgraph_create("Ship it", priority="high")
    assert created.id == "item-1"

    updated = await handle.workgraph_update("item-1", 1, title="Ship it faster")
    assert updated.revision == 2

    claimed = await handle.workgraph_claim(
        "item-1", 1, {"key": {"kind": "agent", "id": "worker-1"}}
    )
    assert claimed.revision == 2

    released = await handle.workgraph_release("item-1", 2)
    assert released.revision == 3

    closed = await handle.workgraph_close("item-1", 3)
    assert closed.status == "completed"

    blocked = await handle.workgraph_block("item-1", 1)
    assert blocked.status == "blocked"

    linked = await handle.workgraph_link("blocks", "item-1", "item-2")
    assert linked.kind == "blocks"
    assert linked.from_id == "item-1"

    with_evidence = await handle.workgraph_add_evidence(
        "item-1", 1, {"kind": "self_attest", "id": "e1"}
    )
    assert with_evidence.revision == 2

    escalated = await handle.workgraph_escalate_policy(
        "binding-1", "item-1", 1, {"kind": "host_confirmed"}
    )
    assert escalated.revision == 2

    goal_created = await handle.workgraph_goal_create(
        "Ship the release", {"kind": "session", "session_id": "sess-1"}
    )
    assert goal_created.item.id == "item-1"
    assert goal_created.attention.binding_id == "binding-1"

    confirmed = await handle.workgraph_goal_confirm(
        "binding-1", 1, evidence={"kind": "self_attest", "id": "e1"}
    )
    assert confirmed.item.status == "completed"

    close_requested = await handle.workgraph_goal_request_close("binding-1", 1)
    assert close_requested.item.status == "completed"

    paused = await handle.workgraph_attention_pause("binding-1", 1)
    assert paused.status["state"] == "paused"

    resumed = await handle.workgraph_attention_resume("binding-1", 2)
    assert resumed.binding_id == "binding-1"

    reassigned = await handle.workgraph_attention_reassign(
        "binding-1", 1, {"kind": "owner", "owner_key": {"kind": "agent", "id": "worker-2"}}
    )
    assert reassigned.previous.binding_id == "binding-1"
    assert reassigned.attention.binding_id == "binding-2"

    pruned = await handle.workgraph_attention_prune(
        updated_before="2026-07-01T00:00:00Z"
    )
    assert pruned == 3

    assert [c[0] for c in calls] == [
        "meta/workgraph/snapshot",
        "meta/workgraph/list",
        "meta/workgraph/get",
        "meta/workgraph/ready",
        "meta/workgraph/events",
        "meta/workgraph/attention/list",
        "meta/workgraph/goal/status",
        "meta/workgraph/create",
        "meta/workgraph/update",
        "meta/workgraph/claim",
        "meta/workgraph/release",
        "meta/workgraph/close",
        "meta/workgraph/block",
        "meta/workgraph/link",
        "meta/workgraph/evidence/add",
        "meta/workgraph/policy/escalate",
        "meta/workgraph/goal/create",
        "meta/workgraph/goal/confirm",
        "meta/workgraph/goal/request_close",
        "meta/workgraph/attention/pause",
        "meta/workgraph/attention/resume",
        "meta/workgraph/attention/reassign",
        "meta/workgraph/attention/prune",
    ]
    assert calls[0][1] == {"namespace": "default"}
    assert calls[1][1] == {"statuses": ["open"]}
    assert calls[2][1] == {"id": "item-1"}
    assert calls[3][1] == {"labels": ["backend"]}
    assert calls[4][1] == {"after_seq": 0}
    assert calls[5][1] == {"status": "active"}
    assert calls[6][1] == {"binding_id": "binding-1"}
    assert calls[7][1] == {"title": "Ship it", "priority": "high"}
    assert calls[8][1] == {"id": "item-1", "expected_revision": 1, "title": "Ship it faster"}
    assert calls[9][1] == {
        "id": "item-1",
        "expected_revision": 1,
        "owner": {"key": {"kind": "agent", "id": "worker-1"}},
    }
    assert calls[10][1] == {"id": "item-1", "expected_revision": 2}
    assert calls[11][1] == {"id": "item-1", "expected_revision": 3}
    assert calls[12][1] == {"id": "item-1", "expected_revision": 1}
    assert calls[13][1] == {"kind": "blocks", "from_id": "item-1", "to_id": "item-2"}
    assert calls[14][1] == {
        "id": "item-1",
        "expected_revision": 1,
        "evidence": {"kind": "self_attest", "id": "e1"},
    }
    assert calls[15][1] == {
        "binding_id": "binding-1",
        "id": "item-1",
        "expected_revision": 1,
        "completion_policy": {"kind": "host_confirmed"},
    }
    assert calls[16][1] == {
        "title": "Ship the release",
        "target": {"kind": "session", "session_id": "sess-1"},
    }
    assert calls[17][1] == {
        "binding_id": "binding-1",
        "expected_revision": 1,
        "evidence": {"kind": "self_attest", "id": "e1"},
    }
    assert calls[18][1] == {"binding_id": "binding-1", "expected_revision": 1}
    assert calls[19][1] == {"binding_id": "binding-1", "expected_revision": 1}
    assert calls[20][1] == {"binding_id": "binding-1", "expected_revision": 2}
    assert calls[21][1] == {
        "binding_id": "binding-1",
        "expected_revision": 1,
        "target": {"kind": "owner", "owner_key": {"kind": "agent", "id": "worker-2"}},
    }
    assert calls[22][1] == {"updated_before": "2026-07-01T00:00:00Z"}


@pytest.mark.asyncio
async def test_workgraph_read_methods_omit_filter_when_no_kwargs():
    """Filter-style reads must send `{}` (server default filter), matching
    the mobpack_list/list_runs precedent, not None."""
    handle, calls = make_mock_mob_handle({
        "meta/workgraph/snapshot": {
            "realm_id": "realm-1",
            "captured_at": "2026-07-08T00:00:00Z",
            "all_namespaces": False,
            "items": [],
            "edges": [],
            "attention": [],
            "ready_item_ids": [],
        },
        "meta/workgraph/list": {"items": []},
        "meta/workgraph/ready": {"items": []},
        "meta/workgraph/events": {"events": []},
        "meta/workgraph/attention/list": {"attention": []},
    })

    await handle.workgraph_snapshot()
    await handle.workgraph_list()
    await handle.workgraph_ready()
    await handle.workgraph_events()
    await handle.workgraph_attention_list()

    assert [c[1] for c in calls] == [{}, {}, {}, {}, {}]


@pytest.mark.asyncio
async def test_workgraph_unavailable_error_reifies_from_payload():
    """A -32041 RpcError must reify into WorkGraphUnavailableError."""
    from meerkat_meta.errors import WorkGraphUnavailableError
    from meerkat_meta.runtime import _rpc_error_from_payload

    err = _rpc_error_from_payload(
        {
            "code": -32041,
            "message": "workgraph service not configured",
            "data": {"kind": "workgraph_unavailable"},
        },
        request_id="rid",
        method="meta/workgraph/snapshot",
    )

    assert isinstance(err, WorkGraphUnavailableError)
    assert err.code == -32041
    assert err.method == "meta/workgraph/snapshot"
    assert err.data == {"kind": "workgraph_unavailable"}


@pytest.mark.asyncio
async def test_workgraph_conflict_error_reifies_from_payload_and_carries_detail():
    """A -32042 RpcError must reify into WorkGraphConflictError carrying detail."""
    from meerkat_meta.errors import WorkGraphConflictError
    from meerkat_meta.runtime import _rpc_error_from_payload

    err = _rpc_error_from_payload(
        {
            "code": -32042,
            "message": "revision conflict",
            "data": {"kind": "workgraph_conflict", "detail": "expected 3, found 4"},
        },
        request_id="rid",
        method="meta/workgraph/update",
    )

    assert isinstance(err, WorkGraphConflictError)
    assert err.code == -32042
    assert err.method == "meta/workgraph/update"
    assert err.detail == "expected 3, found 4"


@pytest.mark.asyncio
async def test_workgraph_conflict_error_reifies_via_from_rpc_error():
    """WorkGraphConflictError.from_rpc_error must lift detail off a generic RpcError."""
    from meerkat_meta.errors import RpcError, WorkGraphConflictError

    generic = RpcError(
        code=-32042,
        message="revision conflict",
        request_id="rid",
        method="meta/workgraph/claim",
        data={"detail": "expected 1, found 2"},
    )
    typed = WorkGraphConflictError.from_rpc_error(generic)
    assert typed.detail == "expected 1, found 2"
    assert typed.code == -32042
    assert typed.method == "meta/workgraph/claim"


@pytest.mark.asyncio
async def test_wire_local_omits_pubkey_when_absent():
    """Backward compat: inproc-only callers must not see remote_pubkey_b64
    leak into the wire-local params dict."""
    handle, calls = make_mock_mob_handle()
    await handle.wire_local(
        "alice",
        "remote-name",
        "00000000-0000-4000-8000-000000000001",
        "inproc://remote-name",
    )
    assert calls[0][0] == "meta/cross_mob/wire_local"
    assert "remote_pubkey_b64" not in calls[0][1]


@pytest.mark.asyncio
async def test_live_method_names_and_identity_param():
    handle, calls = make_mock_mob_handle({
        "meta/live/open": {
            "channel_id": "ch-1",
            "transport": {"type": "websocket", "url": "ws://x/live/ws", "token": "t"},
        },
        "meta/live/status": {"open": True, "channel_id": "ch-1"},
        "meta/live/close": {"closed": True},
        "meta/live/refresh": {"refreshed": True},
    })

    opened = await handle.live_open(
        "robot",
        model="gpt-realtime-2",
        instructions=[
            "Use the configured voice.",
            "Keep replies concise.",
        ],
    )
    assert opened["channel_id"] == "ch-1"
    assert opened["transport"]["type"] == "websocket"

    status = await handle.live_status("robot")
    assert status["open"] is True

    closed = await handle.live_close("live-channel-1")
    assert closed["closed"] is True
    assert calls[-1] == ("meta/live/close", {"channel_id": "live-channel-1"})

    refreshed = await handle.live_refresh("robot")
    assert refreshed["refreshed"] is True

    assert [c[0] for c in calls] == [
        "meta/live/open",
        "meta/live/status",
        "meta/live/close",
        "meta/live/refresh",
    ]
    assert calls[0][1] == {
        "identity": "robot",
        "model": "gpt-realtime-2",
        "instructions": [
            "Use the configured voice.",
            "Keep replies concise.",
        ],
    }
    assert calls[1][1] == {"identity": "robot"}


@pytest.mark.asyncio
async def test_live_open_typed_serializes_v1_and_returns_handle():
    from meerkat_meta.live import LiveExecutionIdentityV1

    handle, calls = make_mock_mob_handle({
        "meta/capabilities": {
            "contract_version": "0.5.0",
            "methods": ["meta/live/open"],
            "loaded_modules": [],
            "feature_capabilities": [
                "live.execution_identity.v1",
                "live.execution.function_bridge.v1",
            ],
        },
        "meta/live/open": {
            "channel_id": "ch-typed",
            "target_identity": "identity:robot",
            "execution_mode": "function_bridge",
            "pending_receipt": "pending-receipt",
            "transport": {"transport": "webrtc", "token": "t", "answer_method": "live/webrtc/answer"},
            "capabilities": {
                "audio_in": True,
                "audio_out": True,
                "text_in": True,
                "text_out": True,
                "image_in": False,
                "video_in": False,
                "transcript_supported": True,
                "barge_in_supported": True,
                "provider_native_resume": False,
            },
            "continuity": {"mode": "transcript_only"},
        }
    })

    opened = await handle.live_open_typed(
        "identity:robot",
        LiveExecutionIdentityV1(
            profile_id="example.device.open-room.v1",
        ),
    )

    assert opened.channel_id == "ch-typed"
    assert opened.target_identity == "identity:robot"
    assert opened.execution_mode == "function_bridge"
    assert calls[0][0] == "meta/capabilities"
    assert calls[1] == (
        "meta/live/open",
        {
            "identity": "identity:robot",
            "execution_identity": {
                "version": "v1",
                "profile_id": "example.device.open-room.v1",
            },
        },
    )


@pytest.mark.asyncio
async def test_live_open_typed_refuses_execution_identity_before_old_gateway_open():
    from meerkat_meta.errors import CapabilityUnavailableError
    from meerkat_meta.live import LiveExecutionIdentityV1

    handle, calls = make_mock_mob_handle({
        "meta/capabilities": {
            "contract_version": "0.5.0",
            "methods": ["meta/live/open"],
            "loaded_modules": [],
        }
    })

    with pytest.raises(CapabilityUnavailableError):
        await handle.live_open_typed(
            "identity:robot",
            LiveExecutionIdentityV1(
                profile_id="example.device.open-room.v1",
            ),
        )

    assert calls == [("meta/capabilities", None)]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "field,value",
    [
        ("execution_mode", "responses"),
        ("profile_id", "gpt-live-function-bridge-v1"),
        ("responses_model", "gpt-5.5"),
        ("responses_tools", []),
        ("responses_instructions", "delegate"),
        ("auth_binding", {"realm": "example", "binding": "other"}),
        ("self_hosted_server_id", "server"),
        ("provider_params", {}),
        ("tools", []),
        ("instructions", "delegate"),
    ],
)
async def test_strict_live_open_rejects_catalog_and_responses_bridge_overrides(
    field, value
):
    from meerkat_meta.live import LiveExecutionIdentityV1

    handle, calls = make_mock_mob_handle({})
    with pytest.raises(ValueError, match="experimental live/open does not accept"):
        await handle.live_open_typed(
            "identity:robot",
            LiveExecutionIdentityV1(
                profile_id="example.device.open-room.v1",
            ),
            **{field: value},
        )
    assert calls == []


@pytest.mark.asyncio
async def test_strict_live_open_refuses_missing_server_target_identity():
    from meerkat_meta.live import LiveExecutionIdentityV1

    handle, _calls = make_mock_mob_handle({
        "meta/capabilities": {
            "contract_version": "0.5.0",
            "methods": ["meta/live/open"],
            "loaded_modules": [],
            "feature_capabilities": ["live.execution_identity.v1"],
        },
        "meta/live/open": {
            "channel_id": "ch-typed",
            "transport": {"transport": "webrtc", "token": "t", "answer_method": "live/webrtc/answer"},
            "capabilities": {
                "audio_in": True, "audio_out": True, "text_in": True, "text_out": True,
                "image_in": False, "video_in": False, "transcript_supported": True,
                "barge_in_supported": True, "provider_native_resume": False,
            },
            "continuity": {"mode": "transcript_only"},
        },
    })
    with pytest.raises(ValueError, match="unknown field|must be a non-empty string"):
        await handle.live_open_typed(
            "caller-alias",
            LiveExecutionIdentityV1(
                profile_id="example.device.open-room.v1",
            ),
        )


@pytest.mark.asyncio
async def test_live_webrtc_answer_uses_bootstrap_method_shape():
    handle, calls = make_mock_mob_handle({
        "live/webrtc/answer": {"answer_sdp": "v=0\\r\\nanswer"},
    })
    answer = await handle.live_webrtc_answer("chan-1", "token-1", "v=0\\r\\noffer")
    assert answer == "v=0\\r\\nanswer"
    assert calls == [(
        "live/webrtc/answer",
        {"channel_id": "chan-1", "token": "token-1", "offer_sdp": "v=0\\r\\noffer"},
    )]


@pytest.mark.asyncio
async def test_live_connect_orders_owner_readiness_answer_and_activation():
    from meerkat_meta.live import LiveExecutionIdentityV1

    events: list[str] = []

    class Owner:
        async def prepare(self, pending):
            events.append(f"prepare:{pending.pending_receipt}")
            return "v=0\r\noffer"

        async def accept_answer(self, answer_sdp):
            events.append(f"answer:{answer_sdp}")

        async def activate(self, active):
            events.append(f"activate:{active.activation_receipt}")

        async def abort(self):
            events.append("abort")

    handle, calls = make_mock_mob_handle({
        "meta/capabilities": {
            "contract_version": "0.5.0",
            "methods": [],
            "loaded_modules": [],
            "feature_capabilities": [
                "live.execution_identity.v1",
                "live.execution.function_bridge.v1",
            ],
        },
        "meta/live/open": {
            "channel_id": "chan-1",
            "target_identity": "identity:robot",
            "execution_mode": "function_bridge",
            "pending_receipt": "pending-receipt",
            "transport": {
                "transport": "webrtc",
                "token": "token-1",
                "answer_method": "live/webrtc/answer",
            },
            "capabilities": {
                "audio_in": True,
                "audio_out": True,
                "text_in": False,
                "text_out": True,
                "image_in": False,
                "video_in": False,
                "transcript_supported": True,
                "barge_in_supported": True,
                "provider_native_resume": False,
            },
            "continuity": {"mode": "transcript_only"},
        },
        "meta/live/playback_owner/register": {
            "channel_id": "chan-1",
            "readiness_receipt": "ready-receipt",
        },
        "live/webrtc/answer": {"answer_sdp": "v=0\r\nanswer"},
        "meta/live/status": {
            "phase": "active",
            "handle": {
                "channel_id": "chan-1",
                "target_identity": "identity:robot",
                "execution_mode": "function_bridge",
                "activation_receipt": "active-receipt",
            },
        },
        "meta/live/playback_owner/revoke": {"phase": "revoked"},
    })

    active = await handle.live_connect(
        "identity:robot",
        LiveExecutionIdentityV1(
            profile_id="example.device.open-room.v1",
        ),
        Owner(),
        activation_poll_interval=0,
    )
    assert active.activation_receipt == "active-receipt"
    assert active.pending_receipt == "pending-receipt"
    assert active.readiness_receipt == "ready-receipt"
    assert events == [
        "prepare:pending-receipt",
        "answer:v=0\r\nanswer",
        "activate:active-receipt",
    ]
    assert [method for method, _ in calls] == [
        "meta/capabilities",
        "meta/live/open",
        "meta/live/playback_owner/register",
        "live/webrtc/answer",
        "meta/live/status",
    ]
    assert calls[2][1] == {
        "identity": "identity:robot",
        "channel_id": "chan-1",
        "pending_receipt": "pending-receipt",
    }
    assert calls[3][1]["readiness_receipt"] == "ready-receipt"
    assert calls[4][1] == {
        "identity": "identity:robot",
        "channel_id": "chan-1",
        "pending_receipt": "pending-receipt",
    }
    revoked = await active.owner_lost()
    assert revoked.phase == "revoked"
    assert events[-1] == "abort"
    assert calls[-1] == (
        "meta/live/playback_owner/revoke",
        {
            "identity": "identity:robot",
            "channel_id": "chan-1",
            "pending_receipt": "pending-receipt",
            "readiness_receipt": "ready-receipt",
            "activation_receipt": "active-receipt",
        },
    )


@pytest.mark.asyncio
async def test_live_connect_aborts_and_closes_pending_channel_when_owner_is_revoked():
    from meerkat_meta.live import LiveExecutionIdentityV1

    events: list[str] = []

    class Owner:
        async def prepare(self, _pending):
            events.append("prepare")
            return "v=0\r\noffer"

        async def accept_answer(self, _answer_sdp):
            events.append("answer")

        async def activate(self, _active):
            events.append("activate")

        async def abort(self):
            events.append("abort")

    handle, calls = make_mock_mob_handle({
        "meta/capabilities": {
            "contract_version": "0.5.0",
            "methods": [],
            "loaded_modules": [],
            "feature_capabilities": [
                "live.execution_identity.v1",
                "live.execution.function_bridge.v1",
            ],
        },
        "meta/live/open": {
            "channel_id": "chan-1",
            "target_identity": "identity:robot",
            "execution_mode": "function_bridge",
            "pending_receipt": "pending-receipt",
            "transport": {
                "transport": "webrtc",
                "token": "token-1",
                "answer_method": "live/webrtc/answer",
            },
            "capabilities": {
                "audio_in": True,
                "audio_out": True,
                "text_in": False,
                "text_out": True,
                "image_in": False,
                "video_in": False,
                "transcript_supported": True,
                "barge_in_supported": True,
                "provider_native_resume": False,
            },
            "continuity": {"mode": "transcript_only"},
        },
        "meta/live/playback_owner/register": {
            "channel_id": "chan-1",
            "readiness_receipt": "ready-receipt",
        },
        "live/webrtc/answer": {"answer_sdp": "v=0\r\nanswer"},
        "meta/live/status": {"phase": "revoked"},
        "meta/live/close": {"closed": True},
    })

    with pytest.raises(RuntimeError, match="revoked before activation"):
        await handle.live_connect(
            "identity:robot",
            LiveExecutionIdentityV1(
                profile_id="example.device.open-room.v1",
            ),
            Owner(),
            activation_poll_interval=0,
        )

    assert events == ["prepare", "answer", "abort"]
    assert [method for method, _ in calls] == [
        "meta/capabilities",
        "meta/live/open",
        "meta/live/playback_owner/register",
        "live/webrtc/answer",
        "meta/live/status",
        "meta/live/close",
    ]
    assert calls[-1][1] == {
        "identity": "identity:robot",
        "channel_id": "chan-1",
        "pending_receipt": "pending-receipt",
    }


@pytest.mark.asyncio
async def test_pending_handle_cannot_invoke_active_provider_operations():
    from meerkat_meta import PendingLiveChannelHandle

    pending = PendingLiveChannelHandle.from_dict({
        "channel_id": "chan-1",
        "target_identity": "identity:robot",
        "execution_mode": "function_bridge",
        "pending_receipt": "pending-receipt",
        "transport": {
            "transport": "webrtc",
            "token": "token-1",
            "answer_method": "live/webrtc/answer",
        },
        "capabilities": {
            "audio_in": True,
            "audio_out": True,
            "text_in": False,
            "text_out": True,
            "image_in": False,
            "video_in": False,
            "transcript_supported": True,
            "barge_in_supported": True,
            "provider_native_resume": False,
        },
        "continuity": {"mode": "transcript_only"},
    })
    handle, calls = make_mock_mob_handle({})
    with pytest.raises(TypeError, match="requires an active live channel handle"):
        await handle.live_interrupt_active(pending)
    assert calls == []


@pytest.mark.asyncio
async def test_live_replacement_required_uses_active_channel_authority():
    from meerkat_meta import ActiveLiveChannelHandle

    handle, calls = make_mock_mob_handle({
        "meta/live/replacement_required": {"required": False},
    })
    active = ActiveLiveChannelHandle(
        "chan-1", "identity:robot", "function_bridge", "active-receipt"
    )
    result = await handle.live_replacement_required(active)
    assert result.required is False
    assert calls == [(
        "meta/live/replacement_required",
        {
            "identity": "identity:robot",
            "channel_id": "chan-1",
            "activation_receipt": "active-receipt",
        },
    )]


@pytest.mark.asyncio
async def test_live_playback_complete_has_no_caller_interaction_identity():
    from meerkat_meta import ActiveLiveChannelHandle, LiveAssistantOutputAddress

    handle, calls = make_mock_mob_handle({
        "meta/live/playback_complete": {"status": "completed"},
    })
    active = ActiveLiveChannelHandle(
        "chan-1", "identity:robot", "function_bridge", "active-receipt"
    )
    output = LiveAssistantOutputAddress("chan-1", "opaque-output-1", 0)
    result = await handle.live_playback_complete(active, output)
    assert result.status == "completed"
    assert calls == [(
        "meta/live/playback_complete",
        {
            "identity": "identity:robot",
            "channel_id": "chan-1",
            "activation_receipt": "active-receipt",
            "output_id": "opaque-output-1",
        },
    )]


@pytest.mark.asyncio
async def test_live_truncate_wire_shape():
    from meerkat_meta import ActiveLiveChannelHandle, LiveAssistantOutputAddress

    handle, calls = make_mock_mob_handle({
        "meta/live/truncate": {"status": "truncated"},
    })
    active = ActiveLiveChannelHandle(
        "chan-1", "identity:robot", "function_bridge", "active-receipt"
    )
    output = LiveAssistantOutputAddress("chan-1", "opaque-output-1", 0)
    result = await handle.live_truncate(active, output, 1200)
    assert result["status"] == "truncated"
    assert calls[0][0] == "meta/live/truncate"
    assert calls[0][1] == {
        "identity": "identity:robot",
        "channel_id": "chan-1",
        "activation_receipt": "active-receipt",
        "output_id": "opaque-output-1",
        "audio_played_ms": 1200,
    }


@pytest.mark.asyncio
async def test_live_output_callback_streams_to_terminal_and_closes_on_teardown():
    from meerkat_meta import ActiveLiveChannelHandle
    from meerkat_meta.agent_builder import CallbackDispatcher

    handle, calls = make_mock_mob_handle({
        "meta/live/playback_complete": {"status": "completed"},
        "meta/live/close": {"status": "closed"},
    })
    dispatcher = CallbackDispatcher()
    handle._runtime._dispatcher = dispatcher
    active = ActiveLiveChannelHandle(
        "chan-1", "identity:robot", "function_bridge", "active-receipt"
    )
    stream = handle.live_outputs(active, capacity=1)
    next_output = asyncio.create_task(anext(stream))
    await asyncio.sleep(0)
    assert await dispatcher.handle_callback(
        "meta/live/assistant_output_available",
        {"channel_id": "chan-1", "output_id": "opaque-output-1", "content_index": 0},
    ) == {"accepted": True}
    output = await next_output
    assert output.output_id == "opaque-output-1"
    assert (await handle.live_playback_complete(active, output)).status == "completed"
    await stream.aclose()
    assert calls == [
        (
            "meta/live/playback_complete",
            {
                "identity": "identity:robot",
                "channel_id": "chan-1",
                "activation_receipt": "active-receipt",
                "output_id": "opaque-output-1",
            },
        ),
        (
            "meta/live/close",
            {
                "identity": "identity:robot",
                "channel_id": "chan-1",
                "activation_receipt": "active-receipt",
            },
        ),
    ]


@pytest.mark.asyncio
async def test_live_output_queue_overflow_and_missing_consumer_fail_loudly():
    from meerkat_meta.agent_builder import CallbackDispatcher

    dispatcher = CallbackDispatcher()
    queue = dispatcher.register_live_output_queue("chan-1", 1)
    await dispatcher.handle_callback(
        "meta/live/assistant_output_available",
        {"channel_id": "chan-1", "output_id": "opaque-output-1", "content_index": 0},
    )
    with pytest.raises(RuntimeError, match="queue is full"):
        await dispatcher.handle_callback(
            "meta/live/assistant_output_available",
            {"channel_id": "chan-1", "output_id": "opaque-output-2", "content_index": 0},
        )
    dispatcher.unregister_live_output_queue("chan-1", queue)
    with pytest.raises(RuntimeError, match="no live output consumer"):
        await dispatcher.handle_callback(
            "meta/live/assistant_output_available",
            {"channel_id": "chan-1", "output_id": "opaque-output-3", "content_index": 0},
        )


@pytest.mark.asyncio
async def test_live_send_input_image_wire_shape():
    handle, calls = make_mock_mob_handle({
        "meta/live/send_input": {"accepted": True},
    })
    result = await handle.live_send_input_image(
        "robot", "frame-0001", "image/jpeg", "aGVsbG8="
    )
    assert result["accepted"] is True
    assert calls[0][0] == "meta/live/send_input"
    assert calls[0][1] == {
        "identity": "robot",
        "chunk": {
            "kind": "image",
            "idempotency_key": "frame-0001",
            "mime": "image/jpeg",
            "data": "aGVsbG8=",
        },
    }


_SCHEDULE_LIST_TOOL = {
    "name": "meerkat_schedule_list",
    "description": "List all schedules in the active realm.",
    "inputSchema": {"type": "object", "properties": {}, "additionalProperties": False},
}


@pytest.mark.asyncio
async def test_schedule_tools_rpc_name_and_typed_result():
    handle, calls = make_mock_mob_handle({
        "meta/schedule/tools": {"tools": [_SCHEDULE_LIST_TOOL]},
    })

    result = await handle.schedule_tools()

    assert calls == [("meta/schedule/tools", None)]
    assert [tool.name for tool in result.tools] == ["meerkat_schedule_list"]
    assert result.tools[0].description == _SCHEDULE_LIST_TOOL["description"]
    assert result.tools[0].input_schema == _SCHEDULE_LIST_TOOL["inputSchema"]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "raw",
    [
        {},
        {"tools": {}},
        {"tools": ["meerkat_schedule_list"]},
        {"tools": [{**_SCHEDULE_LIST_TOOL, "name": 7}]},
        {"tools": [{**_SCHEDULE_LIST_TOOL, "description": None}]},
        {"tools": [{
            key: value for key, value in _SCHEDULE_LIST_TOOL.items() if key != "inputSchema"
        }]},
        {"tools": [{**_SCHEDULE_LIST_TOOL, "inputSchema": "object"}]},
    ],
)
async def test_schedule_tools_fails_closed_on_malformed_results(raw):
    handle, _calls = make_mock_mob_handle({"meta/schedule/tools": raw})

    with pytest.raises(ValueError, match="meta/schedule/tools"):
        await handle.schedule_tools()


# ---------------------------------------------------------------------------
# meta/run_flow role_targets
# ---------------------------------------------------------------------------

from meerkat_meta.errors import (  # noqa: E402
    CapabilityUnavailableError,
    FlowRoleTargetsNotAppliedError,
    FlowRoleTargetsRefusedError,
)

_FLOW_ROLE_TARGETS_CAPABILITIES = {
    "contract_version": "0.9.5",
    "methods": ["meta/run_flow", "meta/cancel_flow"],
    "loaded_modules": [],
    "feature_capabilities": ["flow_role_targets"],
}


@pytest.mark.asyncio
async def test_run_flow_with_role_targets_checks_the_capability_then_sends_the_binding():
    handle, calls = make_mock_mob_handle({
        "meta/capabilities": _FLOW_ROLE_TARGETS_CAPABILITIES,
        "meta/run_flow": {
            "run_id": "run-a",
            "role_targets": {"reviewer": ["rev-a2", "rev-a1"]},
        },
    })

    run_id = await handle.run_flow(
        "review",
        {"issue": 7},
        role_targets={"reviewer": ["rev-a1", "rt:rev-a2:3"]},
    )

    assert run_id == "run-a"
    assert [method for method, _ in calls] == ["meta/capabilities", "meta/run_flow"]
    assert calls[1][1] == {
        "flow_id": "review",
        "params": {"issue": 7},
        "role_targets": {"reviewer": ["rev-a1", "rt:rev-a2:3"]},
    }


@pytest.mark.asyncio
async def test_run_flow_refuses_role_targets_locally_without_the_capability():
    handle, calls = make_mock_mob_handle({
        "meta/capabilities": {**_FLOW_ROLE_TARGETS_CAPABILITIES, "feature_capabilities": []},
    })

    with pytest.raises(CapabilityUnavailableError) as raised:
        await handle.run_flow("review", role_targets={"reviewer": ["rev-a1"]})

    assert raised.value.method == "meta/run_flow"
    assert raised.value.data == {"capability": "flow_role_targets"}
    assert [method for method, _ in calls] == ["meta/capabilities"], (
        "an older gateway would run the flow role-wide, so nothing is started"
    )


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "echoed",
    [
        None,
        {"reviewer": ["rev-b1"]},
        {"reviewer": ["rev-a1"], "coder": ["coder-a"]},
        {"reviewer": "rev-a1"},
        ["rev-a1"],
    ],
)
async def test_run_flow_cancels_and_raises_when_the_binding_is_not_echoed(echoed):
    response = {"run_id": "run-a"}
    if echoed is not None:
        response["role_targets"] = echoed
    handle, calls = make_mock_mob_handle({
        "meta/capabilities": _FLOW_ROLE_TARGETS_CAPABILITIES,
        "meta/run_flow": response,
        "meta/cancel_flow": {},
    })

    with pytest.raises(FlowRoleTargetsNotAppliedError) as raised:
        await handle.run_flow("review", role_targets={"reviewer": ["rev-a1"]})

    assert raised.value.run_id == "run-a"
    assert raised.value.requested == {"reviewer": ["rev-a1"]}
    assert raised.value.echoed == echoed
    assert raised.value.cancel_error is None
    assert calls[-1] == ("meta/cancel_flow", {"run_id": "run-a"})


@pytest.mark.asyncio
async def test_run_flow_reports_a_failed_cancel_of_an_unechoed_run():
    handle, calls = make_mock_mob_handle({
        "meta/capabilities": _FLOW_ROLE_TARGETS_CAPABILITIES,
        "meta/run_flow": {"run_id": "run-a"},
    })
    inner_rpc = handle._runtime._rpc

    async def rpc_with_failing_cancel(method, params=None, *, transport_timeout=None):
        if method == "meta/cancel_flow":
            calls.append((method, params))
            raise RpcError(-32000, "cancel_flow failed", method=method)
        return await inner_rpc(method, params, transport_timeout=transport_timeout)

    handle._runtime._rpc = rpc_with_failing_cancel

    with pytest.raises(FlowRoleTargetsNotAppliedError) as raised:
        await handle.run_flow("review", role_targets={"reviewer": ["rev-a1"]})

    assert isinstance(raised.value.cancel_error, RpcError)
    assert "canceling it failed" in str(raised.value)
    assert calls[-1] == ("meta/cancel_flow", {"run_id": "run-a"})


@pytest.mark.asyncio
async def test_run_flow_without_role_targets_reads_no_capabilities():
    handle, calls = make_mock_mob_handle({"meta/run_flow": {"run_id": "run-a"}})

    assert await handle.run_flow("review") == "run-a"

    assert calls == [("meta/run_flow", {"flow_id": "review", "params": None})]


@pytest.mark.asyncio
@pytest.mark.parametrize(
    "role_targets",
    [{"reviewer": "rev-a1"}, {"reviewer": ["rev-a1", 7]}, {7: ["rev-a1"]}],
)
async def test_run_flow_refuses_role_targets_that_are_not_member_id_sequences(role_targets):
    handle, calls = make_mock_mob_handle({
        "meta/capabilities": _FLOW_ROLE_TARGETS_CAPABILITIES,
    })

    with pytest.raises(TypeError):
        await handle.run_flow("review", role_targets=role_targets)

    assert calls == []


def test_a_refused_binding_is_a_typed_error():
    from meerkat_meta.runtime import _rpc_error_from_payload

    error = _rpc_error_from_payload(
        {
            "code": -32602,
            "message": "run_flow failed: role targets refused",
            "data": {
                "kind": "flow_role_targets_refused",
                "flow_id": "review",
                "refusal": {"kind": "not_a_member", "role": "reviewer", "member": "ghost"},
            },
        },
        request_id="r-1",
        method="meta/run_flow",
    )

    assert isinstance(error, FlowRoleTargetsRefusedError)
    assert error.code == -32602
    assert error.flow_id == "review"
    assert error.refusal_kind == "not_a_member"
    assert error.refusal == {"kind": "not_a_member", "role": "reviewer", "member": "ghost"}

    plain = _rpc_error_from_payload(
        {"code": -32602, "message": "Invalid params: flow_id required"},
        request_id="r-2",
        method="meta/run_flow",
    )
    assert type(plain) is RpcError


def test_run_records_carry_their_role_targets():
    from meerkat_meta.types import MobRun, MobRunSnapshot

    bound = {"run_id": "r-1", "status": "running", "role_targets": {"reviewer": ["rev-a1"]}}
    assert MobRunSnapshot.from_dict(bound).role_targets == {"reviewer": ["rev-a1"]}
    assert MobRun.from_dict(bound).role_targets == {"reviewer": ["rev-a1"]}

    role_wide = {"run_id": "r-2", "status": "running"}
    assert MobRunSnapshot.from_dict(role_wide).role_targets is None
    assert MobRun.from_dict(role_wide).role_targets is None


# ---------------------------------------------------------------------------
# meta/run_flow idempotency_key
# ---------------------------------------------------------------------------

from meerkat_meta.errors import FlowRunIdempotencyConflictError  # noqa: E402

_FLOW_RUN_KEY_CAPABILITIES = {
    **_FLOW_ROLE_TARGETS_CAPABILITIES,
    "feature_capabilities": ["flow_role_targets", "flow_run_idempotency_key"],
}


@pytest.mark.asyncio
async def test_run_flow_sends_the_idempotency_key_after_checking_its_capability():
    handle, calls = make_mock_mob_handle({
        "meta/capabilities": _FLOW_RUN_KEY_CAPABILITIES,
        "meta/run_flow": {
            "run_id": "run-a",
            "role_targets": {"reviewer": ["rev-a1"]},
        },
    })

    run_id = await handle.run_flow(
        "review",
        {"candidate": "abc123"},
        role_targets={"reviewer": ["rev-a1"]},
        idempotency_key="issue-7-review",
    )

    assert run_id == "run-a"
    assert [method for method, _ in calls] == ["meta/capabilities", "meta/run_flow"]
    assert calls[1][1] == {
        "flow_id": "review",
        "params": {"candidate": "abc123"},
        "role_targets": {"reviewer": ["rev-a1"]},
        "idempotency_key": "issue-7-review",
    }


@pytest.mark.asyncio
async def test_run_flow_sends_a_key_without_role_targets():
    handle, calls = make_mock_mob_handle({
        "meta/capabilities": _FLOW_RUN_KEY_CAPABILITIES,
        "meta/run_flow": {"run_id": "run-a"},
    })

    assert await handle.run_flow("review", idempotency_key="issue-7-review") == "run-a"
    assert calls[1] == (
        "meta/run_flow",
        {"flow_id": "review", "params": None, "idempotency_key": "issue-7-review"},
    )


@pytest.mark.asyncio
async def test_run_flow_refuses_a_key_locally_without_the_capability():
    handle, calls = make_mock_mob_handle({
        "meta/capabilities": _FLOW_ROLE_TARGETS_CAPABILITIES,
    })

    with pytest.raises(CapabilityUnavailableError) as raised:
        await handle.run_flow(
            "review",
            role_targets={"reviewer": ["rev-a1"]},
            idempotency_key="issue-7-review",
        )

    assert raised.value.method == "meta/run_flow"
    assert raised.value.data == {"capability": "flow_run_idempotency_key"}
    assert [method for method, _ in calls] == ["meta/capabilities"], (
        "an older gateway would ignore the key, so nothing is started"
    )


@pytest.mark.asyncio
async def test_run_flow_refuses_a_key_that_is_not_a_string_before_any_call():
    handle, calls = make_mock_mob_handle({
        "meta/capabilities": _FLOW_RUN_KEY_CAPABILITIES,
    })

    with pytest.raises(TypeError):
        await handle.run_flow("review", idempotency_key=7)

    assert calls == []


def test_a_key_conflict_is_a_typed_error():
    from meerkat_meta.runtime import _rpc_error_from_payload

    error = _rpc_error_from_payload(
        {
            "code": -32602,
            "message": "run_flow failed: flow run admitted under this key with other params",
            "data": {
                "kind": "flow_run_idempotency_conflict",
                "run_id": "run-a",
                "flow_id": "review",
            },
        },
        request_id="r-1",
        method="meta/run_flow",
    )

    assert isinstance(error, FlowRunIdempotencyConflictError)
    assert error.code == -32602
    assert error.run_id == "run-a"
    assert error.flow_id == "review"
