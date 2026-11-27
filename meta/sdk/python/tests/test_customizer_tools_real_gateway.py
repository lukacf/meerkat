"""customize_build tools follow an identity member across a restart.

Each build of a member runs the host's customize_build right before it and
publishes the tools into the member's stable dispatcher, so the member is
never built without them. A member that is not materialized is answered
from its last recorded build (``tools_source``), without a wake; a member
never built yet answers ``no_build_recorded``.
"""
from __future__ import annotations

import os
from pathlib import Path

import pytest

from .gateway_test_support import gateway_transport_factory

from meerkat_meta.builder import Meta
from meerkat_meta.identity_first_models import DurableAgentSpec, IdentityBootstrapMode

_GATEWAY_BIN = os.environ.get("META_GATEWAY_BIN", "").strip()
_MOB = "customizer-tools-restart"
_IDENTITY = "domain:research"
_TOOL = "lookup_workspace"

_MOB_TOML = f"""\
[mob]
id = "{_MOB}"

# Turn-driven: the member owes no first turn, so nothing builds it until a
# wake (an autonomous member's fresh seat is built at start).
[profiles.domain]
model = "gpt-5.5"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.domain.tools]
comms = true
"""


class _Roster:
    async def roster(self, context):
        del context
        return [DurableAgentSpec(identity=_IDENTITY, profile="domain", addressability="addressable")]


class _ToolCustomizer:
    def __init__(self) -> None:
        self.builds = 0
        self.mobs: list = []

    async def customize_build(self, context, spec, draft) -> None:
        del spec
        self.builds += 1
        self.mobs.append(context.mob_id)
        draft.register_tool(
            _TOOL,
            lambda args: {"workspace": "ok", "echo": args},
            description="Look up the workspace record.",
            input_schema={"type": "object", "properties": {}},
        )


async def _build(mob_toml: Path, state_dir: Path, customizer: _ToolCustomizer):
    return await (
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .gateway_transport(gateway_transport_factory(_GATEWAY_BIN, state_dir))
        .mob(str(mob_toml))
        .persistent_state(str(state_dir))
        .roster(_Roster())
        .agent_customizer(customizer)
        .identity_bootstrap_mode(IdentityBootstrapMode.eager_materialize())
        .build()
    )


def _live_scope_serves_the_tool(runtime) -> bool:
    dispatcher = runtime._dispatcher
    # The live scope of the member: its mob and its name. The gateway's
    # customize callback names the launch mob.
    scope = dispatcher._customizer_scope_by_member.get((_MOB, _IDENTITY))
    return scope is not None and (scope, _TOOL) in dispatcher._tool_handlers


@pytest.mark.skipif(
    not (_GATEWAY_BIN and os.path.isfile(_GATEWAY_BIN)),
    reason="META_GATEWAY_BIN not set to a built rpc_gateway",
)
@pytest.mark.asyncio
async def test_customizer_tools_are_still_advertised_after_a_restart(tmp_path):
    """R121 and R109: the first boot seats the identity without building it,
    a warm builds it with its tools and records them, and after a restart the
    record answers them without a wake; the next build publishes them again."""
    mob_toml = tmp_path / "mob.toml"
    mob_toml.write_text(_MOB_TOML)
    state_dir = tmp_path / "state"

    first_customizer = _ToolCustomizer()
    first = await _build(mob_toml, state_dir, first_customizer)
    try:
        # The seat of the new identity built its draft with the host.
        assert first_customizer.builds >= 1
        # Every customize callback of the gateway names its launch mob.
        assert set(first_customizer.mobs) == {_MOB}
        # The seat's build registered its handlers under the launch mob and
        # the member name (before any build of the member).
        assert (_MOB, _IDENTITY) in first._dispatcher._customizer_scope_by_member
        handle = first.mob_handle()
        never_built = await handle.identity_resolved_tools_detail(_IDENTITY)
        assert never_built.tools_source == "no_build_recorded"
        assert never_built.session_id, "the session is returned without a wake"
        assert never_built.tools == []

        warmed = await handle.identity_warm(_IDENTITY)
        assert _TOOL in warmed.tools
        assert _TOOL in await handle.identity_resolved_tools(_IDENTITY)
        assert _live_scope_serves_the_tool(first)
    finally:
        await first.shutdown()

    # Same persistent state, new process: the member holds no work, so the
    # start materializes nothing and asks the host for no build.
    second_customizer = _ToolCustomizer()
    second = await _build(mob_toml, state_dir, second_customizer)
    try:
        assert second_customizer.builds == 0, "no member was built at start"
        handle = second.mob_handle()
        recorded = await handle.identity_resolved_tools_detail(_IDENTITY)
        assert recorded.tools_source == "recorded"
        assert _TOOL in recorded.tools, "the record answers the tools without a wake"
        assert second_customizer.builds == 0, "the read built nothing"

        await handle.identity_warm(_IDENTITY)
        assert second_customizer.builds >= 1, "the build ran customize_build"
        assert set(second_customizer.mobs) == {_MOB}
        assert _TOOL in await handle.identity_resolved_tools(_IDENTITY)
        # The advertised tool is served by the scope the latest
        # customize_build registered in this process.
        assert _live_scope_serves_the_tool(second)
    finally:
        await second.shutdown()


class _RecordingCustomizer(_ToolCustomizer):
    """Also records the mob each ``after_create`` is told."""

    def __init__(self) -> None:
        super().__init__()
        self.created_mobs: list = []

    async def after_create(self, identity, session_id, context) -> None:
        del identity, session_id
        self.created_mobs.append(context.mob_id)


class _RecordingTopology:
    """A topology provider that wires nothing and records the mob each
    ``compute_edges`` is told."""

    def __init__(self) -> None:
        self.mobs: list = []

    async def compute_edges(self, target_identities, context):
        del target_identities
        self.mobs.append(context.get("mob_id") if isinstance(context, dict) else None)
        return []


@pytest.mark.skipif(
    not (_GATEWAY_BIN and os.path.isfile(_GATEWAY_BIN)),
    reason="META_GATEWAY_BIN not set to a built rpc_gateway",
)
@pytest.mark.asyncio
async def test_the_member_callbacks_of_a_gateway_name_its_launch_mob(tmp_path):
    """The three callbacks that build a member name the gateway's launch
    mob: ``customize_build`` and ``after_create`` when the identity is
    created, ``compute_edges`` when the roster's edges are asked. Nothing is
    built for it (no wake), so the row needs no provider."""
    mob_toml = tmp_path / "mob.toml"
    mob_toml.write_text(_MOB_TOML)
    customizer = _RecordingCustomizer()
    topology = _RecordingTopology()
    runtime = await (
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .mob(str(mob_toml))
        .persistent_state(str(tmp_path / "state"))
        .roster(_Roster())
        .topology_provider(topology)
        .agent_customizer(customizer)
        .identity_bootstrap_mode(IdentityBootstrapMode.eager_materialize())
        .build()
    )
    try:
        assert customizer.mobs and set(customizer.mobs) == {_MOB}
        assert customizer.created_mobs and set(customizer.created_mobs) == {_MOB}
        assert topology.mobs and set(topology.mobs) == {_MOB}
    finally:
        await runtime.shutdown()
