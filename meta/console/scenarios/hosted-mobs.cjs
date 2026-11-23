"use strict";

// A gateway that hosts a second mob behind a door of its own, in a real
// browser on the built console. The gateway is the recording fake the unit
// rows use (src/lib/fixtures/two-root-gateway.ts, which answers as contract
// v0.6.0 says), bundled for node and put behind a local HTTP server: every
// request of the page is handed to the fake and its answer, the event
// stream included, is written back. The page and its two assets come from
// console/dist. No backend binary and no model.

const assert = require("node:assert/strict");
const fs = require("node:fs");
const http = require("node:http");
const path = require("node:path");
const { buildSync } = require("esbuild");
const { chromium } = require("playwright");
const { eventually } = require("../acceptance-runtime.cjs");

const consoleDir = path.join(__dirname, "..");
const HOSTED = "support";
const HOSTED_REVIEWER = "support/reviewer";

/** The fake gateway's module, bundled for node. */
function loadTwoRootGateway() {
  const outfile = path.join(consoleDir, ".tmp", "hosted-mobs", "two-root-gateway.cjs");
  buildSync({
    entryPoints: [path.join(__dirname, "hosted-mobs-entry.ts")],
    bundle: true, platform: "node", format: "cjs", target: "node20", outfile, logLevel: "silent",
  });
  delete require.cache[require.resolve(outfile)];
  return require(outfile);
}

/** A local server: the console's page and assets from dist, every other request from the fake. */
async function serveGateway(gateway) {
  const assets = new Map([
    ["/console", ["index.html", "text/html"]],
    ["/console/", ["index.html", "text/html"]],
    ["/console/assets/console-app.js", ["console-app.js", "text/javascript"]],
    ["/console/assets/console-app.css", ["console-app.css", "text/css"]],
  ].map(([url, [name, type]]) => [url, { type, bytes: fs.readFileSync(path.join(consoleDir, "dist", name)) }]));
  const open = new Set();
  const server = http.createServer(async (req, res) => {
    const url = new URL(req.url, "http://console.test");
    const asset = req.method === "GET" ? assets.get(url.pathname) : undefined;
    if (asset) { res.writeHead(200, { "content-type": asset.type, "cache-control": "no-store" }); res.end(asset.bytes); return; }
    const chunks = [];
    for await (const chunk of req) chunks.push(chunk);
    const body = Buffer.concat(chunks).toString("utf8");
    const abort = new AbortController();
    res.on("close", () => abort.abort());
    try {
      const headers = Object.fromEntries(Object.entries(req.headers).filter(([, value]) => typeof value === "string"));
      const answer = await gateway.fetch(`${url.pathname}${url.search}`, {
        method: req.method, headers, signal: abort.signal, ...(body ? { body } : {}),
      });
      res.writeHead(answer.status, Object.fromEntries(answer.headers.entries()));
      if (!answer.body) { res.end(); return; }
      // An event stream stays open: its blocks are written as the fake serves them.
      const reader = answer.body.getReader();
      open.add(reader);
      res.on("close", () => { open.delete(reader); reader.cancel().catch(() => {}); });
      for (;;) {
        const { done, value } = await reader.read();
        if (done) break;
        res.write(value);
      }
      res.end();
    } catch (error) {
      if (!res.headersSent) res.writeHead(502, { "content-type": "text/plain" });
      res.end(String(error && error.message));
    }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  return {
    baseUrl: `http://127.0.0.1:${server.address().port}`,
    async close() {
      for (const reader of open) await reader.cancel().catch(() => {});
      server.closeAllConnections?.();
      await new Promise((resolve) => server.close(resolve));
    },
  };
}

const DIGEST = `sha256:${"b".repeat(64)}`;
/** A set of questions as the gateway lists it: asked by `by`, of the mob `mob` (absent: the launch mob). */
const questionsFrom = (by, pendingId, prompt, mob) => ({
  pending_id: pendingId,
  origin: { identity: by, ...(mob ? { mob_id: mob } : {}) },
  questions: [{ id: "q-region", prompt, choices: [{ id: "c-eu", label: "Europe" }, { id: "c-us", label: "United States" }] }],
  created_at_ms: Date.now() - 60_000,
  deadline_at_ms: Date.now() + 3_600_000,
  subject_digest: DIGEST,
});

async function runHostedMobsTwoRoots() {
  const { createTwoRootGateway, CONSOLE_RPC_METHODS } = loadTwoRootGateway();
  const gateway = createTwoRootGateway({
    // The gateway lists every method, so the page reads the host's lists too.
    methods: Object.values(CONSOLE_RPC_METHODS),
    // Every mob the host lists is loaded: no other team sleeps beside the two.
    sleepers: false,
    hostQuestionSets: [questionsFrom("reviewer", "q-hosted", "Which region does Support take first?", HOSTED)],
  });
  const server = await serveGateway(gateway);
  const evidenceDir = path.join(process.env.META_BROWSER_EVIDENCE || path.join(consoleDir, "../output/playwright/console-acceptance"), "hosted-mobs-two-roots");
  fs.mkdirSync(evidenceDir, { recursive: true });
  let browser;
  try {
    browser = await chromium.launch({ headless: true });
    const context = await browser.newContext({ viewport: { width: 1400, height: 1000 } });
    const page = await context.newPage();
    const pageErrors = [];
    page.on("pageerror", (error) => pageErrors.push(error.message));
    const shot = (name) => page.screenshot({ path: path.join(evidenceDir, `${name}.png`) });
    const text = (testId) => page.getByTestId(testId).innerText();
    const paramsOf = (request) => request.params || {};
    const last = (list) => list[list.length - 1];

    await page.goto(`${server.baseUrl}/console`, { waitUntil: "domcontentloaded", timeout: 30_000 });

    // 1. Two teams at the top of the roster, an agent `reviewer` in each, each under its own key.
    await page.getByTestId("sidebar-agent:reviewer").waitFor({ timeout: 30_000 });
    await page.getByTestId(`sidebar-agent:${HOSTED_REVIEWER}`).waitFor({ timeout: 30_000 });
    await page.getByTestId(`sidebar-mob:${HOSTED}`).waitFor({ timeout: 10_000 });
    assert.equal(await page.getByTestId("sidebar-agent:support/triage").count(), 1, "the hosted team lists its second agent");
    await shot("1-roster");

    // 2. A message to the hosted reviewer goes through that team's door, and
    // what that door then serves for the agent is shown in that chat alone.
    await page.getByTestId(`sidebar-agent:${HOSTED_REVIEWER}`).click();
    const hostedComposer = page.getByTestId(`chat-composer:${HOSTED_REVIEWER}`);
    await hostedComposer.waitFor({ timeout: 10_000 });
    await hostedComposer.fill("hello support reviewer");
    await hostedComposer.press("Enter");
    await eventually(() => gateway.calls("meta/console/send").length === 1, "the hosted reviewer's message is sent");
    const hostedSend = paramsOf(last(gateway.calls("meta/console/send")));
    assert.deepEqual(
      { mob_id: hostedSend.mob_id, identity: hostedSend.identity, content: hostedSend.content },
      { mob_id: HOSTED, identity: "reviewer", content: "hello support reviewer" },
      "the message names the hosted team and the agent as that team's door knows it",
    );
    const SERVED = "asked of the support reviewer by a peer";
    gateway.push({ mob: HOSTED, identity: "reviewer", kind: "interaction_started", interactionId: "hosted-1", payload: { content: SERVED, prompt: SERVED } });
    gateway.push({ mob: HOSTED, identity: "reviewer", kind: "interaction_complete", interactionId: "hosted-1", payload: {} });
    const hostedChat = page.getByTestId(`chat-pane:${HOSTED_REVIEWER}`);
    await eventually(async () => (await hostedChat.innerText()).includes(SERVED), "a frame of the hosted team is in its own agent's chat");

    // 3. The hosted reviewer's set of questions is in that chat: the list was
    // asked for every mob, and the entry names its mob.
    await hostedChat.locator('[data-testid="question-set:q-hosted"]').waitFor({ timeout: 10_000 });
    for (const method of ["meta/gating/pending", "meta/gating/questions"]) {
      const selectors = [...new Set(gateway.calls(method).map((call) => paramsOf(call).mobs))];
      assert.deepEqual([method, selectors], [method, ["*"]], "a list of the host that names agents is asked for every mob");
    }
    // The stream the page holds open is the one for every mob. (A page that
    // opens its stream before it knows what the gateway hosts opens one for
    // the launch mob first and replaces it.)
    const streams = gateway.requests.filter((request) => request.route === "GET /console/timeline/stream");
    assert.ok(streams.length > 0 && last(streams).query.some(([name, value]) => name === "mobs" && value === "*"),
      "the event stream is asked for every mob");
    await shot("2-hosted-chat");

    // The launch team's reviewer is another agent: its chat holds nothing of
    // the hosted one's, and its message names no mob.
    await page.getByTestId("sidebar-agent:reviewer").click();
    const launchComposer = page.getByTestId("chat-composer:reviewer");
    await launchComposer.waitFor({ timeout: 10_000 });
    const launchChat = page.getByTestId("chat-pane:reviewer");
    const launchText = await launchChat.innerText();
    assert.ok(!launchText.includes(SERVED), "the hosted team's frame is not in the launch reviewer's chat");
    assert.equal(await launchChat.locator('[data-testid="question-set:q-hosted"]').count(), 0, "nor is its set of questions");
    await launchComposer.fill("hello launch reviewer");
    await launchComposer.press("Enter");
    await eventually(() => gateway.calls("meta/console/send").length === 2, "the launch reviewer's message is sent");
    const launchSend = paramsOf(last(gateway.calls("meta/console/send")));
    assert.deepEqual(
      { mob_id: launchSend.mob_id, identity: launchSend.identity, content: launchSend.content },
      { mob_id: undefined, identity: "reviewer", content: "hello launch reviewer" },
      "the launch reviewer's message names no mob",
    );
    await shot("3-launch-chat");

    // 4. The hosted team falls asleep. The page says so, names the team in
    // no request it makes on its own, and wakes it when the person opens it.
    gateway.set({ hostedState: "dormant" });
    gateway.push({ identity: "planner", kind: "state_changed", payload: {} });
    await eventually(async () => (await text(`sidebar-mob-state:${HOSTED}`)) === "Asleep", "the roster says the hosted team is asleep");
    await page.waitForTimeout(600);
    gateway.clear();
    const wakesBefore = gateway.wakes();
    gateway.push({ identity: "planner", kind: "state_changed", payload: {} });
    await page.waitForTimeout(900);
    gateway.push({ identity: "planner", kind: "state_changed", payload: {} });
    await page.waitForTimeout(900);
    assert.ok(gateway.requests.length > 0, "the page kept reading its lists");
    const namedOneMob = gateway.requests.filter((request) => "mob_id" in paramsOf(request) || request.query.some(([name]) => name === "mob_id"));
    assert.deepEqual(namedOneMob.map((request) => request.method || request.route), [], "no request the page makes on its own names a team that is asleep");
    assert.equal(gateway.wakes(), wakesBefore, "nothing woke the team");
    await shot("4-asleep");
    await page.getByTestId(`sidebar-mob-toggle:${HOSTED}`).click();
    await eventually(() => gateway.wakes() === wakesBefore + 1, "the person's opening wakes the team once");
    await page.getByTestId(`sidebar-agent:${HOSTED_REVIEWER}`).waitFor({ timeout: 10_000 });
    assert.equal(gateway.wakes(), wakesBefore + 1, "one opening request, not more");

    // 5. The page is loaded again with the hosted reviewer's chat open: the
    // pane is restored for that agent, with what was said in it.
    await page.getByTestId(`sidebar-agent:${HOSTED_REVIEWER}`).click();
    await page.getByTestId(`chat-composer:${HOSTED_REVIEWER}`).waitFor({ timeout: 10_000 });
    await page.waitForTimeout(600);
    await page.reload({ waitUntil: "domcontentloaded" });
    await page.getByTestId(`chat-composer:${HOSTED_REVIEWER}`).waitFor({ timeout: 30_000 });
    await eventually(async () => (await page.getByTestId(`chat-pane:${HOSTED_REVIEWER}`).innerText()).includes(SERVED),
      "the restored chat holds what the hosted team's door served for the agent");
    assert.equal(await page.getByTestId("chat-composer:reviewer").count(), 0, "the launch reviewer's chat was not opened in its place");
    const kept = await page.evaluate(() => Object.keys(window.localStorage).filter((key) => key.includes("support%2F") || key.includes("support/")));
    assert.ok(kept.length > 0, "what the browser keeps for the hosted agent is under that agent's own key");
    await shot("5-reloaded");

    assert.deepEqual(pageErrors, [], "the page raised no error");
    process.stdout.write("browser hosted mobs two roots ok\n");
  } finally {
    if (browser) await browser.close();
    await server.close();
  }
}

module.exports = {
  scenarios: [{ id: "hosted-mobs-two-roots", family: "console", backend: "mock", run: runHostedMobsTwoRoots }],
};
if (require.main === module) {
  require("../scenario-registry.cjs").runScenarios(module.exports.scenarios).catch((error) => {
    process.stderr.write(`${error.stack || error.message}\n`);
    process.exit(1);
  });
}
