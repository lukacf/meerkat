"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { randomUUID } = require("node:crypto");
const { chromium } = require("playwright");
const { startFixture, eventually } = require("../acceptance-runtime.cjs");
const { assertSidebarCompletion, waitForExplicitIdleRoster, waitForSidebarCompletion } = require("../sidebar-activity-oracle.cjs");

const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
const router = "router:main";
const delivery = "domain:delivery";
const activePhases = new Set(["waiting", "tool-executing", "generating"]);

async function ownerRoster(fixture) {
  const response = await fetch(`${fixture.baseUrl}/console/experience`);
  assert.equal(response.status, 200);
  const experience = await response.json();
  const agents = experience.agent_sidebar?.live_snapshot?.agents;
  assert(Array.isArray(agents), "experience contains the actual owner roster");
  assert.equal(agents.length, 2, "this fixture owns exactly two live agents");
  return agents;
}
async function timeline(fixture) {
  const response = await fetch(`${fixture.baseUrl}/console/timeline?identity=router%3Amain&mode=recent&limit=1000`);
  assert.equal(response.status, 200);
  return (await response.json()).frames;
}
function initializationCancellation(request) {
  if (!request.failure()?.errorText?.includes("ERR_ABORTED")) return false;
  if (new URL(request.url()).pathname.endsWith("/timeline/stream")) return true;
  try {
    const body = JSON.parse(request.postData() || "{}");
    return body.method === "meta/console/query_timeline" && body.params?.mode === "recent"
      && body.params?.limit === 200 && !body.params?.identity;
  } catch { return false; }
}

async function sidebarActivity() {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Use the coordinator's prebuilt fixture; this scenario must not compile Rust.");
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  page.setDefaultTimeout(15_000);
  const result = { errors: [], expectedCancellations: [], checks: [], geometry: [], initialOwnerSamples: [], finalOwnerSamples: [], completionSamples: [] };
  let initializing = true;
  let barrierId;
  page.on("pageerror", error => result.errors.push(error.message));
  page.on("requestfailed", request => {
    const detail = { url: request.url(), error: request.failure()?.errorText };
    if (initializing && initializationCancellation(request)) result.expectedCancellations.push(detail);
    else result.errors.push(detail);
  });
  const sidebar = () => page.getByTestId("sidebar-root");
  const activity = () => sidebar().getByRole("group", { name: "Agent activity", exact: true });
  const rows = () => sidebar().locator('[data-testid^="sidebar-agent:"]');
  const search = () => page.getByTestId("sidebar-search");
  async function visibleIds() {
    return rows().evaluateAll(nodes => nodes.filter(node => node.getClientRects().length)
      .map(node => node.getAttribute("data-testid").slice("sidebar-agent:".length)));
  }
  async function assertRows(expected, label) {
    const started = Date.now();
    await eventually(async () => JSON.stringify(await visibleIds()) === JSON.stringify(expected), label, 5_000);
    const actual = await visibleIds();
    assert.deepEqual(actual, expected, `${label}: exact roster IDs and order`);
    result.checks.push({ label, ids: actual, convergenceMs: Date.now() - started });
  }
  async function choose(name, expected, label = name) {
    const button = activity().getByRole("button", { name, exact: true });
    await button.click();
    assert.equal(await button.getAttribute("aria-pressed"), "true");
    assert.equal(await activity().locator('[aria-pressed="true"]').count(), 1, "exactly one activity filter selected");
    await assertRows(expected, label);
    if (!expected.length && (name !== "All" || await search().inputValue())) {
      await sidebar().getByRole("status").filter({ hasText: "No matching agents." }).waitFor();
    }
  }
  async function capture(label) {
    const geometry = await activity().evaluate(group => {
      const box = node => { const r = node.getBoundingClientRect(); return { x: r.x, y: r.y, right: r.right, bottom: r.bottom, width: r.width, height: r.height }; };
      return { viewport: { width: innerWidth, height: innerHeight }, sidebar: box(group.closest("aside")), group: box(group),
        buttons: [...group.querySelectorAll("button")].map(button => ({ name: button.textContent, selected: button.getAttribute("aria-pressed"), ...box(button),
          clientWidth: button.clientWidth, scrollWidth: button.scrollWidth })),
        documentWidth: document.documentElement.scrollWidth };
    });
    for (const button of geometry.buttons) {
      assert(button.width >= 32 && button.height >= 22, `${label}: usable ${button.name} target`);
      assert(button.x >= geometry.sidebar.x && button.right <= geometry.sidebar.right + 1, `${label}: ${button.name} remains inside sidebar`);
      assert(button.scrollWidth <= button.clientWidth + 1, `${label}: ${button.name} label fits`);
    }
    assert(geometry.documentWidth <= geometry.viewport.width + 1, `${label}: no horizontal document overflow`);
    result.geometry.push({ label, ...geometry });
    await fs.mkdir(evidence, { recursive: true });
    await page.screenshot({ path: path.join(evidence, `stock-sidebar-activity-${label}.png`), fullPage: true });
  }
  try {
    // Runtime startup readiness can precede the console's terminal phase projection.
    result.initialOwner = await waitForExplicitIdleRoster({
      readRoster: () => ownerRoster(fixture),
      onObservation: agents => result.initialOwnerSamples.push({ at: Date.now(), agents }),
    });
    await page.goto(`${fixture.baseUrl}/console`);
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    await activity().waitFor();
    await assertRows([delivery, router], "initial owner order");
    initializing = false;
    const baselineOrder = await visibleIds();
    await choose("Quiet", baselineOrder, "known idle members are Quiet");
    await choose("Working", [], "idle members are not Working");
    await choose("Unknown", [], "known owner phases are not Unknown");
    await choose("All", baselineOrder);
    await page.getByTestId(`sidebar-agent:${router}`).click();
    await page.getByTestId(`chat-composer:${router}`).waitFor();
    barrierId = `sidebar-${randomUUID().slice(0, 8)}`;
    const instruction = `Inspect release readiness for ${barrierId} and report the result.`;
    const answer = `Release readiness checked for ${barrierId}. No blocked work remains.`;
    result.plan = { barrierId, instruction, answer };
    await fixture.control("model", { source: answer, delay_ms: 0, chunk_chars: 11 });
    result.armed = await fixture.control("model-barrier", { action: "arm", plan: { id: barrierId, match_text: instruction, source: answer } });
    await page.getByTestId(`chat-composer:${router}`).fill(instruction);
    await page.getByTestId(`chat-composer:${router}`).press("Enter");
    result.barrierEntered = await eventually(async () => {
      const status = await fixture.control("model-barrier", { action: "status", id: barrierId });
      return status.entered && !status.released && status;
    }, "real model request is held at its explicit barrier");
    result.workingOwner = await eventually(async () => {
      const agents = await ownerRoster(fixture);
      return activePhases.has(agents.find(agent => agent.identity === router)?.response_phase) && agents;
    }, "owner reports router Working");
    assert.equal(result.workingOwner.find(agent => agent.identity === delivery).response_phase, null, "other agent remains explicitly idle");
    await choose("Working", [router], "live Working filter follows owner phase");
    await capture("working-light-1600");
    await page.getByTestId("theme-toggle").click();
    await capture("working-dark-1600");
    await choose("Quiet", [delivery], "Quiet excludes the currently working router");
    await choose("Unknown", [], "active and explicit idle owners leave Unknown empty");
    await choose("All", baselineOrder, "activity changes preserve roster order");

    const section = sidebar().locator('[data-testid^="sidebar-section-toggle:"][aria-expanded="true"]').first();
    assert.equal(await section.count(), 1, "real roster exposes an expanded section");
    const sectionId = await section.getAttribute("data-testid");
    await section.click();
    await assertRows([], "explicit section collapse hides its members");
    await choose("Working", [router], "Working reveals matching member in a collapsed section");
    await search().fill("delivery");
    await assertRows([], "search intersects Working instead of widening it");
    await choose("Quiet", [delivery], "search and Quiet reveal the matching idle member");
    await capture("quiet-search-dark-1600");
    await search().fill("");
    await choose("All", [], "clearing filters restores saved collapsed preference");
    assert.equal(await page.getByTestId(sectionId).getAttribute("aria-expanded"), "false");
    await page.getByTestId(sectionId).click();
    await assertRows(baselineOrder, "expansion restores original order");
    await choose("Working", [router]);
    await page.setViewportSize({ width: 1280, height: 900 });
    await capture("working-dark-1280");
    await page.getByTestId("theme-toggle").click();
    await capture("working-light-1280");

    result.released = await fixture.control("model-barrier", { action: "release", id: barrierId });
    const toolId = `fixture-${barrierId}-peer-ready`;
    // A live terminal frame can arrive before timeline backfill retains the tool pair.
    result.finalFrames = await waitForSidebarCompletion({ toolId, answer,
      readFrames: () => timeline(fixture),
      onObservation: frames => result.completionSamples.push({ at: Date.now(), frameCount: frames.length,
        completed: frames.some(frame => frame.kind === "interaction_complete" && frame.payload?.result === answer),
        toolFrames: frames.filter(frame => frame.payload?.id === toolId) }),
    });
    result.finalOwner = await waitForExplicitIdleRoster({
      readRoster: () => ownerRoster(fixture),
      onObservation: agents => result.finalOwnerSamples.push({ at: Date.now(), agents }),
    });
    await assertRows([], "Working clears automatically after owner completion");
    await choose("Quiet", baselineOrder, "completed router returns to Quiet without reordering");
    await choose("Unknown", []);
    await choose("All", baselineOrder);
    await page.setViewportSize({ width: 1600, height: 1000 });
    await capture("completed-light-1600");
    assertSidebarCompletion(result.finalFrames, { toolId, answer });
    assert.deepEqual(result.errors, [], "no unexpected browser or network errors");
  } catch (error) {
    result.failure = error.stack || String(error);
    result.html = await page.content();
    await fs.mkdir(evidence, { recursive: true });
    await page.screenshot({ path: path.join(evidence, "stock-sidebar-activity-failure.png"), fullPage: true }).catch(() => {});
    throw error;
  } finally {
    if (barrierId) await fixture.control("model-barrier", { action: "release", id: barrierId }).catch(() => {});
    result.observations = fixture.observations;
    result.logs = fixture.logs();
    await fs.mkdir(evidence, { recursive: true });
    await fs.writeFile(path.join(evidence, "stock-sidebar-activity.json"), JSON.stringify(result, null, 2));
    await browser.close();
    await fixture.close();
  }
}

const scenarios = [{ id: "real-stock-sidebar-activity", family: "real-console", backend: "real", run: sidebarActivity }];
module.exports = { scenarios };
if (require.main === module) require("../scenario-registry.cjs").runScenarios(scenarios).catch(error => { process.stderr.write(`${error.stack || error}\n`); process.exitCode = 1; });
