/**
 * Live smoke tests for the TypeScript SDK against a real rkat-rpc runtime.
 */

import { afterEach, before, describe, it } from "node:test";
import assert from "node:assert/strict";
import { execSync } from "node:child_process";
import { existsSync, mkdtempSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { setTimeout as delay } from "node:timers/promises";

const workspaceRoot = path.resolve(
  path.dirname(new URL(import.meta.url).pathname),
  "..",
  "..",
  "..",
);

const candidateBinaries = [
  path.join(workspaceRoot, "target", "debug", "rkat-rpc"),
  path.join(workspaceRoot, "target-codex", "debug", "rkat-rpc"),
];

const binaryPath = (() => {
  const override = process.env.MEERKAT_BIN_PATH || process.env.MEERKAT_RPC_BINARY;
  if (override) {
    return override;
  }
  for (const candidate of candidateBinaries) {
    if (existsSync(candidate)) {
      return candidate;
    }
  }
  try {
    return execSync("which rkat-rpc", { encoding: "utf8" }).trim();
  } catch {
    try {
      return execSync("which rkat", { encoding: "utf8" }).trim();
    } catch {
      return "";
    }
  }
})();

function hasAnthropicKey() {
  return Boolean(
    process.env.RKAT_ANTHROPIC_API_KEY || process.env.ANTHROPIC_API_KEY,
  );
}

function hasOpenAIKey() {
  return Boolean(
    process.env.RKAT_OPENAI_API_KEY || process.env.OPENAI_API_KEY,
  );
}

function anthropicModel() {
  return (
    process.env.SMOKE_MODEL_ANTHROPIC ||
    process.env.SMOKE_MODEL ||
    "claude-sonnet-4-5"
  );
}

function openaiModel() {
  return process.env.SMOKE_MODEL_OPENAI || "gpt-4.1-mini";
}

function includeScenario(id) {
  void id;
  return true;
}

async function withoutOpenAiRealtimeEnv(run) {
  const keys = ["RKAT_OPENAI_API_KEY", "OPENAI_API_KEY"];
  const saved = new Map();
  for (const key of keys) {
    saved.set(key, process.env[key]);
    delete process.env[key];
  }
  try {
    return await run();
  } finally {
    for (const [key, value] of saved.entries()) {
      if (value == null) {
        delete process.env[key];
      } else {
        process.env[key] = value;
      }
    }
  }
}

function logScenarioStep(scenario, step) {
  const timestamp = new Date().toISOString();
  console.log(`[${timestamp}] ${scenario}: ${step}`);
}

async function withStepTimeout(scenario, step, promise, timeoutMs = 60000) {
  logScenarioStep(scenario, `start ${step}`);
  let timer = null;
  try {
    const result = await Promise.race([
      promise,
      new Promise((_, reject) => {
        timer = setTimeout(
          () => reject(new Error(`${scenario}: timed out after ${timeoutMs}ms during ${step}`)),
          timeoutMs,
        );
      }),
    ]);
    logScenarioStep(scenario, `done ${step}`);
    return result;
  } catch (error) {
    logScenarioStep(scenario, `fail ${step}: ${String(error)}`);
    throw error;
  } finally {
    if (timer !== null) {
      clearTimeout(timer);
    }
  }
}

async function waitFor(fetch, predicate, { timeoutMs = 60000, intervalMs = 200 } = {}) {
  const deadline = Date.now() + timeoutMs;
  let lastValue;
  while (Date.now() < deadline) {
    lastValue = await fetch();
    if (predicate(lastValue)) {
      return lastValue;
    }
    await delay(intervalMs);
  }
  throw new Error(`waitFor timed out; last value: ${JSON.stringify(lastValue)}`);
}

describe("Live Smoke: TypeScript SDK", { skip: !binaryPath }, () => {
  let MeerkatClient;
  let RealtimeChannel;
  const clients = [];

  before(async () => {
    ({ MeerkatClient, RealtimeChannel } = await import("../dist/index.js"));
  });

  afterEach(async () => {
    await Promise.all(
      clients.splice(0).map(async (client) => {
        await client.close().catch(() => {});
      }),
    );
  });

  async function connectClient(options = {}) {
    const client = new MeerkatClient(binaryPath);
    clients.push(client);
    await client.connect(options);
    return client;
  }

  if (includeScenario(41)) {
    it(
      "Scenario 41: full lifecycle and recall through the packaged SDK",
      { skip: !hasAnthropicKey() },
      async () => {
      const client = await connectClient({ isolated: true });

      const session = await client.createSession(
        "My name is TsBot and my favorite color is teal. Reply briefly.",
        {
          model: anthropicModel(),
          provider: "anthropic",
        },
      );
      assert.ok(session.id);
      assert.ok(session.text.length > 0);

      const followUp = await session.turn(
        "What is my name and favorite color? Reply in one sentence.",
      );
      const textLower = followUp.text.toLowerCase();
      assert.ok(textLower.includes("tsbot"));
      assert.ok(textLower.includes("teal"));

      const details = await client.readSession(session.id);
      assert.equal(details.sessionId, session.id);
      assert.ok(Number(details.messageCount) >= 4);
      assert.equal(details.isActive, false);

      const history = await client.readSessionHistory(session.id);
      assert.equal(history.sessionId, session.id);
      assert.ok(history.messageCount >= 4);
      assert.ok(history.messages.length >= 4);

      const sessions = await client.listSessions();
      assert.ok(sessions.some((entry) => entry.sessionId === session.id));

      await session.archive();
      const archivedHistory = await session.history();
      assert.equal(archivedHistory.sessionId, session.id);
      assert.ok(archivedHistory.messageCount >= 4);
      assert.ok(archivedHistory.messages.length >= 4);
      const sessionsAfter = await client.listSessions();
      assert.ok(!sessionsAfter.some((entry) => entry.sessionId === session.id));
      },
    );
  }

  if (includeScenario(42)) {
    it(
      "Scenario 42: deferred session, injected context, and streaming through the packaged SDK",
      { skip: !hasAnthropicKey() },
      async () => {
      const client = await connectClient({ isolated: true });

      const deferred = await client.createDeferredSession(
        "Remember the codeword ORBIT-7 for later.",
        {
          model: anthropicModel(),
          provider: "anthropic",
        },
      );

      const injected = await client.injectContext(
        deferred.id,
        "Always include the marker [TS-SDK-CTX] in your replies.",
        { source: "typescript-smoke" },
      );
      assert.ok(["Staged", "staged", "Duplicate", "duplicate"].includes(String(injected.status)));

      const first = await deferred.startTurn(
        "What is the codeword? Include any markers you were told about.",
      );
      const firstTextLower = first.text.toLowerCase();
      assert.ok(firstTextLower.includes("orbit-7") || firstTextLower.includes("orbit 7"));
      assert.ok(firstTextLower.includes("ts-sdk-ctx"));

      const stream = deferred.stream(
        "Repeat the codeword and marker in two short clauses.",
      );
      let streamedText = "";
      let eventCount = 0;
      const seenEventTypes = new Set();
      for await (const event of stream) {
        eventCount += 1;
        seenEventTypes.add(event.type);
        if (event.type === "text_delta") {
          streamedText += event.delta;
        }
      }
      assert.ok(eventCount > 0);
      const finalText = (streamedText || stream.result.text).toLowerCase();
      assert.ok(finalText.includes("orbit"));
      assert.ok(
        seenEventTypes.size > 0,
        "streaming turn should surface at least one public event before returning the final result",
      );
      },
    );
  }

  if (includeScenario(43)) {
    it(
      "Scenario 43: persistent reconnect and resume through the packaged SDK",
      { skip: !hasAnthropicKey() },
      async () => {
      const stateRoot = mkdtempSync(path.join(os.tmpdir(), "ts-sdk-smoke-"));
      const realmId = `ts-sdk-smoke-${Date.now()}`;

      const clientA = await connectClient({
        realmId,
        stateRoot,
        realmBackend: "sqlite",
      });
      const session = await clientA.createSession(
        "Remember that the passphrase is comet-trail.",
        {
          model: anthropicModel(),
          provider: "anthropic",
        },
      );
      const sessionId = session.id;
      await clientA.close();

      const clientB = await connectClient({
        realmId,
        stateRoot,
        realmBackend: "sqlite",
      });
      const details = await clientB.readSession(sessionId);
      assert.equal(details.sessionId, sessionId);
      assert.ok(Number(details.messageCount) >= 2);

      const resumed = await clientB._startTurn(
        sessionId,
        "What passphrase did I ask you to remember?",
      );
      const resumedTextLower = resumed.text.toLowerCase();
      assert.ok(
        resumedTextLower.includes("comet-trail") ||
          resumedTextLower.includes("comet trail"),
      );
      },
    );
  }

  if (includeScenario(44)) {
    it(
      "Scenario 44: mixed-provider swarm probe through the packaged SDK",
      { skip: !(hasAnthropicKey() && hasOpenAIKey()) },
      async () => {
      const scenario = "Scenario 44";
      const client = await withStepTimeout(
        scenario,
        "connect isolated client",
        connectClient({ isolated: true }),
      );
      const mob = await withStepTimeout(scenario, "create mob", client.createMob({
        definition: {
          id: `ts-sdk-swarm-${Date.now()}`,
          orchestrator: { profile: "lead" },
          profiles: {
            lead: {
              model: anthropicModel(),
              tools: { comms: true },
              peer_description: "Lead coordinator",
              external_addressable: true,
            },
            reviewer: {
              model: openaiModel(),
              tools: { comms: true },
              peer_description: "Review worker",
              external_addressable: true,
            },
            broken: {
              model: "definitely-invalid-live-smoke-model",
              tools: { comms: true },
              peer_description: "Deterministic failure worker",
              external_addressable: true,
            },
          },
        },
      }));

      const lead = await withStepTimeout(scenario, "spawn lead", mob.spawn({
        profile: "lead",
        agentIdentity: "lead-1",
        initialMessage: "Acknowledge the lead role in one sentence.",
        runtimeMode: "autonomous_host",
      }));
      const reviewer = await withStepTimeout(scenario, "spawn reviewer", mob.spawn({
        profile: "reviewer",
        agentIdentity: "reviewer-1",
        initialMessage: "Acknowledge the reviewer role in one sentence.",
        runtimeMode: "turn_driven",
      }));
      assert.equal(lead.agentIdentity, "lead-1");
      assert.ok(lead.agentRuntimeId);
      assert.ok(Number.isInteger(lead.fenceToken));
      assert.equal(reviewer.agentIdentity, "reviewer-1");
      assert.ok(reviewer.agentRuntimeId);
      assert.ok(Number.isInteger(reviewer.fenceToken));

      await withStepTimeout(scenario, "wire lead -> reviewer", mob.wire("lead-1", "reviewer-1"));
      const append = await withStepTimeout(scenario, "append reviewer system context", mob.appendSystemContext(
        "reviewer-1",
        "Remember the swarm marker [TS-SWARM].",
        { source: "typescript-sdk", idempotencyKey: "ts-swarm-marker" },
      ));
      assert.ok(["Staged", "staged", "Duplicate", "duplicate"].includes(String(append.status)));
      if (append.agent_identity != null) {
        assert.equal(append.agent_identity, "reviewer-1");
      }

      const subscription = await withStepTimeout(
        scenario,
        "subscribe reviewer member events",
        mob.subscribeMemberEvents("reviewer-1"),
      );
      try {
        const reviewerReceipt = await withStepTimeout(
          scenario,
          "send reviewer ready turn",
          mob.member("reviewer-1").send(
          "Repeat the swarm marker and say reviewer ready.",
          ),
        );
        assert.equal(reviewerReceipt.agentIdentity, "reviewer-1");
        assert.equal(reviewerReceipt.agentRuntimeId, reviewer.agentRuntimeId);
        assert.equal(reviewerReceipt.fenceToken, reviewer.fenceToken);
        const iterator = subscription[Symbol.asyncIterator]();
        let firstEventTimer = null;
        const firstEvent = await withStepTimeout(scenario, "receive first reviewer event", Promise.race([
          iterator.next(),
          new Promise((_, reject) => {
            firstEventTimer = setTimeout(
              () => reject(new Error("timed out waiting for reviewer event")),
              60000,
            );
          }),
        ]).finally(() => {
          if (firstEventTimer !== null) {
            clearTimeout(firstEventTimer);
          }
        }));
        assert.equal(firstEvent.done, false);
      } finally {
        await withStepTimeout(scenario, "close reviewer member event subscription", subscription.close());
      }

      const reviewerState = await waitFor(
        async () => withStepTimeout(scenario, "poll reviewer member status", mob.memberStatus("reviewer-1")),
        (state) => (state.outputPreview || "").toLowerCase().includes("reviewer ready"),
        { timeoutMs: 120000, intervalMs: 500 },
      );
      const reviewerText = (reviewerState.outputPreview || "").toLowerCase();
      assert.ok(reviewerText.includes("reviewer ready"));
      assert.ok(reviewerText.includes("ts-swarm"));

      const members = await withStepTimeout(scenario, "list members before respawn", mob.listMembers());
      const memberIds = members.map((member) => member.agentIdentity);
      assert.ok(memberIds.includes("lead-1"));
      assert.ok(memberIds.includes("reviewer-1"));

      const respawn = await withStepTimeout(scenario, "respawn reviewer", mob.respawn(
        "reviewer-1",
        "Come back online and say REVIEWER_RESPAWN_44.",
      ));
      assert.equal(respawn.receipt.agentIdentity, "reviewer-1");
      assert.ok(respawn.receipt.agentRuntimeId);
      const membersAfterRespawn = await waitFor(
        async () => withStepTimeout(scenario, "poll members after respawn", mob.listMembers()),
        (items) => items.some(
          (member) =>
            member.agentIdentity === "reviewer-1"
            && member.agentRuntimeId === respawn.receipt.agentRuntimeId
            && member.fenceToken === respawn.receipt.fenceToken
            && member.state === "Active",
        ),
        { timeoutMs: 60000, intervalMs: 200 },
      );
      assert.ok(membersAfterRespawn.some((member) => member.agentIdentity === "reviewer-1"));
      const respawnedReviewer = membersAfterRespawn.find(
        (member) => member.agentIdentity === "reviewer-1",
      );
      assert.ok(respawnedReviewer?.agentRuntimeId);
      const respawnReceipt = await withStepTimeout(
        scenario,
        "send respawn reviewer turn",
        mob.member("reviewer-1").send(
        "Reply with REVIEWER_RESPAWN_44.",
        ),
      );
      assert.equal(respawnReceipt.agentIdentity, "reviewer-1");
      assert.equal(respawnReceipt.agentRuntimeId, respawn.receipt.agentRuntimeId);
      assert.equal(respawnReceipt.fenceToken, respawn.receipt.fenceToken);
      const respawnedState = await waitFor(
        async () => withStepTimeout(scenario, "poll reviewer status after respawn send", mob.memberStatus("reviewer-1")),
        (state) => (state.outputPreview || "").toLowerCase().includes("reviewer_respawn_44"),
        { timeoutMs: 120000, intervalMs: 500 },
      );
      assert.ok((respawnedState.outputPreview || "").toLowerCase().includes("reviewer_respawn_44"));

      await withStepTimeout(scenario, "retire reviewer", mob.retire("reviewer-1"));
      const membersAfterRetire = await waitFor(
        async () => withStepTimeout(scenario, "poll members after retire", mob.listMembers()),
        (items) => items.every((member) => member.agentIdentity !== "reviewer-1"),
        { timeoutMs: 60000, intervalMs: 200 },
      );
      assert.ok(membersAfterRetire.every((member) => member.agentIdentity !== "reviewer-1"));

      try {
        const broken = await withStepTimeout(scenario, "spawn broken member", mob.spawn({
          profile: "broken",
          agentIdentity: "broken-1",
          runtimeMode: "turn_driven",
        }));
        assert.ok(broken.agentRuntimeId);
        await assert.rejects(
          () => withStepTimeout(
            scenario,
            "send broken member turn",
            mob.member("broken-1").send(
            "This turn must fail because the member model is invalid.",
          ),
          ),
        );
      } catch (error) {
        assert.match(String(error), /definitely-invalid-live-smoke-model/);
      }
      },
    );
  }

  if (includeScenario(59)) {
    it(
      "Scenario 59: realtime channel session exchange through the packaged SDK",
      { skip: !hasAnthropicKey() },
      async () => withoutOpenAiRealtimeEnv(async () => {
      const scenario = "Scenario 59";
      const client = await withStepTimeout(
        scenario,
        "connect isolated client",
        connectClient({ isolated: true }),
      );

      const session = await withStepTimeout(
        scenario,
        "create session",
        client.createSession(
        "When asked through realtime, reply with TS-REALTIME-59 and mention cedar.",
        {
          model: anthropicModel(),
          provider: "anthropic",
        },
        ),
      );
      assert.ok(session.id);

      const channel = RealtimeChannel.session(client, session.id);
      const openInfo = await withStepTimeout(
        scenario,
        "request realtime open info",
        channel.openInfo(),
      );
      assert.match(openInfo.ws_url, /^ws:\/\//);
      assert.ok(openInfo.default_protocol_version);

      const connection = await withStepTimeout(
        scenario,
        "connect realtime websocket",
        channel.connect(),
      );
      await withStepTimeout(
        scenario,
        "send realtime text chunk",
        connection.sendInput({
          kind: "text_chunk",
          text: "Reply with TS-REALTIME-59 and the word cedar.",
        }),
      );

      const frames = [];
      await withStepTimeout(scenario, "receive realtime turn commit", (async () => {
        while (true) {
          const frame = await connection.nextFrame();
          assert.notEqual(frame, null, "realtime websocket closed before turn commit");
          if (frame.type === "channel.error") {
            throw new Error(`realtime channel error: ${JSON.stringify(frame)}`);
          }
          frames.push(frame);
          if (frame.type === "channel.event" && frame.event?.type === "turn_committed") {
            break;
          }
        }
      })());

      const eventTypes = frames
        .filter((frame) => frame.type === "channel.event" && frame.event?.type)
        .map((frame) => frame.event.type);
      assert.deepEqual(eventTypes.slice(0, 4), [
        "turn_started",
        "input_transcript_partial",
        "input_transcript_final",
        "turn_committed",
      ]);

      const sessionState = await waitFor(
        async () => withStepTimeout(scenario, "poll session after realtime turn", client.readSession(session.id)),
        (state) => (state.lastAssistantText || "").toLowerCase().includes("ts-realtime-59"),
        { timeoutMs: 120000, intervalMs: 500 },
      );
      const lastText = (sessionState.lastAssistantText || "").toLowerCase();
      assert.ok(lastText.includes("ts-realtime-59"));
      assert.ok(lastText.includes("cedar"));

      const history = await withStepTimeout(
        scenario,
        "read session history",
        client.readSessionHistory(session.id),
      );
      assert.ok(history.messages.some(
        (message) =>
          message.role === "user"
          && typeof message.content === "string"
          && message.content.includes("Reply with TS-REALTIME-59 and the word cedar."),
      ));

      await withStepTimeout(scenario, "close realtime channel", connection.close());
      const closed = await withStepTimeout(scenario, "wait for realtime close", (async () => {
        while (true) {
          const frame = await connection.nextFrame();
          if (frame?.type === "channel.closed") {
            return frame;
          }
        }
      })());
      assert.equal(closed.type, "channel.closed");
      }),
    );
  }
});
