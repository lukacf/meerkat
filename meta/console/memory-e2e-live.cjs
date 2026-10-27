#!/usr/bin/env node

// LIVE-LOOP memory console e2e (UI-P1.C acceptance lane).
//
// Unlike memory-e2e.cjs (the deterministic seeded lane CI runs), this lane
// asserts the console against what the REAL memory system organically
// produces: the real rpc_gateway (production identity-first wiring, booted
// through the Python SDK), a real haiku identity executing real memory-tool
// calls, the real distiller (interaction-triggered) and the real steward
// dream loop (short cadence) running real model calls. NOTHING is seeded.
//
// Lane posture: creds-gated acceptance. Exit 3 = loud SKIP when no
// provider auth resolves (same contract as the live eval bins). Assertions
// are SHAPE-based (structure, counts>=, authorship, event presence) —
// never exact prose, because model output varies. Bounded cost: <=6 haiku
// agent turns + interaction distillations (haiku) + one steward dream
// (sonnet); order of magnitude a few cents per run.
//
// Run: npm run e2e:memory:live   (CI runs e2e:memory — the seeded lane)

const assert = require("node:assert/strict");
const fs = require("node:fs");
const os = require("node:os");
const path = require("node:path");
const { spawn, spawnSync } = require("node:child_process");
const { setTimeout: sleep } = require("node:timers/promises");
const { chromium } = require("playwright");

const T = require("./memory-testids.cjs");

const repoRoot = path.resolve(__dirname, "..");
const screenshotDir = fs.mkdtempSync(path.join(os.tmpdir(), "memory-e2e-live-"));
const startedAt = Date.now();

const IDENTITY = "identity:curator";

// The observed-values report the lead asked for, printed at the end.
const observed = [];
function note(line) {
  observed.push(line);
  process.stdout.write(`live e2e: ${line}\n`);
}
function loudGap(line) {
  observed.push(`GAP(tolerated): ${line}`);
  process.stdout.write(`live e2e GAP (tolerated, logged loud): ${line}\n`);
}

// ── Gateway/browser plumbing ─────────────────────────────────────────────

async function launchBrowser() {
  try {
    return await chromium.launch({ headless: true });
  } catch (_) {
    const npxCommand = process.platform === "win32" ? "npx.cmd" : "npx";
    const install = spawnSync(npxCommand, ["playwright", "install", "chromium"], {
      cwd: __dirname,
      stdio: "inherit",
    });
    if (install.status !== 0) throw new Error("playwright chromium install failed");
    return chromium.launch({ headless: true });
  }
}

async function gotoConsole(page, url) {
  await page.goto(url, { waitUntil: "domcontentloaded", timeout: 30_000 });
  await page.waitForSelector(
    '[data-testid="meerkat-console"], .cc-workbench, .cc-conversation-pane',
    { timeout: 30_000 },
  );
}

let rpcSeq = 0;
async function rpc(baseUrl, method, params = {}) {
  rpcSeq += 1;
  const response = await fetch(`${baseUrl}/console/rpc`, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({ jsonrpc: "2.0", id: `live-${rpcSeq}`, method, params }),
  });
  assert(response.ok, `${method} HTTP ${response.status}`);
  const payload = await response.json();
  assert(!payload.error, `${method} RPC error: ${JSON.stringify(payload.error)}`);
  return payload.result;
}

/// Poll `probe` until it returns a truthy value or the deadline passes.
async function pollUntil(label, timeoutMs, intervalMs, probe) {
  const deadline = Date.now() + timeoutMs;
  for (;;) {
    const value = await probe();
    if (value) return value;
    if (Date.now() > deadline) {
      throw new Error(`timed out after ${timeoutMs}ms waiting for: ${label}`);
    }
    await sleep(intervalMs);
  }
}

async function timelineFrames(baseUrl, params = {}) {
  const page = await rpc(baseUrl, "meta/console/query_timeline", {
    mode: "recent",
    limit: 400,
    ...params,
  });
  return page.frames || [];
}

const AUTH_ERROR_RE = /auth|credential|api[_ ]?key|unauthorized|401|permission/i;

// ── Boot the live fixture through the Python SDK ────────────────────────

function buildGatewayBinary() {
  if (process.env.META_GATEWAY_BIN) return;
  process.stdout.write("live e2e: building rpc_gateway (repo-cargo)…\n");
  const build = spawnSync(
    path.join(repoRoot, "scripts", "repo-cargo"),
    ["build", "-p", "meerkat-meta", "--bin", "rpc_gateway"],
    {
      cwd: repoRoot,
      env: { ...process.env, CARGO_INCREMENTAL: "0" },
      stdio: ["ignore", "inherit", "inherit"],
    },
  );
  if (build.status !== 0) throw new Error("rpc_gateway build failed");
}

function startDriver() {
  const driver = spawn("python3", [path.join(__dirname, "memory-e2e-live-driver.py")], {
    cwd: repoRoot,
    env: { ...process.env },
    stdio: ["pipe", "pipe", "pipe"],
  });
  driver.stderr.on("data", (chunk) => process.stderr.write(chunk));
  return new Promise((resolve, reject) => {
    let buffered = "";
    const onData = (chunk) => {
      buffered += chunk.toString();
      let index;
      while ((index = buffered.indexOf("\n")) >= 0) {
        const line = buffered.slice(0, index).trim();
        buffered = buffered.slice(index + 1);
        if (!line) continue;
        process.stdout.write(`driver: ${line}\n`);
        if (line.startsWith("READY ")) {
          driver.stdout.off("data", onData);
          // Keep echoing later driver output.
          driver.stdout.on("data", (later) => process.stdout.write(`driver: ${later}`));
          resolve({ driver, info: JSON.parse(line.slice("READY ".length)) });
          return;
        }
        if (line.startsWith("SKIP")) {
          driver.stdout.off("data", onData);
          reject(Object.assign(new Error(line), { skip: true }));
          return;
        }
        if (line.startsWith("ERROR")) {
          driver.stdout.off("data", onData);
          reject(new Error(line));
          return;
        }
      }
    };
    driver.stdout.on("data", onData);
    driver.once("exit", (code) => {
      reject(
        Object.assign(new Error(`driver exited before READY (code ${code})`), {
          skip: code === 3,
        }),
      );
    });
    setTimeout(() => reject(new Error("driver did not become READY in 300s")), 300_000);
  });
}

async function stopDriver(driver) {
  if (!driver || driver.exitCode !== null) return;
  driver.stdin.end(); // stdin EOF -> graceful runtime shutdown
  await new Promise((resolve) => {
    const timer = setTimeout(() => {
      if (driver.exitCode === null) driver.kill("SIGKILL");
      resolve();
    }, 30_000);
    driver.once("exit", () => {
      clearTimeout(timer);
      resolve();
    });
  });
}

// ── Phase A: real turns through the console send lane ───────────────────

const TURNS = [
  "Remember this for future deploys: the staging endpoint is " +
    "https://staging.internal.acme.dev — store it in your durable memory.",
  "Another durable fact to remember: our release train runs on Tuesdays, " +
    "only after CI is green.",
  "Correction — the staging endpoint has MOVED to " +
    "https://staging-eu.internal.acme.dev. Update the fact in your memory " +
    "so the old endpoint is no longer current.",
  "From your memory: what is our current staging endpoint, and when does " +
    "the release train run?",
];

async function driveRealTurns(baseUrl) {
  let completed = 0;
  for (const [index, content] of TURNS.entries()) {
    const send = await rpc(baseUrl, "meta/console/send", {
      identity: IDENTITY,
      content,
      origin: "memory-e2e-live",
      idempotency_key: `live-turn-${index + 1}`,
    });
    assert(send, `send ${index + 1} accepted`);
    const target = completed + 1;
    await pollUntil(`turn ${index + 1} completion`, 180_000, 1_000, async () => {
      const frames = await timelineFrames(baseUrl, { identity: IDENTITY });
      const failed = frames.find(
        (frame) =>
          frame.kind === "interaction_failed" &&
          AUTH_ERROR_RE.test(JSON.stringify(frame.payload || {})),
      );
      if (failed) {
        // The real resolution failure IS the skip signal (eval-bin contract).
        throw Object.assign(
          new Error(`SKIP auth failed during live turn: ${JSON.stringify(failed.payload)}`),
          { skip: true },
        );
      }
      const done = frames.filter((frame) => frame.kind === "interaction_complete").length;
      return done >= target ? done : null;
    });
    completed += 1;
    note(`turn ${index + 1} completed (real model, real memory tool available)`);
  }
  return completed;
}

// ── Main ─────────────────────────────────────────────────────────────────

async function main() {
  buildGatewayBinary();
  const { driver, info } = await startDriver();
  const baseUrl = info.http_base_url;
  note(`gateway ready at ${baseUrl} (state: ${info.state_dir})`);

  let browser;
  try {
    // Phase A — real turns.
    await driveRealTurns(baseUrl);

    // Phase B — the real store fills organically. Agent-authored records
    // must exist; update/supersede evidence should exist because turn 3
    // contradicts turn 1 (tolerate either shape, loudly).
    const records = await pollUntil(
      "agent-authored records in the real store",
      120_000,
      2_000,
      async () => {
        const result = await rpc(baseUrl, "meta/memory/panel/records", { limit: 200 });
        const rows = result.records || [];
        const agentRows = rows.filter(
          (row) => row.provenance?.author?.author === "agent",
        );
        return agentRows.length >= 2 ? rows : null;
      },
    );
    const agentRows = records.filter((row) => row.provenance?.author?.author === "agent");
    note(
      `real store: ${records.length} records total, ${agentRows.length} agent-authored ` +
        `(titles: ${agentRows.map((row) => JSON.stringify(row.title)).join(", ")})`,
    );
    const superseded = records.filter((row) => row.status?.status === "superseded");
    const updated = records.filter(
      (row) =>
        typeof row.updated_at_ms === "number" &&
        typeof row.created_at_ms === "number" &&
        row.updated_at_ms > row.created_at_ms,
    );
    if (superseded.length > 0) {
      note(`contradiction produced a SUPERSEDE (${superseded.length} superseded records)`);
    } else if (updated.length > 0) {
      note(`contradiction produced an UPDATE (${updated.length} records updated in place)`);
    } else {
      loudGap(
        "no supersede or in-place update evidence after the contradicting turn — " +
          "the model may have written a fresh record instead; records: " +
          records.map((row) => `${row.title}:${row.status?.status}`).join(", "),
      );
    }

    // Distiller: interaction-triggered (min_interactions=1) off RunCompleted.
    // Judgment-dependent — the model may extract nothing; tolerate loudly.
    let distillerRows = [];
    try {
      const withDistiller = await pollUntil(
        "distiller-authored records",
        180_000,
        3_000,
        async () => {
          const result = await rpc(baseUrl, "meta/memory/panel/records", { limit: 200 });
          const rows = (result.records || []).filter(
            (row) => row.provenance?.author?.author === "distiller",
          );
          return rows.length > 0 ? rows : null;
        },
      );
      distillerRows = withDistiller;
      note(
        `real distiller committed ${distillerRows.length} record(s): ` +
          distillerRows.map((row) => JSON.stringify(row.title)).join(", "),
      );
    } catch (_) {
      loudGap(
        "no distiller-authored records within 180s — interaction distillation ran " +
          "against transcripts the model may have judged fully covered by recorder writes",
      );
    }

    // Phase C — the real steward dream loop (cadence */30s, min_signals 1).
    const dreams = await pollUntil(
      "a real steward dream in the durable audit",
      300_000,
      5_000,
      async () => {
        const result = await rpc(baseUrl, "meta/memory/panel/dreams", {});
        const runs = result.runs || [];
        return runs.length > 0 ? runs : null;
      },
    );
    note(
      `real steward dream(s): ${dreams
        .map((run) => `${run.run_id} (${run.ops} ops, kinds: ${JSON.stringify(run.op_kinds)})`)
        .join("; ")}`,
    );
    const memoryFrames = (await timelineFrames(baseUrl)).filter((frame) =>
      String(frame.kind || "").startsWith("memory."),
    );
    const memoryKinds = [...new Set(memoryFrames.map((frame) => frame.kind))];
    note(`memory.* events on the live timeline: ${memoryKinds.join(", ") || "(none)"}`);
    assert(
      memoryKinds.includes("memory.dream.started") ||
        memoryKinds.includes("memory.dream.completed") ||
        memoryKinds.includes("memory.dream.skipped"),
      `real dream lifecycle events must reach the timeline: ${memoryKinds.join(", ")}`,
    );

    // Phase D — drive the real console over the organic store.
    browser = await launchBrowser();
    const page = await browser.newPage({ viewport: { width: 1440, height: 900 } });
    await gotoConsole(page, `${baseUrl}/console`);
    await page.getByTestId(T.NAV_MEMORY).click();
    await page.getByTestId(T.PANEL).waitFor({ timeout: 15_000 });

    // Holdings + verdict strip compute over the real store without error.
    await page.getByTestId(T.HOLDINGS).waitFor({ timeout: 15_000 });
    await page.getByTestId(T.VERDICT_STRIP).waitFor({ timeout: 15_000 });
    const tileStatuses = {};
    for (const id of ["echo-safety", "taint-wall", "lattice", "recall", "dreams", "store-floor"]) {
      const tile = page.getByTestId(T.verdictTile(id));
      await tile.waitFor({ timeout: 15_000 });
      const status = await tile.getAttribute("data-status");
      assert(
        ["holding", "degraded", "violated", "unverifiable", "no-grant"].includes(status),
        `tile ${id} has a legal status, got ${status}`,
      );
      tileStatuses[id] = status;
    }
    note(`verdict tiles over the real store: ${JSON.stringify(tileStatuses)}`);
    assert.equal(await page.getByTestId("memory-error").count(), 0, "panel renders no error");
    await page.screenshot({ path: path.join(screenshotDir, "1-holdings.png"), fullPage: true });

    // Records tab: agent-authored rows reached through the UI.
    await page.getByTestId(T.tab("records")).evaluate((node) => node.click());
    await page.locator('[data-testid^="memory-record:"]').first().waitFor({ timeout: 15_000 });
    const uiRowCount = await page.locator('[data-testid^="memory-record:"]').count();
    assert(uiRowCount >= agentRows.length, `UI shows the organic rows (${uiRowCount})`);
    note(`records tab renders ${uiRowCount} organic rows`);
    await page.screenshot({ path: path.join(screenshotDir, "2-records.png"), fullPage: true });

    // Biography of a real agent-authored record: sections render; BORN names
    // the agent author; LINEAGE appears when the contradiction superseded.
    const bioTarget =
      superseded.length > 0
        ? records.find((row) => row.status?.status === "active" && row.supersedes)
        : agentRows.find((row) => row.status?.status === "active");
    const bioId = (bioTarget || agentRows[0]).id;
    await page.getByTestId(T.record(bioId)).click();
    await page.getByTestId(T.DETAIL).waitFor({ timeout: 15_000 });
    for (const section of [T.DETAIL_BORN, T.DETAIL_LIFE, T.DETAIL_DREAMS]) {
      await page.getByTestId(section).waitFor({ timeout: 15_000 });
    }
    const born = await page.getByTestId(T.DETAIL_BORN).innerText();
    assert(
      born.includes("agent") || born.includes("distiller"),
      `biography BORN names the real author: ${born}`,
    );
    const chainCount = await page.locator('[data-testid^="memory-chain:"]').count();
    if (chainCount > 1) {
      note(`biography LINEAGE shows a real ${chainCount}-node chain`);
    } else {
      loudGap("biography has no multi-node lineage (contradiction handled without supersede)");
    }
    const detail = await rpc(baseUrl, "meta/memory/panel/record", { memory_id: bioId });
    if ((detail.injections || []).length > 0) {
      const life = await page.getByTestId(T.DETAIL_LIFE).innerText();
      assert(
        /build|turn/.test(life),
        `LIFE renders the real injection ledger rows: ${life}`,
      );
      note(`biography LIFE shows ${detail.injections.length} real injection row(s)`);
    } else {
      loudGap("no injection-ledger rows for the inspected record (no rebuild since write)");
    }
    await page.screenshot({ path: path.join(screenshotDir, "3-biography.png"), fullPage: true });
    await page.getByTestId(T.DETAIL_BACK).click();

    // Dreams tab: the REAL dream run with its real op count.
    await page.getByTestId(T.tab("dreams")).evaluate((node) => node.click());
    const realRun = dreams[0];
    await page.getByTestId(T.dream(realRun.run_id)).waitFor({ timeout: 15_000 });
    const dreamText = await page.getByTestId(T.dream(realRun.run_id)).innerText();
    assert(
      dreamText.includes(`${realRun.ops} ops`) || realRun.ops === 0,
      `dreams tab shows the real op count: ${dreamText}`,
    );
    note(`dreams tab renders real run ${realRun.run_id} (${realRun.ops} ops)`);
    await page.screenshot({ path: path.join(screenshotDir, "4-dreams.png"), fullPage: true });

    // Pipeline live strip: real memory.* frames, humanized.
    await page.getByTestId(T.tab("pipeline")).evaluate((node) => node.click());
    await page.getByTestId(T.LIVE_STRIP).waitFor({ timeout: 15_000 });
    await page
      .locator('[data-testid^="memory-live-row:"]')
      .first()
      .waitFor({ timeout: 15_000 });
    const stripText = await page.getByTestId(T.LIVE_STRIP).innerText();
    assert(!stripText.includes('{"'), `live strip copy is humanized: ${stripText.slice(0, 300)}`);
    note(
      `pipeline live strip renders ${await page
        .locator('[data-testid^="memory-live-row:"]')
        .count()} real memory event rows`,
    );
    await page.screenshot({ path: path.join(screenshotDir, "5-pipeline.png"), fullPage: true });

    // Signals rail: real dream lifecycle signal with humanized copy.
    const rail = page.getByTestId(T.SIGNALS_RAIL);
    await rail.waitFor({ timeout: 15_000 });
    const dreamSignal = rail
      .locator('[data-testid^="signal:"]')
      .filter({ hasText: /Memory dream/i })
      .first();
    try {
      await dreamSignal.waitFor({ timeout: 20_000 });
      assert(!(await rail.innerText()).includes('{"'), "rail copy is humanized");
      note("signals rail shows the real dream signal with humanized copy");
    } catch (_) {
      loudGap(
        "no dream signal visible in the rail (dream may have been skipped-quiet; " +
          `timeline kinds were: ${memoryKinds.join(", ")})`,
      );
    }
    await page.screenshot({ path: path.join(screenshotDir, "6-signals.png"), fullPage: true });

    const seconds = Math.round((Date.now() - startedAt) / 1000);
    process.stdout.write("\nlive e2e OBSERVED VALUES:\n");
    for (const line of observed) process.stdout.write(`  - ${line}\n`);
    process.stdout.write(`live e2e screenshots: ${screenshotDir}\n`);
    process.stdout.write(`live e2e complete in ${seconds}s\n`);
  } finally {
    if (browser) await browser.close();
    await stopDriver(driver);
  }
}

main().catch(async (error) => {
  if (error && error.skip) {
    process.stdout.write(`live e2e SKIP: ${error.message}\n`);
    process.exit(3);
  }
  process.stderr.write(`${error.stack || error.message}\n`);
  process.exit(1);
});
