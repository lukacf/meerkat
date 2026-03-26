import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createServer } from "node:http";
import test from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

import { MeerkatRuntime } from "../dist/index.js";
import { createProxyHandler, corsHeaders } from "../proxy/index.mjs";
import * as rawWasm from "../wasm/meerkat_web_runtime.js";

function hasAnthropicKey() {
  return Boolean(process.env.RKAT_ANTHROPIC_API_KEY || process.env.ANTHROPIC_API_KEY);
}

function hasOpenAIKey() {
  return Boolean(process.env.RKAT_OPENAI_API_KEY || process.env.OPENAI_API_KEY);
}

function anthropicModel() {
  return process.env.SMOKE_MODEL_ANTHROPIC || process.env.SMOKE_MODEL || "claude-sonnet-4-5";
}

function openaiModel() {
  return process.env.SMOKE_MODEL_OPENAI || "gpt-4.1-mini";
}

async function makeNodeCompatibleWasmModule() {
  const wasmUrl = new URL("../wasm/meerkat_web_runtime_bg.wasm", import.meta.url);
  const wasmBytes = await readFile(fileURLToPath(wasmUrl));
  return {
    ...rawWasm,
    default: async () => rawWasm.default({ module_or_path: wasmBytes }),
  };
}

async function withProviderProxy(fn) {
  const handlers = {};
  if (hasAnthropicKey()) {
    handlers.anthropic = createProxyHandler("anthropic", {
      apiKey: process.env.RKAT_ANTHROPIC_API_KEY || process.env.ANTHROPIC_API_KEY,
    });
  }
  if (hasOpenAIKey()) {
    handlers.openai = createProxyHandler("openai", {
      apiKey: process.env.RKAT_OPENAI_API_KEY || process.env.OPENAI_API_KEY,
    });
  }
  assert.ok(
    Object.keys(handlers).length > 0,
    "at least one provider proxy must be configured",
  );

  const server = createServer(async (nodeReq, nodeRes) => {
    const cors = corsHeaders("*");
    try {
      if (nodeReq.method === "OPTIONS") {
        nodeRes.writeHead(204, cors);
        nodeRes.end();
        return;
      }

      const origin = `http://${nodeReq.headers.host || "127.0.0.1"}`;
      const url = new URL(nodeReq.url || "/", origin);
      const match = url.pathname.match(/^\/([^/]+)(\/.*)?$/);
      const provider = match?.[1];
      const strippedPath = match?.[2] || "/";
      const handler = provider ? handlers[provider] : undefined;

      if (!handler) {
        nodeRes.writeHead(404, { "content-type": "application/json", ...cors });
        nodeRes.end(JSON.stringify({ error: "not_found", provider }));
        return;
      }

      const headers = new Headers();
      for (let index = 0; index < nodeReq.rawHeaders.length; index += 2) {
        headers.append(nodeReq.rawHeaders[index], nodeReq.rawHeaders[index + 1]);
      }

      const webReq = new Request(url.toString(), {
        method: nodeReq.method,
        headers,
        body:
          nodeReq.method !== "GET" && nodeReq.method !== "HEAD" ? nodeReq : undefined,
        duplex: "half",
      });

      const upstream = await handler(webReq, strippedPath);
      const responseHeaders = {};
      for (const [key, value] of upstream.headers) {
        responseHeaders[key] = value;
      }
      nodeRes.writeHead(upstream.status, responseHeaders);
      if (upstream.body) {
        const reader = upstream.body.getReader();
        try {
          while (true) {
            const { done, value } = await reader.read();
            if (done) {
              break;
            }
            nodeRes.write(value);
          }
        } finally {
          reader.releaseLock();
        }
      }
      nodeRes.end();
    } catch (error) {
      nodeRes.writeHead(502, { "content-type": "application/json", ...cors });
      nodeRes.end(JSON.stringify({ error: "proxy_error", message: String(error) }));
    }
  });

  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  const baseUrls = {
    anthropicBaseUrl: handlers.anthropic ? `http://127.0.0.1:${port}/anthropic` : undefined,
    openaiBaseUrl: handlers.openai ? `http://127.0.0.1:${port}/openai` : undefined,
  };

  try {
    return await fn(baseUrls);
  } finally {
    await new Promise((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
}

async function withLiveRuntime(options, fn) {
  const { requireOpenAI = false } = options ?? {};
  return withProviderProxy(async (baseUrls) => {
    if (requireOpenAI) {
      assert.ok(baseUrls.openaiBaseUrl, "OpenAI proxy base URL must be available");
    }

    const wasm = await makeNodeCompatibleWasmModule();
    const runtime = await MeerkatRuntime.init(wasm, {
      anthropicApiKey: baseUrls.anthropicBaseUrl ? "proxy" : undefined,
      openaiApiKey: baseUrls.openaiBaseUrl ? "proxy" : undefined,
      anthropicBaseUrl: baseUrls.anthropicBaseUrl,
      openaiBaseUrl: baseUrls.openaiBaseUrl,
      model: anthropicModel(),
    });
    return fn(runtime, baseUrls);
  });
}

async function waitFor(description, fetch, predicate, { timeoutMs = 120000, intervalMs = 250 } = {}) {
  const deadline = Date.now() + timeoutMs;
  let lastValue;
  while (true) {
    lastValue = await fetch();
    if (predicate(lastValue)) {
      return lastValue;
    }
    if (Date.now() >= deadline) {
      throw new Error(`Timed out waiting for ${description}. Last value: ${JSON.stringify(lastValue)}`);
    }
    await delay(intervalMs);
  }
}

async function waitForSubscriptionItems(description, subscription, predicate, options) {
  return waitFor(
    description,
    async () => subscription.poll(),
    predicate,
    options,
  );
}

function eventType(item) {
  return item?.event?.type || item?.payload?.type || item?.type || "";
}

test(
  "Scenario 45: live WASM direct-session lifecycle and recall",
  { skip: !hasAnthropicKey() },
  async () => {
    await withLiveRuntime({}, async (runtime, baseUrls) => {
      const session = runtime.createSession({
        model: anthropicModel(),
        apiKey: "proxy",
        anthropicBaseUrl: baseUrls.anthropicBaseUrl,
      });

      const first = await session.turn(
        "My codename is WasmOtter45. Reply briefly and remember it.",
      );
      assert.ok(first.text.length > 0);

      const second = await session.turn(
        "What is my codename? Reply with just the codename.",
      );
      const lower = second.text.toLowerCase();
      assert.ok(lower.includes("wasmotter45") || lower.includes("wasm otter 45"));

      const state = session.getState();
      assert.equal(state.model, anthropicModel());
      assert.ok(state.session_id);

      session.destroy();
      assert.equal(session.isDestroyed, true);
    });
  },
);

test(
  "Scenario 46: live WASM system-context append and session event subscription",
  { skip: !hasAnthropicKey() },
  async () => {
    await withLiveRuntime({}, async (runtime, baseUrls) => {
      const session = runtime.createSession({
        model: anthropicModel(),
        apiKey: "proxy",
        anthropicBaseUrl: baseUrls.anthropicBaseUrl,
      });

      const staged = session.appendSystemContext({
        text: "Always include the marker [WASM-CTX-46] in your replies.",
        source: "web-live-smoke",
        idempotencyKey: "wasm-ctx-46",
      });
      assert.ok(["staged", "duplicate"].includes(staged.status));

      const subscription = session.subscribe();
      try {
        const turn = await session.turn(
          "Remember the marker ORBIT-46 and include any required markers in your reply.",
        );
        const lower = turn.text.toLowerCase();
        assert.ok(lower.includes("orbit-46") || lower.includes("orbit 46"));
        assert.ok(lower.includes("wasm-ctx-46"));

        const items = await waitForSubscriptionItems(
          "session subscription events",
          subscription,
          (events) => events.some((item) => ["text_delta", "turn_completed", "run_completed"].includes(eventType(item))),
          { timeoutMs: 30000, intervalMs: 100 },
        );
        assert.ok(items.length > 0);

        const followUp = await session.turn(
          "What marker was I asked to remember? Include the required marker.",
        );
        const followUpLower = followUp.text.toLowerCase();
        assert.ok(followUpLower.includes("orbit-46") || followUpLower.includes("orbit 46"));
        assert.ok(followUpLower.includes("wasm-ctx-46"));
      } finally {
        subscription.close();
        session.destroy();
      }
    });
  },
);

test(
  "Scenario 47: live WASM mixed-provider mob member turn and event stream",
  { skip: !(hasAnthropicKey() && hasOpenAIKey()) },
  async () => {
    await withLiveRuntime({ requireOpenAI: true }, async (runtime, baseUrls) => {
      const mob = await runtime.createMob({
        id: `wasm-mob-47-${Date.now()}`,
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
            peer_description: "Reviewer",
            external_addressable: true,
          },
          broken: {
            model: "definitely-invalid-live-smoke-model",
            tools: { comms: true },
            peer_description: "Deterministic failure worker",
            external_addressable: true,
          },
        },
        wiring: {
          auto_wire_orchestrator: false,
          role_wiring: [{ a: "lead", b: "reviewer" }],
        },
      });

      const spawned = await mob.spawn([
        {
          profile: "lead",
          meerkat_id: "lead-1",
          runtime_mode: "autonomous_host",
          initial_message: "Acknowledge the lead role in one sentence.",
        },
        {
          profile: "reviewer",
          meerkat_id: "reviewer-1",
          runtime_mode: "turn_driven",
          initial_message: "Acknowledge the reviewer role in one sentence.",
        },
      ]);
      assert.equal(spawned.length, 2);
      assert.ok(spawned.every((entry) => entry.status === "ok"));

      const reviewerSessionId = spawned[1].member_ref?.session_id;
      assert.ok(reviewerSessionId);

      await mob.wire("lead-1", "reviewer-1");
      const appended = await mob.appendSystemContext("reviewer-1", {
        text: "Remember the swarm marker [WASM-SWARM-47].",
        source: "web-live-smoke",
        idempotencyKey: "wasm-swarm-47",
      });
      assert.ok(["staged", "duplicate"].includes(appended.status));

      const subscription = await mob.member("reviewer-1").subscribe();
      try {
        await mob.member("reviewer-1").send(
          "Reply with REVIEWER_READY_47 and include [WASM-SWARM-47].",
        );
        const items = await waitForSubscriptionItems(
          "reviewer member events",
          subscription,
          (events) => events.some((item) => ["text_delta", "turn_completed", "run_completed"].includes(eventType(item))),
          { timeoutMs: 120000, intervalMs: 200 },
        );
        assert.ok(items.length > 0);
      } finally {
        subscription.close();
      }

      const members = await mob.listMembers();
      const reviewer = members.find((member) => member.meerkat_id === "reviewer-1");
      assert.ok(reviewer);
      assert.equal(reviewer.member_ref.session_id, reviewerSessionId);
      const reviewerSnapshot = await mob.memberStatus("reviewer-1");
      assert.equal(reviewerSnapshot.current_session_id, reviewerSessionId);

      const ledger = await mob.events("", 200);
      assert.ok(Array.isArray(ledger));
      assert.ok(ledger.length >= 1);

      const brokenSpawn = await mob.spawn([
        {
          profile: "broken",
          meerkat_id: "broken-1",
          runtime_mode: "turn_driven",
        },
      ]);
      assert.equal(brokenSpawn[0]?.status, "ok");
      await assert.rejects(
        () => mob.member("broken-1").send(
          "This turn must fail because the member model is invalid.",
        ),
      );
    });
  },
);

test(
  "Scenario 48: live WASM swarm lifecycle chaos across active members",
  { skip: !(hasAnthropicKey() && hasOpenAIKey()) },
  async () => {
    await withLiveRuntime({ requireOpenAI: true }, async (runtime) => {
      const mob = await runtime.createMob({
        id: `wasm-mob-48-${Date.now()}`,
        orchestrator: { profile: "lead" },
        profiles: {
          lead: {
            model: anthropicModel(),
            tools: { comms: true },
            peer_description: "Lead",
            external_addressable: true,
          },
          analyst: {
            model: anthropicModel(),
            tools: { comms: true },
            peer_description: "Analyst",
            external_addressable: true,
          },
          reviewer: {
            model: openaiModel(),
            tools: { comms: true },
            peer_description: "Reviewer",
            external_addressable: true,
          },
        },
        wiring: {
          auto_wire_orchestrator: false,
          role_wiring: [
            { a: "lead", b: "analyst" },
            { a: "lead", b: "reviewer" },
            { a: "analyst", b: "reviewer" },
          ],
        },
      });

      const spawned = await mob.spawn([
        {
          profile: "lead",
          meerkat_id: "lead-1",
          runtime_mode: "autonomous_host",
          initial_message: "Acknowledge the lead role in one sentence.",
        },
        {
          profile: "analyst",
          meerkat_id: "analyst-1",
          runtime_mode: "turn_driven",
          initial_message: "Acknowledge the analyst role in one sentence.",
        },
        {
          profile: "reviewer",
          meerkat_id: "reviewer-1",
          runtime_mode: "turn_driven",
          initial_message: "Acknowledge the reviewer role in one sentence.",
        },
      ]);
      assert.equal(spawned.length, 3);
      assert.ok(spawned.every((entry) => entry.status === "ok"));

      const reviewerSessionId = spawned[2].member_ref?.session_id;
      assert.ok(reviewerSessionId);

      const allSubscription = await mob.subscribeEvents();
      try {
        await mob.member("analyst-1").send(
          "Reply with ANALYST_READY_48.",
        );
        await mob.member("reviewer-1").send(
          "Reply with REVIEWER_READY_48.",
        );
        const items = await waitForSubscriptionItems(
          "mob-wide swarm events",
          allSubscription,
          (events) =>
            events.some((item) => item?.meerkat_id === "analyst-1")
            && events.some((item) => item?.meerkat_id === "reviewer-1"),
          { timeoutMs: 120000, intervalMs: 200 },
        );
        assert.ok(items.length > 0);
      } finally {
        allSubscription.close();
      }

      await mob.retire("reviewer-1");
      const withoutReviewer = await waitFor(
        "reviewer retirement",
        () => mob.listMembers(),
        (members) => !members.some((member) => member.meerkat_id === "reviewer-1"),
        { timeoutMs: 30000, intervalMs: 150 },
      );
      assert.ok(!withoutReviewer.some((member) => member.meerkat_id === "reviewer-1"));

      await mob.respawn("reviewer-1", "Return online and say REVIEWER_RESPAWN_48.");
      const withRespawnedReviewer = await waitFor(
        "reviewer respawn",
        () => mob.listMembers(),
        (members) =>
          members.some(
            (member) =>
              member.meerkat_id === "reviewer-1"
              && member.member_ref?.session_id
              && member.member_ref.session_id !== reviewerSessionId,
          ),
        { timeoutMs: 60000, intervalMs: 200 },
      );
      assert.ok(
        withRespawnedReviewer.some((member) => member.meerkat_id === "reviewer-1"),
      );

      await mob.lifecycle("stop");
      const stopped = await mob.status();
      assert.equal(stopped.state, "Stopped");

      await mob.lifecycle("resume");
      const resumed = await mob.status();
      assert.equal(resumed.state, "Running");

      const events = await mob.events("", 300);
      assert.ok(events.length >= 2);
    });
  },
);
