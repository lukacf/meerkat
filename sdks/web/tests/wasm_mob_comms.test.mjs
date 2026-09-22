import assert from "node:assert/strict";
import { spawnSync } from "node:child_process";
import { createHash } from "node:crypto";
import { readFile } from "node:fs/promises";
import test from "node:test";
import { setTimeout as delay } from "node:timers/promises";
import { fileURLToPath } from "node:url";
import * as wasm from "../wasm/meerkat_web_runtime.js";

// Run against a freshly built artifact:
// node --test --test-force-exit sdks/web/tests/wasm_mob_comms.test.mjs
// Force-exit is only for leftover WASM timer handles after all assertions/teardown.
const bytes = await readFile(new URL("../wasm/meerkat_web_runtime_bg.wasm", import.meta.url));
const { version } = JSON.parse(await readFile(new URL("../package.json", import.meta.url)));
await wasm.default({ module_or_path: bytes });
assert.equal(wasm.runtime_version(), version);

const parse = value => typeof value === "string" ? JSON.parse(value) : value;
const MODEL = "claude-sonnet-4-6";
const PROVIDER_URL = "http://127.0.0.1:1/v1/messages";

async function bounded(label, action, timeout = 5_000) {
  let timer;
  try {
    return await Promise.race([
      Promise.resolve().then(action),
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error(`${label} exceeded ${timeout}ms`)), timeout);
      }),
    ]);
  } finally {
    clearTimeout(timer);
  }
}

async function eventually(label, read, predicate) {
  return bounded(label, async () => {
    for (;;) {
      const value = await read();
      if (predicate(value)) return value;
      await delay(10);
    }
  });
}

function anthropicResponse(number, model, tool) {
  const events = [
    { type: "message_start", message: {
      id: `msg_${number}`, type: "message", role: "assistant", model,
      content: [], stop_reason: null, usage: { input_tokens: 1, output_tokens: 0 },
    } },
    { type: "content_block_start", index: 0, content_block: tool
      ? { type: "tool_use", id: `tool_${number}`, name: tool.name, input: {} }
      : { type: "text", text: "" } },
    { type: "content_block_delta", index: 0, delta: tool
      ? { type: "input_json_delta", partial_json: JSON.stringify(tool.input) }
      : { type: "text_delta", text: `WASM_COMMS_OK_${number}` } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: tool ? "tool_use" : "end_turn" },
      usage: { output_tokens: 1 } },
    { type: "message_stop" },
  ];
  return new Response(events.map(event =>
    `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join(""), {
    status: 200, headers: { "content-type": "text/event-stream" },
  });
}

function installProvider(t) {
  const requests = [];
  const results = [];
  const actions = new Map();
  const original = globalThis.fetch;
  globalThis.fetch = async (input, init) => {
    const request = new Request(input, init);
    assert.equal(request.url, PROVIDER_URL, "all other network access is forbidden");
    assert.equal(request.method, "POST");
    const body = await request.json();
    requests.push(body);
    assert.ok(requests.length <= 40, "unexpected autonomous request loop");
    const last = body.messages.at(-1);
    const toolResults = Array.isArray(last.content)
      ? last.content.filter(block => block.type === "tool_result") : [];
    for (const result of toolResults) {
      assert.notEqual(result.is_error, true, JSON.stringify(result));
      results.push(result);
    }
    let tool;
    if (toolResults.length === 0) {
      const text = JSON.stringify(last.content);
      for (const [marker, action] of actions) {
        if (text.includes(marker)) {
          assert.ok(body.tools.some(row => row.name === action.name), action.name);
          tool = action;
          break;
        }
      }
    }
    return anthropicResponse(requests.length, body.model, tool);
  };
  t.after(() => { globalThis.fetch = original; });
  return { requests, results, actions };
}

function capturePanicStacks(t) {
  const originalLimit = Error.stackTraceLimit;
  Error.stackTraceLimit = 100;
  t.after(() => { Error.stackTraceLimit = originalLimit; });
  for (const method of ["log", "error", "warn", "info", "debug"]) {
    const original = console[method].bind(console);
    t.mock.method(console, method, (...args) => {
      original(...args);
      if (args.some(arg => typeof arg === "string" && arg.includes("wasm panic:"))) {
        original(new Error("WASM panic call stack").stack);
      }
    });
  }
}

async function status(mob, member) {
  return parse(await wasm.mob_member_status(mob, member));
}

async function idleAfter(mob, member, tokens = 0) {
  return eventually(`${member} completed turn`, () => status(mob, member), value =>
    value.output_preview?.startsWith("WASM_COMMS_OK_")
      && value.tokens_used > tokens
      && value.progress.run_state === "idle"
      && value.progress.in_flight_work === 0);
}

async function send(mob, member, content) {
  const receipt = parse(await bounded(`send ${member}`, () =>
    wasm.mob_member_send(mob, member, JSON.stringify({ content, handling_mode: "queue" }))));
  assert.equal(receipt.agent_identity, member);
  return receipt;
}

async function lifecycle(mob, action) {
  const result = parse(await bounded(action, () => wasm.mob_lifecycle(mob, action)));
  assert.equal(result.ok, true, JSON.stringify(result));
}

test("WASM mob comms: real idle drains, volatile topology and actionable messages", {
  timeout: 90_000,
}, async t => {
  t.diagnostic(`runtime=${version} wasm_sha256=${createHash("sha256").update(bytes).digest("hex")}`);
  const cases = [
    ["autonomous_host", false],
    ["turn_driven", false],
    [undefined, true],
    ["turn_driven", true],
  ];
  assert.ok(process.env.RKAT_WASM_COMMS_CASE === undefined
    || /^[0-3]$/.test(process.env.RKAT_WASM_COMMS_CASE), "invalid isolated case selector");
  for (const [index, [mode, autoWire]] of cases.entries()) {
    const selected = process.env.RKAT_WASM_COMMS_CASE;
    if (selected !== undefined && selected !== String(index)) continue;
    await t.test(`${mode ?? "default"} ${autoWire ? "auto-wire" : "explicit wire"}`, {
      timeout: 25_000,
    }, async t => {
      if (selected === undefined) {
        // A timed-out product future is not cancelled by the observation bound.
        // Isolate cases so a failed drain cannot poison the next runtime.
        const env = { ...process.env, RKAT_WASM_COMMS_CASE: String(index) };
        delete env.NODE_TEST_CONTEXT;
        const child = spawnSync(process.execPath, [
          "--test", "--test-force-exit", "--test-reporter=spec", fileURLToPath(import.meta.url),
        ], {
          env,
          encoding: "utf8", timeout: 24_000, maxBuffer: 8 * 1024 * 1024,
        });
        assert.equal(child.status, 0,
          `${child.error ?? ""}\n${child.stdout?.slice(-12_000)}\n${child.stderr}`);
        assert.doesNotMatch(child.stderr, /TimeoutOverflowWarning|RuntimeError|panicked/);
        assert.doesNotMatch(child.stdout, /wasm panic:/);
        const proof = child.stdout.split("\n").filter(line => line.includes("provider_calls=")).join("\n");
        assert.ok(proof, "child must actually finish the real-WASM contract");
        t.diagnostic(proof);
        return;
      }
      capturePanicStacks(t);
      const provider = installProvider(t);
      wasm.init_runtime_from_config(JSON.stringify({
        anthropic_api_key: "synthetic-no-network", anthropic_base_url: "http://127.0.0.1:1",
        model: MODEL,
      }));
      let mob;
      let failed = false;
      const subscriptions = [];
      try {
        mob = String(await wasm.mob_create(JSON.stringify({
          id: `wasm-comms-${mode ?? "default"}-${autoWire}`,
          profiles: { worker: {
            model: MODEL, ...(mode ? { runtime_mode: mode } : {}),
            tools: { comms: true }, external_addressable: true,
          } },
          wiring: autoWire ? { role_wiring: [{ a: "worker", b: "worker" }] } : {},
          flows: {},
        })));
        for (const member of ["a", "b"]) {
          const result = parse(await bounded(`spawn ${member}`, () =>
            wasm.mob_spawn(mob, JSON.stringify([{
              profile: "worker", agent_identity: member,
              ...(mode ? { runtime_mode: mode } : {}),
              initial_message: "Complete one deterministic fixture turn.",
            }]))));
          assert.equal(result[0].status, "spawned", JSON.stringify(result));
          await idleAfter(mob, member);
          subscriptions.push(await bounded(`subscribe ${member}`, () =>
            wasm.mob_member_subscribe(mob, member)));
        }
        // Auto-wiring before autonomous kickoff also delivers the two
        // machine-emitted kickoff Requests. They are not PeerAdded notices.
        const startupRequests = autoWire && mode !== "turn_driven" ? 4 : 2;
        if (startupRequests === 4) {
          await idleAfter(mob, "a", 4);
          const prompts = provider.requests.map(request => JSON.stringify(request.messages.at(-1)));
          for (const phase of ["starting", "started"]) {
            assert.equal(prompts.filter(prompt => prompt.includes(`Intent: mob.kickoff_${phase}`)).length, 1);
          }
        }
        assert.equal(provider.requests.length, startupRequests,
          JSON.stringify(provider.requests.map(request => request.messages.at(-1))));
        if (!autoWire) {
          await bounded("wire idle members", () => wasm.mob_wire(mob, "a", "b"));
        }
        // Idempotent wiring must not duplicate a drain or turn peer_added into LLM work.
        await bounded("wire again", () => wasm.mob_wire(mob, "a", "b"));
        await delay(50);
        assert.equal(provider.requests.length, startupRequests, "volatile lifecycle handoff is not a model turn");
        for (const member of ["a", "b"]) {
          const snapshot = await status(mob, member);
          // member_status projects structural edges as unknown, not a live
          // reachability probe. The peers tool below proves actual trust.
          assert.deepEqual(snapshot.peer_connectivity.snapshot, {
            reachable_peer_count: 0, unknown_peer_count: 1,
          });
          assert.equal(snapshot.progress.run_state, "idle");
        }

        const targets = {};
        for (const member of ["a", "b"]) {
          targets[member] = parse(await wasm.mob_member_peer_target(mob, member)).external.peer_id;
          assert.equal(typeof targets[member], "string");
        }
        // Inspect the real peers tool, not a mirrored edge list in JavaScript.
        provider.actions.set("WASM_QUERY_PEERS", { name: "peers", input: {} });
        for (const member of ["a", "b"]) {
          const before = await status(mob, member);
          const resultIndex = provider.results.length;
          await send(mob, member, "WASM_QUERY_PEERS");
          await idleAfter(mob, member, before.tokens_used);
          const result = provider.results[resultIndex];
          assert.ok(result, "peers result reaches the real next provider request");
          assert.ok(JSON.stringify(result.content).includes(targets[member === "a" ? "b" : "a"]));
        }

        if (mode !== "turn_driven") {
          // Ordinary send_message uses actionable ingress, unlike peer_added's
          // exact volatile handoff. Prove a later idle recipient turn completes.
          provider.actions.set("WASM_SEND_TO_B", {
            name: "send_message",
            input: { peer_id: targets.b, body: "WASM_ORDINARY_PEER_MESSAGE", handling_mode: "queue" },
          });
          const beforeA = await status(mob, "a");
          const beforeB = await status(mob, "b");
          const beforeRequests = provider.requests.length;
          await send(mob, "a", "WASM_SEND_TO_B");
          await idleAfter(mob, "a", beforeA.tokens_used);
          await idleAfter(mob, "b", beforeB.tokens_used);
          assert.equal(provider.requests.length, beforeRequests + 3);
          assert.ok(provider.requests.slice(beforeRequests).some(request =>
            JSON.stringify(request.messages.at(-1)).includes("WASM_ORDINARY_PEER_MESSAGE")));
          const events = subscriptions.flatMap(stream => parse(wasm.poll_subscription(stream)));
          assert.ok(events.some(event => event.payload.type === "peer_content_ingested"),
            "ordinary peer message must commit typed ingestion, not only admit a send");
        }

        await lifecycle(mob, "stop");
        assert.equal(parse(await wasm.mob_status(mob)).status, "Stopped");
        await assert.rejects(() => send(mob, "a", "must not execute while stopped"));
        const stoppedRequests = provider.requests.length;
        await delay(50);
        assert.equal(provider.requests.length, stoppedRequests);
        await lifecycle(mob, "resume");
        assert.equal(parse(await wasm.mob_status(mob)).status, "Running");
        await bounded("unwire after resume", () => wasm.mob_unwire(mob, "a", "b"));
        await bounded("rewire after resume", () => wasm.mob_wire(mob, "a", "b"));
        await delay(50);
        assert.equal(provider.requests.length, stoppedRequests);
        const before = await status(mob, "a");
        await send(mob, "a", "Explicit work after resuming the member ingress.");
        await idleAfter(mob, "a", before.tokens_used);
        assert.equal(provider.requests.length, stoppedRequests + 1);
      } catch (error) {
        failed = true;
        throw error;
      } finally {
        try {
          for (const subscription of subscriptions) wasm.close_subscription(subscription);
          if (mob) await lifecycle(mob, "destroy");
        } catch (error) {
          if (!failed) throw error;
          t.diagnostic(`cleanup after failed contract: ${error}`);
        } finally {
          wasm.destroy_runtime();
        }
      }
      assert.throws(() => wasm.poll_subscription(subscriptions[0]), /invalid_stream_id/);
      t.diagnostic(`provider_calls=${provider.requests.length}; live member turns, topology and teardown verified`);
    });
  }
});
