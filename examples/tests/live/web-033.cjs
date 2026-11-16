// Example 033 (the office) in headless Chromium: the served build, a real provider through the proxy,
// one scenario, a bounded time. Exit 0 when the office started, the scenario ran and no provider call failed.
// usage: node web-033.cjs <node_modules with playwright> <app url with ?proxy= and &model=> <seconds after the scenario>
const path = require("node:path");
const { chromium } = require(path.join(process.argv[2], "playwright"));
const url = process.argv[3];
const after = Number(process.argv[4] || 60) * 1000;
(async () => {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  const calls = {}, bad = [], errors = [], models = {};
  page.on("request", (request) => {
    if (!/\/(anthropic|openai|gemini)\//.test(request.url())) return;
    let model = null;
    try { model = JSON.parse(request.postData() || "{}").model; } catch {}
    if (!model) { const m = /models\/([^:\/?]+)/.exec(request.url()); if (m) model = m[1]; }
    models[model] = (models[model] || 0) + 1;
  });
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
    if (seen[seen.length - 1] !== line) { seen.push(line); console.log(new Date().toISOString().slice(11, 19), line.slice(0, 200)); }
    return { status, badge };
  };
  await page.goto(url);
  await page.waitForSelector("#statusBadge", { timeout: 20000 });
  await page.waitForTimeout(1500);
  await note();
  // With a proxy in the address the office starts by itself; otherwise press start.
  if (await page.locator("#startBigBtn").isVisible()) { await page.click("#startBigBtn"); console.log("pressed start"); }
  let outcome = "start timeout";
  const started = Date.now();
  while (Date.now() - started < 180000) {
    const { status, badge } = await note();
    if (badge === "ERROR" || badge === "CONFIG") { outcome = "error at start"; break; }
    if (badge === "LIVE" || /Office running/.test(status || "")) { outcome = "running"; break; }
    await page.waitForTimeout(1000);
  }
  if (outcome === "running") {
    const scenario = await page.locator("[data-scenario]").first();
    console.log("injecting scenario:", (await scenario.textContent()).trim());
    await scenario.click();
    const t0 = Date.now();
    while (Date.now() - t0 < after) {
      const { badge } = await note();
      if (badge === "ERROR" || badge === "CONFIG") { outcome = "error after the scenario"; break; }
      await page.waitForTimeout(1000);
    }
    if (outcome === "running") outcome = "scenario ran";
  }
  const log = await page.evaluate(() => Array.from(document.querySelectorAll("[class*=log] [class*=entry], [class*=log-item], [class*=bubble]")).map((e) => e.textContent.trim()).filter((t) => t.length > 15).slice(-5).map((t) => t.slice(0, 180)));
  const ok = Object.entries(calls).filter(([k]) => k.endsWith(" 200")).reduce((a, [, v]) => a + v, 0);
  console.log("OUTCOME:", outcome, "| provider 200 responses:", ok);
  console.log("provider responses (provider status: count):", JSON.stringify(calls));
  console.log("models named in requests:", JSON.stringify(models));
  console.log("failed responses:", JSON.stringify(bad));
  console.log("page errors:", JSON.stringify(errors));
  console.log("log sample:", JSON.stringify(log));
  await page.click("#pauseBtn").catch(() => {});
  await page.waitForTimeout(1500);
  await browser.close();
  process.exit(outcome === "scenario ran" && ok > 0 && bad.length === 0 ? 0 : 1);
})();
