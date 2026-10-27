"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { randomUUID } = require("node:crypto");
const { chromium } = require("playwright");
const { startFixture, eventually, rpc } = require("../acceptance-runtime.cjs");

const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
const source = "Check again again and check again.";
const answer = "Review complete. Both independent checks support the release candidate.";
const chunkChars = 6;

function assertReasoningSource(frames, interactionId, label) {
  const reasoning = frames.filter(frame => frame.interaction_id === interactionId && ["reasoning_delta", "reasoning_complete"].includes(frame.kind));
  assert.equal(new Set(reasoning.map(frame => frame.id)).size, reasoning.length, `${label}: source event IDs are distinct`);
  const blocks = [];
  let current = "";
  let deltaIds = [];
  for (const frame of reasoning) {
    assert.equal(frame.source.kind, "console_event", `${label}: actual runtime events`);
    assert.equal(frame.source_event_id, frame.id, `${label}: source identity survives projection`);
    if (frame.kind === "reasoning_delta") {
      current += frame.payload.delta;
      deltaIds.push(frame.id);
    } else {
      assert.equal(current, source, `${label}: exact delta join per completed block`);
      assert.equal(frame.payload.content, source, `${label}: exact owner completion`);
      assert.equal(deltaIds.length, Math.ceil(Array.from(source).length / chunkChars), `${label}: every actual delta retained`);
      blocks.push({ source: current, deltaIds, completeId: frame.id });
      current = ""; deltaIds = [];
    }
  }
  assert.equal(current, "", `${label}: no unterminated reasoning fragment`);
  assert.equal(blocks.length, 2, `${label}: identical sequential blocks remain separate`);
  assert.equal(reasoning.filter(frame => frame.kind === "reasoning_delta" && frame.payload.delta === "again ").length, 4,
    `${label}: repeated equal fragments all survive`);
  return { blocks, eventIds: reasoning.map(frame => frame.id) };
}

async function captureStream(url) {
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
      if (done) throw new Error("reasoning capture stream ended before inspection finished");
    }
  })().catch(error => { if (!abort.signal.aborted) fault = error; });
  return { events, check() { if (fault) throw fault; }, async close() { abort.abort(); await task; } };
}

function initializationCancellation(request) {
  if (!request.failure()?.errorText?.includes("ERR_ABORTED")) return false;
  if (new URL(request.url()).pathname.endsWith("/timeline/stream")) return true;
  try {
    const body = JSON.parse(request.postData() || "{}");
    return body.method === "meta/console/query_timeline" && body.params?.mode === "recent" && body.params?.limit === 200 && !body.params?.identity;
  } catch { return false; }
}

async function repeatedReasoning(host) {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Use the coordinator's prebuilt fixture; this scenario must not compile Rust.");
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  const result = { host, source, answer, chunkChars, errors: [], expectedCancellations: [] };
  const captures = [];
  let allowance = "initialization";
  page.on("pageerror", error => result.errors.push(error.message));
  page.on("requestfailed", request => {
    const detail = { url: request.url(), error: request.failure()?.errorText };
    if ((allowance === "initialization" && initializationCancellation(request))
      || (allowance === "disconnect" && new URL(request.url()).pathname.endsWith("/timeline/stream"))) {
      result.expectedCancellations.push({ ...detail, reason: allowance });
    } else result.errors.push(detail);
  });
  const viewport = () => page.locator(host === "stock" ? ".conv__body" : '[data-testid="shared-pane-0"] .cc-conversation-pane__scroll').first();
  const status = () => page.locator('[data-testid="console-transport-status"][data-phase="live"]');
  async function open(reload = false) {
    allowance = "initialization";
    if (reload) await page.reload();
    else await page.goto(fixture.baseUrl + (host === "stock" ? "/console" : "/shared"));
    if (host === "stock" && !await page.locator(".conv__body").count()) {
      await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: /router/i }).first().click();
    }
    await status().waitFor();
    await viewport().waitFor();
    allowance = null;
  }
  async function inspectRendered(label) {
    await eventually(async () => await viewport().locator(".cc-rich-thinking__body").count() === 2, `${host} ${label}: two rendered thinking blocks`);
    const actual = await viewport().locator(".cc-rich-thinking__body").allTextContents();
    assert.deepEqual(actual, [source, source], `${host} ${label}: exact raw reasoning source in both distinct blocks`);
    assert.equal(await viewport().getByText(answer, { exact: true }).count(), 1, `${host} ${label}: one final answer`);
    const rowIds = await viewport().locator("[data-conversation-row-id]").evaluateAll(nodes => nodes.map(node => node.dataset.conversationRowId));
    assert.equal(new Set(rowIds).size, rowIds.length, `${host} ${label}: replay cannot duplicate rendered rows`);
    const summaries = await viewport().locator("details.cc-rich-thinking > summary").allTextContents();
    assert.equal(summaries.length, 2, `${host} ${label}: both thinking disclosures are labeled`);
    assert(summaries.every(summary => /thinking/i.test(summary)), `${host} ${label}: accessible thinking labels: ${JSON.stringify(summaries)}`);
    // Open retained blocks for a useful actual screenshot instead of judging closed disclosures.
    for (const block of await viewport().locator("details.cc-rich-thinking").all()) {
      if (!await block.evaluate(node => node.open)) await block.locator("summary").click();
    }
    await viewport().getByText(answer, { exact: true }).scrollIntoViewIfNeeded();
    await fs.mkdir(evidence, { recursive: true });
    await page.screenshot({ path: path.join(evidence, `${host}-real-reasoning-${label}.png`), fullPage: true });
    return { sources: actual, rowIds };
  }
  try {
    await open();
    const baselineResponse = await fetch(`${fixture.baseUrl}/console/timeline?identity=router%3Amain&mode=recent&limit=1000`);
    assert.equal(baselineResponse.status, 200);
    const baseline = await baselineResponse.json();
    assert(baseline.latest_cursor);
    const streamUrl = `${fixture.backendUrl}/console/timeline/stream?identity=router%3Amain&after=${encodeURIComponent(baseline.latest_cursor)}`;
    const live = await captureStream(streamUrl); captures.push(live);
    await eventually(() => { live.check(); return live.events.some(event => event.type === "snapshot_complete"); }, "direct live capture ready before send");
    await fixture.control("model", { source: answer, reasoning_blocks: [source, source], delay_ms: 20, chunk_chars: chunkChars });
    const sent = await rpc(fixture.baseUrl, "meta/console/send", {
      identity: "router:main", origin: "console:reasoning-acceptance", origin_kind: "operator",
      idempotency_key: randomUUID(), content: "Check the release candidate twice and report the result.",
    });
    assert(sent.body.result?.input_frame_id && sent.body.result?.interaction_id, JSON.stringify(sent.body));
    const interactionId = sent.body.result.interaction_id;
    result.accepted = sent.body.result;
    await eventually(() => {
      live.check();
      return live.events.some(event => event.frame?.interaction_id === interactionId && event.frame.kind === "interaction_complete");
    }, "real reasoning stream completes without a healing query");
    result.live = assertReasoningSource(live.events.flatMap(event => event.frame ? [event.frame] : []), interactionId, "live");
    result.rendered = { live: await inspectRendered("live") };
    await live.close();
    const ownerResponse = await fetch(`${fixture.baseUrl}/console/timeline?identity=router%3Amain&mode=recent&limit=1000`);
    assert.equal(ownerResponse.status, 200);
    result.ownerFrames = (await ownerResponse.json()).frames;
    result.owner = assertReasoningSource(result.ownerFrames, interactionId, "owner");
    assert.deepEqual(result.live.eventIds, result.owner.eventIds, "live and owner retain the same exact reasoning event identities");
    const replay = await captureStream(streamUrl); captures.push(replay);
    await eventually(() => { replay.check(); return replay.events.some(event => event.type === "snapshot_complete"); }, "actual replay snapshot completes");
    result.replay = assertReasoningSource(replay.events.flatMap(event => event.frame ? [event.frame] : []), interactionId, "replay");
    assert.deepEqual(result.replay.eventIds, result.owner.eventIds, "replay preserves every event exactly once");
    await replay.close();
    allowance = "disconnect";
    const streamsBefore = fixture.observations.filter(item => item.path.includes("/timeline/stream") && item.status === 200).length;
    fixture.disconnectStreams();
    await eventually(() => fixture.observations.filter(item => item.path.includes("/timeline/stream") && item.status === 200).length > streamsBefore, "browser reconnect reaches a new successful stream");
    await status().waitFor(); allowance = null;
    result.rendered.reconnected = await inspectRendered("reconnected");
    assert.deepEqual(result.rendered.reconnected.rowIds, result.rendered.live.rowIds, "reconnect retains stable row IDs");
    await open(true);
    result.rendered.reloaded = await inspectRendered("reloaded");
    assert.deepEqual(result.errors, [], "no unexpected browser or network errors");
  } catch (error) {
    result.failure = error.stack || String(error);
    result.html = await page.content();
    await fs.mkdir(evidence, { recursive: true });
    await page.screenshot({ path: path.join(evidence, `${host}-real-reasoning-failure.png`), fullPage: true }).catch(() => {});
    throw error;
  } finally {
    result.streams = captures.map(capture => capture.events);
    result.observations = fixture.observations;
    result.logs = fixture.logs();
    await fs.mkdir(evidence, { recursive: true });
    await fs.writeFile(path.join(evidence, `${host}-real-reasoning.json`), JSON.stringify(result, null, 2));
    await Promise.all(captures.map(capture => capture.close()));
    await browser.close(); await fixture.close();
  }
}

const scenarios = ["stock", "shared"].map(host => ({ id: `real-${host}-reasoning`, family: "real-presentation", backend: "real", run: () => repeatedReasoning(host) }));
module.exports = { scenarios, assertReasoningSource };
if (require.main === module) require("../scenario-registry.cjs").runScenarios(scenarios).catch(error => { process.stderr.write(`${error.stack || error}\n`); process.exitCode = 1; });
