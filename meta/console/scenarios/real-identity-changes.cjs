"use strict";

// Long-term agents in the stock console against a real gateway: the
// acceptance fixture in identity mode with its identity policy
// (META_FIXTURE_IDENTITY_POLICY=1). What the roster offers is the gateway's
// own answer (the experience read's section and affordances), the profiles
// and peers of New agent are its directory read, a refusal shows at the
// field it names in the console's sentence for its kind, and an agent's
// details read its record.
//
// The fixture's two agents are declared by its configuration: the gateway
// offers no change of either (their two affordances are false), so the
// console shows no Edit, Archive or Remove for them, and it would refuse a
// change that was sent all the same. The console works none of that out.

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

    // The agents the host's configuration declares: the gateway offers no
    // change of them, and their drawers show none. The roster's own
    // controls of an agent are still there.
    for (const row of experience.agent_sidebar.live_snapshot.agents) {
      assert.equal(row.affordances.can_change_identity, false, row.identity);
      assert.equal(row.affordances.can_remove_identity, false, row.identity);
    }
    for (const name of [router, delivery]) {
      await agent(name).locator(".cc-entity__hit").click();
      const drawer = page.getByTestId(`roster-detail:${name}`);
      await drawer.waitFor();
      await drawer.getByRole("button", { name: "Details", exact: true }).waitFor();
      for (const change of ["edit", "archive", "remove"]) {
        assert.equal(await page.getByTestId(`roster-action:${name}:${change}`).count(), 0, `${name} ${change}`);
      }
    }
    await page.screenshot({ path: path.join(evidence, "real-identity-configured-no-change.png"), fullPage: true });
    // The browser sent no change of them. One that is sent all the same is
    // the gateway's to refuse, and it does.
    for (const change of ["update", "archive", "remove"]) {
      assert.deepEqual(sent(fixture, `meta/identity/${change}`), [], change);
    }
    const refused = (await rpc(fixture.baseUrl, "meta/identity/archive", {
      request_id: "configured-archive", name: router, expected_spec_digest: record.spec_digest,
    })).body.error;
    assert.equal(refused.code, -32058);
    assert.equal(refused.data.kind, "owner_read_only");
    // Nothing changed: the agent is still in the roster and none is archived.
    await agent(router).waitFor();
    const listed = (await rpc(fixture.baseUrl, "meta/identity/list", { include_archived: true })).body.result.agents;
    assert.deepEqual(listed.filter(entry => entry.archived), []);

    // The agent's details read its record: nobody asked for a configured
    // agent, and its spec digest is among the technical details.
    await agent(router).locator(".cc-entity__hit").click();
    await page.getByTestId(`roster-detail:${router}`).getByRole("button", { name: "Details", exact: true }).click();
    const details = page.getByTestId(`inspect-record:${router}`);
    await details.waitFor();
    // Its menu offers no change of its record either.
    await page.getByTestId(`inspect-menu:${router}`).click();
    for (const change of ["edit", "archive", "remove"]) {
      assert.equal(await page.getByTestId(`inspect-action:${router}:${change}`).count(), 0, `details ${change}`);
    }
    await page.keyboard.press("Escape");
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
 * edits it, sets who reaches another and how it runs, archives the first,
 * brings it back and removes it, and a request that
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
    // What a change came to. An outcome is waited for longer than anything
    // else in the story: a change that follows a create can be answered
    // about half a minute after that create. The team's actor then still
    // sends the new agent's kickoff notice to a peer that is not awake, and
    // every later command waits behind that step; the gateway says so in
    // its log ("actor inline step exceeded its budget; every later command
    // is queued behind it", command_kind KickoffOutcomeResolved). The wait
    // stands until that step no longer holds the actor.
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

    // The details of the agent that was just created: its status is read
    // (the pane names it and says how it stands), and its Record says who
    // asked for it, by name and never by key, and how it runs.
    await openAgent("scout");
    await page.getByTestId("roster-detail:scout").getByRole("button", { name: "Details", exact: true }).click();
    await page.getByTestId("inspect-panel:scout").waitFor();
    await page.getByTestId("inspect-state:scout").waitFor();
    const scoutRecord = page.getByTestId("inspect-record:scout");
    await scoutRecord.waitFor();
    // The record's own facts: the first list of the section (the second is
    // its technical details).
    const facts = await scoutRecord.locator(".cc-kv").first().textContent();
    assert.match(facts, /Asked for by/);
    assert.match(facts, /Allowed by the host's rules/);
    assert.match(facts, /How it runs\s*As its profile says/);
    assert.equal(facts.includes(record.provenance.requester), false, "the person's key is no text of the record");
    assert.equal(/principal:/.test(await scoutRecord.innerHTML()), false, "the person's key is in no attribute of the record");
    assert.equal(await scoutRecord.getByText(/created|configured|adopted|imported|origin/i).count(), 0);
    // The host's configuration does not list this agent's name: no note.
    assert.equal(await page.getByTestId("inspect-roster-superseded:scout").count(), 0);
    await page.screenshot({ path: path.join(evidence, "real-identity-story-created-details.png"), fullPage: true });
    // The details opened beside the roster: close them, and the story goes
    // on in the roster.
    await page.locator('[data-testid^="pane:"]').filter({ has: page.getByTestId("inspect-panel:scout") })
      .locator('[data-testid^="pane-close:"]').click();
    await gone(page.getByTestId("inspect-panel:scout"), "the closed details");
    await page.getByTestId("roster-panel").waitFor();
    await agent("scout").waitFor();

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
    // The roster and the sidebar name the agent by its new display name:
    // the experience read labels its row by it, and follows the update.
    await agent("scout").getByText("Chief scout").first().waitFor();
    await page.getByTestId("sidebar-agent:scout").getByText("Chief scout").first().waitFor();
    await page.getByTestId("identity-cancel").click();

    // Who reaches an agent and how it runs, against the gateway. A new
    // agent can receive messages and runs as its profile says unless the
    // person chooses otherwise; one created internal only and turn by turn
    // reads both back, in its record and in its entry of the list.
    await page.getByTestId("roster-new-agent").click();
    await form.getByTestId("identity-profile:lead").check();
    await form.getByTestId("identity-field:name").fill("clerk");
    await form.getByTestId("identity-field:display_name").fill("Clerk");
    assert.equal(await form.getByTestId("identity-addressability:addressable").isChecked(), true);
    assert.equal(await form.getByTestId("identity-runtime-mode:profile").isChecked(), true);
    await form.getByTestId("identity-addressability:internal_only").check();
    await form.getByTestId("identity-runtime-mode:turn_driven").check();
    await form.getByTestId("identity-submit").click();
    await outcome("Clerk is created");
    await agent("clerk").waitFor();
    assert.deepEqual(creates().at(-1).agents, [{
      name: "clerk", profile: "lead", display_name: "Clerk", addressability: "internal_only", runtime_mode_override: "turn_driven",
    }]);
    let clerk = (await call("meta/identity/get", { name: "clerk" })).result;
    assert.equal(clerk.addressability, "internal_only");
    assert.equal(clerk.runtime_mode_override, "turn_driven");
    const listedAgents = (await call("meta/identity/list", {})).result.agents;
    const listedClerk = listedAgents.find(entry => entry.name === "clerk");
    assert.equal(listedClerk.addressability, "internal_only");
    assert.equal(listedClerk.runtime_mode_override, "turn_driven");
    // An agent whose record says neither reads neither.
    for (const entry of [record, listedAgents.find(listed => listed.name === "scout")]) {
      assert.equal(Object.hasOwn(entry, "addressability"), false, "an agent that can be messaged says nothing");
      assert.equal(Object.hasOwn(entry, "runtime_mode_override"), false, "an agent that runs as its profile says says nothing");
    }
    await page.getByTestId("identity-cancel").click();
    // Edit opens on what the record says. A record that names a way of
    // running is not offered "As its profile says": an update cannot hand
    // that back. The update sends the two members that changed and nothing
    // else, and the record reads them back.
    await openAgent("clerk");
    await page.getByTestId("roster-action:clerk:edit").click();
    const settings = page.getByTestId("identity-edit:clerk");
    await settings.getByTestId("identity-field:display_name").waitFor();
    assert.equal(await settings.getByTestId("identity-addressability:internal_only").isChecked(), true);
    assert.equal(await settings.getByTestId("identity-runtime-mode:turn_driven").isChecked(), true);
    assert.equal(await settings.getByTestId("identity-runtime-mode:profile").count(), 0);
    await settings.getByTestId("identity-addressability:addressable").check();
    await settings.getByTestId("identity-runtime-mode:autonomous_host").check();
    await page.screenshot({ path: path.join(evidence, "real-identity-story-settings.png"), fullPage: true });
    await settings.getByTestId("identity-submit").click();
    await outcome(/is updated$/);
    const settingsUpdates = sent(fixture, "meta/identity/update");
    assert.equal(settingsUpdates.length, 2);
    assert.equal(settingsUpdates[1].expected_spec_digest, clerk.spec_digest);
    assert.deepEqual(settingsUpdates[1].set, { addressability: "addressable", runtime_mode_override: "autonomous_host" });
    clerk = (await call("meta/identity/get", { name: "clerk" })).result;
    assert.equal(Object.hasOwn(clerk, "addressability"), false, "an agent that can be messaged says nothing");
    assert.equal(clerk.runtime_mode_override, "autonomous_host");
    await page.getByTestId("identity-cancel").click();
    // The host's configuration now lists the name of this agent too, as a
    // host does that names in its configuration an agent that was created
    // at run time. The agent stays served from its record, and its record
    // says so; the record of an agent the configuration does not list says
    // nothing.
    const listed = await fixture.control("roster", { name: "clerk" });
    assert.deepEqual(listed.superseded, ["clerk"]);
    clerk = (await call("meta/identity/get", { name: "clerk" })).result;
    assert.equal(clerk.roster_listing_superseded, true);
    assert.equal(clerk.display_name, "Clerk", "the record serves, not the configuration's entry");
    assert.equal(Object.hasOwn((await call("meta/identity/get", { name: "scout" })).result, "roster_listing_superseded"), false);
    // Its Record says how it runs, in words, and the pane says at the
    // agent's status, in words, that the configuration's entry is not
    // applied.
    await openAgent("clerk");
    await page.getByTestId("roster-detail:clerk").getByRole("button", { name: "Details", exact: true }).click();
    const clerkRecord = page.getByTestId("inspect-record:clerk");
    await clerkRecord.waitFor();
    assert.match(await clerkRecord.locator(".cc-kv").first().textContent(), /How it runs\s*On its own/);
    const listedNote = page.getByTestId("inspect-roster-superseded:clerk");
    await listedNote.waitFor();
    assert.equal(
      await listedNote.textContent(),
      "The host's configuration lists an agent of this name too. That entry is not applied: this agent runs from its own record, and a change made here changes that record.",
    );
    assert.equal(await page.getByTestId("inspect-roster-superseded:clerk").count(), 1);
    await page.screenshot({ path: path.join(evidence, "real-identity-story-listed-by-the-host.png"), fullPage: true });
    await page.locator('[data-testid^="pane:"]').filter({ has: page.getByTestId("inspect-panel:clerk") })
      .locator('[data-testid^="pane-close:"]').click();
    await gone(page.getByTestId("inspect-panel:clerk"), "the closed details");
    await page.getByTestId("roster-panel").waitFor();

    // One schedule targets the agent. A change of the whole agent takes it
    // with it, and the result says what became of it, by its name.
    const morning = await fixture.control("schedules", { identity: "scout", name: "Morning report" });
    const schedule = async () => (await (await fetch(`${fixture.backendUrl}/__fixture/schedules`)).json())
      .filter(listed => listed.schedule_id === morning.schedule_id)
      .map(({ name, phase, paused_for_target }) => ({ name, phase, paused_for_target }));
    assert.deepEqual(await schedule(), [{ name: "Morning report", phase: "active", paused_for_target: false }]);
    // What a result says in its own lines: the technical details hold the ids.
    const said = async () => (await notice.locator(".identity-notice__line").allTextContents()).join("\n");

    // Archive: the agent leaves the roster and is among the archived agents.
    // Its schedule is paused for it, and the notice names the schedule.
    await openAgent("scout");
    await page.getByTestId("roster-action:scout:archive").click();
    await page.getByTestId("identity-confirm:scout:archive").getByText(/Its last conversation is harvested/).waitFor();
    // The question says what the archive will do to the schedule, by its
    // name: the dialog asked the gateway's plan of this agent, at the digest
    // of its record, when it opened. Asking changed nothing.
    await page.getByTestId("identity-confirm:scout:archive").getByTestId("identity-confirm-schedules")
      .getByText("Its schedule Morning report will be paused until it is unarchived.").waitFor();
    const planned = sent(fixture, "meta/identity/plan").at(-1);
    assert.equal(planned.change, "archive");
    assert.equal(planned.name, "scout");
    assert.equal(planned.expected_spec_digest, record.spec_digest);
    assert.equal((await page.getByTestId("identity-confirm:scout:archive").textContent()).includes(morning.schedule_id), false, "a schedule's id is no part of the question");
    assert.deepEqual(await schedule(), [{ name: "Morning report", phase: "active", paused_for_target: false }]);
    assert.deepEqual(sent(fixture, "meta/identity/archive"), [], "nothing is sent before the person confirms");
    await page.screenshot({ path: path.join(evidence, "real-identity-story-archive-asked.png"), fullPage: true });
    await page.getByTestId("identity-confirm:scout:archive").getByTestId("identity-confirm-yes").click();
    await outcome(/is stopping$|is archived$/);
    await outcome("Its schedule Morning report is paused until it is unarchived.");
    assert.equal((await said()).includes(morning.schedule_id), false, "a schedule's id is no line of the result");
    assert.deepEqual(await schedule(), [{ name: "Morning report", phase: "paused", paused_for_target: true }]);
    assert.equal(sent(fixture, "meta/identity/archive")[0].expected_spec_digest, record.spec_digest);
    await gone(agent("scout"), "the archived agent in the roster");
    await page.getByTestId("identity-confirm-cancel").click();
    const archived = await archivedRow("scout");
    assert.match(await archived.textContent(), /Chief scout/);
    await page.screenshot({ path: path.join(evidence, "real-identity-story-archived.png"), fullPage: true });

    // The removal of the archived agent is asked without a harvest to come
    // (its last conversation was harvested when it was archived), and the
    // question says that its schedule will be ended. It is not confirmed.
    await page.getByTestId("roster-archived-action:scout:remove").click();
    const archivedRemove = page.getByTestId("identity-confirm:scout:remove");
    await archivedRemove.getByText(/Its last conversation was already harvested and distilled when it was archived\./).waitFor();
    await archivedRemove.getByTestId("identity-confirm-schedules").getByText("Its schedule Morning report will be ended.").waitFor();
    assert.equal(/is harvested|Archive is the reversible choice/.test(await archivedRemove.textContent()), false);
    await page.screenshot({ path: path.join(evidence, "real-identity-story-archived-remove-asked.png"), fullPage: true });
    await archivedRemove.getByTestId("identity-confirm-cancel").click();
    assert.deepEqual(sent(fixture, "meta/identity/remove"), []);
    assert.deepEqual(await schedule(), [{ name: "Morning report", phase: "paused", paused_for_target: true }]);

    // Unarchive: it is back in the roster, on the record it had, and its
    // schedule runs again. The question says so before.
    await page.getByTestId("roster-archived-action:scout:unarchive").click();
    await page.getByTestId("identity-confirm:scout:unarchive").getByTestId("identity-confirm-schedules")
      .getByText("Its schedule Morning report will run again.").waitFor();
    await page.getByTestId("identity-confirm:scout:unarchive").getByTestId("identity-confirm-yes").click();
    await outcome(/is back$/);
    await outcome("Its schedule Morning report runs again.");
    assert.deepEqual(await schedule(), [{ name: "Morning report", phase: "active", paused_for_target: false }]);
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

    // A viewer who is no administrator of the runtime is told how many
    // schedules a change takes with it, and not which: the plan and the
    // change's first answer carry the effect and the count without the
    // list. The question and the result then say the count and no name.
    await fixture.control("access", { mode: "no-admin" });
    try {
      const withheld = (await call("meta/identity/plan", {
        change: "archive", request_id: "plan-of-no-administrator", name: "scout", expected_spec_digest: record.spec_digest,
      })).result;
      assert.deepEqual(withheld.schedules, { effect: "paused", count: 1 }, "the plan counts the schedule and does not list it");
      await openAgent("scout");
      await page.getByTestId("roster-action:scout:archive").click();
      const counted = page.getByTestId("identity-confirm:scout:archive");
      await counted.getByTestId("identity-confirm-schedules").getByText("Its schedule will be paused until it is unarchived.").waitFor();
      assert.equal(/Morning report/.test(await counted.textContent()), false, "the question does not name the schedule");
      await page.screenshot({ path: path.join(evidence, "real-identity-story-archive-asked-no-administrator.png"), fullPage: true });
      await counted.getByTestId("identity-confirm-yes").click();
      await outcome(/is stopping$|is archived$/);
      await outcome("Its schedule is paused until it is unarchived.");
      const told = await notice.textContent();
      assert.equal(told.includes("Morning report") || told.includes(morning.schedule_id), false, "the result names no schedule, by name or by id");
      assert.deepEqual(await schedule(), [{ name: "Morning report", phase: "paused", paused_for_target: true }]);
      await gone(agent("scout"), "the archived agent in the roster");
      await page.getByTestId("identity-confirm-cancel").click();
      await archivedRow("scout");
      await page.getByTestId("roster-archived-action:scout:unarchive").click();
      const back = page.getByTestId("identity-confirm:scout:unarchive");
      await back.getByTestId("identity-confirm-schedules").getByText("Its schedule will run again.").waitFor();
      assert.equal(/Morning report/.test(await back.textContent()), false);
      await back.getByTestId("identity-confirm-yes").click();
      await outcome(/is back$/);
      await outcome("Its schedule runs again.");
      assert.equal(/Morning report/.test(await notice.textContent()), false);
      assert.deepEqual(await schedule(), [{ name: "Morning report", phase: "active", paused_for_target: false }]);
      await agent("scout").waitFor();
      await page.getByTestId("identity-confirm-cancel").click();
    } finally {
      await fixture.control("access", { mode: "open" });
    }

    // Remove: for good. Its name is then taken for ever.
    await openAgent("scout");
    await page.getByTestId("roster-action:scout:remove").click();
    const confirm = page.getByTestId("identity-confirm:scout:remove");
    await confirm.getByText(/Its name can never be used again/).waitFor();
    await confirm.getByText(/Its last conversation is harvested/).waitFor();
    await confirm.getByText(/Archive is the reversible choice/).waitFor();
    await confirm.getByTestId("identity-confirm-schedules").getByText("Its schedule Morning report will be ended.").waitFor();
    assert.deepEqual(await schedule(), [{ name: "Morning report", phase: "active", paused_for_target: false }]);
    await page.screenshot({ path: path.join(evidence, "real-identity-story-remove-asked.png"), fullPage: true });
    await confirm.getByTestId("identity-confirm-yes").click();
    await outcome(/is stopping$|is removed$/);
    await outcome(/can never be used again/);
    // A removal ends the agent's schedule.
    await outcome("Its schedule Morning report is ended.");
    assert.deepEqual(await schedule(), []);
    await gone(agent("scout"), "the removed agent in the roster");
    await page.getByTestId("identity-confirm-cancel").click();
    await page.getByTestId("roster-new-agent").click();
    await form.getByTestId("identity-profile:lead").check();
    await form.getByTestId("identity-field:name").fill("scout");
    await form.getByTestId("identity-submit").click();
    // The refusal names the name, so it shows there and not as the form's
    // notice.
    const taken = page.getByTestId("identity-field-error:name");
    await taken.waitFor({ timeout: 90_000 });
    assert.equal(await taken.textContent(), "This name is taken. Choose another; a removed agent's name is never free again.");
    assert.equal(await notice.count(), 0);
    await taken.scrollIntoViewIfNeeded();
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
    assert.equal(ids.length, 13);
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

/**
 * An agent that waits for its host, against the real gateway: the fixture's
 * policy says the host prepares the agents of the profile `prepared`, so a
 * new one is committed and has no member until the host settles its
 * preparation. The console shows it by name, as waiting; a person can give
 * it up (Remove, asked as for an agent that never started) and nothing
 * else; the settle is the host's operator's, which no console caller is.
 */
async function waitingAgent() {
  requirePrebuilt();
  const stateDir = await fs.mkdtemp(path.join(os.tmpdir(), "meta-identity-waiting-"));
  const fixture = await startFixture({
    mode: "identity", identityPolicy: true, identityPreparation: true, person: true, stateDir, allowExtraMembers: true,
  });
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  page.setDefaultTimeout(20_000);
  const errors = [];
  page.on("pageerror", error => errors.push(error.message));
  const call = async (method, params = {}) => (await rpc(fixture.baseUrl, method, params)).body;
  try {
    const openRoster = async () => {
      await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
      await page.getByTestId("nav:roster").click();
      await page.getByTestId("roster-panel").waitFor();
    };
    await page.goto(`${fixture.baseUrl}/console`);
    await openRoster();
    const agent = name => page.locator(`[data-testid="roster-card:${name}"], [data-testid="roster-row:${name}"]`);
    const notice = page.getByTestId("identity-notice");
    await agent(router).waitFor();
    await fs.mkdir(evidence, { recursive: true });

    // A new agent of the profile the host prepares: the create is made, and
    // the result says that the agent is being prepared.
    await page.getByTestId("roster-new-agent").click();
    const form = page.getByTestId("identity-new-agent");
    await form.getByTestId("identity-profile:prepared").check();
    await form.getByTestId("identity-field:name").fill("porter");
    await form.getByTestId("identity-field:display_name").fill("The porter");
    await form.getByTestId("identity-submit").click();
    await notice.getByText("The porter is being prepared").waitFor({ timeout: 90_000 });
    await notice.getByText("The agent cannot be used until its preparation ends.").waitFor();
    await page.screenshot({ path: path.join(evidence, "real-identity-waiting-created.png"), fullPage: true });
    await page.getByTestId("identity-cancel").click();

    // Its record says that it waits, and names the create that is open for
    // it to the person who asked. It has no member, so no row of the roster.
    const record = (await call("meta/identity/get", { name: "porter" })).result;
    assert.equal(record.readiness, "preparation_pending");
    assert.equal(record.archived, false);
    assert.equal(Array.isArray(record.open_operations) && record.open_operations.length, 1);
    const [operation] = record.open_operations;
    assert.equal((await call("meta/identity/operation", { operation_id: operation })).result.outcome, "pending_preparation");
    assert.equal(await agent("porter").count(), 0);

    // The list under the roster shows it: its name first, a plain state,
    // no id, and Remove as its one control.
    await page.getByTestId("roster-archived-open").click();
    const waiting = page.getByTestId("roster-waiting:porter");
    await waiting.waitFor();
    const row = await waiting.textContent();
    assert.match(row, /The porter/);
    assert.match(row, /Waiting for its host/);
    assert.equal(row.includes(operation), false, "an operation id is no text of the row");
    assert.equal(/preparation_pending|principal:/.test(row), false);
    assert.equal(await waiting.locator("button").count(), 1);
    await waiting.getByTestId("roster-waiting-action:porter:remove").waitFor();
    await page.getByTestId("roster-archived").getByText("Its host is still preparing it. It joins the roster when that ends.").waitFor();
    await page.screenshot({ path: path.join(evidence, "real-identity-waiting-listed.png"), fullPage: true });

    // The settle is the host's operator's: the console lists no such method
    // and the gateway refuses it for a console caller, so the console
    // offers none. Nothing changed.
    const offered = (await call("meta/capabilities")).result.methods;
    assert.equal(offered.includes("meta/identity/create"), true);
    assert.equal(offered.includes("meta/identity/settle_preparation"), false);
    const refused = (await call("meta/identity/settle_preparation", { request_id: "console-settle", name: "porter" })).error;
    assert.equal(refused.code, -32058);
    assert.equal(refused.data.kind, "change_denied");
    assert.equal((await call("meta/identity/get", { name: "porter" })).result.readiness, "preparation_pending");

    // A host can give up on an agent it cannot prepare. A second agent
    // waits, and a person removes it from the list: the question is the
    // removal's for an agent that never started, and the create's
    // operation ends with the agent removed.
    await page.getByTestId("roster-new-agent").click();
    await form.getByTestId("identity-profile:prepared").check();
    await form.getByTestId("identity-field:name").fill("runner");
    await form.getByTestId("identity-field:display_name").fill("The runner");
    await form.getByTestId("identity-submit").click();
    await notice.getByText("The runner is being prepared").waitFor({ timeout: 90_000 });
    await page.getByTestId("identity-cancel").click();
    const [runnerCreate] = (await call("meta/identity/get", { name: "runner" })).result.open_operations;
    assert.equal(typeof runnerCreate, "string");
    const listOpen = page.getByTestId("roster-archived-open");
    const runnerRow = page.getByTestId("roster-waiting:runner");
    // The list is read when it is opened: open it, and read it again until
    // the new agent is in it.
    for (let attempt = 0; (await runnerRow.count()) === 0; attempt += 1) {
      assert(attempt < 40, "the second waiting agent is listed");
      if ((await listOpen.getAttribute("aria-expanded")) !== "true") await listOpen.click();
      try {
        await runnerRow.waitFor({ timeout: 1_000 });
      } catch {
        await listOpen.click();
      }
    }
    assert.equal(await runnerRow.locator("button").count(), 1, "Remove is its one control");
    await runnerRow.getByTestId("roster-waiting-action:runner:remove").click();
    const confirm = page.getByTestId("identity-confirm:runner:remove");
    await confirm.getByText("Remove The runner for good?").waitFor();
    await confirm.getByText(/It never started: its host has not finished preparing it/).waitFor();
    await confirm.getByText(/Its name can never be used again, and this cannot be undone\./).waitFor();
    assert.equal(/harvest|Archive is the reversible choice|schedule/i.test(await confirm.textContent()), false);
    assert.equal(await confirm.getByTestId("identity-confirm-archive-instead").count(), 0);
    assert.deepEqual(sent(fixture, "meta/identity/remove"), [], "nothing is sent before the person confirms");
    await page.screenshot({ path: path.join(evidence, "real-identity-waiting-remove-asked.png"), fullPage: true });
    await confirm.getByTestId("identity-confirm-yes").click();
    await notice.getByText(/^The runner is (removed|stopping)$/).waitFor({ timeout: 90_000 });
    assert.equal(/schedule/i.test((await notice.locator(".identity-notice__line").allTextContents()).join(" ")), false);
    const removes = sent(fixture, "meta/identity/remove");
    assert.deepEqual(removes.map(params => params.name), ["runner"]);
    // It leaves the list, and the agent that still waits stays in it.
    for (let attempt = 0; (await runnerRow.count()) > 0; attempt += 1) {
      assert(attempt < 200, "the removed agent leaves the list");
      await new Promise(resolve => setTimeout(resolve, 100));
    }
    await waiting.waitFor();
    await page.getByTestId("identity-confirm-cancel").click();
    // The operation of its create ends: done, with the agent removed.
    let ended;
    for (let attempt = 0; attempt < 300; attempt += 1) {
      ended = (await call("meta/identity/operation", { operation_id: runnerCreate })).result;
      if (ended.outcome === "done") break;
      await new Promise(resolve => setTimeout(resolve, 200));
    }
    assert.equal(ended.outcome, "done");
    assert.deepEqual(ended.agents.map(entry => [entry.name, entry.state]), [["runner", "removed"]]);
    assert.equal(
      (await call("meta/identity/list", { include_archived: true })).result.agents.some(entry => entry.name === "runner"),
      false,
    );

    // The host settles the preparation: the agent has a member, is in the
    // roster, and the list no longer shows it.
    const settled = await fixture.control("settle", { name: "porter" });
    assert.equal(settled.outcome, "done");
    assert.deepEqual(settled.agents.map(entry => [entry.name, entry.state]), [["porter", "committed"]]);
    const after = (await call("meta/identity/get", { name: "porter" })).result;
    assert.notEqual(after.readiness, "preparation_pending");
    assert.equal(Object.hasOwn(after, "open_operations"), false, "nothing is open for it any more");
    await page.reload();
    await openRoster();
    await agent("porter").waitFor({ timeout: 90_000 });
    await agent("porter").getByText("The porter").first().waitFor();
    await page.getByTestId("roster-archived-open").click();
    await page.getByTestId("roster-archived").getByText("No archived agents, and none that waits for its host.").waitFor();
    assert.equal(await page.getByTestId("roster-waiting:porter").count(), 0);
    await page.screenshot({ path: path.join(evidence, "real-identity-waiting-settled.png"), fullPage: true });

    assert.deepEqual(errors, []);
  } catch (error) {
    await fs.mkdir(evidence, { recursive: true });
    await page.screenshot({ path: path.join(evidence, "real-identity-waiting-agent-failure.png"), fullPage: true }).catch(() => {});
    await fs.writeFile(path.join(evidence, "real-identity-waiting-agent-failure.json"), JSON.stringify({
      error: String(error && error.stack || error), errors,
      identity: fixture.observations.filter(item => item.request && item.request.includes("meta/identity/")),
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
  { id: "real-identity-waiting-agent", family: "real-console", backend: "real", run: waitingAgent },
];

module.exports = { scenarios };
if (require.main === module) {
  require("../scenario-registry.cjs").runScenarios(scenarios).catch((error) => {
    process.stderr.write(`${error.stack || error.message}\n`);
    process.exit(1);
  });
}
