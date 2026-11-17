"""A long-lived Meta application in Python: start, work, recover, restart.

A program that keeps three agents for as long as it lives, and survives the
things that happen to such a program. Six steps, each printed as it happens:

1. start the runtime on a state directory, with the agents the roster names;
2. give an agent work and know when that work has finished, in two ways;
3. recover one agent without losing what it knows (a respawn);
4. stop and start again on the same state: the agents are the same agents;
5. lose the gateway process, notice it, and start again;
6. two mistakes a supervisor must not make, and what each looks like.

Steps 1 and 4 to 6 need no provider key. Steps 2 and 3, and the lines of the
later steps that ask an agent what it remembers, run only when
OPENAI_API_KEY is set; without it they say so and the rest still runs.
"""

import asyncio
import os
import subprocess
import sys
from pathlib import Path

from meerkat_meta import DurableAgentSpec, Meta, TurnOutputStatus
from meerkat_meta.errors import (
    GatewayStartupCleanupError,
    InitOutcomeUnknownError,
    TransportReaderFailedError,
)

HERE = Path(__file__).resolve().parent
DESKS = ["desk:intake", "desk:billing", "desk:notes"]
LIVE = bool(os.environ.get("OPENAI_API_KEY"))
WORD = "heron"


def yes_no(value: bool) -> str:
    return "yes" if value else "no"


class Roster:
    """The agents this application keeps. The gateway asks at every start."""

    async def roster(self, context):
        return [
            DurableAgentSpec(
                identity=desk,
                profile="desk",
                display_name=desk.split(":")[1].title(),
                additional_instructions=["Answer in one short sentence."],
            )
            for desk in DESKS
        ]


def builder(state_dir: Path, gateway: str, *, init_deadline: float = 120.0):
    # `persistent_state` is what makes the agents outlive this process: their
    # sessions, generations and history are kept under that directory.
    # `init_deadline` bounds the wait for the start. The SDK has no deadline
    # of its own, because a large restore may honestly take minutes.
    return (
        Meta.builder()
        .mob(str(HERE / "mob.toml"))
        .gateway(gateway)
        .persistent_state(str(state_dir))
        .roster(Roster())
        .init_deadline(init_deadline)
    )


async def stop(runtime) -> None:
    """Shut the runtime down.

    After the gateway process has died, the first `shutdown()` raises: its
    shutdown request went unanswered. `last_gateway_termination` says what
    the stop saw. When the process has exited there is nothing left to stop;
    in every other case the error stands, because the runtime still holds a
    gateway that a later `shutdown()` has to stop.
    """
    try:
        await runtime.shutdown()
    except RuntimeError:
        termination = runtime.last_gateway_termination
        if termination is None or termination.state != "exited":
            raise


async def describe(runtime, step: str) -> dict[str, tuple[int, str]]:
    """Print and return each agent as (generation, session)."""
    seen = {}
    for desk in DESKS:
        status = await runtime.agent(desk).status()
        seen[desk] = (status.generation, status.session_id)
        print(f"[{step}] {desk}: {status.state}, generation {status.generation}")
    return seen


async def remembers(runtime, step: str, when: str) -> None:
    """Ask the intake agent for the word it was told in step 2."""
    if not LIVE:
        return
    answer = await runtime.agent("desk:intake").send_and_wait(
        "Which word did I ask you to remember? Answer with that word only.", timeout=90
    )
    known = WORD in (answer or "").lower()
    print(f"[{step}] {when}, intake still knows the word: {yes_no(known)}")


async def step_work(runtime) -> None:
    if not LIVE:
        print("[2] skipped: set OPENAI_API_KEY to give the agents work")
        return
    intake = runtime.agent("desk:intake")
    # One call: send, wait for THAT turn, return its text. The agents here
    # are `turn_driven`, so the turn is tracked and no other delivery to the
    # agent can be mistaken for the answer.
    answer = await intake.send_and_wait(
        f"Remember the word {WORD}. Reply only: noted.", timeout=90
    )
    noted = (answer or "").strip().lower().rstrip(".")
    print(f"[2] send_and_wait answered: {noted}")
    # The same in two steps, for a program that has other things to do while
    # the agent works: keep the ticket, come back for the turn later.
    sent = await intake.send("Reply only: ready.", track_turn=True)
    if sent.turn_ticket is None:
        print("[2] the turn could not be tracked")
        return
    turn = await intake.wait_for_turn(sent.turn_ticket, timeout=90)
    has_text = turn.output_status is TurnOutputStatus.TEXT
    print(
        f"[2] the tracked turn ended {turn.state.value}, with text: {yes_no(has_text)}"
    )


async def step_respawn(runtime, before) -> None:
    # A respawn is the recovery for one agent that is stuck or broken. It is
    # not a reset: the agent keeps its identity, its session and its history,
    # and comes back one generation later.
    if not LIVE:
        # A respawn builds the agent again, and building it needs the
        # provider key: without one it is refused (`missing_secret`).
        print("[3] skipped: a respawn builds the agent, which needs OPENAI_API_KEY")
        return
    result = await runtime.agent("desk:intake").respawn()
    generation, session = before["desk:intake"]
    same_session = result["session_id"] == session
    print(
        f"[3] respawned desk:intake: generation {generation} to {result['generation']}, "
        f"same session: {yes_no(same_session)}"
    )
    await remembers(runtime, "3", "after the respawn")


def kill_gateway() -> None:
    """Stand in for a crash: kill the gateway child of this process."""
    subprocess.run(
        ["pkill", "-KILL", "-P", str(os.getpid()), "-f", "rpc_gateway"], check=False
    )


async def step_mistakes(state_dir: Path, gateway: str, runtime) -> None:
    # Mistake one: a second runtime on a state directory that is in use. The
    # gateway refuses it. In the 0.9.15 SDK the refusal arrives wrapped: the
    # sentence that says why is in `startup_error`.
    try:
        second = await builder(state_dir, gateway, init_deadline=30).build()
    except GatewayStartupCleanupError as error:
        reason = str(error.startup_error)
        in_use = "in use by another Meta runtime" in reason
        print(
            f"[6] a second runtime on the same state was refused, state in use: {yes_no(in_use)}"
        )
    else:
        await stop(second)
        print("[6] a second runtime on the same state STARTED: this must not happen")
    await stop(runtime)
    # Mistake two: treating an unknown outcome as a refusal. When the wait
    # for the start ends without an answer, the gateway may already have
    # changed state. The error says how far the start got; it does not say
    # the start failed. Find out what happened before starting again. Here
    # the deadline is one millisecond, so the outcome is unknown by design.
    try:
        hasty = await builder(state_dir, gateway, init_deadline=0.001).build()
    except InitOutcomeUnknownError as error:
        print(
            f"[6] a start that ran out of time: outcome unknown, last phase seen: {error.last_phase is not None}"
        )
    else:
        await stop(hasty)
        print("[6] the start beat a deadline of one millisecond")


async def main() -> int:
    gateway = os.environ.get("META_GATEWAY_BIN")
    if not gateway:
        print("set META_GATEWAY_BIN to the rpc_gateway binary", file=sys.stderr)
        return 2
    state_dir = Path(os.environ.get("EXAMPLE_STATE_DIR", HERE / ".state" / "meta"))

    # First life.
    runtime = await builder(state_dir, gateway).build()
    try:
        first = await describe(runtime, "1")
        await step_work(runtime)
        await step_respawn(runtime, first)
        before_restart = await describe(runtime, "3")
    finally:
        await stop(runtime)

    # Second life: the same state directory, a new gateway process.
    runtime = await builder(state_dir, gateway).build()
    after_restart = await describe(runtime, "4")
    print(
        f"[4] same agents after the restart: {yes_no(after_restart == before_restart)}"
    )
    await remembers(runtime, "4", "after the restart")

    # The gateway dies. The runtime notices: the next call raises
    # `TransportReaderFailedError` (or its subclass `GatewayChildEndedError`,
    # once the runtime has seen the child end), and `gateway_ended` says why.
    # Nothing is retried for you.
    kill_gateway()
    try:
        await runtime.agent("desk:notes").status()
        print("[5] a call after the kill was answered: this must not happen")
    except TransportReaderFailedError:
        print(
            f"[5] the gateway is gone and the runtime says so: {not runtime.is_running}"
        )
    await stop(runtime)

    # Third life: what a supervisor does next. The state was not harmed.
    runtime = await builder(state_dir, gateway).build()
    after_crash = await describe(runtime, "5")
    print(f"[5] same agents after the crash: {yes_no(after_crash == before_restart)}")
    await remembers(runtime, "5", "after the crash")

    await step_mistakes(state_dir, gateway, runtime)

    # After an unknown outcome the next start is the check: a state directory
    # that a gateway still holds is refused, and a start that settles ready
    # means the state is good. So: start once more and look.
    runtime = await builder(state_dir, gateway).build()
    try:
        final = await describe(runtime, "end")
        print(f"[end] same agents at the end: {yes_no(final == before_restart)}")
    finally:
        await stop(runtime)
    return 0


if __name__ == "__main__":
    raise SystemExit(asyncio.run(main()))
