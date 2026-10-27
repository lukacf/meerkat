"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { randomUUID, createHash } = require("node:crypto");
const { chromium } = require("playwright");
const { startFixture, eventually, rpc } = require("../acceptance-runtime.cjs");

const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
const identity = "router:main";

async function uploadReviewImage(fixture, browser) {
  const canvas = await browser.newPage({ viewport: { width: 440, height: 180 }, deviceScaleFactor: 1 });
  let bytes;
  try {
    await canvas.setContent('<html><body style="margin:0"><svg xmlns="http://www.w3.org/2000/svg" width="440" height="180"><rect width="440" height="180" rx="14" fill="#f3f1eb"/><g font-family="Arial" fill="#292a28"><text x="26" y="40" font-size="18">Release evidence</text><rect x="26" y="66" width="162" height="66" rx="10" fill="#dbead7"/><text x="48" y="104" font-size="16">Review complete</text><path d="M204 99h31m-8-7 8 7-8 7" fill="none" stroke="#79806e" stroke-width="2"/><rect x="251" y="66" width="162" height="66" rx="10" fill="#fff"/><text x="274" y="104" font-size="16">Publish report</text></g></svg></body></html>');
    bytes = await canvas.screenshot();
  } finally { await canvas.close(); }
  const uploadId = randomUUID();
  const form = new FormData();
  form.append(`file:${uploadId}`, new Blob([bytes], { type: "image/png" }), "release-evidence.png");
  form.append("payload", JSON.stringify({ jsonrpc: "2.0", id: uploadId, method: "meta/blob/upload", params: {
    upload: { type: "image_upload", upload_id: uploadId, media_type: "image/png", alt: "Release evidence" },
  } }));
  const response = await fetch(`${fixture.baseUrl}/console/rpc/multipart`, { method: "POST", body: form });
  assert.equal(response.status, 200);
  const body = await response.json();
  assert(body.result?.blob_id && !body.error, JSON.stringify(body));
  return { blobId: body.result.blob_id, bytes };
}

async function seedReply(fixture, source) {
  await fixture.control("model", { source, delay_ms: 0, chunk_chars: 128 });
  const reply = await rpc(fixture.baseUrl, "meta/console/send", {
    identity, content: "Present the release evidence with links approved by this console host.",
    origin: "console:markdown-policy", origin_kind: "operator", idempotency_key: randomUUID(),
  });
  assert.equal(reply.status, 200); assert(reply.body.result?.interaction_id, JSON.stringify(reply.body));
  const accepted = reply.body.result;
  const terminal = await eventually(async () => {
    const response = await fetch(`${fixture.baseUrl}/console/timeline?identity=${encodeURIComponent(identity)}&mode=recent&limit=500`);
    assert.equal(response.status, 200);
    const { frames } = await response.json();
    return frames.find(frame => frame.interaction_id === accepted.interaction_id && frame.kind === "interaction_complete" && frame.payload?.result === source);
  }, "exact accepted policy reply completes", 40_000);
  return { accepted, terminalId: terminal.id };
}

function initializationCancellation(request) {
  if (!request.failure()?.errorText?.includes("ERR_ABORTED")) return false;
  if (new URL(request.url()).pathname.endsWith("/timeline/stream")) return true;
  try {
    const body = JSON.parse(request.postData() || "{}");
    return body.method === "meta/console/query_timeline" && body.params?.mode === "recent" && body.params?.limit === 200 && !body.params?.identity;
  } catch { return false; }
}

async function urlPolicy(host) {
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: true });
  const result = { host, cases: [] };
  let currentPage;
  try {
    const image = await uploadReviewImage(fixture, browser);
    const approvedUrl = `${fixture.baseUrl}/blobs/${encodeURIComponent(image.blobId)}`;
    // This same-origin URL is deliberately absent from the host image allowlist.
    const deniedUrl = `${fixture.baseUrl}/blobs/not-approved-by-host`;
    const source = `## Release review\n\nThis report includes the evidence approved for this console.\n\n[Relative review](/release/review) - [App review](meta:review) - [Reference](https://example.com/reference) - [Denied link](https://denied.invalid/link)\n\n![Approved release evidence](${approvedUrl})\n\n![Denied local image](${deniedUrl}) ![Denied external image](https://denied.invalid/tracker.png)\n\n[Unsafe script](javascript:alert%281%29)\n\nPolicy review complete.`;
    result.owner = await seedReply(fixture, source);
    result.blobId = image.blobId;
    for (const mode of ["default", "custom"]) {
      const context = await browser.newContext({ viewport: { width: 1600, height: 1000 } });
      const page = currentPage = await context.newPage();
      const errors = [], expectedCancellations = [], imageRequests = [], externalRequests = [];
      let initializing = true;
      page.on("pageerror", error => errors.push(error.message));
      page.on("request", request => {
        if (request.resourceType() === "image") imageRequests.push(request.url());
        // The console carries its own fonts, so nothing, not even a font,
        // is fetched from another origin.
        if (new URL(request.url()).origin !== fixture.baseUrl) externalRequests.push(request.url());
      });
      page.on("requestfailed", request => {
        const detail = { url: request.url(), error: request.failure()?.errorText };
        if (initializing && initializationCancellation(request)) expectedCancellations.push(detail);
        else errors.push(detail);
      });
      try {
        const options = new URLSearchParams(mode === "custom" ? { markdownPolicy: "custom", approvedImage: image.blobId } : {});
        const hostPath = host === "stock" ? "/scoped" : "/shared";
        await page.goto(`${fixture.baseUrl}${hostPath}?${options}`);
        if (host === "stock") await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ has: page.getByText(identity, { exact: true }) }).first().click();
        await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
        initializing = false;
        const viewport = page.locator(host === "stock" ? ".conv__body" : '[data-testid="shared-pane-0"] .cc-conversation-pane__scroll').first();
        const document = viewport.locator(".cc-markdown-document").filter({ hasText: "Policy review complete." });
        await document.waitFor();
        assert.equal(await document.count(), 1, "one real completed reply uses the Markdown document renderer");
        assert.equal(await document.getByRole("link", { name: "Unsafe script", exact: true }).count(), 0);
        assert.equal(await document.getByRole("img", { name: "Denied local image", exact: true }).count(), 0);
        assert.equal(await document.getByRole("img", { name: "Denied external image", exact: true }).count(), 0);
        assert.equal(await document.getByRole("link", { name: "Reference", exact: true }).getAttribute("href"), "https://example.com/reference");
        const approved = document.getByRole("img", { name: "Approved release evidence", exact: true });
        if (mode === "custom") {
          for (const name of ["Relative review", "App review"]) {
            const link = document.getByRole("link", { name, exact: true });
            assert.equal(await link.getAttribute("href"), `${fixture.baseUrl}/console#release-review`);
            assert.equal(await link.getAttribute("rel"), "noopener noreferrer");
          }
          assert.equal(await document.getByRole("link", { name: "Denied link", exact: true }).count(), 0);
          await approved.scrollIntoViewIfNeeded();
          await eventually(() => approved.evaluate(node => node.complete && node.naturalWidth === 440 && node.naturalHeight === 180), "approved real blob image decodes");
          assert.equal(await approved.getAttribute("src"), approvedUrl);
          const delivered = await context.request.get(approvedUrl);
          assert.equal(delivered.status(), 200);
          assert.equal(createHash("sha256").update(await delivered.body()).digest("hex"), createHash("sha256").update(image.bytes).digest("hex"), "actual authorized blob bytes match the uploaded image");
          assert.deepEqual(imageRequests, [approvedUrl], "only the exact host-approved image is requested");
        } else {
          for (const name of ["Relative review", "App review"]) assert.equal(await document.getByRole("link", { name, exact: true }).count(), 0);
          assert.equal(await approved.count(), 0, "default policy requires explicit approval even for same-origin image");
          assert.equal(await document.getByRole("link", { name: "Denied link", exact: true }).getAttribute("href"), "https://denied.invalid/link", "default HTTPS links remain links until a host restricts them");
          assert.deepEqual(imageRequests, [], "default renderer issues no image request");
        }
        assert.deepEqual(externalRequests, [], "rendering fetches nothing from another origin: no tracking URL and no hosted font");
        assert.deepEqual(errors, []);
        await fs.mkdir(evidence, { recursive: true });
        await page.screenshot({ path: path.join(evidence, `${host}-markdown-policy-${mode}.png`), fullPage: true });
        result.cases.push({ mode, imageRequests, externalRequests, errors, expectedCancellations });
      } catch (error) {
        result.cases.push({ mode, imageRequests, externalRequests, errors, expectedCancellations, failure: error.stack || String(error) });
        await fs.mkdir(evidence, { recursive: true });
        await page.screenshot({ path: path.join(evidence, `${host}-markdown-policy-${mode}-failure.png`), fullPage: true }).catch(() => {});
        throw error;
      } finally { await context.close(); currentPage = null; }
    }
  } catch (error) {
    result.failure = error.stack || String(error);
    if (currentPage) await currentPage.screenshot({ path: path.join(evidence, `${host}-markdown-policy-failure.png`), fullPage: true }).catch(() => {});
    throw error;
  } finally {
    await fs.mkdir(evidence, { recursive: true });
    await fs.writeFile(path.join(evidence, `${host}-markdown-policy.json`), JSON.stringify(result, null, 2));
    await browser.close(); await fixture.close();
  }
}

const scenarios = ["stock", "shared"].map(host => ({ id: `real-${host}-markdown-url-policy`, family: "real-presentation", backend: "real", run: () => urlPolicy(host) }));
module.exports = { scenarios };
if (require.main === module) {
  assert(process.argv.includes("--list") || process.env.META_EXAMPLE_BIN_DIR, "Use the coordinator's prebuilt fixture; this scenario must not compile Rust.");
  require("../scenario-registry.cjs").runScenarios(scenarios).catch(error => { process.stderr.write(`${error.stack || error}\n`); process.exitCode = 1; });
}
