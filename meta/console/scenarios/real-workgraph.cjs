"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const os = require("node:os");
const path = require("node:path");
const { randomUUID } = require("node:crypto");
const { chromium } = require("playwright");
const { startFixture, eventually, rpc } = require("../acceptance-runtime.cjs");

const evidenceDir = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
const sender = "router:main";
const recipient = "domain:delivery";
const graphSteps = [
  "create-review", "create-render", "create-publish", "link-review", "link-render",
  "ready-before", "get-review", "get-render", "claim-review", "claim-render",
  "close-review", "close-render", "ready-after", "get-publish", "claim-publish",
  "close-publish", "snapshot",
];

function requirePrebuiltFixture() {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Set META_EXAMPLE_BIN_DIR to the coordinator's prebuilt examples; this lane must not trigger Cargo.");
}

async function ownerRpc(fixture, method, params = {}) {
  const response = await rpc(fixture.baseUrl, method, params);
  assert.equal(response.status, 200, `${method}: ${JSON.stringify(response.body)}`);
  assert(!response.body.error, `${method}: ${JSON.stringify(response.body)}`);
  assert(response.body.result, `${method}: missing result`);
  return response.body.result;
}

async function waitForMembers(fixture) {
  const response = await eventually(async () => {
    const observed = await rpc(fixture.baseUrl, "meta/wait_ready", { timeout_ms: 1_000 });
    if (observed.body.error?.message?.includes("observation_lane_saturated")) return false;
    if (observed.body.result?.timeout === true) return false;
    return observed;
  }, "runtime reports both members startup-ready", 20_000);
  assert.equal(response.status, 200);
  assert(!response.body.error, JSON.stringify(response.body));
  const readiness = response.body.result;
  assert(readiness, JSON.stringify(response.body));
  assert.equal(readiness.timeout, false, "runtime startup must converge before sending work");
  assert.equal(readiness.ready?.length, 2, JSON.stringify(readiness));
  assert.equal(new Set(readiness.ready.map(member => member.agent_identity)).size, 2);
  for (const member of readiness.ready) {
    assert.equal(member.snapshot?.status, "active", JSON.stringify(member));
    assert.equal(member.snapshot?.is_final, false, JSON.stringify(member));
    assert.equal(member.snapshot?.error, null, JSON.stringify(member));
  }
  return readiness;
}

async function recordedRequests(fixture) {
  const response = await fetch(`${fixture.backendUrl}/__fixture/requests`);
  assert.equal(response.status, 200);
  return response.json();
}

async function timeline(fixture, identity) {
  const response = await fetch(`${fixture.baseUrl}/console/timeline?identity=${encodeURIComponent(identity)}&mode=recent&limit=500`);
  assert.equal(response.status, 200);
  const value = await response.json();
  assert(Array.isArray(value.frames), JSON.stringify(value));
  return value.frames;
}

function textContent(content) {
  if (typeof content === "string") return content;
  return (content || []).filter(block => block.type === "text").map(block => block.text).join("");
}

function requestTools(request, runId) {
  const prefix = `fixture-${runId}-`;
  const calls = [];
  const results = new Map();
  for (const message of request.messages || []) {
    if (message.role === "block_assistant") {
      for (const block of message.blocks || []) {
        if (block.block_type === "tool_use" && block.data?.id.startsWith(prefix)) calls.push(block.data);
      }
    }
    if (message.role === "tool_results") {
      for (const result of message.results || []) {
        if (!result.tool_use_id.startsWith(prefix)) continue;
        assert.equal(result.is_error, false, `real tool failed: ${textContent(result.content)}`);
        results.set(result.tool_use_id.slice(prefix.length), JSON.parse(textContent(result.content)));
      }
    }
  }
  return { calls, results };
}

async function completedTranscript(fixture, identity, heading) {
  const outcome = await eventually(async () => {
    const frames = await timeline(fixture, identity);
    const failure = frames.find(frame => ["message_delivery_failed", "interaction_failed"].includes(frame.kind)
      || (["text_complete", "interaction_complete"].includes(frame.kind) && JSON.stringify(frame.payload).includes("Acceptance scenario stopped:")));
    if (failure) return { failure };
    return frames.some(frame => ["text_complete", "interaction_complete"].includes(frame.kind) && JSON.stringify(frame.payload).includes(heading)) && { frames };
  }, `${identity} ${heading}`, 60_000);
  assert(!outcome.failure, `script stopped: ${JSON.stringify(outcome.failure)}`);
  return outcome.frames;
}

async function canonicalSenderOwner(fixture) {
  const peer = await ownerRpc(fixture, "meta/cross_mob/peer_info", { member_id: sender });
  // The runtime builds comms_name from the actual roster entry through
  // MemberCommsName::new(mob_id, role, entry.agent_identity). Meta's public
  // alias can contain ':' while the canonical roster identity is encoded.
  // Lower the returned binding with WorkOwnerKey::mob_agent's wire contract.
  const parts = peer.comms_name?.split("/");
  assert(parts?.length === 3 && parts.every(part => /^[A-Za-z_][A-Za-z0-9_-]*$/.test(part)), JSON.stringify(peer));
  assert.equal(parts[0], peer.mob_id);
  assert.equal(peer.member_id, sender);
  return { kind: "agent", id: `mob/${peer.mob_id}/agent/${parts[2]}` };
}

async function configureGraph(fixture, runId) {
  const owner = await canonicalSenderOwner(fixture);
  await fixture.control("model", {
    source: "Acknowledged.", delay_ms: 0, chunk_chars: 256,
    scenario: { kind: "workgraph", run_id: runId, owner_id: owner.id },
  });
  return `[fixture:${runId}] Review the release diagram, complete both prerequisites, and verify the release WorkGraph.`;
}

async function apiSend(fixture, content) {
  const accepted = await ownerRpc(fixture, "meta/console/send", {
    identity: sender, content, origin: "console:workgraph-acceptance", origin_kind: "operator",
    idempotency_key: randomUUID(), handling_mode: "queue",
  });
  assert(accepted.interaction_id && accepted.input_frame_id, JSON.stringify(accepted));
  return accepted;
}

async function graphEvidence(fixture, runId) {
  const frames = await completedTranscript(fixture, sender, "WorkGraph scenario complete");
  const requests = await recordedRequests(fixture);
  const request = requests.findLast(item => requestTools(item, runId).results.has("snapshot"));
  assert(request, "a real model request must contain the final tool result");
  const { calls, results } = requestTools(request, runId);
  assert.equal(calls.length, graphSteps.length, "one invocation per real WorkGraph operation");
  assert.equal(new Set(calls.map(call => call.id)).size, calls.length, "no duplicate mutation ids in the transcript");
  assert.deepEqual(calls.map(call => call.id.slice(`fixture-${runId}-`.length)).sort(), [...graphSteps].sort(), "exact requested operation set");
  assert.deepEqual([...results.keys()].sort(), [...graphSteps].sort(), "every requested operation has an actual result");
  const owner = await canonicalSenderOwner(fixture);
  const item = step => results.get(step)?.item;
  const review = item("create-review"), render = item("create-render"), publish = item("create-publish");
  assert(review?.id && render?.id && publish?.id);
  assert.equal(new Set([review.id, render.id, publish.id]).size, 3);
  const beforeIds = results.get("ready-before").items.map(value => value.id);
  const afterIds = results.get("ready-after").items.map(value => value.id);
  assert(beforeIds.includes(review.id) && beforeIds.includes(render.id));
  assert(!beforeIds.includes(publish.id), "dependent is blocked by both prerequisites");
  assert(afterIds.includes(publish.id), "dependent becomes ready after prerequisite closure");
  for (const part of ["review", "render", "publish"]) {
    const claim = calls.find(call => call.id === `fixture-${runId}-claim-${part}`);
    const close = calls.find(call => call.id === `fixture-${runId}-close-${part}`);
    assert.equal(claim.args.expected_revision, item(`get-${part}`).revision);
    assert.deepEqual(claim.args.owner.key, owner, "claim addresses the runtime's canonical roster owner");
    assert.equal(close.args.expected_revision, item(`claim-${part}`).revision);
    assert.equal(item(`claim-${part}`).status, "in_progress");
    assert.equal(item(`close-${part}`).status, "completed");
  }
  const snapshot = await ownerRpc(fixture, "meta/workgraph/snapshot", { labels: [`fixture-${runId}`], include_terminal: true });
  assert.equal(snapshot.items.length, 3);
  assert(snapshot.items.every(value => value.status === "completed"));
  assert.equal(snapshot.edges.filter(edge => edge.kind === "blocks").length, 2);
  for (const prerequisite of [review, render]) {
    assert(snapshot.edges.some(edge => edge.kind === "blocks" && edge.from_id === prerequisite.id && edge.to_id === publish.id));
  }
  assert.deepEqual(snapshot.items, results.get("snapshot").snapshot.items, "host RPC and actual tool result read the same owner state");
  return { runId, owner, itemIds: { review: review.id, render: render.id, publish: publish.id }, snapshot, calls, results: Object.fromEntries(results), frames };
}

async function peerEvidence(fixture, graph, send) {
  const runId = `peer-${randomUUID().slice(0, 8)}`;
  const response = await fetch(`${fixture.backendUrl}/__fixture/peers`);
  assert.equal(response.status, 200);
  const peers = await response.json();
  const peerId = peers[recipient]?.[0];
  assert.equal(typeof peerId, "string", "host exposes canonical peer routing id");
  const body = `Review WorkGraph item ${graph.itemIds.publish}. Source prerequisite ${graph.itemIds.review} and badge prerequisite ${graph.itemIds.render} are complete. Record your acknowledgement for ${runId}.`;
  const acknowledgement = `Received the release review request for ${runId}.`;
  await fixture.control("model", {
    source: acknowledgement, chunk_chars: 256,
    scenario: { kind: "peer", run_id: runId, peer_id: peerId, peer_body: body },
  });
  await send(`[fixture:${runId}] Ask the delivery peer to inspect the completed WorkGraph.`);
  const senderFrames = await completedTranscript(fixture, sender, "Peer message submitted");
  await completedTranscript(fixture, recipient, acknowledgement);
  // Terminal live events can precede the canonical session-history notice
  // projection. Wait for that typed notice instead of accepting raw run input.
  const incomingEvidence = await eventually(async () => {
    const frames = await timeline(fixture, recipient);
    const notices = frames.flatMap(frame => (frame.payload?.message?.blocks || frame.payload?.blocks || [])
      .filter(block => block.type === "comms" && block.direction === "incoming")
      .map(block => ({ frameId: frame.id, content: (block.content || []).filter(part => part.type === "text").map(part => part.text).join("") })))
      .filter(candidate => candidate.content.includes(body));
    return notices.length > 0 && { frames, notices };
  }, "typed owner incoming peer notice");
  const recipientFrames = incomingEvidence.frames;
  const incoming = incomingEvidence.notices;
  assert.equal(incoming.length, 1, "one owner notice contains this exact peer delivery");
  const ownerIncoming = incoming[0];
  assert(senderFrames.some(frame => {
    if (["tool_call_requested", "tool_call", "tool_execution_started"].includes(frame.kind)) {
      return frame.payload?.name === "send_message" && frame.payload.args?.body === body;
    }
    return frame.kind === "system_notice" && frame.payload?.message?.blocks?.some(block =>
      block.type === "comms" && block.direction === "outgoing" && JSON.stringify(block).includes(body));
  }), "sender timeline exposes the real outgoing communication");
  const requests = await recordedRequests(fixture);
  const senderRequest = requests.findLast(request => requestTools(request, runId).results.has("send-peer"));
  assert(senderRequest, "sender gets the real send receipt in its model request");
  const { calls, results } = requestTools(senderRequest, runId);
  assert.equal(calls.length, 1, "recipient turns do not repeat the sender tool");
  assert.equal(calls[0].name, "send_message");
  assert.equal(calls[0].args.peer_id, peerId);
  assert.equal(calls[0].args.body, body);
  assert.equal(results.get("send-peer").status, "sent");
  assert(results.get("send-peer").receipt);
  const recipientRequest = requests.find(request => !JSON.stringify(request.messages).includes(`[fixture:${runId}]`)
    && JSON.stringify(request.messages).includes(body));
  assert(recipientRequest, "a separate recipient model request contains actual delivered content");
  assert.equal(requestTools(recipientRequest, runId).calls.length, 0);
  return { runId, peerId, body, ownerIncoming, acknowledgement, receipt: results.get("send-peer"), senderFrames, recipientFrames };
}

function browserMonitor(page) {
  const errors = [], expected = [];
  let allowance = null;
  const admittedCancellations = new WeakMap();
  page.on("request", request => {
    if (allowance?.matches(request)) admittedCancellations.set(request, allowance.reason);
  });
  page.on("pageerror", error => errors.push(error.message));
  page.on("requestfailed", request => {
    const failure = { method: request.method(), url: request.url(), error: request.failure()?.errorText };
    const reason = allowance?.matches(request) ? allowance.reason : admittedCancellations.get(request);
    if (failure.error?.includes("ERR_ABORTED") && reason) expected.push({ ...failure, reason });
    else errors.push({ ...failure, body: request.postData() });
  });
  return { errors, expected, async during(reason, action, matches = request => new URL(request.url()).pathname.endsWith("/timeline/stream")) {
    assert.equal(allowance, null);
    allowance = { reason, matches };
    try { return await action(); } finally { allowance = null; }
  } };
}

function authoritySetupCancellation(request) {
  if (new URL(request.url()).pathname.endsWith("/timeline/stream")) return true;
  try {
    const body = JSON.parse(request.postData() || "{}");
    return body.method === "meta/console/query_timeline" && body.params?.mode === "recent"
      && body.params?.limit === 200 && !body.params?.identity;
  } catch { return false; }
}

const pane = (page, host, identity) => page.getByTestId(host === "shared" ? "shared-pane-0" : `chat-pane:${identity}`);
const transcript = (page, host, identity) => pane(page, host, identity).locator(host === "shared" ? ".cc-conversation-pane__scroll" : ".conv__body");

async function revealLatest(page, host, identity) {
  const jump = pane(page, host, identity).getByRole("button", { name: "Jump to latest", exact: true });
  if (await jump.isVisible()) await jump.click();
}

async function visibleContent(locator, label) {
  await locator.waitFor();
  await locator.scrollIntoViewIfNeeded();
  await eventually(() => locator.evaluate(node => {
    const rect = node.getBoundingClientRect();
    let visible = { left: Math.max(0, rect.left), right: Math.min(innerWidth, rect.right), top: Math.max(0, rect.top), bottom: Math.min(innerHeight, rect.bottom) };
    for (let parent = node.parentElement; parent; parent = parent.parentElement) {
      const style = getComputedStyle(parent);
      if (/(auto|scroll|hidden|clip)/.test(style.overflow + style.overflowY + style.overflowX)) {
        const bounds = parent.getBoundingClientRect();
        visible = { left: Math.max(visible.left, bounds.left), right: Math.min(visible.right, bounds.right), top: Math.max(visible.top, bounds.top), bottom: Math.min(visible.bottom, bounds.bottom) };
      }
    }
    return visible.right > visible.left && visible.bottom > visible.top;
  }), `${label} intersects the actual viewport`);
}

async function selectIdentity(page, host, identity, monitor) {
  const action = async () => {
    if (host === "shared") await page.getByRole("combobox", { name: "Agent", exact: true }).selectOption(identity);
    else await page.getByTestId(`sidebar-agent:${identity}`).click();
    await pane(page, host, identity).waitFor();
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
  };
  if (monitor) return monitor.during(`switch conversation to ${identity}`, action);
  return action();
}

async function browserSend(page, host, identity, content) {
  const scope = pane(page, host, identity);
  const composer = host === "stock" ? scope.getByTestId(`chat-composer:${identity}`) : scope.getByRole("textbox", { name: "Message", exact: true });
  await composer.fill(content);
  const submit = host === "stock" ? scope.getByTestId(`chat-send:${identity}`) : scope.getByRole("button", { name: "Send", exact: true });
  const [response] = await Promise.all([page.waitForResponse(response => {
    if (response.request().method() !== "POST") return false;
    try {
      const envelope = JSON.parse(response.request().postData() || "{}");
      const request = envelope.method === "meta/console/send" ? envelope.params : response.url().endsWith("/send") ? envelope : null;
      return request?.identity === identity && (request.content === content
        || Array.isArray(request.content) && request.content.length === 1 && request.content[0].type === "text" && request.content[0].text === content);
    } catch { return false; }
  }), submit.click()]);
  assert.equal(response.status(), 200);
  const body = await response.json();
  assert(!body.error, JSON.stringify(body));
  const accepted = body.result || body;
  assert(accepted.interaction_id && accepted.input_frame_id, JSON.stringify(body));
  assert.equal(accepted.identity, identity);
  return accepted;
}

async function capture(page, name) {
  await fs.mkdir(evidenceDir, { recursive: true });
  await page.screenshot({ path: path.join(evidenceDir, `${name}.png`), fullPage: true });
}

async function captureFailure(fixture, name, error, page, monitor) {
  const read = async url => {
    try {
      const response = await fetch(url, { signal: AbortSignal.timeout(3_000) });
      return { status: response.status, body: await response.json() };
    } catch (cause) { return { error: String(cause) }; }
  };
  const [requests, senderTimeline, recipientTimeline] = await Promise.all([
    read(`${fixture.backendUrl}/__fixture/requests`),
    read(`${fixture.baseUrl}/console/timeline?identity=${encodeURIComponent(sender)}&mode=recent&limit=500`),
    read(`${fixture.baseUrl}/console/timeline?identity=${encodeURIComponent(recipient)}&mode=recent&limit=500`),
  ]);
  await fs.mkdir(evidenceDir, { recursive: true });
  await fs.writeFile(path.join(evidenceDir, `${name}.json`), JSON.stringify({
    error: error.stack || String(error), logs: fixture.logs(), requests, senderTimeline, recipientTimeline,
    ...(monitor ? { errors: monitor.errors, expectedCancellations: monitor.expected } : {}),
  }, null, 2));
  if (page) await fs.writeFile(path.join(evidenceDir, `${name}.html`), await page.content());
}

async function composedGraphAndPeer(host) {
  requirePrebuiltFixture();
  const stateDir = await fs.mkdtemp(path.join(os.tmpdir(), `meta-${host}-workgraph-`));
  const fixture = await startFixture({ stateDir });
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  const monitor = browserMonitor(page);
  try {
    await waitForMembers(fixture);
    const runId = `graph-${host}-${randomUUID().slice(0, 8)}`;
    const prompt = await configureGraph(fixture, runId);
    await monitor.during("initial host authority setup", async () => {
      await page.goto(fixture.baseUrl + (host === "shared" ? "/shared" : "/console"));
      await selectIdentity(page, host, sender);
    }, authoritySetupCancellation);
    const accepted = await browserSend(page, host, sender, prompt);
    const graph = await graphEvidence(fixture, runId);
    await transcript(page, host, sender).getByRole("heading", { name: "WorkGraph scenario complete", exact: true }).waitFor();
    await revealLatest(page, host, sender);
    await visibleContent(transcript(page, host, sender).getByText(graph.itemIds.publish, { exact: true }).last(), "completed WorkGraph item");
    await capture(page, `${host}-real-workgraph-1600`);
    if (host === "stock") {
      await page.getByTestId("nav:workgraph").click();
      await page.getByTestId("workgraph-view-toggle:graph").click();
      await eventually(async () => await page.getByTestId("workgraph-graph-node").count() === 3, "three real completed graph nodes");
      assert.equal(await page.locator('[data-testid="workgraph-graph-node"][data-status="completed"]').count(), 3);
      assert.equal(await page.locator('[data-testid="workgraph-graph-edge"][data-kind="blocks"]').count(), 2);
      await page.locator(`[data-testid="workgraph-graph-node"][data-item-id="${graph.itemIds.publish}"]`).click();
      const selection = page.getByTestId("workgraph-graph-detail");
      await eventually(async () => {
        return await selection.locator('[data-status="completed"]').count() === 1
          && (await selection.locator(".workgraph-graph__detail-description").textContent()).includes("Both review prerequisites");
      }, "selected node exposes canonical item status and description");
      assert.equal(await selection.getByRole("heading").textContent(), "Publish release candidate", "selected title is readable in full");
      assert.equal(await selection.locator("details").getAttribute("open"), null, "internal metadata starts collapsed");
      await page.setViewportSize({ width: 1440, height: 900 });
      await capture(page, "stock-real-workgraph-graph-1440");
      await selection.getByText("Item details", { exact: true }).click();
      assert.equal(await selection.locator("code").textContent(), graph.itemIds.publish, "exact owner ID remains available");
      await selection.getByRole("button", { name: "Copy work item ID", exact: true }).waitFor();
      await selectIdentity(page, host, sender, monitor);
    } else {
      await page.getByRole("button", { name: "Toggle second pane", exact: true }).click();
      await visibleContent(page.getByTestId("shared-pane-1").getByRole("heading", { name: "WorkGraph scenario complete", exact: true }), "second-pane completed WorkGraph");
      await capture(page, "shared-real-workgraph-split-1600");
    }
    const peer = await peerEvidence(fixture, graph, content => browserSend(page, host, sender, content));
    await revealLatest(page, host, sender);
    await visibleContent(transcript(page, host, sender).getByRole("heading", { name: "Peer message submitted", exact: true }), "sender peer completion");
    await capture(page, `${host}-real-peer-sender`);
    await selectIdentity(page, host, recipient, monitor);
    const incomingBody = transcript(page, host, recipient).locator(".cc-tool-call--incoming .cc-tool-call__peer-body").filter({ hasText: peer.body });
    await eventually(async () => await incomingBody.count() === 1, `${host} has one exact incoming peer message`);
    assert.equal(await incomingBody.textContent(), peer.ownerIncoming.content, "peer card preserves exact typed owner content, including the delivered body");
    await page.setViewportSize({ width: 1440, height: 900 });
    await visibleContent(incomingBody, "incoming peer body");
    await capture(page, `${host}-real-peer-body-1440`);
    await visibleContent(transcript(page, host, recipient).getByText(peer.acknowledgement, { exact: true }), "recipient acknowledgement");
    await capture(page, `${host}-real-peer-recipient-1440`);
    await monitor.during("reload releases previous stream", async () => {
      await page.reload();
      await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    }, authoritySetupCancellation);
    await selectIdentity(page, host, recipient, monitor);
    await incomingBody.waitFor();
    assert.equal(await incomingBody.textContent(), peer.ownerIncoming.content, "reloaded peer body preserves the exact owner content");
    await visibleContent(incomingBody, "reloaded incoming peer body");
    await capture(page, `${host}-real-peer-reloaded-1440`);
    await selectIdentity(page, host, sender, monitor);
    await visibleContent(transcript(page, host, sender).getByRole("heading", { name: "Peer message submitted", exact: true }), "sender peer completion");
    assert.deepEqual(monitor.errors, []);
    await fs.writeFile(path.join(evidenceDir, `${host}-real-workgraph-peer.json`), JSON.stringify({ accepted, graph, peer, errors: monitor.errors, expectedCancellations: monitor.expected }, null, 2));
  } catch (error) {
    await capture(page, `${host}-real-workgraph-peer-failure`).catch(() => {});
    await captureFailure(fixture, `${host}-real-workgraph-peer-failure`, error, page, monitor).catch(() => {});
    throw error;
  } finally {
    await browser.close(); await fixture.close(); await fs.rm(stateDir, { recursive: true, force: true });
  }
}

async function graphOwnerRestart() {
  requirePrebuiltFixture();
  const stateDir = await fs.mkdtemp(path.join(os.tmpdir(), "meta-workgraph-owner-"));
  let fixture = await startFixture({ stateDir });
  try {
    await waitForMembers(fixture);
    const runId = `graph-restart-${randomUUID().slice(0, 8)}`;
    await apiSend(fixture, await configureGraph(fixture, runId));
    const graph = await graphEvidence(fixture, runId);
    const peer = await peerEvidence(fixture, graph, content => apiSend(fixture, content));
    await fixture.close(); fixture = await startFixture({ stateDir });
    await waitForMembers(fixture);
    const restored = await ownerRpc(fixture, "meta/workgraph/snapshot", { labels: [`fixture-${runId}`], include_terminal: true });
    assert.deepEqual(restored.items, graph.snapshot.items, "owner restart preserves exact completed item revisions");
    assert.deepEqual(restored.edges, graph.snapshot.edges, "owner restart preserves exact dependency edges");
    const replayedPeer = await timeline(fixture, recipient);
    assert(replayedPeer.some(frame => JSON.stringify(frame.payload).includes(peer.body)), "durable recipient communication is queryable after restart");
    await fs.mkdir(evidenceDir, { recursive: true });
    await fs.writeFile(path.join(evidenceDir, "real-workgraph-owner-restart.json"), JSON.stringify({ graph, peer, restored }, null, 2));
  } catch (error) {
    await captureFailure(fixture, "real-workgraph-owner-restart-failure", error).catch(() => {});
    throw error;
  } finally { await fixture.close(); await fs.rm(stateDir, { recursive: true, force: true }); }
}

async function graphEventPolls() {
  requirePrebuiltFixture();
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 }, timezoneId: "Europe/Stockholm" });
  const monitor = browserMonitor(page);
  const evidence = { views: {} };
  const runId = `events-${randomUUID().slice(0, 8)}`;
  const steps = ["create-event-item", "events-first", "events-repeat", "events-empty"];
  const heading = "WorkGraph event review complete";
  try {
    await fixture.control("model", {
      source: "Acknowledged.", delay_ms: 0, chunk_chars: 256,
      scenario: { kind: "workgraph_events", run_id: runId },
    });
    await monitor.during("initial host authority setup", async () => {
      await page.goto(`${fixture.baseUrl}/console`);
      await selectIdentity(page, "stock", sender);
    }, authoritySetupCancellation);
    evidence.accepted = await browserSend(page, "stock", sender,
      `[fixture:${runId}] Create a review item, inspect its WorkGraph events twice, and check for newer events.`);
    evidence.frames = await completedTranscript(fixture, sender, heading);
    const requests = await recordedRequests(fixture);
    const request = requests.findLast(item => requestTools(item, runId).results.has("events-empty"));
    assert(request, "the real model request retains the empty poll result");
    const { calls, results } = requestTools(request, runId);
    assert.deepEqual(calls.map(call => call.id.slice(`fixture-${runId}-`.length)), steps);
    assert.deepEqual([...results.keys()], steps);
    const item = results.get("create-event-item").item;
    assert(item?.id && item.title, "a real runtime mutation creates the represented work item");
    const first = results.get("events-first");
    const repeated = results.get("events-repeat");
    const empty = results.get("events-empty");
    assert(first.events.length > 0, "the first poll reads actual committed item events");
    assert(first.events.every(event => event.item_id === item.id && Number.isSafeInteger(event.seq)));
    assert.equal(new Set(first.events.map(event => event.seq)).size, first.events.length);
    assert.deepEqual(repeated, first, "overlapping polls retain the exact owner events");
    assert.deepEqual(empty, { events: [] });
    for (const step of ["events-first", "events-repeat", "events-empty"]) {
      const call = calls.find(call => call.id === `fixture-${runId}-${step}`);
      assert.equal(call.name, "workgraph_events");
      const ownerResult = await ownerRpc(fixture, "meta/workgraph/events", call.args);
      assert.deepEqual(ownerResult, results.get(step), "host RPC and agent tool read the same canonical event ledger");
    }
    const lastSeq = Math.max(...first.events.map(event => event.seq));
    assert.equal(calls.at(-1).args.after_seq, lastSeq, "the empty query starts after the actual observed watermark");
    evidence.ownerItem = await ownerRpc(fixture, "meta/workgraph/get", { id: item.id });
    assert.deepEqual(evidence.ownerItem.item, item, "event polling does not mutate the item");
    evidence.eventClocks = await page.evaluate(events => ({
      timezone: Intl.DateTimeFormat().resolvedOptions().timeZone,
      values: events.map(event => ({
        at: event.at,
        clock: new Date(event.at).toLocaleTimeString("en-GB", { hour: "2-digit", minute: "2-digit", hourCycle: "h23" }),
      })),
    }), first.events.slice(-5));
    assert.equal(evidence.eventClocks.timezone, "Europe/Stockholm", "the event-clock oracle uses the explicit browser timezone");
    const expectedLines = first.events.slice(-5).map((event, index) =>
      `${event.kind.replaceAll("_", " ")} \u00b7 ${evidence.eventClocks.values[index].clock}`);
    const scope = () => transcript(page, "stock", sender);
    const card = () => scope().locator("[data-work-graph-card]");
    const emptyTool = () => scope().locator(".cc-tool-call").filter({
      has: page.locator('.cc-tool-call__name[title="workgraph_events"]'),
    });
    async function inspect(label) {
      await scope().getByRole("heading", { name: heading, exact: true }).waitFor();
      await eventually(async () => {
        const view = await scope().evaluate(root => ({
          cards: [...root.querySelectorAll("[data-work-graph-card]")].map(node => ({
            id: node.dataset.rootId,
            title: node.querySelector(".cc-work-graph__title")?.textContent,
            items: [...node.querySelectorAll('[data-testid^="workgraph-item:"]')].map(item => item.dataset.testid),
            events: [...node.querySelectorAll(".cc-work-graph__event")].map(event => event.textContent),
          })),
          tools: [...root.querySelectorAll(".cc-tool-call")]
            .filter(node => node.querySelector(".cc-tool-call__name")?.getAttribute("title")?.startsWith("workgraph_"))
            .map(node => ({ name: node.querySelector(".cc-tool-call__name")?.getAttribute("title"), count: node.querySelector(".cc-tool-call__count")?.textContent || null, status: node.querySelector(".cc-tool-call__status")?.textContent })),
        }));
        evidence.views[label] = view;
        assert.equal(view.cards.length, 1, "event-only observations keep the one existing work card");
        assert.equal(view.cards[0].title, item.title);
        assert(view.cards[0].items.includes(`workgraph-item:${item.id}`));
        assert.deepEqual(view.cards[0].events, expectedLines, "replayed event sequences appear once with browser-local clocks from their exact owner timestamps");
        assert.deepEqual(view.tools.map(tool => tool.name), ["workgraph_events"], "only the unrepresented empty poll remains as a generic WorkGraph tool");
        assert.equal(view.tools[0].count, null, "represented polls cannot hide in a grouped generic tool row");
        assert.match(view.tools[0].status || "", /Success/);
        return view;
      }, `${label}: one card represents repeated events and the empty query remains inspectable`);
      const header = emptyTool().locator(".cc-tool-call__header");
      if (await header.getAttribute("aria-expanded") !== "true") await header.click();
      const resultSection = emptyTool().locator(".cc-tool-call__section").filter({
        has: page.getByText("Result", { exact: true }),
      });
      const raw = await resultSection.locator("pre").textContent();
      assert.deepEqual(JSON.parse(raw), empty, "the empty tool result remains inspectable without invented events");
      evidence.views[label].emptyResultText = raw;
      await visibleContent(card(), "WorkGraph item and exact event summary");
      await capture(page, `stock-real-workgraph-events-${label}-card`);
      await visibleContent(resultSection, "empty WorkGraph poll result");
      await capture(page, `stock-real-workgraph-events-${label}-empty`);
    }
    await inspect("completed");
    await monitor.during("reload releases previous stream", async () => {
      await page.reload();
      await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    }, authoritySetupCancellation);
    await selectIdentity(page, "stock", sender, monitor);
    await inspect("reloaded");
    assert.deepEqual(evidence.views.reloaded, evidence.views.completed, "canonical reload preserves the exact card and only empty fallback");
    assert.deepEqual(monitor.errors, []);
    const apiFailures = fixture.observations.filter(observation => observation.status >= 400);
    assert.deepEqual(apiFailures, [], "actual API calls succeed");
    Object.assign(evidence, { runId, item, calls, results: Object.fromEntries(results), requests, expectedLines,
      errors: monitor.errors, expectedCancellations: monitor.expected, observations: fixture.observations });
    await fs.writeFile(path.join(evidenceDir, "stock-real-workgraph-events.json"), JSON.stringify(evidence, null, 2));
  } catch (error) {
    await capture(page, "stock-real-workgraph-events-failure").catch(() => {});
    await captureFailure(fixture, "stock-real-workgraph-events-failure", error, page, monitor).catch(() => {});
    await fs.writeFile(path.join(evidenceDir, "stock-real-workgraph-events-last-probe.json"), JSON.stringify(evidence, null, 2)).catch(() => {});
    throw error;
  } finally { await browser.close(); await fixture.close(); }
}

const apiScenarios = [{ id: "api-real-workgraph-peer-restart", family: "real-workgraph", backend: "real", run: graphOwnerRestart }];
const browserScenarios = ["stock", "shared"].map(host => ({
  id: `real-${host}-workgraph-peer`, family: "real-workgraph", backend: "real", run: () => composedGraphAndPeer(host),
}));
browserScenarios.push({ id: "real-stock-workgraph-events", family: "real-workgraph", backend: "real", run: graphEventPolls });

module.exports = { apiScenarios, browserScenarios };

if (require.main === module) {
  require("../scenario-registry.cjs").runScenarios([...apiScenarios, ...browserScenarios])
    .catch(error => { process.stderr.write(`${error.stack || error}\n`); process.exitCode = 1; });
}
