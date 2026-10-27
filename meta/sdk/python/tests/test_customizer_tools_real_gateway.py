"""customize_build tools follow an identity member across a restart (#563).

Before the fix the tools a host returned from ``customize_build`` rode only
the per-spawn overlay of the spawn that ran the customizer. A restart restore
rebuilt the member without them, the resumed materialization's spawn hit
"member already exists", and the adopt branch discarded the new tools: the
handlers stayed registered in this process, but the live session no longer
advertised the tools.
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

[profiles.domain]
model = "gpt-5.5"
external_addressable = true

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
    mob_toml = tmp_path / "mob.toml"
    mob_toml.write_text(_MOB_TOML)
    state_dir = tmp_path / "state"

    first_customizer = _ToolCustomizer()
    first = await _build(mob_toml, state_dir, first_customizer)
    try:
        assert first_customizer.builds >= 1
        assert _TOOL in await first.mob_handle().identity_resolved_tools(_IDENTITY)
        assert _live_scope_serves_the_tool(first)
    finally:
        await first.shutdown()

    # Same persistent state, new process: meerkat restores the member from its
    # stored roster entry, the materialization re-runs customize_build, its
    # spawn finds the member already there and adopts it.
    second_customizer = _ToolCustomizer()
    second = await _build(mob_toml, state_dir, second_customizer)
    try:
        assert second_customizer.builds >= 1, "the restore re-ran customize_build"
        assert _TOOL in await second.mob_handle().identity_resolved_tools(_IDENTITY), (
            "the restored and adopted member must still advertise its customizer tools"
        )
        # The advertised tool is served by the scope the latest
        # customize_build registered in this process.
        assert _live_scope_serves_the_tool(second)
    finally:
        await second.shutdown()
