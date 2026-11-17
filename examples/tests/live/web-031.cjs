// Example 031 (mini diplomacy) in headless Chromium: the served build, real providers through the proxy,
// the first round only. Exit 0 when round 1 resolved.
// usage: node web-031.cjs <node_modules with playwright> <app url with ?proxy= and the model parameters> <seconds>
const path = require("node:path");
const { chromium } = require(path.join(process.argv[2], "playwright"));
const url = process.argv[3];
const limit = Number(process.argv[4] || 300) * 1000;
(async () => {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  const calls = {}, bad = [], errors = [];
  page.on("response", (response) => {
    const match = /\/(anthropic|openai|gemini)\//.exec(response.url());
    if (!match) return;
    const key = match[1] + " " + response.status();
    calls[key] = (calls[key] || 0) + 1;
    if (response.status() >= 400 && bad.length < 5) response.text().then((t) => bad.push(match[1] + " " + response.status() + " " + t.slice(0, 300))).catch(() => {});
  });
  page.on("pageerror", (error) => errors.push(error.message.slice(0, 200)));
  const seen = [];
  const note = async () => {
    const status = await page.locator("#statusLine").textContent().catch(() => null);
    const badge = await page.locator("#statusBadge").textContent().catch(() => null);
    const line = `${badge} | ${status}`;
    if (seen[seen.length - 1] !== line) { seen.push(line); console.log(new Date().toISOString().slice(11, 19), line); }
    return { status, badge };
  };
  await page.goto(url);
  const started = Date.now();
  let dismissed = false, outcome = "timeout";
  while (Date.now() - started < limit) {
    const { status, badge } = await note();
    if (!dismissed && await page.locator("#guideOverlay:not(.hidden) #guideDismiss").count()) { await page.click("#guideDismiss"); dismissed = true; console.log("guide dismissed"); }
    if (badge === "Error") { outcome = "error"; break; }
    if (/Round 1 resolved/.test(status || "") || /Round 2/.test(status || "")) { outcome = "round 1 resolved"; break; }
    await page.waitForTimeout(1000);
  }
  const feed = await page.evaluate(() => Array.from(document.querySelectorAll("[class*=dispatch], [class*=message], [class*=feed] *")).map((e) => e.textContent.trim()).filter((t) => t.length > 20).slice(0, 6).map((t) => t.slice(0, 160)));
  console.log("OUTCOME:", outcome, "after", Math.round((Date.now() - started) / 1000), "s");
  console.log("provider responses (provider status: count):", JSON.stringify(calls));
  console.log("failed responses:", JSON.stringify(bad));
  console.log("page errors:", JSON.stringify(errors));
  console.log("feed sample:", JSON.stringify(feed));
  await browser.close();
  process.exit(outcome === "round 1 resolved" ? 0 : 1);
})();
