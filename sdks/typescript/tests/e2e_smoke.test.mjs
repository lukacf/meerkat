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
  (() => {
    try {
      const envOutput = execSync("./scripts/repo-cargo --print-env", {
        cwd: workspaceRoot,
        encoding: "utf8",
      });
      const targetDir = envOutput
        .split("\n")
        .find((line) => line.startsWith("CARGO_TARGET_DIR="))
        ?.slice("CARGO_TARGET_DIR=".length);
      return targetDir ? path.join(targetDir, "debug", "rkat-rpc") : "";
    } catch {
      return "";
    }
  })(),
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

function hasGeminiKey() {
  return Boolean(
    process.env.RKAT_GEMINI_API_KEY ||
      process.env.GEMINI_API_KEY ||
      process.env.GOOGLE_API_KEY,
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
  // Default to the current approved OpenAI smoke model per CLAUDE.md.
  return process.env.SMOKE_MODEL_OPENAI || "gpt-5.4-mini";
}

function openaiStressModel() {
  return process.env.SMOKE_MODEL_OPENAI_STRESS || "gpt-5.5";
}

function openaiImageModel() {
  return process.env.SMOKE_IMAGE_MODEL_OPENAI || "gpt-image-2";
}

function geminiModel() {
  return process.env.SMOKE_MODEL_GEMINI || "gemini-3.1-pro-preview";
}

function geminiImageModel() {
  return process.env.SMOKE_IMAGE_MODEL_GEMINI || "gemini-3.1-flash-image-preview";
}

function includeScenario(id) {
  const filter = process.env.MEERKAT_TS_SMOKE_SCENARIOS;
  if (!filter) {
    const extendedImageScenarios = new Set([79, 80, 81, 82]);
    if (extendedImageScenarios.has(id)) {
      return process.env.MEERKAT_TS_EXTENDED_IMAGE_SMOKE === "1";
    }
    return true;
  }
  return filter
    .split(",")
    .map((part) => part.trim())
    .filter(Boolean)
    .includes(String(id));
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

function assistantImageBlocks(history) {
  return history.messages.flatMap((message) =>
    message.role === "assistant" || message.role === "block_assistant"
      ? message.blocks.filter((block) => block.blockType === "image")
      : [],
  );
}

async function assertFetchableImageBlob(client, block) {
  assert.ok(block.imageId, "assistant image block should expose imageId");
  assert.ok(block.blobId, "assistant image block should expose blobId");
  assert.match(block.mediaType || "", /^image\//);
  const payload = await client.getBlob(block.blobId);
  assert.equal(payload.blobId, block.blobId);
  assert.match(payload.mediaType, /^image\//);
  assert.ok(payload.dataBase64.length > 1024);
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

  async function withScenarioRetry(scenario, run, attempts = 2) {
    let lastError;
    for (let attempt = 1; attempt <= attempts; attempt += 1) {
      try {
        if (attempt > 1) {
          logScenarioStep(scenario, `retry attempt ${attempt}`);
        }
        return await run();
      } catch (error) {
        lastError = error;
        if (attempt >= attempts) {
          break;
        }
        logScenarioStep(scenario, `retrying after failure: ${String(error)}`);
        await Promise.all(
          clients.splice(0).map(async (client) => {
            await client.close().catch(() => {});
          }),
        );
        await delay(1000);
      }
    }
    throw lastError;
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
      await withScenarioRetry(scenario, async () => {
      const client = await withStepTimeout(
        scenario,
        "connect client",
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
      assert.ok(lead.memberRef);
      assert.equal(reviewer.agentIdentity, "reviewer-1");
      assert.ok(reviewer.memberRef);

      await withStepTimeout(scenario, "wire lead -> reviewer", mob.wire("lead-1", "reviewer-1"));
      const append = await withStepTimeout(scenario, "append reviewer system context", mob.appendSystemContext(
        "reviewer-1",
        "Remember the swarm marker SWARM_MARKER_7F3X2A.",
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
        assert.equal(reviewerReceipt.memberRef, reviewer.memberRef);
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
      // Test-support instrumentation: surface the raw reviewer output
      // when the marker assertion would fail, so post-hoc log inspection
      // does not require a re-run. The assertions below remain untouched
      // (test semantics unchanged).
      if (!reviewerText.includes("swarm_marker_7f3x2a") || !reviewerText.includes("reviewer ready")) {
        console.error(
          `[${scenario}] reviewer output_preview (raw): ${JSON.stringify(reviewerState.outputPreview)}`,
        );
        console.error(
          `[${scenario}] reviewer output_preview (lowercased): ${JSON.stringify(reviewerText)}`,
        );
      }
      assert.ok(reviewerText.includes("reviewer ready"));
      assert.ok(reviewerText.includes("swarm_marker_7f3x2a"));

      const members = await withStepTimeout(scenario, "list members before respawn", mob.listMembers());
      const memberIds = members.map((member) => member.agentIdentity);
      assert.ok(memberIds.includes("lead-1"));
      assert.ok(memberIds.includes("reviewer-1"));

      const respawn = await withStepTimeout(scenario, "respawn reviewer", mob.respawn(
        "reviewer-1",
        "Come back online and say REVIEWER_RESPAWN_44.",
      ));
      assert.equal(respawn.receipt.agentIdentity, "reviewer-1");
      assert.ok(respawn.receipt.memberRef);
      const membersAfterRespawn = await waitFor(
        async () => withStepTimeout(scenario, "poll members after respawn", mob.listMembers()),
        (items) => items.some(
          (member) =>
            member.agentIdentity === "reviewer-1"
            && member.memberRef === respawn.receipt.memberRef
            && member.state === "active",
        ),
        { timeoutMs: 60000, intervalMs: 200 },
      );
      assert.ok(membersAfterRespawn.some((member) => member.agentIdentity === "reviewer-1"));
      const respawnedReviewer = membersAfterRespawn.find(
        (member) => member.agentIdentity === "reviewer-1",
      );
      assert.ok(respawnedReviewer?.memberRef);
      const respawnReceipt = await withStepTimeout(
        scenario,
        "send respawn reviewer turn",
        mob.member("reviewer-1").send(
        "Reply with REVIEWER_RESPAWN_44.",
        ),
      );
      assert.equal(respawnReceipt.agentIdentity, "reviewer-1");
      assert.equal(respawnReceipt.memberRef, respawn.receipt.memberRef);
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
        assert.ok(broken.memberRef);
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
      });
      },
    );
  }

  if (includeScenario(59)) {
    it(
      "Scenario 59: realtime channel session exchange through the packaged SDK",
      { skip: !hasAnthropicKey() },
      async () => {
      const scenario = "Scenario 59";
      const client = await withStepTimeout(
        scenario,
        "connect client",
        connectClient({ realmId: "env_default" }),
      );

      const session = await withStepTimeout(
        scenario,
        "create session",
        client.createSession(
        "When asked through realtime, reply with TS-REALTIME-59 and mention cedar.",
        {
          model: "gpt-realtime-1.5",
          provider: "openai",
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
      },
    );
  }

  if (includeScenario(75)) {
    it(
      "Scenario 75: OpenAI image generation provider params through the packaged SDK",
      { skip: !(hasAnthropicKey() && hasOpenAIKey()) },
      async () => {
      const scenario = "Scenario 75";
      const client = await withStepTimeout(
        scenario,
        "connect client",
        connectClient({ realmId: "env_default" }),
      );

      const session = await withStepTimeout(
        scenario,
        "create OpenAI image session",
        client.createSession(
          `Use the generate_image tool exactly once. Pass request.provider="openai",
request.model="${openaiImageModel()}", request.intent="generate",
request.prompt="A clean white placard with TS-OPENAI-75 written in large blue letters",
request.size="1024x1024", request.quality="low", request.format="webp",
request.count=1, and request.provider_params={"background":"opaque","moderation":"low","action":"generate"}.
After the tool returns, reply with TS-OPENAI-75-DONE and no extra prose.`,
          {
            model: anthropicModel(),
            provider: "anthropic",
            enableBuiltins: true,
          },
        ),
        180000,
      );
      assert.match(session.text.toLowerCase(), /ts-openai-75/);

      const history = await withStepTimeout(
        scenario,
        "read session history",
        client.readSessionHistory(session.id),
      );
      const images = assistantImageBlocks(history);
      assert.ok(images.length >= 1, "OpenAI image generation should append an assistant image block");
      await withStepTimeout(
        scenario,
        "fetch generated image blob",
        assertFetchableImageBlob(client, images.at(-1)),
      );
      },
    );
  }

  if (includeScenario(76)) {
    it(
      "Scenario 76: cross-provider image generation and model-switch stress",
      { skip: !(hasOpenAIKey() && hasGeminiKey()) },
      async () => {
      const scenario = "Scenario 76";
      await withScenarioRetry(scenario, async () => {
      const client = await withStepTimeout(
        scenario,
        "connect client",
        connectClient({ realmId: "env_default" }),
      );

      const initial = await withStepTimeout(
        scenario,
        "OpenAI turn delegates image generation to Gemini",
        client.createSession(
          `Use the generate_image tool exactly once. You are an OpenAI model, but the image target must be Gemini.
Pass request.provider="gemini", request.model="${geminiImageModel()}", request.intent="generate",
request.prompt="A flat white sign with the single word Hello centered in large crisp black letters",
request.size="1536x1024", request.quality="auto", request.format="png", request.count=1,
and request.provider_params={"aspect_ratio":"16:9","image_size":"1K"}.
After the tool returns, reply with CROSS-IMAGE-76-FIRST and no extra prose.`,
          {
            model: openaiStressModel(),
            provider: "openai",
            enableBuiltins: true,
          },
        ),
        240000,
      );
      assert.match(initial.text.toLowerCase(), /cross-image-76-first/);

      const firstHistory = await withStepTimeout(
        scenario,
        "read first image history",
        client.readSessionHistory(initial.id),
      );
      const firstImages = assistantImageBlocks(firstHistory);
      assert.ok(firstImages.length >= 1, "first turn should commit a Gemini image block");
      const firstImage = firstImages.at(-1);
      await withStepTimeout(
        scenario,
        "fetch first image blob",
        assertFetchableImageBlob(client, firstImage),
      );

      const edit = await withStepTimeout(
        scenario,
        "Gemini turn edits previous generated image",
        initial.turn(
          `Switch to Gemini now and use generate_image exactly once to edit the previous assistant image.
Pass request.provider="gemini", request.model="${geminiImageModel()}", request.intent="edit",
request.source_images=[{"kind":"assistant_image","image_id":"${firstImage.imageId}"}],
request.instruction="Modify only the sign text so it says Hello Meerkat in large crisp black letters",
request.size="1536x1024", request.quality="auto", request.format="png", request.count=1,
and request.provider_params={"aspect_ratio":"16:9","image_size":"1K"}.
After the tool returns, reply with CROSS-IMAGE-76-EDITED and no extra prose.`,
          {
            model: geminiModel(),
            provider: "gemini",
            enableBuiltins: true,
          },
        ),
        240000,
      );
      assert.match(edit.text.toLowerCase(), /cross-image-76-edited/);

      const editHistory = await withStepTimeout(
        scenario,
        "read edited image history",
        client.readSessionHistory(initial.id),
      );
      const editedImages = assistantImageBlocks(editHistory);
      assert.ok(editedImages.length >= firstImages.length + 1, "edit turn should commit another image block");
      const editedImage = editedImages.at(-1);
      assert.notEqual(editedImage.imageId, firstImage.imageId);
      await withStepTimeout(
        scenario,
        "fetch edited image blob",
        assertFetchableImageBlob(client, editedImage),
      );

      const description = await withStepTimeout(
        scenario,
        "OpenAI turn describes latest generated image",
        initial.turn(
          `Switch back to OpenAI. Inspect the latest assistant image and describe the visible text.
Reply with CROSS-IMAGE-76-DESCRIBE and a short phrase containing the text you can read.`,
          {
            model: openaiStressModel(),
            provider: "openai",
            enableBuiltins: true,
          },
        ),
        240000,
      );
      const descriptionText = description.text.toLowerCase();
      assert.match(descriptionText, /cross-image-76-describe/);
      assert.ok(
        descriptionText.includes("hello") || descriptionText.includes("meerkat"),
        `expected OpenAI to read some text from the final image, got: ${description.text}`,
      );
      });
      },
    );
  }

  if (includeScenario(77)) {
    it(
      "Scenario 77: stacked generate and edit calls in one assistant turn",
      { skip: !(hasAnthropicKey() && hasGeminiKey()) },
      async () => {
      const scenario = "Scenario 77";
      const client = await withStepTimeout(
        scenario,
        "connect client",
        connectClient({ realmId: "env_default" }),
      );

      const session = await withStepTimeout(
        scenario,
        "run stacked image tool turn",
        client.createSession(
          `Complete this as a single assistant turn. Before your final answer, call generate_image twice in sequence.
First call: request.provider="gemini", request.model="${geminiImageModel()}", request.intent="generate",
request.prompt="A neat museum placard portrait of Johann Sebastian Bach beside a pipe organ, with the text BACH-77 visible",
request.size="1536x1024", request.quality="auto", request.format="png", request.count=1,
and request.provider_params={"aspect_ratio":"16:9","image_size":"1K"}.
Second call: request.provider="gemini", request.model="${geminiImageModel()}", request.intent="edit",
use the assistant image returned by the first call as request.source_images, and transform it into a bright yellow
animated sitcom-style composer portrait with BACH-77 still visible.
After both tool calls finish, describe the second image in detail and rate it on a 1-10 Simpson-like scale.
Your final reply must include STACKED-IMAGE-77-DONE.`,
          {
            model: anthropicModel(),
            provider: "anthropic",
            enableBuiltins: true,
          },
        ),
        420000,
      );
      assert.match(session.text.toLowerCase(), /stacked-image-77-done/);

      const history = await withStepTimeout(
        scenario,
        "read stacked image history",
        client.readSessionHistory(session.id),
      );
      const images = assistantImageBlocks(history);
      assert.ok(images.length >= 2, "stacked turn should commit two assistant image blocks");
      const firstImage = images.at(-2);
      const secondImage = images.at(-1);
      assert.notEqual(secondImage.imageId, firstImage.imageId);
      await withStepTimeout(scenario, "fetch first stacked image", assertFetchableImageBlob(client, firstImage));
      await withStepTimeout(scenario, "fetch second stacked image", assertFetchableImageBlob(client, secondImage));
      },
    );
  }

  if (includeScenario(78)) {
    it(
      "Scenario 78: cross-provider image relay with later visual readback",
      { skip: !(hasOpenAIKey() && hasGeminiKey()) },
      async () => {
      const scenario = "Scenario 78";
      const client = await withStepTimeout(
        scenario,
        "connect client",
        connectClient({ realmId: "env_default" }),
      );

      const generated = await withStepTimeout(
        scenario,
        "OpenAI session routes image generation to Gemini",
        client.createSession(
          `Use generate_image exactly once with request.provider="gemini", request.model="${geminiImageModel()}",
request.intent="generate", request.prompt="A clean product sketch of a tiny glass terrarium containing a lighthouse, with RELAY-78 printed on the base",
request.size="1536x1024", request.quality="auto", request.format="png", request.count=1,
and request.provider_params={"aspect_ratio":"16:9","image_size":"1K"}.
After the image is generated, reply with CROSS-RELAY-78-GENERATED and no extra prose.`,
          {
            model: openaiStressModel(),
            provider: "openai",
            enableBuiltins: true,
          },
        ),
        300000,
      );
      assert.match(generated.text.toLowerCase(), /cross-relay-78-generated/);

      const history = await withStepTimeout(
        scenario,
        "read relay image history",
        client.readSessionHistory(generated.id),
      );
      const images = assistantImageBlocks(history);
      assert.ok(images.length >= 1, "cross-provider relay should commit an image");
      await withStepTimeout(scenario, "fetch relay image", assertFetchableImageBlob(client, images.at(-1)));

      const readback = await withStepTimeout(
        scenario,
        "Gemini model reads prior generated image",
        generated.turn(
          "Switch to Gemini. Inspect the previous assistant image and reply with CROSS-RELAY-78-READBACK plus the main object you see.",
          {
            model: geminiModel(),
            provider: "gemini",
            enableBuiltins: true,
          },
        ),
        240000,
      );
      assert.match(readback.text.toLowerCase(), /cross-relay-78-readback/);
      assert.match(readback.text.toLowerCase(), /terrarium|lighthouse|glass/);
      },
    );
  }

  if (includeScenario(79)) {
    it(
      "Scenario 79: mob member image generation with critic handoff",
      { skip: !(hasAnthropicKey() && hasOpenAIKey()) },
      async () => {
      const scenario = "Scenario 79";
      const client = await withStepTimeout(
        scenario,
        "connect client",
        connectClient({ realmId: "env_default" }),
      );
      const mobId = `ts-image-critic-${Date.now()}`;
      const mob = await withStepTimeout(scenario, "create image critic mob", client.createMob({
        definition: {
          id: mobId,
          orchestrator: { profile: "maker" },
          profiles: {
            maker: {
              model: anthropicModel(),
              tools: { builtins: true, comms: true },
              peer_description: "Image maker that can create and summarize generated images",
              external_addressable: true,
            },
            critic: {
              model: openaiModel(),
              tools: { comms: true },
              peer_description: "Art critic that reviews image descriptions",
              external_addressable: true,
            },
          },
        },
      }));

      await withStepTimeout(scenario, "spawn critic", mob.spawn({
        profile: "critic",
        agentIdentity: "critic-1",
        runtimeMode: "turn_driven",
        initialMessage: "Stand by as an art critic. Reply CRITIC-79-READY.",
      }));
      await withStepTimeout(scenario, "spawn maker", mob.spawn({
        profile: "maker",
        agentIdentity: "maker-1",
        runtimeMode: "turn_driven",
      }));
      await withStepTimeout(scenario, "wire maker to critic", mob.wire("maker-1", "critic-1"));

      await withStepTimeout(
        scenario,
        "send maker image task",
        mob.member("maker-1").send(
          `Use generate_image exactly once with request.provider="openai", request.model="${openaiImageModel()}",
request.intent="generate", request.prompt="A gallery postcard showing a blue teapot orbiting a small moon, with ART-CRITIC-79 on the border",
request.size="1024x1024", request.quality="low", request.format="webp", request.count=1,
and request.provider_params={"background":"opaque","moderation":"low","action":"generate"}.
After it returns, summarize the image in one sentence and include ART-CRITIC-79-MAKER.`,
        ),
        180000,
      );
      const makerState = await waitFor(
        async () => withStepTimeout(scenario, "poll maker image status", mob.memberStatus("maker-1")),
        (state) => (state.outputPreview || "").toLowerCase().includes("art-critic-79-maker"),
        { timeoutMs: 300000, intervalMs: 1000 },
      );
      assert.match((makerState.outputPreview || "").toLowerCase(), /art-critic-79-maker/);

      await withStepTimeout(
        scenario,
        "send critic handoff",
        mob.member("critic-1").send(
          `Review this maker summary as an art critic and reply with ART-CRITIC-79-CRITIQUE: ${makerState.outputPreview}`,
        ),
        120000,
      );
      const criticState = await waitFor(
        async () => withStepTimeout(scenario, "poll critic critique status", mob.memberStatus("critic-1")),
        (state) => (state.outputPreview || "").toLowerCase().includes("art-critic-79-critique"),
        { timeoutMs: 180000, intervalMs: 1000 },
      );
      assert.match((criticState.outputPreview || "").toLowerCase(), /art-critic-79-critique/);
      },
    );
  }

  if (includeScenario(80)) {
    it(
      "Scenario 80: persisted generated image survives reconnect and resume",
      { skip: !(hasAnthropicKey() && hasGeminiKey()) },
      async () => {
      const scenario = "Scenario 80";
      const stateRoot = mkdtempSync(path.join(os.tmpdir(), "ts-image-resume-"));
      const realmId = `ts-image-resume-${Date.now()}`;

      const clientA = await connectClient({
        realmId,
        stateRoot,
        realmBackend: "sqlite",
      });
      const session = await withStepTimeout(
        scenario,
        "create persisted image session",
        clientA.createSession(
          `Use generate_image exactly once with request.provider="gemini", request.model="${geminiImageModel()}",
request.intent="generate", request.prompt="A durable archive card with RESUME-80 printed in green ink beside a small brass key",
request.size="1536x1024", request.quality="auto", request.format="png", request.count=1,
and request.provider_params={"aspect_ratio":"16:9","image_size":"1K"}.
After the image is generated, reply with RESUME-80-GENERATED.`,
          {
            model: anthropicModel(),
            provider: "anthropic",
            enableBuiltins: true,
          },
        ),
        300000,
      );
      const sessionId = session.id;
      const firstHistory = await withStepTimeout(
        scenario,
        "read persisted image history before reconnect",
        clientA.readSessionHistory(sessionId),
      );
      const firstImages = assistantImageBlocks(firstHistory);
      assert.ok(firstImages.length >= 1, "persisted session should have an image before reconnect");
      const persistedImage = firstImages.at(-1);
      await withStepTimeout(scenario, "fetch persisted image before reconnect", assertFetchableImageBlob(clientA, persistedImage));
      await clientA.close();

      const clientB = await connectClient({
        realmId,
        stateRoot,
        realmBackend: "sqlite",
      });
      const secondHistory = await withStepTimeout(
        scenario,
        "read persisted image history after reconnect",
        clientB.readSessionHistory(sessionId),
      );
      const secondImages = assistantImageBlocks(secondHistory);
      assert.ok(secondImages.some((block) => block.imageId === persistedImage.imageId));
      await withStepTimeout(scenario, "fetch persisted image after reconnect", assertFetchableImageBlob(clientB, persistedImage));

      const resumed = await withStepTimeout(
        scenario,
        "resume session and inspect persisted image",
        clientB._startTurn(
          sessionId,
          "Inspect the previous assistant image and reply with RESUME-80-READBACK plus the object printed or shown on it.",
        ),
        240000,
      );
      assert.match(resumed.text.toLowerCase(), /resume-80-readback/);
      assert.match(resumed.text.toLowerCase(), /key|archive|card|resume/);
      },
    );
  }

  if (includeScenario(81)) {
    it(
      "Scenario 81: parallel image storm across independent sessions",
      { skip: !(hasAnthropicKey() && hasOpenAIKey()) },
      async () => {
      const scenario = "Scenario 81";
      const client = await withStepTimeout(
        scenario,
        "connect client",
        connectClient(),
      );
      const markers = ["STORM-81-A", "STORM-81-B", "STORM-81-C"];

      const sessions = await withStepTimeout(
        scenario,
        "run parallel image sessions",
        Promise.all(markers.map((marker) =>
          client.createSession(
            `Use generate_image exactly once with request.provider="openai", request.model="${openaiImageModel()}",
request.intent="generate", request.prompt="A minimal square badge with ${marker} written in crisp black letters",
request.size="1024x1024", request.quality="low", request.format="webp", request.count=1,
and request.provider_params={"background":"opaque","moderation":"low","action":"generate"}.
After the image is generated, reply with ${marker}-DONE and no extra prose.`,
            {
              model: anthropicModel(),
              provider: "anthropic",
              enableBuiltins: true,
            },
          )
        )),
        420000,
      );

      for (let index = 0; index < sessions.length; index += 1) {
        const marker = markers[index].toLowerCase();
        assert.match(sessions[index].text.toLowerCase(), new RegExp(`${marker}-done`));
        const history = await withStepTimeout(
          scenario,
          `read storm history ${markers[index]}`,
          client.readSessionHistory(sessions[index].id),
        );
        const images = assistantImageBlocks(history);
        assert.ok(images.length >= 1, `${markers[index]} should commit an image`);
        await withStepTimeout(
          scenario,
          `fetch storm image ${markers[index]}`,
          assertFetchableImageBlob(client, images.at(-1)),
        );
      }
      },
    );
  }

  if (includeScenario(82)) {
    it(
      "Scenario 82: SDK blob image roundtrip as fresh user input",
      { skip: !(hasAnthropicKey() && hasOpenAIKey()) },
      async () => {
      const scenario = "Scenario 82";
      const client = await withStepTimeout(
        scenario,
        "connect client",
        connectClient(),
      );

      const generated = await withStepTimeout(
        scenario,
        "generate SDK roundtrip image",
        client.createSession(
          `Use generate_image exactly once with request.provider="openai", request.model="${openaiImageModel()}",
request.intent="generate", request.prompt="A simple postcard with SDK-ROUNDTRIP-82 printed above a red compass",
request.size="1024x1024", request.quality="low", request.format="webp", request.count=1,
and request.provider_params={"background":"opaque","moderation":"low","action":"generate"}.
After the image is generated, reply with SDK-ROUNDTRIP-82-GENERATED.`,
          {
            model: anthropicModel(),
            provider: "anthropic",
            enableBuiltins: true,
          },
        ),
        300000,
      );
      const history = await withStepTimeout(
        scenario,
        "read SDK roundtrip image history",
        client.readSessionHistory(generated.id),
      );
      const images = assistantImageBlocks(history);
      assert.ok(images.length >= 1, "SDK roundtrip source image should exist");
      const image = images.at(-1);
      await withStepTimeout(scenario, "fetch SDK roundtrip blob", assertFetchableImageBlob(client, image));

      const described = await withStepTimeout(
        scenario,
        "send blob-backed image as new SDK input",
        client.createSession(
          [
            { type: "text", text: "Inspect this blob-backed image and reply with SDK-ROUNDTRIP-82-READ plus the main object." },
            {
              type: "image",
              source: "blob",
              media_type: image.mediaType,
              blob_id: image.blobId,
            },
          ],
          {
            model: openaiStressModel(),
            provider: "openai",
          },
        ),
        240000,
      );
      assert.match(described.text.toLowerCase(), /sdk-roundtrip-82-read/);
      assert.match(described.text.toLowerCase(), /compass|postcard|sdk/);
      },
    );
  }
});
