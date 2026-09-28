import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import http from "node:http";
import path from "node:path";
import test from "node:test";
import { fileURLToPath } from "node:url";
import { chromium } from "playwright";

const root = path.resolve(fileURLToPath(new URL("..", import.meta.url)));

test("OAuth host example registers and clears the real WASM resolver", { timeout: 30_000 }, async () => {
  const server = http.createServer(async (request, response) => {
    try {
      const pathname = new URL(request.url, "http://localhost").pathname;
      const filename = path.resolve(root, `.${pathname}`);
      if (!filename.startsWith(root + path.sep)) throw new Error("invalid path");
      const bytes = await readFile(filename);
      response.writeHead(200, { "content-type": filename.endsWith(".wasm")
        ? "application/wasm" : filename.endsWith(".html") ? "text/html" : "text/javascript" });
      response.end(bytes);
    } catch {
      response.writeHead(404);
      response.end("not found");
    }
  });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const browser = await chromium.launch({ headless: true });
  try {
    const page = await browser.newPage();
    const errors = [];
    page.on("pageerror", error => errors.push(error.message));
    await page.goto(`http://127.0.0.1:${server.address().port}/examples/oauth-host.html`);
    await page.waitForFunction(() => document.getElementById("status").textContent === "Resolver cleared.");
    for (let cycle = 0; cycle < 2; cycle++) {
      await page.locator("#register").click();
      assert.equal(await page.locator("#status").textContent(), "Resolver registered.");
      await page.locator("#clear").click();
      assert.equal(await page.locator("#status").textContent(), "Resolver cleared.");
      assert.equal(await page.locator("#clear").isDisabled(), true);
    }
    assert.deepEqual(errors, []);
  } finally {
    await browser.close();
    await new Promise((resolve, reject) => server.close(error => error ? reject(error) : resolve()));
  }
});
