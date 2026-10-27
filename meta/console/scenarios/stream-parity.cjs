"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { randomUUID } = require("node:crypto");
const { startFixture, eventually, rpc } = require("../acceptance-runtime.cjs");

async function liveCapture(url) {
  const abort = new AbortController();
  const response = await fetch(url, { signal: abort.signal });
  assert.equal(response.status, 200);
  const events = [];
  let fault;
  const task = (async () => {
    const reader = response.body.getReader();
    const decoder = new TextDecoder();
    let buffer = "";
    while (true) {
      const { value, done } = await reader.read();
      buffer += decoder.decode(value, { stream: !done });
      let end;
      while ((end = buffer.indexOf("\n\n")) >= 0) {
        const block = buffer.slice(0, end); buffer = buffer.slice(end + 2);
        const data = block.split("\n").filter(line => line.startsWith("data:")).map(line => line.slice(5).trimStart()).join("\n");
        if (data) events.push(JSON.parse(data));
      }
      if (done) throw new Error("live stream closed before capture completed");
    }
  })().catch(error => { if (!abort.signal.aborted) fault = error; });
  return {
    events,
    check() { if (fault) throw fault; },
    async close() { abort.abort(); await task; },
  };
}

function assertCapturedSource(frames, source, label) {
  assert.equal(new Set(frames.map(frame => frame.id)).size, frames.length, `${label}: no duplicate canonical frames`);
  const live = frames.filter(frame => frame.source?.kind === "console_event");
  assert.equal(live.filter(frame => frame.kind === "text_delta").map(frame => frame.payload.delta).join(""), source, `${label}: exact delta source`);
  const text = live.filter(frame => frame.kind === "text_complete"), terminals = live.filter(frame => frame.kind === "interaction_complete");
  assert.equal(text.length, 1, `${label}: one live text completion`);
  assert.equal(terminals.length, 1, `${label}: one live interaction completion`);
  assert.equal(text[0].payload.content, source, `${label}: exact live text completion`);
  assert.equal(terminals[0].payload.result, source, `${label}: exact live interaction completion`);
  assert.equal(text[0].run_id, terminals[0].run_id, `${label}: live completion run owner`);
  for (const history of frames.filter(frame => frame.source?.kind === "session_history" && frame.kind === "text_complete")) {
    assert.equal(history.payload.text, source, `${label}: exact saved text`);
    assert.equal(history.payload.result, source, `${label}: exact saved result`);
    assert.equal(history.payload.message.blocks.filter(block => block.block_type === "text").map(block => block.data.text).join(""), source, `${label}: exact authored message text`);
    assert.equal(history.payload.message.identity.run_id, history.run_id, `${label}: saved message run owner`);
    assert.equal(history.payload.message.identity.interaction_id, history.interaction_id, `${label}: saved message interaction owner`);
    for (const key of ["runtime_key", "identity", "run_id", "interaction_id", "session_id"]) {
      assert.equal(history[key], terminals[0][key], `${label}: saved/live ${key}`);
    }
  }
}

async function sharedOwnerStreamParity() {
  assert(process.env.META_EXAMPLE_BIN_DIR, "This acceptance must use the coordinator's prebuilt runtime.");
  const fixture = await startFixture();
  const captures = [];
  const evidence = {};
  try {
    const source = Array.from({ length: 40 }, (_, i) => `Row ${String(i).padStart(3, "0")} preserves exact  whitespace and peer-${i} identifiers.\n`).join("");
    await fixture.control("model", { source, chunk_chars: 8, delay_ms: 3 });
    for (const base of [fixture.backendUrl, `${fixture.backendUrl}/mirror`, fixture.baseUrl]) {
      captures.push(await liveCapture(`${base}/console/timeline/stream?identity=router%3Amain`));
    }
    await eventually(() => captures.every(capture => {
      capture.check();
      return capture.events.some(event => event.type === "snapshot_complete");
    }), "all actual router snapshots complete before send");
    const sent = await rpc(fixture.baseUrl, "meta/console/send", {
      identity: "router:main", origin: "fixture:stream-parity", origin_kind: "operator",
      idempotency_key: randomUUID(), content: "Prove each actual router emits the complete source.",
    });
    assert(!sent.body.error, JSON.stringify(sent.body));
    const interactionId = sent.body.result.interaction_id;
    const relevant = capture => capture.events.flatMap(event => event.frame?.interaction_id === interactionId ? [event.frame] : []);
    // Observe live streams directly. No timeline query or model-request poll is
    // allowed to heal missing frames before all three live terminals arrive.
    await eventually(() => captures.every(capture => {
      capture.check();
      return relevant(capture).some(frame => frame.kind === "interaction_complete" && frame.source.kind === "console_event");
    }), "every router receives live terminal completion", 12_000);
    // Freeze the proof before querying canonical history. The later query may
    // backfill history but can never repair these captured raw stream arrays.
    evidence.streams = captures.map(capture => structuredClone(relevant(capture)));
    await Promise.all(captures.map(capture => capture.close()));
    const ownerResponse = await fetch(`${fixture.backendUrl}/console/timeline?identity=router%3Amain&mode=recent&limit=1000`);
    assert.equal(ownerResponse.status, 200);
    const owner = (await ownerResponse.json()).frames.filter(frame => frame.interaction_id === interactionId);
    evidence.owner = owner;
    assertCapturedSource(owner, source, "canonical owner");
    const ownerIds = owner.filter(frame => frame.source.kind !== "session_history").map(frame => frame.id).sort();
    const ownerLive = owner.filter(frame => frame.source.kind === "console_event");
    for (const [index, frames] of evidence.streams.entries()) {
      assertCapturedSource(frames, source, `router ${index}`);
      assert.deepEqual(frames.filter(frame => frame.source.kind !== "session_history").map(frame => frame.id).sort(), ownerIds, `router ${index}: complete owner frame set`);
      assert.deepEqual(frames.filter(frame => frame.source.kind === "console_event"), ownerLive, `router ${index}: exact canonical live frame payloads and order`);
      assert.deepEqual(frames, evidence.streams[0], `router ${index}: exact raw frame payloads and order across routers`);
    }
    evidence.sourceLength = source.length;
  } catch (error) {
    evidence.failure = error.stack || String(error); throw error;
  } finally {
    evidence.captures = captures.map(capture => capture.events);
    evidence.logs = fixture.logs();
    const dir = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
    await fs.mkdir(dir, { recursive: true });
    await fs.writeFile(path.join(dir, "dual-router-stream-parity.json"), JSON.stringify(evidence, null, 2));
    await Promise.all(captures.map(capture => capture.close()));
    await fixture.close();
  }
}

module.exports = { assertCapturedSource, apiScenarios: [
  { id: "api-dual-router-stream-parity", family: "transport", backend: "real", run: sharedOwnerStreamParity },
] };
