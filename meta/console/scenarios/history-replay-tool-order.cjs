"use strict";
// A stopped run's saved rounds replay into an open stock console. Each saved
// assistant step without text arrives as an `assistant_message` row, the
// canonical row its tool calls render in. The fixture is a captured
// cancel-after-boundary run (console/test-assets/routine-history-replay.json):
// the page the console read at the run's terminal, then the replay the stream
// delivered after it. Its saved and live tool frames share a millisecond.
// Rendered from loose call frames instead of the canonical rows, the saved
// list_files call sorted ahead of the live read_file call, and that first
// read_file joined the later read_file group.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const http = require("node:http");
const path = require("node:path");
const { chromium } = require("playwright");

const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
const assetDirectory = path.join(__dirname, "../../crates/meerkat-meta/console-dist");
const capture = require("../test-assets/routine-history-replay.json");
const { identity, session_id: session } = capture;
const INTERRUPTED_RUN_NOTICE_PREFIX = "A previous run was interrupted";

async function startServer(pageFrames) {
  const assets = new Map();
  for (const [url, name, type] of [["/console", "index.html", "text/html"],
    ["/console/assets/console-app.js", "console-app.js", "application/javascript"],
    ["/console/assets/console-app.css", "console-app.css", "text/css"]]) {
    assets.set(url, { name, type, bytes: await fs.readFile(path.join(assetDirectory, name)) });
  }
  const requests = [], streams = new Set();
  const agent = { identity, member_id: identity, agent_id: identity, session_id: session,
    label: "Router", kind: "identity", state: "active", addressable: true,
    response_phase: null, affordances: { addressable: true, can_send_message: true } };
  // Only the identity page holds what the server had at the run's terminal,
  // all of it, as in the capture. Its rows include the run's turn starts,
  // which the live stream does not route, so the rows the live check waits
  // for prove the page landed. The replay reaches the console through the
  // stream alone.
  function timeline(selected) {
    const visible = selected === identity ? pageFrames : [];
    return { frames: visible, available: true, exhausted: true,
      next_cursor: visible.at(-1)?.cursor ?? null, latest_cursor: visible.at(-1)?.cursor ?? null };
  }
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://localhost");
    const json = value => { response.writeHead(200, { "content-type": "application/json" }); response.end(JSON.stringify(value)); };
    const asset = assets.get(url.pathname);
    if (asset) { response.writeHead(200, { "content-type": asset.type }); response.end(asset.bytes); return; }
    if (url.pathname === "/console/timeline/stream") {
      response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
      response.write(`: connected\n\nevent: snapshot_complete\ndata: ${JSON.stringify({ type: "snapshot_complete", cursor: pageFrames.at(-1)?.cursor })}\n\n`);
      streams.add(response); response.on("close", () => streams.delete(response)); return;
    }
    if (url.pathname === "/console/experience") return json({ contract_version: "0.5.0", runtime_id: "history-replay-proof",
      storage_scope: "history-replay-proof", runtime_capabilities: { can_send_messages: true }, voice: { available: false },
      console_config: { title: "History replay", appearance: { default_theme: "light" },
        layout: { initial_agent: identity, initial_preset: "single" }, rail: { visible: false } },
      agent_sidebar: { live_snapshot: { agents: [agent] } },
      identity_status: { rows: [{ ...agent, display_name: agent.label, addressability: "addressable" }] } });
    if (url.pathname === "/console/modules") return json({ modules: [] });
    if (url.pathname === "/console/identities") return json({ rows: [agent] });
    if (url.pathname === "/console/timeline") return json(timeline(url.searchParams.get("identity")));
    if (url.pathname === "/console/rpc") {
      const chunks = [];
      request.on("data", chunk => chunks.push(chunk));
      request.on("end", () => {
        const call = JSON.parse(Buffer.concat(chunks)); requests.push(call);
        const reply = result => json({ jsonrpc: "2.0", id: call.id, result });
        if (call.method === "meta/capabilities") return reply({ methods: ["meta/console/send"], feature_capabilities: [] });
        if (call.method === "meta/console/query_timeline") return reply(timeline(call.params?.identity));
        if (call.method === "meta/console/inspect_identity") return reply({ identity: agent });
        if (call.method === "meta/console/voice/readiness") return reply({ available: false });
        return reply({});
      }); return;
    }
    response.writeHead(404); response.end("Not found");
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return { baseUrl: `http://127.0.0.1:${server.address().port}`, requests,
    emit(item) {
      for (const response of streams) response.write(`id: ${item.cursor}\nevent: ${item.kind}\ndata: ${JSON.stringify({ type: "console_frame", frame: item })}\n\n`);
    },
    async close() {
      for (const response of streams) response.end();
      server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
    },
  };
}

async function toolOrder(page, pane) {
  const folds = pane.locator("details.cc-completed-tools");
  const group = pane.locator("section.cc-tool-call--group").filter({ hasText: "late-review.txt" }).last();
  const inputs = await group.locator(".cc-tool-call__sub .cc-tool-call__section")
    .filter({ has: page.getByText("Input", { exact: true }) }).locator("pre").allTextContents();
  return {
    folds: await folds.count(),
    folded: await folds.locator(".cc-tool-call__name").allTextContents(),
    group: inputs.map(text => JSON.parse(text).path),
  };
}

async function runHistoryReplayToolOrder() {
  const fixture = await startServer(capture.rpc);
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  page.setDefaultTimeout(10_000);
  const result = { scope: "Real Chromium, embedded bundle, captured HTTP/SSE frames, zero provider calls",
    errors: [], emitted: [] };
  page.on("pageerror", error => result.errors.push(error.message));
  const expected = { folds: 1, folded: ["read_file", "list_files"], group: ["missing.txt", "late-review.txt"] };
  try {
    await page.goto(`${fixture.baseUrl}/console`, { waitUntil: "domcontentloaded" });
    const pane = page.getByTestId(`chat-pane:${identity}`);
    await pane.waitFor();
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    await pane.locator("section.cc-tool-call--group").filter({ hasText: "late-review.txt" }).last().waitFor();
    result.live = await toolOrder(page, pane);
    assert.deepEqual(result.live, expected, "the live rounds fold the two initial successes and group the later reads");
    for (const frame of capture.replay) { result.emitted.push(frame.cursor); fixture.emit(frame); }
    await pane.getByText(INTERRUPTED_RUN_NOTICE_PREFIX).last().waitFor();
    result.replayed = await toolOrder(page, pane);
    assert.deepEqual(result.replayed, expected,
      "after the saved rounds replay, the parallel calls keep their saved order and each read stays in its own round");
    assert.deepEqual(result.errors, []);
    result.passed = true;
    process.stdout.write("browser history replay tool order ok\n");
  } catch (error) {
    result.error = String(error.stack || error); throw error;
  } finally {
    result.methods = fixture.requests.map(call => call.method);
    await fs.mkdir(evidence, { recursive: true });
    await page.screenshot({ path: path.join(evidence, "history-replay-tool-order.png"), fullPage: true });
    await fs.writeFile(path.join(evidence, "history-replay-tool-order.json"), JSON.stringify(result, null, 2) + "\n");
    await browser.close(); await fixture.close();
  }
}

const scenarios = [{ id: "history-replay-keeps-saved-tool-order", family: "presentation", backend: "mock", run: runHistoryReplayToolOrder }];
module.exports = { scenarios };
