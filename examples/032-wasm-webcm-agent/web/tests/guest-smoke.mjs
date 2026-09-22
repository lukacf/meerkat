import assert from "node:assert/strict";
import { mkdir, rm } from "node:fs/promises";
import { resolve } from "node:path";
import { createServer } from "vite";
import { chromium } from "playwright";

const work = resolve(".work/guest-smoke");
await mkdir(work, { recursive: true });
process.env.TMPDIR = work;
const server = await createServer({
  configFile: false, root: process.cwd(),
  server: {
    host: "127.0.0.1", port: 0,
    headers: { "Cross-Origin-Opener-Policy": "same-origin", "Cross-Origin-Embedder-Policy": "require-corp" },
  },
});
await server.listen();
const origin = `http://127.0.0.1:${server.httpServer.address().port}`;
let browser, timeout;
const requests = [], status = [], diagnostics = [];
try {
  browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  await page.route("**/*", route => {
    const url = route.request().url();
    if (!url.startsWith(origin + "/")) {
      diagnostics.push(`Denied off-origin request: ${new URL(url).origin}${new URL(url).pathname}`);
      return route.abort();
    }
    if (url === origin + "/") return route.fulfill({
      contentType: "text/html",
      body: "<div id='terminal' style='width:900px;height:600px'></div>",
      headers: { "Cross-Origin-Opener-Policy": "same-origin", "Cross-Origin-Embedder-Policy": "require-corp" },
    });
    return route.continue();
  });
  page.on("response", response => {
    const path = new URL(response.url()).pathname;
    if (path.includes("webcm")) requests.push({ path, status: response.status() });
  });
  page.on("pageerror", error => diagnostics.push(error.message));
  await page.exposeFunction("recordStatus", message => status.push(message));
  await page.goto(origin);
  const result = await Promise.race([
    page.evaluate(async () => {
      const { WebCMHost } = await import("/src/webcm-host.ts");
      const host = new WebCMHost();
      window.guestHost = host;
      await host.boot(document.getElementById("terminal"), message => window.recordStatus(message));
      const root = `/workspace/meerkat-smoke-${crypto.randomUUID()}`;
      const mkdir = await host.exec(`mkdir -p '${root}'`, 10_000);
      if (mkdir.exitCode !== 0) throw Error(`guest mkdir: ${JSON.stringify(mkdir)}`);
      try {
        const paths = [`${root}/space name`, `${root}/apostrophe'name`, `${root}/$cash*`, `${root}/-dash`];
        const contents = ["  no final newline  ", "line one\r\nline two\n\n", "Unicode é 🐾\n".repeat(100)];
        for (const path of paths) {
          for (const content of contents) {
            const write = await host.writeFile(path, content);
            if (write.exitCode !== 0) throw Error(`guest write: ${JSON.stringify(write)}`);
            const read = await host.readFile(path);
            if (read !== content) throw Error(`guest read mismatch: expected=${JSON.stringify(content)} actual=${JSON.stringify(read)}`);
          }
        }
        for (const [command, output, exitCode] of [
          ["printf abc", "abc", 0],
          ["printf '  x\\n\\n '", "  x\n\n ", 0],
          ["printf one\nprintf two # comment", "onetwo", 0],
          ["printf failed; false # comment", "failed", 1],
        ]) {
          const result = await host.exec(command, 10_000);
          if (result.output !== output || result.exitCode !== exitCode) throw Error(`guest exec mismatch: ${JSON.stringify({ command, expected: { output, exitCode }, actual: result })}`);
        }
        return { booted: host.isBooted(), crossOriginIsolated, fileRoundTrips: paths.length * contents.length, commands: 4 };
      } finally {
        await host.exec(`rm -rf -- '${root}'`, 10_000);
      }
    }),
    new Promise((_, reject) => { timeout = setTimeout(() => reject(Error("Real WebCM smoke exceeded 180-second total budget")), 180_000); }),
  ]);
  assert.equal(result.booted, true);
  assert.equal(diagnostics.filter(d => d.startsWith("Denied off-origin")).length, 0);
  console.log(JSON.stringify({ outcome: "pass", ...result, requests, status, diagnostics }, null, 2));
} catch (error) {
  console.error(JSON.stringify({ outcome: "fail", error: error.message, requests, status, diagnostics }, null, 2));
  process.exitCode = 1;
} finally {
  clearTimeout(timeout);
  await browser?.close();
  await server.close();
  await rm(work, { recursive: true, force: true });
}
