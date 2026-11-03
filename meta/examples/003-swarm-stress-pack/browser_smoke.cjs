#!/usr/bin/env node

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");

const MOBS = ["atlas", "borealis", "cygnus", "draco"];
const BASE_PER_MOB = 75;
const BASE_TOTAL = 300;
const BURST_PARENTS_PER_MOB = 3;
const SUBAGENTS_PER_PARENT = 20;
const BURST_PER_MOB = 60;
const BURST_TOTAL = 240;
const TOTAL = BASE_TOTAL + BURST_TOTAL;
const BASE_PEERS_PER_AGENT = 150;
const PEER_FANOUT_PER_PARENT = BASE_PEERS_PER_AGENT;
const SPAWN_CONCURRENCY = Number(
  process.env.META_SWARM_SPAWN_CONCURRENCY || 240,
);
const RECOVERY_CONCURRENCY = Number(
  process.env.META_SWARM_RECOVERY_CONCURRENCY || 32,
);
const SEND_CONCURRENCY = 32;

const TITLES = {
  atlas: "Atlas Load Mob",
  borealis: "Borealis Memory Mob",
  cygnus: "Cygnus Comms Mob",
  draco: "Draco Recovery Mob",
};

function pad(value) {
  return String(value).padStart(3, "0");
}

function roleFor(index) {
  return ["swarm_worker", "swarm_probe", "swarm_auditor"][index % 3];
}

function baseAgents() {
  return MOBS.flatMap((mob) =>
    Array.from({ length: BASE_PER_MOB }, (_, index) => {
      const ordinal = index + 1;
      const shard = `${mob}-${String(((ordinal - 1) % 12) + 1).padStart(2, "0")}`;
      return {
        identity: `${mob}-base-${pad(ordinal)}`,
        role: ordinal === 1 ? "swarm_coordinator" : roleFor(index),
        displayName: `${TITLES[mob]} seat ${pad(ordinal)}`,
        mob,
        title: TITLES[mob],
        shard,
        ordinal,
      };
    }),
  );
}

function burstParents() {
  const parentOrdinals = Array.from(
    { length: BURST_PARENTS_PER_MOB },
    (_, index) => index * 25 + 1,
  );
  return baseAgents().filter((agent) => parentOrdinals.includes(agent.ordinal));
}

function burstAgents() {
  const parentsByMob = new Map();
  for (const parent of burstParents()) {
    const current = parentsByMob.get(parent.mob) || [];
    current.push(parent);
    parentsByMob.set(parent.mob, current);
  }
  return MOBS.flatMap((mob) =>
    (parentsByMob.get(mob) || []).flatMap((parent, parentIndex) =>
      Array.from({ length: SUBAGENTS_PER_PARENT }, (_, childIndex) => {
        const ordinal = parentIndex * SUBAGENTS_PER_PARENT + childIndex + 1;
        const shard = `${mob}-${String(((ordinal - 1) % 12) + 1).padStart(2, "0")}`;
        return {
          identity: `${mob}-burst-${pad(ordinal)}`,
          role: roleFor(ordinal - 1),
          displayName: `${TITLES[mob]} sub-agent ${pad(ordinal)}`,
          mob,
          title: TITLES[mob],
          shard,
          ordinal,
          parentIdentity: parent.identity,
          parentDisplayName: parent.displayName,
        };
      }),
    ),
  );
}

function groupBurstAgentsByParent() {
  const groups = new Map();
  for (const agent of burstAgents()) {
    const current = groups.get(agent.parentIdentity) || [];
    current.push(agent);
    groups.set(agent.parentIdentity, current);
  }
  return groups;
}

function basePeers(identity) {
  const base = baseAgents();
  const index = base.findIndex((agent) => agent.identity === identity);
  assert.notEqual(index, -1, `unknown base identity ${identity}`);
  const peers = new Set();
  const halfDegree = BASE_PEERS_PER_AGENT / 2;
  for (let offset = 1; offset <= halfDegree; offset += 1) {
    peers.add(base[(index + offset) % base.length].identity);
    peers.add(base[(index - offset + base.length) % base.length].identity);
  }
  return [...peers].sort();
}

function logicalIdentityFromRow(row) {
  const labels = row.labels || {};
  const mob = labels.swarm_mob;
  const wave = labels.wave;
  const ordinal = labels.ordinal;
  if (!mob || !wave || !ordinal) return "";
  return `${mob}-${wave}-${pad(Number(ordinal))}`;
}

function indexRows(rows) {
  const map = new Map();
  for (const row of rows) {
    const actual = String(row.identity || row.member_id || "");
    if (actual) map.set(actual, row);
    const logical = logicalIdentityFromRow(row);
    if (logical) map.set(logical, row);
  }
  return map;
}

function rowRuntimeIdentity(row, fallback) {
  return String(row.identity || row.member_id || fallback);
}

function sleep(ms) {
  return new Promise((resolve) => setTimeout(resolve, ms));
}

async function fetchJson(baseUrl, route, options) {
  const response = await fetch(new URL(route, baseUrl), options);
  const text = await response.text();
  let body = null;
  if (text.trim()) body = JSON.parse(text);
  assert.equal(
    response.status,
    200,
    `${route} should return HTTP 200: ${text}`,
  );
  return body;
}

async function rpc(baseUrl, method, params) {
  const body = await fetchJson(baseUrl, "/console/rpc", {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: `${method}:${Date.now()}:${Math.random()}`,
      method,
      params,
    }),
  });
  if (body.error) {
    throw new Error(`${method} failed: ${JSON.stringify(body.error)}`);
  }
  return body.result;
}

async function rpcWithRetry(baseUrl, method, params, attempts = 5) {
  let lastError;
  for (let attempt = 1; attempt <= attempts; attempt += 1) {
    try {
      return await rpc(baseUrl, method, params);
    } catch (error) {
      lastError = error;
      const message = String(error && error.message ? error.message : error);
      if (
        !message.includes("actor reply dropped") &&
        !message.includes("actor task dropped") &&
        !message.includes("-32000")
      ) {
        throw error;
      }
      const delayMs = 150 * attempt + Math.floor(Math.random() * 150);
      await new Promise((resolve) => setTimeout(resolve, delayMs));
    }
  }
  throw lastError;
}

async function runBounded(items, concurrency, fn) {
  let next = 0;
  await Promise.all(
    Array.from({ length: Math.min(concurrency, items.length) }, async () => {
      while (next < items.length) {
        const index = next;
        next += 1;
        await fn(items[index], index);
      }
    }),
  );
}

function ensureParams(agent) {
  return {
    role: agent.role,
    agent_identity: agent.identity,
    labels: {
      display_name: agent.displayName,
      role: agent.role,
      console_group: agent.title,
      group: agent.title,
      swarm_mob: agent.mob,
      lane: `${agent.mob}-stress`,
      shard: agent.shard,
      wave: "burst",
      ordinal: String(agent.ordinal),
      model: "gemini-3.1-flash-lite-preview",
      burst_cohort: `${agent.mob}-fanout`,
      parent_identity: agent.parentIdentity,
      parent_display_name: agent.parentDisplayName,
    },
    context: {
      identity: agent.identity,
      swarmMob: agent.mob,
      shard: agent.shard,
      wave: "burst",
      stressScenario: "example-003-playwright",
      parentIdentity: agent.parentIdentity,
    },
    additional_instructions: [
      `You were spawned by parent ${agent.parentIdentity} during the Playwright stress action in ${agent.title}.`,
      "Return a compact status message to your parent when prompted.",
      "Your creation should appear in roster and console timeline without app-side history refresh.",
    ],
  };
}

async function waitForCount(baseUrl, expected, timeoutMs = 120000) {
  const started = Date.now();
  let last = 0;
  while (Date.now() - started < timeoutMs) {
    const body = await fetchJson(baseUrl, "/console/identities");
    const rows = body.identities || body.rows || [];
    last = rows.length;
    if (last >= expected) return rows;
    await new Promise((resolve) => setTimeout(resolve, 750));
  }
  throw new Error(
    `timed out waiting for ${expected} identities; last count ${last}`,
  );
}

async function waitForTimeline(baseUrl, identity, needle, timeoutMs = 60000) {
  const started = Date.now();
  while (Date.now() - started < timeoutMs) {
    const page = await fetchJson(
      baseUrl,
      `/console/timeline?identity=${encodeURIComponent(identity)}&limit=80`,
    );
    const text = JSON.stringify(page);
    if (text.includes(needle)) return page;
    await new Promise((resolve) => setTimeout(resolve, 750));
  }
  throw new Error(
    `timed out waiting for timeline frame ${needle} on ${identity}`,
  );
}

async function waitForRealReply(
  baseUrl,
  identity,
  expectedBits,
  timeoutMs = 180000,
) {
  const started = Date.now();
  let last = "";
  while (Date.now() - started < timeoutMs) {
    const page = await fetchJson(
      baseUrl,
      `/console/timeline?identity=${encodeURIComponent(identity)}&limit=120`,
    );
    last = JSON.stringify(page);
    const lower = last.toLowerCase();
    if (
      lower.includes("interaction_complete") &&
      expectedBits.every((bit) => lower.includes(String(bit).toLowerCase())) &&
      !lower.includes('"content":"ok"') &&
      !lower.includes('"text":"ok"')
    ) {
      return page;
    }
    await new Promise((resolve) => setTimeout(resolve, 1000));
  }
  throw new Error(
    `timed out waiting for real non-ok reply on ${identity}; last timeline: ${last.slice(0, 2000)}`,
  );
}

async function expectVisible(locator, label, timeout = 30000) {
  await locator.first().waitFor({ state: "visible", timeout });
  assert.ok(await locator.first().isVisible(), `${label} should be visible`);
}

async function screenshot(page, artifactDir, index, label) {
  if (!artifactDir) return index;
  fs.mkdirSync(artifactDir, { recursive: true });
  const safe = label.replace(/[^A-Za-z0-9._-]+/g, "-");
  await page.screenshot({
    path: path.join(
      artifactDir,
      `${String(index + 1).padStart(2, "0")}-${safe}.png`,
    ),
    fullPage: true,
  });
  return index + 1;
}

async function main() {
  const baseUrl = process.argv[2];
  assert.ok(baseUrl, "Usage: browser_smoke.cjs <base-url>");
  const artifactDir = process.env.META_BROWSER_SMOKE_ARTIFACT_DIR || "";
  let shot = 0;

  const experience = await fetchJson(baseUrl, "/console/experience");
  assert.equal(experience.console_config.title, "Swarm Stress");
  assert.equal(
    experience.console_config.environment.label,
    "example-003 / 300+240 real-agent fan-out",
  );

  const baselineRows = await waitForCount(baseUrl, BASE_TOTAL);
  let rowByIdentity = indexRows(baselineRows);
  const burst = burstAgents();
  const parents = burstParents();
  const groups = groupBurstAgentsByParent();
  const started = Date.now();
  const failed = [];
  await Promise.all(
    parents.map(async (parent, parentIndex) => {
      const parentRow = rowByIdentity.get(parent.identity);
      assert.ok(parentRow, `missing parent row for ${parent.identity}`);
      await sleep(parentIndex * 300);
      // Console RPC plane has no meta/send_message (gateway-only method);
      // use meta/console/send, the console-plane equivalent.
      await rpcWithRetry(baseUrl, "meta/console/send", {
        identity: rowRuntimeIdentity(parentRow, parent.identity),
        content: `stress action ${parentIndex + 1}: spawn ${(groups.get(parent.identity) || []).length} sub-agents over a burst window, collect their returns, then fan out to ${PEER_FANOUT_PER_PARENT} wired peers.`,
        origin: "example-003-playwright",
        idempotency_key: `example-003:burst-kickoff:${parent.identity}`,
        handling_mode: "queue",
      });
      await runBounded(
        groups.get(parent.identity) || [],
        SUBAGENTS_PER_PARENT,
        async (agent) => {
          try {
            await rpc(baseUrl, "meta/ensure_member", ensureParams(agent));
          } catch (error) {
            failed.push({ agent, error });
          }
        },
      );
    }),
  );
  if (failed.length > 0) {
    console.log(`browser-smoke:initial-spawn-errors:${failed.length}`);
    await runBounded(failed, RECOVERY_CONCURRENCY, async ({ agent }) => {
      await rpcWithRetry(
        baseUrl,
        "meta/ensure_member",
        ensureParams(agent),
        8,
      );
    });
  }
  console.log(
    `browser-smoke:spawned:${BURST_TOTAL}:ms=${Date.now() - started}`,
  );

  const rows = await waitForCount(baseUrl, TOTAL);
  assert.equal(
    rows.filter((row) => String(row.identity || "").includes("-burst-"))
      .length >= BURST_TOTAL,
    true,
  );
  rowByIdentity = indexRows(rows);

  await runBounded(burst, SEND_CONCURRENCY, async (agent, index) => {
    const row = rowByIdentity.get(agent.identity);
    assert.ok(row, `missing sub-agent row for ${agent.identity}`);
    await rpcWithRetry(baseUrl, "meta/console/send", {
      identity: rowRuntimeIdentity(row, agent.identity),
      content: `sub-agent return ${index + 1}/${BURST_TOTAL}: return a compact status for parent=${agent.parentIdentity}; include identity=${agent.identity}, shard=${agent.shard}, and wave=burst.`,
      origin: "example-003-playwright",
      idempotency_key: `example-003:sub-return:${agent.identity}`,
      handling_mode: "queue",
    });
  });
  await runBounded(parents, SEND_CONCURRENCY, async (parent, parentIndex) => {
    const children = groups.get(parent.identity) || [];
    const parentRow = rowByIdentity.get(parent.identity);
    assert.ok(parentRow, `missing parent row for ${parent.identity}`);
    await rpcWithRetry(baseUrl, "meta/console/send", {
      identity: rowRuntimeIdentity(parentRow, parent.identity),
      content: `sub-agent returns received for stress action ${parentIndex + 1}: ${children
        .map((child) => `${child.identity}/${child.shard}`)
        .join(", ")}. Publish a compact fanout packet to your wired peers.`,
      origin: "example-003-playwright",
      idempotency_key: `example-003:parent-collect:${parent.identity}`,
      handling_mode: "queue",
    });
  });
  await Promise.all(
    parents.map(async (parent, parentIndex) => {
      const peers = basePeers(parent.identity).slice(0, PEER_FANOUT_PER_PARENT);
      await runBounded(
        peers,
        SEND_CONCURRENCY,
        async (peerIdentity, peerIndex) => {
          const peerRow = rowByIdentity.get(peerIdentity);
          assert.ok(peerRow, `missing peer row for ${peerIdentity}`);
          await rpcWithRetry(baseUrl, "meta/console/send", {
            identity: rowRuntimeIdentity(peerRow, peerIdentity),
            content: `peer fanout from ${parent.identity} action ${parentIndex + 1}: acknowledge parent=${parent.identity}, peer_index=${peerIndex + 1}/${peers.length}, burst_children=${(groups.get(parent.identity) || []).length}.`,
            origin: "example-003-playwright",
            idempotency_key: `example-003:peer-fanout:${parent.identity}:${peerIdentity}`,
            handling_mode: "queue",
          });
        },
      );
    }),
  );

  const sample = burst.filter((_, index) => index % 12 === 0).slice(0, 24);
  await runBounded(
    sample.slice(0, 12),
    SEND_CONCURRENCY,
    async (agent, index) => {
      const row = rowByIdentity.get(agent.identity);
      assert.ok(row, `missing identity row for ${agent.identity}`);
      await rpcWithRetry(baseUrl, "meta/console/send", {
        identity: rowRuntimeIdentity(row, agent.identity),
        content: `direct SDK-style probe ${index + 1}: answer with mob=${agent.mob}, shard=${agent.shard}, wave=burst, and one readiness observation.`,
        origin: "example-003-playwright",
        idempotency_key: `example-003:direct-probe:${agent.identity}`,
        handling_mode: "queue",
      });
    },
  );
  await runBounded(sample.slice(12), SEND_CONCURRENCY, async (agent, index) => {
    const row = rowByIdentity.get(agent.identity);
    assert.ok(row, `missing identity row for ${agent.identity}`);
    await rpcWithRetry(baseUrl, "meta/console/send", {
      identity: rowRuntimeIdentity(row, agent.identity),
      content: `playwright console-send probe ${index + 1}: answer with mob=${agent.mob}, shard=${agent.shard}, wave=burst, and one console observation.`,
      origin: "example-003-playwright",
      idempotency_key: `example-003:${agent.identity}:${index}`,
      handling_mode: "queue",
    });
  });
  const directSampleRow = rowByIdentity.get(sample[0].identity);
  const consoleSampleRow = rowByIdentity.get(sample[12].identity);
  assert.ok(directSampleRow, `missing direct sample row for ${sample[0].identity}`);
  assert.ok(consoleSampleRow, `missing console sample row for ${sample[12].identity}`);
  await waitForTimeline(
    baseUrl,
    rowRuntimeIdentity(directSampleRow, sample[0].identity),
    "direct SDK-style probe",
  );
  await waitForTimeline(
    baseUrl,
    rowRuntimeIdentity(consoleSampleRow, sample[12].identity),
    "playwright console-send probe",
  );
  await waitForRealReply(
    baseUrl,
    rowRuntimeIdentity(directSampleRow, sample[0].identity),
    [sample[0].mob, sample[0].shard, "burst"],
  );
  const parentSampleRow = rowByIdentity.get(parents[0].identity);
  const peerSampleRow = rowByIdentity.get(basePeers(parents[0].identity)[0]);
  assert.ok(parentSampleRow, `missing parent sample row for ${parents[0].identity}`);
  assert.ok(peerSampleRow, `missing peer sample row for ${basePeers(parents[0].identity)[0]}`);
  await waitForTimeline(
    baseUrl,
    rowRuntimeIdentity(parentSampleRow, parents[0].identity),
    "sub-agent returns received",
  );
  await waitForTimeline(
    baseUrl,
    rowRuntimeIdentity(peerSampleRow, basePeers(parents[0].identity)[0]),
    `peer fanout from ${parents[0].identity}`,
  );

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({
    viewport: { width: 1600, height: 1000 },
  });
  try {
    await page.goto(new URL("/console", baseUrl).toString(), {
      waitUntil: "domcontentloaded",
    });
    await expectVisible(page.getByTestId("meerkat-console"), "console root");
    await expectVisible(
      page.getByTestId("meta-topbar").getByText("Meta Load Lab"),
      "brand",
    );
    shot = await screenshot(
      page,
      artifactDir,
      shot,
      "swarm-console-after-burst",
    );

    await page.getByTestId("nav:topology").click();
    await expectVisible(page.getByTestId("topology-panel"), "topology panel");
    await expectVisible(page.getByTestId("topology-dense-map"), "dense topology map");
    await expectVisible(page.getByText(/540 agents/), "topology agent count");
    shot = await screenshot(page, artifactDir, shot, "dense-topology-map");

    await page.getByTestId("sidebar-search").fill(sample[0].displayName);
    await expectVisible(
      page
        .locator('[data-testid^="sidebar-agent:"]')
        .filter({ hasText: sample[0].displayName }),
      "dynamic burst agent",
    );
    await page
      .locator('[data-testid^="sidebar-agent:"]')
      .filter({ hasText: sample[0].displayName })
      .first()
      .click();
    await expectVisible(
      page
        .locator('[data-testid^="chat-pane:"]')
        .filter({ hasText: sample[0].displayName }),
      "burst chat pane",
    );
    shot = await screenshot(page, artifactDir, shot, "dynamic-agent-chat");

    await page.getByTestId("nav:timeline").click();
    await expectVisible(page.getByTestId("timeline-panel"), "timeline panel");
    shot = await screenshot(page, artifactDir, shot, "timeline-panel");

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
