"use strict";
// Side chats on the real runtime: "Fork to side chat" branches an agent's
// conversation into a new member the human continues in, beside the agent.
// The scenario proves the acceptance rows: the source's session, transcript
// and queued work stay as they were; the side chat answers from the source's
// context; it is unwired, listed under its source, kept across a console
// reload and a gateway restart; closing it retires it; a viewer who may not
// spawn members is not offered the fork; and the fork waits while the agent
// works.
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { randomUUID } = require("node:crypto");
const { chromium } = require("playwright");
const { startFixture, eventually } = require("../acceptance-runtime.cjs");

const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
const router = "router:main";
const delivery = "domain:delivery";
// The identity-first host: the stock gateway's default, whose boot keeps
// members a human created beside the declared roster.
const mode = "identity";

async function experienceAgents(fixture) {
  const response = await fetch(`${fixture.baseUrl}/console/experience`);
  assert.equal(response.status, 200);
  const experience = await response.json();
  const agents = experience.agent_sidebar?.live_snapshot?.agents;
  assert(Array.isArray(agents), "experience lists the live roster");
  return { experience, agents };
}

async function agentRow(fixture, identity) {
  const { agents } = await experienceAgents(fixture);
  return agents.find(agent => agent.identity === identity || agent.member_id === identity) || null;
}

async function sessionHistory(fixture, sessionId) {
  const response = await fetch(`${fixture.backendUrl}/__fixture/session-history?session_id=${encodeURIComponent(sessionId)}`);
  const text = await response.text();
  assert.equal(response.status, 200, text);
  return JSON.parse(text);
}

async function modelRequests(fixture) {
  const response = await fetch(`${fixture.backendUrl}/__fixture/requests`);
  assert.equal(response.status, 200);
  return response.json();
}

async function timeline(fixture, identity) {
  const response = await fetch(`${fixture.baseUrl}/console/timeline?identity=${encodeURIComponent(identity)}&mode=recent&limit=500`);
  assert.equal(response.status, 200);
  return (await response.json()).frames;
}

async function completed(fixture, identity, text) {
  return eventually(async () => {
    const frames = await timeline(fixture, identity);
    return frames.some(frame => frame.kind === "interaction_complete" && JSON.stringify(frame.payload || {}).includes(text)) && frames;
  }, `${identity} answers "${text}"`, 30_000);
}

/// The source agent as the runtime holds it: its session and its committed
/// transcript.
async function sourceState(fixture) {
  const row = await agentRow(fixture, router);
  assert(row?.session_id, "the source agent has a session");
  const history = await sessionHistory(fixture, row.session_id);
  return { sessionId: row.session_id, wiredTo: [...(row.wired_to || [])].sort(), history: JSON.stringify(history) };
}

async function sideChats() {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Use the coordinator's prebuilt fixture; this scenario must not compile Rust.");
  const stateDir = await fs.mkdtemp(path.join(os.tmpdir(), "meta-side-chats-"));
  let fixture = await startFixture({ mode, stateDir });
  let browser = await chromium.launch({ headless: true });
  let page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  page.setDefaultTimeout(15_000);
  const result = { errors: [], checks: [] };
  const word = `heron-${randomUUID().slice(0, 8)}`;
  let alias = null;
  let barrierId = null;
  const watch = target => target.on("pageerror", error => result.errors.push(error.message));
  watch(page);
  const check = label => result.checks.push({ label, at: Date.now() });
  const sidebarRow = identity => page.getByTestId(`sidebar-agent:${identity}`);
  const forkAction = () => page.getByTestId(`chat-pane:${router}`).getByTestId("conv-action:fork-side-chat");
  try {
    await page.goto(`${fixture.baseUrl}/console`);
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    await sidebarRow(router).click();
    await page.getByTestId(`chat-composer:${router}`).waitFor();

    // A fact the side chat must later answer from: stated to the source
    // before the fork.
    const fact = `Remember this for later: the release code word is ${word}.`;
    await fixture.control("model", { source: "Noted.", delay_ms: 0, chunk_chars: 32 });
    await page.getByTestId(`chat-composer:${router}`).fill(fact);
    await page.getByTestId(`chat-composer:${router}`).press("Enter");
    await completed(fixture, router, "Noted.");
    check("the source holds a fact before the fork");

    // The fork waits while the agent works: hold one source turn at the model.
    barrierId = `side-chat-${randomUUID().slice(0, 8)}`;
    const held = `Prepare the release summary for ${barrierId}.`;
    await fixture.control("model-barrier", { action: "arm", plan: { id: barrierId, match_text: held, source: "Summary ready." } });
    await page.getByTestId(`chat-composer:${router}`).fill(held);
    await page.getByTestId(`chat-composer:${router}`).press("Enter");
    await eventually(async () => {
      const status = await fixture.control("model-barrier", { action: "status", id: barrierId });
      return status.entered && !status.released;
    }, "the source turn is held at the model");
    await eventually(async () => await forkAction().isDisabled(), "Fork to side chat is disabled while the agent works");
    check("fork disabled while the source runs");
    await fixture.control("model-barrier", { action: "release", id: barrierId });
    barrierId = null;
    await completed(fixture, router, "Summary ready.");
    await eventually(async () => await forkAction().isEnabled(), "Fork to side chat is enabled once the agent is idle");
    check("fork enabled once the source is idle");

    // Fork.
    const before = await sourceState(fixture);
    const requestsBeforeFork = (await modelRequests(fixture)).length;
    await forkAction().click();
    const sidePane = page.locator(`[data-testid^="chat-pane:${router}-side-"]`);
    await sidePane.waitFor();
    alias = (await sidePane.getAttribute("data-testid")).slice("chat-pane:".length);
    result.alias = alias;
    await page.getByTestId(`chat-pane:${router}`).waitFor();
    assert.match(await page.getByTestId(`side-chat-banner:${alias}`).innerText(), /Side chat of/);
    check("the side chat opens beside its source");

    const sideRow = await eventually(() => agentRow(fixture, alias), "the experience lists the side chat");
    assert.equal(sideRow.side_chat_of, router, "the side chat names its source");
    assert.deepEqual(sideRow.wired_to || [], [], "a side chat has no peer edge");
    const afterFork = await sourceState(fixture);
    assert.deepEqual(afterFork, before, "the fork leaves the source's session, transcript and wiring as they were");
    assert.equal((await modelRequests(fixture)).length, requestsBeforeFork, "the fork runs no model turn");
    assert.deepEqual(before.wiredTo, [delivery], "the source keeps its one peer");
    check("the source is untouched by the fork");

    // Listed under its source.
    const row = sidebarRow(alias);
    assert.equal(await row.getAttribute("data-side-chat"), "true");
    assert.equal(await row.getAttribute("data-child-of-host"), "true");
    const order = await page.locator('[data-testid^="sidebar-agent:"]').evaluateAll(nodes =>
      nodes.map(node => node.getAttribute("data-testid").slice("sidebar-agent:".length)));
    assert.equal(order.indexOf(alias), order.indexOf(router) + 1, `the side chat lists right under its source: ${order}`);
    check("the sidebar lists the side chat under its source");

    // Continue in the side chat: its first answer comes from the source's
    // context.
    const question = "What is the release code word?";
    await fixture.control("model", { source: `The code word is ${word}.`, delay_ms: 0, chunk_chars: 32 });
    await page.getByTestId(`chat-composer:${alias}`).fill(question);
    await page.getByTestId(`chat-composer:${alias}`).press("Enter");
    const sideRequest = await eventually(async () => {
      const requests = await modelRequests(fixture);
      return requests.find(request => {
        const text = JSON.stringify(request.messages || []);
        return text.includes(question) && text.includes(word);
      });
    }, "the side chat's model request carries the source's context");
    result.sideChatRequest = sideRequest;
    await completed(fixture, alias, `The code word is ${word}.`);
    assert.deepEqual(await sourceState(fixture), before, "the side chat's turn leaves the source as it was");
    check("the side chat answers from the source's context");

    // A console reload keeps it.
    await page.reload();
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    await page.getByTestId(`chat-pane:${alias}`).waitFor();
    assert.equal(await sidebarRow(alias).getAttribute("data-side-chat"), "true");
    check("the side chat survives a console reload");
    await fs.mkdir(evidence, { recursive: true });
    await page.screenshot({ path: path.join(evidence, "side-chats-open.png"), fullPage: true });

    // A gateway restart keeps it: the roster's durable fork names its source.
    await browser.close();
    await fixture.close();
    fixture = await startFixture({ mode, stateDir, allowExtraMembers: true });
    browser = await chromium.launch({ headless: true });
    page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
    page.setDefaultTimeout(15_000);
    watch(page);
    const restored = await eventually(() => agentRow(fixture, alias), "the restarted gateway lists the side chat", 30_000);
    assert.equal(restored.side_chat_of, router, "after a restart the side chat still names its source");
    assert.deepEqual(restored.wired_to || [], [], "after a restart the side chat is still unwired");
    assert.equal((await sourceState(fixture)).sessionId, before.sessionId, "the source keeps its session across the restart");
    await page.goto(`${fixture.baseUrl}/console`);
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    assert.equal(await sidebarRow(alias).getAttribute("data-child-of-host"), "true");
    await sidebarRow(alias).click();
    await page.getByTestId(`chat-pane:${alias}`).getByText(`The code word is ${word}.`).waitFor();
    check("the side chat survives a gateway restart with its conversation");

    // Close: retire the side chat; the source stays.
    const sourceBeforeClose = await sourceState(fixture);
    await page.getByTestId(`side-chat-close:${alias}`).click();
    await page.getByTestId(`chat-pane:${alias}`).waitFor({ state: "detached" });
    await eventually(async () => (await agentRow(fixture, alias)) === null, "the closed side chat leaves the roster");
    assert.deepEqual(await sourceState(fixture), sourceBeforeClose, "closing the side chat leaves the source as it was");
    check("closing the side chat retires it and only it");

    // A viewer who may not spawn members is not offered the fork.
    await fixture.control("access", { mode: "no-spawn" });
    await page.reload();
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    await sidebarRow(router).click();
    await page.getByTestId(`chat-composer:${router}`).waitFor();
    await eventually(async () => (await page.getByTestId("conv-action:fork-side-chat").count()) === 0,
      "no fork action without the spawn grant");
    await fixture.control("access", { mode: "open" });
    check("the fork needs the spawn grant");

    assert.deepEqual(result.errors, [], "no unexpected browser errors");
  } catch (error) {
    result.failure = error.stack || String(error);
    result.html = await page.content().catch(() => "");
    await fs.mkdir(evidence, { recursive: true });
    await page.screenshot({ path: path.join(evidence, "side-chats-failure.png"), fullPage: true }).catch(() => {});
    throw error;
  } finally {
    if (barrierId) await fixture.control("model-barrier", { action: "release", id: barrierId }).catch(() => {});
    result.observations = fixture.observations;
    result.logs = fixture.logs();
    await fs.mkdir(evidence, { recursive: true });
    await fs.writeFile(path.join(evidence, "side-chats.json"), JSON.stringify(result, null, 2));
    await browser.close().catch(() => {});
    await fixture.close();
    await fs.rm(stateDir, { recursive: true, force: true });
  }
}

const scenarios = [{ id: "real-side-chats", family: "real-console", backend: "real", run: sideChats }];
module.exports = { scenarios };
if (require.main === module) require("../scenario-registry.cjs").runScenarios(scenarios).catch(error => { process.stderr.write(`${error.stack || error}\n`); process.exitCode = 1; });
