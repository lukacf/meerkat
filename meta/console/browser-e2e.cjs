#!/usr/bin/env node

const assert = require("node:assert/strict");
const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");
const { spawn, spawnSync } = require("node:child_process");
const { setTimeout: sleep } = require("node:timers/promises");
const { chromium } = require("playwright");
const { exampleBackendSpec } = require("./example-backend.cjs");
const { LOOPBACK_ANY_PORT, awaitFixtureReady } = require("./fixture-ready.cjs");
const { routeGuard } = require("./scenarios/route-guard.cjs");

const repoRoot = path.resolve(__dirname, "..");

function stockTopologyEndpointId(authority, identity) {
  return `mk1|${encodeURIComponent(authority)}|${encodeURIComponent(identity)}`;
}

function waitForExit(child, timeoutMs = 5_000) {
  return new Promise((resolve) => {
    if (child.exitCode !== null) {
      resolve();
      return;
    }
    const timer = setTimeout(() => {
      if (child.exitCode === null) {
        child.kill("SIGKILL");
      }
      resolve();
    }, timeoutMs);
    child.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

async function stopBackend(child) {
  if (child.exitCode !== null) {
    return;
  }
  child.kill("SIGTERM");
  await waitForExit(child);
}


async function waitForHttpOk(url, timeoutMs = 300_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) {
        return;
      }
    } catch (_) {
      // Backend is still starting.
    }
    await sleep(500);
  }
  throw new Error(`timed out waiting for ${url}`);
}

// The runner reports a scenario's error only after the scenario's finally
// block has closed the browser. Print it first, so that nothing failing
// during the close can hide it.
function printScenarioError(label, error) {
  process.stderr.write(`scenario:error ${label}: ${error?.stack || error}\n`);
}

async function launchBrowser() {
  try {
    return await chromium.launch({ headless: true });
  } catch (launchError) {
    const npxCommand = process.platform === "win32" ? "npx.cmd" : "npx";
    const installResult = spawnSync(npxCommand, ["playwright", "install", "chromium"], {
      cwd: __dirname,
      stdio: "inherit",
    });
    if (installResult.status !== 0) {
      throw new Error(`playwright chromium install failed with status ${installResult.status}`);
    }
    return chromium.launch({ headless: true });
  }
}

async function gotoConsole(page, url) {
  await page.goto(url, { waitUntil: "domcontentloaded", timeout: 30_000 });
  await page.waitForSelector('[data-testid="meerkat-console"], .cc-workbench, .cc-conversation-pane', { timeout: 30_000 });
}

async function sidebarLabels(page) {
  return page.$$eval(
    '.agent[role="button"], .cc-sidebar-row',
    (rows) => rows.map((row) => (row.textContent || "").trim()),
  );
}

async function fillComposer(page, text) {
  const textarea = page
    .locator('[data-testid^="chat-composer:"]:visible, textarea:visible, .cc-composer__textarea:visible')
    .first();
  await textarea.fill(text);
}

async function clickSend(page) {
  const send = page.locator('button:has-text("Send"):visible, .cc-composer__send-btn:visible').first();
  await send.click();
}

async function clickLoadOlderHistory(pane) {
  let lastError;
  for (let attempt = 0; attempt < 3; attempt += 1) {
    try {
      const button = pane.getByRole("button", { name: "Load older history" });
      await button.waitFor({ state: "visible", timeout: 30_000 });
      await button.scrollIntoViewIfNeeded();
      // Revealing the button scrolls the transcript, but the scroll
      // controller only leaves "following the live edge" when the native
      // scroll event is delivered, which is asynchronous (and slow under
      // load). A synthetic click has no pointerdown to switch it earlier, so
      // clicking first lets the older page prepend while the pane still
      // follows the end, and it re-pins to the bottom. When the reveal moved
      // the transcript away from its end, wait for the pane's own published
      // away-from-end state (its "Jump to latest" control) before clicking.
      const awayFromEnd = await pane.locator(".conv__body").evaluate((node) =>
        node.scrollHeight - node.clientHeight - node.scrollTop > 2);
      if (awayFromEnd) {
        await pane.getByRole("button", { name: "Jump to latest" }).waitFor({ state: "visible", timeout: 10_000 });
        // Reaching the top can itself request the older page (the pane
        // auto-loads there), which renames or removes this button. That load
        // was requested after the pane left the live edge, so it is the
        // demand the caller wants; clicking again is only needed otherwise.
        if (!(await button.isVisible()) || !(await button.isEnabled())) return;
      }
      await button.evaluate((node) => node.click());
      return;
    } catch (error) {
      lastError = error;
      await sleep(100);
    }
  }
  throw lastError;
}

async function openSidebarAgentChat(page, labelPattern) {
  const row = page
    .locator('.agent[role="button"], .cc-sidebar-row')
    .filter({ hasText: labelPattern })
    .first();
  await row.click();
  await page.waitForSelector('[data-testid^="chat-composer:"]', { timeout: 30_000 });
}

async function assertNoText(page, text) {
  const count = await page.getByText(text, { exact: true }).count();
  assert.equal(count, 0, `did not expect text to be visible: ${text}`);
}

function rpcMethodsFromRequests(requests) {
  return requests
    .filter((request) => request.method === "POST" && request.url === "/console/rpc")
    .map((request) => {
      try {
        return JSON.parse(request.body || "{}").method;
      } catch (_) {
        return null;
      }
    })
    .filter((method) => typeof method === "string");
}

async function waitForRpcMethod(server, method, minCount = 1) {
  for (let attempt = 0; attempt < 80; attempt += 1) {
    const count = rpcMethodsFromRequests(server.requests)
      .filter((candidate) => candidate === method)
      .length;
    if (count >= minCount) return;
    await sleep(50);
  }
  throw new Error(`expected ${method} RPC; saw ${JSON.stringify(rpcMethodsFromRequests(server.requests))}`);
}

// Listens on port 0 in this process and reports the bound URL; picking a free
// port first and binding it later races other processes for the port.
function startMockConsoleServer(options = {}) {
  const html = fs.readFileSync(path.join(__dirname, "dist", "index.html"), "utf8");
  const js = fs.readFileSync(path.join(__dirname, "dist", "console-app.js"), "utf8");
  const css = fs.readFileSync(path.join(__dirname, "dist", "console-app.css"), "utf8");
  const tinyPng = Buffer.from(
    "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/p9sAAAAASUVORK5CYII=",
    "base64",
  );
  const timelineFrames = Array.isArray(options.timelineFrames) ? options.timelineFrames : [];
  const timelineStreamFrames = Array.isArray(options.timelineStreamFrames)
    ? options.timelineStreamFrames
    : [];
  const timelineFramesByIdentity =
    options.timelineFramesByIdentity && typeof options.timelineFramesByIdentity === "object"
      ? options.timelineFramesByIdentity
      : {};
  const timelineFrameSnapshotsByIdentity =
    options.timelineFrameSnapshotsByIdentity &&
    typeof options.timelineFrameSnapshotsByIdentity === "object"
      ? options.timelineFrameSnapshotsByIdentity
      : {};
  const timelineSnapshotQueryCountsByIdentity = new Map();
  const timelineReplayUnavailableAfterByIdentity =
    options.timelineReplayUnavailableAfterByIdentity &&
    typeof options.timelineReplayUnavailableAfterByIdentity === "object"
      ? options.timelineReplayUnavailableAfterByIdentity
      : {};
  const timelineFramesAfterReplayUnavailableByIdentity =
    options.timelineFramesAfterReplayUnavailableByIdentity &&
    typeof options.timelineFramesAfterReplayUnavailableByIdentity === "object"
      ? options.timelineFramesAfterReplayUnavailableByIdentity
      : {};
  const timelineReplayUnavailableTriggeredByIdentity = new Set();
  const timelineFramesAfterSendByIdentity =
    options.timelineFramesAfterSendByIdentity &&
    typeof options.timelineFramesAfterSendByIdentity === "object"
      ? options.timelineFramesAfterSendByIdentity
      : {};
  const timelineStreamFramesDuringSendByIdentity =
    options.timelineStreamFramesDuringSendByIdentity &&
    typeof options.timelineStreamFramesDuringSendByIdentity === "object"
      ? options.timelineStreamFramesDuringSendByIdentity
      : {};
  // Frames the runtime appends once the send is accepted reach the unscoped
  // live stream (append_and_emit); emitted right after the send response.
  const timelineStreamFramesAfterSendByIdentity =
    options.timelineStreamFramesAfterSendByIdentity &&
    typeof options.timelineStreamFramesAfterSendByIdentity === "object"
      ? options.timelineStreamFramesAfterSendByIdentity
      : {};
  const streamClients = new Set();
  const includeImageAgent = options.includeImageAgent === true;
  const includeBusyWorker = options.includeBusyWorker === true;
  const includeToolOnlyWorker = options.includeToolOnlyWorker === true;
  const includeLifecycleActions = options.includeLifecycleActions === true;
  const collapseWorkersSection = options.collapseWorkersSection === true;
  const topologyControl = options.topologyControl && typeof options.topologyControl === "object"
    ? options.topologyControl
    : null;
  const identityAffordances = includeLifecycleActions
    ? { can_send_message: true, can_respawn: true, can_retire: true }
    : { can_send_message: true };
  const requests = [];

  function cursorOrdinal(cursor, cursorOrder) {
    if (typeof cursor !== "string") return null;
    const match = /^console:(\d+)$/.exec(cursor);
    if (match) return Number(match[1]);
    return cursorOrder.get(cursor) ?? null;
  }

  function timelinePageFor(frames, params = {}) {
    const source = Array.isArray(frames) ? frames : [];
    const cursorOrder = new Map();
    source.forEach((frame, index) => {
      if (typeof frame.cursor === "string") cursorOrder.set(frame.cursor, index + 1);
    });
    const after = cursorOrdinal(params.after, cursorOrder);
    const before = cursorOrdinal(params.before, cursorOrder);
    const mode = params.mode === "recent" ? "recent" : "since";
    const limit = Math.max(1, Math.min(Number(params.limit) || 400, 1000));
    const filtered = source.filter((frame, index) => {
      const ordinal = cursorOrdinal(frame.cursor, cursorOrder) ?? index + 1;
      if (after !== null && ordinal <= after) return false;
      if (before !== null && ordinal >= before) return false;
      return true;
    });
    const pageFrames = mode === "recent" ? filtered.slice(-limit) : filtered.slice(0, limit);
    return {
      frames: pageFrames,
      next_cursor: pageFrames.length > 0 ? pageFrames[pageFrames.length - 1].cursor || null : null,
      latest_cursor: filtered.length > 0 ? filtered[filtered.length - 1].cursor || null : null,
      exhausted: filtered.length <= limit,
    };
  }

  function writeTimelineStreamFrame(res, frame, index) {
    res.write([
      `id: ${frame.id || `timeline-live-${index}`}`,
      `event: ${frame.kind || frame.event || "message"}`,
      `data: ${JSON.stringify({ type: "console_frame", frame })}`,
      "",
      "",
    ].join("\n"));
  }

  function emitStreamFramesForIdentity(identity, framesByIdentity, label) {
    const frames =
      typeof identity === "string" && Array.isArray(framesByIdentity[identity])
        ? framesByIdentity[identity]
        : [];
    if (frames.length === 0 || streamClients.size === 0) return;
    let index = 0;
    for (const res of streamClients) {
      for (const frame of frames) {
        index += 1;
        writeTimelineStreamFrame(res, frame, `${label}-${index}`);
      }
    }
  }

  function emitDuringSendFrames(identity) {
    emitStreamFramesForIdentity(identity, timelineStreamFramesDuringSendByIdentity, "during-send");
  }

  const server = http.createServer((req, res) => {
    const method = req.method || "GET";
    const url = req.url || "/";
    const chunks = [];
    req.on("data", (chunk) => chunks.push(chunk));
    req.on("end", () => {
      const body = Buffer.concat(chunks).toString("utf8");
      requests.push({ method, url, body, headers: req.headers });

      if (method === "GET" && url === "/console") {
        res.writeHead(200, { "content-type": "text/html; charset=utf-8" });
        res.end(html);
        return;
      }
      if (method === "GET" && url.startsWith("/console/assets/console-app.js")) {
        res.writeHead(200, { "content-type": "application/javascript; charset=utf-8" });
        res.end(js);
        return;
      }
      if (method === "GET" && url.startsWith("/console/assets/console-app.css")) {
        res.writeHead(200, { "content-type": "text/css; charset=utf-8" });
        res.end(css);
        return;
      }
      if (method === "GET" && url.startsWith("/blobs/")) {
        res.writeHead(200, { "content-type": "image/png", "cache-control": "no-store" });
        res.end(tinyPng);
        return;
      }
      if (method === "GET" && url === "/console/modules") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ modules: [] }));
        return;
      }
      if (method === "GET" && url === "/console/identities") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({
          rows: [
            {
              identity: "identity:luka",
              display_name: "Identity Example",
              profile: "lead",
              state: "running",
              addressability: "addressable",
            },
            {
              identity: "legacy-router",
              display_name: "Legacy Router",
              profile: "router",
              state: "running",
              addressability: "addressable",
            },
            ...(includeImageAgent ? [{
              identity: "image-agent",
              display_name: "Image Agent",
              profile: "coordinator",
              state: "running",
              addressability: "addressable",
              model_capabilities: { image_input: true },
            }] : []),
            ...(includeBusyWorker ? [{
              identity: "person-worker-alpha",
              display_name: "Person Worker Alpha",
              profile: "person-worker",
              state: "running",
              addressability: "addressable",
            }] : []),
            ...(includeToolOnlyWorker ? [{
              identity: "tool-only-worker",
              display_name: "Tool Only Worker",
              profile: "investigation-worker",
              state: "running",
              addressability: "addressable",
            }] : []),
          ],
        }));
        return;
      }
      if (method === "GET" && url === "/console/experience") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({
          contract_version: "0.5.0",
          runtime_capabilities: {
            can_send_messages: true,
            can_retire_members: true,
            can_spawn_members: true,
          },
          agent_sidebar: {
            panel_id: "console.agent_sidebar",
            schema_version: "1",
            refresh: { mode: "pull", interval_ms: 5000 },
            live_snapshot: {
              agents: [
                {
                  agent_id: "identity:luka",
                  member_id: "identity:luka",
                  identity: "identity:luka",
                  label: "Identity Example",
                  kind: "identity",
                  profile: "lead",
                  state: "running",
                  addressable: true,
                  affordances: identityAffordances,
                },
                {
                  agent_id: "legacy-router",
                  member_id: "legacy-router",
                  label: "Legacy Router",
                  kind: "module_agent",
                  profile: "router",
                  state: "running",
                  addressable: true,
                  affordances: { can_send_message: true },
                },
                ...(includeImageAgent ? [{
                  agent_id: "image-agent",
                  member_id: "image-agent",
                  identity: "image-agent",
                  label: "Image Agent",
                  kind: "identity",
                  profile: "coordinator",
                  state: "running",
                  addressable: true,
                  affordances: { can_send_message: true },
                  model_capabilities: { image_input: true },
                }] : []),
                ...(includeToolOnlyWorker ? [{
                  agent_id: "tool-only-worker",
                  member_id: "tool-only-worker",
                  identity: "tool-only-worker",
                  label: "Tool Only Worker",
                  kind: "identity",
                  profile: "investigation-worker",
                  role: "investigation-worker",
                  group: "Workers",
                  state: "active",
                  addressable: true,
                  affordances: { can_send_message: true },
                }] : []),
                ...(includeBusyWorker ? [{
                  agent_id: "person-worker-alpha",
                  member_id: "person-worker-alpha",
                  identity: "person-worker-alpha",
                  label: "Person Worker Alpha",
                  kind: "identity",
                  profile: "person-worker",
                  role: "person-worker",
                  group: "Workers",
                  state: "active",
                  addressable: true,
                  response_phase: "tool-executing",
                  affordances: { can_send_message: true },
                }] : []),
              ],
            },
          },
          identity_status: {
            panel_id: "console.identity_status",
            schema_version: "1",
            refresh: { mode: "poll", interval_ms: 5000 },
            rows: [
              {
                identity: "identity:luka",
                display_name: "Identity Example",
                profile: "lead",
                state: "running",
                addressability: "addressable",
                labels: {},
              },
              ...(includeImageAgent ? [{
                identity: "image-agent",
                display_name: "Image Agent",
                profile: "coordinator",
                state: "running",
                addressability: "addressable",
                labels: {},
                model_capabilities: { image_input: true },
              }] : []),
              ...(includeBusyWorker ? [{
                identity: "person-worker-alpha",
                display_name: "Person Worker Alpha",
                profile: "person-worker",
                state: "active",
                response_phase: "tool-executing",
                addressability: "addressable",
                labels: {},
              }] : []),
              ...(includeToolOnlyWorker ? [{
                identity: "tool-only-worker",
                display_name: "Tool Only Worker",
                profile: "investigation-worker",
                state: "active",
                addressability: "addressable",
                labels: {},
              }] : []),
            ],
          },
          ...(collapseWorkersSection ? {
            console_config: {
              agent_list: {
                group_by: ["group", "role"],
                section_order: ["Identity", "Router", "Workers"],
                sections: [
                  { name: "Workers", collapsed: true },
                ],
              },
            },
          } : {}),
        }));
        return;
      }
      if (method === "POST" && url === "/console/rpc") {
        const payload = JSON.parse(body || "{}");
        const rpcId = payload.id || "rpc";
        if (payload.method === "meta/capabilities") {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify({
            jsonrpc: "2.0",
            id: rpcId,
            result: {
              version: "browser-e2e",
              methods: [
                "meta/capabilities",
                "meta/console/send",
                "meta/console/inspect_identity",
                "meta/console/query_timeline",
                "meta/blob/upload",
                "meta/retire",
                "meta/respawn",
                "meta/reset",
                "meta/routing/routes/list",
                "meta/delivery/history",
                "meta/gating/pending",
                "meta/gating/audit",
                "meta/gating/decide",
                ...(topologyControl ? topologyControl.methods || [] : []),
              ],
              ...(topologyControl ? { topology_control: topologyControl.capabilities || {} } : {}),
            },
          }));
          return;
        }
        if (payload.method === "meta/topology/query" && topologyControl) {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify({
            jsonrpc: "2.0",
            id: rpcId,
            result: topologyControl.query,
          }));
          return;
        }
        if (
          topologyControl
          && typeof topologyControl.handleRpc === "function"
          && topologyControl.handleRpc(payload, res)
        ) {
          return;
        }
        if (payload.method === "meta/console/query_timeline") {
          const identity = payload.params?.identity;
          let framesForIdentity = null;
          if (
            typeof identity === "string" &&
            typeof timelineReplayUnavailableAfterByIdentity[identity] === "string" &&
            payload.params?.after === timelineReplayUnavailableAfterByIdentity[identity] &&
            !timelineReplayUnavailableTriggeredByIdentity.has(identity)
          ) {
            timelineReplayUnavailableTriggeredByIdentity.add(identity);
            res.writeHead(200, { "content-type": "application/json" });
            res.end(JSON.stringify({
              jsonrpc: "2.0",
              id: rpcId,
              error: {
                code: -32013,
                message: "timeline replay unavailable",
                data: {
                  error: "replay_unavailable",
                  stream: "timeline",
                  requested_cursor: payload.params.after,
                  latest_cursor: payload.params.after,
                },
              },
            }));
            return;
          }
          if (
            typeof identity === "string" &&
            Array.isArray(timelineFrameSnapshotsByIdentity[identity])
          ) {
            const snapshots = timelineFrameSnapshotsByIdentity[identity];
            const queryCount = timelineSnapshotQueryCountsByIdentity.get(identity) || 0;
            timelineSnapshotQueryCountsByIdentity.set(identity, queryCount + 1);
            const snapshot = snapshots[Math.min(queryCount, snapshots.length - 1)];
            if (snapshot && typeof snapshot === "object" && !Array.isArray(snapshot) && snapshot.error) {
              res.writeHead(200, { "content-type": "application/json" });
              res.end(JSON.stringify({
                jsonrpc: "2.0",
                id: rpcId,
                error: snapshot.error,
              }));
              return;
            }
            framesForIdentity = Array.isArray(snapshot) ? snapshot : snapshot?.frames;
          } else if (
            typeof identity === "string" &&
            Array.isArray(timelineFramesByIdentity[identity])
          ) {
            framesForIdentity =
              timelineReplayUnavailableTriggeredByIdentity.has(identity) &&
              Array.isArray(timelineFramesAfterReplayUnavailableByIdentity[identity])
                ? timelineFramesAfterReplayUnavailableByIdentity[identity]
                : timelineFramesByIdentity[identity];
          }
          const frames = framesForIdentity || (identity ? (identity === "image-agent" ? timelineFrames : []) : timelineFrames);
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify({
            jsonrpc: "2.0",
            id: rpcId,
            result: timelinePageFor(frames, payload.params),
          }));
          return;
        }
        if (payload.method === "meta/console/inspect_identity") {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify({
            jsonrpc: "2.0",
            id: rpcId,
            result: { identity: payload.params.identity, peers: [] },
          }));
          return;
        }
        if (
          payload.method === "meta/retire" ||
          payload.method === "meta/respawn" ||
          payload.method === "meta/reset"
        ) {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify({
            jsonrpc: "2.0",
            id: rpcId,
            result: {
              accepted: true,
              identity: payload.params?.identity,
              method: payload.method,
            },
          }));
          return;
        }
        if (payload.method === "meta/routing/routes/list") {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify({
            jsonrpc: "2.0",
            id: rpcId,
            result: {
              routes: [{
                route_key: "route-control-1",
                recipient: "identity:luka",
                channel: "identity",
                sink: "console",
                target_module: "meta",
                retry_max: 2,
                backoff_ms: 25,
                rate_limit_per_minute: 60,
              }],
            },
          }));
          return;
        }
        if (payload.method === "meta/delivery/history") {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify({
            jsonrpc: "2.0",
            id: rpcId,
            result: {
              deliveries: [{
                delivery_id: "delivery-control-1",
                route_id: "route-control-1",
                recipient: "identity:luka",
                sink: "console",
                target_module: "meta",
                status: "delivered",
                first_attempt_ms: 1717171700,
                final_attempt_ms: 1717171710,
                attempts: [{ attempt: 1, status: "delivered", backoff_ms: 0 }],
              }],
            },
          }));
          return;
        }
        if (payload.method === "meta/gating/pending") {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify({
            jsonrpc: "2.0",
            id: rpcId,
            result: {
              pending: [{
                pending_id: "gate-control-1",
                action_id: "deploy-preview",
                identity: "identity:luka",
                waited_ms: 1_500,
                risk_tier: "medium",
                payload: { branch: "codex/console-progressive-customization" },
              }],
            },
          }));
          return;
        }
        if (payload.method === "meta/gating/audit") {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify({
            jsonrpc: "2.0",
            id: rpcId,
            result: {
              entries: [{
                audit_id: "gate-audit-1",
                event_type: "decision_recorded",
                actor: "console-browser-proof",
                decided_at_ms: 1717171717,
              }],
            },
          }));
          return;
        }
        if (payload.method === "meta/gating/decide") {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify({
            jsonrpc: "2.0",
            id: rpcId,
            result: {
              pending_id: payload.params?.pending_id,
              approver_id: payload.params?.approver_id,
              decision: payload.params?.decision,
              outcome: "accepted",
            },
          }));
          return;
        }
        if (payload.method === "meta/console/send") {
          const identity = payload.params?.identity;
          if (
            typeof identity === "string" &&
            Array.isArray(timelineFramesAfterSendByIdentity[identity])
          ) {
            const existing = Array.isArray(timelineFramesByIdentity[identity])
              ? timelineFramesByIdentity[identity]
              : [];
            timelineFramesByIdentity[identity] = [
              ...existing,
              ...timelineFramesAfterSendByIdentity[identity],
            ];
          }
          emitDuringSendFrames(identity);
          const sendResponse = JSON.stringify({
            jsonrpc: "2.0",
            id: rpcId,
            result: {
              accepted: true,
              interaction_id: `turn-${identity || "unknown"}`,
              identity,
            },
          });
          const writeSendResponse = () => {
            res.writeHead(200, { "content-type": "application/json" });
            res.end(sendResponse);
            emitStreamFramesForIdentity(identity, timelineStreamFramesAfterSendByIdentity, "after-send");
          };
          if (options.consoleSendResponseDelayMs) {
            setTimeout(writeSendResponse, options.consoleSendResponseDelayMs);
          } else {
            writeSendResponse();
          }
          return;
        }
        if (payload.method === "meta/interact") {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify({
            jsonrpc: "2.0",
            id: rpcId,
            result: {
              interaction_id: "turn-identity-1",
              identity: payload.params.identity,
            },
          }));
          return;
        }
        if (payload.method === "meta/send_message") {
          res.writeHead(200, { "content-type": "application/json" });
          res.end(JSON.stringify({
            jsonrpc: "2.0",
            id: rpcId,
            result: {
              accepted: true,
              member_id: payload.params.member_id,
              session_id: "sess-legacy-1",
            },
          }));
          return;
        }
      }
      if (method === "POST" && url === "/console/rpc/multipart") {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({
          jsonrpc: "2.0",
          id: "multipart-rpc",
          result: {
            accepted: true,
            interaction_id: "turn-image-upload-1",
            identity: "image-agent",
          },
        }));
        return;
      }
      if (method === "POST" && url === "/console/identity/stream") {
        res.writeHead(200, { "content-type": "text/event-stream" });
        res.end([
          "id: subscribed-identity-1",
          "event: subscribed",
          'data: {"event_id":"subscribed-identity-1","identity":"identity:luka","event_type":"subscribed","timestamp_ms":1,"data":{"stream":"identity"}}',
          "",
          "id: evt-identity-1",
          "event: interaction_complete",
          'data: {"event_id":"evt-identity-1","interaction_id":"turn-identity-1","identity":"identity:luka","event_type":"interaction_complete","timestamp_ms":2,"data":{"text":"identity done"}}',
          "",
        ].join("\n"));
        return;
      }
      if (method === "GET" && url.startsWith("/console/timeline/stream")) {
        res.writeHead(200, { "content-type": "text/event-stream" });
        streamClients.add(res);
        res.on("close", () => {
          streamClients.delete(res);
        });
        if (Object.keys(timelineStreamFramesDuringSendByIdentity).length > 0 || options.holdTimelineStream === true) {
          res.write([
            "id: timeline-ready-1",
            "event: keep-alive",
            'data: {"frame_version":1,"id":"timeline-ready-1","kind":"keep-alive","timestamp_ms":1,"payload":{}}',
            "",
            "",
          ].join("\n"));
          return;
        }
        if (timelineStreamFrames.length === 0) {
          res.end([
            "id: timeline-empty-1",
            "event: keep-alive",
            'data: {"frame_version":1,"id":"timeline-empty-1","kind":"keep-alive","timestamp_ms":1,"payload":{}}',
            "",
          ].join("\n"));
          return;
        }
        let index = 0;
        const writeNext = () => {
          if (index >= timelineStreamFrames.length) {
            res.end();
            return;
          }
          const frame = timelineStreamFrames[index];
          index += 1;
          writeTimelineStreamFrame(res, frame, index);
          setTimeout(writeNext, frame.delay_ms || 50);
        };
        setTimeout(writeNext, options.timelineStreamInitialDelayMs || 150);
        return;
      }
      if (method === "POST" && url === "/interactions/stream") {
        res.writeHead(200, { "content-type": "text/event-stream" });
        res.end([
          "id: evt-legacy-1",
          "event: interaction_complete",
          'data: {"session_id":"sess-legacy-1","text":"legacy done"}',
          "",
        ].join("\n"));
        return;
      }

      res.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
      res.end(`not found: ${method} ${url}`);
    });
  });

  return new Promise((resolve, reject) => {
    server.on("error", reject);
    server.listen(0, "127.0.0.1", () => {
      resolve({
        baseUrl: `http://127.0.0.1:${server.address().port}`,
        requests,
        // Writes frames to every open timeline stream; returns how many
        // streams received them.
        emitTimelineStreamFrames: (frames, label = "pushed") => {
          let index = 0;
          for (const res of streamClients) {
            for (const frame of frames) {
              index += 1;
              writeTimelineStreamFrame(res, frame, `${label}-${index}`);
            }
          }
          return streamClients.size;
        },
        // Writes the owner's access_changed signal, as the timeline stream
        // sends it after a committed access change, to every open timeline
        // stream; returns how many streams received it.
        emitAccessChanged: (revision) => {
          for (const res of streamClients) {
            res.write(`event: access_changed\ndata: ${JSON.stringify({ type: "access_changed", revision })}\n\n`);
          }
          return streamClients.size;
        },
        close: () => new Promise((done, closeError) => {
          server.close((error) => error ? closeError(error) : done());
        }),
      });
    });
  });
}

async function runReferenceBrowserProof() {
  const observedRequests = [];

  const backendSpec = exampleBackendSpec(repoRoot, "library_mode_reference");
  const backend = spawn(
    backendSpec.command,
    backendSpec.args,
    {
      cwd: repoRoot,
      env: { ...process.env, META_REF_ADDR: LOOPBACK_ANY_PORT },
      stdio: ["ignore", "pipe", "pipe"],
    }
  );

  backend.stderr.on("data", (chunk) => {
    process.stderr.write(chunk);
  });

  let browser;
  let baseUrl;
  try {
    ({ baseUrl } = await awaitFixtureReady(backend, {
      label: "reference app",
      timeoutMs: backendSpec.prebuilt ? 60_000 : 300_000,
    }));
    await waitForHttpOk(`${baseUrl}/healthz`);
    await waitForHttpOk(`${baseUrl}/console`);

    browser = await launchBrowser();
    const page = await browser.newPage();
    page.on("request", (request) => {
      observedRequests.push({
        method: request.method(),
        url: request.url(),
        postData: request.postData() || "",
      });
    });
    await gotoConsole(page, `${baseUrl}/console`);

    // Wait for sidebar agent rows to appear
    await page.waitForSelector('.agent[role="button"], .cc-sidebar-row', { timeout: 30_000 });
    const labels = await sidebarLabels(page);
    assert(
      labels.some((label) => label.toLowerCase().includes("router")),
      `sidebar missing router: ${JSON.stringify(labels)}`
    );
    assert(
      labels.some((label) => label.toLowerCase().includes("delivery")),
      `sidebar missing delivery: ${JSON.stringify(labels)}`
    );

    // Verify dock and conversation pane rendered
    await page.waitForSelector(".pane, .cc-conversation-pane", { timeout: 10_000 });

    // Verify activity rail rendered
    await page.waitForSelector('[data-testid="signals-rail"], .cc-activity-rail', { timeout: 10_000 });

    // Send a message via the composer
    await openSidebarAgentChat(page, /router/i);
    await fillComposer(page, "browser proof message");
    const acceptance = page.waitForResponse(response => response.url() === `${baseUrl}/console/rpc`
      && response.request().postData()?.includes('"method":"meta/console/send"'));
    await clickSend(page);
    const acceptedResponse = await acceptance;
    const accepted = await acceptedResponse.json();
    assert.equal(acceptedResponse.status(), 200);
    assert(accepted.result?.input_frame_id, `send was not accepted: ${JSON.stringify(accepted)}`);
    await require("./acceptance-runtime.cjs").eventually(async () => {
      const timeline = await (await fetch(`${baseUrl}/console/timeline?identity=router%3Amain&mode=recent`)).json();
      return timeline.frames?.some(frame => frame.id === accepted.result.input_frame_id);
    }, "reference accepted input in timeline");
    await page.waitForFunction(() => document.querySelector(".conv__body, .cc-conversation-scroll")?.textContent?.includes("browser proof message"));
    try {
      await page.getByTestId("chat-pane:router:main").locator('[data-quote-source="ok"]').first().waitFor({ timeout: 15_000 });
    } catch (error) {
      const evidenceDir = path.join(repoRoot, "output/playwright/console-acceptance");
      fs.mkdirSync(evidenceDir, { recursive: true });
      await page.screenshot({ path: path.join(evidenceDir, "reference-response-failure.png"), fullPage: true });
      fs.writeFileSync(path.join(evidenceDir, "reference-response-failure.html"), await page.content());
      fs.writeFileSync(path.join(evidenceDir, "reference-response-failure.json"), JSON.stringify({ accepted, observedRequests,
        timeline: await (await fetch(`${baseUrl}/console/timeline?identity=router%3Amain&mode=recent&limit=500`)).json() }, null, 2));
      throw error;
    }
    await page.reload();
    await openSidebarAgentChat(page, /router/i);
    await page.waitForFunction(() => document.querySelector(".conv__body, .cc-conversation-scroll")?.textContent?.includes("browser proof message"));

    const usesConsoleSendLane = observedRequests.some(
      (request) =>
        request.url === `${baseUrl}/console/rpc` &&
        request.postData.includes('"method":"meta/console/send"'),
    ) && observedRequests.some(
      (request) =>
        request.method === "GET" && request.url.startsWith(`${baseUrl}/console/timeline/stream`),
    );

    assert(
      usesConsoleSendLane,
      `expected browser flow to use canonical console send/timeline lane; saw ${JSON.stringify(observedRequests, null, 2)}`,
    );

    process.stdout.write("browser e2e ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await stopBackend(backend);
  }
}

async function runCanonicalSendBrowserProof() {
  const server = await startMockConsoleServer();
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);

    await page.waitForSelector('.agent[role="button"], .cc-sidebar-row', { timeout: 30_000 });
    const labels = await sidebarLabels(page);
    assert(
      labels.some((label) => label.includes("Identity Example")),
      `mock sidebar missing identity target: ${JSON.stringify(labels)}`,
    );
    assert(
      labels.some((label) => label.includes("Legacy Router")),
      `mock sidebar missing legacy target: ${JSON.stringify(labels)}`,
    );

    await openSidebarAgentChat(page, /Identity Example/i);
    await fillComposer(page, "identity proof message");
    // A normal send is accepted well inside the pending stack's acceptance
    // grace period, so its row (and the "Sending" notice) must
    // never render, not even for a frame. Sampling the DOM after the fact
    // would miss a flash; an observer installed before the click records it.
    await page.evaluate(() => {
      const seen = { notice: false, row: false };
      const record = () => {
        if (document.querySelector(".needs-acceptance")) seen.notice = true;
        if (document.querySelector('[data-testid="pending-stack"]')) seen.row = true;
        if (document.querySelector('[data-testid^="pending-item:"]')) seen.notice = true;
      };
      const observer = new MutationObserver(record);
      observer.observe(document.body, { childList: true, subtree: true, characterData: true, attributes: true });
      window.__pendingFlashProbe = { seen, stop: () => observer.disconnect() };
    });
    await clickSend(page);
    await page.waitForTimeout(2_500);
    const flash = await page.evaluate(() => {
      window.__pendingFlashProbe.stop();
      return window.__pendingFlashProbe.seen;
    });
    assert(
      !flash.notice && !flash.row,
      `a normal send must not flash the pending stack or its acceptance notice: ${JSON.stringify(flash)}`,
    );

    await openSidebarAgentChat(page, /Legacy Router/i);
    await fillComposer(page, "legacy proof message");
    // Sending first awaits persistence and a fresh capability response.
    // Observe this target's actual RPC response before reading the server log.
    const memberSendResponse = page.waitForResponse((response) => {
      const request = response.request();
      if (response.url() !== `${server.baseUrl}/console/rpc` || request.method() !== "POST") return false;
      const payload = request.postDataJSON();
      return payload?.method === "meta/console/send"
        && payload.params?.identity === "legacy-router"
        && payload.params?.content === "legacy proof message";
    }, { timeout: 30_000 });
    await page.getByTestId("chat-send:legacy-router").click();
    await memberSendResponse;

    const sawIdentityLane = server.requests.some(
      (request) =>
        request.url === "/console/rpc" &&
        request.body.includes('"method":"meta/console/send"') &&
        request.body.includes('"identity":"identity:luka"'),
    );
    const sawMemberLane = server.requests.some(
      (request) =>
        request.url === "/console/rpc" &&
        request.body.includes('"method":"meta/console/send"') &&
        request.body.includes('"identity":"legacy-router"'),
    );

    assert(
      sawIdentityLane,
      `expected mixed migration proof to use identity-native lane for the identity target; saw ${JSON.stringify(server.requests, null, 2)}`,
    );
    assert(
      sawMemberLane,
      `expected browser proof to use canonical console send lane for the member target; saw ${JSON.stringify(server.requests, null, 2)}`,
    );

    process.stdout.write("browser canonical send ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runImageRenderingBrowserProof() {
  const baseTs = Date.parse("2026-05-12T05:45:00.000Z");
  const server = await startMockConsoleServer({
    includeImageAgent: true,
    timelineFrames: [
      {
        id: "img-user",
        kind: "user_input",
        identity: "image-agent",
        timestamp_ms: baseTs,
        cursor: "console:image:1",
        payload: {
          content: [
            { type: "text", text: "Operator attached image:" },
            {
              type: "image_ref",
              source: "blob",
              blob_id: "sha256:user-image",
              media_type: "image/png",
              alt: "operator forwarded image",
            },
          ],
        },
      },
      {
        id: "img-generated",
        kind: "assistant_image",
        identity: "image-agent",
        timestamp_ms: baseTs + 1000,
        cursor: "console:image:2",
        payload: {
          blob_id: "sha256:generated-image",
          media_type: "image/png",
          width: 1,
          height: 1,
        },
      },
      {
        id: "img-peer",
        kind: "interaction_complete",
        identity: "image-agent",
        timestamp_ms: baseTs + 2000,
        cursor: "console:image:3",
        source: { kind: "session_history" },
        payload: {
          message: {
            role: "system_notice",
            blocks: [{
              type: "comms",
              kind: "message",
              peer: { id: "incident-command-center/scribe/scribe", display_name: "incident-command-center/scribe/scribe" },
              request_id: "peer-img-1",
              content: [
                { type: "text", text: "Forwarded generated image." },
                {
                  type: "image_ref",
                  source: "blob",
                  blob_id: "sha256:peer-image",
                  media_type: "image/png",
                  alt: "peer forwarded generated image",
                },
              ],
            }],
          },
        },
      },
    ],
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
    await gotoConsole(page, `${server.baseUrl}/console`);

    await page.waitForSelector('.agent[role="button"], .cc-sidebar-row', { timeout: 30_000 });
    await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: "Image Agent" }).click();
    await page.waitForSelector("img.cc-rich-image", { timeout: 30_000 });
    await page.waitForSelector(".cc-tool-call img.cc-tool-call__image", { timeout: 30_000 });

    const richImageSources = await page.$$eval(
      "img.cc-rich-image",
      (images) => images.map((image) => image.getAttribute("src") || ""),
    );
    assert.equal(richImageSources.length, 2);
    assert(
      richImageSources.some((src) => src.endsWith("/blobs/sha256%3Auser-image")),
      `missing user image ref: ${JSON.stringify(richImageSources)}`,
    );
    assert(
      richImageSources.some((src) => src.endsWith("/blobs/sha256%3Agenerated-image")),
      `missing generated assistant image: ${JSON.stringify(richImageSources)}`,
    );

    const peerImageSources = await page.$$eval(
      ".cc-tool-call img.cc-tool-call__image",
      (images) => images.map((image) => image.getAttribute("src") || ""),
    );
    assert.equal(peerImageSources.length, 1);
    // A tool card image is read through the console transport's readBlob and
    // shown from a local object URL, so its src is a blob: URL; the mock must
    // have served exactly the peer's blob.
    assert(
      peerImageSources[0]?.startsWith("blob:"),
      `peer forwarded image inside tool call should be a local object URL: ${JSON.stringify(peerImageSources)}`,
    );
    assert(
      server.requests.some((request) => request.method === "GET" && request.url.split("?")[0] === "/blobs/sha256%3Apeer-image"),
      `missing peer forwarded image blob read: ${JSON.stringify(server.requests.map((request) => request.url).filter((url) => url.startsWith("/blobs/")))}`,
    );
    const peerImagePlacement = await page.evaluate(() => {
      const image = document.querySelector(".cc-tool-call img.cc-tool-call__image");
      const toolCall = image?.closest(".cc-tool-call");
      return {
        hasImage: Boolean(image),
        hasToolCall: Boolean(toolCall),
        incoming: Boolean(toolCall?.classList.contains("cc-tool-call--incoming")),
        displayLabel: toolCall?.querySelector(".cc-tool-call__name")?.textContent,
        canonicalIdentity: toolCall?.querySelector(".cc-tool-call__name")?.getAttribute("title"),
        body: toolCall?.querySelector(".cc-tool-call__peer-body")?.textContent,
        standalonePeerImageCount: Array.from(document.querySelectorAll('img[src$="/blobs/sha256%3Apeer-image"]'))
          .filter((candidate) => !candidate.closest(".cc-tool-call")).length,
      };
    });
    const evidenceDir = path.join(repoRoot, "output/playwright/console-acceptance");
    fs.mkdirSync(evidenceDir, { recursive: true });
    await page.screenshot({ path: path.join(evidenceDir, "image-rendering.png"), fullPage: true });
    assert.deepEqual(
      peerImagePlacement,
      {
        hasImage: true,
        hasToolCall: true,
        incoming: true,
        displayLabel: "Received from scribe",
        canonicalIdentity: "incident-command-center/scribe/scribe",
        body: "Forwarded generated image.",
        standalonePeerImageCount: 0,
      },
      "peer image should remain inside its incoming message with friendly display label and exact canonical identity",
    );

    const bodyText = await page.locator("body").innerText();
    assert(bodyText.includes("Operator attached image:"), "missing user image prompt text");
    assert(bodyText.includes("Forwarded generated image."), "missing peer image comms text");
    assert(!bodyText.includes("image_ref"), "raw image_ref leaked into visible transcript");
    assert(!bodyText.includes("blob_id"), "raw blob_id leaked into visible transcript");

    process.stdout.write("browser image rendering ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runComposerPasteAttachmentProof() {
  const server = await startMockConsoleServer({ includeImageAgent: true });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);

    await page.waitForSelector('.agent[role="button"], .cc-sidebar-row', { timeout: 30_000 });
    await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: "Image Agent" }).click();
    await page.waitForSelector('[data-testid="chat-composer:image-agent"]', { timeout: 30_000 });

    await page.evaluate(() => {
      const textarea = document.querySelector('[data-testid="chat-composer:image-agent"]');
      if (!textarea) throw new Error("missing image-agent composer");
      const bytes = Uint8Array.from([137, 80, 78, 71, 13, 10, 26, 10]);
      const file = new File([bytes], "pasted-badge.png", { type: "image/png" });
      const data = new DataTransfer();
      data.items.add(file);
      textarea.dispatchEvent(new ClipboardEvent("paste", {
        bubbles: true,
        cancelable: true,
        clipboardData: data,
      }));
    });

    await page.waitForSelector(".composer__attachment img", { timeout: 10_000 });
    const attachmentCount = await page.locator(".composer__attachment img").count();
    assert.equal(attachmentCount, 1);
    await fillComposer(page, "Describe the pasted badge.");
    await clickSend(page);
    await page.waitForTimeout(500);

    const multipartRequest = server.requests.find((request) => request.url === "/console/rpc/multipart");
    assert(multipartRequest, `expected image paste send to use multipart RPC; saw ${JSON.stringify(server.requests, null, 2)}`);
    assert(
      multipartRequest.body.includes("image_upload") && multipartRequest.body.includes("pasted-badge.png"),
      `multipart request missing image upload metadata: ${multipartRequest.body}`,
    );

    process.stdout.write("browser composer image paste ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runBusyWorkerConsoleProof() {
  const baseTs = Date.parse("2026-05-23T20:29:50.000Z");
  const server = await startMockConsoleServer({
    includeBusyWorker: true,
    timelineFramesByIdentity: {
      "person-worker-alpha": [
        {
          id: "worker-tool-first-in-cursor-order",
          kind: "tool_call_requested",
          identity: "person-worker-alpha",
          timestamp_ms: baseTs + 1000,
          cursor: "console:worker:10",
          payload: {
            id: "call-index-search-1",
            name: "index_search",
            arguments: { query: "Jordan Example" },
          },
        },
        {
          id: "worker-run-started-backfilled-late",
          kind: "run_started",
          identity: "person-worker-alpha",
          timestamp_ms: baseTs,
          cursor: "console:worker:11",
          payload: {
            prompt: "Parent audit request: inspect Jordan Example for CTO Integration.",
          },
        },
        {
          id: "worker-tool-done-no-terminal-yet",
          kind: "tool_execution_completed",
          identity: "person-worker-alpha",
          timestamp_ms: baseTs + 2000,
          cursor: "console:worker:12",
          payload: {
            id: "call-index-search-1",
            name: "index_search",
            result: "ok",
          },
        },
      ],
    },
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);

    await page.waitForSelector('.agent[role="button"], .cc-sidebar-row', { timeout: 30_000 });
    await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: "Person Worker Alpha" }).click();
    await page.waitForSelector('[data-testid="chat-pane:person-worker-alpha"]', { timeout: 30_000 });

    const pane = page.locator('[data-testid="chat-pane:person-worker-alpha"]');
    await pane.getByText("Parent audit request: inspect Jordan Example for CTO Integration.").waitFor({ timeout: 10_000 });
    await pane.getByText("index_search").first().waitFor({ timeout: 10_000 });
    await page.waitForSelector('[data-testid="chat-typing:person-worker-alpha"]', { timeout: 10_000 });

    const bodyText = await pane.innerText();
    const promptIndex = bodyText.indexOf("Parent audit request: inspect Jordan Example for CTO Integration.");
    const toolIndex = bodyText.indexOf("index_search");
    assert(promptIndex >= 0, `missing parent prompt in worker transcript: ${bodyText}`);
    assert(toolIndex >= 0, `missing tool card in worker transcript: ${bodyText}`);
    assert(
      promptIndex < toolIndex,
      `parent prompt must render before tool activity even when backfilled late: ${bodyText}`,
    );
    assert(bodyText.includes("working"), `busy worker must show working indicator: ${bodyText}`);

    await page.locator('[data-testid="chat-composer:person-worker-alpha"]').fill("Please prioritize this audit note.");
    await page.locator('[data-testid="chat-send:person-worker-alpha"]').click();
    await page.waitForSelector('[data-testid="pending-stack"]', { timeout: 10_000 });
    await pane.getByText("Please prioritize this audit note.").waitFor({ timeout: 10_000 });
    await page.waitForSelector('[data-testid^="pending-steer:"]', { timeout: 10_000 });
    await page.locator('[data-testid^="pending-steer:"]').first().click();
    await page.waitForTimeout(500);

    const steerRequest = server.requests.find((request) =>
      request.url === "/console/rpc" &&
      request.body.includes('"method":"meta/console/send"') &&
      request.body.includes('"identity":"person-worker-alpha"') &&
      request.body.includes('"handling_mode":"steer"') &&
      request.body.includes("Please prioritize this audit note.")
    );
    assert(
      steerRequest,
      `queued busy-worker draft was not promoted through canonical steer send; saw ${JSON.stringify(server.requests, null, 2)}`,
    );

    process.stdout.write("browser busy worker queue/steer ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runToolOnlyWorkerBusyQueueProof() {
  const baseTs = Date.parse("2026-05-23T20:50:00.000Z");
  const queuedText = "Queue me while the tool-only worker is busy.";
  const server = await startMockConsoleServer({
    includeToolOnlyWorker: true,
    timelineFramesByIdentity: {
      "tool-only-worker": [
        {
          id: "tool-only-parent-handoff",
          kind: "user_input",
          identity: "tool-only-worker",
          timestamp_ms: baseTs,
          cursor: "console:tool-only:1",
          payload: {
            content: "Parent handoff: investigate Daily Comet build with Bazel.",
          },
        },
        {
          id: "tool-only-tool-call",
          kind: "tool_call_requested",
          identity: "tool-only-worker",
          timestamp_ms: baseTs + 1_000,
          cursor: "console:tool-only:2",
          payload: {
            id: "call-index-search-1",
            name: "index_search",
            arguments: { query: "Daily Comet build with Bazel" },
          },
        },
        {
          id: "tool-only-tool-done",
          kind: "tool_execution_completed",
          identity: "tool-only-worker",
          timestamp_ms: baseTs + 2_000,
          cursor: "console:tool-only:3",
          payload: {
            id: "call-index-search-1",
            name: "index_search",
            result: "ok",
          },
        },
        {
          id: "tool-only-reasoning",
          kind: "reasoning_delta",
          identity: "tool-only-worker",
          timestamp_ms: baseTs + 3_000,
          cursor: "console:tool-only:4",
          payload: {
            delta: "Reviewing search results before the next tool call.",
          },
        },
      ],
    },
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);

    await page.waitForSelector('.agent[role="button"], .cc-sidebar-row', { timeout: 30_000 });
    await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: "Tool Only Worker" }).click();
    await page.waitForSelector('[data-testid="chat-pane:tool-only-worker"]', { timeout: 30_000 });

    const pane = page.locator('[data-testid="chat-pane:tool-only-worker"]');
    await pane.getByText("Parent handoff: investigate Daily Comet build with Bazel.").waitFor({ timeout: 10_000 });
    await pane.getByText("index_search").first().waitFor({ timeout: 10_000 });
    await page.waitForSelector('[data-testid="chat-typing:tool-only-worker"]', { timeout: 10_000 });

    const beforeSendText = await pane.innerText();
    assert(
      beforeSendText.includes("working"),
      `tool-only active turn must show working indicator without run_started/response_phase: ${beforeSendText}`,
    );

    await page.locator('[data-testid="chat-composer:tool-only-worker"]').fill(queuedText);
    await page.locator('[data-testid="chat-send:tool-only-worker"]').click();
    await page.waitForSelector('[data-testid="pending-stack"]', { timeout: 10_000 });
    await page.waitForSelector('[data-testid^="pending-steer:"]', { timeout: 10_000 });
    await pane.getByText(queuedText).waitFor({ timeout: 10_000 });

    const directSend = server.requests.find((request) =>
      request.url === "/console/rpc" &&
      request.body.includes('"method":"meta/console/send"') &&
      request.body.includes('"identity":"tool-only-worker"') &&
      request.body.includes(queuedText)
    );
    assert(
      !directSend,
      `busy tool-only worker accepted draft immediately instead of queueing it: ${JSON.stringify(server.requests, null, 2)}`,
    );

    await page.locator('[data-testid^="pending-steer:"]').first().click();
    await page.waitForTimeout(500);

    const steerRequest = server.requests.find((request) =>
      request.url === "/console/rpc" &&
      request.body.includes('"method":"meta/console/send"') &&
      request.body.includes('"identity":"tool-only-worker"') &&
      request.body.includes('"handling_mode":"steer"') &&
      request.body.includes(queuedText)
    );
    assert(
      steerRequest,
      `queued tool-only worker draft was not promoted through canonical steer send; saw ${JSON.stringify(server.requests, null, 2)}`,
    );

    process.stdout.write("browser tool-only worker busy queue ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runToolOnlyWorkerTerminalClearsBusyProof() {
  const baseTs = Date.parse("2026-05-23T20:55:00.000Z");
  const sendText = "This should send immediately after terminal turn_completed.";
  const server = await startMockConsoleServer({
    includeToolOnlyWorker: true,
    timelineFramesByIdentity: {
      "tool-only-worker": [
        {
          id: "terminal-tool-only-parent-handoff",
          kind: "user_input",
          identity: "tool-only-worker",
          timestamp_ms: baseTs,
          cursor: "console:tool-only-terminal:1",
          payload: {
            content: "Parent handoff: run one short terminal tool-only check.",
          },
        },
        {
          id: "terminal-tool-only-tool-call",
          kind: "tool_call_requested",
          identity: "tool-only-worker",
          timestamp_ms: baseTs + 1_000,
          cursor: "console:tool-only-terminal:2",
          payload: {
            id: "call-index-search-2",
            name: "index_search",
            arguments: { query: "terminal tool-only check" },
          },
        },
        {
          id: "terminal-tool-only-tool-done",
          kind: "tool_execution_completed",
          identity: "tool-only-worker",
          timestamp_ms: baseTs + 2_000,
          cursor: "console:tool-only-terminal:3",
          payload: {
            id: "call-index-search-2",
            name: "index_search",
            result: "ok",
          },
        },
        {
          id: "terminal-tool-only-turn-completed",
          kind: "turn_completed",
          identity: "tool-only-worker",
          timestamp_ms: baseTs + 3_000,
          cursor: "console:tool-only-terminal:4",
          payload: {
            stop_reason: "end_turn",
          },
        },
      ],
    },
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);

    await page.waitForSelector('.agent[role="button"], .cc-sidebar-row', { timeout: 30_000 });
    await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: "Tool Only Worker" }).click();
    await page.waitForSelector('[data-testid="chat-pane:tool-only-worker"]', { timeout: 30_000 });

    const pane = page.locator('[data-testid="chat-pane:tool-only-worker"]');
    await pane.getByText("Parent handoff: run one short terminal tool-only check.").waitFor({ timeout: 10_000 });
    await pane.getByText("index_search").first().waitFor({ timeout: 10_000 });
    await page.waitForTimeout(500);

    const terminalText = await pane.innerText();
    assert(
      !terminalText.includes("working"),
      `terminal tool-only turn must clear visible working indicator: ${terminalText}`,
    );
    assert.equal(
      await page.locator('[data-testid="chat-typing:tool-only-worker"]').count(),
      0,
      "terminal turn_completed must remove typing indicator",
    );

    await page.locator('[data-testid="chat-composer:tool-only-worker"]').fill(sendText);
    await page.locator('[data-testid="chat-send:tool-only-worker"]').click();
    await page.waitForTimeout(500);

    assert.equal(
      await page.locator('[data-testid="pending-stack"]').count(),
      0,
      "terminal turn_completed must clear hidden busy state so drafts do not queue",
    );

    const directSend = server.requests.find((request) =>
      request.url === "/console/rpc" &&
      request.body.includes('"method":"meta/console/send"') &&
      request.body.includes('"identity":"tool-only-worker"') &&
      request.body.includes(sendText)
    );
    assert(
      directSend,
      `idle terminal tool-only worker did not send directly; saw ${JSON.stringify(server.requests, null, 2)}`,
    );
    assert(
      !directSend.body.includes('"handling_mode":"steer"'),
      `idle terminal tool-only worker incorrectly sent as steer: ${directSend.body}`,
    );

    process.stdout.write("browser tool-only terminal clears busy ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runNonCommsSystemNoticeDoesNotClearBusyProof() {
  const baseTs = Date.parse("2026-05-23T20:57:00.000Z");
  const queuedText = "Queue me after runtime metadata while the worker is still busy.";
  const server = await startMockConsoleServer({
    includeToolOnlyWorker: true,
    timelineFramesByIdentity: {
      "tool-only-worker": [
        {
          id: "notice-tool-only-parent-handoff",
          kind: "user_input",
          identity: "tool-only-worker",
          timestamp_ms: baseTs,
          cursor: "console:tool-only-notice:1",
          payload: {
            content: "Parent handoff: keep running after metadata notice.",
          },
        },
        {
          id: "notice-tool-only-tool-call",
          kind: "tool_call_requested",
          identity: "tool-only-worker",
          timestamp_ms: baseTs + 1_000,
          cursor: "console:tool-only-notice:2",
          payload: {
            id: "call-index-search-3",
            name: "index_search",
            arguments: { query: "runtime metadata notice check" },
          },
        },
        {
          id: "notice-tool-only-tool-done",
          kind: "tool_execution_completed",
          identity: "tool-only-worker",
          timestamp_ms: baseTs + 2_000,
          cursor: "console:tool-only-notice:3",
          payload: {
            id: "call-index-search-3",
            name: "index_search",
            result: "ok",
          },
        },
      ],
    },
    timelineStreamFrames: [
      {
        id: "notice-tool-only-runtime-notice",
        kind: "system_notice",
        identity: "tool-only-worker",
        timestamp_ms: baseTs + 3_000,
        cursor: "console:tool-only-notice:4",
        payload: {
          message: {
            role: "system_notice",
            kind: "generic",
            body: "Received from runtime control plane: recovered from transient stream lag",
            blocks: [{
              type: "runtime_notice",
              category: "stream",
              detail: "Received from runtime control plane: recovered from transient stream lag",
            }],
          },
        },
      },
    ],
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);

    await page.waitForSelector('.agent[role="button"], .cc-sidebar-row', { timeout: 30_000 });
    await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: "Tool Only Worker" }).click();
    await page.waitForSelector('[data-testid="chat-pane:tool-only-worker"]', { timeout: 30_000 });

    const pane = page.locator('[data-testid="chat-pane:tool-only-worker"]');
    await pane.getByText("Parent handoff: keep running after metadata notice.").waitFor({ timeout: 10_000 });
    await pane.getByText("index_search").first().waitFor({ timeout: 10_000 });
    await pane.getByText("Received from runtime control plane").waitFor({ timeout: 10_000 });
    await page.waitForSelector('[data-testid="chat-typing:tool-only-worker"]', { timeout: 10_000 });

    const noticeText = await pane.innerText();
    assert(
      noticeText.includes("working"),
      `non-comms system_notice must not clear busy state during an active turn: ${noticeText}`,
    );

    await page.locator('[data-testid="chat-composer:tool-only-worker"]').fill(queuedText);
    await page.locator('[data-testid="chat-send:tool-only-worker"]').click();
    await page.waitForSelector('[data-testid="pending-stack"]', { timeout: 10_000 });
    await page.waitForSelector('[data-testid^="pending-steer:"]', { timeout: 10_000 });

    const directSend = server.requests.find((request) =>
      request.url === "/console/rpc" &&
      request.body.includes('"method":"meta/console/send"') &&
      request.body.includes('"identity":"tool-only-worker"') &&
      request.body.includes(queuedText)
    );
    assert(
      !directSend,
      `non-comms system_notice cleared hidden busy state and sent immediately: ${JSON.stringify(server.requests, null, 2)}`,
    );

    process.stdout.write("browser non-comms system_notice stays busy ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runSidebarSearchExpandsCollapsedWorkerSectionProof() {
  const server = await startMockConsoleServer({
    includeToolOnlyWorker: true,
    collapseWorkersSection: true,
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);

    await page.waitForSelector('[data-testid="sidebar-agent-list"]', { timeout: 30_000 });
    await page.getByTestId("sidebar-search").fill("tool-only-worker");
    await page.locator('[data-testid="sidebar-agent:tool-only-worker"]').waitFor({
      timeout: 10_000,
    });
    await page.locator('[data-testid="sidebar-agent:tool-only-worker"]').click();
    await page.waitForSelector('[data-testid="chat-pane:tool-only-worker"]', { timeout: 10_000 });

    process.stdout.write("browser sidebar search expands collapsed worker section ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runChatPaneAutoScrollProof() {
  const baseTs = Date.parse("2026-05-23T20:40:00.000Z");
  const historyFrames = Array.from({ length: 48 }, (_, index) => ({
    id: `history-line-${index}`,
    kind: "interaction_complete",
    identity: "person-worker-alpha",
    interaction_id: `history-turn-${index}`,
    timestamp_ms: baseTs + index * 1_000,
    cursor: `console:auto:${index}`,
    payload: {
      text: `Historical worker line ${index + 1}: enough transcript content to require scrolling.`,
    },
  }));
  const growingFrame = {
    id: "growing-answer",
    kind: "interaction_complete",
    identity: "person-worker-alpha",
    interaction_id: "growing-answer-turn",
    timestamp_ms: baseTs + 55_000,
    cursor: "console:auto:growing",
    payload: {
      text: "Live auto-scroll proof begins.",
    },
  };
  const liveUpdates = [
    {
      id: "live-autoscroll-frame-update",
      kind: "frame_updated",
      identity: "person-worker-alpha",
      timestamp_ms: baseTs + 60_000,
      cursor: "console:auto:live:update",
      payload: {
        frame: {
          ...growingFrame,
          payload: {
            text:
              "Live auto-scroll proof begins. The chat pane should remain stuck to the bottom while this same assistant message grows. AUTO_SCROLL_FINAL_VISIBLE",
          },
        },
      },
    },
  ];
  const server = await startMockConsoleServer({
    includeBusyWorker: true,
    timelineFramesByIdentity: {
      "person-worker-alpha": [...historyFrames, growingFrame],
    },
    timelineStreamFrames: liveUpdates,
    timelineStreamInitialDelayMs: 2_000,
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);

    await page.waitForSelector('.agent[role="button"], .cc-sidebar-row', { timeout: 30_000 });
    await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: "Person Worker Alpha" }).click();
    const pane = page.locator('[data-testid="chat-pane:person-worker-alpha"]');
    await pane.getByText("Historical worker line 48").waitFor({ timeout: 10_000 });
    await pane.getByText("AUTO_SCROLL_FINAL_VISIBLE").waitFor({ timeout: 10_000 });
    const body = pane.locator(".conv__body");
    const sample = () => body.evaluate((node) => ({
      scrollTop: node.scrollTop,
      clientHeight: node.clientHeight,
      scrollHeight: node.scrollHeight,
      text: node.textContent || "",
    }));
    const distanceOf = (state) => state.scrollHeight - state.clientHeight - state.scrollTop;
    // The pane re-pins from its ResizeObserver after the grown message lays
    // out, so the task that sees the new text can still read the scroll
    // position from before that layout. Wait, bounded, for later frames to
    // show the pinned position; an unpinned pane still fails at the deadline.
    const pinnedBy = Date.now() + 5_000;
    let framesWaited = 0;
    let scrollState = await sample();
    while (distanceOf(scrollState) > 4 && Date.now() < pinnedBy) {
      await body.evaluate(() => new Promise((resolve) => {
        requestAnimationFrame(() => requestAnimationFrame(resolve));
        setTimeout(resolve, 100);
      }));
      framesWaited += 1;
      scrollState = await sample();
    }
    const distanceFromBottom = distanceOf(scrollState);
    assert(
      distanceFromBottom <= 4,
      `chat pane did not stay pinned to latest transcript content; distance=${distanceFromBottom} state=${JSON.stringify(scrollState)}`,
    );

    process.stdout.write(`browser chat pane auto-scroll ok (frame waits before pinned: ${framesWaited})\n`);
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runChatPaneTurnRailProof() {
  const baseTs = Date.parse("2026-05-23T21:35:00.000Z");
  const frames = [];
  for (let index = 1; index <= 6; index += 1) {
    frames.push({
      id: `turn-rail-user-${index}`,
      kind: "user_input",
      identity: "person-worker-alpha",
      interaction_id: `turn-rail-${index}`,
      timestamp_ms: baseTs + index * 2_000,
      cursor: `console:turn-rail:${index}:user`,
      payload: {
        content: `Turn rail request ${index}: verify invoice drift and owner follow-up with enough text to create a visible row.`,
      },
    });
    frames.push({
      id: `turn-rail-agent-${index}`,
      kind: "interaction_complete",
      identity: "person-worker-alpha",
      interaction_id: `turn-rail-${index}`,
      timestamp_ms: baseTs + index * 2_000 + 800,
      cursor: `console:turn-rail:${index}:agent`,
      payload: {
        text: `Turn rail answer ${index}: the billing owner should reconcile source totals, confirm due dates, and record the outcome.`,
      },
    });
  }

  const server = await startMockConsoleServer({
    includeBusyWorker: true,
    timelineFramesByIdentity: {
      "person-worker-alpha": frames,
    },
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage({ viewport: { width: 520, height: 540 } });
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.waitForSelector('.agent[role="button"], .cc-sidebar-row', { timeout: 30_000 });
    await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: "Person Worker Alpha" }).click();

    const pane = page.locator('[data-testid="chat-pane:person-worker-alpha"]');
    await pane.getByTestId("chat-turn:person-worker-alpha:5").getByText("Turn rail answer 6").waitFor({ timeout: 10_000 });
    await pane.getByTestId("chat-turn-rail:person-worker-alpha:5").waitFor({ timeout: 10_000 });

    const body = pane.locator(".conv__body");
    const initial = await body.evaluate((node) => ({
      scrollTop: node.scrollTop,
      scrollHeight: node.scrollHeight,
      clientHeight: node.clientHeight,
    }));
    const initialRail = await pane.evaluate((node) => {
      const body = node.querySelector(".conv__body");
      const bodyRect = body?.getBoundingClientRect();
      const rails = [...node.querySelectorAll('[data-testid^="chat-turn-rail:person-worker-alpha:"]')];
      const visibleIndexes = [...node.querySelectorAll("[data-chat-turn-index]")]
        .filter((turnNode) => {
          const rect = turnNode.getBoundingClientRect();
          return bodyRect && rect.bottom >= bodyRect.top && rect.top <= bodyRect.bottom;
        })
        .map((turnNode) => Number(turnNode.dataset.chatTurnIndex));
      const activeIndexes = rails
        .map((rail, index) => rail.getAttribute("aria-current") === "true" ? index : -1)
        .filter((index) => index >= 0);
      return {
        railCount: rails.length,
        activeIndexes,
        visibleIndexes,
      };
    });
    assert.equal(initialRail.railCount, 6, `expected 6 turn rail markers: ${JSON.stringify(initialRail)}`);
    assert.deepEqual(
      initialRail.activeIndexes,
      initialRail.visibleIndexes,
      `initial rail markers should track all visible turns: ${JSON.stringify(initialRail)}`,
    );
    assert(
      initial.scrollHeight > initial.clientHeight,
      `expected overflowing chat body for turn rail proof: ${JSON.stringify(initial)}`,
    );

    const initialLayout = await pane.evaluate((node) => {
      const rail = node.querySelector('[data-testid="chat-turn-rail:person-worker-alpha:0"]');
      const rails = [...node.querySelectorAll('[data-testid^="chat-turn-rail:person-worker-alpha:"]')];
      const railYs = rails.map((marker) => {
        const rect = marker.getBoundingClientRect();
        return Math.round(rect.top + rect.height / 2);
      });
      const buttonSizes = rails.map((marker) => {
        const rect = marker.getBoundingClientRect();
        return {
          width: Math.round(rect.width),
          height: Math.round(rect.height),
        };
      });
      // The transcript mounts only the turns near the viewport, so turn 0 is
      // a spacer here at the live edge; any mounted bubble shows the gutter,
      // and the leftmost is the one nearest the rail.
      const bubbleLefts = [...node.querySelectorAll('[data-testid^="chat-turn:person-worker-alpha:"] .msg__bubble')]
        .map((bubble) => bubble.getBoundingClientRect().left);
      const railRect = rail?.getBoundingClientRect();
      return {
        railRight: railRect?.right ?? 0,
        bubbleLeft: bubbleLefts.length > 0 ? Math.min(...bubbleLefts) : 0,
        markerDeltas: railYs.slice(1).map((y, index) => y - railYs[index]),
        buttonSizes,
      };
    });
    assert(
      initialLayout.bubbleLeft - initialLayout.railRight >= 8,
      `turn rail must stay in its own gutter, not overlap chat content: ${JSON.stringify(initialLayout)}`,
    );
    assert(
      initialLayout.markerDeltas.every((delta) => delta > 0 && delta <= 18),
      `turn rail markers should be tightly spaced: ${JSON.stringify(initialLayout)}`,
    );
    assert(
      initialLayout.buttonSizes.every((size) => size.width <= 18 && size.height <= 18),
      `turn rail buttons should stay visually compact: ${JSON.stringify(initialLayout)}`,
    );

    await body.evaluate((node) => {
      node.scrollTo({ top: Math.round(node.scrollHeight * 0.55), behavior: "instant" });
      node.dispatchEvent(new Event("scroll"));
    });
    await page.waitForTimeout(120);
    const afterMiddleScroll = await pane.evaluate((node) => {
      const body = node.querySelector(".conv__body");
      const bodyRect = body?.getBoundingClientRect();
      const canvas = document.createElement("canvas");
      canvas.width = 1;
      canvas.height = 1;
      const context = canvas.getContext("2d", { willReadFrequently: true });
      const toRgba = (color) => {
        if (!context) {
          return [0, 0, 0, 0];
        }
        context.clearRect(0, 0, 1, 1);
        context.fillStyle = "#0000";
        context.fillStyle = color;
        context.fillRect(0, 0, 1, 1);
        return [...context.getImageData(0, 0, 1, 1).data];
      };
      const colorDistance = (a, b) => Math.hypot(
        a[0] - b[0],
        a[1] - b[1],
        a[2] - b[2],
        a[3] - b[3],
      );
      const bodyColor = toRgba(getComputedStyle(body || node).backgroundColor);
      const visibleIndexes = [...node.querySelectorAll("[data-chat-turn-index]")]
        .filter((turnNode) => {
          const rect = turnNode.getBoundingClientRect();
          return bodyRect && rect.bottom >= bodyRect.top && rect.top <= bodyRect.bottom;
        })
        .map((turnNode) => Number(turnNode.dataset.chatTurnIndex));
      const activeIndexes = [...node.querySelectorAll('[data-testid^="chat-turn-rail:person-worker-alpha:"]')]
        .map((rail, index) => rail.getAttribute("aria-current") === "true" ? index : -1)
        .filter((index) => index >= 0);
      const tickData = [...node.querySelectorAll('[data-testid^="chat-turn-rail:person-worker-alpha:"]')]
        .map((rail, index) => {
          const tick = rail.querySelector(".conv-turn-rail__tick");
          const background = tick ? getComputedStyle(tick).backgroundColor : "";
          return {
            index,
            active: rail.getAttribute("aria-current") === "true",
            background,
            bodyDistance: colorDistance(toRgba(background), bodyColor),
            width: Math.round(tick?.getBoundingClientRect().width ?? 0),
          };
        });
      return {
        activeIndexes,
        tickWidths: tickData.map((tick) => tick.width),
        visibleIndexes,
        activeTickStyles: tickData.filter((tick) => tick.active),
        inactiveTickStyles: tickData.filter((tick) => !tick.active),
      };
    });
    assert.deepEqual(
      afterMiddleScroll.activeIndexes,
      afterMiddleScroll.visibleIndexes,
      `mid-scroll rail markers should highlight every visible turn: ${JSON.stringify(afterMiddleScroll)}`,
    );
    assert.equal(
      new Set(afterMiddleScroll.tickWidths).size,
      1,
      `visible markers should brighten without changing tick width when not hovered: ${JSON.stringify(afterMiddleScroll)}`,
    );
    const weakestActiveDistance = Math.min(...afterMiddleScroll.activeTickStyles.map((tick) => tick.bodyDistance));
    const strongestInactiveDistance = Math.max(...afterMiddleScroll.inactiveTickStyles.map((tick) => tick.bodyDistance));
    assert(
      weakestActiveDistance > strongestInactiveDistance,
      `visible markers should be visually stronger than inactive markers without growing: ${JSON.stringify(afterMiddleScroll)}`,
    );

    await pane.getByTestId("chat-turn-rail:person-worker-alpha:0").click();
    await page.waitForFunction(() => {
      const paneNode = document.querySelector('[data-testid="chat-pane:person-worker-alpha"]');
      const firstRail = paneNode?.querySelector('[data-testid="chat-turn-rail:person-worker-alpha:0"]');
      return firstRail?.getAttribute("aria-current") === "true";
    }, null, { timeout: 3_000 });
    await page.mouse.move(500, 500);

    const afterClick = await pane.evaluate((node) => {
      const body = node.querySelector(".conv__body");
      const rails = [...node.querySelectorAll('[data-testid^="chat-turn-rail:person-worker-alpha:"]')];
      const visiblePreview = [...node.querySelectorAll(".conv-turn-preview")]
        .find((preview) => getComputedStyle(preview).opacity === "1");
      return {
        scrollTop: body?.scrollTop ?? 0,
        activeIndexes: rails
          .map((rail, index) => rail.getAttribute("aria-current") === "true" ? index : -1)
          .filter((index) => index >= 0),
        focusedRail: document.activeElement?.getAttribute("data-testid") || "",
        firstTurnTop: node.querySelector('[data-testid="chat-turn:person-worker-alpha:0"]')?.getBoundingClientRect().top,
        previewTitle: visiblePreview?.querySelector(".conv-turn-preview__title")?.textContent || "",
        firstRailLabel: rails[0]?.getAttribute("aria-label") || "",
      };
    });

    assert(
      afterClick.scrollTop < initial.scrollTop,
      `clicking first turn marker should move the scroll position upward; initial=${JSON.stringify(initial)} after=${JSON.stringify(afterClick)}`,
    );
    assert(
      afterClick.activeIndexes.includes(0),
      `first turn marker should become active after clicking it: ${JSON.stringify(afterClick)}`,
    );
    assert.equal(afterClick.focusedRail, "", `mouse click should not leave a rail marker focused: ${JSON.stringify(afterClick)}`);
    assert.equal(afterClick.previewTitle, "", `mouse click should not leave the preview stuck open: ${JSON.stringify(afterClick)}`);
    assert.match(afterClick.firstRailLabel, /Turn rail request 1/);

    await body.evaluate((node) => {
      node.scrollTo({ top: node.scrollHeight, behavior: "instant" });
      node.dispatchEvent(new Event("scroll"));
    });
    await page.waitForFunction(() => {
      const paneNode = document.querySelector('[data-testid="chat-pane:person-worker-alpha"]');
      const rails = [...(paneNode?.querySelectorAll('[data-testid^="chat-turn-rail:person-worker-alpha:"]') ?? [])];
      return rails[5]?.getAttribute("aria-current") === "true";
    }, null, { timeout: 3_000 });
    const afterManualScroll = await pane.evaluate((node) => {
      const body = node.querySelector(".conv__body");
      const bodyRect = body?.getBoundingClientRect();
      const rails = [...node.querySelectorAll('[data-testid^="chat-turn-rail:person-worker-alpha:"]')];
      const visibleIndexes = [...node.querySelectorAll("[data-chat-turn-index]")]
        .filter((turnNode) => {
          const rect = turnNode.getBoundingClientRect();
          return bodyRect && rect.bottom >= bodyRect.top && rect.top <= bodyRect.bottom;
        })
        .map((turnNode) => Number(turnNode.dataset.chatTurnIndex));
      return {
        activeIndexes: rails
          .map((rail, index) => rail.getAttribute("aria-current") === "true" ? index : -1)
          .filter((index) => index >= 0),
        visibleIndexes,
        focusedRail: document.activeElement?.getAttribute("data-testid") || "",
      };
    });
    assert.deepEqual(
      afterManualScroll.activeIndexes,
      afterManualScroll.visibleIndexes,
      `manual scroll should highlight every visible turn marker: ${JSON.stringify(afterManualScroll)}`,
    );
    assert.equal(afterManualScroll.focusedRail, "", `manual scroll should not focus a rail marker: ${JSON.stringify(afterManualScroll)}`);

    await page.setViewportSize({ width: 1280, height: 760 });
    await page.waitForFunction(() => {
      const paneNode = document.querySelector('[data-testid="chat-pane:person-worker-alpha"]');
      return Boolean(paneNode?.querySelector('[data-testid="chat-turn-rail:person-worker-alpha:1"]'));
    }, null, { timeout: 3_000 });
    await pane.getByTestId("chat-turn-rail:person-worker-alpha:1").hover();
    await page.waitForFunction(() => {
      const paneNode = document.querySelector('[data-testid="chat-pane:person-worker-alpha"]');
      return [...(paneNode?.querySelectorAll(".conv-turn-preview") ?? [])]
        .some((preview) => getComputedStyle(preview).opacity === "1");
    }, null, { timeout: 1_000 });
    const hoverPreview = await pane.evaluate((node) => {
      const visiblePreview = [...node.querySelectorAll(".conv-turn-preview")]
        .find((preview) => getComputedStyle(preview).opacity === "1");
      const rect = visiblePreview?.getBoundingClientRect();
      const rails = [...node.querySelectorAll('[data-testid^="chat-turn-rail:person-worker-alpha:"]')];
      return {
        title: visiblePreview?.querySelector(".conv-turn-preview__title")?.textContent || "",
        body: visiblePreview?.querySelector(".conv-turn-preview__body")?.textContent || "",
        width: rect?.width ?? 0,
        height: rect?.height ?? 0,
        waveWidths: rails.slice(0, 4).map((rail) => Math.round(rail.querySelector(".conv-turn-rail__tick")?.getBoundingClientRect().width ?? 0)),
      };
    });
    assert.match(hoverPreview.title, /Turn rail request 2/);
    assert.match(hoverPreview.body, /Turn rail answer 2/);
    assert(
      hoverPreview.width <= 370 && hoverPreview.height <= 96,
      `turn preview should be compact, not a modal-sized slab: ${JSON.stringify(hoverPreview)}`,
    );
    assert.deepEqual(
      hoverPreview.waveWidths.slice(0, 4),
      [11, 13, 11, 9],
      `hover should restore a compact local wave around the hovered marker: ${JSON.stringify(hoverPreview)}`,
    );

    await page.waitForFunction(() => {
      const paneNode = document.querySelector('[data-testid="chat-pane:person-worker-alpha"]');
      return Boolean(paneNode?.querySelector('[data-testid="chat-turn-rail:person-worker-alpha:0"]'));
    }, null, { timeout: 3_000 });
    const desktopLayout = await pane.evaluate((node) => {
      const rails = [...node.querySelectorAll('[data-testid^="chat-turn-rail:person-worker-alpha:"]')];
      const bubbles = [...node.querySelectorAll('[data-testid^="chat-turn:person-worker-alpha:"] .msg__bubble')];
      const maxRailRight = Math.max(...rails.map((rail) => rail.getBoundingClientRect().right));
      const minBubbleLeft = Math.min(...bubbles.map((bubble) => bubble.getBoundingClientRect().left));
      return {
        maxRailRight,
        minBubbleLeft,
        gap: minBubbleLeft - maxRailRight,
      };
    });
    assert(
      desktopLayout.gap >= 8,
      `desktop turn rail must remain in its own gutter: ${JSON.stringify(desktopLayout)}`,
    );

    process.stdout.write("browser chat pane turn rail ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runGlobalTimelineRecentSeedProof() {
  const baseTs = Date.now() - 60_000;
  const oldFrames = Array.from({ length: 1_200 }, (_, index) => ({
    id: `old-global-${index + 1}`,
    kind: "interaction_complete",
    identity: "identity:luka",
    interaction_id: `old-global-turn-${index + 1}`,
    timestamp_ms: baseTs - 1_200_000 + index,
    cursor: `console:${index + 1}`,
    payload: { text: `Old global event ${index + 1}` },
  }));
  const recentFrame = {
    id: "recent-global-event",
    kind: "interaction_complete",
    identity: "identity:luka",
    interaction_id: "recent-global-turn",
    timestamp_ms: baseTs,
    cursor: "console:1201",
    payload: { text: "RECENT_GLOBAL_EVENT_VISIBLE" },
  };
  const server = await startMockConsoleServer({
    timelineFrames: [...oldFrames, recentFrame],
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.waitForSelector('[data-testid="signals-rail"], .cc-activity-rail', { timeout: 10_000 });
    await page.waitForTimeout(500);

    const timelineRequests = server.requests
      .filter((request) => request.url === "/console/rpc" && request.body)
      .map((request) => JSON.parse(request.body))
      .filter((payload) => payload.method === "meta/console/query_timeline");
    const globalSeed = timelineRequests.find((payload) => !payload.params?.identity);
    assert.equal(globalSeed?.params?.mode, "recent");
    assert.equal(globalSeed?.params?.limit, 200);
    assert(
      server.requests.some(
        (request) =>
          request.method === "GET" &&
          request.url === "/console/timeline/stream" &&
          request.headers?.["last-event-id"] === "console:1201",
      ),
      `expected timeline stream to continue after recent tail cursor via Last-Event-ID; requests=${JSON.stringify(server.requests, null, 2)}`,
    );

    process.stdout.write("browser global timeline recent seed ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runChatPaneRecentFirstPageProof() {
  const baseTs = Date.parse("2026-05-23T21:40:00.000Z");
  const frames = Array.from({ length: 1_500 }, (_, index) => ({
    id: `recent-first-line-${index + 1}`,
    kind: "interaction_complete",
    identity: "person-worker-alpha",
    interaction_id: `recent-first-turn-${index + 1}`,
    timestamp_ms: baseTs + index,
    cursor: `console:${index + 1}`,
    payload: {
      text: `Recent-first worker line ${index + 1}`,
    },
  }));
  const server = await startMockConsoleServer({
    includeBusyWorker: true,
    timelineFramesByIdentity: {
      "person-worker-alpha": frames,
    },
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.waitForSelector('.agent[role="button"], .cc-sidebar-row', { timeout: 30_000 });
    await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: "Person Worker Alpha" }).click();
    const pane = page.locator('[data-testid="chat-pane:person-worker-alpha"]');
    await pane.getByText("Recent-first worker line 1500").waitFor({ timeout: 10_000 });
    await assertNoText(page, "Recent-first worker line 42");

    const identityRequests = server.requests
      .filter((request) => request.url === "/console/rpc" && request.body)
      .map((request) => JSON.parse(request.body))
      .filter(
        (payload) =>
          payload.method === "meta/console/query_timeline" &&
          payload.params?.identity === "person-worker-alpha",
      );
    const firstIdentityRequest = identityRequests[0];
    assert.equal(firstIdentityRequest?.params?.mode, "recent");
    assert.equal(firstIdentityRequest?.params?.limit, 200);
    assert.equal(firstIdentityRequest?.params?.after, undefined);

    process.stdout.write("browser chat pane recent first page ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runChatPaneOlderHistoryDemandPagingProof() {
  const baseTs = Date.parse("2026-05-23T22:40:00.000Z");
  const frames = Array.from({ length: 250 }, (_, index) => ({
    id: `older-demand-line-${index + 1}`,
    kind: "interaction_complete",
    identity: "person-worker-alpha",
    interaction_id: `older-demand-turn-${index + 1}`,
    timestamp_ms: baseTs + index,
    cursor: `console:${index + 1}`,
    payload: {
      text: `Older-demand worker line ${index + 1}`,
    },
  }));
  const server = await startMockConsoleServer({
    includeBusyWorker: true,
    timelineFramesByIdentity: {
      "person-worker-alpha": frames,
    },
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.waitForSelector('.agent[role="button"], .cc-sidebar-row', { timeout: 30_000 });
    await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: "Person Worker Alpha" }).click();
    const pane = page.locator('[data-testid="chat-pane:person-worker-alpha"]');
    await pane.getByText("Older-demand worker line 250").waitFor({ timeout: 10_000 });
    await assertNoText(page, "Older-demand worker line 42");
    await clickLoadOlderHistory(pane);
    await pane.getByText("Older-demand worker line 42").waitFor({ timeout: 10_000 });
    const body = pane.locator(".conv__body");
    const scrollState = await body.evaluate((node) => ({
      scrollTop: node.scrollTop,
      clientHeight: node.clientHeight,
      scrollHeight: node.scrollHeight,
    }));
    const distanceFromBottom =
      scrollState.scrollHeight - scrollState.clientHeight - scrollState.scrollTop;
    assert(
      distanceFromBottom > 100,
      `older history load should preserve reader position instead of snapping to bottom; state=${JSON.stringify(scrollState)}`,
    );

    const identityRequests = server.requests
      .filter((request) => request.url === "/console/rpc" && request.body)
      .map((request) => JSON.parse(request.body))
      .filter(
        (payload) =>
          payload.method === "meta/console/query_timeline" &&
          payload.params?.identity === "person-worker-alpha",
      );
    assert(
      identityRequests.some(
        (payload) => payload.params?.mode === "recent" && payload.params?.before === "console:51",
      ),
      `expected demand-paged older request before first visible tail cursor; saw ${JSON.stringify(identityRequests, null, 2)}`,
    );

    process.stdout.write("browser chat pane older history demand paging ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runChatPaneAsyncBackfillRestoresOlderHistoryProof() {
  const baseTs = Date.parse("2026-05-23T22:45:00.000Z");
  const frames = Array.from({ length: 250 }, (_, index) => ({
    id: `async-backfill-line-${index + 1}`,
    kind: "interaction_complete",
    identity: "person-worker-alpha",
    interaction_id: `async-backfill-turn-${index + 1}`,
    timestamp_ms: baseTs + index,
    cursor: `console:${index + 1}`,
    payload: {
      text: `Async-backfill worker line ${index + 1}`,
    },
  }));
  // The runtime appends asynchronously backfilled session history through
  // `append_and_emit`, so the frames reach the unscoped live stream as they
  // land; the console has no periodic poll. The mock emits the backfilled
  // tail on the stream after the pane has taken its provisional empty first
  // page, and the console's terminal-frame reconcile re-queries the page.
  const server = await startMockConsoleServer({
    includeBusyWorker: true,
    timelineFrameSnapshotsByIdentity: {
      "person-worker-alpha": [[], frames],
    },
    timelineStreamFrames: [frames[249]],
    timelineStreamInitialDelayMs: 4_000,
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.waitForSelector('.agent[role="button"], .cc-sidebar-row', { timeout: 30_000 });
    await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: "Person Worker Alpha" }).click();
    const pane = page.locator('[data-testid="chat-pane:person-worker-alpha"]');
    await pane.getByText("Async-backfill worker line 250").waitFor({ timeout: 10_000 });
    await assertNoText(page, "Async-backfill worker line 42");
    await clickLoadOlderHistory(pane);
    await pane.getByText("Async-backfill worker line 42").waitFor({ timeout: 10_000 });

    const identityRequests = server.requests
      .filter((request) => request.url === "/console/rpc" && request.body)
      .map((request) => JSON.parse(request.body))
      .filter(
        (payload) =>
          payload.method === "meta/console/query_timeline" &&
          payload.params?.identity === "person-worker-alpha",
      );
    assert(
      identityRequests.some((payload) => payload.params?.mode === "recent"),
      `expected initial recent query for async backfill proof; saw ${JSON.stringify(identityRequests, null, 2)}`,
    );
    assert(
      identityRequests.some(
        (payload) => payload.params?.mode === "recent" && payload.params?.before === "console:51",
      ),
      `expected older-history request after async backfill re-enabled the control; saw ${JSON.stringify(identityRequests, null, 2)}`,
    );

    process.stdout.write("browser chat pane async backfill older history recovery ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runChatPaneNonEmptyAsyncBackfillRestoresOlderHistoryProof() {
  const baseTs = Date.parse("2026-05-23T22:46:00.000Z");
  const frames = Array.from({ length: 250 }, (_, index) => ({
    id: `nonempty-async-backfill-line-${index + 1}`,
    kind: "interaction_complete",
    identity: "person-worker-alpha",
    interaction_id: `nonempty-async-backfill-turn-${index + 1}`,
    timestamp_ms: baseTs + index,
    cursor: `console:${index + 1}`,
    payload: {
      text: `Non-empty async-backfill worker line ${index + 1}`,
    },
  }));
  // As above: the backfilled history reaches the stream; here the first
  // page was a provisional non-empty exhausted tail. A backfilled frame
  // that the tail already held is deduplicated, so the stream carries the
  // frame just below it to trigger the reconcile.
  const server = await startMockConsoleServer({
    includeBusyWorker: true,
    timelineFrameSnapshotsByIdentity: {
      "person-worker-alpha": [[frames[249]], frames],
    },
    timelineStreamFrames: [frames[248]],
    timelineStreamInitialDelayMs: 4_000,
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.waitForSelector('.agent[role="button"], .cc-sidebar-row', { timeout: 30_000 });
    await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: "Person Worker Alpha" }).click();
    const pane = page.locator('[data-testid="chat-pane:person-worker-alpha"]');
    await pane.getByText("Non-empty async-backfill worker line 250").waitFor({ timeout: 10_000 });
    await assertNoText(page, "Non-empty async-backfill worker line 42");
    await clickLoadOlderHistory(pane);
    await pane.getByText("Non-empty async-backfill worker line 42").waitFor({ timeout: 10_000 });

    const identityRequests = server.requests
      .filter((request) => request.url === "/console/rpc" && request.body)
      .map((request) => JSON.parse(request.body))
      .filter(
        (payload) =>
          payload.method === "meta/console/query_timeline" &&
          payload.params?.identity === "person-worker-alpha",
      );
    const recentRequests = identityRequests.filter(
      (payload) => payload.params?.mode === "recent" && payload.params?.before === undefined,
    );
    assert(
      recentRequests.length >= 2,
      `expected provisional non-empty exhausted tail to refresh with another recent query; saw ${JSON.stringify(identityRequests, null, 2)}`,
    );
    assert(
      identityRequests.some(
        (payload) => payload.params?.mode === "recent" && payload.params?.before === "console:51",
      ),
      `expected older-history request after non-empty async backfill re-enabled the control; saw ${JSON.stringify(identityRequests, null, 2)}`,
    );

    process.stdout.write("browser chat pane non-empty async backfill older history recovery ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runChatPaneReplayRecoveryReplacesStaleLocalLogProof() {
  const oldFrames = Array.from({ length: 250 }, (_, index) => ({
    id: `stale-replay-line-${index + 1}`,
    kind: "interaction_complete",
    identity: "person-worker-alpha",
    interaction_id: `stale-replay-turn-${index + 1}`,
    timestamp_ms: Date.parse("2026-05-23T22:47:00.000Z") + index,
    cursor: `console:${index + 1}`,
    payload: {
      text: `Stale replay worker line ${index + 1}`,
    },
  }));
  const newFrame = {
    id: "fresh-replay-line-400",
    kind: "interaction_complete",
    identity: "person-worker-alpha",
    interaction_id: "fresh-replay-turn-400",
    timestamp_ms: Date.parse("2026-05-23T22:48:00.000Z"),
    cursor: "console:400",
    payload: {
      text: "Fresh replay worker line 400",
    },
  };
  const server = await startMockConsoleServer({
    includeBusyWorker: true,
    timelineFramesByIdentity: {
      "person-worker-alpha": oldFrames,
    },
    timelineReplayUnavailableAfterByIdentity: {
      "person-worker-alpha": "console:250",
    },
    timelineFramesAfterReplayUnavailableByIdentity: {
      "person-worker-alpha": [newFrame],
    },
    // When the runtime's source log resets it appends a synthetic
    // `replay_unavailable` frame and emits it on the stream; the console
    // repairs each docked identity from its own cursor on that signal.
    timelineStreamFrames: [
      {
        id: "replay-reset-1",
        kind: "replay_unavailable",
        identity: "__console__",
        timestamp_ms: Date.parse("2026-05-23T22:47:30.000Z"),
        payload: { reason: "source gap", source_kind: "console_event" },
      },
    ],
    timelineStreamInitialDelayMs: 4_000,
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.waitForSelector('.agent[role="button"], .cc-sidebar-row', { timeout: 30_000 });
    await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: "Person Worker Alpha" }).click();
    const pane = page.locator('[data-testid="chat-pane:person-worker-alpha"]');
    await pane.getByText("Fresh replay worker line 400").waitFor({ timeout: 10_000 });
    await assertNoText(page, "Stale replay worker line 250");

    const identityRequests = server.requests
      .filter((request) => request.url === "/console/rpc" && request.body)
      .map((request) => JSON.parse(request.body))
      .filter(
        (payload) =>
          payload.method === "meta/console/query_timeline" &&
          payload.params?.identity === "person-worker-alpha",
      );
    assert(
      identityRequests.some((payload) => payload.params?.mode === "since" && payload.params?.after === "console:250"),
      `expected stale since cursor to be rejected; saw ${JSON.stringify(identityRequests, null, 2)}`,
    );
    assert(
      identityRequests.some((payload) => payload.params?.mode === "recent" && payload.params?.after === undefined),
      `expected replay recovery to fall back to a fresh recent page; saw ${JSON.stringify(identityRequests, null, 2)}`,
    );

    process.stdout.write("browser chat pane replay recovery replaces stale local log ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runRunStartedClearsOptimisticPromptProof() {
  const prompt = "ORDER_PROOF send this once and keep the transcript chronological.";
  const baseTs = Date.parse("2026-05-23T20:45:00.000Z");
  const orderProofComplete = {
    id: "order-proof-complete",
    kind: "interaction_complete",
    identity: "identity:luka",
    interaction_id: "turn-identity:luka",
    timestamp_ms: baseTs + 2_000,
    cursor: "console:order:2",
    payload: { text: "ORDER_PROOF_FINAL visible after the prompt." },
  };
  const server = await startMockConsoleServer({
    // Reproduce the send/SSE race: run_started arrives on the live stream
    // while the console send RPC is still in flight, before the optimistic
    // entry has the interaction id from the response.
    consoleSendResponseDelayMs: 400,
    timelineStreamFramesDuringSendByIdentity: {
      "identity:luka": [
        {
          id: "order-proof-run-started",
          kind: "run_started",
          identity: "identity:luka",
          interaction_id: "turn-identity:luka",
          timestamp_ms: baseTs,
          cursor: "console:order:1",
          payload: { prompt },
        },
      ],
    },
    timelineFramesAfterSendByIdentity: {
      "identity:luka": [orderProofComplete],
    },
    // The completion reaches the console on the live stream, as it does
    // from the runtime; the paged query above only backs the reconcile
    // that the terminal frame triggers.
    timelineStreamFramesAfterSendByIdentity: {
      "identity:luka": [orderProofComplete],
    },
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);

    await openSidebarAgentChat(page, "Identity Example");
    await page.waitForSelector('[data-testid="chat-pane:identity:luka"]', { timeout: 30_000 });
    for (let attempt = 0; attempt < 40; attempt += 1) {
      if (server.requests.some((request) => request.url.startsWith("/console/timeline/stream"))) {
        break;
      }
      await sleep(50);
    }
    assert(
      server.requests.some((request) => request.url.startsWith("/console/timeline/stream")),
      `timeline stream was not connected before send: ${JSON.stringify(server.requests, null, 2)}`,
    );
    await fillComposer(page, prompt);
    await clickSend(page);

    const pane = page.locator('[data-testid="chat-pane:identity:luka"]');
    await pane.getByText("ORDER_PROOF_FINAL").waitFor({ timeout: 10_000 });
    await page.waitForTimeout(600);
    const bodyText = await pane.innerText();
    const promptMatches = bodyText.match(/ORDER_PROOF send this once/g) || [];
    assert.equal(
      promptMatches.length,
      1,
      `run_started should replace the optimistic prompt instead of leaving a duplicate tail prompt: ${bodyText}`,
    );
    const promptIndex = bodyText.indexOf(prompt);
    const finalIndex = bodyText.indexOf("ORDER_PROOF_FINAL");
    assert(promptIndex >= 0, `missing run_started prompt: ${bodyText}`);
    assert(finalIndex >= 0, `missing final response: ${bodyText}`);
    assert(
      promptIndex < finalIndex,
      `operator prompt must render before the final response: ${bodyText}`,
    );

    process.stdout.write("browser run_started optimistic cleanup ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runUserInputEchoClearsOptimisticPromptProof() {
  const prompt = "USER_INPUT_ECHO_PROOF should render once after the send race.";
  const baseTs = Date.parse("2026-05-23T21:20:00.000Z");
  const userInputEchoComplete = {
    id: "user-input-echo-complete",
    kind: "interaction_complete",
    identity: "identity:luka",
    interaction_id: "turn-identity:luka",
    timestamp_ms: baseTs + 2_000,
    cursor: "console:user-input-echo:2",
    payload: { text: "USER_INPUT_ECHO_FINAL visible after one prompt." },
  };
  const server = await startMockConsoleServer({
    // Reproduce the send/SSE race for the canonical console user_input
    // echo. This is distinct from run_started: the echoed frame already has
    // an interaction id, but the optimistic entry may not yet have received
    // that id from the still-in-flight send RPC.
    consoleSendResponseDelayMs: 400,
    timelineStreamFramesDuringSendByIdentity: {
      "identity:luka": [
        {
          id: "user-input-echo-proof",
          kind: "user_input",
          identity: "identity:luka",
          interaction_id: "turn-identity:luka",
          timestamp_ms: baseTs,
          cursor: "console:user-input-echo:1",
          payload: {
            content: prompt,
            handling_mode: "queue",
            origin: "console:panel-race",
          },
          status: "delivered",
        },
      ],
    },
    timelineFramesAfterSendByIdentity: {
      "identity:luka": [userInputEchoComplete],
    },
    timelineStreamFramesAfterSendByIdentity: {
      "identity:luka": [userInputEchoComplete],
    },
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);

    await openSidebarAgentChat(page, "Identity Example");
    await page.waitForSelector('[data-testid="chat-pane:identity:luka"]', { timeout: 30_000 });
    for (let attempt = 0; attempt < 40; attempt += 1) {
      if (server.requests.some((request) => request.url.startsWith("/console/timeline/stream"))) {
        break;
      }
      await sleep(50);
    }
    assert(
      server.requests.some((request) => request.url.startsWith("/console/timeline/stream")),
      `timeline stream was not connected before send: ${JSON.stringify(server.requests, null, 2)}`,
    );
    await fillComposer(page, prompt);
    await clickSend(page);

    const pane = page.locator('[data-testid="chat-pane:identity:luka"]');
    await pane.getByText("USER_INPUT_ECHO_FINAL").waitFor({ timeout: 10_000 });
    await page.waitForTimeout(600);
    const bodyText = await pane.innerText();
    const promptMatches = bodyText.match(/USER_INPUT_ECHO_PROOF/g) || [];
    assert.equal(
      promptMatches.length,
      1,
      `user_input echo should replace the optimistic prompt instead of duplicating it: ${bodyText}`,
    );

    process.stdout.write("browser user_input optimistic cleanup ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runLiveSystemNoticeAppearsInOpenChatProof() {
  const baseTs = Date.parse("2026-05-23T21:10:00.000Z");
  const server = await startMockConsoleServer({
    timelineFramesByIdentity: {
      "identity:luka": [
        {
          id: "live-peer-prior-tool-completed",
          kind: "tool_execution_completed",
          identity: "identity:luka",
          timestamp_ms: baseTs - 1_000,
          cursor: "console:peer-live:0",
          payload: {
            id: "call-send-message",
            name: "send_message",
            result: "sent",
          },
        },
      ],
    },
    timelineStreamFrames: [
      {
        id: "live-peer-system-notice",
        kind: "system_notice",
        identity: "identity:luka",
        timestamp_ms: baseTs,
        cursor: "console:peer-live:1",
        payload: {
          blocks: [{
            content: [{
              type: "text",
              text: "Peer message from ops/investigation-worker/investigation-worker-live-proof:\nLIVE_PEER_NOTICE landed in the parent chat.",
            }],
          }],
        },
      },
    ],
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);

    await openSidebarAgentChat(page, "Identity Example");
    const pane = page.locator('[data-testid="chat-pane:identity:luka"]');
    await pane.getByText("LIVE_PEER_NOTICE landed in the parent chat").waitFor({
      timeout: 10_000,
    });
    const bodyText = await pane.innerText();
    assert(
      bodyText.includes("Received from") || bodyText.includes("LIVE_PEER_NOTICE"),
      `live system_notice must route into the open chat pane, not only Signals: ${bodyText}`,
    );
    assert(
      !(await pane.locator('[data-testid="chat-typing:identity:luka"]').count()),
      `live system_notice should clear stale tool-completed busy state: ${bodyText}`,
    );
    assert(
      !bodyText.includes("working"),
      `live system_notice should not leave the parent pane visibly busy: ${bodyText}`,
    );

    process.stdout.write("browser live system_notice chat routing ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runConsoleMountsWithoutCryptoRandomUuidProof() {
  const server = await startMockConsoleServer();
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message || String(error)));
    await page.addInitScript(() => {
      const cryptoObject = globalThis.crypto;
      if (!cryptoObject) return;
      try {
        Object.defineProperty(cryptoObject, "randomUUID", {
          configurable: true,
          value: undefined,
        });
      } catch (_) {
        try {
          delete cryptoObject.randomUUID;
        } catch (_) {
          // Best effort: older browsers may expose a non-configurable property.
        }
      }
    });

    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.locator('[data-testid="meerkat-console"]').waitFor({ timeout: 10_000 });
    const labels = await sidebarLabels(page);
    assert(
      labels.some((label) => label.includes("Identity Example")),
      `console should mount and render agents without crypto.randomUUID; labels=${JSON.stringify(labels)}`,
    );
    assert.equal(
      errors.filter((message) => message.includes("randomUUID")).length,
      0,
      `console should not throw randomUUID page errors: ${JSON.stringify(errors)}`,
    );

    process.stdout.write("browser missing crypto.randomUUID mount ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runHeadlessControlSurfaceBrowserProof() {
  const server = await startMockConsoleServer({
    includeLifecycleActions: true,
  });
  let browser;

  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);

    await page.getByTestId("nav:routing").click();
    await page.getByTestId("routing-panel").waitFor({ timeout: 10_000 });
    await page.getByTestId("routing-route:route-control-1").waitFor({ timeout: 10_000 });
    await page.getByTestId("routing-delivery:delivery-control-1").waitFor({ timeout: 10_000 });
    await waitForRpcMethod(server, "meta/routing/routes/list");
    await waitForRpcMethod(server, "meta/delivery/history");

    await page.getByTestId("nav:gating").click();
    await page.getByTestId("gating-panel").waitFor({ timeout: 10_000 });
    await page.getByTestId("gating-pending:gate-control-1").waitFor({ timeout: 10_000 });
    await waitForRpcMethod(server, "meta/gating/pending");
    await waitForRpcMethod(server, "meta/gating/audit");
    await page.getByTestId("gating-action:gate-control-1:approve").click();
    await waitForRpcMethod(server, "meta/gating/decide", 1);
    await page.getByTestId("gating-action:gate-control-1:reject").waitFor({ timeout: 10_000 });
    await page.getByTestId("gating-action:gate-control-1:reject").click();
    await waitForRpcMethod(server, "meta/gating/decide", 2);
    await page.getByTestId("gating-action:gate-control-1:escalate").waitFor({ timeout: 10_000 });
    await page.getByTestId("gating-action:gate-control-1:escalate").click();
    await waitForRpcMethod(server, "meta/gating/decide", 3);

    await openSidebarAgentChat(page, "Identity Example");
    await page.getByTestId("conv-action:respawn").waitFor({ timeout: 10_000 });
    await page.getByTestId("conv-action:respawn").click();
    await waitForRpcMethod(server, "meta/respawn");
    await page.getByTestId("conv-action:retire").waitFor({ timeout: 10_000 });
    await page.getByTestId("conv-action:retire").click();
    await waitForRpcMethod(server, "meta/retire");
    await page.getByTestId("conv-action:details").click();
    await page.getByTestId("inspect-action:identity:luka:reset").waitFor({ timeout: 10_000 });
    await page.getByTestId("inspect-action:identity:luka:reset").click();
    await waitForRpcMethod(server, "meta/reset");

    process.stdout.write("browser headless control surface ok\n");
  } finally {
    if (browser) {
      await browser.close();
    }
    await server.close();
  }
}

async function runTopologyUnavailableBrowserProof() {
  const server = await startMockConsoleServer();
  let browser;
  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.getByTestId("nav:topology").click();
    await page.getByTestId("topology-panel").waitFor({ timeout: 10_000 });
    assert.equal(
      await page.getByTestId("topology-view:connections").count(),
      0,
      "a topology-unaware runtime must retain the passive graph without mutation controls",
    );
    assert.equal(await page.getByText(/connect all/i).count(), 0);
    assert.equal(
      rpcMethodsFromRequests(server.requests).includes("meta/topology/query"),
      false,
      "the console must not probe an unadvertised topology RPC",
    );
    process.stdout.write("browser topology unavailable fail-closed ok\n");
  } finally {
    if (browser) await browser.close();
    await server.close();
  }
}

async function runTopologyDeniedPairBrowserProof() {
  const server = await startMockConsoleServer({
    topologyControl: {
      methods: [
        "meta/topology/query",
        "meta/topology/plan",
        "meta/topology/apply",
        "meta/topology/operation/get",
      ],
      capabilities: {
        mode: "editable",
        can_query: true,
        can_plan: true,
        can_apply: true,
        can_bulk: false,
        max_batch_size: 1,
        can_cross_authority: false,
      },
      query: {
        authority: "mock-mob",
        revision: 1,
        policy: {
          mode: "editable",
          allow_bulk: false,
          max_batch_size: 1,
          allow_cross_authority: false,
        },
        nodes: [
          {
            endpoint: { identity: "identity:luka" },
            role: "lead",
            labels: {},
            affordances: {
              can_connect: true,
              can_disconnect: true,
              can_reconnect: true,
              can_bulk: false,
              can_cross_authority: false,
            },
          },
          {
            endpoint: { identity: "legacy-router" },
            role: "router",
            labels: {},
            affordances: {
              can_connect: false,
              can_disconnect: false,
              can_reconnect: false,
              can_bulk: false,
              can_cross_authority: false,
            },
          },
        ],
        edges: [],
      },
    },
  });
  let browser;
  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.getByTestId("nav:topology").click();
    await page.getByTestId("topology-view:connections").waitFor({ timeout: 10_000 });
    await page.getByTestId("topology-view:connections").click();
    const sourceId = stockTopologyEndpointId("mock-mob", "identity:luka");
    const targetId = stockTopologyEndpointId("mock-mob", "legacy-router");
    const source = page.getByTestId(`connection-picker-row:${sourceId}`);
    await source.locator(".topo-edit__identity").click();
    await page.getByTestId(`connection-picker-source:${sourceId}`).waitFor({ timeout: 10_000 });
    const connect = page
      .getByTestId(`connection-picker-row:${targetId}`)
      .locator(".topo-edit__toggle");
    await connect.waitFor({ state: "visible", timeout: 10_000 });
    assert.equal(await connect.isDisabled(), true, "one denied endpoint must disable the pair action");
    assert.match((await connect.getAttribute("aria-label")) || "", /Legacy Router.*Identity Example/i);
    assert.match((await connect.getAttribute("title")) || "", /both endpoints|denied/i);
    assert.equal(await page.getByText(/connect all/i).count(), 0);
    assert.equal(
      rpcMethodsFromRequests(server.requests).some((method) =>
        method === "meta/topology/plan" || method === "meta/topology/apply"
      ),
      false,
      "disabled pair controls must never emit a mutation RPC",
    );
    process.stdout.write("browser topology bilateral denial ok\n");
  } finally {
    if (browser) await browser.close();
    await server.close();
  }
}

async function runTopologyAmbiguousCommitBrowserProof() {
  const committedByKey = new Map();
  const applyPayloads = [];
  let physicalMutations = 0;
  const query = {
    authority: "mock-mob",
    revision: 1,
    policy: {
      mode: "editable",
      allow_bulk: false,
      max_batch_size: 1,
      allow_cross_authority: false,
    },
    nodes: ["identity:luka", "legacy-router"].map((identity) => ({
      endpoint: { identity },
      role: identity === "identity:luka" ? "lead" : "router",
      labels: {},
      affordances: {
        can_connect: true,
        can_disconnect: true,
        can_reconnect: true,
        can_bulk: false,
        can_cross_authority: false,
      },
    })),
    edges: [],
  };
  const sendResult = (res, id, result) => {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ jsonrpc: "2.0", id, result }));
  };
  const topologyControl = {
    methods: [
      "meta/topology/query",
      "meta/topology/plan",
      "meta/topology/apply",
      "meta/topology/operation/get",
    ],
    capabilities: {
      mode: "editable",
      can_query: true,
      can_plan: true,
      can_apply: true,
      can_bulk: false,
      max_batch_size: 1,
      can_cross_authority: false,
    },
    query,
    handleRpc(payload, res) {
      if (payload.method === "meta/topology/plan") {
        sendResult(res, payload.id, {
          authority: "mock-mob",
          base_revision: payload.params.expected_revision,
          operations: payload.params.operations,
        });
        return true;
      }
      if (payload.method === "meta/topology/operation/get") {
        const receipt = [...committedByKey.values()].find((candidate) => (
          candidate.operation_id === payload.params.operation_id
        ));
        assert.ok(receipt, "operation/get must resolve the committed operation");
        sendResult(res, payload.id, receipt);
        return true;
      }
      if (payload.method !== "meta/topology/apply") return false;

      applyPayloads.push(structuredClone(payload.params));
      const key = String(payload.params.idempotency_key || "");
      const existing = committedByKey.get(key);
      if (existing) {
        sendResult(res, payload.id, existing);
        return true;
      }

      physicalMutations += 1;
      const receipt = {
        operation_id: "operation-browser-stable",
        idempotency_key: key,
        actor: "browser-proof",
        status: "applied",
        base_revision: 1,
        revision: 2,
        created_at: "2026-07-15T12:00:00Z",
        results: [{
          action: "connect",
          edge: {
            a: { authority: "mock-mob", identity: "identity:luka" },
            b: { authority: "mock-mob", identity: "legacy-router" },
          },
          status: "applied",
          actual_before: false,
          actual_after: true,
        }],
      };
      committedByKey.set(key, receipt);
      query.revision = 2;
      query.edges = [{
        edge: receipt.results[0].edge,
        actual: true,
        declared: false,
        operator_added: true,
        suppressed: false,
        desired: true,
      }];
      // The server has committed and journaled the idempotency key, but an
      // intermediary truncates the HTTP response before the receipt arrives.
      // A syntactically incomplete 200 is deterministic and, unlike a socket
      // reset, cannot be transparently replayed by Chromium's network stack.
      res.writeHead(200, { "content-type": "application/json" });
      res.end('{"jsonrpc":"2.0","id":');
      return true;
    },
  };
  const server = await startMockConsoleServer({ topologyControl });
  let browser;
  try {
    browser = await launchBrowser();
    const page = await browser.newPage();
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.getByTestId("nav:topology").click();
    await page.getByTestId("topology-view:connections").click();
    const sourceId = stockTopologyEndpointId("mock-mob", "identity:luka");
    const targetId = stockTopologyEndpointId("mock-mob", "legacy-router");
    await page
      .getByTestId(`connection-picker-row:${sourceId}`)
      .locator(".topo-edit__identity")
      .click();
    await page.getByTestId(`connection-picker-source:${sourceId}`).waitFor({ timeout: 10_000 });
    await page.getByRole("button", { name: "Connect Legacy Router to Identity Example" }).click();

    const resolve = page.getByRole("button", { name: "Resolve Legacy Router to Identity Example" });
    await resolve.waitFor({ state: "visible", timeout: 10_000 });
    assert.equal(physicalMutations, 1);
    assert.equal(committedByKey.size, 1);
    await resolve.click();
    await waitForRpcMethod(server, "meta/topology/operation/get");
    await page.getByRole("button", { name: "Disconnect Legacy Router from Identity Example" })
      .waitFor({ state: "visible", timeout: 10_000 });

    assert.equal(physicalMutations, 1, "ambiguous recovery must not actuate twice");
    assert.equal(committedByKey.size, 1, "ambiguous recovery must retain one logical operation");
    assert.equal(applyPayloads.length, 2);
    assert.deepEqual(applyPayloads[1], applyPayloads[0], "Resolve must replay the exact apply payload");
    assert.equal(
      rpcMethodsFromRequests(server.requests).filter((method) => method === "meta/topology/plan").length,
      1,
      "Resolve must not silently replan or rebase",
    );
    process.stdout.write("browser topology ambiguous commit resolution ok\n");
  } finally {
    if (browser) await browser.close();
    await server.close();
  }
}

// UI contract test only: local HTTP fixtures do not install native authorization.
async function runAuthorizationFeedbackProof() {
  const evidence = process.env.META_BROWSER_EVIDENCE || "/tmp/adr-001-console-browser-r2";
  fs.mkdirSync(evidence, { recursive: true });
  const base = { identity: "identity:luka", session_id: "session-auth", run_id: "run-auth", interaction_id: "work-auth" };
  const frame = (id, kind, payload, index) => ({ ...base, id, kind, payload, cursor: `console:${index}`, timestamp_ms: Date.now() - 10000 + index });
  const approvalId = "B6503200-06B6-4157-AFC0-2A3C8D124178";
  const reviewFrames = (callId, error, kind, index, approvalReference) => {
    const content = [{ type: "text", text: JSON.stringify({
      error,
      message: `PRIVATE_REVIEW_CANARY_${kind}`,
      data: { kind, ...(approvalReference ? { approval_id: approvalReference } : {}) },
    }) }];
    const result = { id: callId, name: "write_calendar", is_error: true, content };
    return [
      frame(`${callId}-requested`, "tool_call_requested", { id: callId, name: "write_calendar", args: {} }, index),
      frame(`${callId}-result`, "tool_result_received", result, index + 1),
      frame(`${callId}-completed`, "tool_execution_completed", { ...result, duration_ms: 0 }, index + 2),
    ];
  };
  const frames = [
    frame("start", "run_started", {}, 1),
    frame("refusal", "system_notice", { message: { role: "system_notice", kind: "generic", body: "PRIVATE_NOTICE_CANARY", blocks: [{ type: "runtime_notice", category: "operation_refused", payload: { code: "operation_refused" } }] } }, 2),
    ...reviewFrames("consent-call", "review_unsatisfied", "human_consent_required", 3, approvalId),
    ...reviewFrames("unavailable-call", "review_unavailable", "reviewer_failed", 6),
    ...reviewFrames("consumed-call", "review_unsatisfied", "already_consumed", 9),
    frame("read", "tool_call_requested", { id: "read-call", name: "read_calendar", args: {} }, 12),
    frame("read-result", "tool_result_received", { id: "read-call", name: "read_calendar", is_error: false, content: [{ type: "text", text: "Permitted calendar read completed" }] }, 13),
    frame("audit", "operation_observation_failed", { operation_id: "op-read", phase: "outcome", private: "PRIVATE_AUDIT_CANARY" }, 14),
    frame("answer", "text_complete", { text: "I continued with the permitted calendar read." }, 15),
    frame("done", "run_completed", { result: "I continued with the permitted calendar read." }, 16),
    frame("interaction-done", "interaction_complete", { result: "I continued with the permitted calendar read." }, 17),
  ];
  const server = await startMockConsoleServer({ timelineFramesByIdentity: { "identity:luka": frames } });
  let browser, routes;
  let finishPreview;
  try {
    browser = await launchBrowser();
    const page = await browser.newPage({ viewport: { width: 1280, height: 960 } });
    const errors = [];
    page.on("pageerror", error => errors.push(String(error)));
    routes = routeGuard(page, "authorization-feedback");
    let gatingDecisions = 0;
    let gatingPendingReads = 0;
    let admin = true;
    let revision = 1;
    let mutations = 0;
    let previewStarted;
    const previewRequest = new Promise(resolve => { previewStarted = resolve; });
    const metadata = () => ({ owner_instance: "opaque-owner:authorization-feedback", revision, conditional_mutations: "checked_v1" });
    const status = () => ({ available: true, enabled: true, can_administer: admin, subject: "admin@example.test", ...metadata(), actions: ["agent.view", "agent.send"] });
    await routes.route("**/console/experience", async route => {
      const response = await route.fetch();
      const value = await response.json();
      await route.fulfill({ response, json: { ...value, access: status() } });
    });
    await routes.route("**/console/rpc", async route => {
      const request = route.request().postDataJSON();
      const reply = result => route.fulfill({ json: { jsonrpc: "2.0", id: request.id, result } });
      if (request.method === "meta/gating/decide") gatingDecisions += 1;
      // A native owner reference is not an actionable Meta pending request.
      if (request.method === "meta/gating/pending") {
        gatingPendingReads += 1;
        return reply({ pending: [] });
      }
      if (request.method === "meta/capabilities") {
        const response = await route.fetch(); const value = await response.json();
        value.result.methods.push(...["status", "get", "set", "enable", "rules/upsert", "rules/delete", "groups/set", "groups/delete", "preview"].map(name => `meta/access/${name}`));
        return route.fulfill({ response, json: value });
      }
      if (request.method === "meta/access/status") return reply(status());
      if (request.method === "meta/access/get") return reply({ ...metadata(), config: { enabled: true, admins: ["admin@example.test"], rules: [], groups: {} } });
      if (request.method === "meta/access/preview") {
        previewStarted();
        await new Promise(resolve => { finishPreview = resolve; });
        return reply({ allowed: true, reason: "STALE_PREVIEW_CANARY" });
      }
      if (request.method === "meta/access/rules/upsert") {
        mutations += 1;
        return route.fulfill({ json: { jsonrpc: "2.0", id: request.id, error: { code: -32602, message: "Fixture owner rejected this rule" } } });
      }
      return route.continue();
    });
    await gotoConsole(page, `${server.baseUrl}/console`);
    await openSidebarAgentChat(page, "Identity Example");
    const pane = page.getByTestId("chat-pane:identity:luka");
    await pane.getByText("Action refused", { exact: true }).waitFor();
    // The refusal names no reason, so the card claims no permission cause.
    assert.equal(await pane.getByText("Permission denied").count(), 0);
    await pane.getByText("Audit update unavailable", { exact: true }).waitFor();
    await pane.getByText("I continued with the permitted calendar read.", { exact: true }).first().waitFor();
    const consent = pane.locator('[data-feedback-kind="review-unsatisfied"]').filter({ hasText: "Call consent-call" });
    const unavailable = pane.locator('[data-feedback-kind="review-unavailable"]').filter({ hasText: "Call unavailable-call" });
    const consumed = pane.locator('[data-feedback-kind="review-unsatisfied"]').filter({ hasText: "Call consumed-call" });
    for (const feedback of [consent, unavailable, consumed]) {
      await feedback.waitFor({ state: "visible" });
      assert.equal(await feedback.count(), 1, "result/completed twins must produce one feedback card per call");
      assert.equal(await feedback.getAttribute("role"), "status");
      assert.equal(await feedback.locator('button, a, input, select, textarea, [role="button"], [role="link"]').count(), 0,
        "native review feedback must not infer a retry or approval action");
      assert.doesNotMatch(await feedback.innerText(), /Permission denied|Run failed|Awaiting approval|Approved/);
    }
    assert.equal(await pane.locator('[data-feedback-kind="review-unsatisfied"]').count(), 2);
    assert.equal(await pane.locator('[data-feedback-kind="review-unavailable"]').count(), 1);
    assert.equal(await consent.getAttribute("aria-label"), "Review not satisfied");
    assert.match(await consent.innerText(), /requires human consent/);
    assert.equal(await consent.locator(".cc-operation-feedback__reference").filter({ hasText: "Approval reference" }).innerText(),
      `Approval reference ${approvalId}`, "preserve the exact native reference as plain display text");
    assert.equal(await pane.getByText(`Approval reference ${approvalId}`, { exact: true }).count(), 1);
    assert.equal(await unavailable.getAttribute("aria-label"), "Review unavailable");
    assert.match(await unavailable.innerText(), /could not provide a usable decision/);
    assert.doesNotMatch(await unavailable.innerText(), /reviewer declined|Approval reference/);
    assert.match(await consumed.innerText(), /does not establish the action's execution result; do not repeat/);
    assert.doesNotMatch(await consumed.innerText(), /did not run|never entered|no effect|Approval reference/);
    assert.equal(await pane.locator('[data-testid^="gating-pending:"], [data-testid^="gating-action:"]').count(), 0);
    assert.equal(await pane.getByTestId("chat-event:run_failed").count(), 0);
    assert.doesNotMatch(await pane.innerText(), /Run failed/);
    const sibling = pane.locator(".cc-tool-call").filter({ has: page.locator('.cc-tool-call__name[title="read_calendar"]') });
    await sibling.waitFor({ state: "visible" });
    assert.equal(await sibling.count(), 1);
    assert.match(await sibling.getAttribute("class"), /cc-tool-call--success/);
    const siblingHeader = sibling.locator(".cc-tool-call__header");
    if (await siblingHeader.getAttribute("aria-expanded") === "false") await siblingHeader.click();
    await sibling.getByText("Permitted calendar read completed", { exact: true }).waitFor({ state: "visible" });
    assert.doesNotMatch(await page.locator("body").textContent(), /PRIVATE_NOTICE_CANARY|PRIVATE_AUDIT_CANARY|PRIVATE_REVIEW_CANARY/);
    assert.equal(gatingDecisions, 0, "viewing native review feedback must not submit a gating decision");
    await page.screenshot({ path: path.join(evidence, "operation-feedback.png"), fullPage: true });
    await consent.screenshot({ path: path.join(evidence, "native-consent-reference.png") });
    await unavailable.screenshot({ path: path.join(evidence, "native-review-unavailable.png") });
    await consumed.screenshot({ path: path.join(evidence, "native-review-consumed.png") });
    const composer = page.getByTestId("chat-composer:identity:luka");
    await composer.fill("A permitted follow-up request");
    await composer.press("Enter");
    await page.waitForFunction(() => document.querySelector('[data-testid="chat-composer:identity:luka"]')?.value === "", null, { timeout: 10_000 });
    await waitForRpcMethod(server, "meta/console/send");
    const followUpRequests = server.requests.filter(row => row.url === "/console/rpc")
      .map(row => JSON.parse(row.body)).filter(request => request.method === "meta/console/send");
    assert.equal(followUpRequests.length, 1, "follow-up must reach the actual Meta send owner exactly once");
    assert.equal(followUpRequests[0].params.identity, base.identity);
    const followUpContent = followUpRequests[0].params.content;
    assert.deepEqual(typeof followUpContent === "string" ? [{ type: "text", text: followUpContent }] : followUpContent,
      [{ type: "text", text: "A permitted follow-up request" }]);
    assert.equal(gatingDecisions, 0, "permitted follow-up must not infer approval from a native reference");
    await page.getByText("Access", { exact: true }).first().click();
    await page.getByRole("heading", { name: "Console access", exact: true }).waitFor();
    await page.getByTestId("access-toggle-enabled").waitFor();
    await page.getByTestId("access-tab:preview").click();
    await page.getByTestId("access-preview-subject").fill("first@example.test");
    await page.getByTestId("access-preview-run").click();
    await previewRequest;
    await page.getByTestId("access-preview-subject").fill("second@example.test");
    finishPreview();
    await page.getByTestId("access-tab:rules").click();
    await page.getByTestId("access-rule-new").click();
    await page.getByTestId("access-rule-id").fill("draft-survives");
    await page.getByTestId("access-rule-save").click();
    await page.getByTestId("access-error").waitFor();
    assert.equal(await page.getByTestId("access-rule-id").inputValue(), "draft-survives");
    assert.equal(mutations, 1);
    assert(await page.getByTestId("access-rule-save").isDisabled());
    assert.doesNotMatch(await page.locator("body").innerText(), /STALE_PREVIEW_CANARY/);
    await page.screenshot({ path: path.join(evidence, "console-access-failed-draft.png"), fullPage: true });
    admin = false; revision += 1;
    await page.getByTestId("access-refresh").click();
    await page.getByText("Current administrator access is required to view this configuration.").waitFor();
    assert.doesNotMatch(await page.locator("body").innerText(), /admin@example.test|draft-survives/);
    assert.equal(errors.length, 0, errors.join("\n"));
    await page.screenshot({ path: path.join(evidence, "console-access-revoked.png"), fullPage: true });
    await routes.settle();
    routes.check();
    assert.equal(gatingDecisions, 0);
    assert(gatingPendingReads > 0, "the current Meta pending owner must supply the empty actionable-approval set");
    fs.writeFileSync(path.join(evidence, "result.json"), JSON.stringify({ backend: "local mock HTTP", nativeAuthorizationInstalled: false, browser: browser.version(), cases: ["typed local refusal", "native human-consent reference without actions", "review unavailable is distinct from denial", "already-consumed result stays uncertain", "result/completed twins deduplicated", "private diagnostics absent", "permitted sibling result and final answer retained", "no inferred gating decision", "separate audit outcome", "permitted follow-up", "stale preview discarded", "failed draft retained", "revoked data hidden"], approvalId, gatingDecisions, gatingPendingReads, followUpSendCount: followUpRequests.length, mutations, errors }, null, 2));
    process.stdout.write("authorization feedback browser contract passed (mock backend)\n");
  } catch (error) {
    printScenarioError("authorization-feedback", error);
    throw error;
  } finally {
    finishPreview?.();
    await routes?.settle();
    if (browser) await browser.close();
    await server.close();
  }
}

// Real Chromium UI over the existing local HTTP host and mocked access RPC.
// This does not install or execute the Rust access owner.
async function runCheckedSaveBrowserProof(mode) {
  const evidence = path.join(process.env.META_BROWSER_EVIDENCE || path.join(repoRoot, "output/playwright/console-acceptance"), mode);
  fs.mkdirSync(evidence, { recursive: true });
  const initialGroup = { description: "Original group description", members: ["alice@example.test"] };
  const newerGroup = { ...initialGroup, description: "Another admin's updated description" };
  const original = { enabled: true, admins: ["root@example.test", "alice@example.test"], rules: [], groups: mode === "group" ? { ops: initialGroup } : {} };
  const newerRule = { id: "b-newer-rule", effect: "allow", subjects: ["reader@example.test"], actions: ["agent.view"], agents: ["worker"] };
  const state = { owner: "opaque-owner:one", revision: 10, config: structuredClone(original), applied: 0 };
  const writes = [], reads = [], errors = [], checkpoints = [];
  const mutationMethods = new Set(["set", "enable", "rules/upsert", "rules/delete", "groups/set", "groups/delete"].map(name => `meta/access/${name}`));
  const metadata = () => ({ revision: state.revision, owner_instance: state.owner, conditional_mutations: "checked_v1" });
  const status = () => ({ available: true, enabled: true, can_administer: true, subject: "root@example.test", actions: ["agent.view"], ...metadata() });
  const server = await startMockConsoleServer();
  let browser, page, routes, failure;
  try {
    browser = await launchBrowser();
    page = await browser.newPage({ viewport: { width: 1280, height: 960 } });
    page.setDefaultTimeout(10000);
    page.on("pageerror", error => errors.push(String(error)));
    routes = routeGuard(page, `checked-save-${mode}`);
    await routes.route("**/console/experience", async route => {
      const response = await route.fetch(); const value = await response.json();
      await route.fulfill({ response, json: { ...value, access: status() } });
    });
    await routes.route("**/console/rpc", async route => {
      const request = route.request().postDataJSON();
      const reply = payload => route.fulfill({ json: { jsonrpc: "2.0", id: request.id, ...payload } });
      if (request.method === "meta/capabilities") {
        const response = await route.fetch(); const value = await response.json();
        value.result.methods.push("meta/access/status", "meta/access/get", ...mutationMethods);
        return route.fulfill({ response, json: value });
      }
      if (request.method === "meta/access/status") {
        reads.push({ method: request.method, ...metadata() });
        return reply({ result: status() });
      }
      if (request.method === "meta/access/get") {
        reads.push({ method: request.method, ...metadata() });
        return reply({ result: { config: state.config, ...metadata() } });
      }
      if (mutationMethods.has(request.method)) {
        writes.push(structuredClone({ method: request.method, params: request.params }));
        if (mode === "unavailable") {
          // Keep valid read metadata to isolate the finite error projection.
          return reply({ error: { code: -32004, message: "PRIVATE_UNAVAILABLE_CANARY", data: { kind: "access_mutation_unavailable" } } });
        }
        const checked = request.params.checked_v1;
        if (Object.keys(request.params).length !== 1 || checked?.owner_instance !== state.owner) {
          return reply({ error: { code: -32602, message: "PRIVATE_PAYLOAD_CANARY" } });
        }
        if (checked.expected_revision !== state.revision) {
          return reply({ error: { code: -32009, message: "PRIVATE_CONFLICT_CANARY", data: { kind: "access_revision_conflict", expected_revision: checked.expected_revision, actual_revision: state.revision } } });
        }
        if (request.method === "meta/access/set") state.config = checked.config;
        else if (request.method === "meta/access/groups/set" && checked.name === "ops") state.config = { ...state.config, groups: { ...state.config.groups, ops: checked.group } };
        else throw new Error(`unexpected checked mutation ${request.method}`);
        state.applied += 1; state.revision += 1;
        return reply({ result: { config: state.config, ...metadata() } });
      }
      return route.continue();
    });
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.getByText("Access", { exact: true }).first().click();
    await page.getByRole("heading", { name: "Console access", exact: true }).waitFor();
    await page.getByTestId("access-edit-admins").waitFor();
    let draft, save;
    if (mode === "group") {
      await page.getByTestId("access-tab:groups").click();
      await page.getByTestId("access-group-edit:ops").click();
      draft = page.getByTestId("access-group-members"); save = page.getByTestId("access-group-save");
      await draft.fill("carol@example.test");
      state.config = { ...state.config, groups: { ops: newerGroup } }; state.revision = 11;
      const refreshed = page.waitForResponse(response => response.url().endsWith("/console/rpc") && response.request().postDataJSON()?.method === "meta/access/get");
      // Closing the page rejects a wait never awaited after an earlier failure.
      refreshed.catch(() => {});
      await page.getByTestId("access-refresh").click();
      await refreshed;
      await save.click({ trial: true });
    } else {
      await page.getByTestId("access-edit-admins").click();
      draft = page.getByTestId("access-admins-input"); save = page.getByTestId("access-save-admins");
      await draft.fill("root@example.test, alice@example.test, carol@example.test");
      if (mode === "conflict") {
        state.config = { ...state.config, rules: [newerRule] }; state.revision = 11;
        await page.getByTestId("access-refresh").click();
        await page.getByTestId("access-tab:rules").click();
        await page.getByTestId("access-rule:b-newer-rule").waitFor();
        await page.getByTestId("access-tab:overview").click();
      }
    }
    const draftText = await draft.inputValue();
    assert.match(draftText, /carol@example.test/);
    assert.equal(writes.length, 0, "refresh must not save a retained draft");
    await save.click();
    await page.getByTestId("access-error").waitFor();
    const notice = await page.getByTestId("access-error").innerText();
    const disabled = await save.isDisabled();
    checkpoints.push({ step: "first-response", notice, saveDisabled: disabled, draft: await draft.inputValue(), writes: writes.length, readMetadata: metadata() });
    await page.screenshot({ path: path.join(evidence, "first-response.png"), fullPage: true });
    assert.equal(writes.length, 1);
    assert.equal(state.applied, 0);
    assert.equal(await draft.inputValue(), draftText);
    assert.doesNotMatch(await page.locator("body").innerText(), /PRIVATE_.*CANARY/);
    if (mode === "unavailable") {
      assert.equal(notice, "Changes were not saved. Checked access saves are unavailable; your draft is retained.", "typed mutation unavailable must have finite unavailable feedback");
      assert(disabled, "typed mutation unavailable must block repeated Save despite valid read metadata");
      // Save is disabled while any read is in flight, so check the panel only
      // after the refresh landed: its config read answered and the banner
      // that marks a read in flight is gone.
      const readsBeforeRefresh = reads.length;
      const refreshed = page.waitForResponse(response => response.url().endsWith("/console/rpc") && response.request().postDataJSON()?.method === "meta/access/get");
      refreshed.catch(() => {});
      await page.getByTestId("access-refresh").click();
      await refreshed;
      await page.getByTestId("access-panel").getByText("Refreshing owner state. Changes are temporarily unavailable.", { exact: true }).waitFor({ state: "detached" });
      assert.deepEqual(reads.slice(readsBeforeRefresh).map(read => read.method), ["meta/access/status", "meta/access/get"], "Refresh must read the owner state again");
      assert.equal(await page.getByTestId("access-error").innerText(), notice, "an ordinary refresh must keep the unavailable notice");
      assert.equal(await draft.inputValue(), draftText);
      assert(await save.isDisabled(), "ordinary refresh must not silently reapply the draft");
      assert.equal(writes.length, 1);
      assert.equal(state.applied, 0);
    } else {
      assert.equal(notice, "Access configuration changed. Review the latest settings before saving again.");
      assert(disabled);
      const firstPayload = mode === "group" ? { name: "ops", group: { ...initialGroup, members: ["carol@example.test"] } } : { config: { ...original, admins: draftText.split(", ") } };
      assert.deepEqual(writes[0].params, { checked_v1: { owner_instance: state.owner, expected_revision: 10, ...firstPayload } });
      await page.getByRole("button", { name: "Review and reapply", exact: true }).click();
      assert.equal(writes.length, 1, "review and reapply must not submit a mutation");
      assert.equal(await draft.inputValue(), draftText);
      await save.click({ trial: true });
      assert(await save.isEnabled());
      checkpoints.push({ step: "reviewed-without-save", writes: writes.length, draft: await draft.inputValue() });
      await save.click();
      // A member chip also carries its Inspect access control, so match the member text within it.
      if (mode === "group") await page.getByTestId("access-group:ops").getByText("carol@example.test").waitFor();
      else await page.getByTestId("access-admins-input").waitFor({ state: "detached" });
      assert.equal(writes.length, 2);
      const lastPayload = mode === "group" ? { name: "ops", group: { ...newerGroup, members: ["carol@example.test"] } } : { config: { ...original, admins: draftText.split(", "), rules: [newerRule] } };
      assert.deepEqual(writes[1].params, { checked_v1: { owner_instance: state.owner, expected_revision: 11, ...lastPayload } });
      assert.equal(state.applied, 1); assert.equal(state.revision, 12);
      if (mode === "group") assert.deepEqual(state.config.groups.ops, lastPayload.group);
      else assert.deepEqual(state.config, lastPayload.config);
      await page.screenshot({ path: path.join(evidence, "explicit-second-save.png"), fullPage: true });
    }
    await routes.settle();
    routes.check();
    assert.deepEqual(errors, []);
    checkpoints.push({ step: "complete", writes: writes.length, applied: state.applied, revision: state.revision });
  } catch (error) {
    printScenarioError(`checked-save-${mode}`, error);
    failure = String(error.stack || error);
    if (page) fs.writeFileSync(path.join(evidence, "failure-body.txt"), await page.locator("body").innerText().catch(() => "unavailable"));
    throw error;
  } finally {
    await routes?.settle();
    fs.writeFileSync(path.join(evidence, "result.json"), JSON.stringify({ mode, backend: "mock HTTP access RPC", nativeAuthorizationInstalled: false, state, writes, reads, checkpoints, errors, routeFailures: routes?.failures || [], failure: failure || null }, null, 2));
    if (browser) await browser.close();
    await server.close();
  }
}

// Stream events refresh visible panels at any moment. A tab click whose press
// lands before such a refresh starts and whose release lands after it must
// still select the tab: the refresh must not move the tab row under the
// pointer, or the release lands on another element and no click reaches the
// tab. The status read is held open so the refresh is visibly in flight.
async function runAccessTabClickDuringRefreshProof() {
  const evidence = path.join(process.env.META_BROWSER_EVIDENCE || path.join(repoRoot, "output/playwright/console-acceptance"), "access-tab-click-during-refresh");
  fs.mkdirSync(evidence, { recursive: true });
  const config = { enabled: true, admins: ["root@example.test"], rules: [], groups: {} };
  const metadata = { revision: 1, owner_instance: "opaque-owner:one", conditional_mutations: "checked_v1" };
  const status = () => ({ available: true, enabled: true, can_administer: true, subject: "root@example.test", actions: ["agent.view"], ...metadata });
  let holdStatusReads = false;
  let releaseStatusReads;
  const statusReadsReleased = new Promise(resolve => { releaseStatusReads = resolve; });
  const heldReads = [], errors = [], checkpoints = [];
  const server = await startMockConsoleServer();
  let browser, page, routes, failure;
  try {
    browser = await launchBrowser();
    page = await browser.newPage({ viewport: { width: 1280, height: 960 } });
    page.setDefaultTimeout(10000);
    page.on("pageerror", error => errors.push(String(error)));
    routes = routeGuard(page, "access-tab-click-during-refresh");
    await routes.route("**/console/experience", async route => {
      const response = await route.fetch(); const value = await response.json();
      await route.fulfill({ response, json: { ...value, access: status() } });
    });
    await routes.route("**/console/rpc", async route => {
      const request = route.request().postDataJSON();
      const reply = payload => route.fulfill({ json: { jsonrpc: "2.0", id: request.id, ...payload } });
      if (request.method === "meta/capabilities") {
        const response = await route.fetch(); const value = await response.json();
        value.result.methods.push("meta/access/status", "meta/access/get");
        return route.fulfill({ response, json: value });
      }
      if (request.method === "meta/access/status") {
        if (holdStatusReads) {
          heldReads.push(request.id);
          await statusReadsReleased;
        }
        return reply({ result: status() });
      }
      if (request.method === "meta/access/get") return reply({ result: { config, ...metadata } });
      return route.continue();
    });
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.getByText("Access", { exact: true }).first().click();
    await page.getByRole("heading", { name: "Console access", exact: true }).waitFor();
    await page.getByTestId("access-edit-admins").waitFor();
    const refreshing = page.getByTestId("access-panel")
      .getByText("Refreshing owner state. Changes are temporarily unavailable.", { exact: true });
    await refreshing.waitFor({ state: "detached" });
    // A web font that finishes loading between the two measurements changes the
    // tab labels' metrics and moves the row by a fraction of a pixel; settle the
    // fonts first so both boxes are measured with the same faces.
    await page.evaluate(() => document.fonts.ready);
    const tab = page.getByTestId("access-tab:preview");
    const target = await tab.boundingBox();
    await page.mouse.move(target.x + target.width / 2, target.y + target.height / 2);
    await page.mouse.down();
    // Measured while pressed, so press styling is the same in both boxes.
    const pressed = await tab.boundingBox();
    assert.equal(await refreshing.count(), 0, "no refresh may be in flight before the held one");
    holdStatusReads = true;
    await page.getByTestId("access-refresh").evaluate(button => button.click());
    await refreshing.waitFor();
    const refreshingBox = await tab.boundingBox();
    await page.screenshot({ path: path.join(evidence, "pressed-during-refresh.png"), fullPage: true });
    await page.mouse.up();
    const selected = await tab.evaluate(element => element.classList.contains("is-active"));
    checkpoints.push({ step: "released-during-refresh", pressed, refreshingBox, selected, heldReads: [...heldReads] });
    holdStatusReads = false;
    releaseStatusReads();
    await refreshing.waitFor({ state: "detached" });
    assert.deepEqual(refreshingBox, pressed, "a refresh moved the tab row under a pressed pointer");
    assert.equal(selected, true, "a tab click that spans the start of a refresh did not select the tab");
    await page.getByTestId("access-preview-subject").waitFor();
    await routes.settle();
    routes.check();
    assert.deepEqual(errors, []);
    checkpoints.push({ step: "complete" });
  } catch (error) {
    printScenarioError("access-tab-click-during-refresh", error);
    failure = String(error.stack || error);
    if (page) fs.writeFileSync(path.join(evidence, "failure-body.txt"), await page.locator("body").innerText().catch(() => "unavailable"));
    throw error;
  } finally {
    releaseStatusReads();
    await routes?.settle();
    fs.writeFileSync(path.join(evidence, "result.json"), JSON.stringify({ backend: "mock HTTP access RPC", checkpoints, errors, routeFailures: routes?.failures || [], failure: failure || null }, null, 2));
    if (browser) await browser.close();
    await server.close();
  }
}

// The owner's access_changed signal refreshes a settled Access panel in the
// background. While that read is in flight the panel keeps its edits: no
// "Refreshing owner state" notice, and a press and release on Edit admins
// opens the editor. The
// configuration read is held open so the background read is visibly in
// flight; the owner's edit token, not a disabled form, protects a save.
async function runAccessEditDuringBackgroundRefreshProof() {
  const evidence = path.join(process.env.META_BROWSER_EVIDENCE || path.join(repoRoot, "output/playwright/console-acceptance"), "access-edit-during-background-refresh");
  fs.mkdirSync(evidence, { recursive: true });
  const config = { enabled: true, admins: ["root@example.test"], rules: [], groups: {} };
  const metadata = { revision: 1, owner_instance: "opaque-owner:one", conditional_mutations: "checked_v1" };
  const status = () => ({ available: true, enabled: true, can_administer: true, subject: "root@example.test", actions: ["agent.view"], ...metadata });
  let holdConfigReads = false;
  let releaseConfigReads;
  const configReadsReleased = new Promise(resolve => { releaseConfigReads = resolve; });
  const heldReads = [], answeredReads = [], errors = [], checkpoints = [];
  const server = await startMockConsoleServer({ holdTimelineStream: true });
  let browser, page, routes, failure;
  try {
    browser = await launchBrowser();
    page = await browser.newPage({ viewport: { width: 1280, height: 960 } });
    page.setDefaultTimeout(10000);
    page.on("pageerror", error => errors.push(String(error)));
    routes = routeGuard(page, "access-edit-during-background-refresh");
    await routes.route("**/console/experience", async route => {
      const response = await route.fetch(); const value = await response.json();
      await route.fulfill({ response, json: { ...value, access: status() } });
    });
    await routes.route("**/console/rpc", async route => {
      const request = route.request().postDataJSON();
      const reply = payload => route.fulfill({ json: { jsonrpc: "2.0", id: request.id, ...payload } });
      if (request.method === "meta/capabilities") {
        const response = await route.fetch(); const value = await response.json();
        value.result.methods.push("meta/access/status", "meta/access/get");
        return route.fulfill({ response, json: value });
      }
      if (request.method === "meta/access/status") return reply({ result: status() });
      if (request.method === "meta/access/get") {
        if (holdConfigReads) {
          heldReads.push(request.id);
          await configReadsReleased;
        }
        await reply({ result: { config, ...metadata } });
        answeredReads.push(request.id);
        return;
      }
      return route.continue();
    });
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.getByText("Access", { exact: true }).first().click();
    await page.getByRole("heading", { name: "Console access", exact: true }).waitFor();
    const edit = page.getByTestId("access-edit-admins");
    await edit.waitFor();
    const refreshing = page.getByTestId("access-panel")
      .getByText("Refreshing owner state. Changes are temporarily unavailable.", { exact: true });
    // The first read of the scope is not a background read: it shows the notice
    // until it lands.
    await refreshing.waitFor({ state: "detached" });
    holdConfigReads = true;
    const streams = server.emitAccessChanged(metadata.revision + 1);
    assert.ok(streams > 0, "no timeline stream was open for the access signal");
    for (let wait = 0; heldReads.length === 0; wait += 1) {
      assert.ok(wait < 200, "the access signal did not start a background configuration read");
      await page.waitForTimeout(25);
    }
    await page.screenshot({ path: path.join(evidence, "background-read-in-flight.png"), fullPage: true });
    assert.equal(await refreshing.count(), 0, "a background read showed the refresh notice");
    assert.equal(await edit.isEnabled(), true, "a background read disabled Edit admins");
    const target = await edit.boundingBox();
    await page.mouse.move(target.x + target.width / 2, target.y + target.height / 2);
    await page.mouse.down();
    await page.mouse.up();
    await page.getByTestId("access-admins-input").waitFor();
    checkpoints.push({ step: "edited-during-background-read", heldReads: [...heldReads], refreshing: await refreshing.count() });
    holdConfigReads = false;
    releaseConfigReads();
    // The background read lands with the same revision: the open draft stays.
    for (let wait = 0; !answeredReads.includes(heldReads[0]); wait += 1) {
      assert.ok(wait < 200, "the held background read was never answered");
      await page.waitForTimeout(25);
    }
    await page.waitForTimeout(250);
    assert.equal(await page.getByTestId("access-admins-input").count(), 1, "the landed background read closed the open draft");
    assert.equal(await refreshing.count(), 0);
    await routes.settle();
    routes.check();
    assert.deepEqual(errors, []);
    checkpoints.push({ step: "complete" });
  } catch (error) {
    printScenarioError("access-edit-during-background-refresh", error);
    failure = String(error.stack || error);
    if (page) fs.writeFileSync(path.join(evidence, "failure-body.txt"), await page.locator("body").innerText().catch(() => "unavailable"));
    throw error;
  } finally {
    releaseConfigReads();
    await routes?.settle();
    fs.writeFileSync(path.join(evidence, "result.json"), JSON.stringify({ backend: "mock HTTP access RPC", checkpoints, errors, routeFailures: routes?.failures || [], failure: failure || null }, null, 2));
    if (browser) await browser.close();
    await server.close();
  }
}

// A memory refresh that fails shows its error while a tab click may be in
// flight, and the next successful refresh clears it. The error must not move
// the tab row under a pressed pointer, or the release lands on another
// element and no click reaches the tab.
async function runMemoryTabClickDuringFailedRefreshProof() {
  const evidence = path.join(process.env.META_BROWSER_EVIDENCE || path.join(repoRoot, "output/playwright/console-acceptance"), "memory-tab-click-during-failed-refresh");
  fs.mkdirSync(evidence, { recursive: true });
  const replies = {
    "meta/memory/panel/records": { records: [], realms: [] },
    "meta/memory/panel/dreams": { runs: [] },
    "meta/memory/panel/overview": null,
    "meta/memory/panel/proposals": { proposals: [] },
    "meta/memory/panel/injections": { injections: [] },
    "meta/memory/panel/harvests": { harvests: [] },
    "meta/memory/panel/dream_runs": { runs: [] },
    "meta/memory/panel/audit_verdicts": { verdicts: [] },
  };
  let failRecordReads = false;
  const failedReads = [], errors = [], checkpoints = [];
  const server = await startMockConsoleServer();
  let browser, page, routes, failure;
  try {
    browser = await launchBrowser();
    page = await browser.newPage({ viewport: { width: 1280, height: 960 } });
    page.setDefaultTimeout(10000);
    page.on("pageerror", error => errors.push(String(error)));
    routes = routeGuard(page, "memory-tab-click-during-failed-refresh");
    await routes.route("**/console/experience", async route => {
      const response = await route.fetch(); const value = await response.json();
      await route.fulfill({ response, json: { ...value, memory: { can_read: true, can_review_quarantine: false } } });
    });
    await routes.route("**/console/rpc", async route => {
      const request = route.request().postDataJSON();
      const reply = payload => route.fulfill({ json: { jsonrpc: "2.0", id: request.id, ...payload } });
      if (request.method === "meta/capabilities") {
        const response = await route.fetch(); const value = await response.json();
        value.result.methods.push(...Object.keys(replies));
        return route.fulfill({ response, json: value });
      }
      if (request.method === "meta/memory/panel/records" && failRecordReads) {
        failedReads.push(request.id);
        return reply({ error: { code: -32000, message: "memory store unavailable" } });
      }
      if (Object.hasOwn(replies, request.method)) return reply({ result: replies[request.method] });
      return route.continue();
    });
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.getByText("Memory", { exact: true }).first().click();
    await page.getByTestId("memory-panel").waitFor();
    const memoryError = page.getByTestId("memory-error");
    await memoryError.waitFor({ state: "detached" });
    const tab = page.getByTestId("memory-tab:records");
    assert.equal(await tab.evaluate(element => element.classList.contains("is-active")), false);
    const target = await tab.boundingBox();
    await page.mouse.move(target.x + target.width / 2, target.y + target.height / 2);
    await page.mouse.down();
    // Measured while pressed, so press styling is the same in both boxes.
    const pressed = await tab.boundingBox();
    failRecordReads = true;
    await page.getByTestId("memory-refresh").evaluate(button => button.click());
    await memoryError.waitFor();
    const erroredBox = await tab.boundingBox();
    await page.screenshot({ path: path.join(evidence, "pressed-after-failed-refresh.png"), fullPage: true });
    await page.mouse.up();
    const selected = await tab.evaluate(element => element.classList.contains("is-active"));
    checkpoints.push({ step: "released-after-failed-refresh", pressed, erroredBox, selected, failedReads: [...failedReads] });
    failRecordReads = false;
    assert.deepEqual(erroredBox, pressed, "a failed refresh moved the tab row under a pressed pointer");
    assert.equal(selected, true, "a tab click that spans a failed refresh did not select the tab");
    await page.getByTestId("memory-refresh").click();
    await memoryError.waitFor({ state: "detached" });
    await routes.settle();
    routes.check();
    assert.deepEqual(errors, []);
    checkpoints.push({ step: "complete" });
  } catch (error) {
    printScenarioError("memory-tab-click-during-failed-refresh", error);
    failure = String(error.stack || error);
    if (page) fs.writeFileSync(path.join(evidence, "failure-body.txt"), await page.locator("body").innerText().catch(() => "unavailable"));
    throw error;
  } finally {
    await routes?.settle();
    fs.writeFileSync(path.join(evidence, "result.json"), JSON.stringify({ backend: "mock HTTP memory RPC", checkpoints, errors, routeFailures: routes?.failures || [], failure: failure || null }, null, 2));
    if (browser) await browser.close();
    await server.close();
  }
}

// A WorkGraph refresh that fails shows its error while a click on a work item
// action may be in flight, and keeps the last items on screen. The error must
// not move those items under a pressed pointer, or the release lands on
// another element and the action never runs.
async function runWorkGraphActionClickDuringFailedRefreshProof() {
  const evidence = path.join(process.env.META_BROWSER_EVIDENCE || path.join(repoRoot, "output/playwright/console-acceptance"), "workgraph-action-click-during-failed-refresh");
  fs.mkdirSync(evidence, { recursive: true });
  const snapshot = {
    items: [{ id: "item-1", title: "Investigate the outage", status: "open", revision: 1, created_at: "2026-10-18T00:00:00Z" }],
    edges: [], attention: [], captured_at: "2026-10-18T00:00:00Z", event_high_water_mark: 0,
  };
  const methods = ["meta/workgraph/snapshot", "meta/workgraph/events", "meta/workgraph/claim"];
  let failSnapshotReads = false;
  const claims = [], failedReads = [], errors = [], checkpoints = [];
  const server = await startMockConsoleServer();
  let browser, page, routes, failure;
  try {
    browser = await launchBrowser();
    page = await browser.newPage({ viewport: { width: 1280, height: 960 } });
    page.setDefaultTimeout(10000);
    page.on("pageerror", error => errors.push(String(error)));
    routes = routeGuard(page, "workgraph-action-click-during-failed-refresh");
    await routes.route("**/console/experience", async route => {
      const response = await route.fetch(); const value = await response.json();
      await route.fulfill({ response, json: { ...value, workgraph: { available: true, can_view: true, can_manage: true } } });
    });
    await routes.route("**/console/rpc", async route => {
      const request = route.request().postDataJSON();
      const reply = payload => route.fulfill({ json: { jsonrpc: "2.0", id: request.id, ...payload } });
      if (request.method === "meta/capabilities") {
        const response = await route.fetch(); const value = await response.json();
        value.result.methods.push(...methods);
        return route.fulfill({ response, json: value });
      }
      if (request.method === "meta/workgraph/snapshot") {
        if (failSnapshotReads) {
          failedReads.push(request.id);
          return reply({ error: { code: -32000, message: "workgraph store unavailable" } });
        }
        return reply({ result: snapshot });
      }
      if (request.method === "meta/workgraph/events") return reply({ result: { events: [] } });
      if (request.method === "meta/workgraph/claim") {
        claims.push(request.params);
        return reply({ result: { item: { ...snapshot.items[0], status: "claimed", revision: 2 } } });
      }
      return route.continue();
    });
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.getByText("WorkGraph", { exact: true }).first().click();
    await page.getByTestId("workgraph-panel").waitFor();
    const refreshError = page.getByTestId("workgraph-panel").getByRole("alert");
    const action = page.getByTestId("workgraph-panel-action:item-1:claim");
    await action.waitFor();
    assert.equal(await refreshError.count(), 0, "no refresh error before the held click");
    const target = await action.boundingBox();
    await page.mouse.move(target.x + target.width / 2, target.y + target.height / 2);
    await page.mouse.down();
    // Measured while pressed, so press styling is the same in both boxes.
    const pressed = await action.boundingBox();
    const head = page.getByTestId("workgraph-panel").locator(".workgraph__head");
    const headPressed = await head.boundingBox();
    failSnapshotReads = true;
    await page.getByTestId("workgraph-panel-refresh").evaluate(button => button.click());
    await refreshError.waitFor();
    const erroredBox = await action.boundingBox();
    const headErrored = await head.boundingBox();
    const errorBox = await refreshError.boundingBox();
    await page.screenshot({ path: path.join(evidence, "pressed-after-failed-refresh.png"), fullPage: true });
    failSnapshotReads = false;
    const claimed = page.waitForRequest(request => request.url().endsWith("/console/rpc")
      && request.method() === "POST" && request.postDataJSON()?.method === "meta/workgraph/claim", { timeout: 3000 })
      .then(() => true, () => false);
    await page.mouse.up();
    const claimSent = await claimed;
    checkpoints.push({ step: "released-after-failed-refresh", pressed, erroredBox, headPressed, headErrored, errorBox, claimSent, claims: [...claims], failedReads: [...failedReads] });
    assert.deepEqual(erroredBox, pressed, "a failed refresh moved the work item action under a pressed pointer");
    assert.equal(claimSent, true, "a click on a work item action that spans a failed refresh did not run the action");
    await routes.settle();
    routes.check();
    assert.deepEqual(errors, []);
    checkpoints.push({ step: "complete" });
  } catch (error) {
    printScenarioError("workgraph-action-click-during-failed-refresh", error);
    failure = String(error.stack || error);
    if (page) fs.writeFileSync(path.join(evidence, "failure-body.txt"), await page.locator("body").innerText().catch(() => "unavailable"));
    throw error;
  } finally {
    await routes?.settle();
    fs.writeFileSync(path.join(evidence, "result.json"), JSON.stringify({ backend: "mock HTTP workgraph RPC", checkpoints, errors, routeFailures: routes?.failures || [], failure: failure || null }, null, 2));
    if (browser) await browser.close();
    await server.close();
  }
}

// A failed Memory refresh shows its error, and the next good refresh clears
// it, while a click on the panel body may be in flight. Neither may move the
// body under a pressed pointer, or the release lands on another element and
// the click is lost.
async function runMemoryBodyClickAcrossRefreshProof() {
  const evidence = path.join(process.env.META_BROWSER_EVIDENCE || path.join(repoRoot, "output/playwright/console-acceptance"), "memory-body-click-across-refresh");
  fs.mkdirSync(evidence, { recursive: true });
  const record = {
    id: "memory-1", title: "Release checklist", kind: "fact", trust: "agent_verified",
    status: { status: "active" }, scope: { scope: "identity", realm: "default", identity: "router:main" },
    updated_at_ms: 1792368000000,
  };
  const replies = {
    "meta/memory/panel/records": { records: [record], realms: ["default"] },
    "meta/memory/panel/record": { realm: "default", record: { ...record, body: "Check the release sheet." }, chain: [], injections: [] },
    "meta/memory/panel/dreams": { runs: [] },
    "meta/memory/panel/overview": null,
    "meta/memory/panel/proposals": { proposals: [] },
    "meta/memory/panel/injections": { injections: [] },
    "meta/memory/panel/harvests": { harvests: [] },
    "meta/memory/panel/dream_runs": { runs: [] },
    "meta/memory/panel/audit_verdicts": { verdicts: [] },
  };
  let failRecordReads = false;
  const failedReads = [], errors = [], checkpoints = [], problems = [];
  const server = await startMockConsoleServer();
  let browser, page, routes, failure;
  try {
    browser = await launchBrowser();
    page = await browser.newPage({ viewport: { width: 1280, height: 960 } });
    page.setDefaultTimeout(10000);
    page.on("pageerror", error => errors.push(String(error)));
    routes = routeGuard(page, "memory-body-click-across-refresh");
    await routes.route("**/console/experience", async route => {
      const response = await route.fetch(); const value = await response.json();
      await route.fulfill({ response, json: { ...value, memory: { can_read: true, can_review_quarantine: false } } });
    });
    await routes.route("**/console/rpc", async route => {
      const request = route.request().postDataJSON();
      const reply = payload => route.fulfill({ json: { jsonrpc: "2.0", id: request.id, ...payload } });
      if (request.method === "meta/capabilities") {
        const response = await route.fetch(); const value = await response.json();
        value.result.methods.push(...Object.keys(replies));
        return route.fulfill({ response, json: value });
      }
      // Only the refresh's own unfiltered read fails. The lattice page-walk
      // pages with a limit, and a walk failing would change the tile.
      const plainRead = !request.params || Object.keys(request.params).length === 0;
      if (request.method === "meta/memory/panel/records" && failRecordReads && plainRead) {
        failedReads.push(request.id);
        return reply({ error: { code: -32000, message: "memory store unavailable" } });
      }
      if (Object.hasOwn(replies, request.method)) return reply({ result: replies[request.method] });
      return route.continue();
    });
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.getByText("Memory", { exact: true }).first().click();
    await page.getByTestId("memory-panel").waitFor();
    const memoryError = page.getByTestId("memory-error");
    const body = page.locator(".memory-panel__body");
    const refresh = page.getByTestId("memory-refresh");
    // Presses the target, runs one refresh that fails or succeeds, and
    // releases once its error has appeared or cleared. Boxes are measured
    // while pressed, so press styling is the same in both.
    // The pressed control's top-left and the body's box must hold. A tile's
    // height may follow a sibling tile in its grid row (the lattice tile adds
    // a "re-checking" line while its page-walk runs), which leaves the
    // pointer on the same tile.
    const placement = async target => {
      const box = await target.boundingBox();
      return { x: box.x, y: box.y, width: box.width };
    };
    async function pressAcrossRefresh(step, target, failing, landedAfterRelease) {
      const box = await target.boundingBox();
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
      await page.mouse.down();
      const pressed = { target: await placement(target), body: await body.boundingBox() };
      failRecordReads = failing;
      await refresh.evaluate(button => button.click());
      await (failing ? memoryError.waitFor() : memoryError.waitFor({ state: "detached" }));
      const settled = { target: await placement(target), body: await body.boundingBox() };
      await page.screenshot({ path: path.join(evidence, `${step}.png`), fullPage: true });
      const landed = landedAfterRelease();
      await page.mouse.up();
      failRecordReads = false;
      const entry = { step, pressed, settled, landed: await landed };
      checkpoints.push(entry);
      if (JSON.stringify(entry.settled) !== JSON.stringify(entry.pressed)) problems.push(`${step}: the panel body moved under a pressed pointer`);
      if (!entry.landed) problems.push(`${step}: the click was lost`);
      return entry.landed;
    }
    // Each press starts once the lattice page-walk has settled.
    const latticeSettled = () => page.waitForFunction(() => {
      const tile = document.querySelector('[data-testid="memory-verdict:lattice"]');
      return tile?.getAttribute("data-status") === "holding" && !/running|re-checking/.test(tile.textContent || "");
    });
    const knowledgeOpened = () => page.waitForFunction(
      () => document.querySelector('[data-testid="memory-tab:knowledge"]')?.classList.contains("is-active"),
      null, { timeout: 3000 }).then(() => true, () => false);
    const recordRequested = () => page.waitForRequest(request => request.url().endsWith("/console/rpc")
      && request.method() === "POST" && request.postDataJSON()?.method === "meta/memory/panel/record", { timeout: 3000 })
      .then(() => true, () => false);

    await memoryError.waitFor({ state: "detached" });
    const tile = page.locator('[data-testid^="memory-verdict:"]').first();
    await tile.waitFor();
    await latticeSettled();
    await pressAcrossRefresh("verdict-tile-across-failed-refresh", tile, true, knowledgeOpened);
    await page.getByTestId("memory-tab:holdings").click();
    await memoryError.waitFor();
    await latticeSettled();
    await pressAcrossRefresh("verdict-tile-across-recovery", tile, false, knowledgeOpened);

    await page.getByTestId("memory-tab:records").click();
    const row = page.getByTestId("memory-record:memory-1");
    await row.waitFor();
    if (await pressAcrossRefresh("record-row-across-failed-refresh", row, true, recordRequested)) {
      await page.getByTestId("memory-detail").waitFor();
      await page.getByTestId("memory-detail-back").click();
    }
    await row.waitFor();
    // Opening a record clears the panel error, so fail one refresh again
    // before pressing across the recovery.
    failRecordReads = true;
    await refresh.click();
    await memoryError.waitFor();
    failRecordReads = false;
    await pressAcrossRefresh("record-row-across-recovery", row, false, recordRequested);

    assert.deepEqual(problems, [], "a refresh that failed or recovered moved the memory body under a click");
    await routes.settle();
    routes.check();
    assert.deepEqual(errors, []);
    checkpoints.push({ step: "complete" });
  } catch (error) {
    printScenarioError("memory-body-click-across-refresh", error);
    failure = String(error.stack || error);
    if (page) fs.writeFileSync(path.join(evidence, "failure-body.txt"), await page.locator("body").innerText().catch(() => "unavailable"));
    throw error;
  } finally {
    await routes?.settle();
    fs.writeFileSync(path.join(evidence, "result.json"), JSON.stringify({ backend: "mock HTTP memory RPC", checkpoints, problems, failedReads, errors, routeFailures: routes?.failures || [], failure: failure || null }, null, 2));
    if (browser) await browser.close();
    await server.close();
  }
}

// The Memory lattice tile page-walks the loaded records. A good refresh that
// brings the same records must not walk again, and a walk that does run must
// show its re-check without growing the tile row: either would move the tiles
// below it, and with them a click in progress on a lower tile.
async function runMemoryLatticeRecheckAcrossRefreshProof() {
  const evidence = path.join(process.env.META_BROWSER_EVIDENCE || path.join(repoRoot, "output/playwright/console-acceptance"), "memory-lattice-recheck-across-refresh");
  fs.mkdirSync(evidence, { recursive: true });
  const record = id => ({
    id, title: `Release checklist ${id}`, kind: "fact", trust: "agent_verified",
    status: { status: "active" }, scope: { scope: "identity", realm: "default", identity: "router:main" },
    updated_at_ms: 1792368000000,
  });
  let records = [record("memory-1")];
  const replies = {
    "meta/memory/panel/dreams": { runs: [] },
    "meta/memory/panel/overview": null,
    "meta/memory/panel/proposals": { proposals: [] },
    "meta/memory/panel/injections": { injections: [] },
    "meta/memory/panel/harvests": { harvests: [] },
    "meta/memory/panel/dream_runs": { runs: [] },
    "meta/memory/panel/audit_verdicts": { verdicts: [] },
  };
  const { eventually } = require("./acceptance-runtime.cjs");
  let holdWalkReads = false;
  const heldWalkReads = [], walkReads = [], plainReads = [], errors = [], checkpoints = [], problems = [];
  const server = await startMockConsoleServer();
  let browser, page, routes, failure;
  try {
    browser = await launchBrowser();
    page = await browser.newPage({ viewport: { width: 1280, height: 960 } });
    page.setDefaultTimeout(10000);
    page.on("pageerror", error => errors.push(String(error)));
    routes = routeGuard(page, "memory-lattice-recheck-across-refresh");
    await routes.route("**/console/experience", async route => {
      const response = await route.fetch(); const value = await response.json();
      await route.fulfill({ response, json: { ...value, memory: { can_read: true, can_review_quarantine: false } } });
    });
    await routes.route("**/console/rpc", async route => {
      const request = route.request().postDataJSON();
      const reply = payload => route.fulfill({ json: { jsonrpc: "2.0", id: request.id, ...payload } });
      if (request.method === "meta/capabilities") {
        const response = await route.fetch(); const value = await response.json();
        value.result.methods.push("meta/memory/panel/records", ...Object.keys(replies));
        return route.fulfill({ response, json: value });
      }
      if (request.method === "meta/memory/panel/records") {
        const params = request.params || {};
        // The page-walk pages with a limit; the scope probes carry a scope.
        if (params.limit !== undefined && params.scope === undefined) {
          walkReads.push(request.id);
          if (holdWalkReads) await new Promise(resolve => heldWalkReads.push(resolve));
        } else if (Object.keys(params).length === 0) {
          plainReads.push(request.id);
        }
        return reply({ result: { records, realms: ["default"] } });
      }
      if (Object.hasOwn(replies, request.method)) return reply({ result: replies[request.method] });
      return route.continue();
    });
    const releaseWalkReads = () => { holdWalkReads = false; heldWalkReads.splice(0).forEach(resolve => resolve()); };
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.getByText("Memory", { exact: true }).first().click();
    await page.getByTestId("memory-panel").waitFor();
    const lattice = page.getByTestId("memory-verdict:lattice");
    const lowerTile = page.getByTestId("memory-verdict:dreams");
    const refresh = page.getByTestId("memory-refresh");
    const latticeSettled = () => page.waitForFunction(() => {
      const tile = document.querySelector('[data-testid="memory-verdict:lattice"]');
      return tile?.getAttribute("data-status") === "holding" && !/running|re-checking/.test(tile.textContent || "");
    });
    const dreamsOpened = () => page.waitForFunction(
      () => document.querySelector('[data-testid="memory-tab:dreams"]')?.classList.contains("is-active"),
      null, { timeout: 3000 }).then(() => true, () => false);
    // Presses the lower tile, runs one good refresh with walk reads held,
    // measures once the refresh has landed, then releases.
    async function pressAcrossGoodRefresh(step, afterRefresh) {
      await page.getByTestId("memory-tab:holdings").click();
      await latticeSettled();
      const walkReadsBefore = walkReads.length;
      const plainReadsBefore = plainReads.length;
      const box = await lowerTile.boundingBox();
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
      await page.mouse.down();
      const pressed = await lowerTile.boundingBox();
      holdWalkReads = true;
      await refresh.evaluate(button => button.click());
      await eventually(async () => plainReads.length > plainReadsBefore, `${step}: the refresh read records`);
      await afterRefresh();
      const settled = await lowerTile.boundingBox();
      const latticeText = (await lattice.innerText()).replace(/\s+/g, " ");
      await page.screenshot({ path: path.join(evidence, `${step}.png`), fullPage: true });
      const landed = dreamsOpened();
      await page.mouse.up();
      const entry = { step, pressed, settled, latticeText, walkReads: walkReads.length - walkReadsBefore, landed: await landed };
      releaseWalkReads();
      checkpoints.push(entry);
      if (JSON.stringify(settled) !== JSON.stringify(pressed)) problems.push(`${step}: the lower tiles moved under a pressed pointer`);
      if (!entry.landed) problems.push(`${step}: the click was lost`);
      return entry;
    }

    await page.getByTestId("memory-verdict-strip").waitFor();
    await latticeSettled();
    // The same records again: the settled verdict already covers them.
    const unchanged = await pressAcrossGoodRefresh("unchanged-records", () => page.waitForTimeout(400));
    if (unchanged.walkReads !== 0) problems.push(`unchanged-records: the refresh walked the same records again (${unchanged.walkReads} walk reads)`);
    // New records: the walk runs, held here, and shows its re-check.
    records = [record("memory-1"), record("memory-2")];
    const changed = await pressAcrossGoodRefresh("changed-records", async () => {
      await eventually(async () => heldWalkReads.length > 0, "changed-records: the walk started");
      await eventually(async () => /re-checking/.test(await lattice.innerText()), "changed-records: the re-check shows");
    });
    if (!/re-checking/.test(changed.latticeText)) problems.push("changed-records: the running walk showed no re-check");

    assert.deepEqual(problems, [], "a good refresh moved the memory tiles under a click");
    await routes.settle();
    routes.check();
    assert.deepEqual(errors, []);
    checkpoints.push({ step: "complete" });
  } catch (error) {
    printScenarioError("memory-lattice-recheck-across-refresh", error);
    failure = String(error.stack || error);
    if (page) fs.writeFileSync(path.join(evidence, "failure-body.txt"), await page.locator("body").innerText().catch(() => "unavailable"));
    throw error;
  } finally {
    holdWalkReads = false;
    heldWalkReads.splice(0).forEach(resolve => resolve());
    await routes?.settle();
    fs.writeFileSync(path.join(evidence, "result.json"), JSON.stringify({ backend: "mock HTTP memory RPC", checkpoints, problems, walkReads: walkReads.length, plainReads: plainReads.length, errors, routeFailures: routes?.failures || [], failure: failure || null }, null, 2));
    if (browser) await browser.close();
    await server.close();
  }
}

// At narrow widths the WorkGraph header has no room to spare. Its view toggle
// and Refresh must stay inside the panel, a failed refresh's error and the
// good refresh that clears it must not move them under a pressed pointer, and
// the error must get the panel's width (up to 160 px) so it stays readable.
const WORKGRAPH_MIN_READABLE_ERROR_WIDTH = 160;
async function runWorkGraphHeaderClickAcrossRefreshProof(width) {
  const evidence = path.join(process.env.META_BROWSER_EVIDENCE || path.join(repoRoot, "output/playwright/console-acceptance"), `workgraph-header-click-across-refresh-${width}`);
  fs.mkdirSync(evidence, { recursive: true });
  const snapshot = {
    items: [{ id: "item-1", title: "Investigate the outage", status: "open", revision: 1, created_at: "2026-10-18T00:00:00Z" }],
    edges: [], attention: [], captured_at: "2026-10-18T00:00:00Z", event_high_water_mark: 0,
  };
  const methods = ["meta/workgraph/snapshot", "meta/workgraph/events", "meta/workgraph/claim"];
  const longError = "workgraph store unavailable: the backing store refused the read because its connection pool was exhausted after repeated timeouts";
  let failSnapshotReads = false;
  const failedReads = [], errors = [], checkpoints = [], problems = [];
  const server = await startMockConsoleServer();
  let browser, page, routes, failure;
  try {
    browser = await launchBrowser();
    page = await browser.newPage({ viewport: { width, height: 900 } });
    page.setDefaultTimeout(10000);
    page.on("pageerror", error => errors.push(String(error)));
    routes = routeGuard(page, `workgraph-header-click-across-refresh-${width}`);
    await routes.route("**/console/experience", async route => {
      const response = await route.fetch(); const value = await response.json();
      await route.fulfill({ response, json: { ...value, workgraph: { available: true, can_view: true, can_manage: true } } });
    });
    await routes.route("**/console/rpc", async route => {
      const request = route.request().postDataJSON();
      const reply = payload => route.fulfill({ json: { jsonrpc: "2.0", id: request.id, ...payload } });
      if (request.method === "meta/capabilities") {
        const response = await route.fetch(); const value = await response.json();
        value.result.methods.push(...methods);
        return route.fulfill({ response, json: value });
      }
      if (request.method === "meta/workgraph/snapshot") {
        if (failSnapshotReads) {
          failedReads.push(request.id);
          return reply({ error: { code: -32000, message: longError } });
        }
        return reply({ result: snapshot });
      }
      if (request.method === "meta/workgraph/events") return reply({ result: { events: [] } });
      return route.continue();
    });
    await gotoConsole(page, `${server.baseUrl}/console`);
    await page.getByText("WorkGraph", { exact: true }).first().click();
    await page.getByTestId("workgraph-panel").waitFor();
    await page.getByTestId("workgraph-panel-action:item-1:claim").waitFor();
    const refreshError = page.getByTestId("workgraph-panel").getByRole("alert");
    const tree = page.getByTestId("workgraph-view-toggle:tree");
    const graph = page.getByTestId("workgraph-view-toggle:graph");
    const refresh = page.getByTestId("workgraph-panel-refresh");
    assert.equal(await refreshError.count(), 0, "no refresh error before the held click");
    const controls = async () => ({ tree: await tree.boundingBox(), graph: await graph.boundingBox(), refresh: await refresh.boundingBox() });
    const panel = page.getByTestId("workgraph-panel");
    const panelBox = await panel.boundingBox();
    const contentWidth = await panel.evaluate(element => {
      const style = getComputedStyle(element);
      return element.clientWidth - parseFloat(style.paddingLeft) - parseFloat(style.paddingRight);
    });
    const readableWidth = Math.min(WORKGRAPH_MIN_READABLE_ERROR_WIDTH, contentWidth);
    const initial = await controls();
    checkpoints.push({ step: "initial", panelBox, contentWidth, controls: initial });
    for (const [name, box] of Object.entries(initial)) {
      if (box.x < panelBox.x || box.x + box.width > panelBox.x + panelBox.width) {
        problems.push(`${name} sits outside the visible panel, so it cannot be clicked without scrolling`);
      }
    }
    // Presses a view toggle, runs one refresh that fails or succeeds, and
    // releases once its error has appeared or cleared. Boxes are measured
    // while pressed, so press styling is the same in both.
    async function pressAcrossRefresh(step, toggle, failing) {
      // Scrolled into view first, so a control the header pushed out of the
      // panel is still pressed where it is.
      await toggle.scrollIntoViewIfNeeded();
      const box = await toggle.boundingBox();
      await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
      await page.mouse.down();
      const pressed = await controls();
      failSnapshotReads = failing;
      await refresh.evaluate(button => button.click());
      await (failing ? refreshError.waitFor() : refreshError.waitFor({ state: "detached" }));
      const settled = await controls();
      const errorBox = failing ? await refreshError.boundingBox() : null;
      await page.screenshot({ path: path.join(evidence, `${step}.png`), fullPage: true });
      await page.mouse.up();
      failSnapshotReads = false;
      const landed = await page.waitForFunction(
        element => element.getAttribute("aria-pressed") === "true", await toggle.elementHandle(), { timeout: 3000 })
        .then(() => true, () => false);
      const entry = { step, pressed, settled, errorBox, landed };
      checkpoints.push(entry);
      if (JSON.stringify(settled) !== JSON.stringify(pressed)) problems.push(`${step}: the header controls moved under a pressed pointer`);
      if (!landed) problems.push(`${step}: the click was lost`);
      if (errorBox && errorBox.width < readableWidth - 1) {
        problems.push(`${step}: the refresh error is ${Math.round(errorBox.width)} px wide, below ${Math.round(readableWidth)} px`);
      }
    }
    await pressAcrossRefresh("graph-toggle-across-failed-refresh", graph, true);
    // The recovery press must change the view to count as landed, so it
    // starts from the graph view whether or not the first press landed.
    if ((await graph.getAttribute("aria-pressed")) !== "true") await graph.click();
    await pressAcrossRefresh("tree-toggle-across-recovery", tree, false);

    assert.deepEqual(problems, [], `a refresh that failed or recovered moved the WorkGraph header controls at ${width} px`);
    await routes.settle();
    routes.check();
    assert.deepEqual(errors, []);
    checkpoints.push({ step: "complete" });
  } catch (error) {
    printScenarioError(`workgraph-header-click-across-refresh-${width}`, error);
    failure = String(error.stack || error);
    if (page) fs.writeFileSync(path.join(evidence, "failure-body.txt"), await page.locator("body").innerText().catch(() => "unavailable"));
    throw error;
  } finally {
    await routes?.settle();
    fs.writeFileSync(path.join(evidence, "result.json"), JSON.stringify({ backend: "mock HTTP workgraph RPC", width, checkpoints, problems, failedReads, errors, routeFailures: routes?.failures || [], failure: failure || null }, null, 2));
    if (browser) await browser.close();
    await server.close();
  }
}

const scenarios = [
  ...["conflict", "group", "unavailable"].map(mode => ({ id: `checked-save-${mode}`, family: "console", backend: "mock", run: () => runCheckedSaveBrowserProof(mode) })),
  { id: "access-tab-click-during-refresh", family: "console", backend: "mock", run: runAccessTabClickDuringRefreshProof },
  { id: "access-edit-during-background-refresh", family: "console", backend: "mock", run: runAccessEditDuringBackgroundRefreshProof },
  { id: "memory-tab-click-during-failed-refresh", family: "console", backend: "mock", run: runMemoryTabClickDuringFailedRefreshProof },
  { id: "workgraph-action-click-during-failed-refresh", family: "console", backend: "mock", run: runWorkGraphActionClickDuringFailedRefreshProof },
  { id: "memory-body-click-across-refresh", family: "console", backend: "mock", run: runMemoryBodyClickAcrossRefreshProof },
  { id: "memory-lattice-recheck-across-refresh", family: "console", backend: "mock", run: runMemoryLatticeRecheckAcrossRefreshProof },
  ...[760, 560].map(width => ({ id: `workgraph-header-click-across-refresh-${width}`, family: "console", backend: "mock", run: () => runWorkGraphHeaderClickAcrossRefreshProof(width) })),
  { id: "authorization-feedback", family: "console", backend: "mock", run: runAuthorizationFeedbackProof },
  { id: "reference", family: "runtime", backend: "real", run: runReferenceBrowserProof },
  { id: "topology-unavailable", family: "topology", backend: "mock", run: runTopologyUnavailableBrowserProof },
  { id: "topology-denied-pair", family: "topology", backend: "mock", run: runTopologyDeniedPairBrowserProof },
  { id: "topology-ambiguous-commit", family: "topology", backend: "mock", run: runTopologyAmbiguousCommitBrowserProof },
  { id: "canonical-send", family: "console", backend: "mock", run: runCanonicalSendBrowserProof },
  { id: "image-rendering", family: "presentation", backend: "mock", run: runImageRenderingBrowserProof },
  { id: "composer-paste-attachment", family: "presentation", backend: "mock", run: runComposerPasteAttachmentProof },
  { id: "busy-worker-console", family: "console", backend: "mock", run: runBusyWorkerConsoleProof },
  { id: "tool-only-worker-busy-queue", family: "console", backend: "mock", run: runToolOnlyWorkerBusyQueueProof },
  { id: "tool-only-worker-terminal-clears-busy", family: "console", backend: "mock", run: runToolOnlyWorkerTerminalClearsBusyProof },
  { id: "non-comms-system-notice-does-not-clear-busy", family: "console", backend: "mock", run: runNonCommsSystemNoticeDoesNotClearBusyProof },
  { id: "sidebar-search-expands-collapsed-worker-section", family: "console", backend: "mock", run: runSidebarSearchExpandsCollapsedWorkerSectionProof },
  { id: "chat-pane-auto-scroll", family: "presentation", backend: "mock", run: runChatPaneAutoScrollProof },
  { id: "chat-pane-turn-rail", family: "presentation", backend: "mock", run: runChatPaneTurnRailProof },
  { id: "global-timeline-recent-seed", family: "console", backend: "mock", run: runGlobalTimelineRecentSeedProof },
  { id: "chat-pane-recent-first-page", family: "presentation", backend: "mock", run: runChatPaneRecentFirstPageProof },
  { id: "chat-pane-older-history-demand-paging", family: "presentation", backend: "mock", run: runChatPaneOlderHistoryDemandPagingProof },
  { id: "chat-pane-async-backfill-restores-older-history", family: "presentation", backend: "mock", run: runChatPaneAsyncBackfillRestoresOlderHistoryProof },
  { id: "chat-pane-non-empty-async-backfill-restores-older-history", family: "presentation", backend: "mock", run: runChatPaneNonEmptyAsyncBackfillRestoresOlderHistoryProof },
  { id: "chat-pane-replay-recovery-replaces-stale-local-log", family: "presentation", backend: "mock", run: runChatPaneReplayRecoveryReplacesStaleLocalLogProof },
  { id: "run-started-clears-optimistic-prompt", family: "console", backend: "mock", run: runRunStartedClearsOptimisticPromptProof },
  { id: "user-input-echo-clears-optimistic-prompt", family: "console", backend: "mock", run: runUserInputEchoClearsOptimisticPromptProof },
  { id: "live-system-notice-appears-in-open-chat", family: "console", backend: "mock", run: runLiveSystemNoticeAppearsInOpenChatProof },
  { id: "console-mounts-without-crypto-random-uuid", family: "console", backend: "mock", run: runConsoleMountsWithoutCryptoRandomUuidProof },
  { id: "headless-control-surface", family: "console", backend: "mock", run: runHeadlessControlSurfaceBrowserProof },
];

const allScenarios = [
    ...scenarios,
    ...require("./scenarios/settled-history-queue.cjs").scenarios,
    ...require("./scenarios/storage-lock.cjs").scenarios,
    ...require("./scenarios/real-conversation.cjs").scenarios,
    ...require("./scenarios/real-recovery-scope.cjs").scenarios,
    ...require("./scenarios/real-markdown-url-policy.cjs").scenarios,
    ...require("./scenarios/real-reasoning.cjs").scenarios,
    ...require("./scenarios/real-assistant-identity.cjs").scenarios,
    ...require("./scenarios/real-startup-lineage.cjs").scenarios,
    ...require("./scenarios/real-routine-tools.cjs").browserScenarios,
    ...require("./scenarios/approval-lifecycle.cjs").browserScenarios,
    ...require("./scenarios/real-checked-save.cjs").scenarios,
    ...require("./scenarios/real-workgraph.cjs").browserScenarios,
    ...require("./scenarios/real-images.cjs").browserScenarios,
    ...require("./scenarios/real-send-context.cjs").scenarios,
    ...require("./scenarios/real-durable-steer.cjs").scenarios,
    ...require("./scenarios/real-tab-isolation.cjs").scenarios,
    ...require("./scenarios/real-legacy-import.cjs").scenarios,
    ...require("./scenarios/real-sidebar-activity.cjs").scenarios,
    ...require("./scenarios/operator-panel-frame.cjs").scenarios,
  ];

module.exports = { scenarios: allScenarios };
if (require.main === module) {
  require("./scenario-registry.cjs").runScenarios(allScenarios).catch((error) => {
    process.stderr.write(`${error.stack || error.message}\n`);
    process.exit(1);
  });
}
