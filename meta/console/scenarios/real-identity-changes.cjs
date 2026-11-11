"use strict";

// Long-term agents in the stock console against a real gateway: the
// acceptance fixture in identity mode with its identity policy
// (META_FIXTURE_IDENTITY_POLICY=1). What the roster offers is the gateway's
// own answer (the experience read's section and affordances), the profiles
// and peers of New agent are its directory read, a refusal shows at the
// field it names in the console's sentence for its kind, and an agent's
// details read its record.
//
// The fixture's two agents are declared by its configuration, so a change
// of either is refused by the gateway; the console shows that sentence and
// does not work it out itself.

const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
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
    // A name the gateway refuses: the console's sentence, at the name.
    await form.getByTestId("identity-field:name").fill("Not A Name!");
    await form.getByTestId("identity-submit").click();
    assert.equal(
      await page.getByTestId("identity-field-error:name").textContent(),
      "This is not a valid agent name. Choose another.",
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
    await notice.getByText("This agent is set up in the host's configuration and cannot be changed here. Change it in the configuration.").waitFor();
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

/**
 * The whole story against the real gateway: a person creates an agent,
 * edits it, archives it, brings it back and removes it, and a request that
 * a person must decide waits and is withdrawn. The fixture keeps its
 * decisions in a state directory, as a host that can hold a change does.
 */
async function fullStory() {
  requirePrebuilt();
  const stateDir = await fs.mkdtemp(path.join(os.tmpdir(), "meta-identity-story-"));
  const fixture = await startFixture({ mode: "identity", identityPolicy: true, person: true, stateDir, allowExtraMembers: true });
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  page.setDefaultTimeout(20_000);
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  const call = async (method, params = {}) => (await rpc(fixture.baseUrl, method, params)).body;
  try {
    await page.goto(`${fixture.baseUrl}/console`);
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    await page.getByTestId("nav:roster").click();
    await page.getByTestId("roster-panel").waitFor();
    const agent = name => page.locator(`[data-testid="roster-card:${name}"], [data-testid="roster-row:${name}"]`);
    const notice = page.getByTestId("identity-notice");
    // What a change came to. The gateway answers a change that follows a
    // create late (about half a minute after that create), so an outcome is
    // waited for longer than anything else in the story.
    const outcome = text => notice.getByText(text).waitFor({ timeout: 90_000 });
    const openAgent = async (name) => {
      await agent(name).locator(".cc-entity__hit").click();
      await page.getByTestId(`roster-detail:${name}`).waitFor();
    };
    const gone = async (locator, label) => {
      for (let attempt = 0; attempt < 200; attempt += 1) {
        if ((await locator.count()) === 0) return;
        await new Promise(resolve => setTimeout(resolve, 100));
      }
      throw new Error(`still shown: ${label}`);
    };
    // The archived agents are read when the list is opened: open it, and
    // read it again until the agent is in it (an archive ends after its
    // agent stopped).
    const archivedRow = async (name) => {
      const open = page.getByTestId("roster-archived-open");
      const row = page.getByTestId(`roster-archived:${name}`);
      for (let attempt = 0; attempt < 40; attempt += 1) {
        if ((await open.getAttribute("aria-expanded")) !== "true") await open.click();
        try {
          await row.waitFor({ timeout: 1_000 });
          return row;
        } catch {
          await open.click();
        }
      }
      throw new Error(`not among the archived agents: ${name}`);
    };
    await agent(router).waitFor();
    await fs.mkdir(evidence, { recursive: true });

    // Create: a profile and a peer of the gateway's directory.
    await page.getByTestId("roster-new-agent").click();
    const form = page.getByTestId("identity-new-agent");
    await form.getByTestId("identity-profile:lead").check();
    await form.getByTestId("identity-field:name").fill("scout");
    await form.getByTestId("identity-field:display_name").fill("Scout");
    await form.getByTestId("identity-field:instructions:0").fill("Scout the archive.");
    await form.getByTestId(`identity-peer:${router}`).check();
    await form.getByTestId("identity-submit").click();
    await outcome("Scout is created");
    await agent("scout").waitFor();
    const creates = () => sent(fixture, "meta/identity/create");
    assert.deepEqual(creates()[0].agents, [{
      name: "scout", profile: "lead", display_name: "Scout", instructions: ["Scout the archive."], peers: [router],
    }]);
    let record = (await call("meta/identity/get", { name: "scout" })).result;
    assert.equal(record.display_name, "Scout");
    assert.deepEqual(record.peers, [router]);
    assert.equal(record.provenance.requester_kind, "human");
    await page.screenshot({ path: path.join(evidence, "real-identity-story-created.png"), fullPage: true });
    await page.getByTestId("identity-cancel").click();

    // An agent created without a display name has none: Edit opens with an
    // empty display name, and the roster names it by its name in words.
    await page.getByTestId("roster-new-agent").click();
    await form.getByTestId("identity-profile:lead").check();
    await form.getByTestId("identity-field:name").fill("night-desk");
    await form.getByTestId("identity-submit").click();
    await outcome("night-desk is created");
    await page.getByTestId("identity-cancel").click();
    assert.equal(Object.hasOwn((await call("meta/identity/get", { name: "night-desk" })).result, "display_name"), false);
    await openAgent("night-desk");
    await page.getByTestId("roster-action:night-desk:edit").click();
    const unnamed = page.getByTestId("identity-edit:night-desk");
    await unnamed.getByTestId("identity-field:display_name").waitFor();
    assert.equal(await unnamed.getByTestId("identity-field:display_name").inputValue(), "");
    await page.getByTestId("identity-cancel").click();

    // Edit: only what changed, at the digest of the record the form read.
    await openAgent("scout");
    await page.getByTestId("roster-action:scout:edit").click();
    const edit = page.getByTestId("identity-edit:scout");
    await edit.getByTestId("identity-field:display_name").waitFor();
    assert.equal(await edit.getByTestId("identity-field:display_name").inputValue(), "Scout");
    await edit.getByTestId("identity-field:display_name").fill("Chief scout");
    await edit.getByTestId("identity-field:instructions:0").fill("Scout the vault.");
    await edit.getByTestId("identity-submit").click();
    await outcome(/is updated$/);
    const updates = sent(fixture, "meta/identity/update");
    assert.equal(updates.length, 1);
    assert.equal(updates[0].expected_spec_digest, record.spec_digest);
    assert.deepEqual(updates[0].set, { display_name: "Chief scout", instructions: ["Scout the vault."] });
    record = (await call("meta/identity/get", { name: "scout" })).result;
    assert.equal(record.display_name, "Chief scout");
    assert.deepEqual(record.instructions, ["Scout the vault."]);
    await page.getByTestId("identity-cancel").click();

    // Archive: the agent leaves the roster and is among the archived agents.
    await openAgent("scout");
    await page.getByTestId("roster-action:scout:archive").click();
    await page.getByTestId("identity-confirm:scout:archive").getByTestId("identity-confirm-yes").click();
    await outcome(/is stopping$|is archived$/);
    assert.equal(sent(fixture, "meta/identity/archive")[0].expected_spec_digest, record.spec_digest);
    await gone(agent("scout"), "the archived agent in the roster");
    await page.getByTestId("identity-confirm-cancel").click();
    const archived = await archivedRow("scout");
    assert.match(await archived.textContent(), /Chief scout/);
    await page.screenshot({ path: path.join(evidence, "real-identity-story-archived.png"), fullPage: true });

    // Unarchive: it is back in the roster, on the record it had.
    await page.getByTestId("roster-archived-action:scout:unarchive").click();
    await page.getByTestId("identity-confirm:scout:unarchive").getByTestId("identity-confirm-yes").click();
    await outcome(/is back$/);
    await agent("scout").waitFor();
    await page.getByTestId("identity-confirm-cancel").click();
    assert.equal((await call("meta/identity/get", { name: "scout" })).result.archived, false);

    // A change the host's rules hold for a person: it waits, nothing is
    // made, and its requester withdraws it from the form.
    await page.getByTestId("roster-new-agent").click();
    await form.getByTestId("identity-profile:lead").check();
    await form.getByTestId("identity-field:name").fill("held-one");
    await form.getByTestId("identity-submit").click();
    await outcome("A person must decide this change");
    await notice.getByTestId("identity-deadline").waitFor();
    const held = creates().at(-1);
    const openDecisions = async () => (await call("meta/decisions/list", { phase: "open", limit: 20 })).result.decisions;
    assert.deepEqual((await openDecisions()).map(decision => decision.kind), ["identity.create"]);
    assert.equal((await call("meta/identity/list", {})).result.agents.some(entry => entry.name === "held-one"), false);
    await page.screenshot({ path: path.join(evidence, "real-identity-story-pending.png"), fullPage: true });
    await notice.getByTestId("identity-withdraw").click();
    await outcome("The request is withdrawn");
    assert.deepEqual(sent(fixture, "meta/identity/withdraw").map(params => params.request_id), [held.request_id]);
    assert.deepEqual(await openDecisions(), []);
    assert.equal((await call("meta/identity/list", {})).result.agents.some(entry => entry.name === "held-one"), false);
    await page.getByTestId("identity-cancel").click();

    // Remove: for good. Its name is then taken for ever.
    await openAgent("scout");
    await page.getByTestId("roster-action:scout:remove").click();
    const confirm = page.getByTestId("identity-confirm:scout:remove");
    await confirm.getByText(/Its name can never be used again/).waitFor();
    await confirm.getByText(/Archive is the reversible choice/).waitFor();
    await confirm.getByTestId("identity-confirm-yes").click();
    await outcome(/is stopping$|is removed$/);
    await outcome(/can never be used again/);
    await gone(agent("scout"), "the removed agent in the roster");
    await page.getByTestId("identity-confirm-cancel").click();
    await page.getByTestId("roster-new-agent").click();
    await form.getByTestId("identity-profile:lead").check();
    await form.getByTestId("identity-field:name").fill("scout");
    await form.getByTestId("identity-submit").click();
    await page.getByText("This name is taken. Choose another; a removed agent's name is never free again.").waitFor({ timeout: 90_000 });
    await page.screenshot({ path: path.join(evidence, "real-identity-story-removed-name.png"), fullPage: true });
    await page.getByTestId("identity-cancel").click();

    // A second held request, withdrawn from its decision card: the card is
    // the requester's own, says that a person cannot approve their own
    // request, and offers the withdrawal and no approval.
    await page.getByTestId("roster-new-agent").click();
    await form.getByTestId("identity-profile:lead").check();
    await form.getByTestId("identity-field:name").fill("held-two");
    await form.getByTestId("identity-submit").click();
    await outcome("A person must decide this change");
    const second = creates().at(-1);
    const [decision] = await openDecisions();
    assert.equal(decision.kind, "identity.create");
    await page.getByTestId("nav:gating").click();
    const card = page.getByTestId(`decision-card:${decision.decision_id}`);
    await card.waitFor();
    await card.getByText(/a person cannot approve their own request/i).waitFor();
    assert.equal(await card.getByRole("button", { name: /approve/i }).count(), 0);
    await page.screenshot({ path: path.join(evidence, "real-identity-story-own-card.png"), fullPage: true });
    await card.getByTestId("decision-card-withdraw").click();
    await gone(card, "the withdrawn request's card");
    assert.deepEqual(sent(fixture, "meta/identity/withdraw").map(params => params.request_id), [held.request_id, second.request_id]);
    assert.deepEqual(await openDecisions(), []);
    assert.equal((await call("meta/identity/list", {})).result.agents.some(entry => entry.name === "held-two"), false);

    // Each change the browser sent names a request id of its own.
    const ids = ["create", "update", "archive", "unarchive", "remove"]
      .flatMap(change => sent(fixture, `meta/identity/${change}`).map(params => params.request_id));
    assert.equal(ids.length, 9);
    assert.equal(ids.every(id => typeof id === "string" && id.length > 0), true);
    assert.equal(new Set(ids).size, ids.length);
    assert.deepEqual(errors, []);
  } catch (error) {
    await fs.mkdir(evidence, { recursive: true });
    await page.screenshot({ path: path.join(evidence, "real-identity-full-story-failure.png"), fullPage: true }).catch(() => {});
    await fs.writeFile(path.join(evidence, "real-identity-full-story-failure.json"), JSON.stringify({
      error: String(error && error.stack || error), errors,
      identity: fixture.observations.filter(item => item.request && item.request.includes("meta/identity/")),
      recent: fixture.observations.slice(-40).map(item => ({ method: item.method, path: item.path, status: item.status, request: (item.request || "").slice(0, 160) })),
      logs: fixture.logs(),
    }, null, 2)).catch(() => {});
    throw error;
  } finally {
    await browser.close();
    await fixture.close();
    await fs.rm(stateDir, { recursive: true, force: true });
  }
}

const scenarios = [
  { id: "real-identity-reads-and-refusals", family: "real-console", backend: "real", run: readsAndRefusals },
  { id: "real-identity-full-story", family: "real-console", backend: "real", run: fullStory },
];

module.exports = { scenarios };
if (require.main === module) {
  require("../scenario-registry.cjs").runScenarios(scenarios).catch((error) => {
    process.stderr.write(`${error.stack || error.message}\n`);
    process.exit(1);
  });
}
