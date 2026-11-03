"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const http = require("node:http");
const path = require("node:path");
const { createHash } = require("node:crypto");
const { chromium } = require("playwright");

const identity = "history-keeper";
const session = "keeper-session";
const runtime = "default";
const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
const assetDirectory = path.join(__dirname, "../../crates/meerkat-meta/console-dist");
const now = Date.parse("2026-09-28T00:59:40Z");

function frame(cursor, kind, payload, extra = {}) {
  return { id: `keeper-${cursor}`, cursor: `console:${cursor}`, frame_version: 1,
    runtime_key: runtime, conversation_id: identity, identity, session_id: session,
    kind, status: "completed", timestamp_ms: now + cursor, payload,
    source: { kind: "session_history", source_cursor: `${session}:${cursor}` }, ...extra };
}

function historicalFrames() {
  const receipt = JSON.stringify({ kind: "peer_response", receipt: { delivery: "queued",
    envelope_id: "reply-envelope", in_reply_to: "kickoff-interaction", kind: "peer_response_sent" }, status: "sent" });
  return [
    frame(37, "tool_call_requested", { id: "reply-call", tool_call_id: "reply-call", name: "send_response",
      assistant_message_id: "tool-assistant", args: { peer_id: "primary-peer", in_reply_to: "kickoff-interaction", status: "completed" },
      type: "session_history", source_event_type: "session_history" }, { run_id: "kickoff-run", interaction_id: "kickoff-interaction" }),
    // The captured tool result has no run or interaction ID. Saved tool rows
    // remain transcript evidence and never reserve current work.
    frame(38, "tool_execution_completed", { id: "reply-call", tool_call_id: "reply-call", result: receipt,
      content: [{ type: "text", text: receipt }], is_error: false, type: "session_history", source_event_type: "session_history" }),
    frame(39, "text_complete", { assistant_message_id: "final-assistant", text: "Kickoff started acknowledged.",
      result: "Kickoff started acknowledged.", type: "session_history", source_event_type: "session_history",
      message: { role: "block_assistant", assistant_message_id: "final-assistant", stop_reason: "end_turn",
        identity: { run_id: "kickoff-run", interaction_id: "kickoff-interaction" },
        blocks: [{ block_type: "text", data: { text: "Kickoff started acknowledged." } }] } },
    { run_id: "kickoff-run", interaction_id: "kickoff-interaction" }),
  ];
}

function snapshot(extra = {}) {
  return frame(43, "assistant_history_snapshot", { complete: true, session_id: session,
    observed_through: "console:39", assistant_message_ids: ["tool-assistant", "final-assistant"] },
  { status: "delivered", source: { kind: "session_history" }, ...extra });
}

async function startServer(initialFrames) {
  const assets = new Map();
  for (const [url, name, type] of [["/console", "index.html", "text/html"],
    ["/console/assets/console-app.js", "console-app.js", "application/javascript"],
    ["/console/assets/console-app.css", "console-app.css", "text/css"]]) {
    assets.set(url, { name, type, bytes: await fs.readFile(path.join(assetDirectory, name)) });
  }
  const frames = [...initialFrames], requests = [], streams = new Set();
  const agent = { identity, member_id: identity, agent_id: identity, session_id: session,
    label: "History Keeper", kind: "identity", state: "active", addressable: true,
    response_phase: null, affordances: { addressable: true, can_send_message: true } };
  function timeline(selected) {
    const visible = frames.filter(item => !selected || item.identity === selected);
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
      response.write(`: connected\n\nevent: snapshot_complete\ndata: ${JSON.stringify({ type: "snapshot_complete", cursor: frames.at(-1)?.cursor })}\n\n`);
      streams.add(response); response.on("close", () => streams.delete(response)); return;
    }
    if (url.pathname === "/console/experience") return json({ contract_version: "0.5.0", runtime_id: "settled-history-proof",
      storage_scope: "settled-history-proof", runtime_capabilities: { can_send_messages: true }, voice: { available: false },
      console_config: { title: "History recovery", appearance: { default_theme: "light" },
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
        if (call.method === "meta/console/send") return reply({ accepted: true, identity, session_id: session, interaction_id: "queued-send" });
        if (call.method === "meta/console/inspect_identity") return reply({ identity: agent });
        if (call.method === "meta/console/voice/readiness") return reply({ available: false });
        return reply({});
      }); return;
    }
    response.writeHead(404); response.end("Not found");
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  return { baseUrl: `http://127.0.0.1:${server.address().port}`, requests, frames,
    assets: [...assets.values()].map(asset => ({ name: asset.name, sha256: createHash("sha256").update(asset.bytes).digest("hex") })),
    emit(item) {
      for (const response of streams) response.write(`id: ${item.cursor}\nevent: ${item.kind}\ndata: ${JSON.stringify({ type: "console_frame", frame: item })}\n\n`);
    },
    async close() {
      for (const response of streams) response.end();
      server.closeAllConnections(); await new Promise(resolve => server.close(resolve));
    },
  };
}

async function runHistoryQueue(mode) {
  const tools = historicalFrames().slice(0, 2);
  const noText = ["no-final-text", "late-history", "update-to-history", "update-to-live"].includes(mode);
  const initial = noText ? (mode === "late-history" ? [] : [...tools]) : historicalFrames();
  const boundary = snapshot();
  if (noText) boundary.payload.assistant_message_ids = ["tool-assistant"];
  if (["seed", "late-history", "update-to-history", "update-to-live"].includes(mode)) initial.push(boundary);
  if (mode === "update-to-history") initial[0] = { ...initial[0], source: { kind: "console_event" } };
  if (mode === "active-run") {
    initial.unshift(frame(36, "run_started", { run_id: "kickoff-run", source_event_type: "run_started" },
      { source: { kind: "console_event" }, run_id: "kickoff-run", interaction_id: "kickoff-interaction", status: "delivered" }));
    initial.push(frame(40, "run_started", { run_id: "current-run", source_event_type: "run_started" },
      { source: { kind: "console_event" }, run_id: "current-run", interaction_id: "kickoff-interaction", status: "delivered" }));
    initial.push(frame(41, "text_delta", { delta: "Current run is still working." },
      { source: { kind: "console_event" }, run_id: "current-run", interaction_id: "kickoff-interaction", status: "delivered" }));
  }
  if (mode === "newer-live") initial.push(frame(44, "tool_call_requested", { id: "live-call", tool_call_id: "live-call", name: "peers" },
    { source: { kind: "console_event" }, run_id: "current-run", interaction_id: "kickoff-interaction", status: "delivered" }));
  if (mode === "other-session") {
    boundary.session_id = "other-session"; boundary.payload.session_id = "other-session";
  }
  const fixture = await startServer(initial);
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  page.setDefaultTimeout(5_000);
  const result = { mode, scope: "Real Chromium, embedded bundle, captured-shape fixture HTTP/SSE, zero provider calls", assets: fixture.assets,
    errors: [], sent: [], frames: initial, emitted: [] };
  page.on("pageerror", error => result.errors.push(error.message));
  const sends = () => fixture.requests.filter(call => call.method === "meta/console/send");
  const emit = item => { result.emitted.push(item); fixture.emit(item); };
  const waitForSend = () => page.waitForResponse(response => response.url().endsWith("/console/rpc")
    && response.request().postDataJSON()?.method === "meta/console/send");
  const waitsForCurrentWork = ["active-run", "newer-live", "update-to-history", "update-to-live"].includes(mode);
  const text = `Continue the review after history recovery (${mode}).`;
  try {
    await page.goto(`${fixture.baseUrl}/console`, { waitUntil: "domcontentloaded" });
    const pane = page.getByTestId(`chat-pane:${identity}`);
    await pane.waitFor();
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    const typing = page.getByTestId(`chat-typing:${identity}`);
    if (!noText) await pane.getByText("Kickoff started acknowledged.", { exact: true }).waitFor();
    if (noText && mode !== "late-history") {
      await pane.locator("[data-conversation-row-id]").first().waitFor();
    }
    if (mode === "no-final-text") {
      assert.equal(await typing.count(), 0, "saved tools cannot open a phase even without final text or a snapshot");
      emit(boundary);
      await page.waitForTimeout(350);
      assert.equal(await typing.count(), 0, "a history snapshot cannot open a phase either");
    }
    if (mode === "late-history") {
      emit(tools[0]); emit(tools[1]);
      await pane.locator("[data-conversation-row-id]").first().waitFor();
      await page.waitForTimeout(350);
      assert.equal(await typing.count(), 0, "late covered history cannot reopen the working indicator");
    }
    const update = sourceKind => frame(44, "frame_updated", { frame: {
      ...tools[0], frame_version: 2, updated_at_ms: now + 44,
      source: { kind: sourceKind, source_cursor: `${session}:37` },
    } }, { source: { kind: "synthetic" }, status: "delivered" });
    if (mode === "update-to-live") {
      assert.equal(await typing.count(), 0, "covered history initially has no local working phase");
      emit(update("console_event"));
      await typing.waitFor();
    }
    if (!waitsForCurrentWork) {
      assert.equal(await typing.count(), 0, "history-only rows cannot reserve current work, with or without a snapshot");
    }
    await page.getByTestId(`chat-composer:${identity}`).fill(text);
    let sendResponse = waitsForCurrentWork ? null : waitForSend();
    await page.getByTestId(`chat-send:${identity}`).click();
    if (waitsForCurrentWork) {
      await page.getByTestId("pending-stack").waitFor();
      assert.equal(sends().length, 0, "current live work must retain the queued draft");
      emit(boundary);
      await page.waitForTimeout(350);
      await page.getByTestId("pending-stack").waitFor();
      assert.equal(sends().length, 0, "a snapshot cannot settle current live work");
      if (mode === "update-to-history") {
        sendResponse = waitForSend();
        emit(update("session_history"));
      } else {
        const terminal = (cursor, runId) => frame(cursor, "interaction_complete", { source_event_type: "interaction_complete" },
          { source: { kind: "console_event" }, run_id: runId, interaction_id: "kickoff-interaction" });
        emit(terminal(45, mode === "active-run" ? "kickoff-run" : "unrelated-old-run"));
        await page.waitForTimeout(350);
        await page.getByTestId("pending-stack").waitFor();
        assert.equal(sends().length, 0, "an old run terminal cannot release the current run's queued message");
        await typing.waitFor();
        sendResponse = waitForSend();
        emit(terminal(46, mode === "update-to-live" ? "kickoff-run" : "current-run"));
      }
    }
    await sendResponse;
    assert.equal(sends().length, 1, "the draft sends exactly once when current work permits it");
    assert.equal(sends()[0].params.content, text);
    assert.equal(sends()[0].params.identity, identity);
    if (mode !== "no-boundary") emit(boundary);
    await page.waitForTimeout(350);
    assert.equal(sends().length, 1, "history snapshot arrival or replay cannot submit again");
    assert.deepEqual(result.errors, []);
    result.passed = true;
    process.stdout.write(`browser settled history queue ${mode} ok\n`);
  } catch (error) {
    result.error = String(error.stack || error); throw error;
  } finally {
    result.sent = sends(); result.methods = fixture.requests.map(call => call.method);
    await fs.mkdir(evidence, { recursive: true });
    await page.screenshot({ path: path.join(evidence, `settled-history-queue-${mode}.png`), fullPage: true });
    await fs.writeFile(path.join(evidence, `settled-history-queue-${mode}.json`), JSON.stringify(result, null, 2) + "\n");
    await browser.close(); await fixture.close();
  }
}

const scenarios = ["seed", "stream", "active-run", "no-boundary", "other-session", "newer-live",
  "no-final-text", "late-history", "update-to-history", "update-to-live"].map(mode => ({
  id: `settled-history-queue-${mode}`, family: "settled-history", backend: "mock", run: () => runHistoryQueue(mode),
}));
module.exports = { scenarios };
