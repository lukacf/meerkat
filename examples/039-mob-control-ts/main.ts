/**
 * 039: Drive a Mob from the TypeScript SDK
 *
 * An application creates a small mob over `rkat-rpc`, gives it work, follows
 * what the members do and stops work it no longer wants.
 *
 * What you will learn:
 * - Creating a mob, spawning members and wiring them as peers
 * - Sending work to the member that is the entry point of the application
 * - Following every member on one event stream and reading member status
 * - Stopping one run with `mob.stopMemberRun()` (the stop button)
 * - Clearing a member with `client.mobCancelAllWork()` (its run and its queue)
 *
 * Run:
 *   ANTHROPIC_API_KEY=sk-... npx tsx main.ts
 */

import {
  MeerkatClient,
  isRunCompleted,
  isRunFailed,
  isTextDelta,
  isToolCallRequested,
} from "@rkat/sdk";
import type {
  AgentEvent,
  AttributedMobEvent,
  Mob,
  MobDefinition,
  RunStartedEvent,
} from "@rkat/sdk";

const MODEL = "claude-sonnet-5-5";
const MOB_ID = "newsdesk";
const EDITOR = "editor-1";
const RESEARCHER = "researcher-1";

// Two profiles. Only the editor is `external_addressable`: it is the one
// member the application may send work to. Every profile needs comms.
const DEFINITION: MobDefinition = {
  id: MOB_ID,
  profiles: {
    editor: {
      model: MODEL,
      peer_description: "Takes requests from the application and writes the answer",
      external_addressable: true,
      tools: { comms: true },
    },
    researcher: {
      model: MODEL,
      peer_description: "Looks up facts for the editor",
      tools: { comms: true },
    },
  },
};

const EDITOR_BRIEF =
  "You are the editor of a small news desk. Requests arrive from the " +
  "application. When a request needs facts, send your peer researcher-1 a " +
  "message, and write your answer once its reply has arrived. " +
  "Reply READY now and wait for the first request.";
const RESEARCHER_BRIEF =
  "You are the researcher of a small news desk. When editor-1 sends you a " +
  "question, answer it with one message to editor-1. " +
  "Reply READY now and wait.";
const LONG_JOB = "Write a 600-word feature on the history of lighthouses. Do not message anyone.";

// The SDK exports a guard for most event types; this one it does not.
function isRunStarted(event: AgentEvent): event is RunStartedEvent {
  return event.type === "run_started";
}

/** What the application has seen on the mob event stream. */
class Desk {
  readonly events: AttributedMobEvent[] = [];
  private waiters: Array<() => void> = [];

  /** Print a line for each event that matters and keep all of them. */
  async follow(subscription: AsyncIterable<AttributedMobEvent>): Promise<void> {
    for await (const item of subscription) {
      const member = item.source;
      const event = item.envelope.payload;
      if (isRunStarted(event)) {
        console.log(`  [${member}] run ${event.identity?.run_id} started`);
      } else if (isToolCallRequested(event) && event.name.startsWith("send_")) {
        console.log(`  [${member}] calls ${event.name}`);
      } else if (isRunCompleted(event)) {
        console.log(`  [${member}] run completed: ${JSON.stringify(event.result.trim().slice(0, 160))}`);
      } else if (isRunFailed(event)) {
        console.log(`  [${member}] run ended: ${event.errorClass}`);
      }
      this.events.push(item);
      for (const wake of this.waiters.splice(0)) wake();
    }
  }

  /** Wait for the next event of one kind from one member. */
  async next<E extends AgentEvent>(
    member: string,
    isKind: (event: AgentEvent) => event is E,
    after = 0,
    timeoutMs = 120_000,
  ): Promise<E> {
    const deadline = Date.now() + timeoutMs;
    for (;;) {
      for (const item of this.events.slice(after)) {
        const event = item.envelope.payload;
        if (item.source === member && isKind(event)) return event;
      }
      const left = deadline - Date.now();
      if (left <= 0) throw new Error(`${member} sent no such event within ${timeoutMs} ms`);
      await new Promise<void>((resolve) => {
        const timer = setTimeout(resolve, left);
        this.waiters.push(() => {
          clearTimeout(timer);
          resolve();
        });
      });
    }
  }
}

/**
 * Return once every member has been idle on two reads in a row.
 *
 * A script has to end; an application with a window keeps the stream open
 * and shows what arrives instead.
 */
async function waitUntilIdle(mob: Mob, members: string[], timeoutSeconds = 180): Promise<void> {
  let quiet = 0;
  for (let second = 0; second < timeoutSeconds; second++) {
    const states = await Promise.all(members.map((member) => mob.memberStatus(member)));
    // Idle with nothing admitted: a message that has arrived but whose run
    // has not started yet still counts as work.
    const idle = states.every(
      (state) => state.progress?.run_state === "idle" && state.progress.in_flight_work === 0,
    );
    quiet = idle ? quiet + 1 : 0;
    if (quiet === 2) return;
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  throw new Error("the mob did not become idle");
}

/** The steps after the create, on a mob that `main` destroys whatever happens. */
async function drive(client: MeerkatClient, mob: Mob): Promise<void> {
  const editor = await mob.spawn({ profile: "editor", agentIdentity: EDITOR, initialMessage: EDITOR_BRIEF });
  await mob.spawn({ profile: "researcher", agentIdentity: RESEARCHER, initialMessage: RESEARCHER_BRIEF });
  await mob.wire(EDITOR, RESEARCHER);
  // A member runs one first turn on its initial message when it is spawned.
  await mob.waitForKickoffComplete({ timeoutMs: 120_000 });
  for (const member of await mob.listMembers()) {
    console.log(`  ${member.agentIdentity}: ${member.profile}, wired to ${JSON.stringify(member.wiredTo ?? [])}`);
  }

  // What a member was told: the first message of its transcript is its
  // system prompt, with the skills the runtime gave it.
  const history = await mob.memberHistory(EDITOR, { fromIndex: 0, limit: 1 });
  const first = history.page.messages[0] as { content?: unknown } | undefined;
  const systemPrompt = typeof first?.content === "string" ? first.content : "";
  const named = [...systemPrompt.matchAll(/skill_name="([^"]+)"/g)].map((match) => match[1]);
  const skills = [...new Set(named)].sort();
  console.log(`  ${EDITOR} system prompt: ${systemPrompt.length} characters, skills ${JSON.stringify(skills)}`);

  const desk = new Desk();
  const subscription = await mob.subscribeEvents();
  const follower = desk.follow(subscription);
  try {
    await waitUntilIdle(mob, [EDITOR, RESEARCHER]);

    // ── 2. Send work and read the result ──
    console.log("\n=== 2. Send work ===");
    let mark = desk.events.length;
    await mob.member(EDITOR).send(
      "In two sentences, say why lighthouses were built. Get two facts from the researcher first.",
    );
    await desk.next(EDITOR, isRunStarted, mark);
    await waitUntilIdle(mob, [EDITOR, RESEARCHER]);
    const status = await mob.memberStatus(EDITOR);
    console.log(`  ${EDITOR} is ${status.status}, ${status.tokensUsed} tokens used. Its last answer:`);
    console.log(`  ${(status.outputPreview ?? "").trim()}`);

    // ── 3. Stop one run ──
    console.log("\n=== 3. Stop one run ===");
    mark = desk.events.length;
    await mob.member(EDITOR).send(LONG_JOB);
    const started = await desk.next(EDITOR, isRunStarted, mark);
    const runId = started.identity?.run_id;
    if (!runId) throw new Error("run_started carried no run id");
    await desk.next(EDITOR, isTextDelta, mark); // the member is writing
    const stopped = await mob.stopMemberRun(EDITOR, runId, "user pressed stop");
    const ended = stopped.receipt.outcome === "stopped" ? stopped.receipt.contributors : [];
    console.log(`  stop: ${stopped.receipt.outcome}, inputs ended: ${JSON.stringify(ended)}`);
    // The run id fences the stop: a run that is no longer current is left alone.
    const again = await mob.stopMemberRun(EDITOR, runId, "pressed twice");
    console.log(`  the same stop again: ${again.receipt.outcome}`);

    // ── 4. Cancel everything a member holds ──
    console.log("\n=== 4. Cancel all work of a member ===");
    mark = desk.events.length;
    await mob.member(EDITOR).send(LONG_JOB);
    await desk.next(EDITOR, isTextDelta, mark);
    await mob.member(EDITOR).send("Queued behind the feature: reply with the word ONE.");
    await mob.member(EDITOR).send("Queued behind the feature: reply with the word TWO.");
    const result = await client.mobCancelAllWork({ memberRef: editor.memberRef });
    console.log(`  cancel_all_work: ${JSON.stringify(result)}`);
    // The run ends at its next boundary, which can be the end of the model
    // response it is in; the two queued inputs are dropped with it.
    await desk.next(EDITOR, isRunFailed, mark);

    // ── 5. The member is still there ──
    console.log("\n=== 5. The member takes new work ===");
    await mob.member(EDITOR).send("Reply with the single word PONG.");
    // Had ONE and TWO still been queued, they would run before this.
    const done = await desk.next(EDITOR, isRunCompleted, mark);
    console.log(`  ${EDITOR} answered: ${JSON.stringify(done.result.trim())}`);
  } finally {
    await subscription.close();
    await follower;
  }
}

async function main() {
  const client = new MeerkatClient();

  try {
    await client.connect();

    // ── 1. Create, spawn, wire ──
    console.log("=== 1. Create the mob ===");
    const mob = await client.createMob({ definition: DEFINITION });
    try {
      await drive(client, mob);
    } finally {
      // Also when a step failed: the mob is kept in the realm of this run,
      // and nothing else would remove it.
      await client.mobDestroy(MOB_ID);
      console.log("\nMob destroyed.");
    }
  } finally {
    await client.close();
  }
}

main().catch((error: unknown) => {
  console.error(error);
  process.exitCode = 1;
});
