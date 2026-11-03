"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { chromium } = require("playwright");
const { startFixture, eventually, rpc } = require("../acceptance-runtime.cjs");
const { browserFailureMonitor, navigation } = require("./browser-failure-monitor.cjs");
const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");

function requirePrebuilt() {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Set META_EXAMPLE_BIN_DIR to the coordinator's prebuilt fixture directory.");
}
async function pending(fixture) {
  const response = await rpc(fixture.baseUrl, "meta/gating/pending");
  assert.equal(response.status, 200, JSON.stringify(response));
  assert(Array.isArray(response.body.result?.pending), JSON.stringify(response));
  return response.body.result.pending;
}
async function audit(fixture) {
  const response = await rpc(fixture.baseUrl, "meta/gating/audit", { limit: 100 });
  assert(Array.isArray(response.body.result?.entries), JSON.stringify(response));
  return response.body.result.entries;
}
const decide = (fixture, id, decision = "approve", approver = "fixture-operator") => rpc(fixture.baseUrl, "meta/gating/decide", {
  pending_id: id, approver_id: approver, decision, reason: "Reviewed the complete owner request",
});
function accessDenied(response) {
  assert.equal(response.body.error?.code, -32030, JSON.stringify(response));
}
function decisionResult(response, id, decision) {
  const result = response.body.result;
  assert.equal(result?.pending_id, id, JSON.stringify(response));
  assert.equal(result.decision, decision);
  assert.equal(result.outcome, decision === "approve" ? "allowed" : decision === "reject" ? "safe_draft" : "pending_approval");
  return result;
}
function decisions(fixture, id) {
  return fixture.observations.filter(item => {
    if (item.method !== "POST" || !item.request) return false;
    try { const value = JSON.parse(item.request); return value.method === "meta/gating/decide" && value.params?.pending_id === id; }
    catch { return false; }
  });
}
async function writeEvidence(name, fixture, extra = {}) {
  await fs.mkdir(evidence, { recursive: true });
  await fs.writeFile(path.join(evidence, `${name}.json`), JSON.stringify({
    ...extra, observations: fixture.observations, logs: fixture.logs(),
    requests: await (await fetch(`${fixture.backendUrl}/__fixture/requests`)).json(),
  }, null, 2));
}
async function capture(page, name) {
  await fs.mkdir(evidence, { recursive: true });
  await page.screenshot({ path: path.join(evidence, `${name}.png`), fullPage: true });
}
async function withApi(name, run) {
  requirePrebuilt();
  const fixture = await startFixture();
  try { await run(fixture); await writeEvidence(name, fixture); }
  catch (error) { await writeEvidence(`${name}-failure`, fixture, { error: String(error) }).catch(() => {}); throw error; }
  finally { await fixture.close(); }
}
async function withBrowser(name, run) {
  requirePrebuilt();
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  page.setDefaultTimeout(20_000);
  const monitor = browserFailureMonitor(page.context(), { origin: fixture.baseUrl });
  const { errors, expected: expectedFailures, failures: requestFailures } = monitor;
  try {
    await run({ fixture, page });
    monitor.assertClean();
    await writeEvidence(name, fixture, { errors, expectedFailures, requestFailures });
    monitor.assertClean();
  } catch (error) {
    await capture(page, `${name}-failure`).catch(() => {});
    await writeEvidence(`${name}-failure`, fixture, { error: String(error), errors, expectedFailures, requestFailures }).catch(() => {});
    throw error;
  } finally { monitor.stop(); await browser.close(); await fixture.close(); }
}

async function ownerRestart() {
  requirePrebuilt();
  const stateDir = await fs.mkdtemp(path.join(os.tmpdir(), "meta-approval-owner-"));
  let fixture = await startFixture({ stateDir });
  try {
    const origin = { identity: "router:main", interaction_id: "review-interaction", conversation_id: "release-review" };
    const created = await fixture.control("approval", { ...origin, action: "Publish the reviewed workgraph report" });
    assert(created.pending_id, JSON.stringify(created));
    const first = await rpc(fixture.baseUrl, "meta/gating/pending");
    assert.deepEqual(first.body.result.pending[0].origin, origin);
    await fixture.close(); fixture = await startFixture({ stateDir });
    const restored = await rpc(fixture.baseUrl, "meta/gating/pending");
    assert.deepEqual(restored.body.result.pending[0], first.body.result.pending[0]);
    const escalated = await rpc(fixture.baseUrl, "meta/gating/decide", {
      pending_id: created.pending_id, approver_id: "fixture-operator", decision: "escalate", reason: "Second reviewer required",
    });
    assert(escalated.body.result.next_pending_id, JSON.stringify(escalated.body));
    await fixture.close(); fixture = await startFixture({ stateDir });
    const successor = (await rpc(fixture.baseUrl, "meta/gating/pending")).body.result.pending[0];
    assert.equal(successor.pending_id, escalated.body.result.next_pending_id);
    assert.deepEqual(successor.origin, origin);
    const decided = await rpc(fixture.baseUrl, "meta/gating/decide", {
      pending_id: successor.pending_id, approver_id: "fixture-operator", decision: "approve", reason: "Reviewed complete request",
    });
    assert.equal(decided.body.result.outcome, "allowed", JSON.stringify(decided.body));
    await fixture.close(); fixture = await startFixture({ stateDir });
    assert.deepEqual((await rpc(fixture.baseUrl, "meta/gating/pending")).body.result.pending, []);
    const audit = (await rpc(fixture.baseUrl, "meta/gating/audit", { limit: 100 })).body.result.entries;
    assert(audit.some(item => item.detail?.origin?.interaction_id === origin.interaction_id));
    const next = await fixture.control("approval", { ...origin, action: "Review next report" });
    assert.notEqual(next.pending_id, successor.pending_id);
    assert.notEqual(next.pending_id, created.pending_id);
    await fs.mkdir(evidence, { recursive: true });
    await fs.writeFile(path.join(evidence, "approval-owner-restart.json"), JSON.stringify({ origin, created, successor, audit, next }, null, 2));
  } finally { await fixture.close(); await fs.rm(stateDir, { recursive: true, force: true }); }
}

async function lateApprovalGeometry(host) {
  requirePrebuilt();
  const geometry = require("./real-conversation.cjs").geometry;
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  const monitor = geometry.browserErrors(page);
  const result = { host, errors: monitor.errors, expectedFailures: monitor.expected };
  try {
    result.turns = await geometry.seedReadingHistory(fixture);
    await geometry.open(page, fixture, host, monitor);
    const viewport = geometry.transcript(page, host);
    const retained = viewport.locator("[data-conversation-row-id]").filter({ hasText: result.turns[3].instruction }).last();
    await retained.waitFor();
    const anchor = await geometry.anchorAt(viewport, retained, -10);
    const beforeHeight = await viewport.evaluate(node => node.scrollHeight);
    result.geometry = await geometry.measureAnchor(viewport, anchor, async () => {
      result.created = await fixture.control("approval", { identity: "router:main", interaction_id: result.turns[1].accepted.interaction_id,
        action: "Approve the earlier dependency review before publishing", timeout_ms: 300000 });
      assert(result.created.pending_id);
      const owner = (await pending(fixture)).find(item => item.pending_id === result.created.pending_id);
      assert.equal(owner.origin.interaction_id, result.turns[1].accepted.interaction_id);
      result.owner = owner;
      // Let the actual subscribed resource discover the owner request while the inbox stays closed.
      await viewport.getByTestId(`gating-pending:${result.created.pending_id}`).waitFor({ state: "attached", timeout: 25_000 });
      await geometry.settle(page);
      const relative = await viewport.evaluate((node, id) => {
        const card = node.querySelector(`[data-testid="gating-pending:${id}"]`);
        return { bottom: card.getBoundingClientRect().bottom - node.getBoundingClientRect().top, height: card.getBoundingClientRect().height, scrollHeight: node.scrollHeight };
      }, result.created.pending_id);
      assert(relative.height > 100, "real inline approval occupies layout space");
      assert(relative.bottom < 0, "late correlated approval is inserted above the retained reading viewport");
      assert(relative.scrollHeight > beforeHeight + 100, "late card grows the real transcript above the reader");
      result.card = relative;
    }, `${host} late correlated approval preserves reading position`);
    await capture(page, `${host}-late-approval-reading-1600`);
    assert.equal(decisions(fixture, result.created.pending_id).length, 0, "merely receiving a late approval never decides it");
    // A pending approval blocks the agent, so windowing must never hide it.
    // From the unchanged reading position the reader is told it is waiting
    // and can open it, and its card stays in the transcript (the windowed
    // stock transcript keeps that turn mounted) without scrolling to it.
    const needsYou = host === "shared" ? /^Needs you \(1\)$/ : /^Needs you, 1 pending approval$/;
    await page.getByRole("button", { name: needsYou }).first().waitFor();
    assert.equal(await viewport.getByTestId(`gating-pending:${result.created.pending_id}`)
      .getByRole("button", { name: "Approve", exact: true }).count(), 1, "the late approval's card is actionable where it belongs");
    // Explicit navigation is allowed to move the reader so the actual card can be inspected.
    const card = viewport.getByTestId(`gating-pending:${result.created.pending_id}`);
    await card.scrollIntoViewIfNeeded(); await geometry.settle(page);
    await card.getByRole("button", { name: "Approve", exact: true }).waitFor();
    await capture(page, `${host}-late-approval-review-1600`);
    assert.deepEqual(monitor.errors, []);
  } catch (error) {
    result.failure = error.stack || String(error); await capture(page, `${host}-late-approval-geometry-failure`).catch(() => {}); throw error;
  } finally {
    result.html = result.failure ? await page.content() : undefined;
    await writeEvidence(`${host}-late-approval-geometry`, fixture, result);
    await browser.close(); await fixture.close();
  }
}

async function composedApproval(host) {
  await withBrowser(`real-${host}-approvals`, async ({ fixture, page }) => {
    const accepted = await openConversation(page, fixture, host);
    const created = await fixture.control("approval", { identity: "router:main", interaction_id: accepted.interaction_id, action: "Publish the reviewed workgraph report" });
    assert(created.pending_id, JSON.stringify(created));
    // The resource must discover this while the inbox is closed.
    await page.getByTestId(`gating-pending:${created.pending_id}`).first().waitFor({ timeout: 25_000 });
    if (host === "shared") {
      await page.getByRole("button", { name: "Toggle second pane" }).click();
      await page.getByRole("button", { name: /^Needs you/ }).click();
      assert.equal(await page.getByTestId(`gating-pending:${created.pending_id}`).count(), 3, "two panes and one inbox share the same request");
    } else {
      await page.getByTestId("approval-attention").getByRole("button", { name: /^Needs you/ }).click();
      await page.getByRole("heading", { name: /gating|approval/i }).first().waitFor();
    }
    await capture(page, `${host}-approval-review`);
    await page.setViewportSize({ width: 1440, height: 900 });
    if (host === "shared") {
      const panes = await page.locator(".acceptance-panes > section").evaluateAll(nodes => nodes.map(node => node.getBoundingClientRect().height));
      assert(panes.every(height => height >= 400), "approval inbox preserves usable split transcript space");
    }
    await capture(page, `${host}-approval-review-1440`);
    const before = decisions(fixture, created.pending_id).length;
    await page.getByTestId(`gating-action:${created.pending_id}:approve`).first().click();
    await eventually(async () => (await rpc(fixture.baseUrl, "meta/gating/pending")).body.result.pending.length === 0, "owner approval resolution");
    await eventually(async () => await page.getByTestId(`gating-action:${created.pending_id}:approve`).count() === 0, "all approval copies settle");
    await navigation(page, "explicit approval host reload", async () => {
      const response = await page.reload();
      await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
      return response;
    });
    assert.equal((await rpc(fixture.baseUrl, "meta/gating/pending")).body.result.pending.length, 0);
    assert.equal(decisions(fixture, created.pending_id).length, before + 1, "one UI decision dispatch settles all copies");
    await capture(page, `${host}-approval-settled`);
  });
}

async function openConversation(page, fixture, host) {
  await fixture.control("model", { source: "## Release review\n\nThe graph has two independent review branches. Both must complete before publishing the final report.\n\n- [x] Dependencies checked\n- [ ] Operator approval\n", delay_ms: 0, chunk_chars: 64 });
  const accepted = (await rpc(fixture.baseUrl, "meta/console/send", {
    identity: "router:main", content: "Prepare the workgraph release review", origin: "console:approval-scenario",
    origin_kind: "operator", idempotency_key: "approval-turn", handling_mode: "queue",
  })).body.result;
  assert(accepted?.interaction_id, JSON.stringify(accepted));
  await navigation(page, `open ${host} approval host`, async () => {
    const response = await page.goto(fixture.baseUrl + (host === "shared" ? "/shared" : "/scoped"));
    if (host === "stock" && !await page.getByTestId("chat-composer:router:main").count()) {
      await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: /router/i }).first().click();
    }
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    return response;
  });
  // Real operator ingress and its correlated transcript must exist. A receipt
  // alone cannot substitute for this prerequisite when bootstrap is broken.
  await eventually(async () => {
    const frames = await (await fetch(`${fixture.baseUrl}/console/timeline?identity=router%3Amain&mode=recent&limit=500`)).json();
    return frames.frames?.some(frame => frame.kind === "text_complete" && JSON.stringify(frame.payload).includes("two independent review branches"));
  }, "real approval conversation completion");
  return accepted;
}

async function refreshApprovals(page, host) {
  if (host === "shared") await page.getByRole("button", { name: /^Needs you/ }).click();
  else {
    const refresh = page.getByRole("button", { name: "Refresh approvals", exact: true });
    if (!await refresh.count()) await page.getByTestId("approval-attention").getByRole("button", { name: /^Needs you/ }).click();
    await page.getByRole("button", { name: "Refresh approvals", exact: true }).click();
  }
}

async function accessMatrix() {
  await withApi("api-approval-access", async fixture => {
    const publicRequest = await fixture.control("approval", { identity: "router:main", interaction_id: "public-review", action: "Public review" });
    const privateRequest = await fixture.control("approval", { identity: "domain:delivery", interaction_id: "private-review", conversation_id: "", action: "Private delivery review" });
    const originless = await fixture.control("approval", { action: "Global owner request" });
    assert.equal((await pending(fixture)).length, 3);

    await fixture.control("access", { mode: "read-only" });
    assert.equal((await pending(fixture)).length, 3, "read-only operators retain pending visibility");
    accessDenied(await decide(fixture, publicRequest.pending_id));
    assert.equal((await pending(fixture)).length, 3, "denied decisions do not consume pending owner entries");

    await fixture.control("access", { mode: "denied" });
    accessDenied(await rpc(fixture.baseUrl, "meta/gating/pending"));
    accessDenied(await rpc(fixture.baseUrl, "meta/gating/audit", { limit: 100 }));
    accessDenied(await decide(fixture, publicRequest.pending_id));

    await fixture.control("access", { mode: "hide-origin" });
    const visible = await pending(fixture);
    assert.deepEqual(visible.map(item => item.pending_id).sort(), [publicRequest.pending_id, originless.pending_id].sort());
    const visibleAudit = await audit(fixture);
    assert(!visibleAudit.some(item => item.pending_id === privateRequest.pending_id || item.detail?.origin?.identity === "domain:delivery"));
    for (const id of [privateRequest.pending_id, ` ${privateRequest.pending_id} `]) accessDenied(await decide(fixture, id));

    await fixture.control("access", { mode: "open" });
    assert.equal((await pending(fixture)).length, 3, "access filtering never deletes the hidden request");
    decisionResult(await decide(fixture, publicRequest.pending_id), publicRequest.pending_id, "approve");
    decisionResult(await decide(fixture, privateRequest.pending_id, "reject"), privateRequest.pending_id, "reject");
    decisionResult(await decide(fixture, originless.pending_id, "reject"), originless.pending_id, "reject");
    assert.deepEqual(await pending(fixture), []);
  });
}

async function ownerEdges() {
  await withApi("api-approval-owner-edges", async fixture => {
    const origin = { identity: "router:main", interaction_id: "owner-edges", conversation_id: "release-review" };
    const created = await fixture.control("approval", { ...origin, action: "Escalate the full review" });
    const original = (await pending(fixture))[0];
    const escalated = decisionResult(await decide(fixture, created.pending_id, "escalate"), created.pending_id, "escalate");
    assert(escalated.next_pending_id && escalated.next_pending_id !== created.pending_id);
    const successor = (await pending(fixture))[0];
    assert.equal(successor.pending_id, escalated.next_pending_id);
    assert.equal(successor.action_id, original.action_id);
    assert.equal(successor.deadline_at_ms, original.deadline_at_ms, "escalation does not extend owner expiry");
    assert.deepEqual(successor.origin, origin);
    assert((await decide(fixture, created.pending_id)).body.error, "the predecessor cannot be decided twice");

    // The fixture routes escalations to the fixture operator, so the successor's
    // eligible approver is that operator: the race is between two of its decisions.
    const competitors = await Promise.all([decide(fixture, successor.pending_id, "approve", "fixture-operator"), decide(fixture, successor.pending_id, "reject", "fixture-operator")]);
    assert.equal(competitors.filter(result => result.body.result).length, 1, "exactly one competing decision wins");
    assert.equal(competitors.filter(result => result.body.error).length, 1);
    assert.deepEqual(await pending(fixture), []);
    const settledAudit = await audit(fixture);
    assert.equal(settledAudit.filter(item => item.pending_id === successor.pending_id && ["approval_decided", "rejection_decided"].includes(item.event_type)).length, 1);

    const expiring = await fixture.control("approval", { ...origin, timeout_ms: 1000, action: "Review before the owner deadline" });
    assert((await pending(fixture)).some(item => item.pending_id === expiring.pending_id));
    await eventually(async () => !(await pending(fixture)).some(item => item.pending_id === expiring.pending_id), "owner expiry", 5000);
    assert((await decide(fixture, expiring.pending_id)).body.error, "expired request cannot be approved");
    const expiry = (await audit(fixture)).filter(item => item.pending_id === expiring.pending_id && item.event_type === "timeout_fallback");
    assert.equal(expiry.length, 1);
    assert.equal(expiry[0].outcome, "safe_draft");
    assert.deepEqual(expiry[0].detail.origin, origin);
  });
}

async function accessRevocation(host) {
  await withBrowser(`real-${host}-approval-revocation`, async ({ fixture, page }) => {
    const accepted = await openConversation(page, fixture, host);
    const created = await fixture.control("approval", { identity: "router:main", interaction_id: accepted.interaction_id, action: "Review access-sensitive publication" });
    await refreshApprovals(page, host);
    await page.getByTestId(`gating-pending:${created.pending_id}`).first().waitFor();
    if (host === "shared") {
      await page.getByRole("button", { name: "Toggle second pane" }).click();
      assert.equal(await page.getByTestId(`gating-pending:${created.pending_id}`).count(), 3);
    }
    await capture(page, `${host}-approval-before-revocation`);
    await fixture.control("access", { mode: "denied" });
    await refreshApprovals(page, host);
    await eventually(async () => await page.getByTestId(`gating-pending:${created.pending_id}`).count() === 0, "revoked content clears from every copy");
    await eventually(async () => await page.getByTestId("approval-attention").count() === 0,
      "permanently denied approvals do not leave a global Needs you control");
    assert.equal(decisions(fixture, created.pending_id).length, 0, "read revocation never submits a decision");
    if (host === "stock") {
      // The fixture removes gating from advertised capabilities before dispatch.
      // The explicit inbox must explain that state after Needs you disappears.
      await page.getByTestId("nav:gating").click();
      await page.getByText("Approvals are not available for this connection", { exact: true }).waitFor();
      const capabilityReads = fixture.observations.filter(item => {
        try { return JSON.parse(item.request || "{}").method === "meta/capabilities"; }
        catch { return false; }
      });
      const advertised = JSON.parse(capabilityReads.at(-1)?.response || "{}").result?.methods;
      assert(Array.isArray(advertised), "revocation captured the advertised capabilities");
      assert(!advertised.includes("meta/gating/pending"), "revocation removes the read capability before dispatch");
      assert.equal(await page.getByText("No pending approvals.", { exact: true }).count(), 0, "unsupported is not an empty success");
    }
    await capture(page, `${host}-approval-access-revoked`);
    await fixture.control("access", { mode: "open" });
    assert((await pending(fixture)).some(item => item.pending_id === created.pending_id));
    await navigation(page, "explicit approval host reload", async () => {
      const response = await page.reload();
      await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
      return response;
    });
    if (host === "stock") await refreshApprovals(page, host);
    else await page.getByRole("button", { name: /^Needs you/ }).click();
    await page.getByTestId(`gating-pending:${created.pending_id}`).first().waitFor();
  });
}

async function readOnlyApproval() {
  await withBrowser("real-stock-approval-read-only", async ({ fixture, page }) => {
    const accepted = await openConversation(page, fixture, "stock");
    const created = await fixture.control("approval", { identity: "router:main", interaction_id: accepted.interaction_id, action: "Read this complete approval request" });
    await fixture.control("access", { mode: "read-only" });
    await refreshApprovals(page, "stock");
    const card = page.getByTestId(`gating-pending:${created.pending_id}`).first();
    await card.waitFor();
    const button = card.getByRole("button", { name: "Approve", exact: true });
    // Discovery advertises available methods. The command gateway must refuse
    // locally and convert the visible resource to read-only before any write.
    if (await button.isEnabled()) await button.click();
    await eventually(async () => {
      const copies = page.getByTestId(`gating-action:${created.pending_id}:approve`);
      return await copies.count() > 0 && (await copies.evaluateAll(nodes => nodes.every(node => node.disabled)));
    }, "read-only approval controls");
    assert.equal(decisions(fixture, created.pending_id).length, 0, "unsupported decision capability stops before dispatch");
    assert((await pending(fixture)).some(item => item.pending_id === created.pending_id));
    await card.getByRole("status").filter({ hasText: /^Decision unavailable$/ }).waitFor();
    assert.equal(await card.getAttribute("data-state"), "unavailable");
    assert.equal(await page.getByText("Decision unconfirmed", { exact: true }).count(), 0,
      "a known refusal before dispatch cannot claim an uncertain decision");
    await card.getByText("Complete request details", { exact: true }).click();
    await card.getByText("Read-only access", { exact: true }).waitFor();
    await capture(page, "stock-approval-read-only");
    await page.setViewportSize({ width: 1440, height: 1000 });
    await page.getByTestId("theme-toggle").click();
    assert.equal(await page.getByTestId("meerkat-console").getAttribute("data-cc-theme"), "dark");
    await capture(page, "stock-approval-read-only-dark-1440");
    assert.equal(decisions(fixture, created.pending_id).length, 0, "inspecting the refused request never sends a decision");
  });
}

async function hiddenOriginAndCorrelation() {
  await withBrowser("real-shared-approval-hidden-origin", async ({ fixture, page }) => {
    const accepted = await openConversation(page, fixture, "shared");
    const matching = await fixture.control("approval", { identity: "router:main", interaction_id: accepted.interaction_id, action: "Approval for this exact release review" });
    const otherTurn = await fixture.control("approval", { identity: "router:main", interaction_id: "different-owner-interaction", action: "Approval for another router interaction" });
    const privateRequest = await fixture.control("approval", { identity: "domain:delivery", interaction_id: "private-interaction", conversation_id: "", action: "Private delivery review details" });
    const globalRequest = await fixture.control("approval", { action: "Unattributed global owner review" });
    await page.getByRole("button", { name: "Toggle second pane" }).click();
    await refreshApprovals(page, "shared");
    await eventually(async () => await page.getByTestId(`gating-pending:${matching.pending_id}`).count() === 3, "matching approval appears in both panes and inbox");
    for (const request of [otherTurn, privateRequest, globalRequest]) {
      assert.equal(await page.getByTestId(`gating-pending:${request.pending_id}`).count(), 1, "uncorrelated requests are inbox-only");
      assert.equal(await page.getByTestId("shared-pane-0").getByTestId(`gating-pending:${request.pending_id}`).count(), 0);
      assert.equal(await page.getByTestId("shared-pane-1").getByTestId(`gating-pending:${request.pending_id}`).count(), 0);
    }
    await capture(page, "shared-approval-origin-correlation");
    const geometry = await page.evaluate(() => ({
      viewport: innerHeight, document: document.documentElement.scrollHeight,
      panes: [...document.querySelectorAll(".acceptance-panes > section")].map(node => node.getBoundingClientRect().height),
      inbox: document.querySelector('aside[aria-label="Approval inbox"]').getBoundingClientRect().height,
    }));
    assert(geometry.document <= geometry.viewport + 1, "approval inbox scroll stays inside the application viewport");
    assert(geometry.inbox <= geometry.viewport * 0.35 && geometry.panes.every(height => height >= 400), "four approval cards cannot collapse split conversations");
    await fixture.control("access", { mode: "hide-origin" });
    await refreshApprovals(page, "shared");
    // Open the inbox again after its refresh toggle so hidden-origin absence
    // is tested while the containing view remains visible.
    await refreshApprovals(page, "shared");
    await eventually(async () => await page.getByTestId(`gating-pending:${privateRequest.pending_id}`).count() === 0, "hidden-origin request clears from inbox");
    await eventually(async () => await page.getByTestId(`gating-pending:${matching.pending_id}`).count() === 3, "authorized matching request remains visible");
    assert.equal(await page.getByTestId(`gating-pending:${otherTurn.pending_id}`).count(), 1);
    assert.equal(await page.getByTestId(`gating-pending:${globalRequest.pending_id}`).count(), 1);
    assert(!(await page.locator("body").innerText()).includes("Private delivery review details"));
    assert.equal(decisions(fixture, privateRequest.pending_id).length, 0);
    await page.setViewportSize({ width: 1440, height: 900 });
    await capture(page, "shared-approval-hidden-origin-1440");
  });
}

async function staleCopies() {
  await withBrowser("real-shared-approval-stale-copies", async ({ fixture, page }) => {
    const accepted = await openConversation(page, fixture, "shared");
    const origin = { identity: "router:main", interaction_id: accepted.interaction_id };
    const created = await fixture.control("approval", { ...origin, action: "Resolve this approval exactly once" });
    await page.getByRole("button", { name: "Toggle second pane" }).click();
    await refreshApprovals(page, "shared");
    await eventually(async () => await page.getByTestId(`gating-pending:${created.pending_id}`).count() === 3, "two correlated panes and inbox");

    // Hold an actual authorized owner read, not a fabricated pending fixture.
    // It is released only after another operator settles and the stale UI's
    // decision receives the owner's conflict response.
    let armed = true, held = false, release;
    const gate = new Promise(resolve => { release = resolve; });
    await page.route("**/console/rpc", async route => {
      const body = route.request().postDataJSON();
      if (!armed || body?.method !== "meta/gating/pending") return route.continue();
      armed = false;
      const response = await route.fetch();
      const snapshot = await response.json();
      assert(snapshot.result?.pending.some(item => item.pending_id === created.pending_id));
      held = true;
      await gate;
      await route.fulfill({ response });
    });
    try {
      await refreshApprovals(page, "shared");
      await eventually(() => held, "delayed real pending snapshot");
      decisionResult(await decide(fixture, created.pending_id, "reject", "other-operator"), created.pending_id, "reject");
      await page.getByTestId(`gating-action:${created.pending_id}:approve`).first().click();
      await eventually(() => decisions(fixture, created.pending_id).some(item => item.response && JSON.parse(item.response).error), "stale UI receives owner conflict");
      release();
      await eventually(async () => await page.getByTestId(`gating-pending:${created.pending_id}`).count() === 0, "late pending response cannot recreate settled copies");
      assert.equal(decisions(fixture, created.pending_id).length, 2, "one other-operator decision and one explicit stale decision");
      const resolutions = (await audit(fixture)).filter(item => item.pending_id === created.pending_id && ["approval_decided", "rejection_decided"].includes(item.event_type));
      assert.equal(resolutions.length, 1);
      assert.equal(resolutions[0].event_type, "rejection_decided");
    } finally { release(); await page.unroute("**/console/rpc"); }

    const expired = await fixture.control("approval", { ...origin, timeout_ms: 3000, action: "This approval expires at the owner" });
    await refreshApprovals(page, "shared");
    await page.getByTestId(`gating-pending:${expired.pending_id}`).first().waitFor();
    await eventually(async () => !(await pending(fixture)).some(item => item.pending_id === expired.pending_id), "owner expires visible request", 8000);
    await page.getByTestId(`gating-action:${expired.pending_id}:approve`).first().click();
    await eventually(async () => await page.getByTestId(`gating-pending:${expired.pending_id}`).count() === 0, "expired copies settle after authoritative refresh");
    assert.equal(decisions(fixture, expired.pending_id).length, 1, "expiry never silently resubmits");
    assert.equal((await audit(fixture)).filter(item => item.pending_id === expired.pending_id && item.event_type === "timeout_fallback").length, 1);
    await capture(page, "shared-approval-stale-and-expired-settled");
  });
}

module.exports = {
  apiScenarios: [
    { id: "api-approval-owner-restart", family: "approvals", backend: "real", run: ownerRestart },
    { id: "api-approval-access", family: "approvals", backend: "real", run: accessMatrix },
    { id: "api-approval-owner-edges", family: "approvals", backend: "real", run: ownerEdges },
  ],
  browserScenarios: [
    ...["stock", "shared"].map(host => ({ id: `real-${host}-late-approval-geometry`, family: "real-approvals", backend: "real", run: () => lateApprovalGeometry(host) })),
    ...["stock", "shared"].map(host => ({ id: `real-${host}-approvals`, family: "real-approvals", backend: "real", run: () => composedApproval(host) })),
    ...["stock", "shared"].map(host => ({ id: `real-${host}-approval-revocation`, family: "real-approvals", backend: "real", run: () => accessRevocation(host) })),
    { id: "real-stock-approval-read-only", family: "real-approvals", backend: "real", run: readOnlyApproval },
    { id: "real-shared-approval-hidden-origin", family: "real-approvals", backend: "real", run: hiddenOriginAndCorrelation },
    { id: "real-shared-approval-stale-copies", family: "real-approvals", backend: "real", run: staleCopies },
  ],
};

if (require.main === module) {
  const { runScenarios } = require("../scenario-registry.cjs");
  runScenarios([...module.exports.apiScenarios, ...module.exports.browserScenarios]).catch(error => {
    process.stderr.write(`${error.stack || error}\n`); process.exitCode = 1;
  });
}
