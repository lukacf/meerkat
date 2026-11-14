"""Long-term agents created and changed at run time (Meta 0.9.15 and later).

One program, five steps, each printed as it happens:

1. launch with a policy that says which run-time changes the host may make;
2. adopt `scout`, the agent this program's own roster declares;
3. read that operation back by its request id;
4. create `editor`, update it, archive it, bring it back and remove it;
5. show what a refusal looks like and what it tells you to do.

Run it twice on the same state directory. The second run sends the same
request ids, so every change is answered from the record of the first run and
nothing is changed twice: `editor` is not created a second time. That is the
point of naming every change with a request id. An answer from the record
states less than the first one did (no seat, no instruction outcome).

No provider key is needed: the program starts no agent run.
"""

import asyncio
import os
import sys
from pathlib import Path

from meerkat_meta import (
    AgentOutcome,
    AgentSet,
    AgentSpec,
    IdentitiesHandle,
    IdentityChangeResult,
    IdentityDone,
    IdentityPendingDecision,
    IdentityRefused,
    IdentityRefusedError,
    Meta,
)
from meerkat_meta.identity_first_models import DurableAgentSpec

HERE = Path(__file__).resolve().parent

# What this host may change at run time. A change no rule matches is refused
# (`change_denied`), so every change the program makes is named here. Nothing
# in this policy lets an agent change other agents: every rule is for the
# host, on the RPC surface this SDK speaks.
HOST = {"kind": "host", "surface": "rpc"}
POLICY = {
    "enabled": True,
    "approval": {
        "rules": [
            {
                "change": ["adopt", "create", "descriptive", "archive", "remove"],
                "requester": HOST,
                "outcome": {"kind": "allow"},
            }
        ]
    },
    # `editor` names `scout` as a peer; peers are denied unless allowed here.
    "peers": {"default": "allow"},
}


class Roster:
    """The agents this program declares in configuration: one, `scout`."""

    async def roster(self, context):
        return [
            DurableAgentSpec(
                identity="scout",
                profile="researcher",
                display_name="Scout",
                additional_instructions=["Find sources and report them briefly."],
            )
        ]


def say(step: str, text: str) -> None:
    print(f"[{step}] {text}", flush=True)


async def adopt_scout(identities: IdentitiesHandle) -> None:
    record = await identities.get("scout")
    say("2", f"scout: spec digest {record.spec_digest[:19]}…")
    try:
        outcome = await identities.adopt(
            "scout",
            expected_spec_digest=record.spec_digest,
            request_id="adopt-scout-1",
        )
    except IdentityRefusedError as refused:
        if refused.details.get("reason") == "nothing_to_adopt":
            say("2", "scout was adopted by an earlier run; nothing to do")
            return
        if refused.retryable:
            say("2", f"not yet ({refused.kind}); the same request can be sent again later")
            return
        raise
    say("2", f"adopt answered {outcome.outcome}")


async def read_operation(identities: IdentitiesHandle) -> None:
    # An answer is one of several classes; `isinstance` tells them apart for
    # the program and for a type checker alike.
    state = await identities.operation(request_id="adopt-scout-1")
    if isinstance(state, IdentityDone):
        say("3", "the adoption is committed")
    elif isinstance(state, IdentityPendingDecision):
        say("3", f"the adoption waits for a decision until {state.deadline_ms}")
    elif isinstance(state, IdentityRefused):
        say("3", f"the adoption was refused: {state.refusal.kind}")
    else:
        say("3", f"operation state: {state.outcome}")


def done_agent(result: IdentityChangeResult, request_id: str) -> AgentOutcome | None:
    """The agent of a change that is done, or None with a line saying why not."""
    if not isinstance(result, IdentityDone):
        say("4", f"{request_id} answered {result.outcome}; stopping here")
        return None
    return result.agents[0] if result.agents else None


async def editor_lifecycle(identities: IdentitiesHandle) -> None:
    try:
        created = await identities.create(
            [
                AgentSpec(
                    name="editor",
                    profile="writer",
                    display_name="Editor",
                    instructions=["Edit for clarity."],
                    peers=["scout"],
                )
            ],
            request_id="create-editor-1",
        )
    except IdentityRefusedError as refused:
        # A refusal says what was wrong and where. With a request id that
        # was never used, for instance, a removed agent's name is refused
        # (`name_taken`): it is never available again. Say so and stop.
        say("4", f"create refused: {refused.kind} at {refused.field}: {refused.detail}")
        return
    # The policy above allows this change, so it is done at once. Under a
    # rule that asks a person first, the answer would be a pending decision
    # with no agent in it yet; this example stops there.
    agent = done_agent(created, "create-editor-1")
    if agent is None:
        return
    say("4", f"created editor: state {agent.state}, seat {agent.seat}")

    # Each change names the digest it expects and answers the next one.
    digest = agent.spec_digest
    if digest is None:
        return
    agent = done_agent(
        await identities.update(
            "editor",
            expected_spec_digest=digest,
            request_id="update-editor-1",
            set=AgentSet(instructions=["Edit for clarity.", "Keep the author's voice."]),
        ),
        "update-editor-1",
    )
    if agent is None:
        return
    how = agent.instructions.kind if agent.instructions else "not stated"
    say("4", f"updated editor's instructions: {how}")

    for change, call, request_id in (
        ("archived", identities.archive, "archive-editor-1"),
        ("unarchived", identities.unarchive, "unarchive-editor-1"),
        ("removed", identities.remove, "remove-editor-1"),
    ):
        digest = agent.spec_digest or digest
        agent = done_agent(
            await call("editor", expected_spec_digest=digest, request_id=request_id),
            request_id,
        )
        if agent is None:
            return
        say("4", f"{change} editor: state {agent.state}")


async def show_refusal(identities: IdentitiesHandle) -> None:
    # `scout` exists; asking to create it again is refused, and the refusal
    # says which field of the request was wrong.
    try:
        await identities.create(
            [AgentSpec(name="scout", profile="researcher")],
            request_id="create-scout-again-1",
        )
    except IdentityRefusedError as refused:
        say("5", f"refused as expected: {refused.kind} at {refused.field}: {refused.detail}")
        say("5", f"retryable: {refused.retryable}")
    else:
        say("5", "unexpected: creating scout again was not refused")


async def main() -> int:
    gateway = os.environ.get("META_GATEWAY_BIN")
    if not gateway:
        print("Set META_GATEWAY_BIN to the rpc_gateway binary of Meta 0.9.15 or later.", file=sys.stderr)
        return 2
    state_dir = Path(os.environ.get("EXAMPLE_STATE_DIR", HERE / ".state"))
    state_dir.mkdir(parents=True, exist_ok=True)

    runtime = await (
        Meta.builder()
        .gateway(gateway)
        .mob(str(HERE / "mob.toml"))
        .persistent_state(str(state_dir))
        .roster(Roster())
        .identity_creation(POLICY)
        .build()
    )
    say("1", "runtime started with a policy that allows the host to adopt, create, describe, archive and remove")
    try:
        identities = runtime.identities
        await adopt_scout(identities)
        await read_operation(identities)
        await editor_lifecycle(identities)
        await show_refusal(identities)
        page = await identities.list(include_archived=True)
        say("end", "agents now: " + ", ".join(f"{a.name} ({a.readiness})" for a in page.agents))
    finally:
        await runtime.shutdown()
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
