"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const http = require("node:http");
const path = require("node:path");
const { spawn } = require("node:child_process");
const { setTimeout: delay } = require("node:timers/promises");
const { exampleBackendSpec } = require("./example-backend.cjs");
const { LOOPBACK_ANY_PORT, awaitFixtureReady } = require("./fixture-ready.cjs");

const repoRoot = path.resolve(__dirname, "..");

async function listen(server) {
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  return server.address().port;
}

async function eventually(probe, label, timeout = 20_000) {
  const deadline = Date.now() + timeout;
  let last;
  do {
    try { const value = await probe(); if (value) return value; } catch (error) { last = error; }
    await delay(50);
  } while (Date.now() < deadline);
  throw new Error(`Timed out: ${label}${last ? `: ${last.message}` : ""}`);
}

async function stop(child) {
  if (child.exitCode !== null || child.signalCode !== null) return;
  child.kill("SIGTERM");
  await Promise.race([new Promise(resolve => child.once("exit", resolve)), delay(3000)]);
  if (child.exitCode === null && child.signalCode === null) child.kill("SIGKILL");
}

async function startFixture({ mode = "member", stateDir, liveImages = false, liveModel = false, routineTools = false, diagnoseStreams = false, sharedAssets = path.join(__dirname, ".tmp/acceptance") } = {}) {
  // Freeze the built production UI for this fixture lifetime. UI-only iterations
  // reuse the real Rust backend without recompiling its embedded asset bytes.
  const consoleAssets = new Map(await Promise.all([
    ["/console", "index.html", "text/html; charset=utf-8"],
    ["/console/assets/console-app.js", "console-app.js", "application/javascript"],
    ["/console/assets/console-app.css", "console-app.css", "text/css"],
  ].map(async ([url, file, contentType]) => [url, { bytes: await fs.readFile(path.join(__dirname, "dist", file)), contentType }])));
  const spec = exampleBackendSpec(repoRoot, "console_acceptance_fixture");
  const child = spawn(spec.command, spec.args, {
    cwd: repoRoot,
    env: { ...process.env, META_FIXTURE_ADDR: LOOPBACK_ANY_PORT, META_FIXTURE_MODE: mode,
      META_FIXTURE_LIVE_IMAGES: liveImages ? "1" : "0", META_FIXTURE_LIVE_MODEL: liveModel ? "1" : "0",
      META_FIXTURE_ROUTINE_TOOLS: routineTools ? "1" : "0",
      ...(stateDir ? { META_FIXTURE_STATE: stateDir } : {}) },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let logs = "";
  for (const output of [child.stdout, child.stderr]) output.on("data", chunk => { logs = (logs + chunk).slice(-30_000); });
  let backendUrl;
  try {
    ({ baseUrl: backendUrl } = await awaitFixtureReady(child, {
      label: `acceptance fixture (${mode})`, timeoutMs: spec.prebuilt ? 30_000 : 300_000,
    }));
    await eventually(async () => {
      if (child.exitCode !== null) return true;
      return (await fetch(`${backendUrl}/healthz`)).ok;
    }, `fixture readiness (${mode})`, spec.prebuilt ? 30_000 : 300_000);
    if (child.exitCode !== null) throw new Error(`fixture exited ${child.exitCode}: ${logs}`);
    const ready = await eventually(async () => {
      const { status, body } = await rpc(backendUrl, "meta/wait_ready", { timeout_ms: 1000 });
      assert.equal(status, 200);
      if (body.error) throw new Error(JSON.stringify(body.error));
      return body.result?.timeout === false && body.result.ready?.length === 2 ? body.result : null;
    }, "both fixture members finish startup", 30_000);
    assert.equal(new Set(ready.ready.map(member => member.agent_identity)).size, 2);
  } catch (error) { await stop(child); throw new Error(`${error.message}\n${logs}`); }

  const observations = [];
  const streamDiagnostics = [];
  const streams = new Set();
  let unavailableStreams = 0;
  let dropSend = false;
  const proxy = http.createServer(async (req, res) => {
    const url = new URL(req.url, backendUrl);
    const asset = consoleAssets.get(url.pathname === "/console/" ? "/console" : url.pathname);
    if (asset && req.method === "GET") {
      res.writeHead(200, { "content-type": asset.contentType }); res.end(asset.bytes); return;
    }
    if (["/shared", "/shared/", "/scoped", "/scoped/"].includes(url.pathname)) {
      const asset = url.pathname.startsWith("/scoped") ? "scoped" : "host";
      res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
      res.end(`<!doctype html><html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1"><title>Console acceptance host</title><link rel="stylesheet" href="/shared/${asset}.css"></head><body><div id="root"></div><script src="/shared/${asset}.js"></script></body></html>`);
      return;
    }
    if (["/shared/host.js", "/shared/host.css", "/shared/scoped.js", "/shared/scoped.css"].includes(url.pathname)) {
      try {
        const bytes = await fs.readFile(path.join(sharedAssets, path.basename(url.pathname)));
        res.writeHead(200, { "content-type": url.pathname.endsWith(".js") ? "application/javascript" : "text/css" }); res.end(bytes);
      } catch (error) { res.writeHead(500); res.end(`missing built shared host: ${error.message}`); }
      return;
    }
    const observation = { method: req.method, path: req.url, request: "", status: null, response: "" };
    observations.push(observation);
    const isStream = url.pathname.endsWith("/timeline/stream");
    if (isStream && unavailableStreams > 0) {
      unavailableStreams -= 1; observation.status = 503;
      res.writeHead(503, { "content-type": "application/json" });
      res.end('{"error":"fixture_unavailable"}'); return;
    }
    const upstream = http.request(url, { method: req.method, headers: { ...req.headers, host: url.host } }, incoming => {
      observation.status = incoming.statusCode;
      const drop = dropSend && req.method === "POST" && (url.pathname.endsWith("/send") || observation.request.includes('"method":"meta/console/send"'));
      if (drop) dropSend = false;
      res.writeHead(incoming.statusCode, incoming.headers);
      // Commit actual response headers before simulating body loss. Otherwise
      // Chromium may transparently replay the POST on a stale pooled socket.
      if (drop) res.flushHeaders();
      const stream = { incoming, response: res };
      if (isStream) streams.add(stream);
      const diagnostic = isStream && diagnoseStreams && streamDiagnostics.length < 16
        ? { path: req.url, headersAtMs: Date.now(), chunks: 0, bytes: 0, frames: 0, kinds: {}, runs: {}, first: [], last: [], oversizedBlocks: 0, parseErrors: 0 }
        : null;
      if (diagnostic) streamDiagnostics.push(diagnostic);
      const decoder = diagnostic ? new TextDecoder() : null;
      let diagnosticBuffer = "";
      incoming.on("data", chunk => {
        if (!isStream && observation.response.length < 1_048_576) observation.response += chunk;
        if (!diagnostic) return;
        diagnostic.chunks += 1; diagnostic.bytes += chunk.length;
        diagnosticBuffer += decoder.decode(chunk, { stream: true });
        let end;
        while ((end = diagnosticBuffer.indexOf("\n\n")) >= 0) {
          const block = diagnosticBuffer.slice(0, end); diagnosticBuffer = diagnosticBuffer.slice(end + 2);
          const source = block.split("\n").filter(line => line.startsWith("data:")).map(line => line.slice(5).trimStart()).join("\n");
          if (!source) continue;
          try {
            const value = JSON.parse(source);
            const frame = value.frame || value;
            const kind = frame.kind || frame.event || block.split("\n").find(line => line.startsWith("event:"))?.slice(6).trim();
            const sample = { atMs: Date.now(), kind, id: frame.id, cursor: frame.cursor, timestampMs: frame.timestamp_ms, runId: frame.run_id, deltaChars: frame.payload?.delta?.length };
            diagnostic.frames += 1; diagnostic.kinds[kind] = (diagnostic.kinds[kind] || 0) + 1;
            if (sample.runId && (diagnostic.runs[sample.runId] || Object.keys(diagnostic.runs).length < 16)) {
              const run = diagnostic.runs[sample.runId] ||= { firstAtMs: sample.atMs, frames: 0, deltas: 0, deltaChars: 0 };
              run.lastAtMs = sample.atMs; run.frames += 1;
              if (kind === "text_delta") {
                run.deltas += 1; run.deltaChars += sample.deltaChars || 0;
                run.firstDeltaAtMs ??= sample.atMs; run.lastDeltaAtMs = sample.atMs;
              }
            }
            if (diagnostic.first.length < 32) diagnostic.first.push(sample);
            diagnostic.last.push(sample); if (diagnostic.last.length > 32) diagnostic.last.shift();
          } catch { diagnostic.parseErrors += 1; }
        }
        if (diagnosticBuffer.length > 65_536) { diagnosticBuffer = ""; diagnostic.oversizedBlocks += 1; }
      });
      incoming.on("end", () => { streams.delete(stream); if (drop) { observation.dropped = true; res.destroy(); } });
      incoming.on("error", error => { streams.delete(stream); if (!res.destroyed) res.destroy(error); });
      res.on("close", () => { streams.delete(stream); incoming.destroy(); });
      if (drop) incoming.resume(); else incoming.pipe(res);
    });
    req.on("data", chunk => { if (observation.request.length < 1_048_576) observation.request += chunk; });
    upstream.on("error", error => { if (!res.headersSent) res.writeHead(502); if (!res.destroyed) res.end(error.message); });
    req.pipe(upstream);
  });
  const port = await listen(proxy);
  const baseUrl = `http://127.0.0.1:${port}`;
  return {
    baseUrl, backendUrl, observations, streamDiagnostics,
    async control(name, body) {
      const response = await fetch(`${backendUrl}/__fixture/${name}`, {
        method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
      });
      const text = await response.text();
      assert.equal(response.status, 200, text);
      return JSON.parse(text);
    },
    dropNextSendResponse() { dropSend = true; },
    rejectNextStreams(count = 1) { unavailableStreams = count; },
    disconnectStreams() { for (const stream of streams) { stream.response.destroy(); stream.incoming.destroy(); } streams.clear(); },
    async close() {
      for (const stream of streams) { stream.response.destroy(); stream.incoming.destroy(); }
      proxy.closeAllConnections();
      await new Promise(resolve => proxy.close(resolve));
      await stop(child);
    },
    logs: () => logs,
  };
}

async function rpc(baseUrl, method, params = {}, prefix = "") {
  const response = await fetch(`${baseUrl}${prefix}/console/rpc`, {
    method: "POST", headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: 1, method, params }),
  });
  return { status: response.status, body: await response.json() };
}

async function snapshot(baseUrl, query = "", headers = {}) {
  const abort = new AbortController();
  const timer = setTimeout(() => abort.abort(), 15_000);
  const frames = [];
  try {
    const response = await fetch(`${baseUrl}/console/timeline/stream${query}`, { signal: abort.signal, headers });
    assert.equal(response.status, 200);
    const reader = response.body.getReader();
    const decoder = new TextDecoder(); let buffer = "";
    while (true) {
      const { value, done } = await reader.read();
      buffer += decoder.decode(value, { stream: !done });
      let end;
      while ((end = buffer.indexOf("\n\n")) >= 0) {
        const block = buffer.slice(0, end); buffer = buffer.slice(end + 2);
        const event = block.split("\n").find(line => line.startsWith("event:"))?.slice(6).trim();
        const data = block.split("\n").filter(line => line.startsWith("data:")).map(line => line.slice(5).trim()).join("\n");
        if (!data) continue;
        frames.push({ event, data: JSON.parse(data) });
        if (event === "snapshot_complete") return frames;
      }
      if (done) throw new Error("stream ended before snapshot_complete");
    }
  } finally { clearTimeout(timer); abort.abort(); }
}

module.exports = { startFixture, eventually, rpc, snapshot };
