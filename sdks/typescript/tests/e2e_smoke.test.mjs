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
  const selected = process.env.LIVE_SMOKE_SCENARIO;
  return !selected || selected === String(id);
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
  const clients = [];

  before(async () => {
    ({ MeerkatClient } = await import("../dist/index.js"));
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
      assert.equal(details.session_id, session.id);
      assert.ok(Number(details.message_count) >= 4);
      assert.equal(details.is_active, false);

      const sessions = await client.listSessions();
      assert.ok(sessions.some((entry) => entry.sessionId === session.id));

      await session.archive();
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

      const injected = await client.request("session/inject_context", {
        session_id: deferred.id,
        text: "Always include the marker [TS-SDK-CTX] in your replies.",
        source: "typescript-smoke",
      });
      assert.ok(["Staged", "staged", "Duplicate", "duplicate"].includes(String(injected.status)));

      const first = await deferred.startTurn(
        "What is the codeword? Include any markers you were told about.",
      );
      const firstTextLower = first.text.toLowerCase();
      assert.ok(firstTextLower.includes("orbit-7") || firstTextLower.includes("orbit 7"));
      assert.ok(firstTextLower.includes("ts-sdk-ctx"));

      const stream = client._startTurnStreaming(
        deferred.id,
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
        realmBackend: "redb",
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
        realmBackend: "redb",
      });
      const details = await clientB.readSession(sessionId);
      assert.equal(details.session_id, sessionId);
      assert.ok(Number(details.message_count) >= 2);

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
      const client = await connectClient({ isolated: true });
      const mob = await client.createMob({
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
      });

      const lead = await mob.spawn({
        profile: "lead",
        meerkatId: "lead-1",
        initialMessage: "Acknowledge the lead role in one sentence.",
        runtimeMode: "autonomous_host",
      });
      const reviewer = await mob.spawn({
        profile: "reviewer",
        meerkatId: "reviewer-1",
        initialMessage: "Acknowledge the reviewer role in one sentence.",
        runtimeMode: "turn_driven",
      });
      assert.ok(lead.session_id);
      assert.ok(reviewer.session_id);

      await mob.wire("lead-1", "reviewer-1");
      const append = await mob.appendSystemContext(
        "reviewer-1",
        "Remember the swarm marker [TS-SWARM].",
        { source: "typescript-sdk", idempotencyKey: "ts-swarm-marker" },
      );
      assert.ok(["Staged", "staged", "Duplicate", "duplicate"].includes(String(append.status)));

      const subscription = await mob.subscribeMemberEvents("reviewer-1");
      try {
        await mob.sendMessage(
          "reviewer-1",
          "Repeat the swarm marker and say reviewer ready.",
        );
        const iterator = subscription[Symbol.asyncIterator]();
        const firstEvent = await Promise.race([
          iterator.next(),
          new Promise((_, reject) =>
            setTimeout(() => reject(new Error("timed out waiting for reviewer event")), 60000),
          ),
        ]);
        assert.equal(firstEvent.done, false);
      } finally {
        await subscription.close();
      }

      const members = await mob.listMembers();
      const memberIds = members.map((member) => member.meerkatId);
      assert.ok(memberIds.includes("lead-1"));
      assert.ok(memberIds.includes("reviewer-1"));

      await waitFor(
        async () => client.request("runtime/state", { session_id: reviewer.session_id }),
        (payload) => payload.state === "idle",
        { timeoutMs: 120000, intervalMs: 200 },
      );

      await mob.respawn(
        "reviewer-1",
        "Come back online and say REVIEWER_RESPAWN_44.",
      );
      const membersAfterRespawn = await waitFor(
        async () => mob.listMembers(),
        (items) => items.some(
          (member) =>
            member.meerkatId === "reviewer-1"
            && member.state === "Active",
        ),
        { timeoutMs: 60000, intervalMs: 200 },
      );
      assert.ok(membersAfterRespawn.some((member) => member.meerkatId === "reviewer-1"));
      const respawnedReviewer = membersAfterRespawn.find(
        (member) => member.meerkatId === "reviewer-1",
      );
      assert.ok(respawnedReviewer?.sessionId);
      await waitFor(
        async () => client.request("runtime/state", { session_id: respawnedReviewer.sessionId }),
        (payload) => payload.state === "idle",
        { timeoutMs: 120000, intervalMs: 200 },
      );
      await mob.sendMessage(
        "reviewer-1",
        "Reply with REVIEWER_RESPAWN_44.",
      );
      await waitFor(
        async () => client.readSession(respawnedReviewer.sessionId),
        (state) => String(state.last_assistant_text ?? "").toLowerCase().includes("reviewer_respawn_44"),
        { timeoutMs: 120000, intervalMs: 200 },
      );

      await mob.retire("reviewer-1");
      const membersAfterRetire = await waitFor(
        async () => mob.listMembers(),
        (items) => items.every((member) => member.meerkatId !== "reviewer-1"),
        { timeoutMs: 60000, intervalMs: 200 },
      );
      assert.ok(membersAfterRetire.every((member) => member.meerkatId !== "reviewer-1"));

      try {
        const broken = await mob.spawn({
          profile: "broken",
          meerkatId: "broken-1",
          runtimeMode: "turn_driven",
        });
        assert.ok(broken.session_id);
        await assert.rejects(
          () => mob.sendMessage(
            "broken-1",
            "This turn must fail because the member model is invalid.",
          ),
        );
        const brokenState = await client.readSession(broken.session_id);
        assert.equal(String(brokenState.last_assistant_text ?? "").trim(), "");
      } catch (error) {
        assert.match(String(error), /definitely-invalid-live-smoke-model/);
      }
      },
    );
  }
});
