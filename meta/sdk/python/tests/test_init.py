"""Tests that the public surface is correct and legacy symbols are removed."""
import pytest
import meerkat_meta


class TestNewSymbolsExist:
    @pytest.mark.parametrize("name", [
        "Meta", "MetaBuilder", "MetaRuntime",
        "DiscoverySpec", "PreSpawnData", "SessionBuildOptions", "SessionQuery",
        "SessionAgentBuilder",
        "MetaError", "TransportError", "RpcError",
        "GatewayStartupCleanupError", "GatewayCleanup", "TransportTermination",
        "CapabilityUnavailableError", "ConsoleTimelineReplayUnavailableError",
        "ContractMismatchError", "NotConnectedError",
        "StatusResult", "CapabilitiesResult", "RuntimeCapabilities", "ProfileCapabilities", "ReconcileResult",
        "SpawnResult", "SpawnMemberResult", "SendMessageResult", "SubscribeResult",
        "KeepAliveConfig", "EventEnvelope",
        "RoutingResolution", "DeliveryResult", "MemoryQueryResult", "CallToolResult",
        "ReconcileEdgesReport", "RediscoverReport", "ToolCaller",
        "MobpackAgentDefinitionsResult", "MobpackCatalogsResult",
        "MobpackSkillsCatalogResult", "MobpackTemplatesResult",
        "MobpackToolsCatalogResult",
        "ScheduleToolDefinition", "ScheduleToolsResult",
        "MobpackValidationResult", "MobpackDiagnostic", "MobpackDisplayRow",
        "MobpackSourceFile", "MobpackSourceResult", "MobpackExportResult",
        "MobpackImportResult", "MobpackDraftRow", "MobpackDraftSummary",
        "MobpackDraftListResult",
        "MobpackDraftGetResult", "MobpackDraftSaveResult",
        "MobpackDraftHistoryResult",
        "MobpackDraftDeleteResult", "MobpackApplyOperationResult",
        "MobpackDeployCommandResult", "MobpackDeployResult",
        "Event", "MobEvent", "AgentEvent", "EventStream",
        "RunStarted", "RunCompleted", "RunFailed",
        "TurnStarted", "TextDelta", "TextComplete",
        "ToolCallRequested", "ToolResultReceived", "TurnCompleted",
        "ToolExecutionStarted", "ToolExecutionCompleted", "UnknownEvent",
        "auth", "memory",
    ])
    def test_symbol_exists(self, name):
        assert hasattr(meerkat_meta, name), f"{name} should be in public surface"


class TestIdentityFirstCustomizerExports:
    """The customizer boundary types are importable from the top level.

    ``customize_build(context, spec, draft)`` is the per-identity equivalent of
    ``SessionAgentBuilder.build_agent``; its argument types used to be public
    only through ``meerkat_meta.identity_first_models`` /
    ``identity_first_providers``.
    """

    NAMES = [
        "AgentBuildContext",
        "AgentBuildDraft",
        "AgentCustomizerProtocol",
        "DurableAgentSpec",
        "ExternalToolDef",
    ]

    @pytest.mark.parametrize("name", NAMES)
    def test_symbol_is_exported_and_declared(self, name):
        assert hasattr(meerkat_meta, name), f"{name} should be importable from meerkat_meta"
        assert name in meerkat_meta.__all__, f"{name} should be declared in __all__"

    def test_top_level_names_are_the_canonical_objects(self):
        from meerkat_meta import (
            AgentBuildContext,
            AgentBuildDraft,
            AgentCustomizerProtocol,
            DurableAgentSpec,
            ExternalToolDef,
        )
        from meerkat_meta import identity_first_models, identity_first_providers

        assert AgentBuildContext is identity_first_models.AgentBuildContext
        assert AgentBuildDraft is identity_first_models.AgentBuildDraft
        assert DurableAgentSpec is identity_first_models.DurableAgentSpec
        assert ExternalToolDef is identity_first_models.ExternalToolDef
        assert AgentCustomizerProtocol is identity_first_providers.AgentCustomizerProtocol

    def test_draft_register_tool_round_trips_through_top_level_import(self):
        from meerkat_meta import AgentBuildDraft, ExternalToolDef

        draft = AgentBuildDraft()
        draft.register_tool("lookup", lambda args: {"ok": True}, description="lookup")
        assert draft.external_tools == [
            ExternalToolDef(name="lookup", description="lookup", input_schema={"type": "object"})
        ]


class TestLegacySymbolsRemoved:
    @pytest.mark.parametrize("name", [
        "MetaTypedClient",
        "MetaAsyncTypedClient",
        "MetaRpcError",
        "create_gateway_sync_transport",
        "create_gateway_async_transport",
        "create_http_transport",
        "PersistentTransport",
        "create_persistent_transport",
        "InteractionEvent",
        "SseEvent",
        "SseEventStream",
        "parse_sse_stream",
        "MobHandle",
        "SseBridge",
        "CallbackDispatcher",
        "ModuleDefinition",
        "ModuleSpec",
        "ModuleTool",
        "build_console_experience_route",
        "build_console_modules_route",
        "build_console_route",
        "build_console_routes",
        "build_module_spec",
        "decorate_module_spec",
        "decorate_module_tool",
        "define_module",
        "define_module_spec",
        "define_module_tool",
    ])
    def test_symbol_removed(self, name):
        assert not hasattr(meerkat_meta, name), f"{name} should NOT be in public surface"


class TestHelpersStillImportable:
    """Module authoring helpers are importable via meerkat_meta.helpers."""

    def test_helpers_importable(self):
        from meerkat_meta.helpers import (
            ModuleDefinition,
            ModuleSpec,
            ModuleTool,
            build_module_spec,
            define_module,
            define_module_tool,
        )
        assert ModuleSpec is not None
