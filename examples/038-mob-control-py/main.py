#!/usr/bin/env python3
"""038 — Drive a Mob from the Python SDK

An application creates a small mob over `rkat-rpc`, gives it work, follows
what the members do and stops work it no longer wants.

What you will learn:
- Creating a mob, spawning members and wiring them as peers
- Sending work to the member that is the entry point of the application
- Following every member on one event stream and reading member status
- Stopping one run with `mob.stop_member_run()` (the stop button)
- Clearing a member with `client.mob_cancel_all_work()` (its run and its queue)

Run:
    ANTHROPIC_API_KEY=sk-... python3 main.py
"""

import asyncio
import re

from meerkat import (
    MeerkatClient,
    RunCompleted,
    RunFailed,
    RunStarted,
    TextDelta,
    ToolCallRequested,
)

MODEL = "claude-sonnet-5-5"
MOB_ID = "newsdesk"
EDITOR = "editor-1"
RESEARCHER = "researcher-1"

# Two profiles. Only the editor is `external_addressable`: it is the one
# member the application may send work to. Every profile needs comms.
DEFINITION = {
    "id": MOB_ID,
    "profiles": {
        "editor": {
            "model": MODEL,
            "peer_description": "Takes requests from the application and writes the answer",
            "external_addressable": True,
            "tools": {"comms": True},
        },
        "researcher": {
            "model": MODEL,
            "peer_description": "Looks up facts for the editor",
            "tools": {"comms": True},
        },
    },
}

EDITOR_BRIEF = (
    "You are the editor of a small news desk. Requests arrive from the "
    "application. When a request needs facts, send your peer researcher-1 a "
    "message, and write your answer once its reply has arrived. "
    "Reply READY now and wait for the first request."
)
RESEARCHER_BRIEF = (
    "You are the researcher of a small news desk. When editor-1 sends you a "
    "question, answer it with one message to editor-1. "
    "Reply READY now and wait."
)
LONG_JOB = "Write a 600-word feature on the history of lighthouses. Do not message anyone."


class Desk:
    """What the application has seen on the mob event stream."""

    def __init__(self) -> None:
        self.events: list[tuple[str, object]] = []
        self._changed = asyncio.Condition()

    async def follow(self, subscription) -> None:
        """Print a line for each event that matters and keep all of them."""
        async for item in subscription:
            member, event = item.source, item.envelope.payload
            match event:
                case RunStarted(identity=identity):
                    print(f"  [{member}] run {identity['run_id']} started")
                case ToolCallRequested(name=name) if name.startswith("send_"):
                    print(f"  [{member}] calls {name}")
                case RunCompleted(result=result):
                    print(f"  [{member}] run completed: {result.strip()[:160]!r}")
                case RunFailed(error_class=error_class):
                    print(f"  [{member}] run ended: {error_class}")
            async with self._changed:
                self.events.append((member, event))
                self._changed.notify_all()

    async def next(self, member: str, kind: type, *, after: int = 0, timeout: float = 120):
        """Wait for the next event of one kind from one member."""

        def found():
            for seen_member, event in self.events[after:]:
                if seen_member == member and isinstance(event, kind):
                    return event
            return None

        async with self._changed:
            await asyncio.wait_for(self._changed.wait_for(found), timeout)
            return found()


async def wait_until_idle(mob, members: list[str], *, timeout: float = 180) -> None:
    """Return once every member has been idle on two reads in a row.

    A script has to end; an application with a window keeps the stream open
    and shows what arrives instead.
    """
    quiet = 0
    for _ in range(int(timeout)):
        states = [(await mob.member_status(member)).get("progress", {}) for member in members]
        idle = all(state.get("run_state") == "idle" for state in states)
        quiet = quiet + 1 if idle else 0
        if quiet == 2:
            return
        await asyncio.sleep(1)
    raise TimeoutError("the mob did not become idle")


async def main() -> None:
    client = MeerkatClient()
    await client.connect()
    try:
        # ── 1. Create, spawn, wire ──
        print("=== 1. Create the mob ===")
        mob = await client.create_mob(definition=DEFINITION)
        editor = await mob.spawn(profile="editor", agent_identity=EDITOR, initial_message=EDITOR_BRIEF)
        await mob.spawn(profile="researcher", agent_identity=RESEARCHER, initial_message=RESEARCHER_BRIEF)
        await mob.wire(EDITOR, RESEARCHER)
        # A member runs one first turn on its initial message when it is spawned.
        await mob.wait_for_kickoff_complete(timeout_ms=120_000)
        for member in await mob.members():
            print(f"  {member['agent_identity']}: {member['profile']}, wired to {member.get('wired_to', [])}")

        # What a member was told: the first message of its transcript is its
        # system prompt, with the skills the runtime gave it.
        history = await mob.member_history(EDITOR, from_index=0, limit=1)
        system_prompt = str(history.page.messages[0].get("content", ""))
        skills = sorted(set(re.findall(r'skill_name="([^"]+)"', system_prompt)))
        print(f"  {EDITOR} system prompt: {len(system_prompt)} characters, skills {skills}")

        desk = Desk()
        subscription = await mob.subscribe_events()
        follower = asyncio.create_task(desk.follow(subscription))
        await wait_until_idle(mob, [EDITOR, RESEARCHER])

        # ── 2. Send work and read the result ──
        print("\n=== 2. Send work ===")
        mark = len(desk.events)
        await mob.member(EDITOR).send(
            "In two sentences, say why lighthouses were built. Get two facts from the researcher first."
        )
        await desk.next(EDITOR, RunStarted, after=mark)
        await wait_until_idle(mob, [EDITOR, RESEARCHER])
        status = await mob.member_status(EDITOR)
        print(f"  {EDITOR} is {status['status']}, {status['tokens_used']} tokens used. Its last answer:")
        print(f"  {status.get('output_preview', '').strip()}")

        # ── 3. Stop one run ──
        print("\n=== 3. Stop one run ===")
        mark = len(desk.events)
        await mob.member(EDITOR).send(LONG_JOB)
        started = await desk.next(EDITOR, RunStarted, after=mark)
        run_id = started.identity["run_id"]
        await desk.next(EDITOR, TextDelta, after=mark)  # the member is writing
        stopped = await mob.stop_member_run(EDITOR, run_id, reason="user pressed stop")
        print(f"  stop: {stopped.receipt['outcome']}, inputs ended: {stopped.receipt.get('contributors', [])}")
        # The run id fences the stop: a run that is no longer current is left alone.
        again = await mob.stop_member_run(EDITOR, run_id, reason="pressed twice")
        print(f"  the same stop again: {again.receipt['outcome']}")

        # ── 4. Cancel everything a member holds ──
        print("\n=== 4. Cancel all work of a member ===")
        mark = len(desk.events)
        await mob.member(EDITOR).send(LONG_JOB)
        await desk.next(EDITOR, TextDelta, after=mark)
        await mob.member(EDITOR).send("Queued behind the feature: reply with the word ONE.")
        await mob.member(EDITOR).send("Queued behind the feature: reply with the word TWO.")
        result = await client.mob_cancel_all_work(editor["member_ref"])
        print(f"  cancel_all_work: {result}")
        # The run ends at its next boundary, which can be the end of the model
        # response it is in; the two queued inputs are dropped with it.
        await desk.next(EDITOR, RunFailed, after=mark)

        # ── 5. The member is still there ──
        print("\n=== 5. The member takes new work ===")
        await mob.member(EDITOR).send("Reply with the single word PONG.")
        # Had ONE and TWO still been queued, they would run before this.
        done = await desk.next(EDITOR, RunCompleted, after=mark)
        print(f"  {EDITOR} answered: {done.result.strip()!r}")

        follower.cancel()
        await subscription.close()
        await client.mob_destroy(MOB_ID)
        print("\nMob destroyed.")
    finally:
        await client.close()


if __name__ == "__main__":
    asyncio.run(main())
