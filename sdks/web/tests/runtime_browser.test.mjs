import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { cp, mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import { tmpdir } from "node:os";
import test from "node:test";
import { fileURLToPath } from "node:url";

import { chromium } from "playwright";
import { buildFixtureMobpack } from "../../../tests/live_smoke/browser/harness/mobpack-builder.mjs";

const SDK_ROOT = fileURLToPath(new URL("..", import.meta.url));
const WASM_ROOT = path.resolve(process.env.MEERKAT_WEB_WASM_OUT_DIR || path.join(SDK_ROOT, "wasm"));
const { version } = JSON.parse(await readFile(path.join(SDK_ROOT, "package.json"), "utf8"));
const wasmBytes = await readFile(path.join(WASM_ROOT, "meerkat_web_runtime_bg.wasm"));
const fixtureRoot = fileURLToPath(new URL(
  "../../../tests/live_smoke/browser/fixtures/browser_safe_mobpack/", import.meta.url,
));
const fixturePack = await buildFixtureMobpack(fixtureRoot);
const PACK_EXCLUSIONS = [
  ["shell", "shell", "use_host_process_runtime"],
  ["process_spawn", "process_spawn", "use_host_process_runtime"],
  ["mcp_stdio", "mcp_stdio", "use_host_process_runtime"],
  ["hooks", "hooks", "use_hook_runtime"],
  ["skills", "runtime_skills", "use_skill_runtime"],
  ["mcp_live", "mcp_client", "use_mcp_runtime"],
  ["session_store", "durable_persistence", "use_persistent_runtime"],
  ["schedule", "schedule", "use_schedule_runtime"],
  ["work_graph", "work_graph", "use_work_graph_runtime"],
  ["memory_store", "semantic_memory", "use_semantic_memory_runtime"],
];
const PROFILE_EXCLUSIONS = [
  ["durable_persistence", "use_persistent_runtime"],
  ["background_execution", "use_background_runtime"],
  ["shell", "use_host_process_runtime"],
  ["process_spawn", "use_host_process_runtime"],
  ["mcp_stdio", "use_host_process_runtime"],
  ["remote_member_placement", "use_local_member_placement"],
  ["hooks", "use_hook_runtime"],
  ["runtime_skills", "use_skill_runtime"],
  ["file_schema_resolution", "use_inline_schema"],
  ["mcp_client", "use_mcp_runtime"],
  ["tcp_comms", "use_in_process_comms"],
  ["uds_comms", "use_in_process_comms"],
  ["schedule", "use_schedule_runtime"],
  ["work_graph", "use_work_graph_runtime"],
  ["semantic_memory", "use_semantic_memory_runtime"],
  ["image_generation", "use_image_generation_runtime"],
  ["fallback_web_search", "use_web_search_runtime"],
];
const PROFILE_SUPPORTED = [
  "in_memory_persistence", "foreground_execution", "keep_alive", "comms", "transient_turn_context", "embedded_skills",
];
const excludedPacks = new Map();
const temporaryFixture = await mkdtemp(path.join(tmpdir(), "meerkat-browser-profile-"));
try {
  await cp(fixtureRoot, temporaryFixture, { recursive: true });
  await rm(path.join(temporaryFixture, "signature.toml"));
  const manifest = await readFile(path.join(temporaryFixture, "manifest.toml"), "utf8");
  for (const [capability] of PACK_EXCLUSIONS) {
    await writeFile(path.join(temporaryFixture, "manifest.toml"), manifest.replace('["comms"]', JSON.stringify([capability])));
    excludedPacks.set(`/fixture-${capability}.mobpack`, await buildFixtureMobpack(temporaryFixture));
  }
} finally { await rm(temporaryFixture, { recursive: true }); }
const MODEL = "claude-sonnet-4-6";

function anthropicResponse(request, sequence, tool) {
  const text = `BROWSER_RUNTIME_OK_${sequence}`;
  const content = tool
    ? { type: "tool_use", id: `browser_tool_${sequence}`, name: tool.name, input: {} }
    : { type: "text", text: "" };
  const events = [
    { type: "message_start", message: {
      id: `browser_message_${sequence}`, type: "message", role: "assistant", model: request.model,
      content: [], stop_reason: null, usage: { input_tokens: 10, output_tokens: 0 },
    } },
    { type: "content_block_start", index: 0, content_block: content },
    { type: "content_block_delta", index: 0, delta: tool
      ? { type: "input_json_delta", partial_json: JSON.stringify(tool.input) }
      : { type: "text_delta", text } },
    { type: "content_block_stop", index: 0 },
    { type: "message_delta", delta: { stop_reason: tool ? "tool_use" : "end_turn" }, usage: { output_tokens: 5 } },
    { type: "message_stop" },
  ];
  return events.map(event => `event: ${event.type}\ndata: ${JSON.stringify(event)}\n\n`).join("");
}

async function startServer() {
  const server = http.createServer(async (request, response) => {
    const pathname = new URL(request.url, "http://localhost").pathname;
    try {
      if (pathname === "/") {
        response.writeHead(200, { "content-type": "text/html" });
        response.end("<!doctype html><title>Meerkat runtime browser contracts</title>");
        return;
      }
      if (pathname === "/fixture.mobpack" || excludedPacks.has(pathname)) {
        response.writeHead(200, { "content-type": "application/gzip" });
        response.end(excludedPacks.get(pathname) ?? fixturePack);
        return;
      }
      const root = pathname.startsWith("/wasm/") ? WASM_ROOT
        : pathname.startsWith("/sdk/") ? path.join(SDK_ROOT, "dist") : undefined;
      if (!root) throw new Error("unknown path");
      const relative = pathname.slice(pathname.indexOf("/", 1) + 1);
      const filename = path.resolve(root, relative);
      if (!filename.startsWith(root + path.sep)) throw new Error("path outside fixture root");
      const body = await readFile(filename);
      response.writeHead(200, {
        "content-type": filename.endsWith(".wasm") ? "application/wasm" : "text/javascript",
        "cache-control": "no-store",
      });
      response.end(body);
    } catch {
      response.writeHead(404);
      response.end("not found");
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return {
    origin: `http://127.0.0.1:${server.address().port}`,
    close: () => new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve())),
  };
}

test("canonical runtime direct-session contracts execute in Chromium", { timeout: 180_000 }, async t => {
  t.diagnostic(`runtime=${version} wasm_sha256=${createHash("sha256").update(wasmBytes).digest("hex")}`);
  const schemas = JSON.parse(await readFile(new URL("../../../artifacts/schemas/wire-types.json", import.meta.url), "utf8"));
  assert.deepEqual([...schemas.RuntimeProfileCapability.enum].sort(),
    [...PROFILE_SUPPORTED, ...PROFILE_EXCLUSIONS.map(([capability]) => capability)].sort(),
    "every canonical capability must have a real browser acceptance or refusal assertion");
  const server = await startServer();
  const browser = await chromium.launch({ headless: true });
  try {
    async function scenario(name, action) {
      if (process.env.MEERKAT_BROWSER_CASE && !name.includes(process.env.MEERKAT_BROWSER_CASE)) return;
      await t.test(name, { timeout: 20_000 }, async () => {
        const page = await browser.newPage();
        const requests = [];
        const pageErrors = [];
        const externalRequests = [];
        const provider = { mode: "success", tool: undefined, chooseTool: undefined, beforeResponse: undefined };
        page.on("pageerror", error => pageErrors.push(error.message));
        await page.route("**/*", async route => {
          const request = route.request();
          if (!request.url().startsWith(server.origin + "/")) {
            externalRequests.push(request.url());
            return route.abort();
          }
          if (request.url() !== server.origin + "/anthropic/v1/messages") return route.continue();
          const body = request.postDataJSON();
          requests.push(body);
          assert.ok(requests.length <= 25, "unexpected autonomous provider loop");
          if (provider.beforeResponse) await provider.beforeResponse(body);
          if (provider.mode === "failure") {
            return route.fulfill({ status: 401, contentType: "application/json", body: JSON.stringify({
              type: "error", error: { type: "authentication_error", message: "deterministic provider failure" },
            }) });
          }
          const last = body.messages.at(-1);
          const hasResult = Array.isArray(last?.content) && last.content.some(block => block.type === "tool_result");
          const tool = provider.chooseTool ? provider.chooseTool(body)
            : !hasResult && provider.tool && JSON.stringify(last).includes(provider.tool.marker)
              ? provider.tool : undefined;
          return route.fulfill({ contentType: "text/event-stream", body: anthropicResponse(body, requests.length, tool) });
        });
        try {
          await page.goto(server.origin);
          await page.evaluate(async ({ origin, model, version }) => {
            window.wasm = await import("/wasm/meerkat_web_runtime.js");
            window.sdk = await import("/sdk/index.js");
            window.runtime = await window.sdk.MeerkatRuntime.init(window.wasm, {
              anthropicApiKey: "synthetic-not-a-key", anthropicBaseUrl: `${origin}/anthropic`, model,
              mobpackTrust: { policy: "permissive" },
            });
            if (window.wasm.runtime_version() !== version) throw new Error("stale WASM artifact");
            window.model = model;
            window.parse = value => typeof value === "string" ? JSON.parse(value) : value;
            window.errorEnvelope = error => typeof error === "string" ? JSON.parse(error)
              : { code: error.code, message: error.message, data: error.data };
            window.bounded = async (promise, label) => {
              let timer;
              try {
                return await Promise.race([promise, new Promise((_, reject) => {
                  timer = setTimeout(() => reject(new Error(`${label} exceeded 5 seconds`)), 5_000);
                })]);
              } finally { clearTimeout(timer); }
            };
          }, { origin: server.origin, model: MODEL, version });
          let timeout;
          try {
            await Promise.race([
              action({ page, requests, provider }),
              new Promise((_, reject) => {
                timeout = setTimeout(() => reject(new Error(`${name}: browser contract exceeded 15 seconds`)), 15_000);
              }),
            ]);
          } finally { clearTimeout(timeout); }
          assert.deepEqual(externalRequests, [], "fixture must never use an external provider");
          assert.deepEqual(pageErrors, [], "no unhandled errors or WASM panics");
        } finally {
          try { await page.evaluate(async () => { await window.bounded(window.wasm?.destroy_runtime(), "runtime teardown"); }); }
          finally { await page.close(); }
        }
      });
    }

    await scenario("SDK creation is deferred and identity/context come from the session owner", async ({ page, requests }) => {
      const result = await page.evaluate(async () => {
        const session = await window.runtime.createSession({ model: window.model });
        const initial = await session.getState();
        const first = await session.appendSystemContext({ text: "BROWSER_CONTEXT_MARKER", idempotencyKey: "same-context" });
        const repeated = await session.appendSystemContext({ text: "BROWSER_CONTEXT_MARKER", idempotencyKey: "same-context" });
        window.session = session;
        return { handle: session.handle, identity: await session.sessionId, initial, first, repeated };
      });
      assert.equal(requests.length, 0, "deferred creation must not invoke the provider");
      assert.equal(result.identity, result.initial.session_id);
      assert.notEqual(result.identity, String(result.handle));
      assert.equal(result.initial.is_active, false);
      assert.equal(result.initial.handle, undefined);
      assert.equal(result.initial.mob_id, undefined);
      assert.equal(result.initial.provider, "anthropic");
      assert.deepEqual(result.first, { status: "applied" });
      assert.deepEqual(result.repeated, { status: "duplicate" });
      await page.evaluate(async () => { await window.session.turn("Read the admitted system context."); });
      assert.equal(requests.length, 1);
      assert.equal(JSON.stringify(requests[0]).match(/BROWSER_CONTEXT_MARKER/g)?.length, 1);
    });

    await scenario("success results and event polling expose canonical terminality across turns", async ({ page, requests }) => {
      const result = await page.evaluate(async () => {
        const session = await window.runtime.createSession({ model: window.model });
        const first = await session.turn("First deterministic turn.");
        const second = await session.turn([{ type: "text", text: "Second deterministic turn." }]);
        const events = session.pollEvents();
        const drained = session.pollEvents();
        return { first, second, events, drained, state: await session.getState() };
      });
      assert.equal(requests.length, 2);
      assert.equal(result.first.text, "BROWSER_RUNTIME_OK_1");
      assert.equal(result.second.text, "BROWSER_RUNTIME_OK_2");
      assert.equal(result.first.session_id, result.state.session_id);
      assert.equal(result.second.session_id, result.state.session_id);
      // Successful completion carries no exceptional terminal cause.
      assert.equal(result.first.terminal_cause_kind ?? null, null);
      assert.equal(result.second.terminal_cause_kind, result.first.terminal_cause_kind);
      assert.equal(result.events.filter(event => event.type === "run_completed").length, 2);
      assert.equal(result.events.filter(event => event.type === "run_started").length, 2);
      assert.deepEqual(result.drained, []);
      assert.equal(result.state.is_active, false);
      assert.equal(result.state.last_assistant_text, result.second.text);
    });

    await scenario("malformed structured content refuses before provider dispatch", async ({ page, requests }) => {
      const error = await page.evaluate(async () => {
        const handle = await window.wasm.create_session_simple(JSON.stringify({ model: window.model }));
        try {
          await window.wasm.start_turn(handle, JSON.stringify({ blocks: [{ type: "unknown" }] }));
          throw new Error("invalid content was accepted");
        } catch (error) { return window.errorEnvelope(error); }
      });
      assert.equal(error.code, "INVALID_PARAMS");
      assert.equal(requests.length, 0);
    });

    await scenario("internal execution and unsupported skill metadata reject before admission", async ({ page, requests }) => {
      const errors = await page.evaluate(async () => {
        const handle = await window.wasm.create_session_simple(JSON.stringify({ model: window.model }));
        const errors = [];
        for (const options of [{ skill_references: ["unsupported/browser-skill"] }, { execution_kind: "resume" }]) {
          try {
            await window.wasm.start_turn(handle, JSON.stringify({ text: "must not execute" }), JSON.stringify(options));
            errors.push({ accepted: true });
          } catch (error) { errors.push(window.errorEnvelope(error)); }
        }
        return errors;
      });
      assert.ok(errors.every(error => error.code === "invalid_request"), JSON.stringify(errors));
      assert.equal(requests.length, 0);
    });

    await scenario("typed embedded turn skills resolve through the canonical skill engine", async ({ page, requests }) => {
      const result = await page.evaluate(async () => {
        const session = await window.runtime.createSession({ model: window.model });
        const turn = await session.turn("BROWSER_EMBEDDED_SKILL", { skillReferences: [{
          source_uuid: "00000000-0000-4b11-8111-000000000001", skill_name: "task-workflow",
        }] });
        return { turn, events: session.pollEvents() };
      });
      assert.equal(result.turn.text, "BROWSER_RUNTIME_OK_1");
      assert.equal(requests.length, 1);
      assert.ok(JSON.stringify(requests[0].messages).includes("Use builtin task tools for lightweight project work tracking"),
        "per-turn skills must reach the provider as typed skill-context content");
      assert.ok(result.events.some(event => event.type === "skills_resolved" && event.skills.some(skill =>
        skill.source_uuid === "00000000-0000-4b11-8111-000000000001" && skill.skill_name === "task-workflow")),
      JSON.stringify(result.events));
    });

    await scenario("external typed turn skills refuse with profile remediation before provider admission", async ({ page, requests }) => {
      const result = await page.evaluate(async () => {
        const session = await window.runtime.createSession({ model: window.model });
        let error;
        try {
          await session.turn("BROWSER_EXTERNAL_SKILL", { skillReferences: [{
            source_uuid: "e2377cb8-8374-4501-8113-893527589e49", skill_name: "outside-skill",
          }] });
        } catch (failure) { error = window.errorEnvelope(failure); }
        return { error, state: await session.getState(), events: session.pollEvents() };
      });
      assert.equal(result.error?.code, "CAPABILITY_UNAVAILABLE", JSON.stringify(result));
      assert.deepEqual(result.error.data, {
        profile: "browser", capability: "runtime_skills", clearing_action: "use_skill_runtime",
      });
      assert.equal(result.state.is_active, false);
      assert.equal(result.events.some(event => event.type === "run_started"), false);
      assert.equal(requests.length, 0);
    });

    await scenario("provider failure rejects and projects a failed terminal event", async ({ page, requests, provider }) => {
      provider.mode = "failure";
      const result = await page.evaluate(async () => {
        const session = await window.runtime.createSession({ model: window.model });
        window.session = session;
        let failure;
        try { await session.turn("Return the deterministic provider failure."); }
        catch (error) { failure = window.errorEnvelope(error); }
        return { failure, events: session.pollEvents(), state: await session.getState() };
      });
      assert.equal(requests.length, 1);
      assert.ok(result.failure?.code, "faults must reject with a typed code");
      assert.notEqual(result.failure.code, "UNKNOWN");
      assert.ok(result.events.some(event => event.type === "run_failed"));
      assert.equal(result.events.find(event => event.type === "run_failed").terminal_cause_kind, "llm_failure");
      assert.equal(result.events.some(event => event.type === "run_completed"), false);
      assert.equal(result.state.is_active, false);
      provider.mode = "success";
      const retry = await page.evaluate(async () => {
        return await window.session.turn("Fresh work after a provider failure.");
      });
      assert.equal(retry.text, "BROWSER_RUNTIME_OK_2");
    });

    await scenario("concurrent prompts queue through runtime admission and complete in order", async ({ page, requests, provider }) => {
      let release;
      let entered;
      const started = new Promise(resolve => { entered = resolve; });
      const gate = new Promise(resolve => { release = resolve; });
      provider.beforeResponse = async () => {
        if (requests.length === 1) { entered(); await gate; }
      };
      try {
        await page.evaluate(async () => {
          window.session = await window.runtime.createSession({ model: window.model });
          window.first = window.session.turn("BROWSER_QUEUE_FIRST").then(value => ({ value }), error => ({ error: window.errorEnvelope(error) }));
        });
        await started;
        const active = await page.evaluate(async () => {
          window.second = window.session.turn("BROWSER_QUEUE_SECOND").then(value => ({ value }), error => ({ error: window.errorEnvelope(error) }));
          return await window.session.getState();
        });
        assert.equal(active.is_active, true);
        assert.equal(requests.length, 1, "second prompt must wait for the active run");
        release();
        const result = await page.evaluate(async () => ({
          results: await window.bounded(Promise.all([window.first, window.second]), "queued prompts"),
          state: await window.session.getState(), events: window.session.pollEvents(),
        }));
        assert.deepEqual(result.results.map(row => row.error), [undefined, undefined]);
        assert.deepEqual(result.results.map(row => row.value.text), ["BROWSER_RUNTIME_OK_1", "BROWSER_RUNTIME_OK_2"]);
        assert.equal(requests.length, 2);
        assert.ok(JSON.stringify(requests[1].messages.at(-1)).includes("BROWSER_QUEUE_SECOND"));
        assert.equal(result.events.filter(event => event.type === "run_completed").length, 2);
        assert.equal(result.state.is_active, false);
      } finally { release(); }
    });

    await scenario("interrupt terminalizes a pending run and the same session accepts later work", async ({ page, requests, provider }) => {
      let release;
      let entered;
      const started = new Promise(resolve => { entered = resolve; });
      const gate = new Promise(resolve => { release = resolve; });
      provider.beforeResponse = async () => {
        if (requests.length === 1) { entered(); await gate; }
      };
      try {
        await page.evaluate(async () => {
          window.session = await window.runtime.createSession({ model: window.model });
          window.pending = window.session.turn("BROWSER_INTERRUPT_PENDING", {
            transientTurnContext: "BROWSER_INTERRUPT_TRANSIENT_MARKER",
          }).then(value => ({ value }), error => ({ error: window.errorEnvelope(error) }));
        });
        await started;
        const result = await page.evaluate(async () => {
          await window.bounded(window.session.interrupt(), "interrupt command");
          const terminal = await window.bounded(window.pending, "interrupted turn");
          return { terminal, events: window.session.pollEvents(), state: await window.session.getState() };
        });
        assert.ok(result.terminal.error?.code, JSON.stringify(result));
        assert.notEqual(result.terminal.error.code, "UNKNOWN");
        assert.equal(result.state.is_active, false);
        assert.equal(result.events.some(event => event.type === "run_completed"), false);
        release();
        provider.beforeResponse = undefined;
        const retry = await page.evaluate(() => window.bounded(window.session.turn("BROWSER_AFTER_INTERRUPT"), "turn after interruption"));
        assert.equal(retry.text, "BROWSER_RUNTIME_OK_2");
        assert.ok(JSON.stringify(requests[0]).includes("BROWSER_INTERRUPT_TRANSIENT_MARKER"));
        assert.equal(JSON.stringify(requests[1]).includes("BROWSER_INTERRUPT_TRANSIENT_MARKER"), false);
      } finally { release(); }
    });

    await scenario("transient turn context is admitted for one request and never persisted", async ({ page, requests }) => {
      await page.evaluate(async () => {
        const session = await window.runtime.createSession({ model: window.model });
        await session.turn("First contextual prompt.", { transientTurnContext: "BROWSER_TRANSIENT_PRIVATE_MARKER" });
        await session.turn("A later prompt has no prior transient context.");
      });
      assert.equal(requests.length, 2);
      assert.ok(JSON.stringify(requests[0]).includes("BROWSER_TRANSIENT_PRIVATE_MARKER"));
      assert.equal(JSON.stringify(requests[1]).includes("BROWSER_TRANSIENT_PRIVATE_MARKER"), false);
    });

    await scenario("failed turns release their transient context before retry", async ({ page, requests, provider }) => {
      provider.mode = "failure";
      const failure = await page.evaluate(async () => {
        window.session = await window.runtime.createSession({ model: window.model });
        try {
          await window.session.turn("BROWSER_TRANSIENT_FAILURE", { transientTurnContext: "BROWSER_FAILED_TRANSIENT_MARKER" });
          return { accepted: true };
        } catch (error) { return window.errorEnvelope(error); }
      });
      assert.ok(failure.code, JSON.stringify(failure));
      assert.equal(requests.length, 1);
      assert.ok(JSON.stringify(requests[0]).includes("BROWSER_FAILED_TRANSIENT_MARKER"));
      provider.mode = "success";
      const retry = await page.evaluate(() => window.session.turn("BROWSER_TRANSIENT_CLEAN_RETRY"));
      assert.equal(retry.text, "BROWSER_RUNTIME_OK_2");
      assert.equal(JSON.stringify(requests[1]).includes("BROWSER_FAILED_TRANSIENT_MARKER"), false);
    });

    await scenario("queued transient contexts are applied only to their admitted turn", async ({ page, requests, provider }) => {
      let release;
      let entered;
      const started = new Promise(resolve => { entered = resolve; });
      const gate = new Promise(resolve => { release = resolve; });
      provider.beforeResponse = async () => {
        if (requests.length === 1) { entered(); await gate; }
      };
      try {
        await page.evaluate(async () => {
          window.session = await window.runtime.createSession({ model: window.model });
          window.first = window.session.turn("BROWSER_CONTEXT_QUEUE_ONE", { transientTurnContext: "BROWSER_QUEUE_CONTEXT_ONE" });
        });
        await started;
        await page.evaluate(() => {
          window.second = window.session.turn("BROWSER_CONTEXT_QUEUE_TWO", { transientTurnContext: "BROWSER_QUEUE_CONTEXT_TWO" });
        });
        assert.equal(requests.length, 1);
        release();
        await page.evaluate(() => window.bounded(Promise.all([window.first, window.second]), "queued transient turns"));
        assert.equal(requests.length, 2);
        assert.ok(JSON.stringify(requests[0]).includes("BROWSER_QUEUE_CONTEXT_ONE"));
        assert.equal(JSON.stringify(requests[0]).includes("BROWSER_QUEUE_CONTEXT_TWO"), false);
        assert.ok(JSON.stringify(requests[1]).includes("BROWSER_QUEUE_CONTEXT_TWO"));
        assert.equal(JSON.stringify(requests[1]).includes("BROWSER_QUEUE_CONTEXT_ONE"), false);
      } finally { release(); }
    });

    await scenario("keep-alive is supported on canonical direct sessions with comms", async ({ page, requests }) => {
      const results = await page.evaluate(async () => {
        const values = [];
        for (const keep_alive of [true, false]) {
          const handle = await window.wasm.create_session_simple(JSON.stringify({
            model: window.model, comms_name: `browser-keep-${keep_alive}`, keep_alive,
          }));
          values.push(window.parse(await window.wasm.start_turn(handle, JSON.stringify({ text: "One explicit runtime turn." }))));
          await window.wasm.destroy_session(handle);
        }
        return values;
      });
      assert.equal(requests.length, 2);
      assert.equal(results.length, 2);
      assert.notEqual(results[0].session_id, results[1].session_id);
      assert.ok(results.every(result => result.text.startsWith("BROWSER_RUNTIME_OK_")));
    });

    await scenario("direct peers drain idle messages and complete a correlated request", async ({ page, requests, provider }) => {
      const toolResults = [];
      const actions = new Map();
      let responseArguments;
      provider.chooseTool = body => {
        const content = body.messages.at(-1).content;
        const results = Array.isArray(content) ? content.filter(block => block.type === "tool_result") : [];
        if (results.length) {
          for (const result of results) {
            assert.notEqual(result.is_error, true, JSON.stringify(result));
            toolResults.push(result);
          }
          return undefined;
        }
        const text = typeof content === "string" ? content : content.map(block => block.text ?? "").join("\n");
        if (text.includes("Intent: checksum_token") && text.includes("BROWSER_PEER_SUBJECT")) {
          const projection = text.match(/send_response with arguments (\{[^\n]+?\})\./);
          assert.ok(projection, "reply addressing must be supplied by the canonical request projection");
          responseArguments = JSON.parse(projection[1]);
          return { name: "send_response", input: { ...responseArguments, result: {
            request_intent: "checksum_token", request_subject: "BROWSER_PEER_SUBJECT", token: "BROWSER_PEER_RECEIPT",
          } } };
        }
        for (const [marker, action] of actions) if (text.includes(marker)) return action;
        return undefined;
      };
      await page.evaluate(async () => {
        window.left = await window.runtime.createSession({ model: window.model, commsName: "browser-left", keepAlive: true });
        window.right = await window.runtime.createSession({ model: window.model, commsName: "browser-right", keepAlive: true });
        await window.left.wirePeer(window.right);
        await window.right.wirePeer(window.left);
      });
      assert.equal(requests.length, 0, "trust installation must not start provider work");
      actions.set("BROWSER_DISCOVER_PEERS", { name: "peers", input: {} });
      await page.evaluate(async () => { await window.left.turn("BROWSER_DISCOVER_PEERS"); });
      const peersResult = toolResults[0];
      const peersText = typeof peersResult.content === "string" ? peersResult.content
        : peersResult.content.map(block => block.text ?? "").join("\n");
      const peers = JSON.parse(peersText).peers;
      assert.equal(peers.length, 1);
      assert.equal(peers[0].name, "browser-right");
      const target = peers[0].peer_id;
      assert.equal(typeof target, "string");
      await page.evaluate(() => { window.left.pollEvents(); window.right.pollEvents(); });
      actions.set("BROWSER_SEND_ORDINARY", { name: "send_message", input: {
        peer_id: target, body: "BROWSER_IDLE_MESSAGE", handling_mode: "queue",
      } });
      await page.evaluate(async () => { await window.left.turn("BROWSER_SEND_ORDINARY"); });
      const ordinary = await page.evaluate(async () => {
        const events = [];
        return window.bounded((async () => {
          for (;;) {
            events.push(...window.right.pollEvents());
            if (events.some(event => event.type === "run_completed")) return events;
            await new Promise(resolve => setTimeout(resolve, 10));
          }
        })(), "idle direct peer message");
      });
      assert.ok(ordinary.some(event => event.type === "peer_content_ingested"), JSON.stringify(ordinary));
      assert.ok(requests.some(request => JSON.stringify(request.messages.at(-1)).includes("BROWSER_IDLE_MESSAGE")));
      await page.evaluate(() => { window.left.pollEvents(); window.right.pollEvents(); });
      actions.set("BROWSER_SEND_CORRELATED", { name: "send_request", input: {
        peer_id: target, intent: "checksum_token", params: { subject: "BROWSER_PEER_SUBJECT" }, handling_mode: "queue",
      } });
      await page.evaluate(async () => { await window.left.turn("BROWSER_SEND_CORRELATED"); });
      const correlated = await page.evaluate(async () => {
        const leftEvents = [];
        const rightEvents = [];
        return window.bounded((async () => {
          for (;;) {
            leftEvents.push(...window.left.pollEvents());
            rightEvents.push(...window.right.pollEvents());
            if (JSON.stringify(leftEvents).includes("BROWSER_PEER_RECEIPT")
                && leftEvents.some(event => event.type === "run_completed" && event.identity?.interaction_id)
                && rightEvents.some(event => event.type === "run_completed")
                && !(await window.left.getState()).is_active && !(await window.right.getState()).is_active) {
              return { leftEvents, rightEvents };
            }
            await new Promise(resolve => setTimeout(resolve, 10));
          }
        })(), "direct peer correlated response");
      });
      assert.ok(responseArguments?.in_reply_to, "recipient must observe canonical request identity");
      assert.ok(correlated.leftEvents.some(event => event.type === "run_completed"
        && event.identity?.interaction_id === responseArguments.in_reply_to), JSON.stringify(correlated));
      assert.ok(correlated.rightEvents.some(event => event.type === "run_completed"
        && event.identity?.interaction_id === responseArguments.in_reply_to), JSON.stringify(correlated));
      assert.equal([...correlated.leftEvents, ...correlated.rightEvents].some(event => event.type === "run_failed"), false);
      assert.ok(correlated.rightEvents.some(event => event.type === "peer_content_ingested"), JSON.stringify(correlated));
      assert.ok(toolResults.length >= 4, "peers, message, request, response tools all return through the real provider loop");
    });

    await scenario("real callback tool results enter the next provider request", async ({ page, requests, provider }) => {
      provider.tool = { marker: "BROWSER_CALL_TOOL", name: "browser_echo", input: { value: "literal browser payload" } };
      const result = await page.evaluate(async () => {
        const calls = [];
        window.runtime.registerTool("browser_echo", "Return the supplied browser payload.", {
          type: "object", properties: { value: { type: "string" } }, required: ["value"],
        }, async args => {
          const input = JSON.parse(args);
          calls.push(input);
          return { content: `ECHO:${input.value}`, is_error: false };
        });
        const session = await window.runtime.createSession({ model: window.model });
        const turn = await session.turn("BROWSER_CALL_TOOL");
        return { calls, turn, events: session.pollEvents() };
      });
      assert.deepEqual(result.calls, [{ value: "literal browser payload" }]);
      assert.equal(requests.length, 2);
      assert.ok(JSON.stringify(requests[1].messages.at(-1)).includes("ECHO:literal browser payload"));
      assert.equal(result.turn.text, "BROWSER_RUNTIME_OK_2");
      assert.ok(result.events.some(event => event.type === "tool_call_requested"));
    });

    await scenario("excluded mobpack capabilities refuse with profile codes and clearing actions", async ({ page, requests }) => {
      const errors = await page.evaluate(async exclusions => {
        const errors = [];
        for (const [token, capability, clearingAction] of exclusions) {
          const pack = new Uint8Array(await (await fetch(`/fixture-${token}.mobpack`)).arrayBuffer());
          try { await window.wasm.create_session(pack, JSON.stringify({ model: window.model })); }
          catch (error) { errors.push({ capability, clearingAction, ...window.errorEnvelope(error) }); }
        }
        return errors;
      }, PACK_EXCLUSIONS);
      assert.equal(errors.length, PACK_EXCLUSIONS.length);
      for (const error of errors) {
        assert.equal(error.code, "CAPABILITY_UNAVAILABLE");
        assert.deepEqual(error.data, {
          profile: "browser", capability: error.capability, clearing_action: error.clearingAction,
        });
      }
      assert.equal(requests.length, 0);
    });

    await scenario("required capabilities refuse both bootstrap ingresses without replacing live state", async ({ page, requests }) => {
      const result = await page.evaluate(async exclusions => {
        const existing = await window.runtime.createSession({ model: window.model });
        const originalId = await existing.sessionId;
        const errors = [];
        for (const [capability, clearingAction] of exclusions) {
          try {
            await window.sdk.MeerkatRuntime.init(window.wasm, {
              anthropicApiKey: "synthetic-not-a-key", model: window.model, requiredCapabilities: [capability],
            });
            errors.push({ accepted: true, capability, clearingAction });
          } catch (error) { errors.push({ capability, clearingAction, ...window.errorEnvelope(error) }); }
        }
        const pack = new Uint8Array(await (await fetch("/fixture.mobpack")).arrayBuffer());
        try {
          await window.wasm.init_runtime(pack, JSON.stringify({
            anthropic_api_key: "synthetic-not-a-key", model: window.model,
            required_capabilities: ["background_execution"], mobpack_trust: { policy: "permissive" },
          }));
          errors.push({ accepted: true });
        } catch (error) { errors.push({
          capability: "background_execution", clearingAction: "use_background_runtime", ...window.errorEnvelope(error),
        }); }
        return { errors, originalId, currentId: await existing.sessionId };
      }, PROFILE_EXCLUSIONS);
      assert.equal(result.errors.length, PROFILE_EXCLUSIONS.length + 1);
      for (const error of result.errors) {
        assert.equal(error.code, "CAPABILITY_UNAVAILABLE", JSON.stringify(error));
        assert.deepEqual(error.data, {
          profile: "browser", capability: error.capability, clearing_action: error.clearingAction,
        });
      }
      assert.equal(result.currentId, result.originalId, "rejected replacement must retain the initialized runtime");
      assert.equal(requests.length, 0);
    });

    await scenario("supported runtime requirements are accepted by the same browser profile", async ({ page, requests }) => {
      const result = await page.evaluate(async requiredCapabilities => {
        window.runtime = await window.sdk.MeerkatRuntime.init(window.wasm, {
          anthropicApiKey: "synthetic-not-a-key", anthropicBaseUrl: `${location.origin}/anthropic`,
          model: window.model, requiredCapabilities,
        });
        const session = await window.runtime.createSession({ model: window.model, commsName: "supported-browser", keepAlive: true });
        return session.turn("BROWSER_SUPPORTED_PROFILE", { transientTurnContext: "BROWSER_SUPPORTED_TRANSIENT" });
      }, PROFILE_SUPPORTED);
      assert.equal(result.text, "BROWSER_RUNTIME_OK_1");
      assert.equal(requests.length, 1);
      assert.ok(JSON.stringify(requests[0]).includes("BROWSER_SUPPORTED_TRANSIENT"));
    });

    await scenario("per-session requirements refuse both creation ingresses before service allocation", async ({ page, requests }) => {
      const result = await page.evaluate(async exclusions => {
        window.runtime = await window.sdk.MeerkatRuntime.init(window.wasm, {
          anthropicApiKey: "synthetic-not-a-key", anthropicBaseUrl: `${location.origin}/anthropic`,
          model: window.model, maxSessions: 1, mobpackTrust: { policy: "permissive" },
        });
        const pack = new Uint8Array(await (await fetch("/fixture.mobpack")).arrayBuffer());
        const errors = [];
        for (const [capability, clearingAction] of exclusions) {
          for (const ingress of ["simple", "pack"]) {
            const config = JSON.stringify({ model: window.model, required_capabilities: [capability] });
            try {
              if (ingress === "simple") await window.wasm.create_session_simple(config);
              else await window.wasm.create_session(pack, config);
              errors.push({ capability, clearingAction, ingress, accepted: true });
            } catch (error) { errors.push({ capability, clearingAction, ingress, ...window.errorEnvelope(error) }); }
          }
        }
        const admitted = await window.runtime.createSession({ model: window.model });
        return { errors, id: await admitted.sessionId };
      }, PROFILE_EXCLUSIONS);
      assert.equal(result.errors.length, PROFILE_EXCLUSIONS.length * 2);
      for (const error of result.errors) {
        assert.equal(error.code, "CAPABILITY_UNAVAILABLE", JSON.stringify(error));
        assert.deepEqual(error.data, {
          profile: "browser", capability: error.capability, clearing_action: error.clearingAction,
        });
      }
      assert.equal(typeof result.id, "string");
      assert.equal(requests.length, 0);
    });

    await scenario("remote member placement refuses through the same capability profile", async ({ page, requests }) => {
      const result = await page.evaluate(async () => {
        const mob = await window.runtime.createMob({ id: "browser-profile-test", profiles: { worker: { model: window.model } } });
        let failure;
        try {
          await window.wasm.mob_spawn(mob.mobId, JSON.stringify([{
            profile: "worker", agent_identity: "remote-worker", placement: "remote-host",
          }]));
        } catch (error) { failure = window.errorEnvelope(error); }
        return { failure, members: await mob.listMembers() };
      });
      assert.equal(result.failure?.code, "CAPABILITY_UNAVAILABLE");
      assert.deepEqual(result.failure.data, {
        profile: "browser", capability: "remote_member_placement", clearing_action: "use_local_member_placement",
      });
      assert.deepEqual(result.members, []);
      assert.equal(requests.length, 0);
    });

    await scenario("unsupported mob tools refuse at the shared factory boundary", async ({ page, requests }) => {
      const requirements = [
        ["shell", "shell", "use_host_process_runtime"],
        ["schedule", "schedule", "use_schedule_runtime"],
        ["workgraph", "work_graph", "use_work_graph_runtime"],
        ["memory", "semantic_memory", "use_semantic_memory_runtime"],
        ["image_generation", "image_generation", "use_image_generation_runtime"],
      ];
      const results = await page.evaluate(async requirements => {
        const results = [];
        for (const [tool, capability, clearingAction] of requirements) {
          const mob = await window.runtime.createMob({ id: `browser-${tool}-profile`, profiles: {
            worker: { model: window.model, tools: { [tool]: true, comms: true } },
          } });
          const rows = window.parse(await window.wasm.mob_spawn(mob.mobId, JSON.stringify([{
            profile: "worker", agent_identity: `${tool}-worker`,
          }])));
          results.push({ capability, clearingAction, rows, members: await mob.listMembers() });
        }
        return results;
      }, requirements);
      assert.equal(results.length, requirements.length);
      for (const result of results) {
        assert.equal(result.rows.length, 1);
        assert.equal(result.rows[0].status, "failed", JSON.stringify(result));
        assert.equal(result.rows[0].result.code, "CAPABILITY_UNAVAILABLE", JSON.stringify(result));
        assert.deepEqual(result.rows[0].result.structured_data, {
          profile: "browser", capability: result.capability, clearing_action: result.clearingAction,
        });
        assert.deepEqual(result.members, []);
      }
      assert.equal(requests.length, 0);
    });

    await scenario("mob file skills refuse through the shared capability profile", async ({ page, requests }) => {
      const result = await page.evaluate(async () => {
        const mob = await window.runtime.createMob({
          id: "browser-file-skill-profile",
          skills: { external: { source: "path", path: "/unavailable/SKILL.md" } },
          profiles: { worker: { model: window.model, skills: ["external"], tools: { comms: true } } },
        });
        const rows = window.parse(await window.wasm.mob_spawn(mob.mobId, JSON.stringify([{
          profile: "worker", agent_identity: "file-skill-worker",
        }])));
        return { rows, members: await mob.listMembers() };
      });
      assert.equal(result.rows.length, 1);
      assert.equal(result.rows[0].status, "failed", JSON.stringify(result));
      assert.equal(result.rows[0].result.code, "CAPABILITY_UNAVAILABLE", JSON.stringify(result));
      assert.deepEqual(result.rows[0].result.structured_data, {
        profile: "browser", capability: "runtime_skills", clearing_action: "use_skill_runtime",
      });
      assert.deepEqual(result.members, []);
      assert.equal(requests.length, 0);
    });

    await scenario("file schema flows refuse with a typed inline schema clearing action", async ({ page, requests }) => {
      const result = await page.evaluate(async () => {
        const mob = await window.runtime.createMob({
          id: "browser-file-schema-profile",
          profiles: { worker: { model: window.model, tools: { comms: true } } },
          flows: { file_schema: { steps: { check: {
            role: "worker", message: "FILE_SCHEMA_MUST_NOT_RUN", expected_schema_ref: "/unavailable/schema.json",
          } } } },
        });
        await mob.spawn([{ profile: "worker", agent_identity: "schema-worker", runtime_mode: "turn_driven" }]);
        let failure;
        try { await window.wasm.mob_run_flow(mob.mobId, "file_schema", "{}"); }
        catch (error) { failure = window.errorEnvelope(error); }
        return { failure, members: await mob.listMembers() };
      });
      assert.equal(result.failure?.code, "CAPABILITY_UNAVAILABLE", JSON.stringify(result));
      assert.deepEqual(result.failure.data, {
        profile: "browser", capability: "file_schema_resolution", clearing_action: "use_inline_schema",
      });
      assert.equal(result.members.length, 1);
      assert.equal(requests.length, 0);
    });

    await scenario("destroy retires the handle and rejects stale work before dispatch", async ({ page, requests }) => {
      const errors = await page.evaluate(async () => {
        const session = await window.runtime.createSession({ model: window.model });
        await session.destroy();
        const errors = [];
        for (const action of [() => session.getState(), () => session.pollEvents(), () => session.turn("stale turn")]) {
          try { await action(); errors.push({ accepted: true }); }
          catch (error) { errors.push(window.errorEnvelope(error)); }
        }
        return errors;
      });
      assert.ok(errors.every(error => error.code === "invalid_session_handle"), JSON.stringify(errors));
      assert.equal(requests.length, 0);
    });

    await scenario("replacing a runtime never aliases an old session handle to a new session", async ({ page, requests }) => {
      const result = await page.evaluate(async () => {
        const old = await window.runtime.createSession({ model: window.model });
        const oldId = await old.sessionId;
        window.runtime = await window.sdk.MeerkatRuntime.init(window.wasm, {
          anthropicApiKey: "synthetic-not-a-key", anthropicBaseUrl: `${location.origin}/anthropic`, model: window.model,
        });
        const current = await window.runtime.createSession({ model: window.model });
        const errors = [];
        for (const action of [() => old.getState(), () => old.turn("OLD_HANDLE_MUST_NOT_EXECUTE")]) {
          try { await action(); errors.push({ accepted: true }); }
          catch (error) { errors.push(window.errorEnvelope(error)); }
        }
        return { oldId, currentId: await current.sessionId, errors };
      });
      assert.notEqual(result.oldId, result.currentId);
      assert.ok(result.errors.every(error => error.code === "invalid_session_handle"), JSON.stringify(result));
      assert.equal(requests.length, 0);
    });

    await scenario("stale SDK runtime destruction cannot tear down its replacement", async ({ page, requests }) => {
      const rows = await page.evaluate(async () => {
        const config = {
          anthropicApiKey: "synthetic-not-a-key", anthropicBaseUrl: `${location.origin}/anthropic`, model: window.model,
        };
        const rows = [];
        for (const explicitlyDestroyed of [true, false]) {
          const old = window.runtime;
          if (explicitlyDestroyed) await old.destroy();
          window.runtime = await window.sdk.MeerkatRuntime.init(window.wasm, config);
          const current = await window.runtime.createSession({ model: window.model });
          const id = await current.sessionId;
          let staleDestroy;
          try { await old.destroy(); staleDestroy = { resolved: true }; }
          catch (error) { staleDestroy = window.errorEnvelope(error); }
          rows.push({ id, after: (await current.getState()).session_id, staleDestroy,
            turn: await current.turn("BROWSER_CURRENT_RUNTIME_SURVIVES_STALE_WRAPPER") });
        }
        return rows;
      });
      assert.equal(rows.length, 2);
      assert.ok(rows.every(row => row.id === row.after && row.turn.session_id === row.id), JSON.stringify(rows));
      assert.equal(requests.length, 2);
    });

    await scenario("closed browser requests reject unknown fields without replacing or allocating runtime state", async ({ page, requests }) => {
      const result = await page.evaluate(async () => {
        const session = await window.runtime.createSession({ model: window.model });
        const id = await session.sessionId;
        const pack = new Uint8Array(await (await fetch("/fixture.mobpack")).arrayBuffer());
        const errors = [];
        for (const [key, value] of [
          ["api_key", "unused-per-session-key"], ["anthropic_base_url", "https://invalid.example"],
          ["shell", true], ["backend", "jsonl"], ["skill_references", ["external-skill"]],
          ["runtime_mode", "autonomous_host"], ["mob_id", "surface-owned-membership"],
          ["keepAlive", true], ["unknown_field", true],
        ]) {
          for (const ingress of ["simple", "pack"]) {
            const config = JSON.stringify({ model: window.model, [key]: value });
            try {
              if (ingress === "simple") await window.wasm.create_session_simple(config);
              else await window.wasm.create_session(pack, config);
              errors.push({ ingress, key, accepted: true });
            } catch (error) { errors.push({ ingress, key, ...window.errorEnvelope(error) }); }
          }
        }
        const bootstrap = JSON.stringify({ anthropic_api_key: "synthetic-not-a-key", model: window.model,
          mobpack_trust: { policy: "permissive" }, unknown_field: true });
        for (const ingress of ["config", "pack"]) {
          try {
            if (ingress === "config") await window.wasm.init_runtime_from_config(bootstrap);
            else await window.wasm.init_runtime(pack, bootstrap);
            errors.push({ ingress, accepted: true });
          } catch (error) { errors.push({ ingress, ...window.errorEnvelope(error) }); }
        }
        return { errors, id, after: (await session.getState()).session_id };
      });
      assert.equal(result.errors.length, 20);
      for (const error of result.errors) {
        assert.equal(error.code, error.ingress === "pack" && !error.key ? "invalid_credentials" : "invalid_config", JSON.stringify(error));
        assert.match(error.message, /unknown field/);
      }
      assert.equal(result.after, result.id);
      assert.equal(requests.length, 0);
    });

    await scenario("direct sessions and mob members share one service capacity", async ({ page, requests }) => {
      const blocked = await page.evaluate(async () => {
        window.runtime = await window.sdk.MeerkatRuntime.init(window.wasm, {
          anthropicApiKey: "synthetic-not-a-key", anthropicBaseUrl: `${location.origin}/anthropic`, model: window.model, maxSessions: 1,
        });
        window.capacityDirect = await window.runtime.createSession({ model: window.model });
        window.capacityMob = await window.runtime.createMob({ id: "shared-service-capacity", profiles: { worker: { model: window.model, tools: { comms: true } } } });
        return window.parse(await window.wasm.mob_spawn(window.capacityMob.mobId, JSON.stringify([{
          profile: "worker", agent_identity: "blocked-member", runtime_mode: "turn_driven",
        }])));
      });
      assert.equal(blocked[0].status, "failed", JSON.stringify(blocked));
      assert.equal(requests.length, 0, "refused admission must not dispatch provider work");
      const result = await page.evaluate(async () => {
        await window.capacityDirect.destroy();
        const admitted = window.parse(await window.wasm.mob_spawn(window.capacityMob.mobId, JSON.stringify([{
          profile: "worker", agent_identity: "admitted-member", runtime_mode: "turn_driven",
        }])));
        return { admitted, members: await window.capacityMob.listMembers() };
      });
      assert.equal(result.admitted[0].status, "spawned", JSON.stringify(result));
      assert.equal(result.members.length, 1);
      assert.equal(result.members[0].agent_identity, "admitted-member");
      assert.equal(requests.length, 0);
    });

    await scenario("creation races serialize with runtime replacement and teardown", async ({ page, requests }) => {
      const result = await page.evaluate(async () => {
        const config = {
          anthropicApiKey: "synthetic-not-a-key", anthropicBaseUrl: `${location.origin}/anthropic`, model: window.model,
        };
        const creating = window.runtime.createSession({ model: window.model, commsName: "racing-session", keepAlive: true });
        const replacing = window.sdk.MeerkatRuntime.init(window.wasm, config);
        const [old, replacement] = await window.bounded(Promise.all([creating, replacing]), "creation and replacement race");
        window.runtime = replacement;
        let stale;
        try { await old.getState(); stale = { accepted: true }; }
        catch (error) { stale = window.errorEnvelope(error); }
        const current = await window.runtime.createSession({ model: window.model });
        const currentId = await current.sessionId;
        const creatingMob = window.runtime.createMob({ id: "racing-mob", profiles: { worker: { model: window.model } } });
        const destroying = window.runtime.destroy();
        await window.bounded(Promise.all([creatingMob, destroying]), "mob creation and teardown race");
        let retired;
        try { await window.wasm.mob_status("racing-mob"); retired = { accepted: true }; }
        catch (error) { retired = window.errorEnvelope(error); }
        window.runtime = await window.sdk.MeerkatRuntime.init(window.wasm, config);
        const fresh = await window.runtime.createMob({ id: "racing-mob", profiles: { worker: { model: window.model } } });
        return { stale, retired, currentId, members: await fresh.listMembers() };
      });
      assert.equal(result.stale.code, "invalid_session_handle", JSON.stringify(result));
      assert.equal(result.retired.code, "not_initialized", JSON.stringify(result));
      assert.equal(typeof result.currentId, "string");
      assert.deepEqual(result.members, []);
      assert.equal(requests.length, 0);
    });

    await scenario("multiple mobs retain exact runtime lifecycle signal ownership during teardown", async ({ page, requests }) => {
      const result = await page.evaluate(async () => {
        const mobs = [];
        for (const name of ["first", "second"]) {
          const mob = await window.runtime.createMob({ id: `browser-teardown-${name}`, profiles: {
            worker: { model: window.model, tools: { comms: true } },
          } });
          await mob.spawn([{ profile: "worker", agent_identity: "worker", runtime_mode: "turn_driven" }]);
          mobs.push(mob);
        }
        const second = await mobs[1].lifecycle("destroy");
        const first = await mobs[0].lifecycle("destroy");
        const remaining = await window.runtime.listMobs();
        return { first, second, remaining };
      });
      assert.equal(result.first.ok, true);
      assert.equal(result.second.ok, true);
      assert.deepEqual(result.remaining, []);
      assert.equal(requests.length, 0);
    });

    await scenario("busy mob Stop reaches its owner before the provider boundary and prevents tool dispatch", async ({ page, requests, provider }) => {
      let release;
      let entered;
      const started = new Promise(resolve => { entered = resolve; });
      const gate = new Promise(resolve => { release = resolve; });
      provider.beforeResponse = async () => {
        if (requests.length === 1) { entered(); await gate; }
      };
      provider.tool = { marker: "BROWSER_BUSY_MOB_STOP", name: "after_stop_probe", input: {} };
      let beforeRelease;
      let observationError;
      try {
        const spawned = await page.evaluate(async () => {
          window.afterStopToolCalls = 0;
          window.runtime.registerTool("after_stop_probe", "Record an observable tool dispatch.", {
            type: "object", properties: {},
          }, async () => {
            window.afterStopToolCalls++;
            return { content: "BROWSER_TOOL_RAN_AFTER_STOP", is_error: false };
          });
          window.busyMob = await window.runtime.createMob({ id: "browser-busy-stop", profiles: {
            worker: { model: window.model, tools: { comms: true }, runtime_mode: "autonomous_host" },
          } });
          const rows = await window.busyMob.spawn([{
            profile: "worker", agent_identity: "worker", runtime_mode: "autonomous_host",
            initial_message: "BROWSER_BUSY_MOB_STOP",
          }]);
          window.busyEvents = await window.busyMob.subscribeMemberEvents("worker");
          return rows;
        });
        assert.equal(spawned.length, 1);
        assert.equal(spawned[0].agent_identity, "worker");
        assert.equal(spawned[0].mob_id, "browser-busy-stop");
        assert.ok(spawned[0].member_ref);
        await started;
        assert.equal(requests.length, 1);
        // Public event polling is an actor command, not a local JS flag. It
        // must observe the durable Stop fence while the response is held.
        // A transcript export queued behind this busy turn blocks both Stop
        // and this read; a presence-only observation lets cancellation start.
        beforeRelease = await page.evaluate(async () => {
          window.stopSettled = false;
          window.busyStop = window.busyMob.lifecycle("stop").then(
            value => { window.stopSettled = true; return { value }; },
            error => { window.stopSettled = true; return { error: window.errorEnvelope(error) }; },
          );
          const ledger = await window.bounded((async () => {
            for (;;) {
              const events = await window.busyMob.events();
              if (events.some(event => event.kind.type === "placed_completion_lifecycle_quiesce_started")) return events;
            }
          })(), "Stop owner acknowledgement while provider response is held");
          return { ledger, stopSettled: window.stopSettled, calls: window.afterStopToolCalls };
        }).catch(error => { observationError = error.message; });
      } finally { release(); }
      const terminal = await page.evaluate(async () => {
        const stop = await window.bounded(window.busyStop, "busy mob Stop terminal after response boundary");
        const status = await window.busyMob.status();
        const events = window.busyEvents.poll();
        const ledger = await window.busyMob.events();
        window.busyEvents.close();
        return { stop, status, events, ledger, calls: window.afterStopToolCalls };
      });
      console.log("BUSY_MOB_STOP_OBSERVATION", JSON.stringify({
        beforeRelease, observationError, terminal, providerRequests: requests.length,
      }));
      assert.equal(observationError, undefined, "Stop must not wait for busy transcript export before cancellation");
      assert.equal(beforeRelease.stopSettled, false, "Stop cannot acknowledge terminal drain before the held boundary");
      assert.equal(beforeRelease.calls, 0);
      assert.equal(terminal.stop.error, undefined, JSON.stringify(terminal));
      assert.equal(terminal.stop.value.ok, true);
      assert.equal(terminal.status.status, "Stopped");
      assert.equal(terminal.calls, 0, "Stop cancellation must prevent the tool dispatch at the next provider boundary");
      assert.equal(requests.length, 1, "cancelled tool response must not start a follow-up provider turn");
      assert.ok(terminal.ledger.some(event => event.kind.type === "mob_stopped"));
      assert.ok(terminal.events.some(event => event.payload.type === "run_failed"
        && event.payload.error_report.class === "cancelled"), JSON.stringify(terminal.events));
      assert.equal(terminal.events.some(event => event.payload.type === "run_completed"), false);
    });

    await scenario("helper callback can create a direct session without lifecycle lock reentrancy", async ({ page, requests, provider }) => {
      provider.tool = { marker: "BROWSER_HELPER_CREATE_SESSION", name: "create_browser_session", input: {} };
      const result = await page.evaluate(async () => {
        const created = [];
        window.runtime.registerTool("create_browser_session", "Create a deferred direct browser session.", { type: "object", properties: {} }, async () => {
          const session = await window.runtime.createSession({ model: window.model });
          created.push(await session.sessionId);
          await session.destroy();
          return { content: "BROWSER_HELPER_REENTRANCY_OK", is_error: false };
        });
        const mob = await window.runtime.createMob({ id: "browser-reentrant-helper", profiles: { worker: {
          model: window.model, tools: { comms: true },
        } } });
        const helper = await window.bounded(mob.spawnHelper("BROWSER_HELPER_CREATE_SESSION", {
          agentIdentity: "temporary-helper", profileName: "worker", resultLabel: "answer", maxTextBytes: 4096,
        }), "reentrant helper turn");
        return { created, helper };
      });
      assert.equal(result.created.length, 1);
      assert.equal(result.helper.output, "BROWSER_RUNTIME_OK_2");
      assert.equal(requests.length, 2);
      assert.ok(JSON.stringify(requests[1]).includes("BROWSER_HELPER_REENTRANCY_OK"));
    });

    await scenario("spawned and forked helpers preserve profile refusal code and cleanup", async ({ page, requests }) => {
      await page.evaluate(async () => {
        window.helperMob = await window.runtime.createMob({ id: "browser-refused-helpers", profiles: {
          source: { model: window.model, tools: { comms: true } },
          shell: { model: window.model, tools: { comms: true, shell: true } },
        } });
        await window.helperMob.spawn([{ profile: "source", agent_identity: "source", runtime_mode: "turn_driven" }]);
      });
      const before = requests.length;
      const result = await page.evaluate(async () => {
        const errors = [];
        for (const mode of ["spawn", "fork"]) {
          const options = { agentIdentity: `refused-${mode}`, profileName: "shell", resultLabel: "answer", maxTextBytes: 4096 };
          try {
            if (mode === "spawn") await window.helperMob.spawnHelper("BROWSER_REFUSED_HELPER", options);
            else await window.helperMob.forkHelper("source", "BROWSER_REFUSED_HELPER", options);
            errors.push({ mode, accepted: true });
          } catch (error) { errors.push({ mode, ...window.errorEnvelope(error) }); }
        }
        return { errors, members: await window.helperMob.listMembers() };
      });
      assert.equal(result.errors.length, 2);
      for (const error of result.errors) {
        assert.equal(error.code, "CAPABILITY_UNAVAILABLE", JSON.stringify(error));
        assert.deepEqual(error.data, {
          profile: "browser", capability: "shell", clearing_action: "use_host_process_runtime",
        });
      }
      assert.deepEqual(result.members.map(member => member.agent_identity), ["source"]);
      assert.equal(requests.length, before, "refused helpers must not dispatch provider work");
    });

    await scenario("runtime teardown cancels a pending helper before provider completion", async ({ page, requests, provider }) => {
      let release;
      let entered;
      const started = new Promise(resolve => { entered = resolve; });
      const gate = new Promise(resolve => { release = resolve; });
      provider.beforeResponse = async () => { entered(); await gate; };
      try {
        await page.evaluate(async () => {
          window.helperMob = await window.runtime.createMob({ id: "browser-pending-helper", profiles: { worker: {
            model: window.model, tools: { comms: true },
          } } });
          window.pending = window.helperMob.spawnHelper("BROWSER_HELPER_PENDING", {
            agentIdentity: "pending-helper", profileName: "worker", resultLabel: "answer", maxTextBytes: 4096,
          }).then(value => ({ value }), error => ({ error: window.errorEnvelope(error) }));
        });
        await started;
        const result = await page.evaluate(async () => {
          await window.bounded(window.runtime.destroy(), "pending helper teardown");
          return window.bounded(window.pending, "pending helper terminal");
        });
        assert.ok(result.error?.code, JSON.stringify(result));
        assert.equal(requests.length, 1);
      } finally { release(); }
    });

    await scenario("runtime teardown cancels active work before returning", async ({ page, requests, provider }) => {
      let release;
      let entered;
      const started = new Promise(resolve => { entered = resolve; });
      const gate = new Promise(resolve => { release = resolve; });
      provider.beforeResponse = async () => { entered(); await gate; };
      try {
        await page.evaluate(async () => {
          window.session = await window.runtime.createSession({ model: window.model, commsName: "teardown-agent", keepAlive: true });
          window.pending = window.session.turn("BROWSER_TEARDOWN_PENDING").then(value => ({ value }), error => ({ error: window.errorEnvelope(error) }));
        });
        await started;
        const result = await page.evaluate(async () => {
          await window.bounded(window.runtime.destroy(), "active runtime teardown");
          const terminal = await window.bounded(window.pending, "teardown terminal result");
          let stateError;
          try { await window.session.getState(); }
          catch (error) { stateError = window.errorEnvelope(error); }
          return { terminal, stateError };
        });
        assert.ok(result.terminal.error?.code, JSON.stringify(result));
        assert.equal(result.stateError.code, "not_initialized");
        assert.equal(requests.length, 1);
      } finally { release(); }
    });

    await scenario("both mobpack creation ingresses feed the same direct-session runtime", async ({ page, requests }) => {
      const results = await page.evaluate(async () => {
        const pack = new Uint8Array(await (await fetch("/fixture.mobpack")).arrayBuffer());
        const config = JSON.stringify({ model: window.model, comms_name: "pack-direct", keep_alive: true });
        const handle = await window.wasm.create_session(pack, config);
        const direct = window.parse(await window.wasm.start_turn(handle, JSON.stringify({ text: "Read the packed browser guide." })));
        await window.wasm.destroy_session(handle);
        await window.runtime.destroy();
        window.runtime = await window.sdk.MeerkatRuntime.initFromMobpack(window.wasm, pack, {
          anthropicApiKey: "synthetic-not-a-key", anthropicBaseUrl: `${location.origin}/anthropic`,
          model: window.model, mobpackTrust: { policy: "permissive" },
        });
        const session = await window.runtime.createSession({ model: window.model });
        const bootstrap = await session.turn("Read the retained bootstrap browser guide.");
        return { direct, bootstrap };
      });
      assert.equal(requests.length, 2);
      assert.equal(results.direct.text, "BROWSER_RUNTIME_OK_1");
      assert.equal(results.bootstrap.text, "BROWSER_RUNTIME_OK_2");
      assert.ok(requests.every(request => JSON.stringify(request.system).includes("browser-guide")));
    });
  } finally {
    await browser.close();
    await server.close();
  }
});
