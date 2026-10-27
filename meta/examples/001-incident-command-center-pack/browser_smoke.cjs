#!/usr/bin/env node

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");
const YAML = require("yaml");

const LABEL_TO_ID = {
  "Incident Commander": "incident-commander",
  "Payments SRE": "payments-sre",
  "API Investigator": "api-investigator",
  "Approval Gate": "approval-gate",
  "Merchant Comms": "merchant-comms",
  "Merchant Success": "merchant-success",
  Scribe: "scribe",
  "Health Monitor": "health-monitor",
};

async function waitForText(scope, text, timeout = 60000) {
  await scope.getByText(text, { exact: false }).waitFor({ state: "visible", timeout });
}

async function dragHandle(page, locator, deltaX) {
  const box = await locator.boundingBox();
  assert.ok(box, "expected resize handle bounding box");
  await page.mouse.move(box.x + box.width / 2, box.y + box.height / 2);
  await page.mouse.down();
  await page.mouse.move(box.x + box.width / 2 + deltaX, box.y + box.height / 2, { steps: 12 });
  await page.mouse.up();
}

async function chatPane(page, identity) {
  const pane = page.getByTestId(`chat-pane:${identity}`).first();
  await pane.waitFor({ state: "visible", timeout: 10000 });
  return pane;
}

async function dockPanelForIdentity(page, identity, index = 0) {
  const currentPanel = page
    .locator('[data-testid^="pane:"]')
    .filter({ has: page.getByTestId(`chat-pane:${identity}`) })
    .nth(index);
  if (await currentPanel.count()) {
    await currentPanel.waitFor({ state: "visible", timeout: 10000 });
    return currentPanel;
  }
  const panel = page
    .locator('[data-testid^="dock-panel:"]')
    .filter({ has: page.getByTestId(`chat-pane:${identity}`) })
    .nth(index);
  await panel.waitFor({ state: "visible", timeout: 10000 });
  return panel;
}

async function sendPanelMessage(panel, identity, text) {
  const textarea = panel.getByTestId(`chat-composer:${identity}`);
  const submit = panel.getByTestId(`chat-send:${identity}`);
  await textarea.waitFor({ state: "visible", timeout: 10000 });
  await textarea.fill(text);
  await assertEventually(async () => !(await submit.isDisabled()), "expected composer submit button to enable");
  await submit.click({ force: true });
}

async function panelId(panel) {
  const id = (await panel.getAttribute("data-panel-id"))
    || (await panel.getAttribute("data-testid"))?.replace(/^pane:/, "");
  assert.ok(id, "expected panel id");
  return id;
}

async function currentPendingId(page) {
  const locator = page.locator('[data-testid^="gating-pending:"]').first();
  await locator.waitFor({ state: "visible", timeout: 10000 });
  const value = await locator.getAttribute("data-testid");
  assert.ok(value, "expected gating pending id");
  return value.slice("gating-pending:".length);
}

async function selectSidebarItem(page, label) {
  const identity = LABEL_TO_ID[label];
  if (identity) {
    const current = page.getByTestId(`sidebar-agent:${identity}`);
    if (await current.count()) {
      await current.waitFor({ state: "visible", timeout: 10000 });
      await current.click();
      return;
    }
  }
  const row = page
    .locator('[data-console-workbench-part="launcher"] [data-console-sidebar-part="row"]')
    .filter({ hasText: label })
    .first();
  await row.waitFor({ state: "visible", timeout: 10000 });
  await row.click();
}

async function waitForSidebarItem(page, label) {
  const identity = LABEL_TO_ID[label];
  if (identity) {
    const current = page.getByTestId(`sidebar-agent:${identity}`);
    if (await current.count()) {
      await current.waitFor({ state: "visible", timeout: 60000 });
      return;
    }
  }
  await page
    .locator('[data-console-workbench-part="launcher"] [data-console-sidebar-part="row"]')
    .filter({ hasText: label })
    .first()
    .waitFor({ state: "visible", timeout: 60000 });
}

async function assertEventually(check, message, timeout = 10000, interval = 100) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    if (await check()) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, interval));
  }
  assert.fail(message);
}

async function boundingBoxForAny(page, testIds, legacySelector) {
  for (const testId of testIds) {
    const locator = page.getByTestId(testId);
    if (await locator.count()) {
      return locator.first().boundingBox();
    }
  }
  return page.locator(legacySelector).boundingBox();
}

async function clickTestIdIfPresent(page, testId) {
  const locator = page.getByTestId(testId);
  if (await locator.count()) {
    await locator.first().click();
    return true;
  }
  return false;
}

async function clickNav(page, id) {
  if (await clickTestIdIfPresent(page, `nav:${id}`)) return;
  await page.getByTestId(`sidebar-action:open_${id}`).click();
}

async function splitPanelRight(page, id) {
  if (await clickTestIdIfPresent(page, `pane-split-right:${id}`)) return;
  await page.getByTestId(`dock-split:${id}:right`).click();
}

async function waitForPanelChange(panel, previousText, timeout = 90000) {
  await assertEventually(async () => (await panel.innerText()) !== previousText, "expected panel text to change", timeout);
}

async function waitForComposerIdle(panel, identity, timeout = 180000) {
  await panel
    .getByTestId(`chat-typing:${identity}`)
    .waitFor({ state: "detached", timeout })
    .catch(() => {});
  await assertEventually(
    async () => !(await panel.getByTestId(`chat-composer:${identity}`).isDisabled()),
    `expected ${identity} composer to be ready`,
    10000,
    250,
  );
}

async function transcriptMessages(panel) {
  return panel.locator(".msg:not(.msg--typing):not(.msg--origin)").evaluateAll((nodes) =>
    nodes.map((node) => ({
      className: node.className,
      text: (node.textContent || "").trim().replace(/\s+/g, " "),
      imageAlts: Array.from(node.querySelectorAll("img")).map((img) => img.getAttribute("alt") || ""),
      imageSrcs: Array.from(node.querySelectorAll("img")).map((img) => img.getAttribute("src") || ""),
    })),
  );
}

function normalizePromptText(text) {
  return String(text || "").trim().replace(/\s+/g, " ");
}

async function queryTimelinePage(baseUrl, identity, after = null, limit = 1000) {
  const url = new URL("/console/timeline", baseUrl);
  url.searchParams.set("identity", identity);
  url.searchParams.set("limit", String(limit));
  if (after) url.searchParams.set("after", after);
  const response = await fetch(url);
  assert.ok(response.ok, `timeline query failed: ${response.status}`);
  return response.json();
}

async function latestTimelineCursor(baseUrl, identity) {
  let after = null;
  let latest = null;
  for (let i = 0; i < 100; i += 1) {
    const page = await queryTimelinePage(baseUrl, identity, after);
    const frames = Array.isArray(page.frames) ? page.frames : [];
    if (frames.length === 0) return latest;
    latest = frames[frames.length - 1].cursor || latest;
    if (!page.next_cursor || page.next_cursor === after) return latest;
    after = page.next_cursor;
  }
  return latest;
}

async function waitForTimelineQuiet(baseUrl, identity, quietMs = 12000, timeout = 180000) {
  let cursor = await latestTimelineCursor(baseUrl, identity);
  let quietSince = Date.now();
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const page = await queryTimelinePage(baseUrl, identity, cursor, 1000);
    const frames = Array.isArray(page.frames) ? page.frames : [];
    if (frames.length > 0) {
      cursor = frames[frames.length - 1].cursor || cursor;
      quietSince = Date.now();
    } else if (Date.now() - quietSince >= quietMs) {
      return;
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  assert.fail(`expected ${identity} timeline to be quiet for ${quietMs}ms`);
}

async function main() {
  const baseUrl = process.argv[2];
  assert.ok(baseUrl, "baseUrl is required");
  const artifactDir = process.env.META_BROWSER_SMOKE_ARTIFACT_DIR || "";
  if (artifactDir) fs.mkdirSync(artifactDir, { recursive: true });
  let screenshotIndex = 0;
  async function screenshot(page, label) {
    if (!artifactDir) return;
    screenshotIndex += 1;
    const safeLabel = label.replace(/[^A-Za-z0-9._-]+/g, "-");
    await page.screenshot({
      path: path.join(artifactDir, `${String(screenshotIndex).padStart(2, "0")}-${safeLabel}.png`),
      fullPage: true,
    });
  }
  const runTag = `smoke-${Date.now().toString(36)}`;
  const scenario = YAML.parse(
    fs.readFileSync(path.join(__dirname, "scenario.yaml"), "utf8"),
  );
  const prompts = scenario.smoke?.prompts || {};
  const toolSweepPrompt = normalizePromptText(`${prompts.tool_sweep || "Run a status sweep and use both tools before answering."} [${runTag}:tool]`);
  const merchantStatusPrompt = normalizePromptText(`${prompts.merchant_status || "Give a one-sentence merchant status update for the fictional incident."} [${runTag}:merchant]`);
  const alphaFollowUpPrompt = normalizePromptText(`${prompts.alpha_follow_up || "Panel alpha follow-up. Give one short sentence about rollback guardrails."} [${runTag}:alpha]`);
  const bravoFollowUpPrompt = normalizePromptText(`${prompts.bravo_follow_up || "Panel bravo follow-up. Give one short sentence about customer impact."} [${runTag}:bravo]`);
  const imageGenerationPrompt = normalizePromptText(`Use generate_image to create one square incident command dashboard image for the fictional CardinalPay payments-api outage. Include visible labels for payments-api, OUTAGE, CRITICAL, US-East, US-Central, EU-West, and rollback 64%. After the image is generated, reply with one short sentence. [${runTag}:image]`);
  const apiInvestigatorPrompt = normalizePromptText(`Describe the attached generated incident dashboard image. Mention the visible service name, severity, regions, and rollback percentage. [${runTag}:api-image]`);

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });

  try {
    console.log("smoke:load");
    await page.goto(`${baseUrl}/console`, { waitUntil: "domcontentloaded" });
    await page.getByTestId("meerkat-console").waitFor({ state: "visible", timeout: 30000 });

    console.log("smoke:commander");
    await waitForSidebarItem(page, "Incident Commander");
    await waitForSidebarItem(page, "Payments SRE");
    await waitForSidebarItem(page, "Approval Gate");
    await selectSidebarItem(page, "Incident Commander");

    const sidebarHandle = page.getByTestId("resize:sidebar");
    const activityHandle = page.getByTestId("resize:activity");
    const sidebarBefore = await boundingBoxForAny(
      page,
      ["sidebar-root"],
      '[data-console-workbench-part="launcher"]',
    );
    const activityBefore = await boundingBoxForAny(
      page,
      ["signals-rail"],
      '[data-console-workbench-part="activity"]',
    );
    await dragHandle(page, sidebarHandle, 80);
    await dragHandle(page, activityHandle, -60);
    const sidebarAfter = await boundingBoxForAny(
      page,
      ["sidebar-root"],
      '[data-console-workbench-part="launcher"]',
    );
    const activityAfter = await boundingBoxForAny(
      page,
      ["signals-rail"],
      '[data-console-workbench-part="activity"]',
    );
    assert.ok(sidebarBefore && sidebarAfter, "sidebar should be measurable");
    assert.ok(activityBefore && activityAfter, "activity rail should be measurable");

    let commanderPanel = await dockPanelForIdentity(page, "incident-commander");
    const commanderPanelId = await panelId(commanderPanel);

    const commanderBefore = await commanderPanel.innerText();
    await sendPanelMessage(
      commanderPanel,
      "incident-commander",
      toolSweepPrompt,
    );
    await commanderPanel
      .getByTestId("chat-typing:incident-commander")
      .waitFor({ state: "visible", timeout: 10000 })
      .catch(() => {});
    await assertEventually(async () => {
      const transcript = await transcriptMessages(commanderPanel);
      const promptIndex = transcript.findIndex((entry) => entry.text.includes(toolSweepPrompt));
      return promptIndex >= 0
        && transcript.slice(promptIndex + 1).some((entry) => entry.className.includes("msg--agent"));
    }, "commander transcript should have at least user + assistant", 90000, 250);
    const commanderTranscript = await transcriptMessages(commanderPanel);
    await assertEventually(async () => {
      const currentTranscript = await transcriptMessages(commanderPanel);
      return currentTranscript.filter((entry) => entry.text.includes(toolSweepPrompt)).length === 1;
    }, "commander transcript should not duplicate the active user prompt", 60000, 250);
    assert.ok(
      commanderTranscript.every((entry) => entry.text.length > 0),
      "commander transcript must not contain blank message bubbles",
    );
    await waitForComposerIdle(commanderPanel, "incident-commander");
    await screenshot(page, "commander-status-sweep");
    await assertEventually(
      async () => {
        const sreCount = await page
          .locator('[data-testid^="activity-item:pulse:"], [data-testid^="signal:"]')
          .filter({ hasText: "Payments SRE" })
          .count();
        const commsCount = await page
          .locator('[data-testid^="activity-item:pulse:"], [data-testid^="signal:"]')
          .filter({ hasText: "Merchant Comms" })
          .count();
        const scribeCount = await page
          .locator('[data-testid^="activity-item:pulse:"], [data-testid^="signal:"]')
          .filter({ hasText: "Scribe" })
          .count();
        return sreCount > 0 || commsCount > 0 || scribeCount > 0;
      },
      "expected cross-agent activity after commander coordination",
      90000,
      250,
    );
    await waitForTimelineQuiet(baseUrl, "incident-commander");

    console.log("smoke:image-generation");
    await sendPanelMessage(
      commanderPanel,
      "incident-commander",
      imageGenerationPrompt,
    );
    await assertEventually(async () => {
      const transcript = await transcriptMessages(commanderPanel);
      return transcript.some((entry) => entry.text.includes(imageGenerationPrompt));
    }, "commander user image-generation prompt should render", 180000, 250);
    const generatedImageButton = commanderPanel.getByRole("button", { name: /generated image/i }).last();
    await generatedImageButton.waitFor({ state: "visible", timeout: 360000 });
    const generatedImage = generatedImageButton.locator("img").first();
    await assertEventually(async () => {
      const src = await generatedImage.getAttribute("src");
      return !!src && src.includes("/blobs/");
    }, "generated image should be rendered from the blob route", 30000, 250);
    const generatedImageSrc = await generatedImage.getAttribute("src");
    assert.ok(generatedImageSrc, "expected generated image src");
    const generatedImageUrl = new URL(generatedImageSrc, baseUrl).href;
    await screenshot(page, "commander-generated-image");

    console.log("smoke:image-upload");
    await selectSidebarItem(page, "API Investigator");
    const apiPanel = await dockPanelForIdentity(page, "api-investigator");
    const apiComposer = apiPanel.getByTestId("chat-composer:api-investigator");
    await apiComposer.fill(`${apiInvestigatorPrompt}\n${generatedImageUrl}`);
    await assertEventually(
      async () => (await apiPanel.getByRole("button", { name: "Remove attachment" }).count()) > 0,
      "typing a console blob URL should stage an image attachment",
      30000,
      250,
    );
    await assertEventually(async () => {
      const value = await apiComposer.inputValue();
      return value.includes(apiInvestigatorPrompt) && !value.includes("/blobs/");
    }, "composer should keep text while stripping staged blob URL", 30000, 250);
    await apiPanel.getByTestId("chat-send:api-investigator").click({ force: true });
    await assertEventually(async () => {
      const messages = await transcriptMessages(apiPanel);
      return messages.some((entry) =>
        entry.className.includes("msg--user")
        && entry.text.includes(apiInvestigatorPrompt)
        && entry.imageAlts.includes("attached image")
      );
    }, "API Investigator user message should show text and inline attached image", 60000, 250);
    await assertEventually(async () => {
      const messages = await transcriptMessages(apiPanel);
      return messages.some((entry) =>
        entry.className.includes("msg--agent")
        && /payments-api|outage|critical|rollback|region/i.test(entry.text)
      );
    }, "API Investigator should describe the uploaded image", 180000, 500);
    await screenshot(page, "api-investigator-image-description");

    console.log("smoke:merchant");
    await selectSidebarItem(page, "Merchant Success");
    const merchantPanel = await dockPanelForIdentity(page, "merchant-success");
    const merchantBefore = await merchantPanel.innerText();
    await sendPanelMessage(
      merchantPanel,
      "merchant-success",
      merchantStatusPrompt,
    );
    await waitForPanelChange(merchantPanel, merchantBefore);

    if (!(await clickTestIdIfPresent(page, "activity-action:pulse:watched-only"))) {
      await clickTestIdIfPresent(page, "signals-filter:warning");
    }
    await page.waitForTimeout(300);
    if (!(await clickTestIdIfPresent(page, "activity-action:pulse:all"))) {
      await clickTestIdIfPresent(page, "signals-filter:all");
    }

    console.log("smoke:routing");
    await clickNav(page, "routing");
    await page.getByTestId("routing-panel").waitFor();
    await page.getByTestId("routing-route:incident-statuspage").waitFor();

    console.log("smoke:gating");
    await clickNav(page, "gating");
    await page.getByTestId("gating-panel").waitFor();
    if (await page.locator('[data-testid^="gating-pending:"]').count()) {
      const originalPendingId = await currentPendingId(page);
      await page.getByTestId(`gating-action:${originalPendingId}:escalate`).click();
      await page.waitForFunction(
        (previousId) => {
          const next = document.querySelector('[data-testid^="gating-pending:"]');
          return next && next.getAttribute("data-testid") !== `gating-pending:${previousId}`;
        },
        originalPendingId,
      );
      const successorPendingId = await currentPendingId(page);
      assert.notEqual(successorPendingId, originalPendingId, "escalation should create successor pending entry");
      await page.getByTestId(`gating-action:${successorPendingId}:approve`).click();
      await page.waitForFunction(() => document.querySelectorAll('[data-testid^="gating-pending:"]').length === 0);
    } else {
      await waitForText(page.getByTestId("gating-panel"), "No pending items.");
    }

    console.log("smoke:topology");
    await clickNav(page, "topology");
    await page.getByTestId("topology-panel").waitFor();
    // TopologyPanel defaults to the dense graph view, which only exposes
    // topology-dense-map; per-node topology-node:* testids exist only in the
    // Roles view, so switch views before waiting on them.
    await page.getByTestId("topology-view:roles").click();
    const topologyNode = page.getByTestId("topology-node:incident-commander");
    await topologyNode.waitFor();
    await page.getByTestId("topology-node:payments-sre").waitFor({ state: "visible", timeout: 60000 });
    await screenshot(page, "topology");

    console.log("smoke:inspect");
    await selectSidebarItem(page, "Incident Commander");
    commanderPanel = await dockPanelForIdentity(page, "incident-commander");
    // The console renamed the inspect affordance testid to conv-action:details;
    // keep the old id as a fallback for older console builds.
    const inspectAction = (await commanderPanel.getByTestId("conv-action:details").count())
      ? commanderPanel.getByTestId("conv-action:details")
      : commanderPanel.getByTestId("conv-action:inspect");
    await inspectAction.evaluate((button) => button.click());
    await page.getByTestId("inspect-panel:incident-commander").waitFor();
    await waitForText(page.getByTestId("inspect-panel:incident-commander"), "addressable");

    console.log("smoke:split");
    await selectSidebarItem(page, "Incident Commander");
    commanderPanel = await dockPanelForIdentity(page, "incident-commander");
    const activeCommanderPanelId = await panelId(commanderPanel);
    await splitPanelRight(page, activeCommanderPanelId);
    const commanderPanels = page
      .locator('[data-testid^="pane:"], [data-testid^="dock-panel:"]')
      .filter({ has: page.getByTestId("chat-pane:incident-commander") });
    await page.waitForFunction(
      () => document.querySelectorAll('[data-testid="chat-pane:incident-commander"]').length >= 2,
    );

    const [alphaId, bravoId] = await Promise.all([
      panelId(commanderPanels.nth(0)),
      panelId(commanderPanels.nth(1)),
    ]);
    const panelAlpha = page.locator(`[data-panel-id="${alphaId}"][data-testid^="dock-panel:"], [data-testid="pane:${alphaId}"]`);
    const panelBravo = page.locator(`[data-panel-id="${bravoId}"][data-testid^="dock-panel:"], [data-testid="pane:${bravoId}"]`);
    assert.notEqual(alphaId, bravoId, "split should create a second panel");

    await panelAlpha.getByTestId("chat-pane:incident-commander").waitFor({ state: "visible" });
    await panelBravo.getByTestId("chat-pane:incident-commander").waitFor({ state: "visible" });
    await screenshot(page, "split-commander-panes");
    await waitForTimelineQuiet(baseUrl, "incident-commander", 5000, 120000);
    await waitForComposerIdle(panelAlpha, "incident-commander");
    await waitForComposerIdle(panelBravo, "incident-commander");

    const alphaBefore = await panelAlpha.innerText();
    const bravoBefore = await panelBravo.innerText();

    // Split panes share the identity-scoped conversation log: the console
    // deliberately removed per-panel frame filtering (panel ids are ephemeral
    // UI instance ids; filtering by them hid history after refresh — see
    // framesVisibleInPanel in console/src/ConsoleApp.tsx). A prompt sent from
    // either pane must therefore mirror into both panes exactly once.
    await sendPanelMessage(panelAlpha, "incident-commander", alphaFollowUpPrompt);
    await waitForPanelChange(panelAlpha, alphaBefore);
    for (const [panel, label] of [[panelAlpha, "alpha"], [panelBravo, "bravo"]]) {
      await assertEventually(async () => {
        const messages = await transcriptMessages(panel);
        return messages.filter((entry) =>
          entry.className.includes("msg--user")
          && entry.text.includes(alphaFollowUpPrompt)
        ).length === 1;
      }, `${label} panel must render the shared alpha prompt exactly once`, 60000, 250);
    }
    await waitForTimelineQuiet(baseUrl, "incident-commander", 5000, 120000);
    await waitForComposerIdle(panelBravo, "incident-commander");

    await sendPanelMessage(panelBravo, "incident-commander", bravoFollowUpPrompt);
    await waitForPanelChange(panelBravo, bravoBefore);
    for (const [panel, label] of [[panelAlpha, "alpha"], [panelBravo, "bravo"]]) {
      await assertEventually(async () => {
        const messages = await transcriptMessages(panel);
        return messages.filter((entry) =>
          entry.className.includes("msg--user")
          && entry.text.includes(bravoFollowUpPrompt)
        ).length === 1;
      }, `${label} panel must render the shared bravo prompt exactly once`, 60000, 250);
    }
    await waitForComposerIdle(panelAlpha, "incident-commander");
    await waitForComposerIdle(panelBravo, "incident-commander");

    console.log("smoke:scribe");
    await selectSidebarItem(page, "Scribe");
    const scribePanel = await dockPanelForIdentity(page, "scribe");
    await assertEventually(async () => {
      const messages = await transcriptMessages(scribePanel);
      return messages.some((entry) =>
        /payments-api|enterprise merchants|payment failures|status-page|timeline/i.test(entry.text)
      );
    }, "scribe panel should show recent peer activity", 60000, 250);
    await screenshot(page, "scribe-peer-activity");
  } finally {
    await browser.close();
  }

  console.log("incident browser smoke passed");
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
