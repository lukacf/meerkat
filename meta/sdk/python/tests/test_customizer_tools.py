"""Tests for AgentCustomizer external-tool handler registration.

The identity-first ``AgentCustomizer.customize_build`` callback runs when an
identity is created (seated), starts a fresh life or moves to a declared new
role, and for each build of its member, the first build after a restart (the
member's first wake) included (unlike ``build_agent``, whose options are
empty on restore). At a restart it runs only for the members the start
builds (for example, members that owe work or a kickoff, or every member
under ``member_restore: "eager"``), after they are built and before they
run; a restart calls it for none of the other members until they are built. A
customizer can register callable tools via ``draft.register_tool(...)``;
those handlers ride the SAME (scope_id, tool) map + ``callback/call_tool``
dispatch that ``build_agent`` uses, and each build registers them again, so
a rebuilt member keeps its identity-scoped tools (MCP, comms, etc.).
"""
import asyncio
import dataclasses
import json
import queue
import threading
from unittest.mock import MagicMock

import pytest
from meerkat_meta._transport import PersistentTransport
from meerkat_meta.agent_builder import CallbackDispatcher
from meerkat_meta.errors import ContractMismatchError
from meerkat_meta.identity_first_models import AgentBuildContext, AgentBuildDraft
from meerkat_meta.models import SessionCreatedContext

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
        """customize_build invoked again with a NEW scope: latest wins.

        Models the gateway minting a fresh ``customize-<identity>-N`` scope for
        each build of the member (a restarted member's first wake included);
        the host re-registers handlers and the newest scope dispatches.
        Releasing the stale scope leaves the latest working.
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


class TestCallbacksOfAHostWithMoreThanOneMob:
    """The three callbacks that build a member carry the member's mob.

    A host that serves more than one mob sends a top-level ``mob_id`` with
    ``customize_build``, ``after_create`` and ``compute_edges``. Two mobs can
    each have a member of the same name, so the callback tools a customizer
    registers are kept per mob and identity: a build in one mob leaves the
    other mob's tools in place. A host that sends no ``mob_id`` has its own
    mob only; its callbacks read as before.
    """

    class _Recorder(_ToolCustomizer):
        def __init__(self) -> None:
            self.build_contexts: list = []
            self.created: list = []

        async def customize_build(self, context, spec, draft) -> None:
            self.build_contexts.append(context)
            await super().customize_build(context, spec, draft)

        async def after_create(self, identity, session_id, context) -> None:
            self.created.append((identity, session_id, context))

    @pytest.fixture
    def customizer(self):
        return self._Recorder()

    @pytest.fixture
    def dispatcher(self, customizer):
        d = CallbackDispatcher()
        d.register_agent_customizer(customizer)
        return d

    @staticmethod
    async def _build(dispatcher, scope_id, mob_id=None):
        params = _customize_params(scope_id)
        if mob_id is not None:
            params["mob_id"] = mob_id
        return await dispatcher.handle_callback(
            "callback/agent_customizer/customize_build", params
        )

    @staticmethod
    async def _serves(dispatcher, scope_id) -> bool:
        try:
            result = await dispatcher.handle_callback(
                "callback/call_tool",
                {"scope_id": scope_id, "tool": "sync_tool", "arguments": {"input": "hi"}},
            )
        except ValueError:
            return False
        return result == {"content": {"echo": "hi"}}

    @pytest.mark.asyncio
    async def test_two_mobs_with_the_same_member_name_keep_both_scopes_live(self, dispatcher):
        await self._build(dispatcher, "customize-agent:alpha-1", "mob-a")
        await self._build(dispatcher, "customize-agent:alpha-2", "mob-b")

        # The second mob's build of agent:alpha did not release the first's.
        assert await self._serves(dispatcher, "customize-agent:alpha-1")
        assert await self._serves(dispatcher, "customize-agent:alpha-2")

    @pytest.mark.asyncio
    async def test_a_rebuild_releases_the_scope_of_its_own_mob_only(self, dispatcher):
        await self._build(dispatcher, "customize-agent:alpha-1", "mob-a")
        await self._build(dispatcher, "customize-agent:alpha-2", "mob-b")
        await self._build(dispatcher, "customize-agent:alpha-3", "mob-a")

        # Newest wins inside mob-a; mob-b's member keeps its tools.
        assert not await self._serves(dispatcher, "customize-agent:alpha-1")
        assert await self._serves(dispatcher, "customize-agent:alpha-2")
        assert await self._serves(dispatcher, "customize-agent:alpha-3")

    @pytest.mark.asyncio
    async def test_a_build_without_a_mob_is_the_hosts_own_mob(self, dispatcher):
        # A host that sends no mob_id (an earlier one) and a named mob are
        # two members: neither build releases the other's tools.
        await self._build(dispatcher, "customize-agent:alpha-1")
        await self._build(dispatcher, "customize-agent:alpha-2", "mob-b")
        assert await self._serves(dispatcher, "customize-agent:alpha-1")
        assert await self._serves(dispatcher, "customize-agent:alpha-2")

        # And without a mob the newest build of an identity still wins.
        await self._build(dispatcher, "customize-agent:alpha-3")
        assert not await self._serves(dispatcher, "customize-agent:alpha-1")
        assert await self._serves(dispatcher, "customize-agent:alpha-2")
        assert await self._serves(dispatcher, "customize-agent:alpha-3")

    @pytest.mark.asyncio
    async def test_customize_build_tells_the_customizer_the_members_mob(
        self, dispatcher, customizer
    ):
        await self._build(dispatcher, "customize-agent:alpha-1", "mob-b")
        await self._build(dispatcher, "customize-agent:alpha-2")

        named, unnamed = customizer.build_contexts
        assert (named.identity, named.mob_id) == ("agent:alpha", "mob-b")
        # No mob_id from the host: the host's own mob.
        assert (unnamed.identity, unnamed.mob_id) == ("agent:alpha", None)
        # The context's own wire form is unchanged: the mob rides beside it.
        assert named.to_dict() == _CONTEXT

    @pytest.mark.asyncio
    @pytest.mark.parametrize("not_text", [{"id": "mob-b"}, 7, True, ["mob-b"]])
    async def test_a_mob_id_that_is_not_text_is_refused(self, dispatcher, customizer, not_text):
        # A host that names mobs names them by text. Reading another value as
        # "no mob" would hand the customizer the host's own mob for a member
        # of another one, so each of the three callbacks is refused instead.
        await self._build(dispatcher, "customize-agent:alpha-1")
        params = _customize_params("customize-agent:alpha-2")
        params["mob_id"] = not_text
        with pytest.raises(ContractMismatchError):
            await dispatcher.handle_callback("callback/agent_customizer/customize_build", params)
        with pytest.raises(ContractMismatchError):
            await dispatcher.handle_callback(
                "callback/agent_customizer/after_create",
                {"identity": "agent:alpha", "session_id": "session-1", "mob_id": not_text},
            )

        # The customizer was not asked, and the refused build registered and
        # released nothing.
        assert len(customizer.build_contexts) == 1 and customizer.created == []
        assert await self._serves(dispatcher, "customize-agent:alpha-1")
        assert not await self._serves(dispatcher, "customize-agent:alpha-2")

    @pytest.mark.asyncio
    @pytest.mark.parametrize("not_text", [{"id": "mob-b"}, 7, True, ["mob-b"]])
    async def test_compute_edges_refuses_a_mob_id_that_is_not_text(self, not_text):
        asked: list = []

        class _Topology:
            async def compute_edges(self, target_identities, context):
                asked.append(context)
                return []

        dispatcher = CallbackDispatcher()
        dispatcher.register_topology_provider(_Topology())
        with pytest.raises(ContractMismatchError):
            await dispatcher.handle_callback(
                "callback/topology_provider/compute_edges",
                {"target_identities": ["agent:alpha"], "context": {}, "mob_id": not_text},
            )
        assert asked == []

    @pytest.mark.asyncio
    async def test_a_null_mob_id_is_the_hosts_own_mob(self, dispatcher, customizer):
        params = _customize_params("customize-agent:alpha-1")
        params["mob_id"] = None
        await dispatcher.handle_callback("callback/agent_customizer/customize_build", params)
        await self._build(dispatcher, "customize-agent:alpha-2")

        # Null is absent: told as none, and one member with the build that
        # names no mob.
        assert [context.mob_id for context in customizer.build_contexts] == [None, None]
        assert not await self._serves(dispatcher, "customize-agent:alpha-1")
        assert await self._serves(dispatcher, "customize-agent:alpha-2")

    def test_a_context_subclass_may_add_a_required_field(self):
        # The mob is told by keyword only, so a context a host extends with a
        # field of its own that has no default still defines, and the
        # contexts are built by position as before.
        @dataclasses.dataclass(frozen=True)
        class _BuildContext(AgentBuildContext):
            tenant: str

        @dataclasses.dataclass(frozen=True)
        class _CreatedContext(SessionCreatedContext):
            tenant: str

        build = _BuildContext("agent:alpha", [], [], "acme")
        assert (build.tenant, build.mob_id) == ("acme", None)
        assert _BuildContext("agent:alpha", [], [], "acme", mob_id="mob-b").mob_id == "mob-b"
        created = _CreatedContext("gpt-5.5", {}, None, "acme")
        assert (created.tenant, created.mob_id) == ("acme", None)
        assert _CreatedContext("gpt-5.5", {}, None, "acme", mob_id="mob-b").mob_id == "mob-b"
        assert AgentBuildContext("agent:alpha", [], []) == AgentBuildContext(
            "agent:alpha", [], [], mob_id=None
        )

    @pytest.mark.asyncio
    async def test_after_create_tells_the_customizer_the_members_mob(
        self, dispatcher, customizer
    ):
        created = {"identity": "agent:alpha", "session_id": "session-1", "model": "gpt-5.5"}
        await dispatcher.handle_callback(
            "callback/agent_customizer/after_create", {**created, "mob_id": "mob-b"}
        )
        await dispatcher.handle_callback("callback/agent_customizer/after_create", created)

        (_, _, named), (_, _, unnamed) = customizer.created
        assert (named.model, named.mob_id) == ("gpt-5.5", "mob-b")
        assert (unnamed.model, unnamed.mob_id) == ("gpt-5.5", None)

    @pytest.mark.asyncio
    async def test_compute_edges_tells_the_provider_the_mob(self):
        seen: list = []

        class _Topology:
            async def compute_edges(self, target_identities, context):
                seen.append((list(target_identities), context))
                return []

        dispatcher = CallbackDispatcher()
        dispatcher.register_topology_provider(_Topology())
        roster = {"roster": [{"identity": "agent:alpha", "profile": "default"}]}
        params = {"target_identities": ["agent:alpha"], "context": roster}

        await dispatcher.handle_callback(
            "callback/topology_provider/compute_edges", {**params, "mob_id": "mob-b"}
        )
        await dispatcher.handle_callback("callback/topology_provider/compute_edges", params)

        (targets, named), (_, unnamed) = seen
        assert targets == ["agent:alpha"]
        # The provider's context names the mob; what the host sent as context
        # is otherwise as it was.
        assert named == {**roster, "mob_id": "mob-b"}

        # A context that is not an object has no place for the mob: it is
        # handed on as it came.
        for not_an_object in (["agent:alpha"], "roster", None):
            await dispatcher.handle_callback(
                "callback/topology_provider/compute_edges",
                {**params, "context": not_an_object, "mob_id": "mob-b"},
            )
            assert seen[-1][1] == not_an_object and type(seen[-1][1]) is type(not_an_object)
        # No mob_id from the host: the context is what it was before.
        assert unnamed == roster
        assert params["context"] == roster


_CUSTOMIZE = "callback/agent_customizer/customize_build"
_LEFT_OUT = object()


class _CountingCustomizer:
    """Registers one tool whose answer names the build that registered it,
    and records the mob each build's context names."""

    def __init__(self) -> None:
        self.builds = 0
        self.mobs: list = []

    async def customize_build(self, context, spec, draft) -> None:
        self.builds += 1
        build = self.builds
        self.mobs.append(context.mob_id)
        draft.register_tool("lookup", lambda args: {"registered_by_build": build})


def _build_params(scope_id, mob_id=_LEFT_OUT, **extra):
    """Params of one customize callback. ``mob_id`` is sent as given (a null
    included) unless it is left out."""
    params = {"scope_id": scope_id, "context": _CONTEXT, "spec": _SPEC, "draft": {}}
    if mob_id is not _LEFT_OUT:
        params["mob_id"] = mob_id
    params.update(extra)
    return params


async def _lookup(dispatcher, scope_id):
    return await dispatcher.handle_callback(
        "callback/call_tool", {"scope_id": scope_id, "tool": "lookup", "arguments": {}}
    )


class TestCustomizerScopeKeyedByMobAndMember:
    """One live customize scope per (mob, member name): each scope reaches
    the handler its own build registered, the member name stays part of the
    key, and one mob alone behaves as a host that names none."""

    @pytest.fixture
    def customizer(self):
        return _CountingCustomizer()

    @pytest.fixture
    def dispatcher(self, customizer):
        d = CallbackDispatcher()
        d.register_agent_customizer(customizer)
        return d

    @pytest.mark.asyncio
    async def test_each_scope_of_two_mobs_reaches_the_handler_its_own_build_registered(
        self, dispatcher
    ):
        await dispatcher.handle_callback(_CUSTOMIZE, _build_params("customize-agent:alpha-1", "mob-a"))
        await dispatcher.handle_callback(_CUSTOMIZE, _build_params("customize-agent:alpha-2", "mob-b"))
        await dispatcher.handle_callback(_CUSTOMIZE, _build_params("customize-agent:alpha-3", "mob-a"))
        with pytest.raises(ValueError, match="no handler registered"):
            await _lookup(dispatcher, "customize-agent:alpha-1")
        assert await _lookup(dispatcher, "customize-agent:alpha-2") == {
            "content": {"registered_by_build": 2}
        }
        assert await _lookup(dispatcher, "customize-agent:alpha-3") == {
            "content": {"registered_by_build": 3}
        }
        # One live scope per (mob, member name): two, not three.
        assert sorted(dispatcher._scope_tools) == [
            "customize-agent:alpha-2",
            "customize-agent:alpha-3",
        ]

    @pytest.mark.asyncio
    async def test_two_member_names_of_one_mob_keep_both_scopes(self, dispatcher):
        """The member name stays part of the key."""
        beta = {"identity": "agent:beta", "active_peers": [], "managed_edges": []}
        await dispatcher.handle_callback(_CUSTOMIZE, _build_params("customize-agent:alpha-1", "mob-a"))
        await dispatcher.handle_callback(
            _CUSTOMIZE, _build_params("customize-agent:beta-2", "mob-a", context=beta)
        )
        assert await _lookup(dispatcher, "customize-agent:alpha-1") == {
            "content": {"registered_by_build": 1}
        }
        assert await _lookup(dispatcher, "customize-agent:beta-2") == {
            "content": {"registered_by_build": 2}
        }

    @pytest.mark.asyncio
    async def test_one_mob_alone_behaves_as_before(self, dispatcher):
        """One mob on the gateway: every build names the same mob, and a host
        observes what it observed with a gateway that names none."""
        await dispatcher.handle_callback(_CUSTOMIZE, _build_params("customize-agent:alpha-1", "mob-a"))
        await dispatcher.handle_callback(_CUSTOMIZE, _build_params("customize-agent:alpha-2", "mob-a"))
        with pytest.raises(ValueError, match="no handler registered"):
            await _lookup(dispatcher, "customize-agent:alpha-1")
        assert await _lookup(dispatcher, "customize-agent:alpha-2") == {
            "content": {"registered_by_build": 2}
        }
        assert sorted(dispatcher._scope_tools) == ["customize-agent:alpha-2"]

    @pytest.mark.asyncio
    async def test_a_member_the_sdk_does_not_know_is_ignored_in_either_place(self, dispatcher):
        """A callback is answered, and its draft comes back, with an unknown
        member beside the context and inside it."""
        context = dict(_CONTEXT, some_later_member="x")
        result = await dispatcher.handle_callback(
            _CUSTOMIZE,
            _build_params("customize-agent:alpha-1", some_later_member="x", context=context),
        )
        assert [tool["name"] for tool in result["external_tools"]] == ["lookup"]
        assert await _lookup(dispatcher, "customize-agent:alpha-1") == {
            "content": {"registered_by_build": 1}
        }

    @pytest.mark.asyncio
    async def test_a_mob_inside_the_context_object_is_not_the_members_mob(
        self, dispatcher, customizer
    ):
        """The mob travels beside the context. A ``mob_id`` inside the
        context object is a member this SDK does not know: it names no mob
        for the customizer and none for the key."""
        context = dict(_CONTEXT, mob_id="mob-inside")
        await dispatcher.handle_callback(
            _CUSTOMIZE, _build_params("customize-agent:alpha-1", context=context)
        )
        await dispatcher.handle_callback(
            _CUSTOMIZE, _build_params("customize-agent:alpha-2", "mob-a", context=context)
        )
        assert customizer.mobs == [None, "mob-a"]
        # Two keys (no mob, and mob-a): neither build released the other.
        assert sorted(dispatcher._scope_tools) == [
            "customize-agent:alpha-1",
            "customize-agent:alpha-2",
        ]

    @pytest.mark.asyncio
    async def test_a_mob_id_that_is_a_number_is_refused_and_releases_nothing(
        self, dispatcher, customizer
    ):
        """A ``mob_id`` that is present and not text is never read as no mob:
        the callback is refused, the customizer is not asked, and the build
        that named no mob keeps its scope."""
        await dispatcher.handle_callback(_CUSTOMIZE, _build_params("customize-agent:alpha-1"))
        with pytest.raises(ContractMismatchError):
            await dispatcher.handle_callback(_CUSTOMIZE, _build_params("customize-agent:alpha-2", 7))
        assert customizer.mobs == [None]
        assert await _lookup(dispatcher, "customize-agent:alpha-1") == {
            "content": {"registered_by_build": 1}
        }
        assert sorted(dispatcher._scope_tools) == ["customize-agent:alpha-1"]

    @pytest.mark.asyncio
    async def test_an_empty_mob_id_is_a_mob_of_its_own(self, dispatcher, customizer):
        """Text is a mob's id, the empty text included: it is not read as no
        mob, so it and a build that names none keep their scopes."""
        await dispatcher.handle_callback(_CUSTOMIZE, _build_params("customize-agent:alpha-1"))
        await dispatcher.handle_callback(_CUSTOMIZE, _build_params("customize-agent:alpha-2", ""))
        assert customizer.mobs == [None, ""]
        assert sorted(dispatcher._scope_tools) == [
            "customize-agent:alpha-1",
            "customize-agent:alpha-2",
        ]


_BETA = {"identity": "agent:beta", "active_peers": [], "managed_edges": []}
_GAMMA = {"identity": "agent:gamma", "active_peers": [], "managed_edges": []}


class TestCustomizerScopesOfAMobCreatedUnderAUsedId:
    """Beside ``mob_id`` the customize callback names the mob's incarnation
    (``mob_incarnation``). A build that names another incarnation of a mob
    releases every scope kept for that mob id; scopes of other mobs and of
    the same incarnation stay; a gateway that names no incarnation is served
    as before."""

    @pytest.fixture
    def customizer(self):
        return _CountingCustomizer()

    @pytest.fixture
    def dispatcher(self, customizer):
        d = CallbackDispatcher()
        d.register_agent_customizer(customizer)
        return d

    @pytest.mark.asyncio
    async def test_a_build_of_another_incarnation_releases_every_scope_of_that_mob_id(
        self, dispatcher
    ):
        await dispatcher.handle_callback(
            _CUSTOMIZE, _build_params("customize-agent:alpha-1", "mob-a", mob_incarnation="one")
        )
        await dispatcher.handle_callback(
            _CUSTOMIZE,
            _build_params("customize-agent:beta-2", "mob-a", mob_incarnation="one", context=_BETA),
        )
        await dispatcher.handle_callback(
            _CUSTOMIZE,
            _build_params("customize-agent:gamma-3", "mob-a", mob_incarnation="two", context=_GAMMA),
        )
        for earlier in ("customize-agent:alpha-1", "customize-agent:beta-2"):
            with pytest.raises(ValueError, match="no handler registered"):
                await _lookup(dispatcher, earlier)
        assert await _lookup(dispatcher, "customize-agent:gamma-3") == {
            "content": {"registered_by_build": 3}
        }
        assert sorted(dispatcher._scope_tools) == ["customize-agent:gamma-3"]

    @pytest.mark.asyncio
    async def test_another_incarnation_of_one_mob_keeps_the_scopes_of_another_mob(
        self, dispatcher
    ):
        await dispatcher.handle_callback(
            _CUSTOMIZE, _build_params("customize-agent:alpha-1", "mob-a", mob_incarnation="one")
        )
        await dispatcher.handle_callback(
            _CUSTOMIZE, _build_params("customize-agent:alpha-2", "mob-b", mob_incarnation="other")
        )
        await dispatcher.handle_callback(
            _CUSTOMIZE,
            _build_params("customize-agent:beta-3", "mob-a", mob_incarnation="two", context=_BETA),
        )
        assert await _lookup(dispatcher, "customize-agent:alpha-2") == {
            "content": {"registered_by_build": 2}
        }
        assert sorted(dispatcher._scope_tools) == [
            "customize-agent:alpha-2",
            "customize-agent:beta-3",
        ]

    @pytest.mark.asyncio
    async def test_builds_of_one_incarnation_keep_each_others_scopes(self, dispatcher):
        """As before: one live scope per member of the mob, newest wins."""
        await dispatcher.handle_callback(
            _CUSTOMIZE, _build_params("customize-agent:alpha-1", "mob-a", mob_incarnation="one")
        )
        await dispatcher.handle_callback(
            _CUSTOMIZE,
            _build_params("customize-agent:beta-2", "mob-a", mob_incarnation="one", context=_BETA),
        )
        await dispatcher.handle_callback(
            _CUSTOMIZE, _build_params("customize-agent:alpha-3", "mob-a", mob_incarnation="one")
        )
        assert sorted(dispatcher._scope_tools) == [
            "customize-agent:alpha-3",
            "customize-agent:beta-2",
        ]

    @pytest.mark.asyncio
    async def test_a_gateway_that_names_no_incarnation_releases_the_members_own_scope_only(
        self, dispatcher
    ):
        """A gateway of an earlier release names a mob and no incarnation,
        before and after a build that named one: nothing but the member's
        own earlier scope is released."""
        await dispatcher.handle_callback(_CUSTOMIZE, _build_params("customize-agent:alpha-1", "mob-a"))
        await dispatcher.handle_callback(
            _CUSTOMIZE,
            _build_params("customize-agent:beta-2", "mob-a", mob_incarnation="one", context=_BETA),
        )
        await dispatcher.handle_callback(
            _CUSTOMIZE, _build_params("customize-agent:gamma-3", "mob-a", context=_GAMMA)
        )
        assert sorted(dispatcher._scope_tools) == [
            "customize-agent:alpha-1",
            "customize-agent:beta-2",
            "customize-agent:gamma-3",
        ]

    @pytest.mark.asyncio
    @pytest.mark.parametrize("not_text", [7, True, ["one"], {"id": "one"}, ""])
    async def test_an_incarnation_that_is_no_text_is_refused_and_releases_nothing(
        self, dispatcher, customizer, not_text
    ):
        """A ``mob_incarnation`` that is present and is not a non-empty text
        is never read as "none": the callback is refused, the customizer is
        not asked, and the earlier build keeps its scope."""
        await dispatcher.handle_callback(
            _CUSTOMIZE, _build_params("customize-agent:alpha-1", "mob-a", mob_incarnation="one")
        )
        with pytest.raises(ContractMismatchError):
            await dispatcher.handle_callback(
                _CUSTOMIZE,
                _build_params(
                    "customize-agent:beta-2", "mob-a", mob_incarnation=not_text, context=_BETA
                ),
            )
        assert customizer.builds == 1
        assert sorted(dispatcher._scope_tools) == ["customize-agent:alpha-1"]

    @pytest.mark.asyncio
    async def test_a_build_that_registers_no_tool_releases_the_earlier_mobs_scopes_too(self):
        """The release follows from what the gateway names, whatever the
        customizer does with the build."""

        class _OnlyAlpha:
            async def customize_build(self, context, spec, draft) -> None:
                if context.identity == "agent:alpha":
                    draft.register_tool("lookup", lambda args: {"ok": True})

        d = CallbackDispatcher()
        d.register_agent_customizer(_OnlyAlpha())
        await d.handle_callback(
            _CUSTOMIZE, _build_params("customize-agent:alpha-1", "mob-a", mob_incarnation="one")
        )
        await d.handle_callback(
            _CUSTOMIZE,
            _build_params("customize-agent:beta-2", "mob-a", mob_incarnation="two", context=_BETA),
        )
        assert sorted(d._scope_tools) == []

    @pytest.mark.asyncio
    async def test_a_build_that_ends_after_another_incarnation_was_named_registers_no_scope(self):
        """A build of the earlier mob is still with the customizer when a
        build of the later mob arrives: when it ends, its tools are not
        registered, so the later mob's scopes are the only ones kept."""
        asked = asyncio.Event()
        held = asyncio.Event()

        class _HeldForAlpha:
            async def customize_build(self, context, spec, draft) -> None:
                if context.identity == "agent:alpha":
                    asked.set()
                    await held.wait()
                draft.register_tool("lookup", lambda args: {"ok": True})

        d = CallbackDispatcher()
        d.register_agent_customizer(_HeldForAlpha())
        earlier = asyncio.ensure_future(
            d.handle_callback(
                _CUSTOMIZE,
                _build_params("customize-agent:alpha-1", "mob-a", mob_incarnation="one"),
            )
        )
        await asked.wait()
        await d.handle_callback(
            _CUSTOMIZE,
            _build_params("customize-agent:beta-2", "mob-a", mob_incarnation="two", context=_BETA),
        )
        held.set()
        await earlier
        assert sorted(d._scope_tools) == ["customize-agent:beta-2"]

    @pytest.mark.asyncio
    async def test_a_build_of_an_earlier_incarnation_handled_late_releases_and_registers_nothing(
        self, dispatcher, customizer
    ):
        """A build of the earlier mob that is handled after a build of the
        later one is of a mob that is gone: the later mob's scopes stay and
        the late build's tools are not registered. The customizer is still
        asked and its draft is answered. The later mob's next build is kept
        beside its others."""
        await dispatcher.handle_callback(
            _CUSTOMIZE, _build_params("customize-agent:alpha-1", "mob-a", mob_incarnation="one")
        )
        await dispatcher.handle_callback(
            _CUSTOMIZE,
            _build_params("customize-agent:beta-2", "mob-a", mob_incarnation="two", context=_BETA),
        )
        late = await dispatcher.handle_callback(
            _CUSTOMIZE,
            _build_params("customize-agent:gamma-3", "mob-a", mob_incarnation="one", context=_GAMMA),
        )
        assert customizer.builds == 3
        assert [tool["name"] for tool in late["external_tools"]] == ["lookup"]
        assert await _lookup(dispatcher, "customize-agent:beta-2") == {
            "content": {"registered_by_build": 2}
        }
        assert sorted(dispatcher._scope_tools) == ["customize-agent:beta-2"]
        await dispatcher.handle_callback(
            _CUSTOMIZE, _build_params("customize-agent:alpha-4", "mob-a", mob_incarnation="two")
        )
        assert sorted(dispatcher._scope_tools) == [
            "customize-agent:alpha-4",
            "customize-agent:beta-2",
        ]

    @pytest.mark.asyncio
    async def test_the_transport_hands_builds_on_in_the_order_they_arrived(
        self, dispatcher, monkeypatch
    ):
        """A destroy does not wait for the answer to a build it aborted, so
        the first build of the destroyed mob can arrive just before the
        first build of the mob created under its id. The transport hands
        the two on in the order they arrived, however slow the hand-off of
        the first one is: the later mob keeps its scopes, and its next
        build registers its tools."""
        transport = PersistentTransport("unused")
        process = MagicMock()
        answers: queue.Queue = queue.Queue()
        process.stdin.write.side_effect = lambda data: answers.put(json.loads(data))
        transport._process = process
        transport._loop = asyncio.get_running_loop()
        transport.set_callback_handler(dispatcher.handle_callback)

        hand_on = asyncio.run_coroutine_threadsafe
        turns: list[int] = []
        turns_lock = threading.Lock()
        first_handing_on = threading.Event()
        second_handed_on = threading.Event()

        def slow_first_hand_on(coroutine, loop):
            # The first callback's hand-off waits for the second's (at most
            # a second), as a thread that is slow to run would.
            with turns_lock:
                turn = len(turns)
                turns.append(turn)
            if turn == 0:
                first_handing_on.set()
                second_handed_on.wait(1.0)
                return hand_on(coroutine, loop)
            future = hand_on(coroutine, loop)
            second_handed_on.set()
            return future

        monkeypatch.setattr(
            "meerkat_meta._transport.asyncio.run_coroutine_threadsafe", slow_first_hand_on
        )

        def line(callback_id, params):
            return json.dumps(
                {"jsonrpc": "2.0", "id": callback_id, "method": _CUSTOMIZE, "params": params}
            ).encode() + b"\n"

        destroyed = line(
            "cb-1", _build_params("customize-agent:alpha-1", "mob-a", mob_incarnation="one")
        )
        created = line(
            "cb-2",
            _build_params("customize-agent:beta-2", "mob-a", mob_incarnation="two", context=_BETA),
        )

        def read():
            transport._read_line(destroyed, process=process)
            first_handing_on.wait(5)
            transport._read_line(created, process=process)

        reader = threading.Thread(target=read, daemon=True)
        reader.start()
        await asyncio.to_thread(reader.join, 5)
        answered = sorted(
            [(await asyncio.to_thread(answers.get, True, 5))["id"] for _ in range(2)]
        )
        assert answered == ["cb-1", "cb-2"]

        # The live mob's next build: its tools are registered beside the
        # scope of its first build, and the destroyed mob's scope is gone.
        await dispatcher.handle_callback(
            _CUSTOMIZE,
            _build_params("customize-agent:gamma-3", "mob-a", mob_incarnation="two", context=_GAMMA),
        )
        assert sorted(dispatcher._scope_tools) == [
            "customize-agent:beta-2",
            "customize-agent:gamma-3",
        ]
        assert await _lookup(dispatcher, "customize-agent:gamma-3") == {
            "content": {"registered_by_build": 3}
        }
