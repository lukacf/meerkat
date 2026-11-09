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

from meerkat_meta.builder import Meta
from meerkat_meta.identity_first_models import DurableAgentSpec, IdentityBootstrapMode

_GATEWAY_BIN = os.environ.get("META_GATEWAY_BIN", "").strip()
_IDENTITY = "domain:research"
_TOOL = "lookup_workspace"

_MOB_TOML = """\
[mob]
id = "customizer-tools-restart"

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

    async def customize_build(self, context, spec, draft) -> None:
        del context, spec
        self.builds += 1
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
        .mob(str(mob_toml))
        .persistent_state(str(state_dir))
        .roster(_Roster())
        .agent_customizer(customizer)
        .identity_bootstrap_mode(IdentityBootstrapMode.eager_materialize())
        .build()
    )


def _live_scope_serves_the_tool(runtime) -> bool:
    dispatcher = runtime._dispatcher
    scope = dispatcher._customizer_scope_by_identity.get(_IDENTITY)
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
        assert _TOOL in await handle.identity_resolved_tools(_IDENTITY)
        # The advertised tool is served by the scope the latest
        # customize_build registered in this process.
        assert _live_scope_serves_the_tool(second)
    finally:
        await second.shutdown()
