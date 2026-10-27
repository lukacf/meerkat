"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { randomUUID } = require("node:crypto");
const { startFixture, eventually, rpc } = require("../acceptance-runtime.cjs");

const sender = "router:main";
const recipient = "domain:delivery";
const sourceKind = frame => frame.source?.kind === "console_event";
const cursor = frame => Number(frame.cursor.split(":")[1]);
const inputText = frame => frame.payload?.input?.content;

async function owner(fixture, method, params) {
  const response = await rpc(fixture.baseUrl, method, params);
  assert.equal(response.status, 200, JSON.stringify(response));
  assert(!response.body.error, JSON.stringify(response));
  return response.body.result;
}
async function frames(fixture, identity) {
  const response = await fetch(`${fixture.baseUrl}/console/timeline?identity=${encodeURIComponent(identity)}&mode=recent&limit=1000`);
  assert.equal(response.status, 200);
  const result = await response.json();
  assert(Array.isArray(result.frames));
  return result.frames;
}
async function send(fixture, identity, content, key) {
  const result = await owner(fixture, "meta/console/send", {
    identity, content, origin: "console:correlation-overlap", origin_kind: "operator",
    handling_mode: "queue", idempotency_key: key,
  });
  assert.equal(result.identity, identity);
  assert(result.interaction_id && result.input_frame_id, JSON.stringify(result));
  return result;
}
function terminal(frame, source) {
  return sourceKind(frame) && frame.kind === "interaction_complete"
    && frame.payload?.source_event_type === "run_completed" && frame.payload?.result === source;
}
function assertForeignCorrelation(foreignFrames, accepted, foreignStart) {
  assert(foreignFrames.length > 0);
  assert(foreignStart.interaction_id && foreignStart.run_id, "foreign runtime boundary carries canonical lineage");
  assert.notEqual(foreignStart.interaction_id, accepted.interaction_id);
  for (const frame of foreignFrames) {
    assert.notEqual(frame.interaction_id, accepted.interaction_id, `foreign ${frame.kind} ${frame.id} must not acquire operator ${accepted.interaction_id}`);
    if (["run_started", "text_delta", "text_complete", "tool_call_requested", "tool_result_received", "interaction_complete"].includes(frame.kind)) {
      assert.equal(frame.interaction_id, foreignStart.interaction_id, `foreign ${frame.kind} keeps its actual owner`);
      assert.equal(frame.run_id, foreignStart.run_id, `foreign ${frame.kind} keeps its actual run`);
    }
  }
}
function assertOperatorCorrelation(operatorFrames, accepted) {
  assert(operatorFrames.length > 0);
  for (const frame of operatorFrames) {
    if (["run_started", "text_delta", "text_complete", "tool_call_requested", "tool_result_received", "interaction_complete"].includes(frame.kind)) {
      assert.equal(frame.interaction_id, accepted.interaction_id, `actual operator ${frame.kind} ${frame.id} must preserve accepted interaction`);
    }
  }
}

async function overlap() {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Use coordinator prebuilt fixtures; this scenario must not trigger Cargo.");
  const fixture = await startFixture();
  const evidence = { ordering: [] };
  try {
    const id = `overlap-${randomUUID().slice(0, 8)}`;
    const peerBody = `Please inspect the release prerequisites. Exact foreign peer marker ${id}.`;
    const foreignSource = `Foreign peer ${id} checked the real ready set and completed independently.`;
    const operatorInput = `Operator ${id}: preserve this exact console interaction while the peer finishes.`;
    const operatorSource = `Operator ${id} completed its own accepted instruction. A\u030A and \ud83d\ude80 remain exact.`;
    const peerResponse = await fetch(`${fixture.backendUrl}/__fixture/peers`);
    assert.equal(peerResponse.status, 200);
    const peerId = (await peerResponse.json())[recipient]?.[0];
    assert.equal(typeof peerId, "string");
    evidence.plan = { id, peerBody, foreignSource, operatorInput, operatorSource, peerId };
    evidence.armed = await fixture.control("model-barrier", { action: "arm", plan: { id, match_text: peerBody, source: foreignSource } });
    await fixture.control("model", { source: "Unrelated fixture traffic acknowledged.", delay_ms: 0, chunk_chars: 11,
      scenario: { kind: "peer", run_id: id, peer_id: peerId, peer_body: peerBody },
    });
    evidence.senderAccepted = await send(fixture, sender, `[fixture:${id}] Ask the delivery peer to inspect the prerequisites.`, `sender-${id}`);
    evidence.entered = await eventually(async () => {
      const status = await fixture.control("model-barrier", { action: "status", id });
      return status.entered && !status.released && status;
    }, "incoming peer model request reached its explicit barrier");
    evidence.ordering.push("foreign model entered barrier");
    evidence.foreignStart = await eventually(async () => (await frames(fixture, recipient)).find(frame =>
      sourceKind(frame) && frame.kind === "run_started" && typeof inputText(frame) === "string" && inputText(frame).includes(peerBody)),
    "raw owner records the actual foreign peer run before operator acceptance");
    assert(evidence.foreignStart.interaction_id && evidence.foreignStart.run_id, "foreign start has typed runtime lineage");
    // The peer model has already captured its barrier turn and the sender
    // already dispatched its tool. Only now configure the later operator's
    // ordinary reply, avoiding bootstrap traffic with the same final source.
    await fixture.control("model", { source: operatorSource, delay_ms: 0, chunk_chars: 11 });
    evidence.operatorAccepted = await send(fixture, recipient, operatorInput, `operator-${id}`);
    const accepted = evidence.operatorAccepted;
    evidence.ordering.push("operator accepted while foreign model blocked");
    const blockedFrames = await frames(fixture, recipient);
    const acceptedInput = blockedFrames.find(frame => frame.id === accepted.input_frame_id);
    assert.equal(acceptedInput?.kind, "user_input");
    assert.equal(acceptedInput.payload.content, operatorInput);
    assert.equal(acceptedInput.interaction_id, accepted.interaction_id);
    assert(!blockedFrames.some(frame => sourceKind(frame) && frame.kind === "run_started" && inputText(frame) === operatorInput), "operator cannot start while peer model is held");
    assert(!blockedFrames.some(frame => terminal(frame, foreignSource)), "foreign terminal cannot precede explicit release");
    evidence.stillBlocked = await fixture.control("model-barrier", { action: "status", id });
    assert.equal(evidence.stillBlocked.phase, "entered");
    evidence.released = await fixture.control("model-barrier", { action: "release", id });
    evidence.ordering.push("explicitly released foreign model after operator receipt");
    evidence.frames = await eventually(async () => {
      const current = await frames(fixture, recipient);
      return current.some(frame => terminal(frame, foreignSource)) && current.some(frame => terminal(frame, operatorSource)) && current;
    }, "foreign and actual operator runs both finish through real runtime", 45_000);
    const current = evidence.frames;
    const foreignEnd = current.find(frame => terminal(frame, foreignSource));
    const operatorStart = current.find(frame => sourceKind(frame) && frame.kind === "run_started" && inputText(frame) === operatorInput);
    const operatorEnd = current.find(frame => terminal(frame, operatorSource));
    assert(operatorStart && operatorEnd);
    assert(cursor(evidence.foreignStart) < cursor(acceptedInput));
    assert(cursor(acceptedInput) < cursor(foreignEnd));
    assert(cursor(foreignEnd) < cursor(operatorStart));
    assert(cursor(operatorStart) < cursor(operatorEnd));
    const foreignFrames = current.filter(frame => sourceKind(frame) && cursor(frame) >= cursor(evidence.foreignStart) && cursor(frame) <= cursor(foreignEnd));
    const operatorFrames = current.filter(frame => sourceKind(frame) && cursor(frame) >= cursor(operatorStart) && cursor(frame) <= cursor(operatorEnd));
    assertForeignCorrelation(foreignFrames, accepted, evidence.foreignStart);
    assertOperatorCorrelation(operatorFrames, accepted);
    const toolId = `fixture-${id}-peer-ready`;
    const calls = foreignFrames.filter(frame => frame.kind === "tool_call_requested" && frame.payload?.id === toolId);
    const results = foreignFrames.filter(frame => frame.kind === "tool_result_received" && frame.payload?.id === toolId);
    assert.equal(calls.length, 1, "one real foreign ready-set tool call");
    assert.equal(calls[0].payload.name, "workgraph_ready");
    assert.equal(results.length, 1, "foreign tool has its real runtime result");
    assert.equal(results[0].payload.is_error, false);
    const resultText = results[0].payload.content.filter(block => block.type === "text").map(block => block.text).join("");
    assert(Array.isArray(JSON.parse(resultText).items));
    assert.equal(foreignFrames.filter(frame => frame.kind === "text_delta").map(frame => frame.payload.delta).join(""), foreignSource);
    assert.equal(operatorFrames.filter(frame => frame.kind === "text_delta").map(frame => frame.payload.delta).join(""), operatorSource);
    const retained = current.filter(frame => frame.id === accepted.input_frame_id);
    assert.equal(retained.length, 1, "the accepted operator input remains unique");
    assert.equal(retained[0].payload.content, operatorInput);
    assert.equal(retained[0].interaction_id, accepted.interaction_id);
    assert(!current.some(frame => frame.interaction_id === accepted.interaction_id && ["interaction_failed", "message_delivery_failed"].includes(frame.kind)));
    evidence.finalBarrier = await fixture.control("model-barrier", { action: "status", id });
    assert.equal(evidence.finalBarrier.requests, 2, "foreign tool call and result continuation both used the barrier plan; operator did not");
    evidence.ordering.push("foreign canonical completion followed by exactly correlated operator completion");
    evidence.foreignFrameIds = foreignFrames.map(frame => frame.id);
    evidence.operatorFrameIds = operatorFrames.map(frame => frame.id);
  } catch (error) {
    evidence.failure = error.stack || String(error);
    evidence.frames = await frames(fixture, recipient).catch(() => []);
    evidence.senderFrames = await frames(fixture, sender).catch(() => []);
    evidence.recordedRequests = await fetch(`${fixture.backendUrl}/__fixture/requests`).then(response => response.json()).catch(() => []);
    evidence.barrierAtFailure = evidence.plan
      ? await fixture.control("model-barrier", { action: "status", id: evidence.plan.id }).catch(() => null)
      : null;
    throw error;
  } finally {
    const folder = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
    await fs.mkdir(folder, { recursive: true });
    await fs.writeFile(path.join(folder, "real-correlation-overlap.json"), JSON.stringify({ ...evidence, logs: fixture.logs() }, null, 2));
    await fixture.close();
  }
}
const apiScenarios = [{ id: "api-peer-operator-correlation-overlap", family: "correlation", backend: "real", run: overlap }];
module.exports = { apiScenarios, assertForeignCorrelation, assertOperatorCorrelation };
if (require.main === module) require("../scenario-registry.cjs").runScenarios(apiScenarios).catch(error => { console.error(error); process.exitCode = 1; });
