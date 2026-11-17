/**
 * Meta TypeScript quick start: one long-lived agent, a question, its answer.
 *
 * The program starts a Meta runtime on a state directory, with the one agent
 * its roster names. It asks the agent what it remembers, tells it a word,
 * and stops. Run it a second time on the same state directory: the agent is
 * the same agent, and it knows the word.
 *
 * Three steps, each printed as it happens:
 *
 * 1. start the runtime and read the agent;
 * 2. send a question and wait for the answer of that turn;
 * 3. send a second message, to have something to remember next time.
 *
 * Step 1 needs no provider key. Steps 2 and 3 run when OPENAI_API_KEY is set;
 * without it they say so.
 */
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { Meta } from "@rkat/meta-sdk";
import type { DurableAgentSpec, RosterProvider } from "@rkat/meta-sdk";

const HERE = dirname(fileURLToPath(import.meta.url));
const AGENT = "assistant";

// The agents this application keeps. The gateway asks for them at every
// start: an agent the roster names is there after a restart, with the
// conversation it had.
const roster: RosterProvider = {
  async roster(): Promise<DurableAgentSpec[]> {
    return [
      {
        identity: AGENT,
        profile: "worker",
        addressability: "addressable",
        displayName: "Assistant",
        labels: {},
        context: null,
        additionalInstructions: ["Answer in one short sentence."],
      },
    ];
  },
};

function say(step: string, text: string): void {
  console.log(`[${step}] ${text}`);
}

async function main(): Promise<number> {
  const gateway = process.env.META_GATEWAY_BIN;
  if (!gateway) {
    console.error("Set META_GATEWAY_BIN to the rpc_gateway binary of the same version as @rkat/meta-sdk.");
    return 2;
  }
  // `persistentState` is what makes the agent outlive this process: its
  // session and its history are kept under that directory.
  const stateDir = process.env.EXAMPLE_STATE_DIR ?? join(HERE, ".meta", "state");
  mkdirSync(stateDir, { recursive: true });

  const runtime = await Meta.builder()
    .mob(join(HERE, "config", "mob.toml"))
    .gateway(gateway)
    .persistentState(stateDir)
    .rosterProvider(roster)
    .build();
  try {
    // ── 1. The runtime is up, and the agent the roster names is there ──
    const status = await runtime.mobHandle().status();
    const agent = await runtime.status(AGENT);
    say("1", `runtime running: ${status.running}`);
    say("1", `${AGENT}: ${agent.lifecycleState}, generation ${agent.generation}, session ${agent.sessionId}`);

    if (!process.env.OPENAI_API_KEY) {
      say("2", "skipped: set OPENAI_API_KEY to send the agent work");
      say("3", "skipped: set OPENAI_API_KEY to send the agent work");
      return 0;
    }

    // ── 2. Send, wait for that turn, read its answer ──
    // `sendAndWait` waits for the turn this send started, by its ticket, and
    // resolves the text of that turn. Nothing is polled.
    const remembered = await runtime.sendAndWait(
      AGENT,
      "Which word did I ask you to remember in an earlier conversation? " +
        "Reply with that word only, or with the word none.",
    );
    say("2", `the word from an earlier run: ${remembered}`);

    // ── 3. The typed form of the same call says how the turn ended ──
    const turn = await runtime.sendAndWaitOutcome(
      AGENT,
      "Remember the word heron. Reply with the single word: noted",
    );
    say("3", `answer: ${turn.text} (output ${turn.outputStatus}, attributed to this send: ${turn.attributed})`);
    return 0;
  } finally {
    await runtime.shutdown();
  }
}

main().then(
  (code) => {
    process.exitCode = code;
  },
  (error: unknown) => {
    console.error(error);
    process.exitCode = 1;
  },
);
