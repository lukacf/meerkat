"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const os = require("node:os");
const { randomUUID } = require("node:crypto");
const { chromium } = require("playwright");
const { startFixture, eventually, rpc, snapshot } = require("../acceptance-runtime.cjs");
const { browserFailureMonitor, navigation } = require("./browser-failure-monitor.cjs");
const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
const actions = ["cancel_after_boundary", "interrupt"];
function stoppedTools(scenarioId) { return require("./real-routine-tools.cjs").expectedTools(scenarioId).slice(0, 4); }
function textContent(value) { return typeof value === "string" ? value : (value || []).filter(block => block.type === "text").map(block => block.text).join(""); }
function unknown(spec, tool) { return spec.action === "interrupt" && tool.step === "late"; }
function assertFrameOwner(frame, spec, requireSession = false) {
  assert.equal(frame.source?.kind, "console_event", "original runtime source");
  assert.equal(frame.source_event_id, frame.id, "original source event ID");
  assert.equal(frame.identity, "router:main", "canonical member");
  assert.equal(frame.interaction_id, spec.accepted.interaction_id, "canonical interaction");
  assert.equal(frame.run_id, spec.runId, "canonical run");
  if (requireSession || frame.session_id != null) assert.equal(frame.session_id, spec.accepted.session_id, "canonical session");
}
function isDirectedCancelled(frame, spec) {
  return frame.source?.kind === "console_event" && frame.kind === "interaction_failed"
    && frame.interaction_id === spec.accepted.interaction_id
    && [undefined, "interaction_failed"].includes(frame.payload?.source_event_type)
    && frame.payload?.reason?.kind === "cancelled";
}
function isCancelled(frame, spec) {
  if (spec.action === "interrupt") return isDirectedCancelled(frame, spec);
  // Operator prompts retain transcript correlation but do not publish directed
  // interaction terminals. Their cooperative cancellation is the agent report.
  return frame.source?.kind === "console_event" && frame.kind === "interaction_failed"
    && frame.identity === "router:main" && frame.session_id === spec.accepted.session_id
    && frame.interaction_id === spec.accepted.interaction_id && frame.run_id === spec.runId
    && frame.payload?.source_event_type === "run_failed"
    && frame.payload?.error_report?.class === "cancelled";
}
function assertStoppedFrames(frames, spec) {
  assert(actions.includes(spec.action));
  const start = frames.filter(frame => frame.source?.kind === "console_event" && frame.kind === "run_started" && frame.interaction_id === spec.accepted.interaction_id);
  assert.equal(start.length, 1, "one canonical start"); assertFrameOwner(start[0], spec, true);
  const terminal = frames.filter(frame => frame.source?.kind === "console_event" && frame.kind === "interaction_failed"
    && frame.interaction_id === spec.accepted.interaction_id);
  const agentFailures = terminal.filter(frame => frame.payload?.source_event_type === "run_failed");
  if (spec.action === "interrupt") {
    assert(terminal.some(frame => isDirectedCancelled(frame, spec)), "typed directed cancellation terminal, never cancelled prose");
    assert.equal(agentFailures.length, 0, "hard interruption has no agent cancellation report");
  } else assert.equal(agentFailures.length, 1, "cooperative stop has one actual agent cancellation report");
  assert.equal(new Set(terminal.map(frame => frame.id)).size, terminal.length, "distinct original terminal event IDs");
  for (const frame of terminal) {
    assertFrameOwner(frame, spec);
    if (agentFailures.includes(frame)) {
      assert.equal(frame.payload?.error_report?.class, "cancelled", "typed agent cancellation report");
      assert(isCancelled(frame, spec), "typed agent cancellation report belongs to the exact owner");
    } else assert(isDirectedCancelled(frame, spec), "typed directed cancellation terminal");
  }
  const expected = stoppedTools(spec.scenarioId), ids = new Set(expected.map(tool => tool.id));
  const owned = frames.filter(frame => frame.source?.kind === "console_event" && ids.has(frame.payload?.tool_call_id));
  assert.equal(new Set(owned.map(frame => frame.id)).size, owned.length, "distinct original tool event IDs");
  assert(!frames.some(frame => frame.payload?.tool_call_id === `fixture-${spec.scenarioId}-ready`), "no later tool after the stopped boundary");
  assert(!frames.some(frame => frame.kind === "interaction_complete" && frame.interaction_id === spec.accepted.interaction_id), "stopped interaction never completes successfully");
  const tools = expected.map(tool => {
    const calls = owned.filter(frame => frame.kind === "tool_call_requested" && frame.payload.tool_call_id === tool.id);
    const results = owned.filter(frame => ["tool_result_received", "tool_execution_completed"].includes(frame.kind) && frame.payload.tool_call_id === tool.id);
    assert.equal(calls.length, 1, `${tool.step}: one call`); assertFrameOwner(calls[0], spec);
    assert.equal(calls[0].payload.name, tool.name); assert.deepEqual(calls[0].payload.args, tool.args, "exact arguments");
    if (unknown(spec, tool)) {
      assert.equal(results.length, 0, "interrupted dispatch has no authoritative result");
    } else {
      for (const kind of ["tool_execution_completed", "tool_result_received"]) {
        const channel = results.filter(frame => frame.kind === kind);
        assert.equal(channel.length, 1, `${tool.step}: one ${kind} result`);
        const result = channel[0]; assertFrameOwner(result, spec, true);
        assert.equal(result.payload.name, tool.name, "exact tool name");
        if (kind === "tool_result_received") {
          assert.equal(textContent(result.payload.content ?? result.payload.result), tool.result, "exact result bytes");
          if (result.payload.result !== undefined) assert.equal(textContent(result.payload.result), tool.result, "exact result bytes in raw result");
        } else {
          // The execution fact carries no body; the result event carries the output.
          assert.equal(result.payload.content ?? result.payload.result, undefined, "the execution fact carries no result body");
        }
        assert.equal(result.payload.is_error, tool.error, "actual result outcome");
        assert(frames.indexOf(result) > frames.indexOf(calls[0]), "result follows its call");
        assert(frames.indexOf(result) < frames.indexOf(terminal[0]), "result precedes the owner terminal");
      }
    }
    return { id: tool.id, callId: calls[0].id,
      completionId: results.find(frame => frame.kind === "tool_execution_completed")?.id ?? null,
      resultId: results.find(frame => frame.kind === "tool_result_received")?.id ?? null,
      outcome: unknown(spec, tool) ? "unknown" : tool.error ? "error" : "success" };
  });
  return { startId: start[0].id, terminalIds: terminal.map(frame => frame.id), tools };
}
function assertProofRetained(previous, next) {
  assert.equal(next.startId, previous.startId, "exact original start identity");
  assert.deepEqual(next.tools, previous.tools, "exact tool source identities and outcomes");
  for (const id of previous.terminalIds) assert(next.terminalIds.includes(id), "original typed terminal identity retained");
}
function assertCompletedBaseline(page, frames, warmup) {
  assert.equal(warmup.accepted.session_id, warmup.sessionId, "warmup accepted by the pre-run actual owner session");
  assert.equal(page.session_id, warmup.sessionId, "pre-run actual owner session");
  assert.equal(page.offset, 0, "complete pre-run owner history"); assert.equal(page.has_more, false, "complete pre-run owner history");
  assert.equal(page.messages.length, page.message_count, "complete pre-run owner history");
  const terminals = frames.filter(frame => frame.source?.kind === "console_event"
    && frame.kind === "interaction_complete" && frame.payload?.source_event_type === "run_completed"
    && frame.interaction_id === warmup.accepted.interaction_id && frame.payload?.result === warmup.source);
  assert.equal(terminals.length, 1, "one exact completed warmup terminal");
  const terminal = terminals[0]; assert(terminal.run_id, "completed warmup has a canonical run");
  assertFrameOwner(terminal, { accepted: warmup.accepted, runId: terminal.run_id }, true);
  const replies = page.messages.flatMap((message, index) => {
    const source = message.role === "block_assistant"
      ? (message.blocks || []).filter(block => block.block_type === "text").map(block => block.data?.text ?? block.text ?? "").join("\n\n")
      : message.role === "assistant" ? message.content : undefined;
    return source === warmup.source && message.identity?.run_id === terminal.run_id
      && message.identity?.interaction_id === warmup.accepted.interaction_id ? [index] : [];
  });
  assert.equal(replies.length, 1, "one committed warmup reply with exact source and canonical identity");
  return { runId: terminal.run_id, terminalId: terminal.id, replyIndex: replies[0] };
}
// A cancelled run is closed by the same durable-tail recovery commit as a
// restart, so it ends with that generic notice; its stable prefix is asserted.
const INTERRUPTED_RUN_NOTICE_PREFIX = "A previous run was interrupted";
function cancelledRunRounds(spec) {
  const [notes, files, missing, late] = stoppedTools(spec.scenarioId);
  const identity = { interaction_id: spec.accepted.interaction_id, run_id: spec.runId };
  const calls = tools => ({ blocks: tools.map(tool => ({ block_type: "tool_use", data: { args: tool.args, id: tool.id, name: tool.name } })),
    identity, role: "block_assistant", stop_reason: "tool_use" });
  const results = tools => ({ results: tools.map(tool => ({ content: tool.result, is_error: tool.error, tool_use_id: tool.id })), role: "tool_results" });
  return [{ content: spec.sentContent, identity, role: "user" },
    calls([notes, files]), results([notes, files]), calls([missing]), results([missing]), calls([late]), results([late])];
}
function assertStoppedHistory(page, spec) {
  const baseline = spec.baselineHistory;
  assert(baseline, "pre-run owner history was captured before the tested send");
  assert.equal(baseline.session_id, spec.accepted.session_id, "pre-run actual owner session");
  assert.equal(baseline.offset, 0, "complete pre-run owner history"); assert.equal(baseline.has_more, false, "complete pre-run owner history");
  assert.equal(baseline.messages.length, baseline.message_count, "complete pre-run owner history");
  assert.equal(page.session_id, spec.accepted.session_id, "actual owner session");
  assert.equal(page.offset, 0, "complete owner history"); assert.equal(page.has_more, false, "complete owner history");
  assert.equal(page.messages.length, page.message_count, "complete owner history");
  assert.deepEqual(page.messages.slice(0, baseline.message_count), baseline.messages,
    "pre-run committed messages and canonical identities are unchanged and in place");
  if (spec.action === "cancel_after_boundary") {
    // A persistent run cancelled after its held tool's boundary commits its
    // rounds through that boundary, then the interruption notice, and nothing
    // after: no reply and no later tool call.
    const run = page.messages.slice(baseline.message_count).map(({ created_at, assistant_message_id, ...row }) => {
      assert.equal(typeof created_at, "string", "committed row has its commit time");
      if (row.role === "block_assistant") assert.equal(typeof assistant_message_id, "string", "committed assistant row has its message identity");
      else assert.equal(assistant_message_id, undefined, "only an assistant row carries a message identity");
      return row;
    });
    const notice = run.at(-1);
    assert.deepEqual(run.slice(0, -1), cancelledRunRounds(spec), "cancelled run commits its rounds through the held boundary, and nothing after");
    assert.deepEqual(Object.keys(notice ?? {}).sort(), ["body", "kind", "role"], "cancelled run ends with its interruption notice");
    assert.equal(notice.role, "system_notice", "cancelled run ends with its interruption notice");
    assert.equal(notice.kind, "generic", "cancelled run's interruption notice is the generic recovery notice");
    assert(notice.body.startsWith(INTERRUPTED_RUN_NOTICE_PREFIX), `cancelled run's interruption notice reads as an interrupted run: ${notice.body}`);
  } else {
    // The interrupt diagnostic keeps its earlier expectation that the run
    // commits nothing. On 0.9.5 an interrupt also commits its tail, with the
    // held call answered outcome-unknown, and the notice; the diagnostic is
    // excluded from release acceptance (hardInterruptDiagnostic).
    assert.equal(page.message_count, baseline.message_count, "pre-run committed message count is unchanged");
  }
  return page;
}

async function assertStoppedToolDisclosure(section, state, { fullReload = false } = {}) {
  const header = section.locator(".cc-tool-call__header").first();
  const expandedBefore = await header.getAttribute("aria-expanded");
  let openedAfterReload = false;
  // A fresh successful standalone card may start closed. Held/unknown calls,
  // failed groups, and the live success transition must retain their disclosure.
  if (fullReload && state === "success" && expandedBefore === "false"
    && await section.evaluate(node => node.classList.contains("cc-tool-call--success")
      && !node.classList.contains("cc-tool-call--group"))) {
    await header.click();
    openedAfterReload = true;
  }
  assert.equal(await header.getAttribute("aria-expanded"), "true", "held/stopped call stays disclosed");
  return { expandedBefore, openedAfterReload };
}

async function stoppedScenario(host, action) {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Use the coordinator's prebuilt fixture; this scenario must not invoke Cargo.");
  const stateDir = await fs.mkdtemp(path.join(os.tmpdir(), `meta-routine-${action}-`));
  const result = { host: host || "api", action, scenarioId: `routine-${action}-${randomUUID().slice(0, 8)}`, errors: [] };
  let fixture, browser, page, live, monitor;
  const viewport = () => page.locator(host === "stock" ? ".conv__body" : '[data-testid="shared-pane-0"] .cc-conversation-pane__scroll').first();
  const read = async url => { const response = await fetch(url); assert.equal(response.status, 200, await response.clone().text()); return response.json(); };
  const timeline = async () => (await read(`${fixture.baseUrl}/console/timeline?identity=router%3Amain&mode=recent&limit=1000`)).frames;
  const history = (sessionId = result.accepted.session_id) => read(`${fixture.backendUrl}/__fixture/session-history?session_id=${encodeURIComponent(sessionId)}`);
  const runControl = (next, runId = result.runId) => fixture.control("routine-run", { identity: "router:main", session_id: result.accepted.session_id, run_id: runId, action: next });
  const shot = async label => { await fs.mkdir(evidence, { recursive: true }); await page.screenshot({ path: path.join(evidence, `${host}-real-routine-${action}-${label}.png`), fullPage: true }); };
  async function open(reload = false) {
    await navigation(page, `${reload ? "reload" : "open"} ${host} routine ${action} host`, async () => {
      const response = reload ? await page.reload() : await page.goto(fixture.baseUrl + (host === "stock" ? "/console" : "/shared"));
      if (host === "stock" && !await page.locator(".conv__body").count()) await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: /router/i }).first().click();
      await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor(); await viewport().waitFor();
      return response;
    });
  }
  async function inspectTool(label, state, options) {
    const noResult = state !== "success";
    const late = stoppedTools(result.scenarioId).at(-1);
    const section = viewport().locator("section.cc-tool-call").filter({ hasText: "late-review.txt" }).last();
    await section.waitFor();
    assert.equal(await section.evaluate(node => Boolean(node.closest(".cc-completed-tools"))), false, "held/stopped call remains outside successful folding");
    const disclosure = await assertStoppedToolDisclosure(section, state, options);
    const detail = section.locator(".cc-tool-call__sub").filter({ hasText: '"path": "late-review.txt"' });
    const body = await detail.count() ? detail : section;
    const input = body.locator(".cc-tool-call__section").filter({ has: page.getByText("Input", { exact: true }) });
    assert.deepEqual(JSON.parse(await input.locator("pre").textContent()), late.args, "exact held call arguments");
    const output = body.locator(".cc-tool-call__section").filter({ has: page.getByText("Result", { exact: true }) });
    if (noResult) {
      assert.equal(await output.count(), 0, `${state} completion has no result body`);
      if (await detail.count()) {
        assert(await detail.locator(".cc-tool-call__peer-status--pending").count(), "exact held child has no success status");
        if (state === "unknown") assert(await detail.getByText("Completion unknown", { exact: true }).isVisible(), "exact interrupted child exposes its unknown completion");
      }
      else {
        assert(await section.evaluate(node => node.classList.contains("cc-tool-call--pending")), "exact incomplete call has no success status");
        if (state === "unknown") assert.match(await section.locator(".cc-tool-call__status").textContent(), /Completion unknown/);
      }
    } else assert.equal(await output.locator("pre").textContent(), late.result, "boundary cancellation retains real completed result");
    await section.getByRole("button", { name: "Copy", exact: true }).click();
    const copied = await page.evaluate(() => navigator.clipboard.readText());
    const expectedLate = `$ ${late.name}\nInput: ${JSON.stringify(late.args)}${noResult ? "" : `\nResult: ${late.result}`}`;
    const grouped = await detail.count() > 0;
    const missing = stoppedTools(result.scenarioId).find(tool => tool.step === "missing");
    const expectedCopy = grouped ? `$ ${missing.name}\nInput: ${JSON.stringify(missing.args)}\nResult: ${missing.result}\n${expectedLate}` : expectedLate;
    assert.equal(copied, expectedCopy, "clipboard preserves each exact source, with no invented cancellation/result text");
    assert.equal(await viewport().locator("details.cc-completed-tools").count(), 1, "only the actual initial successes fold");
    await shot(label); return { label, state, grouped, copied, disclosure };
  }
  try {
    fixture = await startFixture({ mode: "identity", routineTools: true, stateDir });
    if (host) {
      browser = await chromium.launch({ headless: true });
      const context = await browser.newContext({ viewport: { width: 1600, height: 1000 }, permissions: ["clipboard-read", "clipboard-write"] });
      monitor = browserFailureMonitor(context, { origin: fixture.baseUrl });
      result.errors = monitor.errors; result.expectedFailures = monitor.expected; result.requestFailures = monitor.failures;
      page = await context.newPage();
      page.on("response", response => { if (response.status() >= 400) result.errors.push({ url: response.url(), status: response.status() }); });
      await open();
    }
    const owner = await rpc(fixture.baseUrl, "meta/status_identity", { identity: "router:main" });
    assert.equal(owner.status, 200); assert.equal(owner.body.result?.identity, "router:main");
    result.baselineOwner = owner.body.result; assert(result.baselineOwner.session_id, "actual pre-run session owner");
    const sessionId = result.baselineOwner.session_id;
    const idle = () => fixture.control("routine-run", { identity: "router:main", session_id: sessionId, run_id: randomUUID(), action: "status" });
    await eventually(async () => (await idle()).current_run_id === null, "owner is idle before warmup");
    result.warmup = { sessionId, source: `Prior completed routine check ${result.scenarioId}.`, content: `Record the baseline for ${result.scenarioId}; no tools are needed.` };
    await fixture.control("model", { source: result.warmup.source, delay_ms: 0, chunk_chars: 256 });
    const warmed = await rpc(fixture.baseUrl, "meta/console/send", { identity: "router:main", content: result.warmup.content,
      origin: "console:routine-negative-warmup", origin_kind: "operator", handling_mode: "queue", idempotency_key: randomUUID() });
    assert.equal(warmed.status, 200); assert(warmed.body.result?.interaction_id, JSON.stringify(warmed.body)); result.warmup.accepted = warmed.body.result;
    result.baselineHistory = await eventually(async () => {
      const frames = await timeline(), page = await history(sessionId);
      result.warmup.proof = assertCompletedBaseline(page, frames, result.warmup);
      result.baselineIdle = await idle(); assert.equal(result.baselineIdle.current_run_id, null, "warmup owner is idle before baseline capture");
      return page;
    }, "exact warmup terminal, committed reply and idle owner precede cancellation baseline");
    const baseline = await read(`${fixture.baseUrl}/console/timeline?identity=router%3Amain&mode=recent&limit=1000`);
    live = await require("./real-routine-tools.cjs").capture(`${fixture.backendUrl}/console/timeline/stream?identity=router%3Amain&after=${encodeURIComponent(baseline.latest_cursor)}`);
    await eventually(() => { live.check(); return live.events.some(event => event.type === "snapshot_complete"); }, "subscriber connected before real tool dispatch");
    await fixture.control("model", { source: "Acknowledged.", delay_ms: 0, chunk_chars: 256, scenario: { kind: "routine", run_id: result.scenarioId } });
    result.sentContent = `[fixture:${result.scenarioId}] Inspect the release files, then hold the delayed review for cancellation.`;
    const sent = await rpc(fixture.baseUrl, "meta/console/send", { identity: "router:main", content: result.sentContent,
      origin: "console:routine-negative", origin_kind: "operator", handling_mode: "queue", idempotency_key: randomUUID() });
    assert.equal(sent.status, 200); assert(sent.body.result?.interaction_id, JSON.stringify(sent.body)); result.accepted = sent.body.result;
    assert.equal(result.accepted.session_id, sessionId, "tested cancellation uses the captured pre-run owner session");
    await eventually(async () => (await fixture.control("routine-tools", { action: "status" })).entered, "actual file dispatcher is held");
    const started = await eventually(() => live.frames().find(frame => frame.kind === "run_started" && frame.interaction_id === result.accepted.interaction_id), "exact accepted run started");
    result.runId = started.run_id; assert(result.runId && result.accepted.session_id);
    const lateId = stoppedTools(result.scenarioId).at(-1).id;
    await eventually(() => live.frames().some(frame => frame.kind === "tool_call_requested" && frame.payload.tool_call_id === lateId), "actual held call reached SSE");
    assert(!live.frames().some(frame => frame.kind === "tool_result_received" && frame.payload.tool_call_id === lateId));
    result.before = await runControl("status"); assert.equal(result.before.current_run_id, result.runId);
    result.stale = await runControl(action, randomUUID()); assert.equal(result.stale.accepted, false, "stale exact-run control cannot interrupt current work");
    assert.equal(result.stale.current_run_id, result.runId);
    if (host) result.pendingView = await inspectTool("held", "held");
    result.control = await runControl(action); assert.equal(result.control.accepted, true, "actual owner admitted exact-run control");
    if (action === "cancel_after_boundary") {
      result.waiting = await fixture.control("routine-tools", { action: "status" });
      assert.equal(result.waiting.completed, false); assert.equal(result.waiting.dropped, false);
      assert.equal((await runControl("status")).current_run_id, result.runId, "cooperative cancel waits for the held tool boundary");
      assert(!live.frames().some(frame => isCancelled(frame, result)), "cancel admission is not terminality");
      await fixture.control("routine-tools", { action: "release" });
    }
    await eventually(() => { live.check(); return live.frames().some(frame => isCancelled(frame, result)); }, "actual typed interaction cancellation reaches live SSE", 30_000);
    result.settled = await eventually(async () => { const current = await runControl("status"); return current.current_run_id === null ? current : null; }, "owner unbinds the stopped run");
    result.dispatch = await fixture.control("routine-tools", { action: "status" });
    assert.equal(result.dispatch.completed, action === "cancel_after_boundary");
    assert.equal(result.dispatch.dropped, action === "interrupt");
    result.live = await eventually(() => { live.check(); return assertStoppedFrames(live.frames(), result); }, "all exact owner cancellation carriers reach live SSE");
    // Releasing the old wait handle after interruption must not resurrect work.
    if (action === "interrupt") await fixture.control("routine-tools", { action: "release" });
    // The cancelled run's rounds and notice commit after its terminal frame and
    // its owner unbinding, so the durable owner is read until it holds them.
    result.ownerHistory = action === "cancel_after_boundary"
      ? await eventually(async () => { const page = await history(); assertStoppedHistory(page, result); return page; },
        "durable owner commits the cancelled run's rounds and its interruption notice")
      : await history();
    result.history = assertStoppedHistory(result.ownerHistory, result);
    result.ownerFrames = await timeline(); result.ownerProof = assertStoppedFrames(result.ownerFrames, result);
    assertProofRetained(result.live, result.ownerProof);
    result.replayFrames = (await snapshot(fixture.backendUrl, `?identity=router%3Amain&after=${encodeURIComponent(baseline.latest_cursor)}`)).flatMap(item => item.data.frame ? [item.data.frame] : []);
    result.replayProof = assertStoppedFrames(result.replayFrames, result); assertProofRetained(result.ownerProof, result.replayProof);
    result.postTerminalRetry = await runControl(action); assert.equal(result.postTerminalRetry.accepted, false, "terminal exact-run control is a no-op");
    if (host) {
      const state = action === "interrupt" ? "unknown" : "success";
      // Inspect the stopped transcript once it has settled. After the run's
      // terminal the owner's session history replays its rounds and ends
      // with the interruption notice; until that notice renders, the live
      // rows and the replay are still being reconciled.
      const settled = () => action === "cancel_after_boundary" ? viewport().getByText(INTERRUPTED_RUN_NOTICE_PREFIX).last().waitFor() : undefined;
      await settled(); result.stoppedView = await inspectTool("stopped", state);
      await open(true); await settled(); result.reloadedView = await inspectTool("reloaded", state, { fullReload: true });
      assert.deepEqual(assertStoppedHistory(await history(), result), result.history, "durable owner proof survives browser reload");
      result.reloadProof = assertStoppedFrames(await timeline(), result); assertProofRetained(result.replayProof, result.reloadProof);
      monitor.assertClean();
    }
  } catch (error) {
    result.failure = error.stack || String(error); if (page) await shot("failure").catch(() => {}); throw error;
  } finally {
    try {
      result.stream = live?.events; result.observations = fixture?.observations; result.logs = fixture?.logs();
      await fs.mkdir(evidence, { recursive: true }); await fs.writeFile(path.join(evidence, `${host || "api"}-real-routine-${action}.json`), JSON.stringify(result, null, 2));
      if (!result.failure) monitor?.assertClean();
    } finally {
      monitor?.stop();
      const cleanup = await Promise.allSettled([live?.close(), browser?.close(), fixture?.close()]);
      await fs.rm(stateDir, { recursive: true, force: true });
      const rejected = cleanup.find(item => item.status === "rejected");
      if (rejected && !result.failure) throw rejected.reason;
    }
  }
}
const hardInterruptDiagnostic = {
  reason: "Hard cancellation can finalize the runtime input without a terminal event, leaving the console Working. Durable receipt projection is deferred; this known failing repro is excluded from release acceptance.",
  issues: ["https://github.com/lukacf/meerkat-meta/issues/460", "https://github.com/lukacf/meerkat/issues/1233"],
};
const diagnosticMetadata = action => action === "interrupt" ? { diagnostic: hardInterruptDiagnostic } : {};
const apiScenarios = actions.map(action => ({ id: `api-routine-${action.replaceAll("_", "-")}`, family: "routine-tools", backend: "real", ...diagnosticMetadata(action), run: () => stoppedScenario(null, action) }));
const browserScenarios = ["stock", "shared"].flatMap(host => actions.map(action => ({ id: `real-${host}-routine-${action.replaceAll("_", "-")}`, family: "real-presentation", backend: "real", ...diagnosticMetadata(action), run: () => stoppedScenario(host, action) })));
module.exports = { apiScenarios, browserScenarios, assertStoppedFrames, assertStoppedHistory, assertCompletedBaseline, assertProofRetained, assertStoppedToolDisclosure, stoppedTools };
