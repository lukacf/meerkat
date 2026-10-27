"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { randomUUID } = require("node:crypto");
const { performance } = require("node:perf_hooks");
const { setTimeout: delay } = require("node:timers/promises");
const { chromium } = require("playwright");
const { startFixture, eventually, rpc, snapshot } = require("../acceptance-runtime.cjs");

const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
const source = "## Acceptance reply\n\nReal runtime **Markdown**, a [safe link](https://example.com), and `code`.\n\n| Item | Value |\n| --- | --- |\n| Result | Ready |\n";
const agentIdentity = "router:main";

function assertStartupLineage(frames, historyPages) {
  const terminals = frames.filter(frame => frame.identity === agentIdentity && frame.source?.kind === "console_event"
    && ["interaction_complete", "run_completed"].includes(frame.kind) && frame.payload?.result === source);
  assert(terminals.length > 0, "startup has an actual completed Acceptance reply");
  const owners = terminals.map(terminal => {
    assert(terminal.run_id, "live completion has a canonical run");
    assert.equal(terminal.source_event_id, terminal.id, "live source event identity survives projection");
    const runId = terminal.run_id;
    const { runtime_key: runtimeKey, identity, session_id: sessionId } = terminal;
    let assistantMessageId = terminal.payload?.assistant_message_id;
    const live = frames.filter(frame => frame.run_id === runId && frame.source?.kind === "console_event");
    for (const frame of live) {
      assert.equal(frame.runtime_key, runtimeKey, `${runId}: live runtime owner`);
      assert.equal(frame.identity, identity, `${runId}: live agent owner`);
      assert.equal(frame.session_id, sessionId, `${runId}: live session owner`);
    }
    const starts = live.filter(frame => frame.kind === "run_started");
    assert.equal(starts.length, 1, `${runId}: one canonical start`);
    const interactionId = terminal.interaction_id;
    assert.equal(starts[0].interaction_id, interactionId, `${runId}: live start and completion interaction`);
    for (const boundary of [starts[0], terminal]) {
      assert.equal(boundary.payload.identity?.run_id, runId, `${runId}: runtime boundary owner`);
      assert.equal(boundary.payload.identity?.interaction_id, interactionId, `${runId}: runtime interaction owner`);
    }
    const deltas = live.filter(frame => frame.kind === "text_delta");
    assert.equal(deltas.map(frame => frame.payload.delta).join(""), source, `${runId}: exact source deltas`);
    assert(deltas.every(frame => frame.interaction_id === interactionId), `${runId}: delta interaction owner`);
    const history = frames.filter(frame => frame.source?.kind === "session_history" && frame.run_id === runId && frame.payload?.result === source);
    if (!historyPages) assert.equal(history.length, 1, `${runId}: one persisted reply`);
    assert(history.length <= 1, `${runId}: at most one projected persisted counterpart`);
    for (const projected of history) {
      assert.equal(projected.payload.message?.identity?.run_id, runId, `${runId}: persisted owner`);
      assert.equal(projected.interaction_id, interactionId, `${runId}: live and history interaction`);
    }
    let historyOffset;
    if (historyPages) {
      const page = historyPages.find(candidate => candidate.session_id === terminal.session_id);
      assert(page, `${runId}: actual durable session owner was read`);
      assert.equal(page.offset, 0, `${runId}: durable transcript starts at its origin`);
      assert.equal(page.has_more, false, `${runId}: complete durable transcript was read`);
      assert.equal(page.messages.length, page.message_count, `${runId}: all committed messages were read`);
      const matches = page.messages.flatMap((message, offset) => {
        const text = message.role === "block_assistant"
          ? (message.blocks || []).filter(block => block.block_type === "text").map(block => block.data?.text ?? block.text ?? "").join("\n\n")
          : message.role === "assistant" ? message.content : undefined;
        return message.identity?.run_id === runId && text === source ? [{ message, offset }] : [];
      });
      assert.equal(matches.length, 1, `${runId}: exactly one actual committed reply`);
      assert.equal(matches[0].message.identity.interaction_id, interactionId, `${runId}: actual persisted interaction owner`);
      if (assistantMessageId !== undefined) {
        assert.equal(matches[0].message.assistant_message_id, assistantMessageId, `${runId}: actual persisted assistant message owner`);
      } else assistantMessageId = matches[0].message.assistant_message_id;
      historyOffset = matches[0].offset;
    }
    if (assistantMessageId !== undefined) {
      assert(typeof assistantMessageId === "string" && assistantMessageId.trim(), `${runId}: typed assistant message owner`);
      for (const frame of deltas) assert.equal(frame.payload.assistant_message_id, assistantMessageId, `${runId}: delta assistant message owner`);
    }
    for (const projected of history) {
      assert.equal(projected.runtime_key, runtimeKey, `${runId}: persisted runtime owner`);
      assert.equal(projected.identity, identity, `${runId}: persisted agent owner`);
      assert.equal(projected.session_id, sessionId, `${runId}: persisted session owner`);
      if (assistantMessageId !== undefined) {
        assert.equal(projected.payload.assistant_message_id, assistantMessageId, `${runId}: persisted assistant message owner`);
        assert.equal(projected.payload.message.assistant_message_id, assistantMessageId, `${runId}: persisted message assistant owner`);
      }
    }
    const frameIds = frames.filter(frame => frame.run_id === runId).map(frame => frame.id);
    assert.equal(new Set(frameIds).size, frameIds.length, `${runId}: source IDs are unique`);
    return { runtimeKey, identity, sessionId, runId, interactionId, assistantMessageId,
      frameIds, liveIds: live.map(frame => frame.id), historyId: history[0]?.id, historyOffset };
  });
  assert.equal(new Set(owners.map(owner => owner.runId)).size, owners.length, "one canonical terminal per run");
  return owners;
}

// Member readiness and a live UI do not prove the requested reply committed.
// Observe its exact terminal and durable owner under one deadline.
// `expiry`, when given, is the deadline itself: an AbortSignal that aborts
// when the budget is spent, in place of `timeoutMs` of wall-clock time. Tests
// use it to expire the deadline at a typed point instead of racing a timer.
async function waitForStartupLineage({ timeline, durableHistory, expectedInteractionId, expectedOwners, readRendered, onObservation = () => {}, timeoutMs = 20_000, pollIntervalMs = 50, expiry }) {
  const controller = new AbortController();
  const { signal } = controller;
  const deadline = performance.now() + timeoutMs;
  let last;
  const timeoutError = () => new Error(`Timed out: startup Acceptance reply has completed with exact durable lineage${last ? `: ${last.message}` : ""}`);
  const deadlinePassed = () => expiry ? expiry.aborted : performance.now() >= deadline;
  function checkDeadline() {
    if (!signal.aborted && deadlinePassed()) controller.abort(timeoutError());
    signal.throwIfAborted();
  }
  let timer;
  let expire;
  const expired = new Promise((_, reject) => {
    expire = () => {
      controller.abort(timeoutError());
      reject(signal.reason);
    };
    if (!expiry) timer = setTimeout(expire, timeoutMs);
    else if (expiry.aborted) expire();
    else expiry.addEventListener("abort", expire, { once: true });
  });
  async function observe() {
    while (true) {
      checkDeadline();
      try {
        const { frames } = await timeline({ signal });
        checkDeadline();
        onObservation({ frames });
        checkDeadline();
        const history = await durableHistory(frames, { signal });
        checkDeadline();
        onObservation({ frames, history });
        const owners = assertStartupLineage(frames, history);
        if (expectedInteractionId) {
          assert.equal(owners.filter(owner => owner.interactionId === expectedInteractionId).length, 1,
            `accepted startup interaction completed with exact durable lineage: ${expectedInteractionId}`);
        }
        if (expectedOwners) {
          const identities = rows => rows.map(owner => [owner.runtimeKey, owner.identity, owner.sessionId,
            owner.runId, owner.interactionId, owner.assistantMessageId, owner.historyOffset]);
          assert.deepEqual(identities(owners), identities(expectedOwners), "same durable startup owners and transcript positions after refresh");
        }
        let rendered;
        if (readRendered) {
          checkDeadline();
          rendered = await readRendered({ signal });
          checkDeadline();
          onObservation({ frames, history, rendered });
          assertStartupRendering(rendered, owners);
        }
        checkDeadline();
        return { frames, history, owners, ...(readRendered ? { rendered } : {}) };
      } catch (error) {
        signal.throwIfAborted();
        last = error;
        checkDeadline();
      }
      await delay(expiry ? pollIntervalMs : Math.min(pollIntervalMs, Math.max(0, deadline - performance.now())), undefined, { signal });
    }
  }
  try {
    return await Promise.race([observe(), expired]);
  } catch (error) {
    // A cancelled fetch or delay can reject before the deadline promise wins.
    signal.throwIfAborted();
    throw error;
  } finally {
    clearTimeout(timer);
    expiry?.removeEventListener("abort", expire);
    // Cancel response bodies and sibling history reads as well as pending polls.
    controller.abort();
  }
}

async function observeStartupPhase(phase, { result, timeline, durableHistory, readRendered, timeoutMs, pollIntervalMs }) {
  assert(["initial", "repeated", "reconnected", "reloaded"].includes(phase), "known startup observation phase");
  assert.equal(typeof readRendered, "function", "every startup phase observes its actual rendered sources");
  const accepted = phase === "initial" ? result.startupAccepted : phase === "repeated" ? result.accepted : undefined;
  if (phase === "initial" || phase === "repeated") assert(accepted?.interaction_id, `${phase}: actual accepted interaction`);
  const expectedOwners = phase === "reconnected" || phase === "reloaded" ? result.repeatedOwners : undefined;
  if (phase === "reconnected" || phase === "reloaded") assert(expectedOwners?.length, `${phase}: previously committed startup owners`);
  // A durable reply can precede its canonical timeline carrier. Every phase
  // retries the timeline, history and DOM together under the existing deadline.
  const observation = await waitForStartupLineage({
    timeline, durableHistory, readRendered, expectedInteractionId: accepted?.interaction_id,
    expectedOwners, timeoutMs, pollIntervalMs,
    onObservation: ({ frames, history, rendered }) => {
      result[`${phase}Frames`] = frames;
      if (history) result[`${phase}History`] = history;
      else delete result[`${phase}History`];
      if (rendered) result[`${phase}Rendered`] = rendered;
      else delete result[`${phase}Rendered`];
    },
  });
  if (phase === "repeated") assert.equal(observation.owners.length, result.initialOwners.length + 1,
    "same words belong to one additional canonical run");
  result[`${phase}Owners`] = observation.owners;
  result.rendered[phase] = observation.rendered;
  return observation;
}

function startupLineageReaders({ baseUrl, backendUrl, fetch: request = globalThis.fetch }) {
  return {
    async timeline({ signal } = {}) {
      const response = await request(`${baseUrl}/console/timeline?identity=${encodeURIComponent(agentIdentity)}&mode=recent&limit=1000`, { signal });
      assert.equal(response.status, 200);
      return response.json();
    },
    async durableHistory(frames, { signal } = {}) {
      const sessions = [...new Set(frames.filter(frame => frame.identity === agentIdentity && frame.session_id).map(frame => frame.session_id))];
      assert(sessions.length > 0, "canonical runtime frames identify the actual session to read");
      return Promise.all(sessions.map(async sessionId => {
        const response = await request(`${backendUrl}/__fixture/session-history?session_id=${encodeURIComponent(sessionId)}`, { signal });
        assert.equal(response.status, 200, "fixture-only reader returns actual runtime session history");
        return response.json();
      }));
    },
  };
}

function assertStartupRendering(rendered, owners) {
  const replies = rendered.quotes.filter(quote => quote.source === source);
  assert.equal(replies.length, owners.length, "one complete startup reply per actual canonical run");
  assert.equal(new Set(rendered.rowIds).size, rendered.rowIds.length, "replay does not duplicate row IDs");
  const runIds = replies.map(reply => {
    const owner = owners.find(candidate => candidate.frameIds.includes(reply.id));
    assert(owner, `rendered source ID belongs to an exact canonical run: ${reply.id}`);
    return owner.runId;
  });
  assert.equal(new Set(runIds).size, owners.length, "one rendered owner per canonical run, including equal replies");
  assert.equal(rendered.tables, owners.length, "each canonical reply retains its complete Markdown table");
  const partial = rendered.quotes.filter(quote => quote.source !== source
    && quote.source.trim() && (quote.source.includes("Acceptance reply") || /^[\s|]+$/.test(quote.source)
      || (quote.source.trim() !== "Ready." && source.includes(quote.source.trim()))));
  assert.deepEqual(partial, [], "no partial startup duplicate or trailing pipe row");
  return runIds;
}

function expectedCancellation(request) {
  if (!request.failure()?.errorText?.includes("ERR_ABORTED")) return false;
  if (new URL(request.url()).pathname.endsWith("/timeline/stream")) return true;
  try {
    const body = JSON.parse(request.postData() || "{}");
    return body.method === "meta/console/query_timeline" && body.params?.mode === "recent" && body.params?.limit === 200 && !body.params?.identity;
  } catch { return false; }
}

async function startupLineage(host) {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Use the coordinator's prebuilt typed-lineage fixture; never compile Rust in this scenario.");
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: process.env.META_HEADED !== "1" });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  const result = { host, source, errors: [], expectedCancellations: [], rendered: {} };
  let allowance = "initialization";
  const pane = () => host === "shared" ? page.getByTestId("shared-pane-0") : page.getByTestId(`chat-pane:${agentIdentity}`).first();
  const viewport = () => pane().locator(host === "shared" ? ".cc-conversation-pane__scroll" : ".conv__body");
  page.on("pageerror", error => result.errors.push(error.message));
  page.on("requestfailed", request => {
    const detail = { url: request.url(), error: request.failure()?.errorText };
    if ((allowance === "initialization" && expectedCancellation(request))
      || (allowance === "disconnect" && new URL(request.url()).pathname.endsWith("/timeline/stream"))) {
      result.expectedCancellations.push({ ...detail, reason: allowance });
    } else result.errors.push(detail);
  });
  const { timeline, durableHistory } = startupLineageReaders(fixture);
  async function open(reload = false) {
    allowance = "initialization";
    if (reload) await page.reload(); else await page.goto(fixture.baseUrl + (host === "shared" ? "/shared" : "/console"));
    if (host === "stock" && !await pane().count()) {
      await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: /router:main/ }).first().click();
    }
    if (host === "shared") await page.getByRole("combobox", { name: "Agent", exact: true }).selectOption(agentIdentity);
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    await viewport().waitFor();
    allowance = null;
  }
  const readRendered = () => viewport().evaluate(node => ({
        rowIds: [...node.querySelectorAll("[data-conversation-row-id]")].map(row => row.dataset.conversationRowId),
        quotes: [...node.querySelectorAll("[data-quote-message-id]")].map(quote => ({ id: quote.dataset.quoteMessageId, source: quote.dataset.quoteSource })),
        tables: node.querySelectorAll("table").length,
      }));
  async function inspect(label) {
    const observation = await observeStartupPhase(label, { result, timeline, durableHistory, readRendered });
    const heading = viewport().getByRole("heading", { name: "Acceptance reply", exact: true }).last();
    await heading.scrollIntoViewIfNeeded();
    const bounds = await heading.boundingBox(), viewBounds = await viewport().boundingBox();
    assert(bounds && viewBounds && bounds.y >= viewBounds.y && bounds.y + bounds.height <= viewBounds.y + viewBounds.height,
      `${host} ${label}: complete reply heading is in the visible transcript`);
    await fs.mkdir(evidence, { recursive: true });
    await page.screenshot({ path: path.join(evidence, `${host}-real-startup-lineage-${label}.png`), fullPage: true });
    return observation;
  }
  try {
    // Kickoff notifications only reach peers already wired at that instant.
    // Request the report explicitly after readiness and retain its real receipt.
    result.startupRequest = {
      identity: agentIdentity, origin: "console:startup-lineage-acceptance", origin_kind: "operator",
      content: "Give the startup report for this console session.", idempotency_key: randomUUID(), handling_mode: "queue",
    };
    result.startupSend = await rpc(fixture.baseUrl, "meta/console/send", result.startupRequest);
    assert.equal(result.startupSend.status, 200);
    assert(result.startupSend.body.result?.interaction_id && result.startupSend.body.result?.input_frame_id,
      JSON.stringify(result.startupSend.body));
    result.startupAccepted = result.startupSend.body.result;
    await open();
    await inspect("initial");
    result.snapshot = await snapshot(fixture.backendUrl, `?identity=${encodeURIComponent(agentIdentity)}`);
    const snapshotFrames = result.snapshot.flatMap(item => item.data?.frame ? [item.data.frame] : []);
    assert.deepEqual(assertStartupLineage(snapshotFrames, result.initialHistory).map(owner => owner.runId).sort(), result.initialOwners.map(owner => owner.runId).sort(), "real SSE replay preserves canonical startup runs");

    await fixture.control("model", { source, delay_ms: 20, chunk_chars: 8 });
    const sent = await rpc(fixture.baseUrl, "meta/console/send", {
      identity: agentIdentity, origin: "console:startup-lineage-acceptance", origin_kind: "operator",
      content: "Repeat the startup report as a separate review turn.", idempotency_key: randomUUID(), handling_mode: "queue",
    });
    assert(sent.body.result?.interaction_id, JSON.stringify(sent.body));
    result.accepted = sent.body.result;
    await inspect("repeated");

    allowance = "disconnect";
    const streamsBefore = fixture.observations.filter(item => item.path.includes("/timeline/stream") && item.status === 200).length;
    fixture.disconnectStreams();
    await eventually(() => fixture.observations.filter(item => item.path.includes("/timeline/stream") && item.status === 200).length > streamsBefore, "browser reconnect opens another successful real stream");
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor(); allowance = null;
    await inspect("reconnected");
    assert.deepEqual(result.rendered.reconnected.rowIds, result.rendered.repeated.rowIds, "reconnect preserves source row identity");
    await open(true);
    await inspect("reloaded");
    assert.deepEqual(result.rendered.reloaded.rowIds, result.rendered.repeated.rowIds, "reload preserves source row identity");
    assert.deepEqual(result.errors, [], "no unexpected browser or network failures");
  } catch (error) {
    result.failure = error.stack || String(error);
    result.html = await page.content();
    await fs.mkdir(evidence, { recursive: true });
    await page.screenshot({ path: path.join(evidence, `${host}-real-startup-lineage-failure.png`), fullPage: true }).catch(() => {});
    throw error;
  } finally {
    result.observations = fixture.observations;
    result.logs = fixture.logs();
    await fs.mkdir(evidence, { recursive: true });
    await fs.writeFile(path.join(evidence, `${host}-real-startup-lineage.json`), JSON.stringify(result, null, 2));
    await browser.close(); await fixture.close();
  }
}

const scenarios = ["stock", "shared"].map(host => ({ id: `real-${host}-startup-lineage`, family: "real-presentation", backend: "real", run: () => startupLineage(host) }));
module.exports = { scenarios, assertStartupLineage, assertStartupRendering, waitForStartupLineage, observeStartupPhase, startupLineageReaders, source };
if (require.main === module) require("../scenario-registry.cjs").runScenarios(scenarios).catch(error => { process.stderr.write(`${error.stack || error}\n`); process.exitCode = 1; });
