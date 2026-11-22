"""One agent name in two mobs of a host reaches only its own mob's customizer
tools, against a real gateway.

``test_customizer_tools.py`` pins on a scripted transport that the SDK keeps
the tool handlers a customizer registers per mob and member name. These rows
run the shipping path, Python SDK -> rpc_gateway -> Meta: the SDK host's
customizer gives every agent it builds one tool under the same name, and the
handler it registers keeps the mob the build was for. An agent of one name is
created in the launch's mob and in a mob the host creates beside it; each is
given a turn in which the model calls the tool, and the call of each mob's
agent is served by the handler of ITS mob's build, both ways. A reset in one
mob builds that mob's agent again and leaves the other mob's agent with its
tool. The first row is the launch's mob alone, with no second mob: what a
host of one mob has always had.

No provider key is used: the mobs' profiles run on the self-hosted model
route with no credential, against a local chat-completions fake that answers
a turn whose text carries ``CALL-TOOL token=<t>`` with one call of the
customizer's tool, once for each token, and every other request with fixed
text.

The rows are skipped without a built gateway. The two-mob row needs a
gateway that serves a mob beside its launch mob and that mob's long-term
agents.

Run:
    scripts/repo-cargo build -p meerkat-meta --bin rpc_gateway
    PYTHONPATH=sdk/python python3 -m pytest \
        sdk/python/tests/test_customizer_two_mobs_real_gateway.py -q
"""

from __future__ import annotations

import json
import os
import re
from pathlib import Path

import pytest

from meerkat_meta import AgentSpec, IdentityDone, Meta
from meerkat_meta.identity_first_models import DurableAgentSpec

from .test_identity_family_real_gateway import _HOST_MAY_CHANGE_AGENTS
from .test_wire_cross_mob_real_gateways import _GATEWAY_BIN, _MODEL, _FakeModel, _config_toml

pytestmark = [
    pytest.mark.skipif(
        not (_GATEWAY_BIN and os.path.isfile(_GATEWAY_BIN)),
        reason="META_GATEWAY_BIN not set to a built rpc_gateway",
    ),
    pytest.mark.asyncio,
    pytest.mark.timeout(240),
]

_LAUNCH_MOB = "customizer-mobs-launch"
_SECOND_MOB = "customizer-mobs-second"
_AGENT = "scribe"
_TOOL = "lookup_record"
_CALL = re.compile(r"CALL-TOOL token=([a-z0-9-]+)")


def _mob_toml(mob_id: str) -> str:
    return f"""\
[mob]
id = "{mob_id}"

[profiles.writer]
model = "{_MODEL}"
provider = "self_hosted"
self_hosted_server_id = "local"
external_addressable = true
runtime_mode = "turn_driven"

[profiles.writer.tools]
comms = true
"""


class _ToolCallingModel(_FakeModel):
    """The cross-mob rows' fake, scripted to call the customizer's tool.

    A request whose messages carry ``CALL-TOOL token=<t>`` is answered with
    one call of ``_TOOL`` carrying the token, once for each token; every
    other request is answered with fixed text. ``offered`` keeps, for each
    request, whether ``_TOOL`` was among the tools the model was offered.
    """

    def __init__(self) -> None:
        self.offered: list[bool] = []
        super().__init__()

    def answer(self, body: dict) -> list[dict]:
        text = json.dumps(body.get("messages", []))
        names = [tool.get("function", {}).get("name") for tool in body.get("tools") or []]
        tokens = _CALL.findall(text)
        with self._lock:
            self.offered.append(_TOOL in names)
            if tokens and tokens[-1] not in self._sent:
                token = tokens[-1]
                self._sent.add(token)
                delta = {
                    "role": "assistant",
                    "tool_calls": [
                        {
                            "index": 0,
                            "id": f"call-{token}",
                            "type": "function",
                            "function": {
                                "name": _TOOL,
                                "arguments": json.dumps({"token": token}),
                            },
                        }
                    ],
                }
                finish = "tool_calls"
            else:
                self.requests.append(text)
                delta = {"role": "assistant", "content": "observed"}
                finish = "stop"
        base = {"id": "fake", "object": "chat.completion.chunk", "created": 1, "model": _MODEL}
        return [
            {**base, "choices": [{"index": 0, "delta": delta, "finish_reason": None}]},
            {**base, "choices": [{"index": 0, "delta": {}, "finish_reason": finish}]},
            {
                **base,
                "choices": [],
                "usage": {"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2},
            },
        ]


class _NoDeclaredAgents:
    """The host's configuration declares no agent: each one is created."""

    async def roster(self, context: dict) -> list[DurableAgentSpec]:
        return []


class _MobToolCustomizer:
    """Gives every agent it builds ``_TOOL``, served by a handler that keeps
    the mob the build was for, and records what each callback names."""

    def __init__(self) -> None:
        self.built_in: list = []
        self.created_in: list = []
        self.created_for: dict[str, object] = {}
        self.served: list[tuple[object, str]] = []

    async def customize_build(self, context, spec, draft) -> None:
        del spec
        mob = context.mob_id
        self.built_in.append(mob)

        def serve(args, mob=mob):
            self.served.append((mob, args.get("token")))
            return {"record": "found", "mob": mob}

        draft.register_tool(
            _TOOL,
            serve,
            description="Look up a record.",
            input_schema={"type": "object", "properties": {"token": {"type": "string"}}},
        )

    async def after_create(self, identity, session_id, context) -> None:
        del identity
        self.created_in.append(context.mob_id)
        self.created_for[str(session_id)] = context.mob_id


async def _gateway(tmp_path: Path, fake: _ToolCallingModel, customizer: _MobToolCustomizer):
    (tmp_path / "mob.toml").write_text(_mob_toml(_LAUNCH_MOB))
    (tmp_path / "config.toml").write_text(_config_toml(fake.base_url))
    return await (
        Meta.builder()
        .gateway(_GATEWAY_BIN)
        .mob(str(tmp_path / "mob.toml"))
        .meerkat_config(tmp_path / "config.toml")
        .persistent_state(str(tmp_path / "state"))
        .roster(_NoDeclaredAgents())
        .agent_customizer(customizer)
        .identity_creation(_HOST_MAY_CHANGE_AGENTS)
        .build()
    )


async def _create_the_agent(handle, mob: str) -> None:
    outcome = await handle.identities.create(
        [
            AgentSpec(
                name=_AGENT,
                profile="writer",
                instructions=[f"You are the scribe of the mob {mob}."],
            )
        ],
        request_id="create-scribe",
    )
    assert isinstance(outcome, IdentityDone), (mob, outcome)


async def _call_the_tool(handle, token: str) -> None:
    """Give the agent a turn in which the model calls the customizer's tool."""
    text = await handle.agent(_AGENT).send_and_wait(f"CALL-TOOL token={token}", timeout=90)
    assert text == "observed", (token, text)


async def test_a_launch_mobs_agent_reaches_its_customizer_tool_with_no_second_mob(
    tmp_path: Path,
) -> None:
    """A host of one mob: the agent's build and its ``after_create`` name
    the launch's mob, the model is offered the customizer's tool, and the
    agent's call of it is served by the handler of that build."""
    fake = _ToolCallingModel()
    customizer = _MobToolCustomizer()
    runtime = await _gateway(tmp_path, fake, customizer)
    try:
        handle = runtime.mob_handle()
        await _create_the_agent(handle, _LAUNCH_MOB)
        await _call_the_tool(handle, "launch-1")
        assert customizer.served == [(_LAUNCH_MOB, "launch-1")]
        assert customizer.built_in and set(customizer.built_in) == {_LAUNCH_MOB}
        assert customizer.created_in and set(customizer.created_in) == {_LAUNCH_MOB}
        assert any(fake.offered), "the model was offered the customizer's tool"
    finally:
        await runtime.shutdown()
        fake.close()


async def test_one_agent_name_in_two_mobs_reaches_only_its_own_mobs_customizer_tool(
    tmp_path: Path,
) -> None:
    """Two mobs, one agent name, one tool name: each agent's call is served
    by the handler of its own mob's build, both ways, and a reset in one mob
    leaves the other mob's agent with its tool."""
    fake = _ToolCallingModel()
    customizer = _MobToolCustomizer()
    runtime = await _gateway(tmp_path, fake, customizer)
    try:
        entry = await runtime.mobs.create(_mob_toml(_SECOND_MOB))
        assert (entry.mob_id, entry.default) == (_SECOND_MOB, False)
        handles = {_LAUNCH_MOB: runtime.mob_handle(), _SECOND_MOB: runtime.mob(_SECOND_MOB)}
        for mob, handle in handles.items():
            await _create_the_agent(handle, mob)

        # An agent is built at its first turn, not at its create. Both ways:
        # each agent's build names the agent's own mob, and its call reaches
        # the handler of its own mob.
        assert customizer.built_in == [], "a create builds nothing"
        await _call_the_tool(handles[_LAUNCH_MOB], "launch-1")
        assert customizer.built_in and set(customizer.built_in) == {_LAUNCH_MOB}
        builds = len(customizer.built_in)
        await _call_the_tool(handles[_SECOND_MOB], "second-1")
        assert customizer.built_in[builds:], "the second mob's agent was built"
        assert set(customizer.built_in[builds:]) == {_SECOND_MOB}
        assert customizer.served == [(_LAUNCH_MOB, "launch-1"), (_SECOND_MOB, "second-1")]

        # `after_create` is told for each agent's session and names the mob
        # of the agent whose session it is.
        for mob, handle in handles.items():
            session = (await handle.identity_resolved_tools_detail(_AGENT)).session_id
            assert customizer.created_for.get(session) == mob, (mob, customizer.created_for)
        assert set(customizer.created_in) == {_LAUNCH_MOB, _SECOND_MOB}

        # A reset builds the second mob's agent again. The launch mob's
        # agent keeps its tool, and the rebuilt agent has its own again.
        builds_before = len(customizer.built_in)
        await handles[_SECOND_MOB].reset(_AGENT)
        await _call_the_tool(handles[_LAUNCH_MOB], "launch-2")
        await _call_the_tool(handles[_SECOND_MOB], "second-2")
        assert customizer.built_in[builds_before:], "the reset built the agent again"
        assert set(customizer.built_in[builds_before:]) == {_SECOND_MOB}
        assert customizer.served[2:] == [(_LAUNCH_MOB, "launch-2"), (_SECOND_MOB, "second-2")]
        assert any(fake.offered), "the model was offered the customizer's tool"
    finally:
        await runtime.shutdown()
        fake.close()
