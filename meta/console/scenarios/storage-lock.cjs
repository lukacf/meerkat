"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const http = require("node:http");
const path = require("node:path");
const { build } = require("esbuild");
const { chromium } = require("playwright");

async function runIndexedDbStorageLock() {
  const bundle = await build({ entryPoints: [path.join(__dirname, "../src/lib/send-storage-lock.ts")], bundle: true,
    platform: "browser", format: "iife", globalName: "QueueLock", write: false });
  const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
  const server = http.createServer((req, res) => {
    res.writeHead(200, { "content-type": req.url === "/lock.js" ? "text/javascript" : "text/html" });
    res.end(req.url === "/lock.js" ? bundle.outputFiles[0].text : '<!doctype html><html><head><meta charset="utf-8"><title>Queue storage coordination proof</title></head><body><h1>Queue storage coordination</h1><p>Real Chromium IndexedDB, two same-origin tabs, plain HTTP.</p><pre id="proof">Running</pre><script src="/lock.js"></script></body></html>');
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const origin = `http://queue-lock.test:${server.address().port}`;
  const browser = await chromium.launch({ headless: true, args: ["--no-proxy-server", "--host-resolver-rules=MAP queue-lock.test 127.0.0.1"] });
  const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
  const first = await context.newPage();
  const second = await context.newPage();
  const errors = [];
  context.on("page", page => page.on("pageerror", error => errors.push(error.message)));
  for (const page of [first, second]) page.on("pageerror", error => errors.push(error.message));
  const proof = {};
  // A live readwrite transaction keeps requesting records until explicitly
  // released. It proves ordering without a timing window or artificial delay.
  async function holdTransaction(page) {
    await page.evaluate(async () => {
      const db = await new Promise((resolve, reject) => {
        const open = indexedDB.open("meta-console-queue-locks", 1);
        open.onupgradeneeded = () => open.result.createObjectStore("locks");
        open.onsuccess = () => resolve(open.result); open.onerror = () => reject(open.error);
      });
      await new Promise((resolve, reject) => {
        const transaction = db.transaction("locks", "readwrite");
        let active = true;
        window.releaseHeldTransaction = () => { active = false; transaction.abort(); };
        transaction.onabort = transaction.oncomplete = () => db.close();
        const pump = () => {
          const request = transaction.objectStore("locks").get("counter");
          request.onsuccess = () => { resolve(); if (active) pump(); };
          request.onerror = () => reject(request.error);
        };
        pump();
      });
    });
  }
  async function queueIncrement(page) {
    await page.evaluate(() => {
      window.queuedCallbackRan = false;
      window.queuedResult = null;
      window.queuedIncrement = QueueLock.withConsoleSendStorageLock("counter", () => {
        window.queuedCallbackRan = true;
        const next = Number(localStorage.getItem("counter") || "0") + 1;
        localStorage.setItem("counter", String(next));
        return next;
      });
      window.queuedIncrement.then(value => { window.queuedResult = { value }; }, error => {
        window.queuedResult = { error: error.message };
      });
    });
  }
  async function completedIncrement(page) {
    await page.waitForFunction(() => window.queuedResult !== null, null, { timeout: 10_000 });
    const result = await page.evaluate(() => window.queuedResult);
    assert.equal(result.error, undefined);
    return result.value;
  }
  try {
    await Promise.all([first.goto(origin), second.goto(origin)]);
    proof.environment = await first.evaluate(() => ({ secure: isSecureContext, webLocks: Boolean(navigator.locks), indexedDB: Boolean(indexedDB) }));
    assert.deepEqual(proof.environment, { secure: false, webLocks: false, indexedDB: true });
    await first.evaluate(() => localStorage.setItem("counter", "0"));
    await holdTransaction(first);
    await queueIncrement(second);
    // Two evaluations cross browser task boundaries; transaction ownership,
    // rather than elapsed time, is the reason the callback must still wait.
    assert.equal(await second.evaluate(() => window.queuedCallbackRan), false, "another tab's active IndexedDB transaction must block the callback");
    await first.evaluate(() => window.releaseHeldTransaction());
    assert.equal(await completedIncrement(second), 1);
    proof.heldTransactionSerialized = true;

    const count = 40;
    const values = await Promise.all([first, second].map(page => page.evaluate(count => Promise.all(Array.from({ length: count }, () =>
      QueueLock.withConsoleSendStorageLock("counter", () => {
        const next = Number(localStorage.getItem("counter")) + 1;
        localStorage.setItem("counter", String(next));
        return next;
      }))), count)));
    const ordered = values.flat().sort((a, b) => a - b);
    assert.deepEqual(ordered, Array.from({ length: count * 2 }, (_, index) => index + 2), "every cross-tab increment must be unique and retained");
    assert.equal(await first.evaluate(() => Number(localStorage.getItem("counter"))), 81);
    proof.concurrentCallbacks = count * 2;
    proof.counterAfterConcurrentCallbacks = 81;

    const failed = await first.evaluate(async () => {
      try { await QueueLock.withConsoleSendStorageLock("counter", () => { throw new Error("intentional callback failure"); }); return null; }
      catch (error) { return error.message; }
    });
    assert.equal(failed, "intentional callback failure");
    await queueIncrement(second);
    assert.equal(await completedIncrement(second), 82);
    proof.failedCallbackReleased = true;

    await holdTransaction(first);
    await queueIncrement(second);
    assert.equal(await second.evaluate(() => window.queuedCallbackRan), false);
    await first.reload();
    assert.equal(await completedIncrement(second), 83);
    proof.reloadedTabReleased = true;

    await holdTransaction(first);
    await queueIncrement(second);
    assert.equal(await second.evaluate(() => window.queuedCallbackRan), false);
    await first.close();
    assert.equal(await completedIncrement(second), 84);
    proof.closedTabReleased = true;
    assert.deepEqual(errors, []);
    proof.errors = errors;
    await second.evaluate(proof => { document.getElementById("proof").textContent = JSON.stringify(proof, null, 2); }, proof);
    await fs.mkdir(evidence, { recursive: true });
    await second.screenshot({ path: path.join(evidence, "storage-lock-indexeddb.png"), fullPage: true });
    await fs.writeFile(path.join(evidence, "storage-lock-indexeddb.json"), JSON.stringify(proof, null, 2) + "\n");
    process.stdout.write("browser IndexedDB queue storage lock ok\n");
  } catch (error) {
    await fs.mkdir(evidence, { recursive: true });
    await fs.writeFile(path.join(evidence, "storage-lock-indexeddb-failure.json"), JSON.stringify({ proof, errors, error: String(error) }, null, 2) + "\n");
    throw error;
  } finally { await browser.close(); await new Promise(resolve => server.close(resolve)); }
}
module.exports = { scenarios: [{ id: "storage-lock-indexeddb", family: "console", backend: "mock", run: runIndexedDbStorageLock }] };
if (require.main === module) require("../scenario-registry.cjs").runScenarios(module.exports.scenarios)
  .catch(error => { process.stderr.write(`${error.stack}\n`); process.exitCode = 1; });
