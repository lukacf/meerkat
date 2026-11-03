"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { chromium } = require("playwright");
const { startFixture, eventually, rpc } = require("../acceptance-runtime.cjs");
const { browserFailureMonitor, navigation } = require("./browser-failure-monitor.cjs");

const name = "real-checked-save-recovery";
const mutationMethods = new Set(["set", "enable", "rules/upsert", "rules/delete", "groups/set", "groups/delete"]
  .map(method => `meta/access/${method}`));
const evidenceDir = process.env.META_BROWSER_EVIDENCE
  || path.join(__dirname, "../../output/playwright/console-acceptance");

async function ownerRpc(fixture, method, params = {}) {
  const response = await rpc(fixture.baseUrl, method, params);
  assert.equal(response.status, 200, JSON.stringify(response));
  assert(!response.body.error, JSON.stringify(response.body));
  assert(response.body.result, `${method}: missing real owner result`);
  return response.body.result;
}
function checked(base, payload) {
  return { checked_v1: { owner_instance: base.owner_instance, expected_revision: base.revision, ...payload } };
}
function isRpc(response, method) {
  return response.url().endsWith("/console/rpc") && response.request().postDataJSON()?.method === method;
}
async function openAccess(page, baseUrl, suffix = "") {
  await navigation(page, "open real checked-save console", async () => {
    const response = await page.goto(`${baseUrl}/console${suffix}`);
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    return response;
  });
  await page.getByText("Access", { exact: true }).first().click();
  await page.getByRole("heading", { name: "Console access", exact: true }).waitFor();
  await page.getByTestId("access-tab:preview").waitFor();
}
async function refresh(page) {
  const response = page.waitForResponse(value => isRpc(value, "meta/access/get"));
  await page.getByTestId("access-refresh").click();
  const body = await (await response).json();
  assert(!body.error, JSON.stringify(body));
  await eventually(async () => !(await page.getByTestId("access-panel")
    .getByText("Refreshing owner state. Changes are temporarily unavailable.", { exact: true }).count()), "protected refresh settles");
  return body.result;
}
async function preview(page, subject, allowed, attempts) {
  const attempt = { page: page.url(), startedAtMs: Date.now(), requests: [] };
  attempts.push(attempt);
  const observeRequest = request => {
    if (!request.url().endsWith("/console/rpc") || request.method() !== "POST") return;
    const body = request.postDataJSON();
    if (["meta/capabilities", "meta/access/preview"].includes(body?.method)) {
      attempt.requests.push({ method: body.method, id: body.id, atMs: Date.now() });
    }
  };
  page.on("request", observeRequest);
  try {
    await page.getByTestId("access-tab:preview").click();
    await page.getByTestId("access-preview-subject").fill(subject);
    await page.getByTestId("access-preview-action").selectOption("agent.send");
    await page.getByTestId("access-preview-agent").selectOption("router:main");
    attempt.clickStartedAtMs = Date.now();
    // Handle both promises immediately so either failure reaches the artifact cleanup.
    const [response] = await Promise.all([
      page.waitForResponse(value => isRpc(value, "meta/access/preview")),
      page.getByTestId("access-preview-run").click().then(() => { attempt.clickCompletedAtMs = Date.now(); }),
    ]);
    const body = await response.json();
    assert.equal(body.result?.allowed, allowed, JSON.stringify(body));
    await eventually(async () => (await page.getByTestId("access-preview-result").getAttribute("data-allowed")) === String(allowed), "actual preview is rendered");
  } catch (error) {
    attempt.failureAtMs = Date.now();
    attempt.panelState = await page.getByTestId("access-panel").ariaSnapshot()
      .catch(snapshotError => ({ unavailable: String(snapshotError.message || snapshotError) }));
    throw error;
  } finally {
    page.off("request", observeRequest);
    attempt.finishedAtMs = Date.now();
  }
}

async function realCheckedSaveRecovery() {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Use the prebuilt console_acceptance_fixture; this scenario must not invoke Cargo.");
  const fixture = await startFixture();
  let browser, monitor, page, failure;
  const browserWrites = [], readonlyWrites = [], checkpoints = [];
  const previewAttempts = [];
  try {
    // Existing loopback fixture grants access.admin through its real allow-all
    // rule. This tests an open/bootstrap caller, not JWT principal isolation.
    await fixture.control("access", { mode: "open" });
    const original = await ownerRpc(fixture, "meta/access/get");
    assert.equal(original.conditional_mutations, "checked_v1");
    assert.equal(typeof original.owner_instance, "string");
    assert(original.owner_instance.length > 0);
    assert(Number.isSafeInteger(original.revision));
    browser = await chromium.launch({ headless: true });
    const context = await browser.newContext({ viewport: { width: 1440, height: 1000 } });
    monitor = browserFailureMonitor(context, { origin: fixture.baseUrl });
    page = await context.newPage();
    page.setDefaultTimeout(10_000);
    page.on("request", request => {
      if (!request.url().endsWith("/console/rpc") || request.method() !== "POST") return;
      const value = request.postDataJSON();
      if (mutationMethods.has(value?.method)) browserWrites.push(value);
    });
    await openAccess(page, fixture.baseUrl);
    await preview(page, "reader@example.test", true, previewAttempts);
    await page.getByTestId("access-preview-subject").fill("edited-reader@example.test");
    await page.getByTestId("access-preview-result").waitFor({ state: "detached" });
    await preview(page, "reader@example.test", true, previewAttempts);
    assert.equal(browserWrites.length, 0, "preview and input editing never write policy");

    await page.getByTestId("access-tab:overview").click();
    await page.getByTestId("access-edit-admins").click();
    const draft = page.getByTestId("access-admins-input");
    const save = page.getByTestId("access-save-admins");
    const admins = [...original.config.admins, "new-admin@example.test"];
    await draft.fill(admins.join(", "));
    const newerRule = { id: "checked-save-newer-rule", effect: "deny", subjects: ["reader@example.test"], actions: ["agent.send"], agents: ["router:main"] };
    const competing = await ownerRpc(fixture, "meta/access/rules/upsert", checked(original, { rule: newerRule }));
    assert.equal(competing.revision, original.revision + 1);
    const newer = await ownerRpc(fixture, "meta/access/get");
    assert.equal(newer.owner_instance, original.owner_instance);
    assert.equal(newer.revision, competing.revision);
    assert(newer.config.rules.some(rule => rule.id === newerRule.id));
    assert.deepEqual((await refresh(page)).config, newer.config);
    await page.getByTestId("access-tab:preview").click();
    assert.equal(await page.getByTestId("access-preview-result").count(), 0, "owner refresh invalidates the old preview");
    await preview(page, "reader@example.test", false, previewAttempts);
    await page.getByTestId("access-tab:overview").click();
    assert.equal(await draft.inputValue(), admins.join(", "), "refresh retains the unsaved draft");

    const staleResponse = page.waitForResponse(value => isRpc(value, "meta/access/set"));
    await save.click();
    const stale = await (await staleResponse).json();
    assert.equal(stale.error?.code, -32009, JSON.stringify(stale));
    assert.deepEqual(stale.error.data, { kind: "access_revision_conflict", expected_revision: original.revision, actual_revision: newer.revision });
    await page.getByTestId("access-error").waitFor();
    assert.equal(await page.getByTestId("access-error").innerText(), "Access configuration changed. Review the latest settings before saving again.");
    assert.equal(await draft.inputValue(), admins.join(", "));
    assert(await save.isDisabled());
    assert.deepEqual(browserWrites.map(value => value.params), [checked(original, { config: { ...original.config, admins } })]);
    assert.deepEqual(await ownerRpc(fixture, "meta/access/get"), newer, "stale write leaves real owner config and revision unchanged");
    await refresh(page);
    assert(await save.isDisabled(), "refresh alone must not rebase and retry the draft");
    assert.equal(browserWrites.length, 1);
    await page.getByRole("button", { name: "Review and reapply", exact: true }).click();
    assert.equal(browserWrites.length, 1, "explicit review itself sends no write");
    assert.equal(await draft.inputValue(), admins.join(", "));
    const currentResponse = page.waitForResponse(value => isRpc(value, "meta/access/set"));
    await save.click();
    const current = await (await currentResponse).json();
    assert.equal(current.result?.revision, newer.revision + 1, JSON.stringify(current));
    await draft.waitFor({ state: "detached" });
    const committed = await ownerRpc(fixture, "meta/access/get");
    assert.deepEqual(committed.config, { ...newer.config, admins }, "reviewed whole-config commit retains the competing rule");
    assert.equal(committed.revision, newer.revision + 1);
    assert.deepEqual(browserWrites[1].params, checked(newer, { config: { ...newer.config, admins } }));
    assert.equal(browserWrites.length, 2);
    checkpoints.push({ step: "stale-then-reviewed-commit", originalRevision: original.revision, committedRevision: committed.revision });

    // Exercise the supported presentation-only read-only switch against the
    // same actual owner. It is not a claim of backend authorization denial.
    const readonly = await context.newPage();
    readonly.setDefaultTimeout(10_000);
    readonly.on("request", request => {
      if (!request.url().endsWith("/console/rpc") || request.method() !== "POST") return;
      const value = request.postDataJSON();
      if (mutationMethods.has(value?.method)) readonlyWrites.push(value);
    });
    await openAccess(readonly, fixture.baseUrl, "?console_read_only=1");
    await readonly.getByText("This connection is read-only.", { exact: true }).waitFor();
    for (const id of ["access-edit-admins", "access-toggle-enabled"]) {
      assert.equal(await readonly.getByTestId(id).count(), 0);
    }
    await readonly.getByTestId("access-tab:rules").click();
    assert.equal(await readonly.getByTestId("access-rule-new").count(), 0);
    assert.equal(await readonly.getByTestId(`access-rule-edit:${newerRule.id}`).count(), 0);
    assert.equal(await readonly.getByTestId(`access-rule-delete:${newerRule.id}`).count(), 0);
    await preview(readonly, "reader@example.test", false, previewAttempts);
    assert.equal(readonlyWrites.length, 0);
    assert.deepEqual(await ownerRpc(fixture, "meta/access/get"), committed);

    await page.getByTestId("access-edit-admins").click();
    const recoveredAdmins = [...admins, "recovered-admin@example.test"];
    await draft.fill(recoveredAdmins.join(", "));
    let rejectedReads = 0;
    // Fault only the next protected read at the real HTTP transport boundary.
    // No synthetic owner result, mutation result, or capability is returned.
    await page.route("**/console/rpc", async route => {
      if (!rejectedReads && route.request().postDataJSON()?.method === "meta/access/get") {
        rejectedReads += 1;
        monitor.expectFailure(route.request(), "deliberate protected-read transport outage", "net::ERR_FAILED");
        return route.abort("failed");
      }
      return route.continue();
    });
    await page.getByTestId("access-refresh").click();
    await page.getByTestId("access-error").waitFor();
    assert.equal(rejectedReads, 1);
    assert.equal(await draft.inputValue(), recoveredAdmins.join(", "));
    assert(await save.isDisabled(), "unavailable protected read blocks writes from cached state");
    assert.equal(browserWrites.length, 2);
    assert.deepEqual(await ownerRpc(fixture, "meta/access/get"), committed);
    assert.deepEqual((await refresh(page)).config, committed.config);
    await page.getByTestId("access-error").waitFor({ state: "detached" });
    assert.equal(await draft.inputValue(), recoveredAdmins.join(", "));
    assert(await save.isEnabled());
    assert.equal(browserWrites.length, 2, "recovery refresh must not automatically save");
    const recoveredResponse = page.waitForResponse(value => isRpc(value, "meta/access/set"));
    await save.click();
    const recovered = await (await recoveredResponse).json();
    assert.equal(recovered.result?.revision, committed.revision + 1, JSON.stringify(recovered));
    await draft.waitFor({ state: "detached" });
    const final = await ownerRpc(fixture, "meta/access/get");
    assert.deepEqual(final.config, { ...committed.config, admins: recoveredAdmins });
    assert.equal(final.revision, committed.revision + 1);
    assert.equal(final.owner_instance, original.owner_instance);
    assert.deepEqual(browserWrites[2].params, checked(committed, { config: { ...committed.config, admins: recoveredAdmins } }));
    assert.equal(browserWrites.length, 3);
    assert.equal(readonlyWrites.length, 0);
    checkpoints.push({ step: "read-only-and-transport-recovery", rejectedReads, finalRevision: final.revision });
    monitor.assertClean();
    await fs.mkdir(evidenceDir, { recursive: true });
    await page.bringToFront();
    await page.screenshot({ path: path.join(evidenceDir, `${name}.png`), fullPage: true });
  } catch (error) {
    failure = String(error.stack || error);
    throw error;
  } finally {
    try {
      await fs.mkdir(evidenceDir, { recursive: true });
      await fs.writeFile(path.join(evidenceDir, `${name}.json`), JSON.stringify({
        backend: "real console_acceptance_fixture AccessController", caller: "existing open/bootstrap fixture rule",
        unavailableBoundary: "one deliberately aborted protected HTTP read, not owner entropy failure",
        browserWrites, readonlyWrites, checkpoints, previewAttempts,
        observations: fixture.observations, logs: fixture.logs(),
        browserFailures: monitor?.failures, browserErrors: monitor?.errors, failure: failure || null,
      }, null, 2));
    } finally {
      monitor?.stop();
      try { if (browser) await browser.close(); }
      finally { await fixture.close(); }
    }
  }
}

module.exports = { scenarios: [{ id: name, family: "real-access", backend: "real", run: realCheckedSaveRecovery }] };
