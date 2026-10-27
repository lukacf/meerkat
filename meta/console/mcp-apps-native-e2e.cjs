#!/usr/bin/env node
"use strict";

// Real stock Console -> native session admission -> physical stdio MCP server.
// Uses a prebuilt native backend and the current production UI.
// META_MCP_APPS_LONG_POLL=1 adds three live App pollers during a >65s model hold.
//
// META_MCP_APPS_BACKEND selects the native backend:
// - gateway (default): the mcp_apps_gateway_fixture example runs the persistent
//   rpc_gateway composition, so members are built by the gateway's own member
//   factories and their MCP routers carry the stock connector call-context
//   provider every gateway deployment uses. The harness drives it over stdin
//   JSON-RPC like an SDK: meta/init, then meta/spawn_member.
// - builder: console_acceptance_fixture, a UnifiedRuntime::builder() host whose
//   member routers carry no call-context provider.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const http = require("node:http");
const path = require("node:path");
const { spawn } = require("node:child_process");
const { build } = require("esbuild");
const { chromium } = require("playwright");
const { exampleBackendSpec } = require("./example-backend.cjs");
const { LOOPBACK_ANY_PORT, awaitFixtureReady } = require("./fixture-ready.cjs");
const { eventually, rpc } = require("./acceptance-runtime.cjs");
const { createSandboxServer } = require("./mcp-apps-sandbox.cjs");

async function listen(server) {
  await new Promise((resolve, reject) => { server.once("error", reject); server.listen(0, "127.0.0.1", resolve); });
  return `http://127.0.0.1:${server.address().port}`;
}
async function closeServer(server) {
  server?.closeAllConnections();
  if (server?.listening) await new Promise(resolve => server.close(resolve));
}
async function stop(child) {
  if (!child || child.exitCode !== null || child.signalCode !== null) return;
  child.kill("SIGTERM");
  await new Promise(resolve => {
    const timer = setTimeout(() => { child.kill("SIGKILL"); resolve(); }, 3_000);
    child.once("exit", () => { clearTimeout(timer); resolve(); });
  });
}
// One JSON-RPC request per stdin line; responses arrive on stdout by id.
function stdioRpc(child) {
  const pending = new Map();
  let buffer = "";
  let next = 0;
  let exit = null;
  child.once("exit", (code, signal) => {
    exit = { code, signal };
    for (const waiter of pending.values()) waiter.fail(new Error(`gateway exited (code ${code}, signal ${signal}) before answering ${waiter.method}`));
    pending.clear();
  });
  child.stdout.setEncoding("utf8");
  child.stdout.on("data", chunk => {
    buffer += chunk;
    let end;
    while ((end = buffer.indexOf("\n")) >= 0) {
      const line = buffer.slice(0, end).trim();
      buffer = buffer.slice(end + 1);
      let message;
      try { message = JSON.parse(line); } catch { continue; }
      const waiter = typeof message?.id === "string" ? pending.get(message.id) : undefined;
      if (!waiter) continue;
      pending.delete(message.id);
      if (message.error) waiter.fail(new Error(`${waiter.method} failed: ${JSON.stringify(message.error)}`));
      else waiter.done(message.result);
    }
  });
  return {
    request(method, params, timeoutMs) {
      if (exit) return Promise.reject(new Error(`gateway exited (code ${exit.code}, signal ${exit.signal}) before ${method}`));
      const id = `harness-${++next}`;
      return new Promise((resolve, reject) => {
        const timer = setTimeout(() => {
          pending.delete(id);
          reject(new Error(`${method} got no answer within ${timeoutMs} ms`));
        }, timeoutMs);
        pending.set(id, {
          method,
          done: value => { clearTimeout(timer); resolve(value); },
          fail: error => { clearTimeout(timer); reject(error); },
        });
        child.stdin.write(`${JSON.stringify({ jsonrpc: "2.0", id, method, params })}\n`);
      });
    },
  };
}

// The acceptance fixture's lead profile, with the stdio MCP Apps server as its
// own declarative MCP server.
function gatewayMobToml({ node, server, html, log }) {
  const string = value => JSON.stringify(value);
  return [
    "[mob]", 'id = "mcp-apps-gateway"', "",
    "[profiles.lead]", 'model = "gpt-5.5"', "external_addressable = true", "",
    "[profiles.lead.tools]", "comms = true", "",
    "[[profiles.lead.tools.mcp_servers]]", 'name = "apps-fixture"',
    `command = ${string(node)}`, `args = [${string(server)}]`, "",
    "[profiles.lead.tools.mcp_servers.env]",
    `META_FIXTURE_APP_HTML = ${string(html)}`, `META_FIXTURE_APP_LOG = ${string(log)}`, "",
  ].join("\n");
}

async function post(base, endpoint, body) {
  const response = await fetch(base + endpoint, {
    method: "POST", headers: { "content-type": "application/json" }, body: JSON.stringify(body),
  });
  return { status: response.status, body: await response.json(), cache: response.headers.get("cache-control") };
}

async function exerciseLivePolling({ baseUrl, fixture, page, apps, requests, evidence }) {
  const views = ['display', 'display-two', 'display-three'];
  const barrierCommand = action => post(baseUrl, `${fixture}/model-barrier`, { action, id: 'apps-live-polling' });
  const readStates = () => Promise.all(apps.map(app => app.locator('#native-poll-status').getAttribute('data-state').then(JSON.parse)));
  const assertHeld = async () => {
    const status = await barrierCommand('status');
    assert.equal(status.status, 200);
    assert.equal(status.body.released, false, 'the test must not release the model while exercising live App calls');
    assert.equal(status.body.waiting_requests, 1, 'the real model stream must remain alive and blocked');
    return status.body;
  };
  await eventually(async () => {
    const status = await barrierCommand('status');
    return status.body.waiting_requests === 1;
  }, 'final native model request enters its barrier', 15_000);
  const holdStartedAt = Date.now();
  for (const [index, view] of views.entries()) {
    await apps[index].getByRole('heading', { name: `Records ${view}`, exact: true }).waitFor({ timeout: 15_000 });
  }
  assert.equal(await page.locator('iframe[title="Interactive tool result"]').count(), 3,
    'three independent stock App views must be mounted simultaneously');
  const modelBefore = await (await fetch(`${baseUrl}${fixture}/requests`)).json();
  // Activate the App's buttons as DOM clicks. A coordinate click two iframes
  // deep can land while the console lays out three Apps that mounted at once,
  // and never reach the button (main run 39023555191); the rendering and
  // reload step still clicks Refresh with a real pointer.
  const refreshStartedAt = Date.now();
  await apps[0].getByRole('button', { name: 'Refresh result', exact: true }).dispatchEvent('click');
  await apps[0].getByText('8 matching records', { exact: true }).waitFor({ timeout: 5_000 }).catch(async (error) => {
    const [state] = await readStates().catch(() => [undefined]);
    const calls = requests.filter(request => request.path === '/console/mcp-apps/call-tool').length;
    throw new Error(`${error.message}\nRefresh handler ran ${state?.refreshActivations ?? 'an unknown number of'} time(s); App errors ${JSON.stringify(state?.errors ?? null)}; ${calls} call-tool request(s)`);
  });
  const refreshMs = Date.now() - refreshStartedAt;
  assert(refreshMs < 5_000, 'a fresh App action must not wait for the active model turn');
  await assertHeld();
  for (const app of apps) await app.getByRole('button', { name: 'Start polling', exact: true }).dispatchEvent('click');
  const samples = [];
  // Cross the stock HTTP action timeout while each App independently polls at
  // two-second intervals. Every response has its own bounded request deadline.
  while (Date.now() - holdStartedAt < 66_000) {
    await new Promise(resolve => setTimeout(resolve, 2_000));
    const states = await readStates();
    for (const [index, state] of states.entries()) {
      assert.equal(state.viewId, views[index]);
      assert.deepEqual(state.errors, [], `native poller ${views[index]} must remain healthy`);
      assert.equal(state.polling, true, `native poller ${views[index]} must remain active`);
      assert(state.completions.every(call => call.latencyMs < 5_000), 'polls must not queue behind the model turn');
    }
    const barrier = await assertHeld();
    samples.push({ atMs: Date.now(), barrier, states });
    await fs.writeFile(path.join(evidence, 'live-polling-progress.json'), JSON.stringify(samples, null, 2));
  }
  for (const app of apps) await app.getByRole('button', { name: 'Stop polling', exact: true }).dispatchEvent('click');
  const finalStates = await eventually(async () => {
    const states = await readStates();
    return states.every(state => !state.inFlight) ? states : null;
  }, 'all already-admitted native polls finish', 6_000);
  const holdFinishedAt = Date.now();
  for (const state of finalStates) {
    assert.deepEqual(state.errors, []);
    assert(state.completions.length >= 25, 'each mounted App must keep polling throughout the long model turn');
    assert(state.completions.some(call => call.startedAt > holdStartedAt + 60_000), 'each App must complete a fresh poll after sixty seconds');
    for (let index = 1; index < state.completions.length; index++) {
      assert(state.completions[index].startedAt - state.completions[index - 1].startedAt < 8_000,
        'polling must not silently stop and recover only after the model completes');
    }
  }
  await assertHeld();
  const modelDuring = await (await fetch(`${baseUrl}${fixture}/requests`)).json();
  assert.deepEqual(modelDuring, modelBefore, 'App polling must not trigger model turns or change the in-flight model request');
  const actionRequests = requests.filter(request => request.path === '/console/mcp-apps/call-tool' && request.atMs >= holdStartedAt);
  const pollRequests = actionRequests.filter(request => JSON.parse(request.body).name === 'poll');
  assert.equal(pollRequests.length, finalStates.reduce((total, state) => total + state.completions.length, 0),
    'each successful App poll must correspond to exactly one native HTTP action');
  assert(actionRequests.every(request => request.status === 200 && request.finishedAt <= holdFinishedAt),
    'all native App responses must complete before releasing the model');
  await page.screenshot({ path: path.join(evidence, 'stock-native-app-polling-live.png'), fullPage: true });
  const released = await barrierCommand('release');
  assert.equal(released.status, 200);
  await page.locator('[data-testid^="chat-turn:router:main:"] .cc-markdown-document')
    .getByText('The MCP App is ready.', { exact: true }).waitFor({ timeout: 15_000 });

  // A subsequent real model request reveals the canonical conversation after
  // polling; neither App action output nor private metadata belongs in it.
  const configured = await post(baseUrl, `${fixture}/model`, { source: 'Follow-up complete.', delay_ms: 0, chunk_chars: 64 });
  assert.equal(configured.status, 200);
  await page.getByTestId('chat-composer:router:main').fill('Summarize the records already shown.');
  await page.locator('button:has-text("Send"):visible, .cc-composer__send-btn:visible').first().click();
  await page.locator('[data-testid^="chat-turn:router:main:"] .cc-markdown-document')
    .getByText('Follow-up complete.', { exact: true }).waitFor({ timeout: 15_000 });
  const modelAfter = await (await fetch(`${baseUrl}${fixture}/requests`)).json();
  assert(modelAfter.length > modelBefore.length, 'privacy check must include a fresh post-poll model request');
  for (const marker of ['PRIVATE_POLL_DETAIL', 'APP_POLL_RESULT_', 'APP_REFRESH_RESULT_']) {
    assert(!JSON.stringify(modelAfter).includes(marker), `${marker} must not enter later model requests`);
  }
  const result = { holdStartedAt, holdFinishedAt, heldMs: holdFinishedAt - holdStartedAt, refreshMs,
    widgets: finalStates, pollActions: pollRequests.length, modelRequestsBefore: modelBefore.length,
    modelRequestsAfter: modelAfter.length, persistenceWrites: {
      measured: false, reason: 'requires a separate native instrumented-store regression',
    } };
  await fs.writeFile(path.join(evidence, 'live-polling.json'), JSON.stringify(result, null, 2));
  return result;
}

async function main() {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Set META_EXAMPLE_BIN_DIR to the prebuilt native backend examples; this harness never starts Cargo.");
  const backend = process.env.META_MCP_APPS_BACKEND || "gateway";
  assert(["gateway", "builder"].includes(backend), `META_MCP_APPS_BACKEND must be gateway or builder, not ${backend}`);
  // Where the backend serves the scripted model's controls.
  const fixture = backend === "gateway" ? "/host/__fixture" : "/__fixture";
  const repoRoot = path.resolve(__dirname, "..");
  const longPolling = process.env.META_MCP_APPS_LONG_POLL === '1';
  const expectedViews = longPolling ? 3 : 1;
  const evidenceRoot = process.env.META_BROWSER_EVIDENCE || path.join(repoRoot, "output/playwright");
  await fs.mkdir(evidenceRoot, { recursive: true });
  const evidence = await fs.mkdtemp(path.join(evidenceRoot, "mcp-apps-native-"));
  const stateDir = path.join(evidence, "state");
  await fs.mkdir(stateDir);
  const logPath = path.join(evidence, "mcp-wire.jsonl");
  await fs.writeFile(logPath, "");
  const bundle = await build({ entryPoints: [path.join(__dirname, longPolling ? 'fixtures/mcp-apps-native.ts' : 'src/mcp-apps/preview-app.ts')], bundle: true,
    write: false, format: "esm", platform: "browser", target: "es2022", nodePaths: [path.join(__dirname, "node_modules")] });
  const htmlPath = path.join(evidence, "app.html");
  await fs.writeFile(htmlPath, `<!doctype html><html><body><script type="module">${bundle.outputFiles[0].text.replaceAll("</script", "<\\/script")}</script></body></html>`);
  const assets = new Map(await Promise.all([
    ["/console", "index.html", "text/html"],
    ["/console/assets/console-app.js", "console-app.js", "text/javascript"],
    ["/console/assets/console-app.css", "console-app.css", "text/css"],
  ].map(async ([url, name, type]) => [url, { type, bytes: await fs.readFile(path.join(__dirname, "dist", name)) }])));
  const spec = exampleBackendSpec(repoRoot, backend === "gateway" ? "mcp_apps_gateway_fixture" : "console_acceptance_fixture");
  let backendUrl, child, sandbox, browser, page, logs = "";
  const requests = [];
  const browserLogs = [];
  let streamEvidence = "";
  // Bind before the backend so the sandbox has an exact host origin without
  // closing and reserving a port. This proxy forwards all APIs to the native host.
  const proxy = http.createServer((req, res) => {
    const pathname = new URL(req.url, "http://localhost").pathname;
    const asset = assets.get(pathname === "/console/" ? "/console" : pathname);
    if (asset && req.method === "GET") { res.writeHead(200, { "content-type": asset.type }); res.end(asset.bytes); return; }
    if (!backendUrl) { res.writeHead(503); res.end(); return; }
    const target = new URL(req.url, backendUrl);
    const observed = { method: req.method, path: target.pathname, body: "", status: null, atMs: Date.now(), finishedAt: null };
    requests.push(observed);
    req.on("data", chunk => { if (observed.body.length < 65_536) observed.body += chunk; });
    const upstream = http.request(target, { method: req.method, headers: { ...req.headers, host: target.host } }, incoming => {
      observed.status = incoming.statusCode;
      incoming.on('end', () => { observed.finishedAt = Date.now(); });
      if (target.pathname === "/console/timeline/stream") {
        incoming.on("data", chunk => { streamEvidence = (streamEvidence + chunk).slice(-1_000_000); });
      }
      res.writeHead(incoming.statusCode, incoming.headers);
      incoming.on("error", error => res.destroy(error));
      res.on("close", () => incoming.destroy());
      incoming.pipe(res);
    });
    upstream.on("error", () => { if (!res.headersSent) res.writeHead(502); res.end(); });
    req.pipe(upstream);
  });
  try {
    const baseUrl = await listen(proxy);
    sandbox = createSandboxServer({ allowedHostOrigins: [baseUrl] });
    const sandboxUrl = `${await listen(sandbox)}/sandbox.html`;
    const appServer = path.join(__dirname, "fixtures/mcp-apps-server.cjs");
    if (backend === "gateway") {
      const consoleToml = path.join(evidence, "console.toml");
      await fs.writeFile(consoleToml, `mcp_apps_sandbox_url = ${JSON.stringify(sandboxUrl)}\n`);
      child = spawn(spec.command, spec.args, { cwd: repoRoot, stdio: ["pipe", "pipe", "pipe"], env: {
        // The scripted model answers every member and needs no key. A fake
        // one keeps a developer's real key away from the gateway, so a member
        // that ever bypassed the scripted model could not reach a provider.
        ...process.env, OPENAI_API_KEY: "sk-regression-test",
        XDG_STATE_HOME: path.join(evidence, "xdg-state"),
      } });
      const gateway = stdioRpc(child);
      for (const output of [child.stdout, child.stderr]) output.on("data", chunk => { logs = (logs + chunk).slice(-30_000); });
      const init = await gateway.request("meta/init", {
        mob_config: gatewayMobToml({ node: process.execPath, server: appServer, html: htmlPath, log: logPath }),
        persistent_state: stateDir,
        runtime_options: { console_config_path: consoleToml, console_require_app_auth: false },
      }, 120_000);
      backendUrl = init.http_base_url;
      for (const identity of ["router:main", "domain:delivery"]) {
        await gateway.request("meta/spawn_member", { profile: "lead", meerkat_id: identity }, 60_000);
      }
    } else {
      child = spawn(spec.command, spec.args, { cwd: repoRoot, stdio: ["ignore", "pipe", "pipe"], env: {
        ...process.env, META_FIXTURE_ADDR: LOOPBACK_ANY_PORT, META_FIXTURE_MODE: "identity",
        META_FIXTURE_LIVE_MODEL: "0", META_FIXTURE_LIVE_IMAGES: "0", META_FIXTURE_ROUTINE_TOOLS: "0",
        META_FIXTURE_STATE: stateDir, META_FIXTURE_NODE: process.execPath,
        META_FIXTURE_MCP_APPS_SERVER: appServer,
        META_FIXTURE_APP_HTML: htmlPath, META_FIXTURE_APP_LOG: logPath,
        META_FIXTURE_APP_SANDBOX: sandboxUrl,
      } });
      const ready = awaitFixtureReady(child, { label: "native MCP Apps fixture", timeoutMs: 60_000 });
      for (const output of [child.stdout, child.stderr]) output.on("data", chunk => { logs = (logs + chunk).slice(-30_000); });
      ({ baseUrl: backendUrl } = await ready);
    }
    await eventually(async () => {
      const value = await rpc(baseUrl, "meta/wait_ready", { timeout_ms: 1000 });
      return value.body.result?.timeout === false && value.body.result.ready?.length === 2;
    }, "native members ready", 60_000);
    // The quick case streams beyond the host's 15-second setup limit. The long
    // case holds that native model request until all live polling checks finish.
    const configured = await post(baseUrl, `${fixture}/model`, {
      source: "Ready.", delay_ms: longPolling ? 0 : 1000, chunk_chars: longPolling ? 64 : 1,
      scenario: { kind: longPolling ? 'mcp_apps_polling' : 'mcp_apps', run_id: "apps-native" },
    });
    assert.equal(configured.status, 200);
    if (longPolling) {
      const armed = await post(baseUrl, `${fixture}/model-barrier`, { action: 'arm', plan: {
        id: 'apps-live-polling', match_text: '[fixture:apps-native]', source: 'The MCP App is ready.',
        after_tool_result: 'fixture-apps-native-display-three',
      } });
      assert.equal(armed.status, 200);
    }
    browser = await chromium.launch({ headless: true });
    page = await browser.newPage({ viewport: { width: 1280, height: longPolling ? 1600 : 900 } });
    page.on("console", message => browserLogs.push({ type: message.type(), text: message.text() }));
    page.on("pageerror", error => browserLogs.push({ type: "pageerror", text: error.message }));
    await page.goto(`${baseUrl}/console`, { waitUntil: "domcontentloaded" });
    await page.getByTestId("sidebar-agent:router:main").click();
    await page.getByTestId("chat-composer:router:main").fill("[fixture:apps-native] Show matching records.");
    const sentAt = Date.now();
    await page.locator('button:has-text("Send"):visible, .cc-composer__send-btn:visible').first().click();
    const apps = Array.from({ length: expectedViews }, (_, index) => page.frameLocator('iframe[title="Interactive tool result"]').nth(index).frameLocator('iframe'));
    const app = apps[0];
    await app.getByText("7 matching records", { exact: true }).waitFor({ timeout: 15_000 });
    const firstRenderMs = Date.now() - sentAt;
    assert(firstRenderMs < 15_000, "retained view must render while the final model turn is still running");
    assert.equal(await page.getByText("The MCP App is ready.", { exact: true }).count(), 0,
      "the app must be visible before the delayed final model response completes");
    await app.getByText("PRIVATE_APP_DETAIL", { exact: true }).waitFor();
    await page.locator('iframe[title="Interactive tool result"]').first().scrollIntoViewIfNeeded();
    await page.screenshot({ path: path.join(evidence, "stock-native-app-live.png"), fullPage: true });
    let pollingEvidence;
    if (longPolling) {
      pollingEvidence = await exerciseLivePolling({ baseUrl, fixture, page, apps, requests, evidence });
    } else {
      await app.getByRole("button", { name: "Refresh result" }).click();
      await app.getByText("8 matching records", { exact: true }).waitFor({ timeout: 30_000 });
    }
    const locatorRequest = requests.find(request => request.path === "/console/mcp-apps/resolve" && request.status === 200
      && JSON.parse(request.body).toolCallId === 'fixture-apps-native-display');
    assert(locatorRequest, "stock host must resolve via the native gateway");
    const locator = JSON.parse(locatorRequest.body);
    assert.equal(locator.identity, "router:main");
    assert.equal(locator.toolCallId, "fixture-apps-native-display");
    const call = requests.find(request => request.path === "/console/mcp-apps/call-tool" && request.status === 200);
    assert(call, "button action must use native admission");
    assert.deepEqual(JSON.parse(call.body), { ...locator, name: "refresh", arguments: longPolling
      ? { viewId: 'display', requestId: 'display-refresh' } : {} });
    await page.reload({ waitUntil: "domcontentloaded" });
    for (const view of apps) {
      await view.getByText("7 matching records", { exact: true }).waitFor({ timeout: 60_000 });
      await view.getByText("PRIVATE_APP_DETAIL", { exact: true }).waitFor();
    }
    const wire = (await fs.readFile(logPath, "utf8")).trim().split("\n").filter(Boolean).map(line => JSON.parse(line));
    const displays = wire.filter(row => row.method === "tools/call" && row.params.name === "display");
    const refreshes = wire.filter(row => row.method === "tools/call" && row.params.name === "refresh");
    const polls = wire.filter(row => row.method === 'tools/call' && row.params.name === 'poll');
    assert.equal(displays.length, expectedViews, "reload must not replay the original tools");
    assert.equal(refreshes.length, 1, "one button activation must dispatch exactly once");
    assert.equal(wire.filter(row => row.method === "resources/read").length, expectedViews,
      "app actions and reload must reuse the original view instead of prefetching more HTML");
    assert.equal(displays[0].pid, refreshes[0].pid, "app action must use its original physical connection");
    if (longPolling) {
      assert.equal(new Set(displays.map(row => row.pid)).size, 1, 'all three original views must come from the same native connection');
      assert.equal(polls.length, pollingEvidence.pollActions, 'every successful poll must make exactly one physical MCP call');
      assert(polls.every(row => row.pid === displays[0].pid), 'every poll must use the original physical MCP connection');
      assert.equal(new Set(polls.map(row => row.params.arguments.requestId)).size, polls.length, 'poll request identities must never replay');
      assert(polls.every(row => row.atMs >= pollingEvidence.holdStartedAt && row.atMs <= pollingEvidence.holdFinishedAt),
        'all physical polls must occur while the native model request is held');
    }
    const init = wire.find(row => row.method === "initialize");
    assert(init.params.capabilities.extensions["io.modelcontextprotocol/ui"].mimeTypes.includes("text/html;profile=mcp-app"));
    const model = await (await fetch(`${baseUrl}${fixture}/requests`)).json();
    assert(!JSON.stringify(model).includes("PRIVATE_APP_DETAIL"), "UI-only metadata must not enter model requests");
    assert(model.some(request => request.tools?.some(tool => tool.name === "display")), "launch tool must be exposed to the native model");
    assert(!model.some(request => request.tools?.some(tool => tool.name === "refresh")), "app-only tool must be absent from model discovery");
    assert(!model.some(request => request.tools?.some(tool => tool.name === 'poll')), 'polling tool must be absent from model discovery');
    const timeline = await (await fetch(`${baseUrl}/console/timeline?identity=router%3Amain&mode=recent&limit=500`)).json();
    assert(!JSON.stringify(timeline).includes("PRIVATE_APP_DETAIL"), "UI-only metadata must not enter the browser frame log");
    const storage = await page.evaluate(() => JSON.stringify({ local: { ...localStorage }, session: { ...sessionStorage } }));
    assert(!storage.includes("PRIVATE_APP_DETAIL"), "UI-only metadata must not enter browser storage");
    for (const marker of ['PRIVATE_POLL_DETAIL', 'APP_POLL_RESULT_', 'APP_REFRESH_RESULT_']) {
      assert(!JSON.stringify(timeline).includes(marker), `${marker} must not enter the browser frame log`);
      assert(!streamEvidence.includes(marker), `${marker} must not enter timeline events`);
      assert(!storage.includes(marker), `${marker} must not enter browser storage`);
    }
    const denied = await post(baseUrl, "/console/mcp-apps/call-tool", { ...locator, identity: "missing:member", name: "refresh", arguments: {} });
    assert.equal(denied.status, 403);
    const forbidden = await post(baseUrl, "/console/mcp-apps/call-tool", { ...locator, name: "display", arguments: {} });
    assert.equal(forbidden.status, 403, "model-only tools must reject app invocation");
    const resolved = await post(baseUrl, "/console/mcp-apps/resolve", locator);
    assert.equal(resolved.status, 200);
    assert.equal(resolved.cache, "no-store");
    assert.equal(resolved.body.result._meta.viewDetail, "PRIVATE_APP_DETAIL");
    await page.screenshot({ path: path.join(evidence, "stock-native-app.png"), fullPage: true });
    await Promise.all([
      fs.writeFile(path.join(evidence, "fixture.log"), logs),
      fs.writeFile(path.join(evidence, "requests.json"), JSON.stringify(requests, null, 2)),
      fs.writeFile(path.join(evidence, "browser.json"), JSON.stringify(browserLogs, null, 2)),
      fs.writeFile(path.join(evidence, "model-requests.json"), JSON.stringify(model, null, 2)),
      fs.writeFile(path.join(evidence, "timeline-stream.txt"), streamEvidence),
      fs.writeFile(path.join(evidence, "timeline.json"), JSON.stringify(timeline, null, 2)),
    ]);
    await fs.writeFile(path.join(evidence, "summary.json"), JSON.stringify({
      passed: true, backend, originalToolCalls: displays.length, appActions: refreshes.length, firstRenderMs,
      pollActions: polls.length, polling: pollingEvidence, locator, sandboxUrl, evidence,
    }, null, 2));
    console.log(JSON.stringify({ passed: true, backend, evidence, originalToolCalls: displays.length,
      appActions: refreshes.length, pollActions: polls.length, firstRenderMs, heldMs: pollingEvidence?.heldMs }));
  } catch (error) {
    if (backendUrl) {
      await fetch(new URL(`${fixture}/requests`, backendUrl))
        .then(response => response.json())
        .then(model => fs.writeFile(path.join(evidence, "model-requests.json"), JSON.stringify(model, null, 2)))
        .catch(() => {});
      await fetch(new URL("/console/timeline?identity=router%3Amain&mode=recent&limit=500", backendUrl), { signal: AbortSignal.timeout(2_000) })
        .then(response => response.json())
        .then(timeline => fs.writeFile(path.join(evidence, "timeline.json"), JSON.stringify(timeline, null, 2)))
        .catch(() => {});
    }
    await fs.writeFile(path.join(evidence, "timeline-stream.txt"), streamEvidence);
    await fs.writeFile(path.join(evidence, "fixture.log"), logs);
    await fs.writeFile(path.join(evidence, "requests.json"), JSON.stringify(requests, null, 2));
    await fs.writeFile(path.join(evidence, "browser.json"), JSON.stringify(browserLogs, null, 2));
    if (page && !page.isClosed()) await fs.writeFile(path.join(evidence, "page.html"), await page.content());
    if (page && !page.isClosed()) await page.screenshot({ path: path.join(evidence, "failure.png"), fullPage: true }).catch(() => {});
    throw new Error(`${error.message}\nNative MCP Apps evidence: ${evidence}\n${logs}`);
  } finally {
    await browser?.close();
    await closeServer(proxy);
    await closeServer(sandbox);
    await stop(child);
  }
}

main().catch(error => { console.error(error); process.exitCode = 1; });
