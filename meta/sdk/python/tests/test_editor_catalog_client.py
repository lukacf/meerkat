"""Tests for low-level Meta editor catalog typed-client wrappers."""
from __future__ import annotations

import pytest

from meerkat_meta._client import MetaAsyncTypedClient, MetaTypedClient


def _response(request, result):
    return {"jsonrpc": "2.0", "id": request["id"], "result": result}


def test_sync_typed_client_editor_catalog_methods_call_exact_rpc_names():
    calls = []
    responses = {
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
    }

    def transport(request):
        calls.append(request["method"])
        return _response(request, responses[request["method"]])

    client = MetaTypedClient.from_persistent(transport)
    assert client.tools_catalog()["tool_catalog"] == [{"id": "shell"}]
    assert client.skills_catalog()["skill_realms"] == [{"id": "meta/authoring"}]
    assert client.agent_definitions()["agent_definitions"] == [{"id": "authoring:reviewer"}]
    assert client.mobpack_templates()["sample_mobpacks"] == [{"id": "sample"}]
    assert client.mobpack_catalogs()["sources"] == {"tools": "meta/tools/catalog"}
    assert calls == [
        "meta/tools/catalog",
        "meta/skills/catalog",
        "meta/agent_definitions/list",
        "meta/mobpacks/templates",
        "meta/mobpacks/catalogs",
    ]


@pytest.mark.asyncio
async def test_async_typed_client_editor_catalog_methods_call_exact_rpc_names():
    calls = []

    async def transport(request):
        calls.append(request["method"])
        return _response(
            request,
            {
                "schema_version": "mobpack.editor.v1",
                "runtime_backed": False,
                "source": "meta/tool-config",
                "authoring_provider": {"id": "standalone_authoring"},
                "tool_catalog": [],
                "skill_realms": [],
                "agent_definitions": [],
                "blank_mobpack": {},
                "sample_mobpacks": [],
                "sample_agent_definitions": [],
                "templates": {},
                "sources": {},
                "models": [],
                "provider_defaults": [],
            },
        )

    client = MetaAsyncTypedClient(transport)
    await client.tools_catalog()
    await client.skills_catalog()
    await client.agent_definitions()
    await client.mobpack_templates()
    await client.mobpack_catalogs()
    assert calls == [
        "meta/tools/catalog",
        "meta/skills/catalog",
        "meta/agent_definitions/list",
        "meta/mobpacks/templates",
        "meta/mobpacks/catalogs",
    ]


def _validation_payload():
    return {
        "ok": True,
        "diagnostics": [],
        "display_rows": [],
        "flow_ids": ["flow_a"],
        "validation_source": "meta/mobpacks/validate",
        "deploy_command": "rkat mob deploy",
    }


def _draft_row_payload(revision=1, can_undo=False, can_redo=False):
    return {
        "id": "f_demo",
        "name": "Demo",
        "stage": "draft",
        "revision": revision,
        "etag": f"f_demo:{revision}",
        "document": {"mob_id": "demo"},
        "validation": {"ok": True},
        "can_undo": can_undo,
        "can_redo": can_redo,
    }


_MOBPACK_AUTHORING_RESPONSES = {
    "meta/mobpacks/validate": _validation_payload(),
    "meta/mobpacks/source": {
        "filename": "demo.mobpack",
        "media_type": "application/vnd.meerkat.mobpack",
        "mob_toml": "[mob]\n",
        "source_files": [],
        "validation": _validation_payload(),
        "source": "meta/mobpacks/source",
    },
    "meta/mobpacks/export": {
        "filename": "demo.mobpack",
        "media_type": "application/vnd.meerkat.mobpack",
        "content_base64": "UEsDBA==",
        "mob_toml": "[mob]\n",
        "source_files": [],
        "validation": _validation_payload(),
    },
    "meta/mobpacks/import": {
        "document": {"mob_id": "demo"},
        "validation": _validation_payload(),
        "source": "meta/mobpacks/import:mob.toml",
        "source_label": "mob.toml",
        "source_media_type": "text/x-toml",
    },
    "meta/mobpacks/list": {
        "source": "meta/mobpacks/list",
        "store_path": "/tmp/drafts.json",
        "runtime_backed": False,
        "rows": [_draft_row_payload()],
    },
    "meta/mobpacks/get": {
        "source": "meta/mobpacks/get",
        "store_path": "/tmp/drafts.json",
        "runtime_backed": False,
        "row": _draft_row_payload(),
    },
    "meta/mobpacks/create": {
        "source": "meta/mobpacks/create",
        "store_path": "/tmp/drafts.json",
        "row": _draft_row_payload(),
        "rows": [_draft_row_payload()],
    },
    "meta/mobpacks/save": {
        "source": "meta/mobpacks/save",
        "store_path": "/tmp/drafts.json",
        "row": _draft_row_payload(revision=2),
        "rows": [_draft_row_payload(revision=2)],
    },
    "meta/mobpacks/undo": {
        "source": "meta/mobpacks/undo",
        "store_path": "/tmp/drafts.json",
        "stepped": True,
        "row": _draft_row_payload(revision=3, can_redo=True),
        "rows": [_draft_row_payload(revision=3, can_redo=True)],
    },
    "meta/mobpacks/redo": {
        "source": "meta/mobpacks/redo",
        "store_path": "/tmp/drafts.json",
        "stepped": False,
        "reason": "nothing to redo",
        "row": _draft_row_payload(revision=3, can_redo=True),
        "rows": [_draft_row_payload(revision=3, can_redo=True)],
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
        "document": {"mob_id": "demo"},
        "selection": {"kind": "agent", "id": "reviewer"},
        "validation": _validation_payload(),
    },
    "meta/mobpacks/deploy_command": {
        "command": "rkat mob deploy demo.mobpack",
        "argv": ["rkat", "mob", "deploy", "demo.mobpack"],
        "deploy_command": "rkat mob deploy",
        "filename": "demo.mobpack",
        "validation": _validation_payload(),
        "source": "meerkat_meta::mobpack::deploy_argv",
    },
    "meta/mobpacks/deploy": {
        "filename": "demo.mobpack",
        "pack_path": "/tmp/demo.mobpack",
        "pack_sha256": "deadbeef",
        "command": "rkat mob deploy /tmp/demo.mobpack",
        "argv": ["rkat", "mob", "deploy", "/tmp/demo.mobpack"],
        "plan_trace": [],
        "executed": False,
        "success": False,
        "validation": _validation_payload(),
        "display_rows": [],
    },
}

_EXPECTED_MOBPACK_AUTHORING_CALLS = [
    ("meta/mobpacks/validate", {"document": {"mob_id": "demo"}, "rkat_validate": True}),
    ("meta/mobpacks/source", {"document": {"mob_id": "demo"}}),
    ("meta/mobpacks/export", {"document": {"mob_id": "demo"}}),
    ("meta/mobpacks/import", {"mob_toml": "[mob]\n", "source_name": "mob.toml"}),
    ("meta/mobpacks/list", {}),
    ("meta/mobpacks/get", {"id": "f_demo"}),
    ("meta/mobpacks/create", {"template": "blank", "name": "Demo"}),
    (
        "meta/mobpacks/save",
        {
            "id": "f_demo",
            "document": {"mob_id": "demo"},
            "stage": "draft",
            "expected_revision": 1,
            "expected_etag": "f_demo:1",
        },
    ),
    (
        "meta/mobpacks/undo",
        {"id": "f_demo", "expected_revision": 2, "expected_etag": "f_demo:2"},
    ),
    ("meta/mobpacks/redo", {"id": "f_demo"}),
    ("meta/mobpacks/delete", {"id": "f_demo", "expected_revision": 2}),
    (
        "meta/mobpacks/apply_operation",
        {
            "document": {"mob_id": "demo"},
            "operation": {"type": "add_member"},
            "expected_catalog_snapshot_id": "snap-1",
        },
    ),
    ("meta/mobpacks/deploy_command", {"document": {"mob_id": "demo"}}),
    ("meta/mobpacks/deploy", {"document": {"mob_id": "demo"}, "execute": False}),
]


def test_sync_typed_client_mobpack_authoring_methods_call_exact_rpc_names():
    calls = []

    def transport(request):
        calls.append((request["method"], request["params"]))
        return _response(request, _MOBPACK_AUTHORING_RESPONSES[request["method"]])

    document = {"mob_id": "demo"}
    client = MetaTypedClient.from_persistent(transport)
    assert client.mobpack_validate(document, rkat_validate=True)["ok"] is True
    assert client.mobpack_source(document)["mob_toml"] == "[mob]\n"
    assert client.mobpack_export(document)["content_base64"] == "UEsDBA=="
    assert (
        client.mobpack_import(mob_toml="[mob]\n", source_name="mob.toml")["document"]
        == {"mob_id": "demo"}
    )
    assert client.mobpack_list()["rows"][0]["id"] == "f_demo"
    assert client.mobpack_get("f_demo")["row"]["etag"] == "f_demo:1"
    assert client.mobpack_create(template="blank", name="Demo")["row"]["id"] == "f_demo"
    assert (
        client.mobpack_save(
            "f_demo",
            document,
            stage="draft",
            expected_revision=1,
            expected_etag="f_demo:1",
        )["row"]["revision"]
        == 2
    )
    undone = client.mobpack_undo(
        "f_demo", expected_revision=2, expected_etag="f_demo:2"
    )
    assert undone["stepped"] is True
    assert undone["row"]["can_redo"] is True
    redone = client.mobpack_redo("f_demo")
    assert redone["stepped"] is False
    assert redone["reason"] == "nothing to redo"
    assert client.mobpack_delete("f_demo", expected_revision=2)["deleted"] is True
    assert (
        client.mobpack_apply_operation(
            document,
            {"type": "add_member"},
            expected_catalog_snapshot_id="snap-1",
        )["selection"]
        == {"kind": "agent", "id": "reviewer"}
    )
    assert client.mobpack_deploy_command(document)["deploy_command"] == "rkat mob deploy"
    assert client.mobpack_deploy(document, execute=False)["executed"] is False
    assert calls == _EXPECTED_MOBPACK_AUTHORING_CALLS


@pytest.mark.asyncio
async def test_async_typed_client_mobpack_authoring_methods_call_exact_rpc_names():
    calls = []

    async def transport(request):
        calls.append((request["method"], request["params"]))
        return _response(request, _MOBPACK_AUTHORING_RESPONSES[request["method"]])

    document = {"mob_id": "demo"}
    client = MetaAsyncTypedClient(transport)
    await client.mobpack_validate(document, rkat_validate=True)
    await client.mobpack_source(document)
    await client.mobpack_export(document)
    await client.mobpack_import(mob_toml="[mob]\n", source_name="mob.toml")
    await client.mobpack_list()
    await client.mobpack_get("f_demo")
    await client.mobpack_create(template="blank", name="Demo")
    await client.mobpack_save(
        "f_demo",
        document,
        stage="draft",
        expected_revision=1,
        expected_etag="f_demo:1",
    )
    await client.mobpack_undo(
        "f_demo", expected_revision=2, expected_etag="f_demo:2"
    )
    await client.mobpack_redo("f_demo")
    await client.mobpack_delete("f_demo", expected_revision=2)
    await client.mobpack_apply_operation(
        document,
        {"type": "add_member"},
        expected_catalog_snapshot_id="snap-1",
    )
    await client.mobpack_deploy_command(document)
    await client.mobpack_deploy(document, execute=False)
    assert calls == _EXPECTED_MOBPACK_AUTHORING_CALLS


def test_sync_typed_client_mobpack_validate_rejects_invalid_payload():
    def transport(request):
        return _response(request, {"ok": "yes"})

    client = MetaTypedClient.from_persistent(transport)
    with pytest.raises(ValueError, match="invalid result payload"):
        client.mobpack_validate({"mob_id": "demo"})
