"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const http = require("node:http");
const path = require("node:path");
const { createHash } = require("node:crypto");
const { chromium } = require("playwright");

// A docked operator panel shows a host page inside the Console, as the
// Meta Toolkit Connect route's "Connect setup" button shows the host's
// Toolkit setup page. The host page here stands in for that page at the
// seams the Console owns: the setup page is framable by its own origin only,
// its approval is a same-origin JSON POST that needs the Console's
// SameSite=Strict session cookie and its real Origin, and its sign-in link
// opens a new top-level tab, because the sign-in landing refuses any frame.
// The real page and its host ship outside this repository.
const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
const assetDirectory = path.join(__dirname, "../../crates/meerkat-meta/console-dist");
const session = "meta_console_session=operator-panel-proof";
const button = { id: "connect-setup", label: "Connect setup", panel: "/host/toolkit/setup" };
const review = { review_id: 7, approval_id: "opaque", decision: "approve" };

const setupPage = `<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Toolkit setup</title></head>
<body><h1>Toolkit setup</h1>
<section aria-label="Software review"><p>example-sqlite-tools 1.0.0 asks to be installed.</p>
<button type="button" id="approve">Approve install</button><p id="review-status" role="status">Waiting for a decision</p></section>
<a href="/host/toolkit/setup/callback?sign_in=proof" target="_blank" rel="noopener">Review sign-in</a>
<script src="/host/toolkit/setup/panel.js"></script></body></html>`;

const panelScript = `document.getElementById("approve").addEventListener("click", async () => {
  const status = document.getElementById("review-status");
  try {
    const response = await fetch("/host/toolkit/setup/answer", { method: "POST", credentials: "same-origin",
      headers: { "content-type": "application/json" }, body: JSON.stringify(${JSON.stringify(review)}) });
    status.textContent = response.ok ? "Approved" : "Refused (" + response.status + ")";
  } catch (error) {
    status.textContent = "Failed: " + error.message;
  }
});`;

const signInLanding = '<!doctype html><html lang="en"><head><meta charset="utf-8"><title>Toolkit sign-in</title></head>'
  + "<body><h1>Signed in to the Toolkit</h1></body></html>";

async function startServer() {
  const assets = new Map();
  for (const [url, name, type] of [["/console", "index.html", "text/html; charset=utf-8"],
    ["/console/assets/console-app.js", "console-app.js", "application/javascript"],
    ["/console/assets/console-app.css", "console-app.css", "text/css"]]) {
    assets.set(url, { name, type, bytes: await fs.readFile(path.join(assetDirectory, name)) });
  }
  const host = [];
  const streams = new Set();
  let origin = "";
  const server = http.createServer((request, response) => {
    const url = new URL(request.url, "http://localhost");
    const chunks = [];
    request.on("data", chunk => chunks.push(chunk));
    request.on("end", () => {
      const body = Buffer.concat(chunks).toString("utf8");
      const json = (value, status = 200) => {
        response.writeHead(status, { "content-type": "application/json" });
        response.end(JSON.stringify(value));
      };
      const signedIn = (request.headers.cookie || "").split(/;\s*/).includes(session);
      if (url.pathname.startsWith("/host/")) {
        host.push({ method: request.method, path: url.pathname, search: url.search, body, signedIn,
          origin: request.headers.origin ?? null, fetchSite: request.headers["sec-fetch-site"] ?? null,
          fetchDest: request.headers["sec-fetch-dest"] ?? null, contentType: request.headers["content-type"] ?? null });
      }
      const asset = assets.get(url.pathname);
      if (asset && request.method === "GET") {
        const headers = { "content-type": asset.type, "cache-control": "no-store" };
        if (url.pathname === "/console") headers["set-cookie"] = `${session}; Path=/; HttpOnly; SameSite=Strict`;
        response.writeHead(200, headers);
        response.end(asset.bytes);
        return;
      }
      if (url.pathname === "/host/toolkit/setup" && request.method === "GET") {
        if (!signedIn) return json({ error: "Console login required." }, 401);
        response.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store",
          "content-security-policy": "frame-ancestors 'self'" });
        response.end(setupPage);
        return;
      }
      if (url.pathname === "/host/toolkit/setup/panel.js" && request.method === "GET") {
        if (!signedIn) return json({ error: "Console login required." }, 401);
        response.writeHead(200, { "content-type": "application/javascript", "cache-control": "no-store" });
        response.end(panelScript);
        return;
      }
      if (url.pathname === "/host/toolkit/setup/answer" && request.method === "POST") {
        // The host admits a protected setup decision only from its own
        // origin, with the operator's session.
        const sameOrigin = request.headers.origin === origin && request.headers["sec-fetch-site"] === "same-origin";
        if (!signedIn || !sameOrigin || request.headers["content-type"] !== "application/json") {
          return json({ error: "Refused." }, 403);
        }
        return json({ accepted: true, review_id: JSON.parse(body).review_id });
      }
      if (url.pathname === "/host/toolkit/setup/callback" && request.method === "GET") {
        response.writeHead(200, { "content-type": "text/html; charset=utf-8", "cache-control": "no-store",
          "content-security-policy": "frame-ancestors 'none'" });
        response.end(signInLanding);
        return;
      }
      if (url.pathname === "/console/timeline/stream") {
        response.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
        response.write(`: connected\n\nevent: snapshot_complete\ndata: ${JSON.stringify({ type: "snapshot_complete", cursor: null })}\n\n`);
        streams.add(response);
        response.on("close", () => streams.delete(response));
        return;
      }
      const empty = { frames: [], available: true, exhausted: true, next_cursor: null, latest_cursor: null };
      if (url.pathname === "/console/experience") {
        return json({ contract_version: "0.5.0", runtime_id: "operator-panel-proof", storage_scope: "operator-panel-proof",
          runtime_capabilities: { can_send_messages: true }, voice: { available: false },
          console_config: { title: "Operator panel", appearance: { default_theme: "light" },
            layout: { initial_preset: "single" }, rail: { visible: false }, sidebar: { buttons: [button] } },
          agent_sidebar: { live_snapshot: { agents: [] } }, identity_status: { rows: [] } });
      }
      if (url.pathname === "/console/modules") return json({ modules: [] });
      if (url.pathname === "/console/identities") return json({ rows: [] });
      if (url.pathname === "/console/timeline") return json(empty);
      if (url.pathname === "/console/rpc" && request.method === "POST") {
        const call = JSON.parse(body || "{}");
        const reply = result => json({ jsonrpc: "2.0", id: call.id, result });
        if (call.method === "meta/capabilities") return reply({ methods: [], feature_capabilities: [] });
        if (call.method === "meta/console/query_timeline") return reply(empty);
        if (call.method === "meta/console/voice/readiness") return reply({ available: false });
        return reply({});
      }
      response.writeHead(404, { "content-type": "text/plain; charset=utf-8" });
      response.end("Not found");
    });
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  origin = `http://127.0.0.1:${server.address().port}`;
  return { origin, host,
    assets: [...assets.values()].map(asset => ({ name: asset.name, sha256: createHash("sha256").update(asset.bytes).digest("hex") })),
    async close() {
      for (const response of streams) response.end();
      server.closeAllConnections();
      await new Promise(resolve => server.close(resolve));
    },
  };
}

async function runToolkitSetupInOperatorPanelFrame() {
  const fixture = await startServer();
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  context.setDefaultTimeout(10_000);
  const page = await context.newPage();
  const result = { scope: "Real Chromium, embedded Console bundle, a stand-in host setup page, zero provider calls",
    assets: fixture.assets, errors: [], host: fixture.host };
  context.on("page", opened => opened.on("pageerror", error => result.errors.push(`${opened.url()}: ${error.message}`)));
  page.on("pageerror", error => result.errors.push(`${page.url()}: ${error.message}`));
  const setupUrl = `${fixture.origin}${button.panel}`;
  try {
    await page.goto(`${fixture.origin}/console`, { waitUntil: "domcontentloaded" });
    await page.getByTestId(`nav-panel:${button.id}`).click();
    const frameElement = page.getByTestId(`operator-panel:${button.id}`).locator("iframe.console-operator-panel__frame");
    await frameElement.waitFor();
    assert.equal(await frameElement.getAttribute("src"), button.panel);
    assert.equal(await frameElement.getAttribute("title"), button.label);
    const panel = page.frameLocator(".console-operator-panel__frame");
    await panel.getByRole("heading", { name: "Toolkit setup" }).waitFor();
    assert.deepEqual(fixture.host.filter(item => item.path === button.panel).map(({ signedIn, fetchSite, fetchDest }) =>
      ({ signedIn, fetchSite, fetchDest })), [{ signedIn: true, fetchSite: "same-origin", fetchDest: "iframe" }],
    "the panel loads the setup page in its frame with the Console's SameSite=Strict session");

    // The approval, clicked inside the docked panel.
    await panel.getByRole("button", { name: "Approve install" }).click();
    await panel.getByRole("status").filter({ hasText: /^(Approved|Refused|Failed)/ }).waitFor();
    result.reviewStatus = await panel.getByRole("status").textContent();
    const answers = fixture.host.filter(item => item.path === "/host/toolkit/setup/answer");
    result.answers = answers;
    assert.deepEqual(answers.map(({ method, signedIn, origin, fetchSite, fetchDest, contentType }) =>
      ({ method, signedIn, origin, fetchSite, fetchDest, contentType })), [{ method: "POST", signedIn: true,
      origin: fixture.origin, fetchSite: "same-origin", fetchDest: "empty", contentType: "application/json" }],
    "one same-origin JSON POST with the session cookie and the Console's origin");
    assert.deepEqual(JSON.parse(answers[0].body), review);
    assert.equal(result.reviewStatus, "Approved");

    // The sign-in opens in a new top-level tab; the panel stays on the setup page.
    const opened = context.waitForEvent("page");
    await panel.getByRole("link", { name: "Review sign-in" }).click();
    const signIn = await opened;
    await signIn.getByRole("heading", { name: "Signed in to the Toolkit" }).waitFor();
    result.signInUrl = signIn.url();
    assert.equal(result.signInUrl, `${fixture.origin}/host/toolkit/setup/callback?sign_in=proof`);
    const framed = page.frames().filter(frame => frame !== page.mainFrame()).map(frame => frame.url());
    result.framedUrls = framed;
    assert.deepEqual(framed, [setupUrl], "the docked panel still shows the setup page");
    const callbacks = fixture.host.filter(item => item.path === "/host/toolkit/setup/callback");
    assert.deepEqual(callbacks.map(item => item.fetchDest), ["document"], "the sign-in landing loads as a top-level document");
    // A sandbox would give the host page an opaque origin, or block its
    // sign-in tab; both checks above fail first.
    assert.equal(await frameElement.getAttribute("sandbox"), null, "the panel frame keeps the host page's origin");
    assert.deepEqual(result.errors, []);
    result.passed = true;
    process.stdout.write("browser operator panel Toolkit setup in frame ok\n");
  } catch (error) {
    result.error = String(error.stack || error);
    throw error;
  } finally {
    await fs.mkdir(evidence, { recursive: true });
    await page.screenshot({ path: path.join(evidence, "operator-panel-toolkit-setup.png"), fullPage: true }).catch(() => {});
    await fs.writeFile(path.join(evidence, "operator-panel-toolkit-setup.json"), JSON.stringify(result, null, 2) + "\n");
    await browser.close();
    await fixture.close();
  }
}

module.exports = { scenarios: [{ id: "operator-panel-toolkit-setup-in-frame", family: "console", backend: "mock",
  run: runToolkitSetupInOperatorPanelFrame }] };
if (require.main === module) require("../scenario-registry.cjs").runScenarios(module.exports.scenarios)
  .catch(error => { process.stderr.write(`${error.stack}\n`); process.exitCode = 1; });
