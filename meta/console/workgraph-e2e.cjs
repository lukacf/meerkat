#!/usr/bin/env node

// WorkGraph graph-view browser e2e: boots the real console bundle in
// chromium against crates/meerkat-meta/examples/workgraph_console_reference
// (library-mode runtime, builder-wired ephemeral WorkGraph service,
// unenforced console), seeds a deterministic six-item fixture through the
// real `POST /console/rpc` wire contract (goal/create root + 5 children,
// 5 parent edges + 1 blocks edge, one claim/close/block each), then
// asserts the layered-DAG graph view: node/edge counts, per-status
// classes, wheel-zoom + drag-pan transform changes, click-select detail.
//
// Structure mirrors memory-e2e.cjs / browser-e2e.cjs (spawn backend via
// repo-cargo, wait for /healthz + /console, playwright chromium, assert,
// SIGTERM/SIGKILL teardown). Screenshots always land in a mkdtemp dir
// (path printed) so a green run keeps visual evidence too.

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawn, spawnSync } = require("node:child_process");
const { setTimeout: sleep } = require("node:timers/promises");
const { chromium } = require("playwright");
const { exampleBackendSpec } = require("./example-backend.cjs");
const { LOOPBACK_ANY_PORT, awaitFixtureReady } = require("./fixture-ready.cjs");

const repoRoot = path.resolve(__dirname, "..");
const screenshotDir = fs.mkdtempSync(path.join(os.tmpdir(), "workgraph-e2e-"));

// ── Harness plumbing (matches browser-e2e.cjs) ──────────────────────────


async function waitForHttpOk(url, timeoutMs = 240_000) {
  const deadline = Date.now() + timeoutMs;
  while (Date.now() < deadline) {
    try {
      const response = await fetch(url);
      if (response.ok) return;
    } catch (_) {
      // Backend is still compiling/starting.
    }
    await sleep(500);
  }
  throw new Error(`timed out waiting for ${url}`);
}

function waitForExit(child, timeoutMs = 5_000) {
  return new Promise((resolve) => {
    if (child.exitCode !== null) {
      resolve();
      return;
    }
    const timer = setTimeout(() => {
      if (child.exitCode === null) child.kill("SIGKILL");
      resolve();
    }, timeoutMs);
    child.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

async function stopBackend(child) {
  if (!child || child.exitCode !== null) return;
  child.kill("SIGTERM");
  await waitForExit(child);
}

async function launchBrowser() {
  try {
    return await chromium.launch({ headless: true });
  } catch (launchError) {
    const npxCommand = process.platform === "win32" ? "npx.cmd" : "npx";
    const installResult = spawnSync(npxCommand, ["playwright", "install", "chromium"], {
      cwd: __dirname,
      stdio: "inherit",
    });
    if (installResult.status !== 0) {
      throw new Error(`playwright chromium install failed with status ${installResult.status}`);
    }
    return chromium.launch({ headless: true });
  }
}

async function gotoConsole(page, url) {
  await page.goto(url, { waitUntil: "domcontentloaded", timeout: 30_000 });
  await page.waitForSelector('[data-testid="meerkat-console"]', { timeout: 30_000 });
}

// ── Seed over the real console RPC wire contract ────────────────────────

let rpcSeq = 0;
async function rpc(baseUrl, method, params) {
  rpcSeq += 1;
  const response = await fetch(`${baseUrl}/console/rpc`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: `wg-e2e-${rpcSeq}`, method, params }),
  });
  assert.ok(response.ok, `${method}: http ${response.status}`);
  const body = await response.json();
  if (body.error) {
    // -32041 unavailable / -32042 conflict / -32030 denied are all fatal
    // fixture bugs here - the reference runtime runs unenforced.
    throw new Error(`${method} failed: ${JSON.stringify(body.error)}`);
  }
  return body.result;
}

async function seedWorkGraph(baseUrl) {
  // Root via goal/create so the graph carries an attention binding too.
  const goal = await rpc(baseUrl, "meta/workgraph/goal/create", {
    title: "Ship 0.8.13",
    target: { kind: "identity", identity: "planner" },
  });
  const rootId = goal.item.id;
  assert.ok(rootId, "goal/create must return the goal work item");
  assert.equal(goal.attention.status.state, "active");

  const childTitles = [
    "Graph layout module",
    "Graph view component",
    "Panel toggle wiring",
    "Browser e2e lane",
    "Console docs note",
  ];
  const children = [];
  for (const title of childTitles) {
    const created = await rpc(baseUrl, "meta/workgraph/create", { title });
    assert.ok(created.item.id, `create '${title}' returned an item id`);
    children.push(created.item);
  }

  // Edges first (links never bump item revisions, so the create-time CAS
  // tokens below stay valid). Parent edges run child→parent.
  for (const child of children) {
    await rpc(baseUrl, "meta/workgraph/link", {
      kind: "parent",
      from_id: child.id,
      to_id: rootId,
    });
  }
  await rpc(baseUrl, "meta/workgraph/link", {
    kind: "blocks",
    from_id: children[0].id,
    to_id: children[3].id,
  });

  // One child each: in_progress / completed / blocked. Every mutation
  // threads the item's latest returned revision.
  const claimed = await rpc(baseUrl, "meta/workgraph/claim", {
    id: children[0].id,
    expected_revision: children[0].revision,
    owner: { key: { kind: "agent", id: "builder" } },
  });
  assert.equal(claimed.item.status, "in_progress");
  const closed = await rpc(baseUrl, "meta/workgraph/close", {
    id: children[1].id,
    expected_revision: children[1].revision,
  });
  assert.equal(closed.item.status, "completed");
  const blocked = await rpc(baseUrl, "meta/workgraph/block", {
    id: children[2].id,
    expected_revision: children[2].revision,
  });
  assert.equal(blocked.item.status, "blocked");

  // Wire-level sanity before touching the browser: 6 items, 6 edges. The
  // default snapshot drops terminal rows, so opt in like the panel does.
  const snapshot = await rpc(baseUrl, "meta/workgraph/snapshot", {
    include_terminal: true,
  });
  assert.equal(snapshot.items.length, 6, "seeded item count");
  assert.equal(snapshot.edges.length, 6, "seeded edge count (5 parent + 1 blocks)");
  assert.equal(snapshot.attention.length, 1, "goal binding present");
  console.log(`workgraph e2e: seeded root ${rootId} + ${children.length} children`);
  return { rootId, claimedId: children[0].id };
}

// ── Browser assertions ───────────────────────────────────────────────────

async function runBrowserChecks(page, baseUrl, seeded) {
  await gotoConsole(page, `${baseUrl}/console`);
  await page.click('[data-testid="nav:workgraph"]');
  await page.waitForSelector('[data-testid="workgraph-panel"]', { timeout: 15_000 });

  await page.click('[data-testid="workgraph-view-toggle:graph"]');
  await page.waitForSelector('[data-testid="workgraph-graph"]', { timeout: 15_000 });
  // Snapshot hydration is async; wait for the full fixture to draw.
  await page.waitForFunction(
    () => document.querySelectorAll('[data-testid="workgraph-graph-node"]').length === 6,
    null,
    { timeout: 15_000 },
  );

  assert.equal(await page.locator('[data-testid="workgraph-graph-node"]').count(), 6);
  assert.equal(await page.locator('[data-testid="workgraph-graph-edge"]').count(), 6);
  for (const status of ["in_progress", "completed", "blocked"]) {
    const count = await page
      .locator(`[data-testid="workgraph-graph-node"][data-status="${status}"]`)
      .count();
    assert.equal(count, 1, `exactly one ${status} node`);
    const classes = (await page
      .locator(`[data-testid="workgraph-graph-node"][data-status="${status}"]`)
      .getAttribute("class")) ?? "";
    assert.ok(classes.includes(`is-${status}`), `node carries is-${status}: ${classes}`);
  }
  assert.equal(
    await page.locator('[data-testid="workgraph-graph-edge"][data-kind="parent"]').count(),
    5,
  );
  assert.equal(
    await page.locator('[data-testid="workgraph-graph-edge"][data-kind="blocks"]').count(),
    1,
  );

  // Label legibility: the graph opts out of the global icon stroke (which
  // used to outline every glyph), and titles render at a real pixel size
  // instead of shrinking with a viewBox fit.
  const titleStyle = await page.evaluate(() => {
    const title = document.querySelector(".workgraph-graph__node-title");
    const style = getComputedStyle(title);
    return {
      stroke: style.stroke,
      fontFamily: style.fontFamily,
      renderedHeight: title.getBoundingClientRect().height,
      consoleFont: getComputedStyle(document.querySelector('[data-testid="workgraph-graph-frame"]'))
        .fontFamily,
    };
  });
  assert.equal(titleStyle.stroke, "none", "graph titles carry no inherited stroke");
  assert.ok(
    titleStyle.renderedHeight >= 10,
    `graph titles render legibly at fit: ${titleStyle.renderedHeight}px`,
  );
  assert.equal(titleStyle.fontFamily, titleStyle.consoleFont, "titles use the console font stack");
  assert.equal(
    await page.locator('[data-testid="workgraph-graph-edge-label"]').count(),
    1,
    "the blocks edge carries its label",
  );

  // Pan/zoom smoke: both gestures must move the viewport transform.
  const viewport = page.locator('[data-testid="workgraph-graph-viewport"]');
  const initialTransform = await viewport.getAttribute("transform");
  assert.ok(initialTransform, "viewport carries a transform");
  const box = await page.locator('[data-testid="workgraph-graph"]').boundingBox();
  assert.ok(box, "graph svg has a bounding box");
  const cx = box.x + box.width / 2;
  const cy = box.y + box.height / 2;

  await page.mouse.move(cx, cy);
  await page.mouse.wheel(0, -240);
  await page.waitForFunction(
    (previous) =>
      document
        .querySelector('[data-testid="workgraph-graph-viewport"]')
        ?.getAttribute("transform") !== previous,
    initialTransform,
    { timeout: 5_000 },
  );
  const zoomedTransform = await viewport.getAttribute("transform");
  assert.notEqual(zoomedTransform, initialTransform, "wheel zoom changes the transform");

  // Drag from a corner of the canvas (background, not a node label).
  const panStartX = box.x + box.width - 30;
  const panStartY = box.y + box.height - 30;
  await page.mouse.move(panStartX, panStartY);
  await page.mouse.down();
  await page.mouse.move(panStartX - 70, panStartY - 40, { steps: 5 });
  await page.mouse.up();
  // pointermove is a continuous-priority React event: wait for the state
  // flush to land in the DOM (same settle the zoom assertion gets) instead
  // of racing the CDP round-trip.
  await page.waitForFunction(
    (previous) =>
      document
        .querySelector('[data-testid="workgraph-graph-viewport"]')
        ?.getAttribute("transform") !== previous,
    zoomedTransform,
    { timeout: 5_000 },
  );
  const pannedTransform = await viewport.getAttribute("transform");
  assert.notEqual(pannedTransform, zoomedTransform, "drag pan changes the transform");

  // Fit returns to the initial fitted view.
  await page.click('[data-testid="workgraph-graph-fit"]');
  assert.equal(await viewport.getAttribute("transform"), initialTransform, "Fit resets the view");

  // Click-select: the footer detail names the selected item and status.
  await page.click('[data-testid="workgraph-graph-node"][data-status="in_progress"]');
  const detail =
    (await page.locator('[data-testid="workgraph-graph-detail"]').textContent()) ?? "";
  assert.ok(detail.includes(seeded.claimedId), `detail names the item: ${detail}`);
  assert.ok(detail.includes("In progress"), `detail carries the readable status: ${detail}`);

  const screenshotPath = path.join(screenshotDir, "workgraph-graph.png");
  await page.screenshot({ path: screenshotPath, fullPage: true });
  console.log(`workgraph e2e: screenshot ${screenshotPath}`);
}

// ── Main ─────────────────────────────────────────────────────────────────

async function main() {
  const backendSpec = exampleBackendSpec(repoRoot, "workgraph_console_reference");
  const backend = spawn(
    backendSpec.command,
    backendSpec.args,
    {
      cwd: repoRoot,
      env: {
        ...process.env,
        // Guards the known rustc incremental-build ICE (same as the CI
        // flow-editor job).
        CARGO_INCREMENTAL: "0",
        META_WORKGRAPH_E2E_ADDR: LOOPBACK_ANY_PORT,
      },
      stdio: ["ignore", "pipe", "pipe"],
    },
  );
  backend.stderr.on("data", (chunk) => process.stderr.write(chunk));
  backend.stdout.on("data", (chunk) => process.stdout.write(chunk));

  let browser = null;
  let page = null;
  let baseUrl;
  try {
    ({ baseUrl } = await awaitFixtureReady(backend, {
      label: "workgraph e2e fixture",
      timeoutMs: backendSpec.prebuilt ? 60_000 : 240_000,
    }));
    await waitForHttpOk(`${baseUrl}/healthz`);
    await waitForHttpOk(`${baseUrl}/console`);
    const seeded = await seedWorkGraph(baseUrl);

    browser = await launchBrowser();
    const context = await browser.newContext({ viewport: { width: 1440, height: 900 } });
    page = await context.newPage();
    await runBrowserChecks(page, baseUrl, seeded);
    console.log("workgraph e2e: PASS");
  } catch (error) {
    if (page) {
      const failurePath = path.join(screenshotDir, "workgraph-failure.png");
      await page.screenshot({ path: failurePath, fullPage: true }).catch(() => {});
      console.error(`workgraph e2e: failure screenshot ${failurePath}`);
    }
    throw error;
  } finally {
    if (browser) await browser.close().catch(() => {});
    await stopBackend(backend);
  }
}

main().catch((error) => {
  console.error(error);
  process.exitCode = 1;
});
