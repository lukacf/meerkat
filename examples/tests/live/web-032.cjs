// Example 032 (WebCM mob) in headless Chromium: the dev server, real providers through the proxy,
// one prompt, a bounded time. Exit 0 when the VM booted, the prompt ran and no provider call failed.
// usage: node web-032.cjs <node_modules with playwright> <app url with ?proxy=> <seconds after the prompt>
const path = require("node:path");
const { chromium } = require(path.join(process.argv[2], "playwright"));
const url = process.argv[3];
const after = Number(process.argv[4] || 150) * 1000;
(async () => {
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage();
  const calls = {}, bad = [], errors = [], models = {};
  let aborted = 0;
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
  // A provider request that never got a response (a network error) raises
  // this event and no response event, so it joins the same list. A request
  // the page cancelled itself is not a failure of the provider.
  page.on("requestfailed", (request) => {
    const match = /\/(anthropic|openai|gemini)\//.exec(request.url());
    if (!match) return;
    const reason = (request.failure() || {}).errorText || "request failed";
    if (reason === "net::ERR_ABORTED") { aborted += 1; return; }
    if (bad.length < 5) bad.push(match[1] + " " + reason);
  });
  page.on("pageerror", (error) => errors.push(error.message.slice(0, 200)));
  await page.goto(url);
  await page.click("#boot-btn");
  let last = "";
  const t0 = Date.now();
  let booted = false;
  while (Date.now() - t0 < 120000) {
    const status = (await page.locator("#boot-status").textContent().catch(() => "")) || "";
    if (status !== last) { last = status; console.log(new Date().toISOString().slice(11, 19), "boot:", status.slice(0, 160)); }
    if (await page.locator("#workspace").isVisible().catch(() => false) && await page.locator("#prompt").isEnabled().catch(() => false)) { booted = true; break; }
    await page.waitForTimeout(1000);
  }
  let outcome = booted ? "booted" : "boot timeout";
  if (booted) {
    const tags = {};
    for (const role of ["orchestrator", "planner", "coder", "reviewer"]) tags[role] = (await page.locator("#model-" + role).textContent().catch(() => "")) || "";
    console.log("model tags on the page:", JSON.stringify(tags));
    await page.fill("#prompt", "Ask the planner for a two-step plan, have the coder write hello.sh that prints hello and run it in the VM, and have the reviewer check the output. Keep every message short.");
    await page.press("#prompt", "Enter");
    const t1 = Date.now();
    const seen = {}, first = {};
    while (Date.now() - t1 < after) {
      for (const role of ["orchestrator", "planner", "coder", "reviewer"]) {
        const s = ((await page.locator("#status-" + role).textContent().catch(() => "")) || "").trim();
        if (!(role in first)) first[role] = s;
        if (seen[role] !== s) { seen[role] = s; console.log(new Date().toISOString().slice(11, 19), role + ":", s.slice(0, 60)); }
      }
      // Enough once every role has left the status it had when the prompt was sent.
      const moved = Object.keys(first).filter((role) => seen[role] !== first[role]).length;
      if (moved >= 4 && Date.now() - t1 > 60000) break;
      await page.waitForTimeout(1000);
    }
    outcome = "prompt ran";
  }
  const main = await page.evaluate(() => (document.getElementById("main-stream")?.textContent || "").trim().slice(-400));
  const ok = Object.entries(calls).filter(([k]) => k.endsWith(" 200")).reduce((a, [, v]) => a + v, 0);
  console.log("OUTCOME:", outcome, "| provider 200 responses:", ok);
  console.log("provider responses (provider status: count):", JSON.stringify(calls));
  console.log("models named in requests:", JSON.stringify(models));
  console.log("failed responses and requests:", JSON.stringify(bad), "| requests the page cancelled:", aborted);
  console.log("page errors:", JSON.stringify(errors));
  console.log("end of the main stream:", JSON.stringify(main));
  // Decide before the browser closes: closing cancels what is still open.
  const passed = outcome === "prompt ran" && ok > 0 && bad.length === 0;
  await browser.close();
  process.exit(passed ? 0 : 1);
})();
