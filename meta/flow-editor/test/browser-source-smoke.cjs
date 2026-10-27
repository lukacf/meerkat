#!/usr/bin/env node

const http = require("node:http");
const fs = require("node:fs");
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
let baseUrl = process.env.META_FLOW_EDITOR_BROWSER_URL;
const shouldSpawn = !baseUrl;

// Liveness backstop for the structural waits below, NOT a performance budget.
// Every `timeout:` in this file guards a question of the form "does this
// element/section ever appear", so the only thing the bound decides is how long
// a genuinely hung run takes to fail. Under a loaded hosted runner 30s was
// close enough to the real settling time that the graph-panel source wait
// timed out on a docs-only change; the assertions themselves were never
// measuring duration, so the bound was reporting load, not defects.
const STRUCTURAL_TIMEOUT_MS = 120_000;

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
  await page.locator(".flows-view").waitFor({ state: "visible", timeout: STRUCTURAL_TIMEOUT_MS });
  await page.waitForFunction(() => {
    const button = document.querySelector(".flows-view__head .btn--primary");
    return button && !button.disabled;
  }, null, { timeout: STRUCTURAL_TIMEOUT_MS });
  await page.locator(".flows-view__head .btn--primary").click();
  await page.locator(".modal--new").waitFor({ state: "visible", timeout: STRUCTURAL_TIMEOUT_MS });
  await page.locator(".modal--new .field__input").fill(name);
  await page.locator(".modal--new .template-card").first().click();
  await page.waitForFunction(() => {
    const button = document.querySelector(".modal--new .modal__foot .btn--primary");
    return button && !button.disabled;
  }, null, { timeout: STRUCTURAL_TIMEOUT_MS });
  await page.locator(".modal--new .modal__foot .btn--primary").click();
  await page.locator(".modetoggle").waitFor({ state: "visible", timeout: STRUCTURAL_TIMEOUT_MS });
}

// The inline source panel is a plain <pre>: innerText carries the whole
// rendered file, so the only way this wait fails is the panel never receiving
// a document. When that happens, say what the page shows instead of a bare
// TimeoutError: #398 was the panel sitting open and EMPTY behind a "Source
// render failed" sheet (the read-only source render had raced our own autosave
// and been refused as a draft revision conflict), and the 120s silence hid it.
async function waitForSourceSections(page, panelSelector, sections) {
  try {
    await page.waitForFunction(({ panelSelector, sections }) => {
      const source = document.querySelector(`${panelSelector} [role="textbox"][aria-readonly="true"]`);
      const text = source?.innerText || "";
      return sections.every((section) => text.includes(section));
    }, { panelSelector, sections }, { timeout: STRUCTURAL_TIMEOUT_MS });
  } catch (error) {
    const state = await page.evaluate((panelSelector) => ({
      panelPresent: !!document.querySelector(panelSelector),
      sourceText: (document.querySelector(`${panelSelector} [role="textbox"][aria-readonly="true"]`)?.innerText || "").slice(0, 200),
      validationSheet: (document.querySelector(".validate")?.innerText || "").replace(/\s+/g, " ").slice(0, 400),
    }), panelSelector);
    throw new Error(
      `${panelSelector} never rendered ${sections.map((section) => JSON.stringify(section)).join(" and ")}: ${JSON.stringify(state)}`,
      { cause: error },
    );
  }
}

async function main() {
  let server = null;
  let serverReady = null;
  let draftDir = null;
  if (shouldSpawn) {
    draftDir = fs.mkdtempSync(path.join(os.tmpdir(), "meta-flow-editor-source."));
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
    await page.goto(`${baseUrl}/flow-editor?cache_bust=browser-source-smoke`, {
      waitUntil: "domcontentloaded",
    });
    await page.getByText("api ready").waitFor({ timeout: STRUCTURAL_TIMEOUT_MS });
    await createMobThroughLibrary(page, "Browser Source Mob");

    await page.locator(".bld-toml-toggle").click();
    const inlineEditor = page.locator(".bld-toml");
    await inlineEditor.waitFor({ timeout: STRUCTURAL_TIMEOUT_MS });

    const sourceBox = inlineEditor.locator('[role="textbox"][aria-readonly="true"]');
    await sourceBox.waitFor({ timeout: STRUCTURAL_TIMEOUT_MS });
    await waitForSourceSections(page, ".bld-toml", ["[profiles.", "[flows."]);
    const mobToml = await sourceBox.innerText();
    if (!mobToml.includes("[profiles.") || !mobToml.includes("[flows.")) {
      throw new Error(`inline mob.toml editor did not render Meta source: ${mobToml.slice(0, 400)}`);
    }

    const definitionButton = inlineEditor.locator(".source-file-row", { hasText: "definition.json" });
    await definitionButton.click();
    await waitForSourceSections(page, ".bld-toml", ['"id"', '"profiles"']);
    const definitionJson = await sourceBox.innerText();
    if (!definitionJson.includes('"id"') || !definitionJson.includes('"profiles"')) {
      throw new Error(`inline definition.json editor did not render exported source: ${definitionJson.slice(0, 400)}`);
    }

    const readonly = await sourceBox.getAttribute("aria-readonly");
    if (readonly !== "true") {
      throw new Error(`inline source editor must be read-only, got aria-readonly=${readonly}`);
    }

    await page.locator(".bld-toml-toggle").click();
    await inlineEditor.waitFor({ state: "hidden", timeout: STRUCTURAL_TIMEOUT_MS });

    await page.locator(".bld-toml-toggle").click();
    await inlineEditor.waitFor({ timeout: STRUCTURAL_TIMEOUT_MS });
    await inlineEditor.locator(".bld-toml__head .btn--ghost").click();
    await inlineEditor.waitFor({ state: "hidden", timeout: STRUCTURAL_TIMEOUT_MS });

    await page.locator("button.modetoggle__opt", { hasText: "Graph" }).click();
    const graphSource = page.locator(".node--source-file").first();
    await graphSource.waitFor({ state: "visible", timeout: STRUCTURAL_TIMEOUT_MS });
    const graphSourceClass = await graphSource.getAttribute("class");
    if ((graphSourceClass || "").split(/\s+/).includes("node")) {
      throw new Error(`graph source file affordance must not inherit graph node chrome: ${graphSourceClass}`);
    }
    await graphSource.click();
    const graphInlineEditor = page.locator(".bld-toml--graph");
    await graphInlineEditor.waitFor({ timeout: STRUCTURAL_TIMEOUT_MS });
    const graphSourceBox = graphInlineEditor.locator('[role="textbox"][aria-readonly="true"]');
    // This is the wait #398 timed out on. The earlier reading ("the editor
    // virtualizes rows, scroll it so every section renders") was wrong: the
    // panel is a plain <pre> and innerText already carries the whole file. The
    // panel had simply never received a document; see waitForSourceSections.
    await waitForSourceSections(page, ".bld-toml--graph", ["[profiles.", "[flows."]);
    const graphReadonly = await graphSourceBox.getAttribute("aria-readonly");
    if (graphReadonly !== "true") {
      throw new Error(`graph inline source editor must be read-only, got aria-readonly=${graphReadonly}`);
    }
    await graphInlineEditor.locator(".bld-toml__head .btn--ghost").click();
    await graphInlineEditor.waitFor({ state: "hidden", timeout: STRUCTURAL_TIMEOUT_MS });

    await page.locator("button.viewtab", { hasText: "AGENTS" }).click();
    await page.locator(".agents-view").waitFor({ state: "visible", timeout: STRUCTURAL_TIMEOUT_MS });
    const agentCountBefore = await page.locator(".agents-list__name").count();
    await chooseFirstRealAgentDefinition(page);
    await page.waitForFunction(
      (count) => document.querySelectorAll(".agents-list__name").length > count,
      agentCountBefore,
      { timeout: STRUCTURAL_TIMEOUT_MS },
    );
    await page.locator(".agents-list__scroll").first().locator(".agents-list__item").last().click();
    const titleInput = page.locator(".agent-editor__title-input");
    await titleInput.fill("Browser Source Agent");
    await page.waitForTimeout(1_000);
    const toolSelect = page.locator("select").filter({ has: page.locator('option[value="image_generation"]') }).first();
    await toolSelect.selectOption("image_generation");
    await page.waitForFunction(() => {
      return Array.from(document.querySelectorAll(".tool-row .name")).some((row) => row.textContent.trim() === "image_generation");
    }, null, { timeout: STRUCTURAL_TIMEOUT_MS });
    await page.getByRole("button", { name: /INLINE/i }).click();
    await page.locator(".inline-skill input").fill("browser source");
    await page.locator(".inline-skill textarea").fill("Use this skill to prove edited agent source rendering.");
    await page.locator(".inline-skill .btn", { hasText: "ADD" }).click();
    await page.waitForFunction(() => {
      return Array.from(document.querySelectorAll(".skill-row__name")).some((row) => row.textContent.trim() === "mob.browser.source");
    }, null, { timeout: STRUCTURAL_TIMEOUT_MS });

    await page.locator("button.viewtab", { hasText: "FLOW" }).click();
    await page.locator("button.modetoggle__opt", { hasText: "Basic" }).click();
    await page.locator(".bld-toml-toggle").click();
    const editedSource = page.locator(".bld-toml:visible");
    await editedSource.waitFor({ state: "visible", timeout: STRUCTURAL_TIMEOUT_MS });
    const editedSourceBox = editedSource.locator('[role="textbox"][aria-readonly="true"]');
    await editedSourceBox.waitFor({ timeout: STRUCTURAL_TIMEOUT_MS });
    await editedSource.locator(".source-file-row", { hasText: "definition.json" }).waitFor({ state: "visible", timeout: STRUCTURAL_TIMEOUT_MS });
    await editedSource.locator(".source-file-row", { hasText: "definition.json" }).click();
    await page.waitForFunction(() => {
      const visiblePanel = Array.from(document.querySelectorAll(".bld-toml")).find((panel) => getComputedStyle(panel).display !== "none");
      const source = visiblePanel?.querySelector('[role="textbox"][aria-readonly="true"]');
      return source?.innerText.includes("browser_source_agent")
        && source?.innerText.includes("image_generation")
        && source?.innerText.includes("mob.browser.source");
    }, null, { timeout: STRUCTURAL_TIMEOUT_MS });
    const editedDefinitionJson = await editedSourceBox.innerText();
    for (const required of ["browser_source_agent", "image_generation", "mob.browser.source"]) {
      if (!editedDefinitionJson.includes(required)) {
        throw new Error(`edited definition.json source did not include ${required}: ${editedDefinitionJson.slice(0, 600)}`);
      }
    }
    await editedSource.locator(".source-file-row", { hasText: "meta/mob.toml" }).click();
    await page.waitForFunction(() => {
      const visiblePanel = Array.from(document.querySelectorAll(".bld-toml")).find((panel) => getComputedStyle(panel).display !== "none");
      const source = visiblePanel?.querySelector('[role="textbox"][aria-readonly="true"]');
      return source?.innerText.includes("image_generation")
        && source?.innerText.includes("mob.browser.source")
        && source?.innerText.includes("Use this skill to prove edited agent source rendering.");
    }, null, { timeout: STRUCTURAL_TIMEOUT_MS });
    const editedMobToml = await editedSourceBox.innerText();
    for (const required of ["image_generation", "mob.browser.source", "Use this skill to prove edited agent source rendering."]) {
      if (!editedMobToml.includes(required)) {
        throw new Error(`edited mob.toml source did not include ${required}: ${editedMobToml.slice(0, 600)}`);
      }
    }
    await editedSource.locator(".bld-toml__head .btn--ghost").click();
    await editedSource.waitFor({ state: "hidden", timeout: STRUCTURAL_TIMEOUT_MS });

    await page.locator("button.modetoggle__opt", { hasText: "Basic" }).click();
    await page.setViewportSize({ width: 390, height: 820 });
    await page.waitForTimeout(100);
    const mobileLayout = await page.evaluate(() => {
      const rectInfo = (selector) => {
        const el = document.querySelector(selector);
        if (!el) return null;
        const rect = el.getBoundingClientRect();
        return {
          left: rect.left,
          right: rect.right,
          top: rect.top,
          bottom: rect.bottom,
          width: rect.width,
          height: rect.height,
          scrollWidth: el.scrollWidth,
          clientWidth: el.clientWidth,
        };
      };
      return {
        windowWidth: window.innerWidth,
        documentWidth: document.documentElement.scrollWidth,
        toprail: rectInfo(".toprail"),
        actions: rectInfo(".actions"),
        stage: rectInfo(".bld-stage"),
      };
    });
    if (mobileLayout.documentWidth > mobileLayout.windowWidth) {
      throw new Error(`mobile page must not overflow horizontally: ${JSON.stringify(mobileLayout)}`);
    }
    if (mobileLayout.toprail.scrollWidth > mobileLayout.windowWidth) {
      throw new Error(`mobile toprail must fit viewport: ${JSON.stringify(mobileLayout)}`);
    }
    if (!mobileLayout.stage || mobileLayout.stage.width < 240) {
      throw new Error(`mobile Basic editor stage collapsed: ${JSON.stringify(mobileLayout)}`);
    }

    await page.locator(".toprail .btn", { hasText: "VALIDATE" }).click();
    await page.locator(".validate").waitFor({ state: "visible", timeout: STRUCTURAL_TIMEOUT_MS });
    const validateRect = await page.locator(".validate").evaluate((el) => {
      const rect = el.getBoundingClientRect();
      return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom, width: rect.width };
    });
    if (validateRect.left < -0.5 || validateRect.right > 390.5 || validateRect.width <= 0) {
      throw new Error(`mobile validate sheet must stay inside viewport: ${JSON.stringify(validateRect)}`);
    }
    await page.locator(".validate__head .btn").last().click();
    await page.locator(".validate").waitFor({ state: "hidden", timeout: STRUCTURAL_TIMEOUT_MS });

    await page.locator(".actions-menu__summary").click();
    await page.locator(".actions-menu__item", { hasText: "PLAN TRACE" }).click();
    await page.locator(".deploy-plan").waitFor({ state: "visible", timeout: STRUCTURAL_TIMEOUT_MS });
    const deployPlanRect = await page.locator(".deploy-plan").evaluate((el) => {
      const rect = el.getBoundingClientRect();
      return { left: rect.left, right: rect.right, top: rect.top, bottom: rect.bottom, width: rect.width };
    });
    if (deployPlanRect.left < -0.5 || deployPlanRect.right > 390.5 || deployPlanRect.width <= 0) {
      throw new Error(`mobile deploy plan must stay inside viewport: ${JSON.stringify(deployPlanRect)}`);
    }

    await page.locator("button.viewtab", { hasText: "AGENTS" }).click();
    const runtimeDetails = page.locator("details.agent-runtime").first();
    await runtimeDetails.waitFor({ state: "visible", timeout: STRUCTURAL_TIMEOUT_MS });
    const runtimeState = await runtimeDetails.evaluate((el) => {
      const body = el.querySelector(".agent-runtime__body");
      return {
        open: el.open,
        summary: el.querySelector("summary")?.textContent?.trim() || "",
        bodyDisplay: body ? getComputedStyle(body).display : "",
      };
    });
    if (runtimeState.open || runtimeState.summary !== "RUNTIME" || runtimeState.bodyDisplay !== "none") {
      throw new Error(`agent runtime section must render collapsed behind schema-backed title: ${JSON.stringify(runtimeState)}`);
    }

    if (consoleMessages.length) {
      throw new Error(`browser console errors:\n${consoleMessages.join("\n")}`);
    }
  } finally {
    await browser.close();
    if (server) {
      server.kill("SIGTERM");
    }
  }
}

main().catch((error) => {
  console.error(error);
  process.exit(1);
});
