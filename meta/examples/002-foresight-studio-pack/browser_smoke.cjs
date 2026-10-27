#!/usr/bin/env node

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");

async function fetchJson(baseUrl, route) {
  const response = await fetch(new URL(route, baseUrl));
  assert.equal(response.status, 200, `${route} should return HTTP 200`);
  return response.json();
}

async function expectVisible(locator, label, timeout = 20000) {
  await locator.first().waitFor({ state: "visible", timeout });
  assert.ok(await locator.first().isVisible(), `${label} should be visible`);
}

async function expectHidden(locator, label, timeout = 5000) {
  await locator.first().waitFor({ state: "hidden", timeout }).catch(() => {});
  assert.equal(await locator.first().isVisible().catch(() => false), false, `${label} should be hidden`);
}

async function screenshot(page, artifactDir, index, label) {
  if (!artifactDir) return index;
  fs.mkdirSync(artifactDir, { recursive: true });
  const safe = label.replace(/[^A-Za-z0-9._-]+/g, "-");
  await page.screenshot({
    path: path.join(artifactDir, `${String(index + 1).padStart(2, "0")}-${safe}.png`),
    fullPage: true,
  });
  return index + 1;
}

async function clickNav(page, testId, expectedPanelTestId) {
  await page.getByTestId(testId).first().click();
  await expectVisible(page.getByTestId(expectedPanelTestId), expectedPanelTestId);
}

async function main() {
  const baseUrl = process.argv[2];
  assert.ok(baseUrl, "Usage: browser_smoke.cjs <base-url>");
  const artifactDir = process.env.META_BROWSER_SMOKE_ARTIFACT_DIR || "";
  let shot = 0;

  const experience = await fetchJson(baseUrl, "/console/experience");
  assert.equal(experience.console_config.title, "Foresight Studio");
  assert.equal(experience.console_config.brand.label, "Borealis Foresight");
  assert.equal(experience.console_config.environment.label, "demo / board-readiness");
  assert.deepEqual(experience.console_config.sidebar.visible_controls, [
    "roster",
    "topology",
    "timeline",
    "routing",
    "logs",
    "health",
  ]);
  assert.deepEqual(
    experience.console_config.agent_list.group_by.slice(0, 2),
    ["labels.console_group", "labels.group"],
  );
  assert.deepEqual(
    experience.console_config.agent_list.subgroup_by.slice(0, 2),
    ["labels.org", "labels.lane"],
  );
  assert.deepEqual(
    experience.console_config.agent_list.default_pinned_agent_ids,
    ["risk-red-team"],
  );

  const experienceText = JSON.stringify(experience);
  for (const expected of [
    "Studio Director",
    "Signal Cartographer",
    "Financial Modeler",
    "Risk Red Team",
    "Launch Narrator",
    "Board Scribe",
  ]) {
    assert.match(experienceText, new RegExp(expected), `experience should include ${expected}`);
  }

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1500, height: 950 } });

  try {
    await page.goto(new URL("/console", baseUrl).toString(), { waitUntil: "domcontentloaded" });
    await expectVisible(page.getByTestId("meerkat-console"), "console root");
    await expectHidden(page.getByTestId("console-loading"), "loading indicator");

    await expectVisible(page.getByTestId("meta-topbar").getByText("Borealis Foresight"), "brand label");
    await expectVisible(page.getByTestId("meta-topbar").getByText("Foresight Studio"), "console title");
    await expectVisible(page.getByTestId("meta-topbar").getByText("demo / board-readiness"), "environment label");
    shot = await screenshot(page, artifactDir, shot, "initial-console");

    for (const nav of ["roster", "topology", "timeline", "routing", "logs", "health"]) {
      await expectVisible(page.getByTestId(`nav:${nav}`), `nav:${nav}`);
    }
    await expectHidden(page.getByTestId("nav:gating"), "stock gating nav");
    for (const custom of ["risk-review", "board-brief", "signal-lake"]) {
      await expectVisible(page.getByTestId(`nav-custom:${custom}`), `custom nav:${custom}`);
    }

    for (const filter of ["board-risk", "watched", "review"]) {
      await expectVisible(page.getByTestId(`signals-filter:${filter}`), `signals filter:${filter}`);
    }

    for (const section of ["Studio Leads", "Signal Pods", "Analysis Pods", "Review Board", "Synthesis"]) {
      await expectVisible(page.getByTestId(`sidebar-section-toggle:${section}`), `section:${section}`);
    }
    for (const agent of [
      "Studio Director",
      "Signal Cartographer",
      "Customer Ethnographer",
      "Financial Modeler",
      "Experiment Planner",
      "Risk Red Team",
      "Launch Narrator",
      "Board Scribe",
    ]) {
      await expectVisible(page.locator('[data-testid^="sidebar-agent:"]').filter({ hasText: agent }), `agent:${agent}`);
    }

    await expectVisible(page.locator(".agent__badge").filter({ hasText: "Lane" }), "lane badge");
    await expectVisible(page.locator(".agent__badge").filter({ hasText: "Confidence" }), "confidence badge");
    await expectVisible(page.locator(".sidebar__subgroup").filter({ hasText: "Strategy Office" }), "org subgroup");
    const riskRow = page.locator('[data-testid^="sidebar-agent:"]').filter({ hasText: "Risk Red Team" }).first();
    await expectVisible(riskRow, "default pinned risk row");
    await expectVisible(riskRow.locator(".agent__pin[aria-pressed=\"true\"]"), "default pinned risk button");
    shot = await screenshot(page, artifactDir, shot, "grouped-roster");

    await page.getByTestId("sidebar-search").fill("risk");
    await expectVisible(page.locator('[data-testid^="sidebar-agent:"]').filter({ hasText: "Risk Red Team" }), "filtered risk agent");
    await expectHidden(page.locator('[data-testid^="sidebar-agent:"]').filter({ hasText: "Studio Director" }), "filtered-out director");
    await page.getByTestId("sidebar-search").fill("");

    await page.getByTestId("sidebar-section-toggle:Analysis Pods").click();
    await expectHidden(page.locator('[data-testid^="sidebar-agent:"]').filter({ hasText: "Financial Modeler" }), "collapsed financial modeler");
    await page.reload({ waitUntil: "domcontentloaded" });
    await expectVisible(page.getByTestId("meerkat-console"), "console root after collapse reload");
    await expectHidden(page.locator('[data-testid^="sidebar-agent:"]').filter({ hasText: "Financial Modeler" }), "persisted collapsed financial modeler");
    await page.getByTestId("sidebar-search").fill("financial");
    await expectVisible(page.locator('[data-testid^="sidebar-agent:"]').filter({ hasText: "Financial Modeler" }), "search expands collapsed financial modeler");
    await page.getByTestId("sidebar-search").fill("");
    await expectHidden(page.locator('[data-testid^="sidebar-agent:"]').filter({ hasText: "Financial Modeler" }), "clearing search restores persisted collapse");
    await page.getByTestId("sidebar-section-toggle:Analysis Pods").click();
    await expectVisible(page.locator('[data-testid^="sidebar-agent:"]').filter({ hasText: "Financial Modeler" }), "expanded financial modeler");

    await riskRow.locator(".agent__pin").click();
    await expectVisible(riskRow.locator(".agent__pin[aria-pressed=\"false\"]"), "risk unpinned button");
    await page.reload({ waitUntil: "domcontentloaded" });
    await expectVisible(page.getByTestId("meerkat-console"), "console root after pin reload");
    const reloadedRiskRow = page.locator('[data-testid^="sidebar-agent:"]').filter({ hasText: "Risk Red Team" }).first();
    await expectVisible(reloadedRiskRow.locator(".agent__pin[aria-pressed=\"false\"]"), "risk unpin persisted");
    await reloadedRiskRow.locator(".agent__pin").click();
    await expectVisible(reloadedRiskRow.locator(".agent__pin[aria-pressed=\"true\"]"), "risk repinned button");
    await page.reload({ waitUntil: "domcontentloaded" });
    const repinnedRiskButton = page
      .locator('[data-testid^="sidebar-agent:"]')
      .filter({ hasText: "Risk Red Team" })
      .first()
      .locator(".agent__pin[aria-pressed=\"true\"]");
    await expectVisible(repinnedRiskButton, "risk repin persisted");

    await page.locator('[data-testid^="sidebar-agent:"]').filter({ hasText: "Risk Red Team" }).first().click();
    await expectVisible(page.locator('[data-testid^="chat-pane:"]').filter({ hasText: "Risk Red Team" }), "risk chat pane");
    await expectVisible(page.getByText("Open dossier"), "custom inspect action label");
    await expectVisible(page.getByText("Send brief"), "custom send label");
    shot = await screenshot(page, artifactDir, shot, "risk-agent-chat");

    await clickNav(page, "nav-custom:risk-review", "gating-panel");
    await clickNav(page, "nav:topology", "topology-panel");
    await expectVisible(page.getByTestId("topology-panel").getByText("Studio Director"), "topology director");
    shot = await screenshot(page, artifactDir, shot, "topology-panel");

    await clickNav(page, "nav:routing", "routing-panel");
    await clickNav(page, "nav:logs", "logs-panel");
    await clickNav(page, "nav:health", "health-panel");
    await expectVisible(page.getByTestId("health-panel"), "health panel");
    await clickNav(page, "nav:roster", "roster-panel");
    await expectVisible(page.getByTestId("roster-panel"), "roster panel");

    await page.getByTestId("theme-toggle").click();
    await expectVisible(page.getByTestId("theme-toggle"), "theme toggle after click");

    console.log("browser-smoke:ok");
    if (artifactDir) {
      console.log(`browser-smoke:artifacts:${artifactDir}`);
    }
  } finally {
    await browser.close();
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
