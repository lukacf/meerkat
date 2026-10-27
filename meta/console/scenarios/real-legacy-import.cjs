"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { chromium } = require("playwright");
const { startFixture, eventually } = require("../acceptance-runtime.cjs");
const { browserFailureMonitor, navigation } = require("./browser-failure-monitor.cjs");

async function explicitLegacyImport({ embeddedHttp = false } = {}) {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Use the coordinator's prebuilt fixture");
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: true, ...(embeddedHttp ? { args: ["--no-proxy-server", "--host-resolver-rules=MAP console-storage.test 127.0.0.1"] } : {}) });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  const identity = "router:main";
  const text = "Imported once: review the WorkGraph and preserve this exact instruction.\n\nSecond paragraph.";
  const unsentDraft = "Unsent embedded draft survives plain HTTP reload.";
  const legacyKey = `mobkit-pending-stack:${identity}`;
  const legacyBytes = JSON.stringify([{ id: "legacy-proof", text, addedAt: 1790370000000 }]);
  const origin = embeddedHttp ? fixture.baseUrl.replace("127.0.0.1", "console-storage.test") : fixture.baseUrl;
  const screenshotPrefix = embeddedHttp ? "embedded-http-legacy-import" : "legacy-import";
  let namespace = `${fixture.baseUrl}/acceptance-realm/operator-a`;
  let queueKey;
  const dir = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
  const monitor = browserFailureMonitor(page.context(), { origin });
  const { errors, expected: expectedFailures, failures: requestFailures } = monitor;
  const live = () => page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
  const reload = () => navigation(page, "explicit legacy-import reload", async () => {
    const response = await page.reload();
    await live();
    return response;
  });
  const sends = () => fixture.observations.filter(item => item.method === "POST" &&
    (item.path.endsWith("/send") || item.request.includes('"method":"meta/console/send"')));
  const requests = async () => (await (await fetch(fixture.backendUrl + "/__fixture/requests")).json());
  try {
    if (embeddedHttp) {
      const response = await fetch(`${fixture.baseUrl}/console/experience`);
      assert.equal(response.status, 200);
      const experience = await response.json();
      assert.equal(typeof experience.storage_scope, "string", "default embedded persistence needs server-owned storage authority");
      assert(experience.storage_scope.trim());
      namespace = JSON.stringify([origin, experience.storage_scope]);
    }
    queueKey = `meta-send-attempts:v1:${encodeURIComponent(namespace)}:${encodeURIComponent(identity)}`;
    await navigation(page, "open legacy-import host", async () => {
      const response = await page.goto(origin + (embeddedHttp ? "/console" : "/scoped"));
      await live();
      return response;
    });
    if (embeddedHttp) {
      assert.deepEqual(await page.evaluate(() => ({ secure: isSecureContext, locks: Boolean(navigator.locks), indexedDB: Boolean(indexedDB) })),
        { secure: false, locks: false, indexedDB: true });
    }
    await page.evaluate(([key, bytes]) => localStorage.setItem(key, bytes), [legacyKey, legacyBytes]);
    await navigation(page, "reload seeded legacy queue and select target", async () => {
      const response = await page.reload();
      await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: /router/i }).first().click();
      await live();
      return response;
    });
    const importButton = page.getByRole("button", { name: "Import and send older queued messages", exact: true });
    await importButton.waitFor();
    assert.equal(sends().length, 0, "opening the console cannot submit a legacy queue");
    if (embeddedHttp) {
      await page.getByTestId(`chat-composer:${identity}`).first().fill(unsentDraft);
      await eventually(() => page.evaluate(({ namespace, unsentDraft }) => Object.keys(sessionStorage)
        .filter(key => key.startsWith("meta-composer-draft:v2:"))
        .some(key => { const draft = JSON.parse(sessionStorage.getItem(key)); return draft.namespace === namespace && draft.text === unsentDraft; }),
      { namespace, unsentDraft }), "default embedded composer draft saved in server scope");
      await reload();
      await page.getByTestId(`chat-composer:${identity}`).first().waitFor();
      assert.equal(await page.getByTestId(`chat-composer:${identity}`).first().inputValue(), unsentDraft);
      await importButton.waitFor();
      assert.equal(sends().length, 0, "reloading a draft cannot submit it or import legacy data");
      await fs.mkdir(dir, { recursive: true });
      await page.screenshot({ path: path.join(dir, "embedded-http-draft-reload.png"), fullPage: true });
    }
    const migration = page.getByTestId("legacy-queue-import");
    await migration.waitFor();
    const geometry = [];
    for (const width of [1600, 1024]) {
      await page.setViewportSize({ width, height: width === 1600 ? 1000 : 900 });
      const bounds = await migration.evaluate(node => {
        const rect = node.getBoundingClientRect();
        const button = node.querySelector("button").getBoundingClientRect();
        return { width: innerWidth, overflow: node.scrollWidth - node.clientWidth, left: rect.left, right: rect.right,
          button: { left: button.left, right: button.right, height: button.height } };
      });
      assert(bounds.overflow <= 1 && bounds.left >= 0 && bounds.right <= width, "migration row fits the viewport");
      assert(bounds.button.left >= bounds.left && bounds.button.right <= bounds.right && bounds.button.height >= 28,
        "explicit import and send action stays inside its row");
      geometry.push(bounds);
      await fs.mkdir(dir, { recursive: true });
      await page.screenshot({ path: path.join(dir, `${screenshotPrefix}-offer-${width}.png`), fullPage: true });
    }
    await page.setViewportSize({ width: 1600, height: 1000 });
    await page.getByTestId("theme-toggle").click();
    assert.equal(await page.getByTestId("meerkat-console").getAttribute("data-cc-theme"), "dark");
    await page.screenshot({ path: path.join(dir, `${screenshotPrefix}-offer-dark-1600.png`), fullPage: true });
    await page.getByTestId("theme-toggle").click();
    assert.equal(sends().length, 0, "inspecting migration options never imports or submits old messages");
    assert.equal(await page.evaluate(key => localStorage.getItem(key), queueKey), null);
    assert(!(await requests()).some(request => JSON.stringify(request.messages).includes(text)));
    await fixture.control("model", { source: "Legacy import completed exactly once.", delay_ms: 0, chunk_chars: 32 });
    await importButton.click();
    await eventually(async () => {
      const response = await fetch(`${fixture.baseUrl}/console/timeline?identity=router%3Amain&mode=recent&limit=500`);
      assert.equal(response.status, 200);
      const frames = (await response.json()).frames;
      return frames.some(frame => frame.kind === "interaction_complete" && frame.payload.result === "Legacy import completed exactly once.");
    }, "explicit import reaches actual runtime completion");
    assert.equal(sends().length, 1, "one imported intent creates one POST");
    const submitted = JSON.parse(sends()[0].request);
    const envelope = submitted.params || submitted;
    assert.equal(envelope.content, text);
    assert.equal(envelope.identity, identity);
    await eventually(async () => !await importButton.count(), "one-time import action disappears");
    assert.equal(await page.evaluate(key => localStorage.getItem(key), legacyKey), legacyBytes, "original bytes are preserved");
    const saved = await page.evaluate(key => JSON.parse(localStorage.getItem(key)), queueKey);
    assert.equal(saved.legacyImported, true);
    await reload();
    await page.getByTestId(`chat-composer:${identity}`).waitFor();
    assert.equal(await page.getByRole("button", { name: "Copy work time", exact: true }).count(), 0,
      "work duration remains quiet metadata without a redundant copy action");
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    assert.equal(await importButton.count(), 0);
    assert.equal(sends().length, 1, "reload cannot resubmit the imported intent");
    assert.equal(await page.evaluate(key => localStorage.getItem(key), legacyKey), legacyBytes);
    if (embeddedHttp) {
      assert.equal(await page.getByTestId(`chat-composer:${identity}`).first().inputValue(), unsentDraft,
        "importing an older queued intent cannot erase the current unsent draft");
      assert(!(await requests()).some(request => JSON.stringify(request.messages).includes(unsentDraft)));
    }
    monitor.assertClean();
    await fs.mkdir(dir, { recursive: true });
    await page.screenshot({ path: path.join(dir, `${screenshotPrefix}-after-reload.png`), fullPage: true });
    await fs.writeFile(path.join(dir, `${screenshotPrefix}.json`), JSON.stringify({ embeddedHttp, namespace, saved, envelope, geometry, originalBytesPreserved: true, sends: sends(), errors, expectedFailures, requestFailures }, null, 2));
    monitor.assertClean();
  } catch (error) {
    await fs.mkdir(dir, { recursive: true });
    await page.screenshot({ path: path.join(dir, `${screenshotPrefix}-failure.png`), fullPage: true }).catch(() => {});
    await fs.writeFile(path.join(dir, `${screenshotPrefix}-failure.json`), JSON.stringify({ error: String(error), observations: fixture.observations, logs: fixture.logs(), errors, expectedFailures, requestFailures }, null, 2));
    throw error;
  } finally { monitor.stop(); await browser.close(); await fixture.close(); }
}

module.exports = { scenarios: [
  { id: "real-explicit-legacy-import", family: "real-send", backend: "real", run: explicitLegacyImport },
  { id: "real-embedded-http-legacy-import", family: "real-send", backend: "real", run: () => explicitLegacyImport({ embeddedHttp: true }) },
] };
if (require.main === module) require("../scenario-registry.cjs").runScenarios(module.exports.scenarios)
  .catch(error => { process.stderr.write(`${error.stack}\n`); process.exitCode = 1; });
