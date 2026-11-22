/**
 * Ask the user: an agent stops to ask a person, and goes on with the answer
 * (Meta 0.9.15 and later).
 *
 * One program, six steps, each printed as it happens:
 *
 * 1. launch with a state directory and one agent, `planner`;
 * 2. the host gives the planner a task with two options; the planner does
 *    not choose: it asks with its `ask_user` tool, and its turn ends;
 * 3. the host reads the open question set: who asked, the question, the
 *    choices;
 * 4. the host stops the runtime and starts it again: the question is still
 *    open;
 * 5. the host answers for the person; the answer starts a turn of the
 *    planner, which goes on with the choice;
 * 6. the planner asks a second question and the host dismisses it: the
 *    planner is told that no answer is coming.
 *
 * A question set is kept in the state directory until it is answered,
 * dismissed or its deadline passes (one hour unless the agent names
 * another), so the person may answer much later, and after a restart.
 *
 * This program plays the person: it answers with the choice named by
 * EXAMPLE_CHOICE (default: boat). In an application the same three calls sit
 * behind your own screen: list the open sets, show one, send the answers.
 *
 * All of it needs a provider key (OPENAI_API_KEY): only an agent can ask.
 * Without one the program starts the runtime, lists no question and stops.
 */
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { Meta } from "@rkat/meta-sdk";
import type { DurableAgentSpec, GatingQuestionSet, MetaRuntime, RosterProvider } from "@rkat/meta-sdk";

const HERE = dirname(fileURLToPath(import.meta.url));
const PLANNER = "planner";
const PERSON = "example-person"; // who answered: the host names the person

const TASK =
  "Plan the team outing for Friday. The options are bowling (id bowling) " +
  "and a boat trip (id boat).";
const SECOND =
  "Ask the user whether lunch should be included (ids yes and no), with the " +
  "question id lunch.";

// The agents this program keeps. The gateway asks at every start.
const roster: RosterProvider = {
  async roster(): Promise<DurableAgentSpec[]> {
    return [
      {
        identity: PLANNER,
        profile: "planner",
        addressability: "addressable",
        displayName: "Planner",
        labels: {},
        context: null,
        additionalInstructions: [
          "You plan team outings. You never choose for the user: when " +
            "a task gives options, ask which one they want with your " +
            "ask_user tool, in one question whose id is the one you " +
            "were given, or venue if none was given, and whose choices " +
            "have the ids you were given.",
          "After asking, reply with the single sentence: I asked the user.",
          "When the result of a question arrives, reply with one " +
            "short sentence that says what the user chose, or that no " +
            "answer came.",
        ],
      },
    ];
  },
};

function say(step: string, text: string): void {
  console.log(`[${step}] ${text}`);
}

function start(gateway: string, stateDir: string): Promise<MetaRuntime> {
  // A state directory is what lets a question wait: a launch without one
  // keeps no decisions, and an ask_user call there answers no_human_surface.
  return Meta.builder()
    .gateway(gateway)
    .mob(join(HERE, "mob.toml"))
    .persistentState(stateDir)
    .rosterProvider(roster)
    .build();
}

/** Send one message and wait for the turn it starts. */
async function turn(runtime: MetaRuntime, text: string): Promise<string> {
  const sent = await runtime.send(PLANNER, text);
  if (sent.completionBaseline === null) throw new Error("the send returned no completion baseline");
  return (await runtime.waitForCompletion(PLANNER, sent.completionBaseline, { timeoutMs: 180_000 })) ?? "";
}

function describe(questionSet: GatingQuestionSet): string {
  const question = questionSet.questions[0];
  const choices = question.choices.map((c) => `${c.id} (${c.label})`).join(", ");
  const minutes = Math.round(((questionSet.deadlineAtMs ?? Date.now()) - Date.now()) / 60000);
  const asker = questionSet.origin?.identity ?? "an agent";
  return `asked by ${asker}: ${question.prompt} Choices: ${choices}. Open for about ${minutes} more minutes.`;
}

async function main(): Promise<number> {
  const gateway = process.env.META_GATEWAY_BIN;
  if (!gateway) {
    console.error("Set META_GATEWAY_BIN to the rpc_gateway binary of Meta 0.9.15 or later.");
    return 2;
  }
  const stateDir = process.env.EXAMPLE_STATE_DIR ?? join(HERE, ".state", "typescript");
  const choice = process.env.EXAMPLE_CHOICE ?? "boat";
  const live = Boolean(process.env.OPENAI_API_KEY);

  let runtime = await start(gateway, stateDir);
  try {
    let handle = runtime.mobHandle();
    let openSets = await handle.gatingQuestions();
    say("1", `runtime started, open question sets: ${openSets.length}`);
    if (!live) {
      say("2", "skipped: set OPENAI_API_KEY, only an agent can ask a question");
      return 0;
    }

    // A set left open by an earlier run (stopped before step 5) is taken up
    // here instead of asking again.
    if (openSets.length > 0) {
      say("2", "a question of an earlier run is still open; the planner is not asked again");
    } else {
      const reply = await turn(runtime, TASK);
      say("2", `the planner's turn ended with: ${reply}`);
      openSets = await handle.gatingQuestions();
    }
    if (openSets.length === 0) {
      say("3", "the planner asked nothing; run again");
      return 1;
    }
    let asked = openSets[0];
    say("3", describe(asked));

    // The set lives in the state directory, not in this process.
    await runtime.shutdown();
    runtime = await start(gateway, stateDir);
    handle = runtime.mobHandle();
    const still = (await handle.gatingQuestions()).filter((s) => s.pendingId === asked.pendingId);
    say("4", `after a restart the same question is open: ${still.length > 0 ? "yes" : "no"}`);
    if (still.length === 0) return 1;
    asked = still[0];

    // The answer names the set, the digest it was shown with, the person,
    // and one choice per answered question. It is delivered to the agent as
    // the result of its question, and that delivery starts a turn.
    let before = await runtime.completionCursor(PLANNER);
    if (before === null) throw new Error("the planner has no completion cursor");
    const question = asked.questions[0];
    const picked = question.choices.some((c) => c.id === choice) ? choice : question.choices[0].id;
    let result = await handle.gatingAnswer(asked.pendingId, {
      approverId: PERSON,
      presentedDigest: asked.subjectDigest,
      answers: [{ questionId: question.id, answer: { kind: "choice", choiceId: picked } }],
    });
    say("5", `the host answered ${picked} for the person: the set is ${result.outcome}`);
    const afterAnswer = await runtime.waitForCompletion(PLANNER, before, { timeoutMs: 180_000 });
    say("5", `the planner went on: ${afterAnswer}`);

    // Dismissing ends a set without an answer; the agent is told so.
    await turn(runtime, SECOND);
    const second = await handle.gatingQuestions();
    if (second.length === 0) {
      say("6", "the planner asked no second question");
      return 1;
    }
    before = await runtime.completionCursor(PLANNER);
    if (before === null) throw new Error("the planner has no completion cursor");
    result = await handle.gatingDismiss(second[0].pendingId, {
      approverId: PERSON,
      presentedDigest: second[0].subjectDigest,
    });
    say("6", `the host dismissed the question ${second[0].questions[0].id}: the set is ${result.outcome}`);
    const afterDismiss = await runtime.waitForCompletion(PLANNER, before, { timeoutMs: 180_000 });
    say("6", `the planner was told: ${afterDismiss}`);
    say("end", `open question sets: ${(await handle.gatingQuestions()).length}`);
  } finally {
    await runtime.shutdown();
  }
  return 0;
}

process.exitCode = await main();
