import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import http from "node:http";
import test from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";

import { MeerkatRuntime, Session, isKnownEvent, KNOWN_AGENT_EVENT_TYPES } from "@rkat/web";
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

test("Session.destroy becomes idempotent after the runtime has already been destroyed", () => {
  let destroyAttempts = 0;
  const session = new Session(
    8,
    async () => "{}",
    () => JSON.stringify({
      handle: 8,
      session_id: "sess_runtime_gone",
      mob_id: "",
      model: "claude-sonnet-4-5",
      usage: { input_tokens: 0, output_tokens: 0 },
      run_counter: 0,
      message_count: 0,
      is_active: false,
      last_assistant_text: null,
    }),
    () => {
      destroyAttempts += 1;
      throw new Error("not_initialized: runtime not initialized");
    },
    () => "[]",
    () => JSON.stringify({ handle: 8, status: "staged" }),
  );

  assert.doesNotThrow(() => session.destroy());
  assert.equal(session.isDestroyed, true);
  assert.equal(destroyAttempts, 1);
  assert.doesNotThrow(() => session.destroy());
  assert.equal(destroyAttempts, 1);
  assert.throws(() => session.getState(), /destroyed/i);
});

test("Session.destroy treats typed not_initialized errors as idempotent teardown", () => {
  let destroyAttempts = 0;
  const session = new Session(
    9,
    async () => "{}",
    () => JSON.stringify({
      handle: 9,
      session_id: "sess_runtime_code_gone",
      mob_id: "",
      model: "claude-sonnet-4-5",
      usage: { input_tokens: 0, output_tokens: 0 },
      run_counter: 0,
      message_count: 0,
      is_active: false,
      last_assistant_text: null,
    }),
    () => {
      destroyAttempts += 1;
      const error = new Error("runtime not initialized");
      error.code = "not_initialized";
      throw error;
    },
    () => "[]",
    () => JSON.stringify({ handle: 9, status: "staged" }),
  );

  assert.doesNotThrow(() => session.destroy());
  assert.equal(session.isDestroyed, true);
  assert.equal(destroyAttempts, 1);
  assert.doesNotThrow(() => session.destroy());
  assert.equal(destroyAttempts, 1);
});

test("isKnownEvent recognizes the full current canonical event surface", () => {
  assert.equal(isKnownEvent({ type: "run_started", session_id: "s", prompt: "p" }), true);
  assert.equal(
    isKnownEvent({
      type: "hook_started",
      hook_id: "hook-1",
      point: "before_tool",
    }),
    true,
  );
  assert.equal(
    isKnownEvent({
      type: "budget_warning",
      budget_type: "time",
      used: 10,
      limit: 20,
      percent: 50,
    }),
    true,
  );
  assert.equal(
    isKnownEvent({
      type: "tool_config_changed",
      payload: {
        operation: "reload",
        target: "external",
        status: "applied",
        persisted: true,
        applied_at_turn: 4,
      },
    }),
    true,
  );
  assert.equal(
    isKnownEvent({
      type: "stream_truncated",
      reason: "backpressure",
    }),
    true,
  );
});

test("web known event types stay aligned with the canonical contracts artifact", async () => {
  const eventsJsonPath = new URL("../../../artifacts/schemas/events.json", import.meta.url);
  const raw = JSON.parse(await readFile(eventsJsonPath, "utf8"));
  const canonicalTypes = raw.WireEvent?.known_event_types;

  assert.ok(Array.isArray(canonicalTypes), "contracts events.json must expose known_event_types");
  assert.deepEqual(
    [...KNOWN_AGENT_EVENT_TYPES].sort(),
    [...canonicalTypes].sort(),
  );
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

    const subscription = await mob.subscribeEvents();
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

      const subscription = await mob.member("worker-1").subscribe();
      await mob.member("worker-1").send("Trigger a long streamed response.");

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

test("MeerkatRuntime forwards canonical mob status/helper methods through the wasm binding surface", async () => {
  const calls = [];
  const wasm = {
    async default() {},
    runtime_version() {
      return "0.4.13";
    },
    init_runtime_from_config() {
      return JSON.stringify({ status: "initialized", model: "claude-sonnet-4-5", providers: ["anthropic"] });
    },
    register_tool_callback() {},
    register_js_tool() {},
    clear_tool_callbacks() {},
    destroy_runtime() {},
    async mob_create() {
      return "mob-web-parity";
    },
    async mob_status(mobId) {
      return JSON.stringify({ mob_id: mobId, state: "running" });
    },
    async mob_list() {
      return JSON.stringify([]);
    },
    async mob_lifecycle(mobId, action) {
      calls.push(["lifecycle", mobId, action]);
    },
    async mob_events() {
      return JSON.stringify([]);
    },
    async mob_spawn() {
      return JSON.stringify([]);
    },
    async mob_retire(mobId, meerkatId) {
      calls.push(["retire", mobId, meerkatId]);
    },
    async mob_wire(mobId, member, peer) {
      calls.push(["wire", mobId, member, peer]);
    },
    async mob_unwire(mobId, member, peer) {
      calls.push(["unwire", mobId, member, peer]);
    },
    async mob_wire_target(mobId, member, targetJson) {
      calls.push(["wire_target", mobId, member, JSON.parse(targetJson)]);
    },
    async mob_unwire_target(mobId, member, targetJson) {
      calls.push(["unwire_target", mobId, member, JSON.parse(targetJson)]);
    },
    async mob_list_members() {
      return JSON.stringify([]);
    },
    async mob_append_system_context(_mobId, meerkatId) {
      return JSON.stringify({
        mob_id: "mob-web-parity",
        meerkat_id: meerkatId,
        session_id: "sess-ctx",
        status: "staged",
      });
    },
    async mob_member_send(_mobId, meerkatId, requestJson) {
      calls.push(["member_send", meerkatId, JSON.parse(requestJson)]);
      return JSON.stringify({
        member_id: meerkatId,
        session_id: "sess-send",
        handling_mode: "queue",
      });
    },
    async mob_member_status(_mobId, meerkatId) {
      return JSON.stringify({
        status: "running",
        tokens_used: 7,
        is_final: false,
        current_session_id: `${meerkatId}-session`,
      });
    },
    async mob_respawn(_mobId, meerkatId) {
      return JSON.stringify({
        status: "completed",
        receipt: {
          member_id: meerkatId,
          old_session_id: "sess-old",
          new_session_id: "sess-new",
        },
      });
    },
    async mob_force_cancel(mobId, meerkatId) {
      calls.push(["force_cancel", mobId, meerkatId]);
    },
    async mob_spawn_helper(mobId, requestJson) {
      calls.push(["spawn_helper", mobId, JSON.parse(requestJson)]);
      return JSON.stringify({
        output: "helper complete",
        tokens_used: 11,
        session_id: "sess-helper",
      });
    },
    async mob_fork_helper(mobId, requestJson) {
      calls.push(["fork_helper", mobId, JSON.parse(requestJson)]);
      return JSON.stringify({
        output: "fork complete",
        tokens_used: 13,
        session_id: "sess-fork",
      });
    },
    async mob_run_flow() {
      return "run-1";
    },
    async mob_flow_status() {
      return JSON.stringify({ run_id: "run-1", status: "running" });
    },
    async mob_cancel_flow(mobId, runId) {
      calls.push(["cancel_flow", mobId, runId]);
    },
    async mob_member_subscribe() {
      return 1;
    },
    async mob_subscribe_events() {
      return 2;
    },
    poll_subscription() {
      return "[]";
    },
    close_subscription(handle) {
      calls.push(["close_subscription", handle]);
    },
  };

  const runtime = await MeerkatRuntime.init(wasm, {
    anthropicApiKey: "sk-test",
    model: "claude-sonnet-4-5",
  });
  try {
    const mob = await runtime.createMob({
      id: "mob-web-parity",
      profiles: {
        worker: {
          model: "claude-sonnet-4-5",
        },
      },
    });

    const receipt = await mob.member("worker-1").send("hello");
    assert.equal(receipt.session_id, "sess-send");

    const snapshot = await mob.memberStatus("worker-1");
    assert.equal(snapshot.current_session_id, "worker-1-session");

    await mob.forceCancel("worker-1");

    const helper = await mob.spawnHelper("Summarize the thread.", {
      meerkatId: "helper-1",
      profileName: "worker",
    });
    assert.equal(helper.session_id, "sess-helper");

    const fork = await mob.forkHelper("worker-1", "Review the draft.", {
      meerkatId: "fork-1",
      profileName: "worker",
      forkContext: { mode: "full_history" },
    });
    assert.equal(fork.session_id, "sess-fork");

    assert.deepEqual(
      calls.filter(([name]) =>
        ["member_send", "force_cancel", "spawn_helper", "fork_helper"].includes(name),
      ),
      [
        [
          "member_send",
          "worker-1",
          { content: "hello", handling_mode: "queue" },
        ],
        ["force_cancel", "mob-web-parity", "worker-1"],
        [
          "spawn_helper",
          "mob-web-parity",
          {
            prompt: "Summarize the thread.",
            meerkat_id: "helper-1",
            profile_name: "worker",
          },
        ],
        [
          "fork_helper",
          "mob-web-parity",
          {
            source_member_id: "worker-1",
            prompt: "Review the draft.",
            meerkat_id: "fork-1",
            profile_name: "worker",
            fork_context: { mode: "full_history" },
          },
        ],
      ],
    );
  } finally {
    runtime.destroy();
  }
});
