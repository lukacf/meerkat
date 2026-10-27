"""Tests for AgentCustomizer external-tool handler registration.

The identity-first ``AgentCustomizer.customize_build`` callback runs on BOTH
fresh create and restore/reconcile (unlike ``build_agent``, whose options are
empty on restore). A customizer can register callable tools via
``draft.register_tool(...)``; those handlers ride the SAME (scope_id, tool)
map + ``callback/call_tool`` dispatch that ``build_agent`` uses, so resumed
agents keep their identity-scoped tools (MCP, comms, etc.).
"""
import pytest
from meerkat_meta.agent_builder import CallbackDispatcher
from meerkat_meta.identity_first_models import AgentBuildDraft

_CONTEXT = {"identity": "agent:alpha", "active_peers": [], "managed_edges": []}
_SPEC = {"identity": "agent:alpha", "profile": "default"}


class _ToolCustomizer:
    """Customizer that registers a sync and an async tool on the draft."""

    async def customize_build(self, context, spec, draft) -> None:
        draft.register_tool(
            "sync_tool",
            lambda args: {"echo": args.get("input", "")},
            description="echoes input",
            input_schema={"type": "object", "properties": {"input": {"type": "string"}}},
        )

        async def async_handler(args):
            return {"async_echo": args.get("input", "")}

        draft.register_tool("async_tool", async_handler)


def _customize_params(scope_id, draft=None):
    params = {"context": _CONTEXT, "spec": _SPEC, "draft": draft or {}}
    if scope_id is not None:
        params["scope_id"] = scope_id
    return params


class TestCustomizerToolDispatch:
    @pytest.fixture
    def dispatcher(self):
        d = CallbackDispatcher()
        d.register_agent_customizer(_ToolCustomizer())
        return d

    @pytest.mark.asyncio
    async def test_customize_build_captures_handlers(self, dispatcher):
        result = await dispatcher.handle_callback(
            "callback/agent_customizer/customize_build", _customize_params("c1")
        )
        # The returned draft carries tool DEFS only — handlers stay in-process.
        names = [t["name"] for t in result["external_tools"]]
        assert names == ["sync_tool", "async_tool"]
        for tool in result["external_tools"]:
            assert "handler" not in tool
        assert ("c1", "sync_tool") in dispatcher._tool_handlers
        assert ("c1", "async_tool") in dispatcher._tool_handlers

    @pytest.mark.asyncio
    async def test_call_sync_tool(self, dispatcher):
        await dispatcher.handle_callback(
            "callback/agent_customizer/customize_build", _customize_params("c1")
        )
        result = await dispatcher.handle_callback(
            "callback/call_tool",
            {"scope_id": "c1", "tool": "sync_tool", "arguments": {"input": "hello"}},
        )
        assert result == {"content": {"echo": "hello"}}

    @pytest.mark.asyncio
    async def test_call_async_tool(self, dispatcher):
        await dispatcher.handle_callback(
            "callback/agent_customizer/customize_build", _customize_params("c1")
        )
        result = await dispatcher.handle_callback(
            "callback/call_tool",
            {"scope_id": "c1", "tool": "async_tool", "arguments": {"input": "world"}},
        )
        assert result == {"content": {"async_echo": "world"}}

    @pytest.mark.asyncio
    async def test_scope_isolation(self, dispatcher):
        await dispatcher.handle_callback(
            "callback/agent_customizer/customize_build", _customize_params("c1")
        )
        with pytest.raises(ValueError, match="no handler registered"):
            await dispatcher.handle_callback(
                "callback/call_tool",
                {"scope_id": "c2", "tool": "sync_tool", "arguments": {}},
            )

    @pytest.mark.asyncio
    async def test_restore_reregisters_under_new_scope(self, dispatcher):
        """customize_build re-invoked on restore with a NEW scope: latest wins.

        Models the gateway minting a fresh ``customize-<identity>-N`` scope on
        every reconcile; the host re-registers handlers and the newest scope
        dispatches. Releasing the stale scope leaves the latest working.
        """
        await dispatcher.handle_callback(
            "callback/agent_customizer/customize_build", _customize_params("customize-agent:alpha-1")
        )
        await dispatcher.handle_callback(
            "callback/agent_customizer/customize_build", _customize_params("customize-agent:alpha-2")
        )
        # Latest scope dispatches.
        result = await dispatcher.handle_callback(
            "callback/call_tool",
            {"scope_id": "customize-agent:alpha-2", "tool": "sync_tool", "arguments": {"input": "hi"}},
        )
        assert result == {"content": {"echo": "hi"}}
        # Release the stale scope; the latest still dispatches.
        dispatcher.release_scope("customize-agent:alpha-1")
        assert ("customize-agent:alpha-1", "sync_tool") not in dispatcher._tool_handlers
        result = await dispatcher.handle_callback(
            "callback/call_tool",
            {"scope_id": "customize-agent:alpha-2", "tool": "sync_tool", "arguments": {"input": "yo"}},
        )
        assert result == {"content": {"echo": "yo"}}

    @pytest.mark.asyncio
    async def test_missing_scope_id_degrades_gracefully(self, dispatcher):
        """A gateway that does not yet send scope_id: no crash, defs still returned.

        Handlers simply aren't harvested (tools appear declared-only).
        """
        result = await dispatcher.handle_callback(
            "callback/agent_customizer/customize_build", _customize_params(None)
        )
        names = [t["name"] for t in result["external_tools"]]
        assert names == ["sync_tool", "async_tool"]
        # No scope means no harvested handlers.
        assert all(k[1] != "sync_tool" for k in dispatcher._tool_handlers)


class TestAgentBuildDraftRegisterTool:
    def test_register_tool_appends_def_and_stores_handler(self):
        draft = AgentBuildDraft()
        draft.register_tool("t", lambda args: args, description="d", input_schema={"type": "object"})
        assert len(draft.external_tools) == 1
        assert draft.external_tools[0].name == "t"
        assert draft.external_tools[0].description == "d"
        assert "t" in draft.tool_handlers

    def test_register_tool_default_schema(self):
        draft = AgentBuildDraft()
        draft.register_tool("t", lambda args: args)
        assert draft.external_tools[0].input_schema == {"type": "object"}

    def test_to_dict_omits_handlers(self):
        draft = AgentBuildDraft()
        draft.register_tool("t", lambda args: args)
        assert "tool_handlers" not in draft.to_dict()
        assert draft.to_dict()["external_tools"] == [
            {"name": "t", "description": "", "input_schema": {"type": "object"}}
        ]

    def test_non_callable_handler_raises(self):
        draft = AgentBuildDraft()
        with pytest.raises(TypeError, match="handler must be callable"):
            draft.register_tool("bad", "not_a_function")


class TestAgentBuildDraftProviderParams:
    """The gateway replaces the draft wholesale with what the SDK returns, so a
    customizer that never touches provider params must still hand them back
    untouched — otherwise every SDK-backed build silently clears the identity's
    cache policy."""

    PROVIDER_PARAMS = {
        "provider_tag": {
            "provider": "open_ai",
            "prompt_cache_key": "tenant-a:stable-prefix",
            "prompt_cache_options": {"mode": "implicit", "ttl": "30m"},
        }
    }

    def test_round_trips_provider_params(self):
        draft = AgentBuildDraft.from_dict(
            {"model": "gpt-5.5", "provider_params": self.PROVIDER_PARAMS}
        )
        assert draft.provider_params == self.PROVIDER_PARAMS
        assert draft.to_dict()["provider_params"] == self.PROVIDER_PARAMS

    def test_omits_provider_params_when_unset(self):
        draft = AgentBuildDraft.from_dict({"model": "gpt-5.5"})
        assert draft.provider_params is None
        assert "provider_params" not in draft.to_dict()
