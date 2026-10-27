"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { randomUUID } = require("node:crypto");
const { chromium } = require("playwright");
const { startFixture, eventually, rpc } = require("../acceptance-runtime.cjs");
const { startScopeProxy } = require("../recovery-scope-proxy.cjs");
const { assertScopeCancellation } = require("../recovery-scope-oracle.cjs");
const { browserFailureMonitor } = require("./browser-failure-monitor.cjs");

const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
const identity = "router:main";

async function timeline(fixture) {
  const response = await fetch(`${fixture.backendUrl}/console/timeline?identity=${encodeURIComponent(identity)}&mode=recent&limit=1000`);
  assert.equal(response.status, 200);
  return response.json();
}

async function modelRequests(fixture) {
  const response = await fetch(`${fixture.backendUrl}/__fixture/requests`);
  assert.equal(response.status, 200); return response.json();
}

async function seed(fixture, marker, turns) {
  for (let turn = 0; turn < turns; turn++) {
    const text = `${marker} ${turn + 1}.`;
    await fixture.control("model", { source: text, delay_ms: 0, chunk_chars: 1024 });
    const accepted = await rpc(fixture.baseUrl, "meta/console/send", {
      identity, content: `Record ${text}`, origin: "console:scope-acceptance", origin_kind: "operator",
      idempotency_key: randomUUID(), handling_mode: "queue",
    });
    assert.equal(accepted.status, 200); assert(accepted.body.result?.input_frame_id, JSON.stringify(accepted.body));
    await eventually(async () => (await timeline(fixture)).frames.some(frame => frame.kind === "interaction_complete" && frame.interaction_id === accepted.body.result.interaction_id), "real owner seed completes");
  }
}

function transcript(page, host) {
  return page.locator(host === "shared" ? '[data-testid="shared-pane-0"] .cc-conversation-pane__scroll' : ".conv__body").first();
}
function editor(page, host) {
  return host === "shared" ? page.getByRole("textbox", { name: "Message", exact: true }) : page.getByTestId(`chat-composer:${identity}`).first();
}
async function openConversation(page, host) {
  if (host === "stock" && await editor(page, host).count() === 0) {
    await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: /router/i }).first().click();
  }
  await editor(page, host).waitFor();
}
async function quoteMarker(page, host, marker) {
  const scope = host === "shared" ? page.getByTestId("shared-pane-0") : page.locator(".conv").first();
  const source = scope.locator("[data-quote-message-id] p").filter({ hasText: marker }).first();
  await source.evaluate(node => {
    const range = document.createRange(); range.selectNodeContents(node);
    const selection = window.getSelection(); selection.removeAllRanges(); selection.addRange(range);
  });
  await scope.getByRole("button", { name: "Add to message", exact: true }).click();
  await scope.locator(".cc-context-chip").waitFor();
}

async function scopeCancellation(host, phase) {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Use the coordinator's immutable prebuilt native fixture; do not build from a browser shard.");
  const name = `real-${host}-scope-cancel-${phase}`;
  const stateRoot = await fs.mkdtemp(path.join(os.tmpdir(), "meta-scope-owners-"));
  const owners = {};
  let proxy, browser, page, monitor, result;
  await fs.mkdir(evidence, { recursive: true });
  try {
    owners["scope-a"] = await startFixture({ stateDir: path.join(stateRoot, "a") });
    owners["scope-b"] = await startFixture({ stateDir: path.join(stateRoot, "b") });
    const oldMarker = `Private old authority ${randomUUID()}`;
    const replacementMarker = `Replacement authority ${randomUUID()}`;
    // Different real history lengths avoid accidental equality of numeric
    // owner cursors and make transferred-cursor assertions meaningful.
    await seed(owners["scope-a"], oldMarker, 1);
    await seed(owners["scope-b"], replacementMarker, 3);
    const before = await Promise.all(Object.values(owners).map(async owner => (await modelRequests(owner)).length));
    const oldHistory = await timeline(owners["scope-a"]);
    const replacementHistory = await timeline(owners["scope-b"]);
    assert.notEqual(oldHistory.latest_cursor, replacementHistory.latest_cursor, "real owner frontiers differ");
    proxy = await startScopeProxy(owners);
    if (phase === "seed") proxy.holdHistory("scope-a", host);
    browser = await chromium.launch({ headless: true });
    page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
    page.setDefaultTimeout(20_000);
    monitor = browserFailureMonitor(page.context(), { origin: proxy.baseUrl, initializationPrefixes: ["/scope-a", "/scope-b"] });
    await monitor.during(page, "initial old-authority host", async () => {
      await page.goto(`${proxy.baseUrl}/${host === "shared" ? "shared" : "scoped"}?runtime-scope-switch=1`);
      await page.getByTestId("host-scope").waitFor();
      if (host === "stock") await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: /router/i }).first().waitFor();
      if (phase === "repair") await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
      else await eventually(() => proxy.held.some(item => !item.observation.abortedAt), "initial successful owner history is held");
    }, { priorRequests: () => false, initialize: ["/scope-a"] });
    if (phase === "repair") {
      await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
      await openConversation(page, host);
      await transcript(page, host).getByText(`${oldMarker} 1.`, { exact: true }).waitFor();
      await editor(page, host).fill("Unsent draft owned by the old authority");
      await quoteMarker(page, host, oldMarker);
      // Only the global subscription recent page is held for stock. Per-pane
      // terminal reconciliation is separate and cannot satisfy this witness.
      proxy.holdHistory("scope-a", host);
      await owners["scope-a"].control("fault", { fault: "expired" });
      proxy.disconnect("scope-a");
      await eventually(() => proxy.observations.some(item => item.scope === "scope-a" && item.path.startsWith("/console/timeline/stream") && item.status === 409), "actual old owner returns expired replay");
    }
    // Wait for the active seed/repair, ignoring initial stock requests which
    // were already cancelled by its experience-derived scope initialization.
    const active = await eventually(() => {
      const items = proxy.held.filter(item => !item.observation.abortedAt);
      return items.length ? items : null;
    }, `real ${phase} response held before authority switch`);
    if (host === "stock") await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: /router/i }).first().waitFor();
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    const held = active.filter(item => !item.observation.abortedAt);
    assert(held.length, "the held old request is still live immediately before the switch");
    const switchedAt = proxy.now();
    await monitor.during(page, "explicit host authority switch", async () => {
      await page.getByRole("button", { name: "Change host scope", exact: true }).click();
      await eventually(() => held.every(item => item.observation.abortedAt >= switchedAt), "scope switch aborts every held old history socket");
      await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
      await openConversation(page, host);
      await transcript(page, host).getByText(`${replacementMarker} 3.`, { exact: true }).waitFor();
    }, { priorRequests: request => new URL(request.url()).pathname.startsWith("/scope-a/"), initialize: ["/scope-b"] });
    proxy.releaseHistory();
    await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
    result = {
      phase, host, authorityEvidence: "two isolated real runtime owners; no server-authenticated principal-change claim",
      oldScope: "scope-a", replacementScope: "scope-b", switchedAt, oldMarker, replacementMarker,
      held: held.map(item => item.observation), observations: proxy.observations,
      replacementFrames: (await timeline(owners["scope-b"])).frames,
      prebrowserReplacementCursor: replacementHistory.latest_cursor,
      transcript: await transcript(page, host).innerText(),
      rows: await transcript(page, host).locator("[data-conversation-row-id]").evaluateAll(nodes => nodes.map(node => node.dataset.conversationRowId)),
      draft: await editor(page, host).inputValue(), contextCount: await page.locator(".cc-context-chip").count(),
      phaseAfter: await page.getByTestId("console-transport-status").getAttribute("data-phase"),
      errors: monitor.errors, expectedFailures: monitor.expected, failedRequests: monitor.failures,
      modelRequestCounts: { before, after: await Promise.all(Object.values(owners).map(async owner => (await modelRequests(owner)).length)) },
    };
    monitor.assertClean();
    assertScopeCancellation(result);
    await page.screenshot({ path: path.join(evidence, `${name}-replacement-1600.png`), fullPage: true });
    monitor.assertClean();
    await fs.writeFile(path.join(evidence, `${name}.json`), JSON.stringify(result, null, 2));
    monitor.assertClean();
  } catch (error) {
    if (page) await page.screenshot({ path: path.join(evidence, `${name}-failure.png`), fullPage: true }).catch(() => {});
    await fs.writeFile(path.join(evidence, `${name}-failure.json`), JSON.stringify({ error: error.stack, result,
      observations: proxy?.observations, held: proxy?.held.map(item => item.observation),
      errors: monitor?.errors, expectedFailures: monitor?.expected, failedRequests: monitor?.failures,
      ownerLogs: Object.fromEntries(Object.entries(owners).map(([scope, owner]) => [scope, owner.logs()])),
    }, null, 2)).catch(() => {});
    throw error;
  } finally {
    monitor?.stop();
    // Always attempt every owned teardown, even if a browser or proxy close
    // fails. Only these private fixture state directories are removed.
    const cleanup = [];
    if (browser) cleanup.push(browser.close());
    if (proxy) cleanup.push(proxy.close());
    const closed = await Promise.allSettled([...cleanup, ...Object.values(owners).map(owner => owner.close())]);
    await fs.rm(stateRoot, { recursive: true, force: true });
    const failures = closed.filter(item => item.status === "rejected").map(item => item.reason);
    if (failures.length) throw new AggregateError(failures, "scope acceptance owned teardown failed");
  }
}

const scenarios = ["stock", "shared"].flatMap(host => ["seed", "repair"].map(phase => ({
  id: `real-${host}-scope-cancel-${phase}`, family: "real-runtime", backend: "real",
  run: () => scopeCancellation(host, phase),
})));
module.exports = { scenarios };
