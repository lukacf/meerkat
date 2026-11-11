"use strict";

// Long-term agents in the stock console against a real gateway: the
// acceptance fixture in identity mode with its identity policy
// (META_FIXTURE_IDENTITY_POLICY=1). What the roster offers is the gateway's
// own answer (the experience read's section and affordances), the profiles
// and peers of New agent are its directory read, a refusal is its sentence
// at the field it names, and an agent's details read its record.
//
// The fixture's two agents are declared by its configuration, so a change
// of either is refused by the gateway; the console shows that sentence and
// does not work it out itself.

const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { chromium } = require("playwright");
const { startFixture, rpc } = require("../acceptance-runtime.cjs");

const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
const router = "router:main";
const delivery = "domain:delivery";

function requirePrebuilt() {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Set META_EXAMPLE_BIN_DIR to the coordinator's prebuilt fixture directory.");
}

/** The params of the calls of `method` the browser sent through the proxy. */
function sent(fixture, method) {
  return fixture.observations.flatMap((item) => {
    if (item.method !== "POST" || !item.request) return [];
    try {
      const body = JSON.parse(item.request);
      return body.method === method ? [body.params || {}] : [];
    } catch { return []; }
  });
}

async function readsAndRefusals() {
  requirePrebuilt();
  const fixture = await startFixture({ mode: "identity", identityPolicy: true, person: true });
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  page.setDefaultTimeout(20_000);
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  try {
    // The gateway's own answers, read beside the browser.
    const experience = await (await fetch(`${fixture.baseUrl}/console/experience`)).json();
    assert.deepEqual(experience.identity_changes, { available: true, can_create: true, can_change: true, can_remove: true });
    const directory = (await rpc(fixture.baseUrl, "meta/identity/directory")).body.result;
    assert.deepEqual(directory.profiles, [{ profile: "lead", verdict: "allowed" }]);
    assert.deepEqual(directory.peers.map(peer => [peer.name, peer.verdict]).sort(), [[delivery, "allowed"], [router, "allowed"]]);
    const record = (await rpc(fixture.baseUrl, "meta/identity/get", { name: router })).body.result;
    assert.match(record.spec_digest, /^sha256:[0-9a-f]{64}$/);

    await page.goto(`${fixture.baseUrl}/console`);
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    await page.getByTestId("nav:roster").click();
    await page.getByTestId("roster-panel").waitFor();
    const agent = name => page.locator(`[data-testid="roster-card:${name}"], [data-testid="roster-row:${name}"]`);
    await agent(router).waitFor();
    const notice = page.getByTestId("identity-notice");

    // New agent: the gateway's profile and peers, each with its verdict.
    await page.getByTestId("roster-new-agent").click();
    const form = page.getByTestId("identity-new-agent");
    const profiles = form.getByTestId("identity-field:profile");
    await profiles.getByText("Allowed").waitFor();
    await form.getByTestId("identity-profile:lead").check();
    await form.getByTestId(`identity-peer:${router}`).waitFor();
    await form.getByTestId(`identity-peer:${delivery}`).waitFor();
    // A name the gateway refuses: its sentence, at the name.
    await form.getByTestId("identity-field:name").fill("Not A Name!");
    await form.getByTestId("identity-submit").click();
    assert.equal(
      await page.getByTestId("identity-field-error:name").textContent(),
      "The name is not a valid agent name: choose another.",
    );
    assert.equal(await notice.count(), 0, "a refusal that names a field shows there, not as the form's notice");
    await fs.mkdir(evidence, { recursive: true });
    await page.screenshot({ path: path.join(evidence, "real-identity-new-agent-refused-name.png"), fullPage: true });
    const creates = sent(fixture, "meta/identity/create");
    assert.equal(creates.length, 1);
    assert.equal(typeof creates[0].request_id, "string");
    assert.deepEqual(creates[0].agents, [{ name: "Not A Name!", profile: "lead" }]);
    assert.deepEqual(Object.keys(creates[0]).filter(key => key !== "expected_caller_scope").sort(), ["agents", "request_id"]);
    await page.getByTestId("identity-cancel").click();

    // An agent the host's configuration declares: the gateway refuses its
    // archive, and the console shows the gateway's sentence.
    await agent(router).locator(".cc-entity__hit").click();
    await page.getByTestId(`roster-detail:${router}`).waitFor();
    await page.getByTestId(`roster-action:${router}:archive`).click();
    await page.getByTestId(`identity-confirm:${router}:archive`).getByTestId("identity-confirm-yes").click();
    await notice.getByText(/declared by the host's configuration/).waitFor();
    assert.equal(await notice.getByText("owner_read_only").count(), 0);
    const archives = sent(fixture, "meta/identity/archive");
    assert.equal(archives.length, 1);
    assert.equal(archives[0].name, router);
    // The change names the spec digest of the record the console just read.
    assert.equal(archives[0].expected_spec_digest, record.spec_digest);
    assert.deepEqual(Object.keys(archives[0]).filter(key => key !== "expected_caller_scope").sort(), ["expected_spec_digest", "name", "request_id"]);
    await page.screenshot({ path: path.join(evidence, "real-identity-configured-refused.png"), fullPage: true });
    await page.getByTestId("identity-confirm-cancel").click();
    // Nothing changed: the agent is still in the roster and none is archived.
    await agent(router).waitFor();
    assert.equal(await page.getByTestId("roster-archived").count(), 0);

    // The agent's details read its record: nobody asked for a configured
    // agent, and its spec digest is among the technical details.
    await agent(router).locator(".cc-entity__hit").click();
    await page.getByTestId(`roster-detail:${router}`).getByRole("button", { name: "Details", exact: true }).click();
    const details = page.getByTestId(`inspect-record:${router}`);
    await details.waitFor();
    assert.equal(await details.getByText("Asked for by").count(), 0);
    assert.equal(await details.getByText(/created|configured|adopted|imported|origin/i).count(), 0);

    assert.deepEqual(errors, []);
  } catch (error) {
    await fs.mkdir(evidence, { recursive: true });
    await page.screenshot({ path: path.join(evidence, "real-identity-reads-and-refusals-failure.png"), fullPage: true }).catch(() => {});
    await fs.writeFile(path.join(evidence, "real-identity-reads-and-refusals-failure.json"), JSON.stringify({
      error: String(error && error.stack || error), errors,
      identity: fixture.observations.filter(item => item.request && item.request.includes("meta/identity/")),
      logs: fixture.logs(),
    }, null, 2)).catch(() => {});
    throw error;
  } finally {
    await browser.close();
    await fixture.close();
  }
}

const scenarios = [
  { id: "real-identity-reads-and-refusals", family: "real-console", backend: "real", run: readsAndRefusals },
];

module.exports = { scenarios };
if (require.main === module) {
  require("../scenario-registry.cjs").runScenarios(scenarios).catch((error) => {
    process.stderr.write(`${error.stack || error.message}\n`);
    process.exit(1);
  });
}
