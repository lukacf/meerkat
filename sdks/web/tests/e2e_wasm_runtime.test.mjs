import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import http from "node:http";
import test from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

import { MeerkatRuntime } from "@rkat/web";
import { Session } from "@rkat/web";
import * as rawWasm from "@rkat/web/wasm/meerkat_web_runtime.js";

async function makeNodeCompatibleWasmModule() {
  const wasmUrl = import.meta.resolve("@rkat/web/wasm/meerkat_web_runtime_bg.wasm");
  const wasmBytes = await readFile(fileURLToPath(wasmUrl));
  return {
    ...rawWasm,
    default: async () => rawWasm.default({ module_or_path: wasmBytes }),
  };
}

async function withAnthropicStreamServer(chunkCount, fn) {
  const sseBody = Array.from(
    { length: chunkCount },
    (_, index) =>
      `data: ${JSON.stringify({ type: "content_block_delta", delta: { type: "text_delta", text: String(index % 10) } })}\n\n`,
  ).join("") + `data: ${JSON.stringify({ type: "message_stop" })}\n\n`;

  const server = http.createServer((req, res) => {
    if (req.method === "POST" && req.url === "/v1/messages") {
      res.writeHead(200, { "content-type": "text/event-stream" });
      res.end(sseBody);
      return;
    }
    res.writeHead(404);
    res.end("not found");
  });

  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  const { port } = server.address();
  try {
    return await fn(`http://127.0.0.1:${port}`);
  } finally {
    await new Promise((resolve, reject) =>
      server.close((error) => (error ? reject(error) : resolve())),
    );
  }
}

test("MeerkatRuntime drives direct-session lifecycle through shipped wasm exports", async () => {
  const wasm = await makeNodeCompatibleWasmModule();
  const runtime = await MeerkatRuntime.init(wasm, {
    anthropicApiKey: "sk-test",
    model: "claude-sonnet-4-5",
  });
  try {
    const session = runtime.createSession({
      model: "claude-sonnet-4-5",
      apiKey: "sk-test",
    });
    const staged = session.appendSystemContext({
      text: "Remember the browser-side coordinator.",
      source: "web",
      idempotencyKey: "ctx-web-1",
    });

    assert.equal(staged.status, "staged");
    assert.equal(staged.handle, session.handle);

    const state = session.getState();
    assert.equal(session.sessionId, state.session_id);
    assert.equal(state.handle, session.handle);
    assert.equal(typeof state.session_id, "string");
    assert.notEqual(state.session_id, String(session.handle));
    assert.equal(state.model, "claude-sonnet-4-5");

    session.destroy();
    assert.equal(session.isDestroyed, true);
    assert.throws(() => session.getState(), /destroyed/i);
  } finally {
    runtime.destroy();
  }
});

test("Session.destroy remains retryable when the underlying wasm destroy throws", () => {
  let destroyAttempts = 0;
  const session = new Session(
    7,
    async () => "{}",
    () => JSON.stringify({
      handle: 7,
      session_id: "sess_busy",
      mob_id: "",
      model: "claude-sonnet-4-5",
      usage: { input_tokens: 0, output_tokens: 0 },
      run_counter: 0,
      message_count: 0,
      is_active: true,
      last_assistant_text: null,
    }),
    () => {
      destroyAttempts += 1;
      if (destroyAttempts === 1) {
        throw new Error("SESSION_BUSY: turn still active");
      }
    },
    () => "[]",
    () => JSON.stringify({ handle: 7, status: "staged" }),
  );

  assert.throws(() => session.destroy(), /SESSION_BUSY/);
  assert.equal(session.isDestroyed, false);
  assert.equal(session.sessionId, "sess_busy");

  session.destroy();
  assert.equal(session.isDestroyed, true);
  assert.throws(() => session.getState(), /destroyed/i);
});

test("MeerkatRuntime opens and closes a public mob subscription through the shipped package surface", async () => {
  let subscribedHandle;
  const wasm = {
    ...(await makeNodeCompatibleWasmModule()),
    async mob_subscribe_events(mobId) {
      const handle = await rawWasm.mob_subscribe_events(mobId);
      subscribedHandle = handle;
      return handle;
    },
  };
  const runtime = await MeerkatRuntime.init(wasm, {
    anthropicApiKey: "sk-test",
    model: "claude-sonnet-4-5",
  });
  try {
    const mob = await runtime.createMob({
      id: "mob-web-phase-0",
      profiles: {
        worker: {
          model: "claude-sonnet-4-5",
          tools: { comms: true },
        },
      },
    });

    const subscription = await mob.subscribeAll();
    assert.deepEqual(subscription.poll(), []);
    subscription.close();
    assert.equal(subscription.isClosed, true);
    assert.notEqual(subscribedHandle, undefined);
    assert.throws(
      () => rawWasm.poll_subscription(subscribedHandle),
      /unknown subscription handle/i,
    );
  } finally {
    runtime.destroy();
  }
});

test("MeerkatRuntime exposes runtime-scoped tool registration on the instance surface", async () => {
  const calls = [];
  const wasm = {
    async default() {},
    runtime_version() {
      return "0.4.13";
    },
    init_runtime_from_config() {
      return JSON.stringify({ status: "initialized", model: "claude-sonnet-4-5", providers: ["anthropic"] });
    },
    register_tool_callback(name, description, schemaJson, callback) {
      calls.push(["callback", name, description, JSON.parse(schemaJson), typeof callback]);
    },
    register_js_tool(name, description, schemaJson) {
      calls.push(["fire_and_forget", name, description, JSON.parse(schemaJson)]);
    },
    clear_tool_callbacks() {
      calls.push(["clear"]);
    },
    destroy_runtime() {},
  };

  const runtime = await MeerkatRuntime.init(wasm, {
    anthropicApiKey: "sk-test",
    model: "claude-sonnet-4-5",
  });
  try {
    runtime.registerTool(
      "echo_browser",
      "Echo browser payloads",
      {
        type: "object",
        properties: {
          value: { type: "string" },
        },
        required: ["value"],
      },
      async (args) => ({ content: args, is_error: false }),
    );
    runtime.registerFireAndForgetTool(
      "notify_browser",
      "Emit a browser-side notification",
      {
        type: "object",
        properties: {
          message: { type: "string" },
        },
        required: ["message"],
      },
    );
    runtime.clearTools();

    assert.deepEqual(calls[0], [
      "callback",
      "echo_browser",
      "Echo browser payloads",
      {
        type: "object",
        properties: {
          value: { type: "string" },
        },
        required: ["value"],
      },
      "function",
    ]);
    assert.deepEqual(calls[1], [
      "fire_and_forget",
      "notify_browser",
      "Emit a browser-side notification",
      {
        type: "object",
        properties: {
          message: { type: "string" },
        },
        required: ["message"],
      },
    ]);
    assert.deepEqual(calls[2], ["clear"]);
  } finally {
    runtime.destroy();
  }
});

test("MeerkatRuntime surfaces lagged subscription signals through the shipped package surface", async () => {
  await withAnthropicStreamServer(305, async (anthropicBaseUrl) => {
    const wasm = await makeNodeCompatibleWasmModule();
    const runtime = await MeerkatRuntime.init(wasm, {
      anthropicApiKey: "sk-test",
      anthropicBaseUrl,
      model: "claude-sonnet-4-5",
    });
    try {
      const mob = await runtime.createMob({
        id: "mob-web-phase-4",
        profiles: {
          worker: {
            model: "claude-sonnet-4-5",
            external_addressable: true,
            tools: { comms: true },
          },
        },
      });

      const spawned = await mob.spawn([
        {
          profile: "worker",
          meerkat_id: "worker-1",
          runtime_mode: "turn_driven",
        },
      ]);
      assert.equal(spawned[0].status, "ok");

      const subscription = await mob.subscribe("worker-1");
      await mob.sendMessage("worker-1", "Trigger a long streamed response.");

      let items = [];
      for (let attempt = 0; attempt < 20; attempt += 1) {
        await delay(25);
        items = subscription.poll();
        if (items.some((item) => item.type === "lagged")) {
          break;
        }
      }

      const lagged = items.find((item) => item.type === "lagged");
      assert.ok(lagged, "expected a lagged signal from the shipped subscription path");
      assert.ok(lagged.skipped >= 1);
      const survivingEvent = items.find((item) => item.payload?.type === "text_delta");
      assert.ok(survivingEvent, "expected a surviving text_delta event after lag");
      subscription.close();
    } finally {
      runtime.destroy();
    }
  });
});
