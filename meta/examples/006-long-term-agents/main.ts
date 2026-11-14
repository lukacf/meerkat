/**
 * Long-term agents created and changed at run time (Meta 0.9.15 and later).
 *
 * One program, five steps, each printed as it happens:
 *
 * 1. launch with a policy that says which run-time changes the host may make;
 * 2. adopt `scout`, the agent this program declares in its own roster;
 * 3. read that operation back by its request id;
 * 4. create `editor`, update it, archive it, bring it back and remove it;
 * 5. show what a refusal looks like and what it tells you to do.
 *
 * Run it twice on the same state directory. The second run sends the same
 * request ids, so every change is answered from the record of the first run
 * and nothing is changed twice: `editor` is not created a second time. That
 * is the point of naming every change with a request id. An answer from the
 * record states less than the first one did (no seat, no instruction
 * outcome).
 *
 * No provider key is needed: the program starts no agent run.
 */
import { mkdirSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { IdentityRefusedError, Meta } from "@rkat/meta-sdk";
import type {
  DurableAgentSpec,
  IdentitiesHandle,
  IdentityChangeResult,
  RosterProvider,
} from "@rkat/meta-sdk";

const HERE = dirname(fileURLToPath(import.meta.url));

// What this host may change at run time. A change no rule matches is refused
// (`change_denied`), so every change the program makes is named here. Nothing
// in this policy lets an agent change other agents: every rule is for the
// host, on the RPC surface this SDK speaks.
const POLICY = {
  enabled: true,
  approval: {
    rules: [
      {
        change: ["adopt", "create", "descriptive", "archive", "remove"],
        requester: { kind: "host", surface: "rpc" },
        outcome: { kind: "allow" },
      },
    ],
  },
  // `editor` names `scout` as a peer; peers are denied unless allowed here.
  peers: { default: "allow" },
};

// The agents this program declares in configuration: one, `scout`.
const roster: RosterProvider = {
  async roster(): Promise<DurableAgentSpec[]> {
    return [
      {
        identity: "scout",
        profile: "researcher",
        addressability: "addressable",
        displayName: "Scout",
        labels: {},
        context: null,
        additionalInstructions: ["Find sources and report them briefly."],
      },
    ];
  },
};

function say(step: string, text: string): void {
  console.log(`[${step}] ${text}`);
}

async function adoptScout(identities: IdentitiesHandle): Promise<void> {
  const record = await identities.get("scout");
  say("2", `scout: spec digest ${record.specDigest.slice(0, 19)}…`);
  try {
    const outcome = await identities.adopt("scout", {
      expectedSpecDigest: record.specDigest,
      requestId: "adopt-scout-1",
    });
    say("2", `adopt answered ${outcome.outcome}`);
  } catch (error) {
    if (!(error instanceof IdentityRefusedError)) throw error;
    if (error.details.reason === "nothing_to_adopt") {
      say("2", "scout was adopted by an earlier run; nothing to do");
    } else if (error.retryable) {
      say("2", `not yet (${error.kind}); the same request can be sent again later`);
    } else {
      throw error;
    }
  }
}

async function readOperation(identities: IdentitiesHandle): Promise<void> {
  // `outcome` tells the answers apart, for the program and for the compiler.
  const state = await identities.operation({ requestId: "adopt-scout-1" });
  switch (state.outcome) {
    case "done":
      say("3", "the adoption is committed");
      break;
    case "pending_decision":
      say("3", `the adoption waits for a decision until ${state.deadlineMs}`);
      break;
    case "refused":
      say("3", `the adoption was refused: ${state.refusal.kind}`);
      break;
    default:
      say("3", `operation state: ${state.outcome}`);
  }
}

/** The agent of a change that is done, or `null` with a line saying why not. */
function doneAgent(result: IdentityChangeResult, requestId: string) {
  if (result.outcome !== "done") {
    say("4", `${requestId} answered ${result.outcome}; stopping here`);
    return null;
  }
  return result.agents[0] ?? null;
}

async function editorLifecycle(identities: IdentitiesHandle): Promise<void> {
  let created: IdentityChangeResult;
  try {
    created = await identities.create(
      [
        {
          name: "editor",
          profile: "writer",
          displayName: "Editor",
          instructions: ["Edit for clarity."],
          peers: ["scout"],
        },
      ],
      { requestId: "create-editor-1" },
    );
  } catch (error) {
    if (!(error instanceof IdentityRefusedError)) throw error;
    // A refusal says what was wrong and where. With a request id that
    // was never used, for instance, a removed agent's name is refused
    // (`name_taken`): it is never available again. Say so and stop.
    say("4", `create refused: ${error.kind} at ${error.field}: ${error.detail}`);
    return;
  }
  // The policy above allows this change, so it is done at once. Under a rule
  // that asks a person first, the answer would be a pending decision with no
  // agent in it yet; this example stops there.
  let agent = doneAgent(created, "create-editor-1");
  if (agent === null) return;
  say("4", `created editor: state ${agent.state}, seat ${agent.seat}`);

  // Each change names the digest it expects and answers the next one.
  let digest = agent.specDigest;
  if (digest === null) return;
  agent = doneAgent(
    await identities.update("editor", {
      expectedSpecDigest: digest,
      requestId: "update-editor-1",
      set: { instructions: ["Edit for clarity.", "Keep the author's voice."] },
    }),
    "update-editor-1",
  );
  if (agent === null) return;
  say("4", `updated editor's instructions: ${agent.instructions?.kind ?? "not stated"}`);

  const steps = [
    ["archived", "archive-editor-1"],
    ["unarchived", "unarchive-editor-1"],
    ["removed", "remove-editor-1"],
  ] as const;
  for (const [change, requestId] of steps) {
    digest = agent.specDigest ?? digest;
    const options = { expectedSpecDigest: digest, requestId };
    const result =
      change === "archived"
        ? await identities.archive("editor", options)
        : change === "unarchived"
          ? await identities.unarchive("editor", options)
          : await identities.remove("editor", options);
    agent = doneAgent(result, requestId);
    if (agent === null) return;
    say("4", `${change} editor: state ${agent.state}`);
  }
}

async function showRefusal(identities: IdentitiesHandle): Promise<void> {
  // `scout` exists; asking to create it again is refused, and the refusal
  // says which field of the request was wrong.
  try {
    await identities.create([{ name: "scout", profile: "researcher" }], {
      requestId: "create-scout-again-1",
    });
    say("5", "unexpected: creating scout again was not refused");
  } catch (error) {
    if (!(error instanceof IdentityRefusedError)) throw error;
    say("5", `refused as expected: ${error.kind} at ${error.field}: ${error.detail}`);
    say("5", `retryable: ${error.retryable}`);
  }
}

async function main(): Promise<number> {
  const gateway = process.env.META_GATEWAY_BIN;
  if (!gateway) {
    console.error("Set META_GATEWAY_BIN to the rpc_gateway binary of Meta 0.9.15 or later.");
    return 2;
  }
  const stateDir = process.env.EXAMPLE_STATE_DIR ?? join(HERE, ".state");
  mkdirSync(stateDir, { recursive: true });

  const runtime = await Meta.builder()
    .gateway(gateway)
    .mob(join(HERE, "mob.toml"))
    .persistentState(stateDir)
    .rosterProvider(roster)
    .identityCreation(POLICY)
    .build();
  say("1", "runtime started with a policy that allows the host to adopt, create, describe, archive and remove");
  try {
    const identities = runtime.identities;
    await adoptScout(identities);
    await readOperation(identities);
    await editorLifecycle(identities);
    await showRefusal(identities);
    const page = await identities.list({ includeArchived: true });
    say("end", "agents now: " + page.agents.map((a) => `${a.name} (${a.readiness})`).join(", "));
  } finally {
    await runtime.shutdown();
  }
  return 0;
}

process.exitCode = await main();
