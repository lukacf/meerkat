#!/usr/bin/env node

const fs = require("node:fs");
const http = require("node:http");
const os = require("node:os");
const path = require("node:path");
const { spawn } = require("node:child_process");
const { chromium } = require("playwright");

const { resolveFlowEditorBinary } = require("./flow-editor-binary.cjs");
const { LOOPBACK_ANY_PORT, awaitFixtureReady } = require("../../console/fixture-ready.cjs");

const repoRoot = path.join(__dirname, "..", "..");
const binary = resolveFlowEditorBinary();
// A spawned editor binds port 0 and announces the bound address; a fixed
// port collides with anything else on the host using it.
let baseUrl = process.env.META_FLOW_EDITOR_INTERACTIONS_URL;
const shouldSpawn = !baseUrl;

function waitForReady(url, child) {
  const deadline = Date.now() + 20_000;
  return new Promise((resolve, reject) => {
    const poll = () => {
      if (child && child.exitCode !== null) {
        reject(new Error(`meta_flow_editor exited before ready with status ${child.exitCode}`));
        return;
      }
      const req = http.request(url, { method: "GET" }, (res) => {
        res.resume();
        if (res.statusCode && res.statusCode >= 200 && res.statusCode < 500) {
          resolve();
          return;
        }
        retry();
      });
      req.on("error", retry);
      req.end();
    };
    const retry = () => {
      if (Date.now() > deadline) {
        reject(new Error(`timed out waiting for ${url}`));
        return;
      }
      setTimeout(poll, 200);
    };
    poll();
  });
}

async function chooseFirstRealAgentDefinition(page) {
  const addDefinition = page.locator(".agents-list__scroll select").first();
  const value = await addDefinition.locator("option").evaluateAll((options) => {
    const option = options.find((candidate) => candidate.value && !candidate.disabled);
    return option?.value || "";
  });
  if (!value) throw new Error("Agent Editor did not expose any real Meta agent definitions");
  await addDefinition.selectOption(value);
}

// The library is home: the app launches with no open mob, so every smoke
// creates one through + NEW MOB (name + Blank template) to enter the editor.
async function createMobThroughLibrary(page, name) {
  await page.locator(".flows-view").waitFor({ state: "visible", timeout: 10_000 });
  await page.waitForFunction(() => {
    const button = document.querySelector(".flows-view__head .btn--primary");
    return button && !button.disabled;
  }, null, { timeout: 10_000 });
  await page.locator(".flows-view__head .btn--primary").click();
  await page.locator(".modal--new").waitFor({ state: "visible", timeout: 10_000 });
  await page.locator(".modal--new .field__input").fill(name);
  await page.locator(".modal--new .template-card").first().click();
  await page.waitForFunction(() => {
    const button = document.querySelector(".modal--new .modal__foot .btn--primary");
    return button && !button.disabled;
  }, null, { timeout: 10_000 });
  await page.locator(".modal--new .modal__foot .btn--primary").click();
  await page.locator(".modetoggle").waitFor({ state: "visible", timeout: 10_000 });
}

async function chooseFirstGraphMember(page) {
  const implementer = page.locator(".add-menu__row", { hasText: "implementer" }).first();
  if (await implementer.count()) {
    await implementer.click();
    return;
  }
  const firstRow = page.locator(".add-menu__row").first();
  if (!(await firstRow.count())) throw new Error("Graph add menu did not expose any member rows");
  await firstRow.click();
}

async function main() {
  let server = null;
  let serverReady = null;
  let draftDir = null;
  if (shouldSpawn) {
    draftDir = fs.mkdtempSync(path.join(os.tmpdir(), "meta-flow-editor-interactions."));
    server = spawn(binary, ["--listen", LOOPBACK_ANY_PORT], {
      cwd: repoRoot,
      env: {
        ...process.env,
        META_FLOW_EDITOR_DRAFT_STORE: path.join(draftDir, "drafts.json"),
      },
      stdio: ["ignore", "pipe", "pipe"],
    });
    // Listen for the ready line from the spawn on: output is not replayed
    // for a listener attached after the browser launch below.
    serverReady = awaitFixtureReady(server, { label: "meta_flow_editor", timeoutMs: 60_000 });
    serverReady.catch(() => {}); // Awaited (and reported) inside the try below.
    server.stdout.on("data", (chunk) => process.stdout.write(chunk));
    server.stderr.on("data", (chunk) => process.stderr.write(chunk));
  }

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1280, height: 900 } });
  const consoleMessages = [];
  page.on("console", (message) => {
    if (message.type() === "error") consoleMessages.push(message.text());
  });
  page.on("pageerror", (error) => consoleMessages.push(error.message || String(error)));

  try {
    if (serverReady) ({ baseUrl } = await serverReady);
    await waitForReady(`${baseUrl}/flow-editor`, server);
    await page.goto(`${baseUrl}/flow-editor?cache_bust=browser-interactions-smoke`, {
      waitUntil: "domcontentloaded",
    });
    await page.getByText("api ready").waitFor({ timeout: 10_000 });
    await createMobThroughLibrary(page, "Browser Interaction Mob");

    // Member nodes only: structural edits reproject the canvas, which can
    // add or remove synthesized gate instances (fan-out/join) and their
    // edges, so counts of [data-inst-id]/.edge are not stable assertions.
    const memberNodes = "[data-inst-id]:not(.node--gate)";
    await page.locator("button.modetoggle__opt", { hasText: "Graph" }).click();
    await page.locator(".canvas-host").waitFor({ state: "visible", timeout: 10_000 });
    const graphNodeCountBefore = await page.locator(memberNodes).count();
    await page.locator(".cell:not(.is-occupied)").first().click();
    await page.locator(".add-menu").waitFor({ state: "visible", timeout: 10_000 });
    await chooseFirstGraphMember(page);
    await page.waitForFunction(
      ({ selector, count }) => document.querySelectorAll(selector).length > count,
      { selector: memberNodes, count: graphNodeCountBefore },
      { timeout: 10_000 },
    );
    const graphNodeCountAfter = await page.locator(memberNodes).count();
    if (graphNodeCountAfter <= graphNodeCountBefore) {
      throw new Error(`Graph add menu did not create a Meta graph node: before=${graphNodeCountBefore} after=${graphNodeCountAfter}`);
    }

    const movedNode = page.locator(memberNodes).last();
    const beforeMove = await movedNode.boundingBox();
    const targetCell = await page.locator(".cell:not(.is-occupied)").last().boundingBox();
    if (!beforeMove || !targetCell) throw new Error("Graph node drag test could not measure node/cell bounds");
    await page.mouse.move(beforeMove.x + beforeMove.width / 2, beforeMove.y + beforeMove.height / 2);
    await page.mouse.down();
    await page.mouse.move(targetCell.x + targetCell.width / 2, targetCell.y + targetCell.height / 2, { steps: 20 });
    await page.mouse.up();
    await page.waitForTimeout(1_000);
    const afterMove = await movedNode.boundingBox();
    if (!afterMove || Math.abs(afterMove.x - beforeMove.x) < 100 || Math.abs(afterMove.y - beforeMove.y) < 40) {
      throw new Error(`Graph drag did not move through Meta graph projection: before=${JSON.stringify(beforeMove)} after=${JSON.stringify(afterMove)}`);
    }

    const sourceNode = page.locator(memberNodes).first();
    const targetNode = page.locator(memberNodes).last();
    const sourceId = await sourceNode.getAttribute("data-inst-id");
    const targetId = await targetNode.getAttribute("data-inst-id");
    const sourcePort = sourceNode.locator(".port-out");
    const sourcePortBox = await sourcePort.boundingBox();
    const targetNodeBox = await targetNode.boundingBox();
    if (!sourcePortBox || !targetNodeBox) throw new Error("Graph connection test could not measure source port/target node bounds");
    await page.mouse.move(sourcePortBox.x + sourcePortBox.width / 2, sourcePortBox.y + sourcePortBox.height / 2);
    await page.mouse.down();
    await page.mouse.move(targetNodeBox.x + Math.min(12, targetNodeBox.width / 4), targetNodeBox.y + targetNodeBox.height / 2, { steps: 24 });
    await page.mouse.up();
    // Asserting the specific member-to-member connection survives canvas
    // reprojection (the sequence edge may run through synthesized gates).
    await page.waitForFunction(
      ({ from, to }) => {
        const edges = Array.from(document.querySelectorAll(".edge"));
        const reachable = new Set([from]);
        let grew = true;
        while (grew) {
          grew = false;
          for (const edge of edges) {
            const edgeFrom = edge.dataset.edgeFrom;
            const edgeTo = edge.dataset.edgeTo;
            if (reachable.has(edgeFrom) && !reachable.has(edgeTo)) {
              reachable.add(edgeTo);
              grew = true;
            }
          }
        }
        return reachable.has(to);
      },
      { from: sourceId, to: targetId },
      { timeout: 10_000 },
    );

    await page.locator("button.viewtab", { hasText: "AGENTS" }).click();
    await page.locator(".agents-view").waitFor({ state: "visible", timeout: 10_000 });
    const agentCountBefore = await page.locator(".agents-list__name").count();
    await chooseFirstRealAgentDefinition(page);
    await page.waitForFunction(
      (count) => document.querySelectorAll(".agents-list__name").length > count,
      agentCountBefore,
      { timeout: 10_000 },
    );
    await page.locator(".agents-list__scroll").first().locator(".agents-list__item").last().click();

    const titleInput = page.locator(".agent-editor__title-input");
    await titleInput.fill("Browser Interaction Agent");
    await page.waitForTimeout(1_000);
    const titleValue = await titleInput.inputValue();
    if (titleValue !== "Browser Interaction Agent") {
      throw new Error(`Agent Editor title did not reflect Meta operation result: ${titleValue}`);
    }

    const toolSelect = page.locator("select").filter({ has: page.locator('option[value="image_generation"]') }).first();
    await toolSelect.selectOption("image_generation");
    await page.waitForFunction(() => {
      return Array.from(document.querySelectorAll(".tool-row .name")).some((row) => row.textContent.trim() === "image_generation");
    }, null, { timeout: 10_000 });

    await page.getByRole("button", { name: /INLINE/i }).click();
    await page.locator(".inline-skill input").fill("browser interaction");
    await page.locator(".inline-skill textarea").fill("Use this skill for browser interaction smoke testing.");
    await page.locator(".inline-skill .btn", { hasText: "ADD" }).click();
    await page.waitForFunction(() => {
      return Array.from(document.querySelectorAll(".skill-row__name")).some((row) => row.textContent.trim() === "mob.browser.interaction");
    }, null, { timeout: 10_000 });

    // ── Adaptive layer authoring (Basic mode) ──
    // Insert from the step picker, then verify the expanded block, the
    // verbatim per-keystroke prompt echo (spaces + mid-string caret), and a
    // limits edit — every assertion reads server-projected state.
    await page.locator("button.viewtab", { hasText: "FLOW" }).click();
    await page.locator("button.modetoggle__opt", { hasText: "Basic" }).click();
    await page.locator(".bld-stage").waitFor({ state: "visible", timeout: 10_000 });
    await page.locator(".bld-insert__btn").last().click();
    const adaptivePickerRow = page.locator(".bld-panel .bld-opt", { hasText: "Adaptive layer" });
    await adaptivePickerRow.waitFor({ state: "visible", timeout: 10_000 });
    await adaptivePickerRow.click();

    const adaptiveBlock = page.locator(".bld-adaptive");
    await adaptiveBlock.waitFor({ state: "visible", timeout: 10_000 });
    // A freshly inserted adaptive layer is a draft (validation gates stage),
    // so the insert does not auto-select; clicking the block opens its panel.
    await adaptiveBlock.locator(".bld-aframe__head").click();
    await page.locator(".bld-panel__title", { hasText: "Adaptive layer" }).waitFor({ state: "visible", timeout: 10_000 });
    // textContent, not innerText: CSS text-transform must not affect the
    // server-contract text assertions.
    const adaptiveHeadText = (await adaptiveBlock.locator(".bld-aframe__head").textContent()).trim();
    if (!adaptiveHeadText.startsWith("ADAPTIVE LAYER · synthesized at runtime · max depth ")) {
      throw new Error(`Adaptive block head did not render server contract text: ${adaptiveHeadText}`);
    }
    const flowmasterTitleBefore = (await adaptiveBlock.locator(".bld-anode--fm .bld-anode__title").textContent()).trim();
    if (flowmasterTitleBefore !== "—") {
      throw new Error(`Adaptive block expected the flowmaster fallback title, got: ${flowmasterTitleBefore}`);
    }
    await adaptiveBlock.locator(".bld-afan__empty").waitFor({ state: "visible", timeout: 10_000 });

    // Selecting a FlowMaster round-trips Meta and retitles the block node.
    const adaptivePanel = page.locator(".bld-panel");
    const flowmasterSelect = adaptivePanel.locator("select.field__select").first();
    const flowmasterPick = await flowmasterSelect.locator("option").evaluateAll((options) => {
      const option = options.find((candidate) => candidate.value);
      return option ? { value: option.value, name: option.textContent.split(" · ")[0].trim() } : null;
    });
    if (!flowmasterPick) throw new Error("Adaptive panel did not expose any flowmaster member options");
    await flowmasterSelect.selectOption(flowmasterPick.value);
    await page.waitForFunction((name) => {
      const title = document.querySelector(".bld-adaptive .bld-anode--fm .bld-anode__title");
      return title && title.textContent.trim() === name;
    }, flowmasterPick.name, { timeout: 10_000 });

    // Toggling a profile template on grows a chip in the layer fan.
    const profileToggle = adaptivePanel.locator(".ap-profile").first();
    const profileName = (await profileToggle.locator(".ap-profile__name").textContent()).trim();
    await profileToggle.click();
    await page.waitForFunction((name) => {
      const chips = Array.from(document.querySelectorAll(".bld-adaptive .bld-afan__chip"));
      return chips.some((chip) => chip.textContent.includes(name));
    }, profileName, { timeout: 10_000 });

    // Prompt typing: every keystroke round-trips apply_operation; spaces are
    // preserved verbatim and a mid-string edit must land at the caret (the
    // EchoTextArea draft keeps the caret from snapping to the end).
    const promptArea = adaptivePanel.locator(".field__textarea").first();
    await promptArea.click();
    await page.keyboard.type("plan wide layers", { delay: 20 });
    for (let i = 0; i < " wide layers".length; i += 1) {
      await page.keyboard.press("ArrowLeft");
    }
    await page.keyboard.type(" and deep", { delay: 20 });
    await page.waitForTimeout(1_000); // let per-keystroke echoes settle
    const promptState = await promptArea.evaluate((el) => ({
      value: el.value,
      selectionStart: el.selectionStart,
      selectionEnd: el.selectionEnd,
    }));
    if (promptState.value !== "plan and deep wide layers") {
      throw new Error(`Adaptive prompt lost mid-string typing or spaces: ${JSON.stringify(promptState.value)}`);
    }
    if (promptState.selectionStart !== "plan and deep".length || promptState.selectionEnd !== "plan and deep".length) {
      throw new Error(`Adaptive prompt caret jumped during echo: ${JSON.stringify(promptState)}`);
    }
    // Trailing spaces survive too — the server never trims adaptive free text.
    // (Caret-to-end via the DOM: the End key does not move the caret on macOS.)
    await promptArea.evaluate((el) => {
      el.selectionStart = el.value.length;
      el.selectionEnd = el.value.length;
    });
    await page.keyboard.type("  ", { delay: 20 });
    await page.waitForTimeout(1_000);
    await promptArea.evaluate((el) => el.blur());
    // Unfocused, the field shows the server document, which trails the typing
    // by the serial per-keystroke apply_operation queue: wait (bounded) for it
    // to converge instead of sampling it after a fixed delay.
    await page.waitForFunction(
      (expected) => document.querySelector(".bld-panel .field__textarea")?.value === expected,
      "plan and deep wide layers  ",
      { timeout: 20_000 },
    ).catch(() => {});
    const promptAfterBlur = await promptArea.inputValue();
    if (promptAfterBlur !== "plan and deep wide layers  ") {
      throw new Error(`Adaptive prompt did not round-trip verbatim through Meta: ${JSON.stringify(promptAfterBlur)}`);
    }

    // A limits edit round-trips and re-renders the block head (max depth).
    const maxDepthInput = adaptivePanel.locator(".ap-grid .ap-num input").first();
    await maxDepthInput.fill("6");
    await page.waitForFunction(() => {
      const head = document.querySelector(".bld-adaptive .bld-aframe__head");
      return head && head.textContent.includes("max depth 6");
    }, null, { timeout: 10_000 });

    if (consoleMessages.length) {
      throw new Error(`browser console errors:\n${consoleMessages.join("\n")}`);
    }
  } finally {
    await browser.close();
    if (server) server.kill("SIGTERM");
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
