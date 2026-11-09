const { spawn } = require("node:child_process");
const { dirname, join, resolve } = require("node:path");
const { fileURLToPath } = require("node:url");
const { chromium } = require("playwright");

const here = __dirname;
const examplesRoot = resolve(here, "..");

function waitForLine(child, pattern, timeoutMs = 120000) {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`timed out waiting for ${pattern}`)), timeoutMs);
    const onData = (chunk) => {
      const text = chunk.toString();
      process.stdout.write(text);
      const match = text.match(pattern);
      if (match) {
        clearTimeout(timer);
        child.stdout.off("data", onData);
        resolve(match[1]);
      }
    };
    child.stdout.on("data", onData);
    child.stderr.on("data", (chunk) => process.stderr.write(chunk.toString()));
  });
}

(async () => {
  const child = spawn(
    process.execPath,
    [
      join(examplesRoot, "node_modules/tsx/dist/cli.mjs"),
      join(here, "run.ts"),
      "--browser-smoke",
      "--allow-empty-targets",
      "--demo-llm",
      "--skip-build",
    ],
    { cwd: examplesRoot, env: process.env, stdio: ["ignore", "pipe", "pipe"] },
  );
  let browser;
  try {
    const consoleUrl = await waitForLine(child, /\[mdm\] console: (http:\/\/[^\s]+)/);
    browser = await chromium.launch();
    const page = await browser.newPage({ viewport: { width: 1440, height: 960 } });
    await page.goto(consoleUrl, { waitUntil: "domcontentloaded", timeout: 120000 });
    await page.getByText("MDM Console").first().waitFor({ timeout: 60000 });
    await page.getByText("Hive").first().waitFor({ timeout: 60000 });
    const screenshot = join(here, ".state", "mdm-console-browser-smoke.png");
    await page.screenshot({ path: screenshot, fullPage: true });
    console.log(`[mdm-browser-smoke] screenshot: ${screenshot}`);
    console.log("[mdm-browser-smoke] ok");
  } finally {
    if (browser) await browser.close();
    child.kill("SIGTERM");
  }
})().catch((error) => {
  console.error(error.stack || error);
  process.exitCode = 1;
});
