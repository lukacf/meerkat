"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { chromium } = require("playwright");
const { startFixture, eventually } = require("../acceptance-runtime.cjs");
const { browserFailureMonitor, navigation } = require("./browser-failure-monitor.cjs");
const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");

async function clonedTabDrafts() {
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: true });
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  const first = await context.newPage();
  const monitor = browserFailureMonitor(context, { origin: fixture.baseUrl });
  const { errors, expected: expectedFailures, failures: requestFailures } = monitor;
  let clone;
  const editor = page => page.getByTestId("chat-composer:router:main").first();
  async function waitSaved(page, text) {
    await eventually(() => page.evaluate(value => [localStorage, sessionStorage].some(storage =>
      Object.keys(storage).some(key => key.startsWith("meta-composer-draft:v2:") && JSON.parse(storage.getItem(key)).text === value)), text), "draft persisted");
  }
  try {
    await navigation(first, "open original draft tab", async () => {
      const response = await first.goto(fixture.baseUrl + "/scoped");
      await first.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: /router/i }).first().click();
      await first.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
      return response;
    });
    await editor(first).fill("Original tab draft");
    await waitSaved(first, "Original tab draft");
    clone = await monitor.popup("explicit same-origin draft clone", async () => {
      const opened = first.waitForEvent("popup");
      await first.evaluate(() => window.open(location.href, "_blank"));
      const copy = await opened;
      await editor(copy).waitFor();
      await copy.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
      return copy;
    });
    // A real same-origin opener copies sessionStorage, including the tab ID.
    assert.equal(await first.evaluate(() => sessionStorage.getItem("meta-composer-tab:v1")),
      await clone.evaluate(() => sessionStorage.getItem("meta-composer-tab:v1")));
    assert.equal(await editor(clone).inputValue(), "Original tab draft");
    await editor(clone).fill("Independent copied tab draft");
    await waitSaved(clone, "Independent copied tab draft");
    await fixture.control("model", { source: "Original tab send completed.", delay_ms: 0, chunk_chars: 32 });
    await editor(first).press("Enter");
    await eventually(async () => (await (await fetch(fixture.backendUrl + "/__fixture/requests")).json())
      .some(request => JSON.stringify(request.messages).includes("Original tab draft")), "original tab real model ingress");
    await navigation(clone, "explicit clone draft tab reload", async () => {
      const response = await clone.reload();
      await editor(clone).waitFor();
      await clone.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
      return response;
    });
    assert.equal(await editor(clone).inputValue(), "Independent copied tab draft", "sending in the original cannot erase a copied tab draft");
    await navigation(first, "explicit first draft tab reload", async () => {
      const response = await first.reload();
      await editor(first).waitFor();
      await first.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
      return response;
    });
    assert.equal(await editor(first).inputValue(), "");
    const requests = await (await fetch(fixture.backendUrl + "/__fixture/requests")).json();
    assert(!requests.some(request => JSON.stringify(request.messages).includes("Independent copied tab draft")));
    monitor.assertClean();
    await fs.mkdir(evidence, { recursive: true });
    await clone.screenshot({ path: path.join(evidence, "cloned-tab-preserved-draft.png"), fullPage: true });
    await fs.writeFile(path.join(evidence, "cloned-tab-evidence.json"), JSON.stringify({ clonedStorage: true, originalSubmitted: true, copiedDraftPreserved: true, errors, expectedFailures, requestFailures }, null, 2));
    monitor.assertClean();
  } catch (error) {
    await fs.mkdir(evidence, { recursive: true });
    await (clone || first).screenshot({ path: path.join(evidence, "cloned-tab-failure.png"), fullPage: true }).catch(() => {});
    await fs.writeFile(path.join(evidence, "cloned-tab-failure.json"), JSON.stringify({ error: error.stack || String(error), errors, expectedFailures, requestFailures, observations: fixture.observations, logs: fixture.logs() }, null, 2));
    throw error;
  } finally { monitor.stop(); await browser.close(); await fixture.close(); }
}

module.exports = { scenarios: [{ id: "real-cloned-tab-drafts", family: "real-send", backend: "real", run: clonedTabDrafts }] };
if (require.main === module) require("../scenario-registry.cjs").runScenarios(module.exports.scenarios)
  .catch(error => { process.stderr.write(`${error.stack}\n`); process.exitCode = 1; });
