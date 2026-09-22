const { test } = require("node:test");
const assert = require("node:assert/strict");
const vm = require("node:vm");
const fs = require("node:fs");
const path = require("node:path");
const ts = require("typescript");
const { runSmoke, TEST_TIMEOUT_MS, OPERATION_TIMEOUT_MS } = require("../.smoke/smoke.cjs");

test("ICE listener/timer ownership on success, deadline, cancellation and peer closure", async (t) => {
  const { waitForIceGatheringComplete } = await import("../public/live-session.js");
  t.mock.timers.enable({ apis: ["setTimeout"] });
  class Peer extends EventTarget {
    iceGatheringState = "gathering";
    connectionState = "new";
    signalingState = "stable";
    listeners = new Set();
    addEventListener(type, fn) { this.listeners.add(type); super.addEventListener(type, fn); }
    removeEventListener(type, fn) { this.listeners.delete(type); super.removeEventListener(type, fn); }
  }
  for (const outcome of ["already", "complete", "timeout", "abort", "closed", "failed", "signaling"]) {
    const peer = new Peer();
    const controller = new AbortController();
    if (outcome === "already") peer.iceGatheringState = "complete";
    const pending = waitForIceGatheringComplete(peer, controller.signal, 10);
    if (outcome === "complete") {
      peer.iceGatheringState = "complete";
      peer.dispatchEvent(new Event("icegatheringstatechange"));
    }
    if (outcome === "timeout") t.mock.timers.tick(11);
    if (outcome === "abort") controller.abort(new Error("user stop"));
    if (["closed", "failed"].includes(outcome)) {
      peer.connectionState = outcome;
      peer.dispatchEvent(new Event("connectionstatechange"));
    }
    if (outcome === "signaling") {
      peer.signalingState = "closed";
      peer.dispatchEvent(new Event("signalingstatechange"));
    }
    if (["already", "complete"].includes(outcome)) await pending;
    else await assert.rejects(pending);
    assert.equal(peer.listeners.size, 0);
    t.mock.timers.tick(100);
  }
});

test("transcript identity protects written output and independently addressed items", async () => {
  const { Transcript } = await import("../public/transcript.js");
  for (const oldComplete of [true, false]) {
    const transcript = new Transcript();
    const old = { response_id: "old", provider_item_id: "audio", content_index: 0 };
    const fresh = { response_id: "fresh", provider_item_id: "new", content_index: 0 };
    transcript.delta({ ...old, delta: "old spoken" }, "spoken", "t");
    transcript.delta({ ...old, provider_item_id: "text", delta: "written" }, "written", "t");
    transcript.interrupt({ response_id: "old" }, "t");
    transcript.interrupt({ ...old, text: "heard" }, "t", true);
    if (oldComplete) transcript.complete({ response_id: "old" });
    transcript.delta({ ...fresh, delta: "fresh" }, "spoken", "t");
    transcript.delta({ ...old, delta: "late" }, "spoken", "t");
    transcript.final({ ...old, text: "stale final" }, "t");
    transcript.complete({ response_id: "old" });
    assert.equal(transcript.turns[2].text, "fresh");
    assert.equal(transcript.turns[2].draft, true);
    assert.deepEqual(transcript.turns.slice(0, 2).map((row) => [row.text, row.draft]), [["heard", false], ["written", false]]);
  }
  const transcript = new Transcript();
  transcript.delta({ response_id: "text", delta: "written only" }, "written", "t");
  transcript.interrupt({ response_id: "text" }, "t");
  transcript.delta({ response_id: "text", delta: " continued" }, "written", "t");
  transcript.complete({ response_id: "text" });
  assert.equal(transcript.turns[0].text, "written only continued");
  assert.equal(transcript.turns[0].draft, false);
  for (const index of [0, 1]) {
    transcript.delta({ response_id: "multi", provider_item_id: "item", content_index: index, delta: `part${index}` }, "spoken", "t");
  }
  transcript.interrupt({ response_id: "multi", provider_item_id: "item", content_index: 0, text: "prefix" }, "t", true);
  assert.equal(transcript.turns[1].text, "prefix");
  assert.equal(transcript.turns[2].draft, true);
  transcript.interrupt({}, "t");
  assert.equal(transcript.turns[2].draft, false);
  assert.equal(transcript.turns[0].text, "written only continued");
  transcript.delta({ delta: "unattributable late audio" }, "spoken", "t");
  assert.equal(transcript.turns.length, 3);
  const anonymous = new Transcript();
  anonymous.delta({ delta: "anonymous written" }, "written", "t");
  anonymous.interrupt({}, "t");
  anonymous.delta({ delta: " continued" }, "written", "t");
  anonymous.complete({});
  assert.equal(anonymous.turns.length, 1);
  assert.equal(anonymous.turns[0].text, "anonymous written continued");
  assert.equal(anonymous.turns[0].draft, false);
  const partial = new Transcript();
  partial.delta({ response_id: "partial", delta: "partial identity" }, "spoken", "t");
  partial.final({ response_id: "partial", provider_item_id: "now-known", content_index: 0, text: "final" }, "t");
  assert.equal(partial.turns.length, 1);
  assert.equal(partial.turns[0].itemId, "now-known");
  assert.equal(partial.turns[0].draft, false);
});

test("identified interruptions reject anonymous/partial late spoken output and infer turn scope", async () => {
  const { Transcript } = await import("../public/transcript.js");
  for (const interruption of [{ response_id: "old" }, {}]) {
    const transcript = new Transcript();
    for (const item of ["one", "two"]) {
      transcript.delta({ response_id: "old", provider_item_id: item, content_index: 0, delta: item }, "spoken", "t");
    }
    transcript.delta({ response_id: "old", provider_item_id: "written", delta: "written" }, "written", "t");
    transcript.interrupt(interruption, "t");
    transcript.delta({ response_id: "new", provider_item_id: "new-item", delta: "new" }, "spoken", "t");
    transcript.delta({ delta: "LATE OLD ANONYMOUS" }, "spoken", "t");
    transcript.final({ text: "LATE OLD ANONYMOUS FINAL" }, "t");
    transcript.delta({ response_id: "old", provider_item_id: "three", delta: "LATE SAME RESPONSE" }, "spoken", "t");
    transcript.delta({ provider_item_id: "one", content_index: 1, delta: "LATE ITEM ONLY" }, "spoken", "t");
    transcript.delta({ response_id: "new", provider_item_id: "one", delta: "CONFLICTING ITEM" }, "spoken", "t");
    transcript.delta({ provider_item_id: "new-item", delta: " continued" }, "spoken", "t");
    transcript.delta({ provider_item_id: "written", delta: " preserved" }, "written", "t");
    assert.equal(transcript.turns.length, 4);
    assert.deepEqual(transcript.turns.slice(0, 2).map((row) => [row.draft, row.interrupted]), [[false, true], [false, true]]);
    assert.equal(transcript.turns[2].text, "written preserved");
    assert.equal(transcript.turns[3].text, "new continued");
    assert.equal(transcript.turns[3].draft, true);
  }
});

test("item truncation refuses ambiguous partial spoken output but preserves identified siblings", async () => {
  const { Transcript } = await import("../public/transcript.js");
  for (const withPriorDelta of [true, false]) {
    const transcript = new Transcript();
    const first = { response_id: "shared", provider_item_id: "one", content_index: 0 };
    const second = { response_id: "shared", provider_item_id: "two", content_index: 0 };
    if (withPriorDelta) transcript.delta({ ...first, delta: "one" }, "spoken", "t");
    transcript.delta({ ...second, delta: "two" }, "spoken", "t");
    transcript.interrupt({ ...first, text: "heard" }, "t", true);
    transcript.delta({ response_id: "shared", delta: "LATE TRUNCATED ITEM" }, "spoken", "t");
    transcript.final({ response_id: "shared", text: "LATE FINAL" }, "t");
    assert.equal(transcript.turns.length, 2);
    transcript.delta({ ...second, delta: " continued" }, "spoken", "t");
    transcript.delta({ response_id: "shared", delta: "written" }, "written", "t");
    transcript.delta({ response_id: "fresh", provider_item_id: "fresh-item", delta: "fresh" }, "spoken", "t");
    assert.deepEqual(transcript.turns.map((row) => row.text).sort(),
      ["fresh", "heard", "two continued", "written"].sort());
  }
  const transcript = new Transcript();
  transcript.interrupt({ response_id: "unseen", provider_item_id: "one", content_index: 0 }, "t", true);
  transcript.delta({ response_id: "unseen", delta: "late before any row" }, "spoken", "t");
  assert.equal(transcript.turns.length, 0);
  transcript.delta({ response_id: "unseen", provider_item_id: "one", content_index: 1, delta: "other content" }, "spoken", "t");
  assert.equal(transcript.turns[0].text, "other content");
  const lateIdentity = new Transcript();
  lateIdentity.interrupt({ provider_item_id: "old-item", content_index: 0 }, "t", true);
  lateIdentity.delta({ response_id: "old", provider_item_id: "old-item", content_index: 1, delta: "surviving old content" }, "spoken", "t");
  lateIdentity.delta({ response_id: "new", provider_item_id: "new-item", delta: "new" }, "spoken", "t");
  lateIdentity.delta({ response_id: "new", delta: " continued" }, "spoken", "t");
  lateIdentity.delta({ response_id: "old", content_index: 0, delta: "late truncated content" }, "spoken", "t");
  assert.deepEqual(lateIdentity.turns.map((row) => row.text),
    ["surviving old content", "new continued"]);
  const knownScope = new Transcript();
  knownScope.interrupt({ response_id: "known", provider_item_id: "shared-item", content_index: 0 }, "t", true);
  knownScope.delta({ provider_item_id: "shared-item", content_index: 1, delta: "identified sibling" }, "spoken", "t");
  assert.equal(knownScope.turns[0].responseId, "known");
  knownScope.interrupt({ response_id: "known" }, "t");
  knownScope.delta({ provider_item_id: "shared-item", content_index: 1, delta: "late" }, "spoken", "t");
  assert.equal(knownScope.turns[0].draft, false);
  assert.equal(knownScope.turns[0].text, "identified sibling");
});

function runtimeHarness(Client) {
  const source = ts.createSourceFile("server.ts",
    fs.readFileSync(path.join(__dirname, "../src/server.ts"), "utf8"), ts.ScriptTarget.Latest, true);
  const fn = source.statements.find((node) => ts.isFunctionDeclaration(node) && node.name.text === "ensureRuntime");
  assert.ok(fn, "test must execute the actual ensureRuntime function");
  const { outputText } = ts.transpileModule(fn.getText(source), { compilerOptions: { target: ts.ScriptTarget.ES2022 } });
  const warnings = [];
  const context = vm.createContext({
    MeerkatClient: Client, process, EXAMPLE_ROOT: path.join(__dirname, ".."),
    registerCallbackTools() {}, console: { warn: (...args) => warnings.push(args) },
  });
  vm.runInContext(`let runtimePromise;\n${outputText}`, context);
  return { run: () => vm.runInContext("ensureRuntime()", context), warnings };
}

test("failed initialization reaps actual SDK subprocesses before retry; success is shared", async () => {
  const { MeerkatClient } = await import("@rkat/sdk");
  const original = process.env.RKAT_RPC;
  const originalMode = process.env.EXAMPLE_TEST_RPC_MODE;
  process.env.RKAT_RPC = path.join(__dirname, "fake-rpc.cjs");
  const clients = [], pids = [];
  class TrackedClient extends MeerkatClient {
    constructor(...args) { super(...args); clients.push(this); }
    async close() {
      if (this.process?.pid) pids.push(this.process.pid);
      return super.close();
    }
  }
  const dead = (pid) => assert.throws(() => process.kill(pid, 0), { code: "ESRCH" });
  try {
    for (const mode of ["missing-mob", "version", "malformed"]) {
      process.env.EXAMPLE_TEST_RPC_MODE = mode;
      const harness = runtimeHarness(TrackedClient);
      for (let i = 0; i < 3; i++) {
        const count = clients.length;
        const results = await Promise.allSettled([harness.run(), harness.run()]);
        assert.ok(results.every((r) => r.status === "rejected"));
        assert.equal(clients.length, count + 1);
        assert.equal(clients.at(-1).process, null);
        for (const pid of pids) dead(pid);
      }
    }
    process.env.EXAMPLE_TEST_RPC_MODE = "success";
    const harness = runtimeHarness(TrackedClient);
    const count = clients.length;
    const results = await Promise.all([harness.run(), harness.run(), harness.run()]);
    assert.equal(clients.length, count + 1);
    assert.ok(results.every((r) => r.client === results[0].client));
    assert.ok(results[0].client.process.pid > 0);
    await results[0].client.close();
    for (const pid of pids) dead(pid);
  } finally {
    await Promise.all(clients.map((client) => client.close()));
    if (original === undefined) delete process.env.RKAT_RPC; else process.env.RKAT_RPC = original;
    if (originalMode === undefined) delete process.env.EXAMPLE_TEST_RPC_MODE; else process.env.EXAMPLE_TEST_RPC_MODE = originalMode;
  }
});

test("cleanup failure logs evidence without replacing the startup error", async () => {
  const original = new Error("initial diagnostic");
  class FailedClient {
    async connect() { throw original; }
    async close() { throw new Error("cleanup diagnostic"); }
  }
  const harness = runtimeHarness(FailedClient);
  await assert.rejects(harness.run(), (error) => error === original);
  assert.equal(harness.warnings.length, 1);
  assert.match(String(harness.warnings[0][1]), /cleanup diagnostic/);
});

test("HTTP caller disconnect closes a late successful live/open on the server", async () => {
  const source = ts.createSourceFile("server.ts",
    fs.readFileSync(path.join(__dirname, "../src/server.ts"), "utf8"), ts.ScriptTarget.Latest, true);
  const fn = source.statements.find((node) => ts.isFunctionDeclaration(node) && node.name.text === "route");
  const { outputText } = ts.transpileModule(fn.getText(source), { compilerOptions: { target: ts.ScriptTarget.ES2022 } });
  let resolveOpen;
  const closes = [];
  const context = vm.createContext({
    URL, readJson: async () => ({}),
    startLive: () => new Promise((resolve) => { resolveOpen = resolve; }),
    ensureRuntime: async () => ({ client: { liveClose: async (params) => closes.push(params.channel_id) } }),
    req: { method: "POST", url: "/api/start" }, res: { destroyed: true },
    sendJson: () => assert.fail("must not send a successful bootstrap to a gone caller"),
  });
  vm.runInContext(`let startQueue = Promise.resolve(); let activeLive = {channelId:"late"};\n${outputText}`, context);
  const pending = vm.runInContext("route(req, res)", context);
  await new Promise((resolve) => setImmediate(resolve));
  resolveOpen({ channel_id: "late" });
  await pending;
  assert.deepEqual(closes, ["late"]);
  assert.equal(vm.runInContext("activeLive", context), undefined);
});

test("smoke outer owner writes failure evidence and explicitly closes its browser", async () => {
  assert.ok(TEST_TIMEOUT_MS > OPERATION_TIMEOUT_MS + 5_000);
  const spec = fs.readFileSync(path.join(__dirname, "../.smoke/webrtc-smoke.spec.cjs"), "utf8");
  assert.match(spec, /test\.setTimeout\(TEST_TIMEOUT_MS\)/);
  const evidencePath = path.join(__dirname, "synthetic-smoke-evidence.json");
  try {
    for (const phase of ["context", "navigation", "evaluate", "remote", "success"]) {
      let closes = 0;
      const result = phase === "remote" ? { error: "signaling rejected" } : { final: { dc: "open", ice: "connected" } };
      const browser = {
        async newContext() {
          if (phase === "context") throw new Error("context failure");
          return { async newPage() { return {
            on() {},
            async goto() { if (phase === "navigation") throw new Error("navigation failure"); },
            async evaluate() { if (phase === "evaluate") throw new Error("evaluate failure"); return result; },
          }; } };
        },
        async close() { assert.ok(fs.existsSync(evidencePath)); closes++; },
      };
      const pending = runSmoke({ launch: async () => browser }, { evidencePath });
      if (phase === "success") await pending;
      else await assert.rejects(pending);
      assert.equal(closes, 1);
      const evidence = JSON.parse(fs.readFileSync(evidencePath, "utf8"));
      assert.equal(Boolean(evidence.error), phase !== "success");
      fs.unlinkSync(evidencePath);
    }
  } finally {
    if (fs.existsSync(evidencePath)) fs.unlinkSync(evidencePath);
  }
});
