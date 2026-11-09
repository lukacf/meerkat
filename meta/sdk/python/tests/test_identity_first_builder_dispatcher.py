"""TDD tests for callback dispatcher routing (REQ-45)."""
import pytest

from meerkat_meta.agent_builder import CallbackDispatcher
from meerkat_meta.identity_first_models import (
    AgentBuildContext,
    AgentBuildDraft,
    DurableAgentSpec,
    ManagedPeerEdge,
    RosterContext,
)


# ---------------------------------------------------------------------------
# REQ-45: Callback dispatcher — provider routing
# ---------------------------------------------------------------------------


class TestCallbackDispatcherRoster:
    @pytest.mark.asyncio
    async def test_roster_routed(self):
        class MockRoster:
            async def roster(self, context):
                return [
                    DurableAgentSpec(identity="a:main", profile="assistant"),
                ]

        d = CallbackDispatcher()
        d.register_roster_provider(MockRoster())

        result = await d.handle_callback("callback/roster_provider/roster", {
            "context": {},
        })
        assert len(result) == 1
        assert result[0]["identity"] == "a:main"

    @pytest.mark.asyncio
    async def test_roster_payload_carries_runtime_mode_override_and_initial_message(self):
        """The roster callback reply is what Rust deserializes into
        DurableAgentSpec; both per-identity knobs must be in it verbatim."""

        class MockRoster:
            async def roster(self, context):
                return [
                    DurableAgentSpec(
                        identity="archivist:main",
                        profile="archivist",
                        runtime_mode_override="turn_driven",
                        initial_message="Index yesterday's mail.",
                    ),
                    DurableAgentSpec(identity="worker:1", profile="worker"),
                ]

        d = CallbackDispatcher()
        d.register_roster_provider(MockRoster())

        payload = await d.handle_callback(
            "callback/roster_provider/roster", {"context": {}}
        )
        assert payload[0]["runtime_mode_override"] == "turn_driven"
        assert payload[0]["initial_message"] == "Index yesterday's mail."
        # An identity that sets neither sends neither (byte-compatible roster).
        assert "runtime_mode_override" not in payload[1]
        assert "initial_message" not in payload[1]

    @pytest.mark.asyncio
    async def test_roster_callback_receives_the_gateway_context(self):
        """Regression: the roster callback used to see an empty context.

        The gateway sends the Rust ``RosterContext`` under ``params["context"]``
        (same envelope as topology/customizer). The provider must receive the
        mob definition and previous identities from it, typed.
        """
        seen: list[RosterContext] = []

        class RecordingRoster:
            async def roster(self, context):
                seen.append(context)
                return [DurableAgentSpec(identity="a:main", profile="assistant")]

        d = CallbackDispatcher()
        d.register_roster_provider(RecordingRoster())

        await d.handle_callback("callback/roster_provider/roster", {
            "context": {
                "mob_definition": {
                    "id": "team",
                    "profiles": {"assistant": {"model": "gpt-5.5"}},
                },
                "previous_identities": ["a:main", "b:main"],
            },
        })
        assert len(seen) == 1
        context = seen[0]
        assert isinstance(context, RosterContext)
        assert context.mob_definition is not None
        assert context.mob_definition["id"] == "team"
        assert context.mob_definition["profiles"]["assistant"]["model"] == "gpt-5.5"
        assert context.previous_identities == ["a:main", "b:main"]

    @pytest.mark.asyncio
    async def test_roster_callback_tolerates_a_bare_context(self):
        """An older gateway that sends `{}` yields a typed context with defaults."""
        seen: list[RosterContext] = []

        class RecordingRoster:
            async def roster(self, context):
                seen.append(context)
                return []

        d = CallbackDispatcher()
        d.register_roster_provider(RecordingRoster())

        await d.handle_callback("callback/roster_provider/roster", {"context": {}})
        await d.handle_callback("callback/roster_provider/roster", {})
        assert [c.to_dict() for c in seen] == [
            {"mob_definition": None, "previous_identities": []},
            {"mob_definition": None, "previous_identities": []},
        ]


class TestCallbackDispatcherTopology:
    @pytest.mark.asyncio
    async def test_compute_edges_routed(self):
        class MockTopology:
            async def compute_edges(self, target_identities, context):
                return [ManagedPeerEdge(a="a:main", b="b:main")]

        d = CallbackDispatcher()
        d.register_topology_provider(MockTopology())

        result = await d.handle_callback(
            "callback/topology_provider/compute_edges",
            {
                "target_identities": ["a:main", "b:main"],
                "context": {"roster": []},
            },
        )
        assert len(result) == 1
        assert result[0] == {"a": "a:main", "b": "b:main"}


class TestCallbackDispatcherCustomizer:
    @pytest.mark.asyncio
    async def test_customize_build_routed(self):
        class MockCustomizer:
            async def customize_build(self, context, spec, draft):
                draft.model = "claude-sonnet-4-5"
                draft.system_prompt = "You are helpful."

        d = CallbackDispatcher()
        d.register_agent_customizer(MockCustomizer())

        result = await d.handle_callback(
            "callback/agent_customizer/customize_build",
            {
                "context": {
                    "identity": "a:main",
                    "active_peers": ["b:main"],
                    "managed_edges": [],
                },
                "spec": {
                    "identity": "a:main",
                    "profile": "assistant",
                },
                "draft": {
                    "model": None,
                    "system_prompt": None,
                    "additional_instructions": [],
                    "labels": {},
                    "app_context": None,
                    "external_tools": [],
                },
            },
        )
        # Returns the mutated draft
        assert result["model"] == "claude-sonnet-4-5"
        assert result["system_prompt"] == "You are helpful."

    @pytest.mark.asyncio
    async def test_after_create_routed(self):
        received = {}

        class MockCustomizer:
            async def customize_build(self, context, spec, draft):
                pass

            async def after_create(self, identity, session_id, context):
                received["identity"] = identity
                received["session_id"] = session_id

        d = CallbackDispatcher()
        d.register_agent_customizer(MockCustomizer())

        await d.handle_callback(
            "callback/agent_customizer/after_create",
            {
                "identity": "a:main",
                "session_id": "s-1",
                "context": {"model": "m1", "labels": {}, "system_prompt": None},
            },
        )
        assert received["identity"] == "a:main"
        assert received["session_id"] == "s-1"

    @pytest.mark.asyncio
    async def test_after_create_reads_the_context_the_gateway_sends(self):
        # The gateway sends the context's fields beside the identity and the
        # session, as it does for callback/after_create.
        received = {}

        class MockCustomizer:
            async def customize_build(self, context, spec, draft):
                pass

            async def after_create(self, identity, session_id, context):
                received["context"] = context

        d = CallbackDispatcher()
        d.register_agent_customizer(MockCustomizer())

        await d.handle_callback(
            "callback/agent_customizer/after_create",
            {
                "identity": "a:main",
                "session_id": "s-1",
                "model": "m1",
                "labels": {"team": "ops"},
                "system_prompt": "be brief",
            },
        )
        context = received["context"]
        assert context.model == "m1"
        assert context.labels == {"team": "ops"}
        assert context.system_prompt == "be brief"
