"""Ask the user: an agent stops to ask a person, and goes on with the answer (Meta 0.9.15 and later).

One program, six steps, each printed as it happens:

1. launch with a state directory and one agent, `planner`;
2. the host gives the planner a task with two options; the planner does not
   choose: it asks with its `ask_user` tool, and its turn ends;
3. the host reads the open question set: who asked, the question, the choices;
4. the host stops the runtime and starts it again: the question is still open;
5. the host answers for the person; the answer starts a turn of the planner,
   which goes on with the choice;
6. the planner asks a second question and the host dismisses it: the planner
   is told that no answer is coming.

A question set is kept in the state directory until it is answered, dismissed
or its deadline passes (one hour unless the agent names another), so the
person may answer much later, and after a restart.

This program plays the person: it answers with the choice named by
EXAMPLE_CHOICE (default: boat). In an application the same three calls sit
behind your own screen: list the open sets, show one, send the answers.

All of it needs a provider key (OPENAI_API_KEY): only an agent can ask.
Without one the program starts the runtime, lists no question and stops.
"""

from __future__ import annotations

import asyncio
import os
import sys
import time
from pathlib import Path

from meerkat_meta import DurableAgentSpec, GatingQuestionAnswer, Meta

HERE = Path(__file__).resolve().parent
PERSON = "example-person"  # who answered: the host names the person

TASK = (
    "Plan the team outing for Friday. The options are bowling (id bowling) "
    "and a boat trip (id boat)."
)
SECOND = (
    "Ask the user whether lunch should be included (ids yes and no), with the "
    "question id lunch."
)


class Roster:
    """The agents this program keeps. The gateway asks at every start."""

    async def roster(self, context):
        return [
            DurableAgentSpec(
                identity="planner",
                profile="planner",
                display_name="Planner",
                additional_instructions=[
                    "You plan team outings. You never choose for the user: when "
                    "a task gives options, ask which one they want with your "
                    "ask_user tool, in one question whose id is the one you "
                    "were given, or venue if none was given, and whose choices "
                    "have the ids you were given.",
                    "After asking, reply with the single sentence: I asked the user.",
                    "When the result of a question arrives, reply with one "
                    "short sentence that says what the user chose, or that no "
                    "answer came.",
                ],
            )
        ]


def say(step: str, text: str) -> None:
    print(f"[{step}] {text}", flush=True)


async def start(gateway: str, state_dir: Path):
    # A state directory is what lets a question wait: a launch without one
    # keeps no decisions, and an ask_user call there answers no_human_surface.
    return await (
        Meta.builder()
        .gateway(gateway)
        .mob(str(HERE / "mob.toml"))
        .persistent_state(str(state_dir))
        .roster(Roster())
        .build()
    )


async def turn(agent, text: str) -> str:
    """Send one message and wait for the turn it starts."""
    sent = await agent.send(text)
    return (await agent.wait_for_completion(sent.completion_baseline, timeout=180)) or ""


def describe(question_set) -> str:
    question = question_set.questions[0]
    choices = ", ".join(f"{c.id} ({c.label})" for c in question.choices)
    minutes = round((question_set.deadline_at_ms - time.time() * 1000) / 60000)
    asker = question_set.origin.identity if question_set.origin else "an agent"
    return (
        f"asked by {asker}: {question.prompt} "
        f"Choices: {choices}. Open for about {minutes} more minutes."
    )


async def main() -> int:
    gateway = os.environ.get("META_GATEWAY_BIN")
    if not gateway:
        print("Set META_GATEWAY_BIN to the rpc_gateway binary of Meta 0.9.15 or later.", file=sys.stderr)
        return 2
    state_dir = Path(os.environ.get("EXAMPLE_STATE_DIR", HERE / ".state" / "python"))
    choice = os.environ.get("EXAMPLE_CHOICE", "boat")
    live = bool(os.environ.get("OPENAI_API_KEY"))

    runtime = await start(gateway, state_dir)
    try:
        handle = runtime.mob_handle()
        planner = runtime.agent("planner")
        open_sets = await handle.gating_questions()
        say("1", f"runtime started, open question sets: {len(open_sets)}")
        if not live:
            say("2", "skipped: set OPENAI_API_KEY, only an agent can ask a question")
            return 0

        # A set left open by an earlier run (stopped before step 5) is taken
        # up here instead of asking again.
        if open_sets:
            say("2", "a question of an earlier run is still open; the planner is not asked again")
        else:
            reply = await turn(planner, TASK)
            say("2", f"the planner's turn ended with: {reply}")
            open_sets = await handle.gating_questions()
        if not open_sets:
            say("3", "the planner asked nothing; run again")
            return 1
        asked = open_sets[0]
        say("3", describe(asked))

        # The set lives in the state directory, not in this process.
        await runtime.shutdown()
        runtime = await start(gateway, state_dir)
        handle = runtime.mob_handle()
        planner = runtime.agent("planner")
        still = [s for s in await handle.gating_questions() if s.pending_id == asked.pending_id]
        say("4", f"after a restart the same question is open: {'yes' if still else 'no'}")
        if not still:
            return 1
        asked = still[0]

        # The answer names the set, the digest it was shown with, the person,
        # and one choice per answered question. It is delivered to the agent
        # as the result of its question, and that delivery starts a turn.
        before = await planner.completion_cursor()
        question = asked.questions[0]
        picked = choice if any(c.id == choice for c in question.choices) else question.choices[0].id
        result = await handle.gating_answer(
            asked.pending_id,
            approver_id=PERSON,
            presented_digest=asked.subject_digest,
            answers=[GatingQuestionAnswer.choice(question.id, picked)],
        )
        say("5", f"the host answered {picked} for the person: the set is {result.outcome}")
        after_answer = await planner.wait_for_completion(before, timeout=180)
        say("5", f"the planner went on: {after_answer}")

        # Dismissing ends a set without an answer; the agent is told so.
        await turn(planner, SECOND)
        second = await handle.gating_questions()
        if not second:
            say("6", "the planner asked no second question")
            return 1
        before = await planner.completion_cursor()
        result = await handle.gating_dismiss(
            second[0].pending_id, approver_id=PERSON, presented_digest=second[0].subject_digest
        )
        say("6", f"the host dismissed the question {second[0].questions[0].id}: the set is {result.outcome}")
        after_dismiss = await planner.wait_for_completion(before, timeout=180)
        say("6", f"the planner was told: {after_dismiss}")
        say("end", f"open question sets: {len(await handle.gating_questions())}")
    finally:
        await runtime.shutdown()
    return 0


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
