"""Tests for typed return models (from_dict)."""
import pytest
from meerkat_meta.types import (
    AgentMemoryForgetResult,
    AgentMemoryRecallResult,
    AgentMemoryRecord,
    CallToolResult,
    CapabilitiesResult,
    DeliveryHistoryResult,
    DeliveryResult,
    ErrorCategory,
    ErrorEvent,
    EventEnvelope,
    EventQuery,
    GatingAuditEntry,
    GatingDecisionResult,
    GatingEvaluateResult,
    GatingOrigin,
    GatingPendingEntry,
    KeepAliveConfig,
    MEMBER_STATE_ACTIVE,
    MEMBER_STATE_RETIRING,
    MemberSnapshot,
    MemoryIndexResult,
    MemoryQueryResult,
    MemoryStoreInfo,
    MobpackAgentDefinitionsResult,
    MobpackApplyOperationResult,
    MobpackCatalogsResult,
    MobpackDeployCommandResult,
    MobpackDeployResult,
    MobpackDraftDeleteResult,
    MobpackDraftGetResult,
    MobpackDraftHistoryResult,
    MobpackDraftListResult,
    MobpackDraftRow,
    MobpackDraftSaveResult,
    MobpackDraftSummary,
    MobpackExportResult,
    MobpackImportResult,
    MobpackSkillsCatalogResult,
    MobpackSourceResult,
    MobpackTemplatesResult,
    MobpackToolsCatalogResult,
    MobpackValidationResult,
    IdentityResolvedToolsResult,
    PeerConnectivitySnapshot,
    PersistedEvent,
    ReconcileEdgesReport,
    ReconcileResult,
    RichMemberSnapshot,
    RediscoverReport,
    RoutingResolution,
    RuntimeRouteResult,
    SendMessageResult,
    SpawnMemberResult,
    SpawnResult,
    StatusResult,
    SubscribeResult,
    UnifiedAgentEvent,
    UnifiedModuleEvent,
    WorkGraphAttentionBinding,
    WorkGraphAttentionReassignResult,
    WorkGraphEdge,
    WorkGraphEventEntry,
    WorkGraphGoalResult,
    WorkGraphItem,
    WorkGraphItemsResult,
    WorkGraphSnapshotResult,
)


class TestStatusResult:
    def test_from_dict(self):
        r = StatusResult.from_dict(
            {"contract_version": "1.0", "running": True, "loaded_modules": ["a", "b"]}
        )
        assert r.contract_version == "1.0"
        assert r.running is True
        assert r.loaded_modules == ["a", "b"]


class TestCapabilitiesResult:
    def test_from_dict(self):
        r = CapabilitiesResult.from_dict(
            {"contract_version": "2.0", "methods": ["status"], "loaded_modules": ["x"]}
        )
        assert r.contract_version == "2.0"
        assert r.methods == ["status"]
        assert r.loaded_modules == ["x"]
        assert r.runtime_capabilities is None
        assert r.workgraph is False

    def test_from_dict_with_workgraph_true(self):
        r = CapabilitiesResult.from_dict(
            {
                "contract_version": "2.0",
                "methods": ["status"],
                "loaded_modules": ["x"],
                "workgraph": True,
            }
        )
        assert r.workgraph is True

    def test_from_dict_with_runtime_capabilities(self):
        r = CapabilitiesResult.from_dict(
            {
                "contract_version": "0.5.0",
                "methods": ["status"],
                "loaded_modules": ["x"],
                "runtime_capabilities": {
                    "can_spawn_members": True,
                    "can_send_messages": True,
                    "can_wire_members": False,
                    "can_retire_members": True,
                    "available_spawn_modes": ["module", "profile"],
                },
            }
        )
        assert r.runtime_capabilities is not None
        assert r.runtime_capabilities.can_spawn_members is True
        assert r.runtime_capabilities.can_wire_members is False
        assert r.runtime_capabilities.available_spawn_modes == ["module", "profile"]
        assert r.runtime_capabilities.profile_capabilities == {}

    def test_from_dict_with_profile_capabilities(self):
        r = CapabilitiesResult.from_dict(
            {
                "contract_version": "0.5.0",
                "methods": [],
                "loaded_modules": [],
                "runtime_capabilities": {
                    "can_spawn_members": True,
                    "can_send_messages": True,
                    "can_wire_members": True,
                    "can_retire_members": True,
                    "available_spawn_modes": ["module", "profile"],
                    "profile_capabilities": {
                        "identity": {
                            "instance_count": 3,
                            "addressable": True,
                            "has_wiring": True,
                        },
                        "gate": {
                            "instance_count": 1,
                            "addressable": False,
                            "has_wiring": False,
                        },
                    },
                },
            }
        )
        rc = r.runtime_capabilities
        assert rc is not None
        assert len(rc.profile_capabilities) == 2
        assert rc.profile_capabilities["identity"].instance_count == 3
        assert rc.profile_capabilities["identity"].addressable is True
        assert rc.profile_capabilities["identity"].has_wiring is True
        assert rc.profile_capabilities["gate"].instance_count == 1
        assert rc.profile_capabilities["gate"].addressable is False


class TestMobpackEditorCatalogResults:
    def test_split_catalog_results_from_dict(self):
        tools = MobpackToolsCatalogResult.from_dict({
            "schema_version": "mobpack.editor.v1",
            "runtime_backed": False,
            "source": "meta/tool-config",
            "authoring_provider": {"id": "standalone_authoring", "runtime_binding": "unbound"},
            "runtime_unavailable_reason": "standalone",
            "tool_catalog": [{"id": "shell"}],
        })
        assert tools.schema_version == "mobpack.editor.v1"
        assert tools.runtime_backed is False
        assert tools.authoring_provider["id"] == "standalone_authoring"
        assert tools.runtime_unavailable_reason == "standalone"
        assert tools.tool_catalog == [{"id": "shell"}]

        skills = MobpackSkillsCatalogResult.from_dict({
            "schema_version": "mobpack.editor.v1",
            "runtime_backed": False,
            "source": "meta/authoring-skill-realms",
            "authoring_provider": {"id": "standalone_authoring"},
            "skill_realms": [{"id": "meta/authoring"}],
        })
        assert skills.authoring_provider["id"] == "standalone_authoring"
        assert skills.skill_realms == [{"id": "meta/authoring"}]

        agents = MobpackAgentDefinitionsResult.from_dict({
            "schema_version": "mobpack.editor.v1",
            "runtime_backed": False,
            "source": "meta/authoring-agent-definitions",
            "authoring_provider": {"id": "standalone_authoring"},
            "agent_definitions": [{"id": "authoring:reviewer"}],
        })
        assert agents.authoring_provider["id"] == "standalone_authoring"
        assert agents.agent_definitions == [{"id": "authoring:reviewer"}]

        templates = MobpackTemplatesResult.from_dict({
            "schema_version": "mobpack.editor.v1",
            "source": "meta/mobpack-templates",
            "authoring_provider": {"id": "standalone_authoring"},
            "runtime_unavailable_reason": "standalone",
            "blank_mobpack": {"document": {}},
            "sample_mobpacks": [{"id": "sample"}],
            "sample_agent_definitions": [{"id": "sample:reviewer"}],
            "templates": {"blank_mobpack": {"document": {}}},
        })
        assert templates.authoring_provider["id"] == "standalone_authoring"
        assert templates.runtime_unavailable_reason == "standalone"
        assert templates.blank_mobpack == {"document": {}}
        assert templates.sample_mobpacks == [{"id": "sample"}]
        assert templates.sample_agent_definitions == [{"id": "sample:reviewer"}]

    def test_composed_catalog_result_from_dict(self):
        catalogs = MobpackCatalogsResult.from_dict({
            "schema_version": "mobpack.editor.v1",
            "runtime_backed": False,
            "authoring_provider": {"id": "standalone_authoring", "runtime_binding": "unbound"},
            "runtime_unavailable_reason": "standalone",
            "sources": {"tools": "meta/tools/catalog"},
            "templates": {},
            "tool_catalog": [{"id": "shell"}],
            "skill_realms": [{"id": "meta/authoring"}],
            "blank_mobpack": {"document": {}},
            "sample_mobpacks": [{"id": "sample"}],
            "agent_definitions": [{"id": "authoring:reviewer"}],
            "sample_agent_definitions": [{"id": "sample:reviewer"}],
            "models": [{
                "id": "gpt-5",
                "display_name": "GPT-5",
                "provider": "openai",
                "tier": "frontier",
                "profile": {"vision": True},
            }],
            "provider_defaults": [{
                "provider": "openai",
                "default_model_id": "gpt-5",
                "models": [],
            }],
        })
        assert catalogs.authoring_provider["runtime_binding"] == "unbound"
        assert catalogs.runtime_unavailable_reason == "standalone"
        assert catalogs.sources == {"tools": "meta/tools/catalog"}
        assert catalogs.tool_catalog == [{"id": "shell"}]
        assert catalogs.models[0].display_name == "GPT-5"
        assert catalogs.provider_defaults[0].default_model_id == "gpt-5"


class TestMobpackAuthoringResults:
    def test_validation_result_from_dict(self):
        validation = MobpackValidationResult.from_dict({
            "ok": False,
            "diagnostics": [{
                "severity": "error",
                "code": "missing_member",
                "message": "no members defined",
                "path": "members",
            }],
            "display_rows": [{
                "kind": "crit",
                "glyph": "!",
                "head": "invalid mobpack",
                "sub": "no members defined",
                "meta": "members",
            }],
            "mob_id": "demo",
            "flow_ids": ["flow_a"],
            "validation_source": "meta/mobpacks/validate",
            "deploy_command": "rkat mob deploy",
        })
        assert validation.ok is False
        assert validation.diagnostics[0].severity == "error"
        assert validation.diagnostics[0].code == "missing_member"
        assert validation.diagnostics[0].path == "members"
        assert validation.display_rows[0].kind == "crit"
        assert validation.display_rows[0].glyph == "!"
        assert validation.mob_id == "demo"
        assert validation.flow_ids == ["flow_a"]
        assert validation.deploy_command == "rkat mob deploy"

    def test_validation_result_defaults(self):
        validation = MobpackValidationResult.from_dict({"ok": True})
        assert validation.ok is True
        assert validation.diagnostics == []
        assert validation.display_rows == []
        assert validation.mob_id is None
        assert validation.flow_ids == []

    def test_source_and_export_results_from_dict(self):
        source = MobpackSourceResult.from_dict({
            "filename": "demo.mobpack",
            "media_type": "application/vnd.meerkat.mobpack",
            "mob_toml": "[mob]\n",
            "source_files": [{
                "path": "mob.toml",
                "media_type": "text/x-toml",
                "size_bytes": 7,
                "content_base64": "W21vYl0K",
                "sha256": "abc",
                "text": "[mob]\n",
            }],
            "validation": {"ok": True},
            "source": "meta/mobpacks/source",
        })
        assert source.filename == "demo.mobpack"
        assert source.source_files[0].path == "mob.toml"
        assert source.source_files[0].size_bytes == 7
        assert source.source_files[0].text == "[mob]\n"
        assert source.validation.ok is True

        export = MobpackExportResult.from_dict({
            "filename": "demo.mobpack",
            "media_type": "application/vnd.meerkat.mobpack",
            "content_base64": "UEsDBA==",
            "mob_toml": "[mob]\n",
            "source_files": [],
            "validation": {"ok": True},
        })
        assert export.content_base64 == "UEsDBA=="
        assert export.validation.ok is True

    def test_import_result_from_dict(self):
        imported = MobpackImportResult.from_dict({
            "document": {"mob_id": "demo"},
            "validation": {"ok": True},
            "source": "meta/mobpacks/import:archive",
            "source_label": "demo.mobpack",
            "source_media_type": "application/vnd.meerkat.mobpack",
        })
        assert imported.document == {"mob_id": "demo"}
        assert imported.source == "meta/mobpacks/import:archive"
        assert imported.source_label == "demo.mobpack"

    def test_draft_row_and_registry_results_from_dict(self):
        row_payload = {
            "id": "f_demo",
            "name": "Demo",
            "version": "mobpack.editor.v1",
            "stage": "draft",
            "trigger": "Meta authoring draft",
            "source": "meta/mobpacks/create",
            "revision": 3,
            "etag": "f_demo:3",
            "updated_at_unix_ms": 1700000000000,
            "document": {"mob_id": "demo"},
            "validation": {"ok": True},
            "can_undo": True,
            "can_redo": False,
        }
        row = MobpackDraftRow.from_dict(row_payload)
        assert row.id == "f_demo"
        assert row.stage == "draft"
        assert row.revision == 3
        assert row.etag == "f_demo:3"
        assert row.document == {"mob_id": "demo"}
        assert row.validation == {"ok": True}
        assert row.can_undo is True
        assert row.can_redo is False

        bare_row = MobpackDraftRow.from_dict({"id": "f_old"})
        assert bare_row.can_undo is None
        assert bare_row.can_redo is None

        listed = MobpackDraftListResult.from_dict({
            "source": "meta/mobpacks/list",
            "store_path": "/tmp/drafts.json",
            "runtime_backed": True,
            "rows": [row_payload],
        })
        assert listed.store_path == "/tmp/drafts.json"
        assert listed.runtime_backed is True
        assert listed.rows[0].id == "f_demo"
        # Draft results list drafts as summaries: no document.
        assert listed.rows[0] == MobpackDraftSummary(
            id="f_demo",
            name="Demo",
            version="mobpack.editor.v1",
            stage="draft",
            trigger="Meta authoring draft",
            source="meta/mobpacks/create",
            description="",
            revision=3,
            etag="f_demo:3",
            updated_at_unix_ms=1700000000000,
            can_undo=True,
            can_redo=False,
        )

        got = MobpackDraftGetResult.from_dict({
            "source": "meta/mobpacks/get",
            "runtime_backed": True,
            "row": row_payload,
        })
        assert got.store_path is None
        assert got.row.revision == 3

        saved = MobpackDraftSaveResult.from_dict({
            "source": "meta/mobpacks/save",
            "store_path": "/tmp/drafts.json",
            "row": row_payload,
            "rows": [row_payload],
        })
        assert saved.row.id == "f_demo"
        assert len(saved.rows) == 1
        assert saved.durability_warning is None
        warned = MobpackDraftSaveResult.from_dict({
            "source": "meta/mobpacks/save",
            "row": row_payload,
            "rows": [],
            "durability_warning": "mobpack draft store was replaced, but its directory could not be synced",
        })
        assert warned.durability_warning == (
            "mobpack draft store was replaced, but its directory could not be synced"
        )

        deleted = MobpackDraftDeleteResult.from_dict({
            "source": "meta/mobpacks/delete",
            "store_path": "/tmp/drafts.json",
            "id": "f_demo",
            "deleted": True,
            "rows": [],
        })
        assert deleted.id == "f_demo"
        assert deleted.deleted is True
        assert deleted.rows == []

    def test_draft_history_result_from_dict(self):
        row_payload = {
            "id": "f_demo",
            "name": "Demo",
            "stage": "draft",
            "revision": 4,
            "etag": "f_demo:4",
            "document": {"mob_id": "demo"},
            "validation": {"ok": True},
            "can_undo": False,
            "can_redo": True,
        }
        stepped = MobpackDraftHistoryResult.from_dict({
            "source": "meta/mobpacks/undo",
            "store_path": "/tmp/drafts.json",
            "stepped": True,
            "row": row_payload,
            "rows": [row_payload],
        })
        assert stepped.source == "meta/mobpacks/undo"
        assert stepped.store_path == "/tmp/drafts.json"
        assert stepped.stepped is True
        assert stepped.reason is None
        assert stepped.row.revision == 4
        assert stepped.row.can_undo is False
        assert stepped.row.can_redo is True
        assert stepped.rows[0].id == "f_demo"

        blocked = MobpackDraftHistoryResult.from_dict({
            "source": "meta/mobpacks/redo",
            "store_path": "/tmp/drafts.json",
            "stepped": False,
            "reason": "nothing to redo",
            "row": row_payload,
            "rows": [row_payload],
        })
        assert blocked.stepped is False
        assert blocked.reason == "nothing to redo"
        assert blocked.row.etag == "f_demo:4"
        assert blocked.durability_warning is None
        warned = MobpackDraftHistoryResult.from_dict({
            "source": "meta/mobpacks/undo",
            "stepped": True,
            "row": row_payload,
            "rows": [],
            "durability_warning": "not synced",
        })
        assert warned.durability_warning == "not synced"
        assert MobpackDraftDeleteResult.from_dict({
            "id": "f_demo",
            "deleted": True,
            "durability_warning": "not synced",
        }).durability_warning == "not synced"

    def test_apply_operation_result_from_dict(self):
        applied = MobpackApplyOperationResult.from_dict({
            "source": "meta/mobpacks/apply_operation",
            "operation": "add_member",
            "ok": True,
            "document": {"mob_id": "demo", "members": [{"id": "reviewer"}]},
            "selection": {"kind": "agent", "id": "reviewer"},
            "validation": {"ok": True},
        })
        assert applied.operation == "add_member"
        assert applied.ok is True
        assert applied.selection == {"kind": "agent", "id": "reviewer"}
        assert applied.validation.ok is True

    def test_apply_operation_result_null_selection(self):
        applied = MobpackApplyOperationResult.from_dict({
            "source": "meta/mobpacks/apply_operation",
            "operation": "delete_member",
            "ok": True,
            "document": {"mob_id": "demo"},
            "selection": None,
            "validation": {"ok": True},
        })
        assert applied.selection is None

    def test_deploy_results_from_dict(self):
        preview = MobpackDeployCommandResult.from_dict({
            "command": "rkat mob deploy demo.mobpack",
            "argv": ["rkat", "mob", "deploy", "demo.mobpack"],
            "deploy_command": "rkat mob deploy",
            "filename": "demo.mobpack",
            "validation": {"ok": True},
            "source": "meerkat_meta::mobpack::deploy_argv",
        })
        assert preview.command == "rkat mob deploy demo.mobpack"
        assert preview.argv == ["rkat", "mob", "deploy", "demo.mobpack"]
        assert preview.deploy_command == "rkat mob deploy"

        deployed = MobpackDeployResult.from_dict({
            "filename": "demo.mobpack",
            "pack_path": "/tmp/demo.mobpack",
            "pack_sha256": "deadbeef",
            "command": "rkat mob deploy /tmp/demo.mobpack",
            "argv": ["rkat", "mob", "deploy", "/tmp/demo.mobpack"],
            "plan_trace": [{"step": "validate"}],
            "executed": True,
            "success": True,
            "status_code": 0,
            "stdout": "deployed",
            "validation": {"ok": True},
            "display_rows": [{
                "kind": "ok",
                "glyph": "✓",
                "head": "deploy executed",
                "sub": "deployed",
                "meta": "/tmp/demo.mobpack",
            }],
        })
        assert deployed.executed is True
        assert deployed.success is True
        assert deployed.status_code == 0
        assert deployed.stdout == "deployed"
        assert deployed.stderr is None
        assert deployed.plan_trace == [{"step": "validate"}]
        assert deployed.display_rows[0].head == "deploy executed"


class TestReconcileResult:
    def test_from_dict(self):
        r = ReconcileResult.from_dict(
            {"accepted": True, "reconciled_modules": ["m1"], "added": 1}
        )
        assert r.accepted is True
        assert r.reconciled_modules == ["m1"]
        assert r.added == 1


class TestSpawnResult:
    def test_from_dict_module_spawn(self):
        r = SpawnResult.from_dict({"accepted": True, "module_id": "mod-1"})
        assert r.accepted is True
        assert r.module_id == "mod-1"
        assert r.agent_identity is None
        assert r.role is None

    def test_from_dict_discovery_spawn(self):
        r = SpawnResult.from_dict({
            "accepted": True,
            "module_id": "mod-1",
            "agent_identity": "mk-123",
            "role": "assistant",
        })
        assert r.agent_identity == "mk-123"
        assert r.role == "assistant"

    def test_from_dict_no_module_id(self):
        """Rust discovery-path may not return module_id."""
        r = SpawnResult.from_dict({"accepted": True, "agent_identity": "mk-123"})
        assert r.accepted is True
        assert r.module_id == ""
        assert r.agent_identity == "mk-123"


class TestSpawnMemberResult:
    def test_is_spawn_result_alias(self):
        assert SpawnMemberResult is SpawnResult


class TestKeepAliveConfig:
    def test_from_dict(self):
        r = KeepAliveConfig.from_dict({"interval_ms": 15000, "event": "ping"})
        assert r.interval_ms == 15000
        assert r.event == "ping"


class TestEventEnvelope:
    def test_from_dict(self):
        r = EventEnvelope.from_dict({
            "event_id": "ev-1",
            "source": "agent-1",
            "timestamp_ms": 1234567890,
            "event": {"kind": "ready"},
        })
        assert r.event_id == "ev-1"
        assert r.source == "agent-1"
        assert r.timestamp_ms == 1234567890
        assert r.event == {"kind": "ready"}


class TestSubscribeResult:
    def test_from_dict(self):
        r = SubscribeResult.from_dict(
            {
                "scope": "mob",
                "replay_from_event_id": "ev-1",
                "keep_alive": {"interval_ms": 15000, "event": "ping"},
                "keep_alive_comment": "ping",
                "event_frames": ["frame1"],
                "events": [
                    {
                        "event_id": "ev-2",
                        "source": "agent-1",
                        "timestamp_ms": 100,
                        "event": {"kind": "init"},
                    }
                ],
            }
        )
        assert r.scope == "mob"
        assert r.replay_from_event_id == "ev-1"
        assert isinstance(r.keep_alive, KeepAliveConfig)
        assert r.keep_alive.interval_ms == 15000
        assert r.keep_alive.event == "ping"
        assert r.keep_alive_comment == "ping"
        assert r.event_frames == ["frame1"]
        assert len(r.events) == 1
        assert isinstance(r.events[0], EventEnvelope)
        assert r.events[0].event_id == "ev-2"
        assert r.events[0].event == {"kind": "init"}


class TestSendMessageResult:
    def test_from_dict(self):
        r = SendMessageResult.from_dict(
            {
                "accepted": True,
                "member_id": "lead-1",
                "session_id": "s-1",
            }
        )
        assert r.accepted is True
        assert r.member_id == "lead-1"
        assert r.session_id == "s-1"


class TestRoutingResolution:
    def test_from_dict(self):
        r = RoutingResolution.from_dict(
            {"recipient": "agent-1", "route": {"path": "/a"}}
        )
        assert r.recipient == "agent-1"
        assert r.route == {"path": "/a"}


class TestDeliveryResult:
    def test_from_dict(self):
        r = DeliveryResult.from_dict({"delivered": True, "delivery_id": "d-1"})
        assert r.delivered is True
        assert r.delivery_id == "d-1"


class TestMemoryQueryResult:
    def test_from_dict(self):
        r = MemoryQueryResult.from_dict({"results": [{"key": "val"}]})
        assert r.results == [{"key": "val"}]


class TestCallToolResult:
    def test_from_dict(self):
        r = CallToolResult.from_dict({
            "module_id": "gmail",
            "tool": "gmail_search",
            "result": {"messages": [{"id": "1", "subject": "Hello"}]},
        })
        assert r.module_id == "gmail"
        assert r.tool == "gmail_search"
        assert r.result == {"messages": [{"id": "1", "subject": "Hello"}]}


class TestToolCaller:
    @pytest.mark.asyncio
    async def test_call_unwraps_result(self):
        """ToolCaller.__call__ should unwrap CallToolResult.result."""
        from unittest.mock import AsyncMock
        from meerkat_meta.runtime import ToolCaller

        mock_handle = AsyncMock()
        mock_handle.call_tool.return_value = CallToolResult.from_dict({
            "module_id": "google-workspace",
            "tool": "gmail_search",
            "result": [{"id": "1", "subject": "Hello"}],
        })

        gmail = ToolCaller(mock_handle, "google-workspace")
        messages = await gmail("gmail_search", query="is:unread")

        assert messages == [{"id": "1", "subject": "Hello"}]
        mock_handle.call_tool.assert_called_once_with(
            "google-workspace", "gmail_search", {"query": "is:unread"}
        )

    def test_tool_caller_stores_module_id(self):
        from unittest.mock import AsyncMock
        from meerkat_meta.runtime import ToolCaller, MobHandle
        mock_handle = AsyncMock(spec=MobHandle)
        caller = ToolCaller(mock_handle, "my-module")
        assert caller._module_id == "my-module"
        assert caller._mob_handle is mock_handle

    @pytest.mark.asyncio
    async def test_call_propagates_errors(self):
        from unittest.mock import AsyncMock
        from meerkat_meta.runtime import ToolCaller

        mock_handle = AsyncMock()
        mock_handle.call_tool.side_effect = RuntimeError("module not loaded")
        gmail = ToolCaller(mock_handle, "google-workspace")
        with pytest.raises(RuntimeError, match="module not loaded"):
            await gmail("gmail_search", query="test")


class TestMemberSnapshot:
    def test_from_dict(self):
        r = MemberSnapshot.from_dict(
            {
                "agent_identity": "agent-1",
                "role": "worker",
                "state": "active",
                "wired_to": ["agent-2"],
                "labels": {"role": "lead"},
            }
        )
        assert r.agent_identity == "agent-1"
        assert r.role == "worker"
        assert r.state == "active"
        assert r.wired_to == ["agent-2"]
        assert r.labels == {"role": "lead"}


class TestRuntimeRouteResult:
    def test_from_dict(self):
        r = RuntimeRouteResult.from_dict(
            {
                "route_key": "r1",
                "recipient": "user-1",
                "channel": "slack",
                "sink": "notify",
                "target_module": "comms",
            }
        )
        assert r.route_key == "r1"
        assert r.recipient == "user-1"
        assert r.channel == "slack"
        assert r.sink == "notify"
        assert r.target_module == "comms"


class TestDeliveryHistoryResult:
    def test_from_dict(self):
        r = DeliveryHistoryResult.from_dict(
            {"deliveries": [{"delivery_id": "d1", "status": "sent"}]}
        )
        assert r.deliveries == [{"delivery_id": "d1", "status": "sent"}]


class TestGatingEvaluateResult:
    def test_from_dict(self):
        r = GatingEvaluateResult.from_dict(
            {
                "action_id": "a1",
                "action": "send_email",
                "actor_id": "bot-1",
                "risk_tier": "r1",
                "outcome": "allowed",
                "pending_id": None,
            }
        )
        assert r.action_id == "a1"
        assert r.action == "send_email"
        assert r.actor_id == "bot-1"
        assert r.risk_tier == "r1"
        assert r.outcome == "allowed"
        assert r.pending_id is None


class TestGatingDecisionResult:
    def test_from_dict(self):
        r = GatingDecisionResult.from_dict(
            {"pending_id": "p1", "action_id": "a1", "decision": "approve"}
        )
        assert r.pending_id == "p1"
        assert r.action_id == "a1"
        assert r.decision == "approve"
        assert r.approver_id is None
        assert r.outcome is None
        assert r.decided_at_ms is None
        assert r.reason is None
        assert r.next_pending_id is None

    def test_original_positional_construction_remains_valid(self):
        result = GatingDecisionResult("p1", "a1", "approve")
        assert result == GatingDecisionResult.from_dict(
            {"pending_id": "p1", "action_id": "a1", "decision": "approve"}
        )
        assert result.outcome is None

    @pytest.mark.parametrize(
        ("decision", "outcome", "next_pending_id"),
        [
            ("approve", "allowed", None),
            ("reject", "safe_draft", None),
            ("escalate", "pending_approval", "gate-pending-000002"),
        ],
    )
    def test_preserves_runtime_decision_wire_shape(
        self, decision, outcome, next_pending_id
    ):
        result = GatingDecisionResult.from_dict(
            {
                "pending_id": "gate-pending-000001",
                "action_id": "gate-action-000001",
                "approver_id": "human-reviewer",
                "decision": decision,
                "outcome": outcome,
                "decided_at_ms": 1720000000123,
                "reason": "Recorded human decision",
                "next_pending_id": next_pending_id,
            }
        )
        assert result.pending_id == "gate-pending-000001"
        assert result.action_id == "gate-action-000001"
        assert result.approver_id == "human-reviewer"
        assert result.decision == decision
        assert result.outcome == outcome
        assert result.decided_at_ms == 1720000000123
        assert result.reason == "Recorded human decision"
        assert result.next_pending_id == next_pending_id

    @pytest.mark.parametrize("outcome", [None, "future_outcome"])
    def test_unknown_outcome_is_not_inferred_from_approval(self, outcome):
        result = GatingDecisionResult.from_dict(
            {
                "pending_id": "p1",
                "action_id": "a1",
                "decision": "approve",
                "outcome": outcome,
                "reason": None,
            }
        )
        assert result.outcome == outcome
        assert result.reason is None


class TestGatingAuditEntry:
    def test_from_dict(self):
        r = GatingAuditEntry.from_dict(
            {
                "audit_id": "au1",
                "timestamp_ms": 1000,
                "event_type": "evaluate",
                "action_id": "a1",
                "actor_id": "bot-1",
                "risk_tier": "r0",
                "outcome": "allowed",
            }
        )
        assert r.audit_id == "au1"
        assert r.timestamp_ms == 1000
        assert r.event_type == "evaluate"
        assert r.action_id == "a1"
        assert r.actor_id == "bot-1"
        assert r.risk_tier == "r0"
        assert r.outcome == "allowed"
        assert r.pending_id is None
        assert r.detail == {}

    def test_preserves_pending_id_and_owner_detail(self):
        detail = {
            "approver_id": "human:reviewer",
            "decision": "approve",
            "next_pending_id": None,
            "origin": {"identity": "agent:writer"},
        }
        r = GatingAuditEntry.from_dict(
            {
                "audit_id": "au2",
                "timestamp_ms": 2000,
                "event_type": "approval_decided",
                "action_id": "a1",
                "pending_id": "p1",
                "actor_id": "agent:writer",
                "risk_tier": "r3",
                "outcome": "allowed",
                "detail": detail,
            }
        )
        assert r.pending_id == "p1"
        assert r.detail == detail
        assert r.detail is not detail

    @pytest.mark.parametrize("pending_id", [None, "", " ", 7, {}])
    @pytest.mark.parametrize("detail", [None, "text", [1], 3])
    def test_malformed_pending_id_and_detail_stay_unknown(self, pending_id, detail):
        r = GatingAuditEntry.from_dict({"pending_id": pending_id, "detail": detail})
        assert r.pending_id is None
        assert r.detail == {}

    def test_original_positional_construction_remains_valid(self):
        r = GatingAuditEntry("au1", 1000, "evaluate", "a1", "bot-1", "r0", "allowed")
        assert r.pending_id is None
        assert r.detail == {}


class TestGatingPendingEntry:
    def test_preserves_origin(self):
        r = GatingPendingEntry.from_dict({
            "pending_id": "p1", "action_id": "a1", "action": "publish",
            "actor_id": "agent:writer", "risk_tier": "r3", "created_at_ms": 5000,
            "origin": {
                "identity": "person:alice",
                "conversation_id": "conv-1",
                "interaction_id": "int-1",
            },
        })
        assert r.origin == GatingOrigin("person:alice", "conv-1", "int-1")
        partial = GatingPendingEntry.from_dict({
            "pending_id": "p1", "action_id": "a1", "action": "publish",
            "actor_id": "agent:writer", "origin": {"identity": "person:alice"},
        })
        assert partial.origin == GatingOrigin("person:alice")

    @pytest.mark.parametrize(
        "origin",
        [None, "person:alice", [], {}, {"identity": ""}, {"identity": 3},
         {"conversation_id": "conv-1"}],
    )
    def test_absent_or_malformed_origin_stays_unknown(self, origin):
        r = GatingPendingEntry.from_dict({
            "pending_id": "p1", "action_id": "a1", "action": "publish",
            "actor_id": "agent:writer", "origin": origin,
        })
        assert r.origin is None

    def test_preserves_owner_routing_and_deadline(self):
        r = GatingPendingEntry.from_dict({
            "pending_id": "p1", "action_id": "a1", "action": "publish",
            "actor_id": "agent:writer", "risk_tier": "r3", "created_at_ms": 5000,
            "requested_approver": "human:reviewer", "approval_recipient": "review-queue",
            "approval_channel": "notification", "approval_route_id": "route:1",
            "approval_delivery_id": "delivery:1", "deadline_at_ms": 65000,
            "rationale": "Requested publication",
        })
        assert r.actor_id == "agent:writer"
        assert r.requested_approver == "human:reviewer"
        assert r.approval_recipient == "review-queue"
        assert r.approval_channel == "notification"
        assert r.approval_route_id == "route:1"
        assert r.approval_delivery_id == "delivery:1"
        assert r.deadline_at_ms == 65000
        assert r.rationale == "Requested publication"

    def test_absent_or_malformed_optional_routing_stays_unknown(self):
        base = {"pending_id": "p1", "action_id": "a1", "action": "publish", "actor_id": "actor"}
        for extra in ({}, {
            "requested_approver": 7, "approval_recipient": {}, "approval_channel": " ",
            "approval_route_id": False, "approval_delivery_id": [],
            "deadline_at_ms": "60000", "rationale": False,
        }):
            r = GatingPendingEntry.from_dict({**base, **extra})
            assert r.requested_approver is None
            assert r.approval_recipient is None
            assert r.approval_channel is None
            assert r.approval_route_id is None
            assert r.approval_delivery_id is None
            assert r.deadline_at_ms is None
            assert r.rationale is None
        for value in (-1, float("nan"), float("inf"), True, 1.5):
            assert GatingPendingEntry.from_dict({**base, "deadline_at_ms": value}).deadline_at_ms is None
        # Existing callers can still construct the six-field model positionally.
        assert GatingPendingEntry("p1", "a1", "publish", "actor", None, 0).deadline_at_ms is None

    def test_from_dict(self):
        r = GatingPendingEntry.from_dict(
            {
                "pending_id": "p1",
                "action_id": "a1",
                "action": "deploy",
                "actor_id": "bot-1",
                "risk_tier": "r2",
                "created_at_ms": 5000,
            }
        )
        assert r.pending_id == "p1"
        assert r.action_id == "a1"
        assert r.action == "deploy"
        assert r.actor_id == "bot-1"
        assert r.risk_tier == "r2"
        assert r.created_at_ms == 5000


class TestMemoryStoreInfo:
    def test_from_dict(self):
        r = MemoryStoreInfo.from_dict(
            {"store": "knowledge_graph", "record_count": 42}
        )
        assert r.store == "knowledge_graph"
        assert r.record_count == 42


class TestMemoryIndexResult:
    def test_from_dict(self):
        r = MemoryIndexResult.from_dict(
            {
                "entity": "user-1",
                "topic": "prefs",
                "store": "knowledge_graph",
                "assertion_id": "mem-001",
            }
        )
        assert r.entity == "user-1"
        assert r.topic == "prefs"
        assert r.store == "knowledge_graph"
        assert r.assertion_id == "mem-001"
        assert r.conflict_active is False

    def test_from_dict_conflict_active(self):
        r = MemoryIndexResult.from_dict(
            {
                "entity": "user-1",
                "topic": "prefs",
                "store": "knowledge_graph",
                "assertion_id": None,
                "conflict_active": True,
            }
        )
        assert r.assertion_id is None
        assert r.conflict_active is True


class TestAgentMemoryRecord:
    def test_from_dict(self):
        r = AgentMemoryRecord.from_dict(
            {
                "memory_id": "mem-1",
                "title": "Release checklist",
                "body": "Checklist runs before release planning.",
                "tags": ["calendar", "team"],
                "created_at_ms": 10,
                "updated_at_ms": 20,
            }
        )
        assert r.memory_id == "mem-1"
        assert r.title == "Release checklist"
        assert r.body == "Checklist runs before release planning."
        assert r.tags == ["calendar", "team"]
        assert r.created_at_ms == 10
        assert r.updated_at_ms == 20

    def test_rejects_malformed_durable_records(self):
        with pytest.raises(ValueError, match="memory_id must be a non-empty string"):
            AgentMemoryRecord.from_dict({})
        with pytest.raises(ValueError, match="tags must be an array of strings"):
            AgentMemoryRecord.from_dict(
                {
                    "memory_id": "mem-1",
                    "title": "Title",
                    "body": "Body",
                    "tags": [42],
                    "created_at_ms": 1,
                    "updated_at_ms": 1,
                }
            )


class TestAgentMemoryRecallResult:
    def test_from_dict(self):
        r = AgentMemoryRecallResult.from_dict(
            {
                "records": [{
                    "memory_id": "mem-1",
                    "title": "Release checklist",
                    "body": "Checklist runs before release planning.",
                    "tags": ["calendar", "team"],
                    "created_at_ms": 10,
                    "updated_at_ms": 20,
                }],
            }
        )
        assert len(r.records) == 1
        assert r.records[0].memory_id == "mem-1"

    def test_rejects_malformed_envelopes(self):
        with pytest.raises(ValueError, match="records must be an array"):
            AgentMemoryRecallResult.from_dict({})
        with pytest.raises(ValueError, match="title must be a non-empty string"):
            AgentMemoryRecallResult.from_dict(
                {"records": [{"memory_id": "mem-1"}]}
            )


class TestAgentMemoryForgetResult:
    def test_from_dict(self):
        r = AgentMemoryForgetResult.from_dict(
            {"memory_id": "mem-1", "deleted": True}
        )
        assert r.memory_id == "mem-1"
        assert r.deleted is True

    def test_rejects_malformed_results(self):
        with pytest.raises(ValueError, match="memory_id must be a non-empty string"):
            AgentMemoryForgetResult.from_dict({})
        with pytest.raises(ValueError, match="deleted must be a boolean"):
            AgentMemoryForgetResult.from_dict(
                {"memory_id": "mem-1", "deleted": "yes"}
            )


class TestReconcileEdgesReport:
    def test_from_dict(self):
        r = ReconcileEdgesReport.from_dict(
            {
                "desired_edges": [],
                "wired_edges": [],
                "unwired_edges": [],
                "retained_edges": [],
                "preexisting_edges": [],
                "skipped_missing_members": [],
                "pruned_stale_managed_edges": [],
                "failures": [],
            }
        )
        assert r.desired_edges == []
        assert r.wired_edges == []
        assert r.unwired_edges == []
        assert r.retained_edges == []
        assert r.preexisting_edges == []
        assert r.skipped_missing_members == []
        assert r.pruned_stale_managed_edges == []
        assert r.failures == []
        assert r.applied is True

    def test_a_candidate_dry_run_is_flagged_not_applied(self):
        r = ReconcileEdgesReport.from_dict(
            {"wired_edges": [{"a": "x", "b": "y"}], "applied": False}
        )
        assert r.applied is False
        assert r.wired_edges == [{"a": "x", "b": "y"}]


class TestRediscoverReport:
    def test_from_dict(self):
        r = RediscoverReport.from_dict(
            {
                "spawned": ["a1"],
                "edges": {
                    "desired_edges": [],
                    "wired_edges": [],
                    "unwired_edges": [],
                    "retained_edges": [],
                    "preexisting_edges": [],
                    "skipped_missing_members": [],
                    "pruned_stale_managed_edges": [],
                    "failures": [],
                },
            }
        )
        assert r.spawned == ["a1"]
        assert isinstance(r.edges, ReconcileEdgesReport)
        assert r.edges.desired_edges == []
        assert r.edges.wired_edges == []
        assert r.edges.unwired_edges == []
        assert r.edges.retained_edges == []
        assert r.edges.preexisting_edges == []
        assert r.edges.skipped_missing_members == []
        assert r.edges.pruned_stale_managed_edges == []
        assert r.edges.failures == []


class TestPersistedEvent:
    def test_from_dict(self):
        r = PersistedEvent.from_dict(
            {
                "id": "ev1",
                "seq": 1,
                "timestamp_ms": 1000,
                "member_id": "agent-1",
                "event": {"Agent": {"agent_id": "agent-1", "event_type": "run_completed"}},
            }
        )
        assert r.id == "ev1"
        assert r.seq == 1
        assert r.timestamp_ms == 1000
        assert r.member_id == "agent-1"
        assert isinstance(r.event, UnifiedAgentEvent)
        assert r.event.agent_id == "agent-1"
        assert r.event.event_type == "run_completed"

    def test_from_dict_accepts_rust_internal_agent_event(self):
        r = PersistedEvent.from_dict(
            {
                "id": "ev-rust",
                "seq": 2,
                "timestamp_ms": 1001,
                "member_id": "agent-1",
                "event": {
                    "kind": "agent",
                    "agent_id": "agent-1",
                    "event_type": "run_completed",
                    "payload": {"ok": True},
                },
            }
        )
        assert isinstance(r.event, UnifiedAgentEvent)
        assert r.event.agent_id == "agent-1"
        assert r.event.event_type == "run_completed"
        assert r.event.payload == {"ok": True}

    def test_from_dict_keeps_the_run_failed_agent_payload(self):
        # docs/concepts/events.mdx: the agent payload is part of the wire
        # contract and must not be dropped. Pre-fix `_parse_unified_event`
        # built UnifiedAgentEvent(agent_id, event_type) only, so query_events
        # consumers never saw meerkat's `error_report`.
        r = PersistedEvent.from_dict(
            {
                "id": "ev-failed",
                "seq": 4,
                "timestamp_ms": 1003,
                "member_id": "agent-1",
                "event": {
                    "kind": "agent",
                    "agent_id": "agent-1",
                    "event_type": "run_failed",
                    "payload": {
                        "type": "run_failed",
                        "session_id": "s",
                        "error_report": {
                            "class": "llm",
                            "reason": {"reason_type": "llm_auth_error"},
                            "message": "LLM error: authentication failed (401)",
                        },
                        "error": "LLM error: authentication failed (401)",
                        "reason": "llm_auth_error",
                    },
                },
            }
        )
        assert isinstance(r.event, UnifiedAgentEvent)
        assert r.event.event_type == "run_failed"
        assert r.event.payload["error_report"]["reason"]["reason_type"] == "llm_auth_error"
        assert r.event.payload["error"] == "LLM error: authentication failed (401)"

    def test_from_dict_legacy_agent_shape_keeps_payload(self):
        r = PersistedEvent.from_dict(
            {
                "id": "ev-legacy",
                "seq": 5,
                "timestamp_ms": 1004,
                "member_id": "agent-1",
                "event": {
                    "Agent": {
                        "agent_id": "agent-1",
                        "event_type": "run_failed",
                        "payload": {"error": "boom"},
                    }
                },
            }
        )
        assert isinstance(r.event, UnifiedAgentEvent)
        assert r.event.payload == {"error": "boom"}

    def test_from_dict_accepts_rust_internal_module_event(self):
        r = PersistedEvent.from_dict(
            {
                "id": "ev-module",
                "seq": 3,
                "timestamp_ms": 1002,
                "member_id": None,
                "event": {
                    "kind": "module",
                    "module": "routing",
                    "event_type": "route_added",
                    "payload": {"route": "pager"},
                },
            }
        )
        assert isinstance(r.event, UnifiedModuleEvent)
        assert r.event.module == "routing"
        assert r.event.event_type == "route_added"
        assert r.event.payload == {"route": "pager"}


class TestUnifiedAgentEvent:
    def test_direct_construction(self):
        e = UnifiedAgentEvent(agent_id="agent-1", event_type="run_completed")
        assert e.agent_id == "agent-1"
        assert e.event_type == "run_completed"
        assert e.payload == {}

    def test_direct_construction_with_payload(self):
        e = UnifiedAgentEvent(
            agent_id="agent-1", event_type="run_failed", payload={"error": "boom"}
        )
        assert e.payload == {"error": "boom"}


class TestUnifiedModuleEvent:
    def test_direct_construction(self):
        e = UnifiedModuleEvent(module="router", event_type="route_added", payload={"key": "val"})
        assert e.module == "router"
        assert e.event_type == "route_added"
        assert e.payload == {"key": "val"}


class TestErrorEvent:
    def test_from_dict(self):
        r = ErrorEvent.from_dict(
            {"category": "spawn_failure", "member_id": "a1", "error": "profile not found"}
        )
        assert r.category == "spawn_failure"
        assert r.context["member_id"] == "a1"
        assert r.context["error"] == "profile not found"
        assert "a1" in r.message
        assert "profile not found" in r.message

    def test_identity_materialization_failure_from_dict(self):
        r = ErrorEvent.from_dict(
            {
                "category": "identity_materialization_failure",
                "identity": "initiative:broken",
                "initiator": "review:singleton",
                "operation": "materialize_reachable_peers",
                "error": "bridge create_session: missing skill",
            }
        )
        assert r.category == "identity_materialization_failure"
        assert (
            r.message
            == "initiative:broken for review:singleton: materialize_reachable_peers: bridge create_session: missing skill"
        )
        assert r.context["identity"] == "initiative:broken"

    def test_compaction_persistence_rejected_from_dict(self):
        r = ErrorEvent.from_dict(
            {
                "category": "compaction_persistence_rejected",
                "identity": "review:singleton",
                "session_id": "sess-9",
                "error": "store rejected summary",
            }
        )
        assert r.category == "compaction_persistence_rejected"
        assert r.message == "review:singleton (sess-9): store rejected summary"

    def test_actor_loop_stalled_from_dict(self):
        r = ErrorEvent.from_dict(
            {
                "category": "actor_loop_stalled",
                "probe_waited_secs": 30,
                "detail": "mob command loop busy",
            }
        )
        assert r.category == "actor_loop_stalled"
        assert r.message == "probe unanswered for 30s: mob command loop busy"

    def test_event_log_flush_failure_from_dict(self):
        r = ErrorEvent.from_dict(
            {"category": "event_log_flush_failure", "error": "disk full"}
        )
        assert r.category == "event_log_flush_failure"
        assert r.message == "disk full"

    def test_actor_loop_recovered_reads_as_a_resolution(self):
        r = ErrorEvent.from_dict(
            {
                "category": "actor_loop_recovered",
                "stall_id": 7,
                "stalled_for_secs": 42,
            }
        )
        assert r.category == "actor_loop_recovered"
        assert r.message == "stall 7 resolved after 42s"
        # Closes a prior stall; a host must be able to resolve rather than
        # open a second incident.
        assert r.is_resolution is True

    def test_mob_stop_proceeded_without_interrupt_does_not_read_as_clean(self):
        r = ErrorEvent.from_dict(
            {
                "category": "mob_stop_proceeded_without_interrupt",
                "waited_ms": 5000,
                "member": "review:singleton",
                "error": "runtime attach not ready",
            }
        )
        assert r.category == "mob_stop_proceeded_without_interrupt"
        assert r.message == (
            "teardown proceeded after 5000ms WITHOUT a successful interrupt "
            "on review:singleton; a turn may have been admitted and may "
            "still be running: runtime attach not ready"
        )
        assert r.is_resolution is False

    def test_mob_stop_proceeded_without_interrupt_without_member(self):
        r = ErrorEvent.from_dict(
            {
                "category": "mob_stop_proceeded_without_interrupt",
                "waited_ms": 250,
                "error": "runtime attach not ready",
            }
        )
        assert r.message == (
            "teardown proceeded after 250ms WITHOUT a successful interrupt; "
            "a turn may have been admitted and may still be running: "
            "runtime attach not ready"
        )

    def test_retired_member_release_failed_names_the_retired_incarnation(self):
        r = ErrorEvent.from_dict(
            {
                "category": "retired_member_release_failed",
                "identity": "keeper",
                "agent_runtime_id": "rt:keeper:2",
                "session_id": "s-1",
                "cause": "retire",
                "error": "native host composition is unavailable",
            }
        )
        assert r.category == "retired_member_release_failed"
        assert r.message == (
            "keeper (rt:keeper:2) was retired (retire), and the host's "
            "release failed: native host composition is unavailable"
        )
        assert r.context["cause"] == "retire"
        assert r.context["session_id"] == "s-1"
        assert r.is_resolution is False

    def test_only_recoveries_and_resolved_escalations_are_resolutions(self):
        resolutions = {ErrorCategory.ACTOR_LOOP_RECOVERED, ErrorCategory.MOB_ANNOUNCEMENT_ESCALATION_RESOLVED}
        for category in ErrorCategory:
            event = ErrorEvent(category=category, message="", context={})
            assert event.is_resolution is (category in resolutions), category

    def test_mob_announcement_escalated_names_event_cause_and_subject(self):
        r = ErrorEvent.from_dict(
            {
                "category": "mob_announcement_escalated",
                "escalation": {
                    "escalation_id": "esc-1",
                    "cause": "failure_streak",
                    "subject": {"kind": "source", "mob_id": "mob-a", "source_id": "mail"},
                    "since_ms": 10,
                    "raised_at_ms": 20,
                    "renotify_count": 1,
                },
                "event": "renotified",
                "renotify_count": 1,
                "cursor": 7,
            }
        )
        assert r.category == ErrorCategory.MOB_ANNOUNCEMENT_ESCALATED
        assert r.message == (
            "renotified esc-1 failure_streak on source mail of mob mob-a "
            "(notified again 1 times, cursor 7)"
        )
        assert r.is_resolution is False

    def test_mob_announcement_escalation_resolved_is_a_resolution(self):
        r = ErrorEvent.from_dict(
            {
                "category": "mob_announcement_escalation_resolved",
                "escalation_id": "esc-2",
                "cause": "audience_hold",
                "subject": {"kind": "member", "member": {"mob_id": "mob-a", "identity": "reader:1"}},
                "resolved_at_ms": 30,
                "cursor": 8,
            }
        )
        assert r.message == "esc-2 audience_hold on member reader:1 of mob mob-a (cursor 8)"
        assert r.is_resolution is True
        declared = ErrorEvent.from_dict(
            {
                "category": "mob_announcement_escalation_resolved",
                "escalation_id": "esc-3",
                "cause": "declaration_pending",
                "subject": {"kind": "declaration", "record_kind": "role_child", "id": "c-1"},
                "resolved_at_ms": 40,
                "cursor": 9,
            }
        )
        assert declared.message == "esc-3 declaration_pending on declared role_child c-1 (cursor 9)"

    # The full set is gated against the Rust `ErrorEvent` enum by
    # crates/meerkat-meta/tests/sdk_error_category_parity.rs; this asserts the exact
    # tags so a rename here cannot pass by editing both sides of that gate.
    def test_error_category_members_match_expected_values(self):
        assert {member.name: member.value for member in ErrorCategory} == {
            "SPAWN_FAILURE": "spawn_failure",
            "RECONCILE_INCOMPLETE": "reconcile_incomplete",
            "CHECKPOINT_FAILURE": "checkpoint_failure",
            "COMPACTION_PERSISTENCE_REJECTED": "compaction_persistence_rejected",
            "ACTOR_LOOP_STALLED": "actor_loop_stalled",
            "ACTOR_LOOP_RECOVERED": "actor_loop_recovered",
            "ACTOR_LOOP_TERMINATED": "actor_loop_terminated",
            "HOST_LOOP_CRASH": "host_loop_crash",
            "REDISCOVER_FAILURE": "rediscover_failure",
            "EVENT_LOG_FLUSH_FAILURE": "event_log_flush_failure",
            "IDENTITY_MATERIALIZATION_FAILURE": "identity_materialization_failure",
            "MOB_STOP_PROCEEDED_WITHOUT_INTERRUPT": "mob_stop_proceeded_without_interrupt",
            "RETIRED_MEMBER_RELEASE_FAILED": "retired_member_release_failed",
            "MOB_ANNOUNCEMENT_ESCALATED": "mob_announcement_escalated",
            "MOB_ANNOUNCEMENT_ESCALATION_RESOLVED": "mob_announcement_escalation_resolved",
        }


class TestEventQuery:
    def test_to_dict_all_fields(self):
        q = EventQuery(
            since_ms=100,
            until_ms=200,
            member_id="agent-1",
            event_types=["run_completed"],
            limit=10,
            after_seq=5,
        )
        d = q.to_dict()
        assert d["since_ms"] == 100
        assert d["until_ms"] == 200
        assert d["member_id"] == "agent-1"
        assert d["event_types"] == ["run_completed"]
        assert d["limit"] == 10
        assert d["after_seq"] == 5

    def test_to_dict_empty(self):
        q = EventQuery()
        d = q.to_dict()
        assert d == {}

    def test_to_dict_partial(self):
        q = EventQuery(since_ms=100, limit=5)
        d = q.to_dict()
        assert d == {"since_ms": 100, "limit": 5}


class TestMemberStateConstants:
    def test_member_state_active(self):
        assert MEMBER_STATE_ACTIVE == "active"

    def test_member_state_retiring(self):
        assert MEMBER_STATE_RETIRING == "retiring"


class TestCrossSdkGracefulParsing:
    """Python from_dict must degrade gracefully (like TS) on missing keys,
    not raise KeyError. Regression for the cross-SDK divergence."""

    def test_gating_audit_entry_defaults_missing_keys(self):
        entry = GatingAuditEntry.from_dict({})
        assert entry.audit_id == ""
        assert entry.timestamp_ms == 0
        assert entry.event_type == ""
        assert entry.action_id == ""
        assert entry.actor_id == ""
        assert entry.outcome == ""
        assert entry.risk_tier is None

    def test_member_snapshot_defaults_missing_keys(self):
        snapshot = MemberSnapshot.from_dict({})
        assert snapshot.agent_identity == ""
        assert snapshot.role == ""
        assert snapshot.state == ""
        assert snapshot.wired_to == []
        assert snapshot.labels == {}


class TestPeerConnectivitySnapshot:
    def test_tri_state_known_reads_counts_from_snapshot(self):
        # 0.7.x nests the counts under `snapshot` behind a `status` discriminator.
        # Regression: the old flat reader returned all-zeros for this shape.
        snap = PeerConnectivitySnapshot.from_dict({
            "status": "known",
            "snapshot": {
                "reachable_peer_count": 3,
                "unknown_peer_count": 1,
                "unreachable_peers": [{"peer": "p1", "reason": "timeout"}],
            },
        })
        assert snap.status == "known"
        assert snap.is_known
        assert snap.reachable_peer_count == 3
        assert snap.unknown_peer_count == 1
        assert len(snap.unreachable_peers) == 1
        assert snap.unreachable_peers[0].peer == "p1"

    def test_tri_state_not_applicable_and_probe_timed_out(self):
        not_applicable = PeerConnectivitySnapshot.from_dict({"status": "not_applicable"})
        assert not_applicable.status == "not_applicable"
        assert not not_applicable.is_known
        assert not_applicable.reachable_peer_count == 0

        timed_out = PeerConnectivitySnapshot.from_dict({"status": "probe_timed_out"})
        assert timed_out.status == "probe_timed_out"
        assert not timed_out.is_known

    def test_legacy_flat_shape_still_parses(self):
        # Backward compat: pre-0.7 flat shape (counts at top level, no status).
        snap = PeerConnectivitySnapshot.from_dict({
            "reachable_peer_count": 2,
            "unknown_peer_count": 0,
            "unreachable_peers": [],
        })
        assert snap.reachable_peer_count == 2
        assert snap.status == "known"

    def test_rich_member_snapshot_reads_nested_counts(self):
        snapshot = RichMemberSnapshot.from_dict({
            "status": "active",
            "tokens_used": 5,
            "is_final": False,
            "peer_connectivity": {
                "status": "known",
                "snapshot": {
                    "reachable_peer_count": 3,
                    "unknown_peer_count": 1,
                    "unreachable_peers": [{"peer": "p1", "reason": "x"}],
                },
            },
        })
        assert snapshot.peer_connectivity is not None
        assert snapshot.peer_connectivity.reachable_peer_count == 3
        assert snapshot.peer_connectivity.unknown_peer_count == 1
        assert len(snapshot.peer_connectivity.unreachable_peers) == 1

    def test_progress_snapshot_parses_and_defaults(self):
        # meerkat 0.7.29 machine-owned liveness projection (ask 14).
        snapshot = RichMemberSnapshot.from_dict({
            "status": "active",
            "progress": {
                "run_state": "run_open",
                "in_flight_work": 2,
                "last_progress_at_ms": 1752300000000,
                "last_progress_event": "execution_advanced",
                "health": "healthy",
            },
        })
        assert snapshot.progress is not None
        assert snapshot.progress.run_state == "run_open"
        assert snapshot.progress.in_flight_work == 2
        assert snapshot.progress.last_progress_at_ms == 1752300000000
        assert snapshot.progress.last_progress_event == "execution_advanced"
        assert snapshot.progress.health == "healthy"

    def test_progress_absent_on_older_gateways(self):
        snapshot = RichMemberSnapshot.from_dict({"status": "active"})
        assert snapshot.progress is None

    def test_progress_tolerates_unknown_vocabulary(self):
        snapshot = RichMemberSnapshot.from_dict({
            "status": "active",
            "progress": {"run_state": "hibernating", "health": "quantum"},
        })
        assert snapshot.progress is not None
        assert snapshot.progress.run_state == "hibernating"
        assert snapshot.progress.health == "quantum"
        assert snapshot.progress.in_flight_work == 0
        assert snapshot.progress.last_progress_event == "unchanged"


class TestIdentityResolvedToolsResult:
    def test_from_dict(self):
        result = IdentityResolvedToolsResult.from_dict({
            "identity": "domain:security",
            "session_id": "sid-1",
            "tools": ["shell", "send_message"],
        })
        assert result.identity == "domain:security"
        assert result.session_id == "sid-1"
        assert result.tools == ["shell", "send_message"]


def _workgraph_item_payload(**overrides):
    payload = {
        "id": "item-1",
        "realm_id": "realm-1",
        "namespace": "default",
        "title": "Ship the feature",
        "description": "Land the WorkGraph SDK",
        "status": "open",
        "completion_policy": {"kind": "self_attest"},
        "priority": "high",
        "labels": ["backend"],
        "owner": {"key": {"kind": "agent", "id": "worker-1"}, "display_name": "Worker One"},
        "claim": {"owner": {"key": {"kind": "agent", "id": "worker-1"}}, "claimed_at": "2026-07-08T00:00:00Z"},
        "machine_state": {"lifecycle_phase": "open", "revision": 3},
        "revision": 3,
        "due_at": None,
        "not_before": None,
        "snoozed_until": None,
        "created_at": "2026-07-01T00:00:00Z",
        "updated_at": "2026-07-02T00:00:00Z",
        "terminal_at": None,
        "external_refs": [{"kind": "github", "id": "123"}],
        "evidence_refs": [{"kind": "self_attest", "id": "e1"}],
    }
    payload.update(overrides)
    return payload


def _workgraph_attention_binding_payload(**overrides):
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


class TestWorkGraphItem:
    def test_from_dict(self):
        item = WorkGraphItem.from_dict(_workgraph_item_payload())
        assert item.id == "item-1"
        assert item.realm_id == "realm-1"
        assert item.namespace == "default"
        assert item.title == "Ship the feature"
        assert item.description == "Land the WorkGraph SDK"
        assert item.status == "open"
        assert item.completion_policy == {"kind": "self_attest"}
        assert item.priority == "high"
        assert item.labels == ["backend"]
        assert item.owner == {
            "key": {"kind": "agent", "id": "worker-1"},
            "display_name": "Worker One",
        }
        assert item.claim["owner"]["key"]["id"] == "worker-1"
        assert item.machine_state == {"lifecycle_phase": "open", "revision": 3}
        assert item.revision == 3
        assert item.created_at == "2026-07-01T00:00:00Z"
        assert item.updated_at == "2026-07-02T00:00:00Z"
        assert item.terminal_at is None
        assert item.external_refs == [{"kind": "github", "id": "123"}]
        assert item.evidence_refs == [{"kind": "self_attest", "id": "e1"}]

    def test_from_dict_defaults_missing_keys(self):
        item = WorkGraphItem.from_dict({})
        assert item.id == ""
        assert item.title == ""
        assert item.description is None
        assert item.status == ""
        assert item.completion_policy == {}
        assert item.priority == ""
        assert item.labels == []
        assert item.owner is None
        assert item.claim is None
        assert item.machine_state == {}
        assert item.revision == 0
        assert item.due_at is None
        assert item.external_refs == []
        assert item.evidence_refs == []


class TestWorkGraphEdge:
    def test_from_dict(self):
        edge = WorkGraphEdge.from_dict({
            "realm_id": "realm-1",
            "namespace": "default",
            "kind": "blocks",
            "from_id": "item-1",
            "to_id": "item-2",
            "created_at": "2026-07-01T00:00:00Z",
        })
        assert edge.realm_id == "realm-1"
        assert edge.kind == "blocks"
        assert edge.from_id == "item-1"
        assert edge.to_id == "item-2"
        assert edge.created_at == "2026-07-01T00:00:00Z"

    def test_from_dict_defaults_missing_keys(self):
        edge = WorkGraphEdge.from_dict({})
        assert edge.realm_id == ""
        assert edge.kind == ""
        assert edge.from_id == ""
        assert edge.to_id == ""


class TestWorkGraphAttentionBinding:
    def test_from_dict(self):
        binding = WorkGraphAttentionBinding.from_dict(_workgraph_attention_binding_payload())
        assert binding.binding_id == "binding-1"
        assert binding.work_ref == {
            "item_id": "item-1",
            "realm_id": "realm-1",
            "namespace": "default",
        }
        assert binding.target == {"kind": "session", "session_id": "sess-1"}
        assert binding.mode == "pursue"
        assert binding.status == {"state": "active"}
        assert binding.machine_state == {"lifecycle_phase": "active", "revision": 1}
        assert binding.delegated_authority == "add_evidence"
        assert binding.projection_policy == {
            "max_text_chars": 4096,
            "include_parent_context": True,
        }

    def test_from_dict_defaults_missing_keys(self):
        binding = WorkGraphAttentionBinding.from_dict({})
        assert binding.binding_id == ""
        assert binding.work_ref == {}
        assert binding.target == {}
        assert binding.mode == ""
        assert binding.status == {}
        assert binding.machine_state == {}
        assert binding.delegated_authority is None
        assert binding.projection_policy == {}


class TestWorkGraphSnapshotResult:
    def test_from_dict(self):
        snapshot = WorkGraphSnapshotResult.from_dict({
            "realm_id": "realm-1",
            "namespace": "default",
            "all_namespaces": False,
            "captured_at": "2026-07-08T00:00:00Z",
            "event_high_water_mark": 42,
            "items": [_workgraph_item_payload()],
            "edges": [{
                "realm_id": "realm-1",
                "namespace": "default",
                "kind": "parent",
                "from_id": "item-0",
                "to_id": "item-1",
                "created_at": "2026-07-01T00:00:00Z",
            }],
            "attention": [_workgraph_attention_binding_payload()],
            "ready_item_ids": ["item-1"],
        })
        assert snapshot.realm_id == "realm-1"
        assert snapshot.all_namespaces is False
        assert snapshot.event_high_water_mark == 42
        assert len(snapshot.items) == 1
        assert snapshot.items[0].id == "item-1"
        assert len(snapshot.edges) == 1
        assert snapshot.edges[0].kind == "parent"
        assert len(snapshot.attention) == 1
        assert snapshot.attention[0].binding_id == "binding-1"
        assert snapshot.ready_item_ids == ["item-1"]

    def test_from_dict_defaults_missing_keys(self):
        snapshot = WorkGraphSnapshotResult.from_dict({})
        assert snapshot.realm_id == ""
        assert snapshot.namespace is None
        assert snapshot.all_namespaces is False
        assert snapshot.event_high_water_mark is None
        assert snapshot.items == []
        assert snapshot.edges == []
        assert snapshot.attention == []
        assert snapshot.ready_item_ids == []


class TestWorkGraphItemsResult:
    def test_from_dict(self):
        result = WorkGraphItemsResult.from_dict({"items": [_workgraph_item_payload()]})
        assert len(result.items) == 1
        assert result.items[0].id == "item-1"

    def test_from_dict_defaults_missing_keys(self):
        result = WorkGraphItemsResult.from_dict({})
        assert result.items == []


class TestWorkGraphGoalResult:
    def test_from_dict(self):
        result = WorkGraphGoalResult.from_dict({
            "item": _workgraph_item_payload(),
            "attention": _workgraph_attention_binding_payload(),
        })
        assert result.item.id == "item-1"
        assert result.attention.binding_id == "binding-1"

    def test_from_dict_defaults_missing_keys(self):
        result = WorkGraphGoalResult.from_dict({})
        assert result.item.id == ""
        assert result.attention.binding_id == ""


class TestWorkGraphAttentionReassignResult:
    def test_from_dict(self):
        result = WorkGraphAttentionReassignResult.from_dict({
            "previous": _workgraph_attention_binding_payload(binding_id="binding-old"),
            "attention": _workgraph_attention_binding_payload(binding_id="binding-new"),
        })
        assert result.previous.binding_id == "binding-old"
        assert result.attention.binding_id == "binding-new"

    def test_from_dict_defaults_missing_keys(self):
        result = WorkGraphAttentionReassignResult.from_dict({})
        assert result.previous.binding_id == ""
        assert result.attention.binding_id == ""


class TestWorkGraphEventEntry:
    def test_from_dict(self):
        entry = WorkGraphEventEntry.from_dict({
            "seq": 7,
            "realm_id": "realm-1",
            "namespace": "default",
            "item_id": "item-1",
            "kind": "created",
            "at": "2026-07-01T00:00:00Z",
            "payload": {"title": "Ship the feature"},
        })
        assert entry.seq == 7
        assert entry.realm_id == "realm-1"
        assert entry.item_id == "item-1"
        assert entry.kind == "created"
        assert entry.payload == {"title": "Ship the feature"}

    def test_from_dict_defaults_missing_keys(self):
        entry = WorkGraphEventEntry.from_dict({})
        assert entry.seq is None
        assert entry.realm_id == ""
        assert entry.namespace == ""
        assert entry.item_id is None
        assert entry.kind == ""
        assert entry.at == ""
        assert entry.payload is None

    def test_graph_level_event_has_no_item_id(self):
        """Graph-level events (e.g. `linked`) carry no `item_id`."""
        entry = WorkGraphEventEntry.from_dict({
            "realm_id": "realm-1",
            "namespace": "default",
            "kind": "linked",
            "at": "2026-07-01T00:00:00Z",
            "payload": {},
        })
        assert entry.item_id is None
        assert entry.kind == "linked"
