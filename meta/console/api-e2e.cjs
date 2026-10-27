#!/usr/bin/env node
"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { startFixture, eventually, rpc, snapshot } = require("./acceptance-runtime.cjs");
const { runScenarios } = require("./scenario-registry.cjs");
const { boundaryRecords, expectedContextContent, assertExactContextMessage, serializeConsoleContextMessage } = require("./scenarios/quote-ingress-proof.cjs");

async function faultClassification() {
  const fixture = await startFixture();
  try {
    for (const fault of ["read", "latest", "progress", "expired"]) {
      await fixture.control("fault", { fault });
      const replay = fault === "expired";
      for (const path of ["/console/timeline?after=console:0", "/console/timeline/stream?after=console:0"]) {
        const response = await fetch(fixture.baseUrl + path);
        assert.equal(response.status, replay ? 409 : 500, `${fault} ${path}`);
        const body = await response.json();
        assert.equal(body.error, replay ? "replay_unavailable" : "timeline_unavailable");
        assert(!JSON.stringify(body).includes("private fixture"));
      }
      for (const prefix of ["", "/aggregate"]) {
        const { body } = await rpc(fixture.baseUrl, "meta/console/query_timeline", { after: "console:0" }, prefix);
        assert.equal(body.error.code, replay ? -32013 : -32000, `${fault} RPC ${prefix}`);
        assert(!JSON.stringify(body).includes("private fixture"));
      }
    }
    for (const path of ["/console/timeline", "/console/timeline/stream", "/console/rpc"]) {
      const response = await fetch(`${fixture.baseUrl}/authenticated${path}`, path.endsWith("rpc") ? {
        method: "POST", headers: { "content-type": "application/json" },
        body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "meta/console/query_timeline", params: {} }),
      } : {});
      assert.equal(response.status, 401, path);
    }
    await fixture.control("fault", { fault: "none" });
    const future = await fetch(`${fixture.baseUrl}/console/timeline?after=console:999999999`);
    assert.equal(future.status, 409);
  } finally { await fixture.close(); }
}

async function ingressAndResume(mode) {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Use the coordinator's prebuilt fixture for both final ingress paths.");
  const fixture = await startFixture({ mode });
  const evidence = { mode, errors: [] };
  try {
    const source = "# Exact reply\n\nRepeated phrase, repeated phrase. A\u030A and \ud83d\ude80.\n\n```ts\nconst ready = true;\n```\n\n- [x] done\n";
    await fixture.control("model", { source, delay_ms: 1, chunk_chars: 4 });
    const instruction = `  Acceptance ${mode}: preserve exact context\nCompare every unverified snapshot.  `;
    const sourceScope = `${fixture.baseUrl}/__context-source-must-not-fetch`;
    const records = boundaryRecords(sourceScope);
    const content = serializeConsoleContextMessage(instruction, records);
    const expected = expectedContextContent(instruction, records);
    assert.deepEqual(content, expected, "the actual serializer matches the independent v1 wire contract");
    const beforeLimits = fixture.observations.length;
    const serializeAndDispatch = values => rpc(fixture.baseUrl, "meta/console/send", {
      identity: "router:main", content: serializeConsoleContextMessage(instruction, values),
    });
    assert.throws(() => serializeAndDispatch([...records, { ...records[7], id: "quote-8" }]), /at most 8 quotes/);
    assert.throws(() => serializeAndDispatch(records.map((record, index) => index === 0 ? { ...record, quote: record.quote + "x" } : record)), /64 KiB/);
    assert.equal(fixture.observations.length, beforeLimits, "out-of-limit drafts stop before HTTP dispatch");
    evidence.limits = { records: records.length, recordUtf8Bytes: Buffer.byteLength(JSON.stringify(records)), blockedCount: 9, blockedUtf8Bytes: 65_537 };
    const envelope = {
      identity: "router:main", origin: "console:api-acceptance", origin_kind: "operator",
      idempotency_key: `acceptance-${mode}`, handling_mode: "queue",
      content,
    };
    evidence.envelope = envelope;
    const before = await snapshot(fixture.baseUrl);
    assert.equal(before[0].event, "snapshot_started");
    fixture.dropNextSendResponse();
    await assert.rejects(() => rpc(fixture.baseUrl, "meta/console/send", envelope));
    const dropped = fixture.observations.find(item => item.dropped);
    assert(dropped, "the proxy must drop an actual completed server response");
    const first = JSON.parse(dropped.response);
    assert(first.result?.input_frame_id, JSON.stringify(first));
    const replay = await rpc(fixture.baseUrl, "meta/console/send", envelope);
    assert.equal(replay.body.result?.interaction_id, first.result.interaction_id, JSON.stringify(replay.body));
    assert.equal(replay.body.result?.input_frame_id, first.result.input_frame_id);
    const conflict = await rpc(fixture.baseUrl, "meta/console/send", { ...envelope, content: "changed" });
    assert(conflict.body.error, "same key with a changed envelope must be rejected");
    const messages = await eventually(async () => {
      const requests = await (await fetch(`${fixture.backendUrl}/__fixture/requests`)).json();
      const matching = requests.flatMap(request => request.messages).filter(message =>
        message.role === "user" && Array.isArray(message.content) && message.content[0]?.text === instruction);
      return matching.length ? matching : null;
    }, `${mode} final model ingress`);
    for (const message of messages) assertExactContextMessage(message, expected);
    evidence.finalModelMessages = messages;
    evidence.first = first;
    evidence.replay = replay;
    evidence.conflict = conflict;
    const page = await eventually(async () => {
      const value = await (await fetch(`${fixture.baseUrl}/console/timeline?identity=router%3Amain&mode=recent&limit=500`)).json();
      return value.frames?.some(frame => JSON.stringify(frame.payload).includes(JSON.stringify(source).slice(1, -1))) ? value : null;
    }, `${mode} completed source query`);
    assert.equal(page.frames.filter(frame => frame.id === first.result.input_frame_id).length, 1);
    const inputFrame = page.frames.find(frame => frame.id === first.result.input_frame_id);
    assert.equal(inputFrame.kind, "user_input");
    assert.deepEqual(inputFrame.payload.content, expected, "durable timeline receipt retains the exact serialized context");
    evidence.durableInputFrame = inputFrame;
    const full = await snapshot(fixture.baseUrl, "?identity=router%3Amain");
    const cursor = full.at(-1).data.cursor;
    assert(cursor, "completed snapshot supplies a resume frontier");
    const resumed = await snapshot(fixture.baseUrl, "?identity=router%3Amain", { "Last-Event-ID": cursor });
    const seen = new Set(full.filter(item => item.event === "console_frame").map(item => item.data.frame.id));
    const frontier = Number(cursor.split(":")[1]);
    for (const item of resumed.filter(item => item.event === "console_frame")) {
      assert(!seen.has(item.data.frame.id), "accepted resume does not redeliver old frames");
      assert(Number(item.data.frame.cursor.split(":")[1]) > frontier, "late live events follow the accepted frontier");
    }
    assert(Number(resumed.at(-1).data.cursor.split(":")[1]) >= frontier);
    assert(!fixture.observations.some(item => item.path.startsWith("/__context-source-must-not-fetch")),
      "unverified source metadata never causes an implicit source fetch");
    evidence.resumeCursor = cursor;
  } catch (error) {
    evidence.errors.push(error.stack || String(error));
    throw error;
  } finally {
    try {
      const dir = process.env.META_API_EVIDENCE || path.join(__dirname, "../output/playwright/console-acceptance");
      await fs.mkdir(dir, { recursive: true });
      await fs.writeFile(path.join(dir, `api-${mode}-ingress.json`), JSON.stringify({ ...evidence, observations: fixture.observations, logs: fixture.logs() }, null, 2));
    } finally { await fixture.close(); }
  }
}

async function restartIngress(mode) {
  const stateDir = await fs.mkdtemp(path.join(os.tmpdir(), `meta-send-${mode}-`));
  let fixture = await startFixture({ mode, stateDir });
  const envelope = { identity: "router:main", origin: "console:restart-proof", origin_kind: "operator",
    idempotency_key: `restart-${mode}`, handling_mode: "queue", content: "Retain this exact accepted intent across server restart." };
  try {
    await fixture.control("model", { source: "Restart proof accepted once.", delay_ms: 0, chunk_chars: 32 });
    const first = await rpc(fixture.baseUrl, "meta/console/send", envelope);
    assert(first.body.result?.input_frame_id, JSON.stringify(first));
    await eventually(async () => {
      const requests = await (await fetch(fixture.backendUrl + "/__fixture/requests")).json();
      return requests.some(request => JSON.stringify(request.messages).includes(envelope.content));
    }, "original send reaches real model");
    const original = await eventually(async () => {
      const page = await (await fetch(fixture.baseUrl + "/console/timeline?identity=router%3Amain&mode=recent&limit=500")).json();
      const receipt = page.frames.find(frame => frame.id === first.body.result.input_frame_id);
      return receipt?.status === "delivered" && page.frames.some(frame => frame.kind === "interaction_complete" && JSON.stringify(frame.payload).includes("Restart proof accepted once.")) && receipt;
    }, "original durable receipt and completion");
    await fixture.close();
    fixture = await startFixture({ mode, stateDir });
    const replay = await rpc(fixture.baseUrl, "meta/console/send", envelope);
    assert.equal(replay.body.result?.interaction_id, first.body.result.interaction_id, JSON.stringify(replay));
    assert.equal(replay.body.result?.input_frame_id, original.id);
    const changed = await rpc(fixture.baseUrl, "meta/console/send", { ...envelope, content: "Different intent" });
    assert(changed.body.error, "restart does not discard envelope conflict detection");
    const frames = (await (await fetch(fixture.baseUrl + "/console/timeline?identity=router%3Amain&mode=recent&limit=500")).json()).frames;
    assert.equal(frames.filter(frame => frame.id === original.id).length, 1);
    const requests = await (await fetch(fixture.backendUrl + "/__fixture/requests")).json();
    const replayedTurns = requests.filter(request => {
      const users = request.messages.filter(message => message.role === "user");
      return users.length && JSON.stringify(users.at(-1)).includes(envelope.content);
    });
    assert.equal(replayedTurns.length, 0, "receipt replay never resubmits the operator turn to the model");
    const evidence = path.join(__dirname, "../output/playwright/console-acceptance");
    await fs.mkdir(evidence, { recursive: true });
    await fs.writeFile(path.join(evidence, `send-${mode}-server-restart.json`), JSON.stringify({ mode, envelope, first, original, replay, changed, newOperatorRequests: replayedTurns.length }, null, 2));
  } finally { await fixture.close(); await fs.rm(stateDir, { recursive: true, force: true }); }
}

const scenarios = [
  ...require("./scenarios/stream-parity.cjs").apiScenarios,
  ...require("./scenarios/approval-lifecycle.cjs").apiScenarios,
  ...require("./scenarios/real-workgraph.cjs").apiScenarios,
  ...require("./scenarios/real-routine-tools.cjs").apiScenarios,
  ...require("./scenarios/real-correlation-overlap.cjs").apiScenarios,
  { id: "api-query-faults", family: "transport", backend: "real", run: faultClassification },
  { id: "api-member-ingress", family: "send", backend: "real", run: () => ingressAndResume("member") },
  { id: "api-identity-ingress", family: "send", backend: "real", run: () => ingressAndResume("identity") },
  { id: "api-member-send-restart", family: "send", backend: "real", run: () => restartIngress("member") },
  { id: "api-identity-send-restart", family: "send", backend: "real", run: () => restartIngress("identity") },
];

runScenarios(scenarios).catch(error => { process.stderr.write(`${error.stack || error}\n`); process.exitCode = 1; });
