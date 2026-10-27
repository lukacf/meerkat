#!/usr/bin/env node

const assert = require("node:assert/strict");
const fs = require("node:fs");
const path = require("node:path");
const { chromium } = require("playwright");

async function rpc(baseUrl, method, params = {}) {
  const response = await fetch(new URL("/console/rpc", baseUrl), {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify({
      jsonrpc: "2.0",
      id: `topology-smoke-${Date.now()}-${Math.random().toString(36).slice(2)}`,
      method,
      params,
    }),
  });
  assert.ok(response.ok, `${method} HTTP failure: ${response.status}`);
  const body = await response.json();
  assert.equal(body.error, undefined, `${method} RPC failure: ${JSON.stringify(body.error)}`);
  return body.result;
}

function topologyEdge(snapshot, left, right) {
  return (snapshot.edges || []).find(({ edge }) => {
    const ids = [edge?.a?.identity, edge?.b?.identity].sort();
    return ids[0] === [left, right].sort()[0] && ids[1] === [left, right].sort()[1];
  });
}

function stockEndpointId(snapshot, identity) {
  const node = (snapshot.nodes || []).find((candidate) =>
    candidate?.endpoint?.identity === identity
  );
  assert.ok(node, `missing topology node ${identity}`);
  const authority = node.endpoint.authority || snapshot.authority || "";
  return `mk1|${encodeURIComponent(authority)}|${encodeURIComponent(identity)}`;
}

async function waitForTopologyEdge(baseUrl, left, right, predicate, message, timeout = 30000) {
  const deadline = Date.now() + timeout;
  while (Date.now() < deadline) {
    const snapshot = await rpc(baseUrl, "meta/topology/query");
    const edge = topologyEdge(snapshot, left, right);
    if (predicate(edge, snapshot)) return { edge, snapshot };
    await new Promise((resolve) => setTimeout(resolve, 150));
  }
  assert.fail(message);
}

async function clickNav(page, id) {
  const current = page.getByTestId(`nav:${id}`);
  if (await current.count()) {
    await current.click();
    return;
  }
  await page.getByTestId(`sidebar-action:open_${id}`).click();
}

async function selectConnectionSource(page, identity) {
  const row = page.getByTestId(`connection-picker-row:${identity}`);
  await row.waitFor({ state: "visible", timeout: 10000 });
  await row.locator(".topo-edit__identity").click();
  await page.getByTestId(`connection-picker-source:${identity}`).waitFor({
    state: "visible",
    timeout: 10000,
  });
}

async function clickTopologyAction(page, pattern) {
  const button = page.getByRole("button", { name: pattern });
  await button.waitFor({ state: "visible", timeout: 10000 });
  assert.equal(await button.isDisabled(), false, `expected ${pattern} to be enabled`);
  await button.click();
}

async function main() {
  const baseUrl = process.argv[2];
  const phase = process.argv[3] || "--all";
  const artifactDir = process.env.INCIDENT_TOPOLOGY_ARTIFACT_DIR;
  assert.ok(baseUrl, "baseUrl is required");
  assert.ok(["--all", "--prepare", "--resume"].includes(phase), `unknown phase ${phase}`);

  const capabilities = await rpc(baseUrl, "meta/capabilities");
  for (const method of [
    "meta/topology/query",
    "meta/topology/plan",
    "meta/topology/apply",
    "meta/topology/operation/get",
    "meta/topology/audit/query",
  ]) {
    assert.ok(capabilities.methods.includes(method), `missing topology capability ${method}`);
  }
  assert.equal(capabilities.topology_control?.mode, "editable");
  assert.equal(capabilities.topology_control?.can_bulk, false);
  const initialTopology = await rpc(baseUrl, "meta/topology/query");
  const commanderEndpointId = stockEndpointId(initialTopology, "incident-commander");

  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  try {
    await page.goto(`${baseUrl}/console`, { waitUntil: "domcontentloaded" });
    await page.getByTestId("meerkat-console").waitFor({ state: "visible", timeout: 30000 });
    await clickNav(page, "topology");
    const panel = page.getByTestId("topology-panel");
    await panel.waitFor({ state: "visible", timeout: 30000 });
    await page.getByTestId("topology-view:connections").waitFor({ state: "visible", timeout: 30000 });
    assert.equal(await panel.getByText(/connect all/i).count(), 0, "stock console must not expose Connect all");
    await page.getByTestId("topology-view:connections").click();
    await page.getByTestId("connection-picker").waitFor({ state: "visible", timeout: 10000 });

    if (phase !== "--resume") {
      // An absent, undeclared pair proves explicit pairwise connect.
      await selectConnectionSource(page, commanderEndpointId);
      await clickTopologyAction(page, /Connect API Investigator to Incident Commander/i);
      await waitForTopologyEdge(
        baseUrl,
        "incident-commander",
        "api-investigator",
        (edge) => edge?.actual === true && edge?.operator_added === true,
        "pairwise connect did not become authoritative",
      );

      // A declared edge proves de-peer is a durable suppression, not merely a
      // one-shot physical unwire that discovery immediately heals.
      await clickTopologyAction(page, /Disconnect Payments SRE from Incident Commander/i);
      await waitForTopologyEdge(
        baseUrl,
        "incident-commander",
        "payments-sre",
        (edge) => edge?.actual === false && edge?.suppressed === true,
        "declared edge was not suppressed",
      );
      await rpc(baseUrl, "meta/reconcile_edges");
      await waitForTopologyEdge(
        baseUrl,
        "incident-commander",
        "payments-sre",
        (edge) => edge?.actual === false && edge?.suppressed === true,
        "reconcile healed an operator-suppressed declared edge",
      );
    }

    if (phase !== "--prepare") {
      // In --resume this assertion runs against a fresh runtime process using
      // the same state directory, proving both additions and suppressions
      // survived restart.
      await waitForTopologyEdge(
        baseUrl,
        "incident-commander",
        "api-investigator",
        (edge) => edge?.actual === true
          && edge?.operator_added === true
          && edge?.desired === true,
        "operator-added connection did not survive runtime restart",
      );
      await waitForTopologyEdge(
        baseUrl,
        "incident-commander",
        "payments-sre",
        (edge) => edge?.actual === false && edge?.suppressed === true,
        "operator suppression did not survive runtime restart",
      );
      // Refresh the console's authoritative query and use the distinct
      // reconnect action, which has its own bilateral permission gate.
      await page.reload({ waitUntil: "domcontentloaded" });
      await page.getByTestId("meerkat-console").waitFor({ state: "visible", timeout: 30000 });
      await clickNav(page, "topology");
      await page.getByTestId("topology-view:connections").waitFor({ state: "visible", timeout: 30000 });
      await page.getByTestId("topology-view:connections").click();
      await selectConnectionSource(page, commanderEndpointId);
      await clickTopologyAction(page, /Reconnect Payments SRE to Incident Commander/i);
      await waitForTopologyEdge(
        baseUrl,
        "incident-commander",
        "payments-sre",
        (edge) => edge?.actual === true && edge?.suppressed === false,
        "reconnect did not restore the declared edge",
      );
    }

    const audit = await rpc(baseUrl, "meta/topology/audit/query", {
      after_seq: 0,
      limit: 20,
    });
    const minimumRecords = phase === "--prepare" ? 2 : 3;
    assert.ok(
      audit.records.length >= minimumRecords,
      `expected at least ${minimumRecords} durable topology attempts: ${JSON.stringify(audit)}`,
    );
    assert.ok(
      audit.records.every((record, index, records) =>
        typeof record.seq === "number"
          && record.seq > 0
          && (index === 0 || records[index - 1].seq < record.seq)
          && typeof record.actor === "string"
          && record.actor.length > 0
      ),
      `topology audit must be ordered and attributed: ${JSON.stringify(audit)}`,
    );
    const exhausted = await rpc(baseUrl, "meta/topology/audit/query", {
      after_seq: audit.next_after_seq,
      limit: 20,
    });
    assert.equal(exhausted.records.length, 0, "audit cursor must not duplicate records");

    if (artifactDir) {
      fs.mkdirSync(artifactDir, { recursive: true });
      await page.screenshot({
        path: path.join(artifactDir, `incident-topology-${phase.slice(2)}.png`),
        fullPage: true,
      });
    }
  } finally {
    await browser.close();
  }

  console.log(`incident topology browser smoke passed (${phase.slice(2)})`);
}

main().catch((error) => {
  console.error(error.stack || error.message);
  process.exit(1);
});
