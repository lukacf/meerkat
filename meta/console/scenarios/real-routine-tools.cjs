"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const os = require("node:os");
const { randomUUID } = require("node:crypto");
const { chromium } = require("playwright");
const { startFixture, eventually, rpc, snapshot } = require("../acceptance-runtime.cjs");
const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
const answer = "Routine workspace review complete. Two initial reads succeeded, the missing file stayed visible, and the delayed review arrived intact.";
const notes = "  Release notes\nKeep both spaces and this final newline.\n";
function expectedTools(runId) {
  return [
    ["notes", "read_file", { path: "release-notes.txt" }, notes, false],
    ["files", "list_files", {}, "late-review.txt\nrelease-notes.txt\n", false],
    ["missing", "read_file", { path: "missing.txt" }, "Cannot read missing.txt: NotFound", true],
    ["late", "read_file", { path: "late-review.txt" }, "Late review: all release artifacts match.\n", false],
    ["ready", "workgraph_ready", {}, '{"items":[]}', false],
  ].map(([step, name, args, result, error]) => ({ id: `fixture-${runId}-${step}`, step, name, args, result, error }));
}
function textContent(value) { return typeof value === "string" ? value : (value || []).filter(block => block.type === "text").map(block => block.text).join(""); }
function assertRoutineFrames(frames, runId, sourceKind) {
  const expected = expectedTools(runId);
  const owned = frames.filter(frame => frame.source?.kind === sourceKind && expected.some(tool => tool.id === frame.payload?.tool_call_id));
  assert.equal(new Set(owned.map(frame => frame.id)).size, owned.length, `${sourceKind}: distinct event IDs`);
  return expected.map(tool => {
    const calls = owned.filter(frame => frame.kind === "tool_call_requested" && frame.payload.tool_call_id === tool.id);
    const results = owned.filter(frame => frame.kind === (sourceKind === "console_event" ? "tool_result_received" : "tool_execution_completed") && frame.payload.tool_call_id === tool.id);
    assert.equal(calls.length, 1, `${sourceKind} ${tool.step}: one call`);
    assert.equal(results.length, 1, `${sourceKind} ${tool.step}: one result`);
    const call = calls[0], result = results[0];
    assert.equal(call.payload.name, tool.name);
    assert.deepEqual(call.payload.args, tool.args, `${sourceKind} ${tool.step}: exact arguments`);
    assert.equal(textContent(result.payload.content ?? result.payload.result), tool.result, `${sourceKind} ${tool.step}: exact result`);
    assert.equal(result.payload.is_error, tool.error, `${sourceKind} ${tool.step}: authoritative outcome`);
    if (sourceKind === "console_event") {
      assert.equal(call.source_event_id, call.id); assert.equal(result.source_event_id, result.id);
    }
    return { id: tool.id, callId: call.id, resultId: result.id, error: tool.error, result: tool.result };
  });
}
function assertRoutineHistory(page, runId, accepted, canonicalRunId) {
  assert.equal(page.session_id, accepted.session_id, "durable owner reads the accepted session");
  assert.equal(page.offset, 0, "complete owner history starts at the first message");
  assert.equal(page.has_more, false, "complete owner history has no unseen page");
  assert.equal(page.messages.length, page.message_count, "complete owner history includes every stored message");
  return expectedTools(runId).map(tool => {
    const calls = page.messages.flatMap((message, messageIndex) => message.role === "block_assistant"
      ? message.blocks.filter(block => block.block_type === "tool_use" && block.data.id === tool.id)
        .map(block => ({ messageIndex, identity: message.identity, call: block.data })) : []);
    const results = page.messages.flatMap((message, messageIndex) => message.role === "tool_results"
      ? message.results.filter(result => result.tool_use_id === tool.id).map(result => ({ messageIndex, result })) : []);
    assert.equal(calls.length, 1, `history ${tool.step}: one call`);
    assert.equal(results.length, 1, `history ${tool.step}: one result`);
    const call = calls[0], result = results[0];
    assert.equal(call.identity?.interaction_id, accepted.interaction_id, `history ${tool.step}: canonical interaction`);
    assert.equal(call.identity?.run_id, canonicalRunId, `history ${tool.step}: canonical run`);
    assert.equal(call.call.name, tool.name);
    assert.deepEqual(call.call.args, tool.args, `history ${tool.step}: exact arguments`);
    assert.equal(textContent(result.result.content), tool.result, `history ${tool.step}: exact result`);
    assert.equal(result.result.is_error, tool.error, `history ${tool.step}: authoritative outcome`);
    assert(result.messageIndex > call.messageIndex, `history ${tool.step}: result follows its own call`);
    return { id: tool.id, callMessageIndex: call.messageIndex, resultMessageIndex: result.messageIndex,
      identity: call.identity, error: result.result.is_error, result: tool.result };
  });
}
async function capture(url) {
  const abort = new AbortController(), events = []; let fault;
  const response = await fetch(url, { signal: abort.signal }); assert.equal(response.status, 200);
  const task = (async () => {
    const reader = response.body.getReader(), decoder = new TextDecoder(); let buffer = "";
    while (true) {
      const { value, done } = await reader.read(); buffer += decoder.decode(value, { stream: !done });
      let end;
      while ((end = buffer.indexOf("\n\n")) >= 0) {
        const block = buffer.slice(0, end); buffer = buffer.slice(end + 2);
        const data = block.split("\n").filter(line => line.startsWith("data:")).map(line => line.slice(5).trimStart()).join("\n");
        if (data) events.push(JSON.parse(data));
      }
      if (done) throw new Error("routine live stream ended early");
    }
  })().catch(error => { if (!abort.signal.aborted) fault = error; });
  return { events, check() { if (fault) throw fault; }, frames() { return events.flatMap(event => event.frame ? [event.frame] : []); }, async close() { abort.abort(); await task; } };
}
async function send(fixture, content) {
  const response = await rpc(fixture.baseUrl, "meta/console/send", { identity: "router:main", content,
    origin: "console:routine-acceptance", origin_kind: "operator", idempotency_key: randomUUID(), handling_mode: "queue" });
  assert(response.body.result?.interaction_id, JSON.stringify(response.body)); return response.body.result;
}
async function timeline(fixture) {
  const response = await fetch(`${fixture.baseUrl}/console/timeline?identity=router%3Amain&mode=recent&limit=1000`);
  assert.equal(response.status, 200); return response.json();
}
async function verifyRequests(fixture, runId) {
  const requests = await (await fetch(fixture.backendUrl + "/__fixture/requests")).json();
  const expected = expectedTools(runId);
  const request = requests.findLast(request => (request.messages || []).some(message => message.role === "tool_results" && message.results?.some(result => result.tool_use_id === expected.at(-1).id)));
  assert(request, "model continuation contains the final actual tool result");
  const calls = request.messages.flatMap(message => message.role === "block_assistant" ? message.blocks.filter(block => block.block_type === "tool_use").map(block => block.data) : []).filter(call => expected.some(tool => tool.id === call.id));
  const results = request.messages.flatMap(message => message.role === "tool_results" ? message.results : []).filter(result => expected.some(tool => tool.id === result.tool_use_id));
  assert.equal(calls.length, expected.length); assert.equal(results.length, expected.length);
  for (const tool of expected) {
    const call = calls.find(call => call.id === tool.id), result = results.find(result => result.tool_use_id === tool.id);
    assert(call && result); assert.equal(call.name, tool.name); assert.deepEqual(call.args, tool.args);
    assert.equal(result.is_error, tool.error); assert.equal(textContent(result.content), tool.result);
  }
  return { calls, results, request };
}
async function routineScenario(host) {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Use the coordinator's newly built routine-tools fixture, without invoking Cargo.");
  const stateDir = await fs.mkdtemp(path.join(os.tmpdir(), `meta-${host || "api"}-routine-`));
  const fixture = await startFixture({ mode: "identity", routineTools: true, stateDir });
  const result = { host: host || "api", runId: `routine-${randomUUID().slice(0, 8)}`, errors: [], checks: [] };
  let browser, page, live; let initializing = true;
  const viewport = () => page.locator(host === "stock" ? ".conv__body" : '[data-testid="shared-pane-0"] .cc-conversation-pane__scroll').first();
  async function open(reload = false) {
    initializing = true;
    if (reload) await page.reload(); else await page.goto(fixture.baseUrl + (host === "stock" ? "/console" : "/shared"));
    if (host === "stock" && !await page.locator(".conv__body").count()) await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: /router/i }).first().click();
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    await viewport().waitFor(); initializing = false;
  }
  async function screenshot(label) { await fs.mkdir(evidence, { recursive: true }); await page.screenshot({ path: path.join(evidence, `${host}-real-routine-tools-${label}.png`), fullPage: true }); }
  try {
    if (host) {
      browser = await chromium.launch({ headless: true });
      const context = await browser.newContext({ viewport: { width: 1600, height: 1000 }, permissions: ["clipboard-read", "clipboard-write"] });
      page = await context.newPage();
      page.on("pageerror", error => result.errors.push(error.message));
      page.on("requestfailed", request => {
        if (initializing && request.failure()?.errorText?.includes("ERR_ABORTED")) {
          if (new URL(request.url()).pathname.endsWith("/timeline/stream")) return;
          try {
            const body = JSON.parse(request.postData() || "{}");
            if (body.method === "meta/console/query_timeline" && body.params?.mode === "recent" && body.params?.limit === 200 && !body.params?.identity) return;
          } catch {}
        }
        result.errors.push({ url: request.url(), error: request.failure()?.errorText });
      });
      await open();
    }
    const seed = Array.from({ length: 18 }, (_, index) => `### Review checkpoint ${index + 1}\n\nThe candidate needs a careful review of the release notes, uploaded diagram, and dependency graph before the final report. Record the evidence and keep each conclusion explicit.\n\n`).join("");
    await fixture.control("model", { source: seed, delay_ms: 0, chunk_chars: 4096 });
    const seedAccepted = await send(fixture, "Keep this release review as the reading anchor while the workspace inspection runs.");
    await eventually(async () => (await timeline(fixture)).frames.some(frame => frame.kind === "interaction_complete" && frame.interaction_id === seedAccepted.interaction_id), "seed review completes");
    const baseline = await timeline(fixture);
    const streamUrl = `${fixture.backendUrl}/console/timeline/stream?identity=router%3Amain&after=${encodeURIComponent(baseline.latest_cursor)}`;
    live = await capture(streamUrl);
    await eventually(() => { live.check(); return live.events.some(event => event.type === "snapshot_complete"); }, "live subscriber connected before routine dispatch");
    await fixture.control("model", { source: "Acknowledged.", delay_ms: 0, chunk_chars: 256, scenario: { kind: "routine", run_id: result.runId } });
    result.accepted = await send(fixture, `[fixture:${result.runId}] Read the notes and file list, inspect the missing file, then wait for the delayed review.`);
    await eventually(async () => (await fixture.control("routine-tools", { action: "status" })).entered, "actual delayed file dispatcher entered");
    const expected = expectedTools(result.runId), late = expected.find(tool => tool.step === "late");
    await eventually(() => live.frames().some(frame => frame.kind === "tool_call_requested" && frame.payload.tool_call_id === late.id), "pending delayed call arrived over real live SSE");
    assert(!live.frames().some(frame => frame.kind === "tool_result_received" && frame.payload.tool_call_id === late.id), "running tool has no fabricated completion");
    result.pendingFrames = live.frames();
    let anchor;
    if (host) {
      const fold = viewport().locator("details.cc-completed-tools");
      await eventually(async () => await fold.count() === 1 && /2 completed tool calls/.test(await fold.textContent()), `${host}: adjacent successful routine tools fold`);
      assert.equal(await fold.evaluate(node => node.open), false, "following reader receives a collapsed completed group");
      const missing = viewport().locator("section.cc-tool-call").filter({ hasText: "Cannot read missing.txt: NotFound" }).last();
      await missing.waitFor(); assert(await missing.isVisible(), "real failed file read is visible");
      assert.equal(await missing.evaluate(node => Boolean(node.closest(".cc-completed-tools"))), false, "failure never enters successful fold");
      const pending = viewport().locator("section.cc-tool-call").filter({ hasText: "late-review.txt" }).last();
      await pending.waitFor();
      const pendingState = await pending.evaluate(node => {
        const child = [...node.querySelectorAll(".cc-tool-call__sub")].find(item => item.textContent.includes('"path": "late-review.txt"'));
        const target = child || node;
        return { pending: child ? Boolean(child.querySelector(".cc-tool-call__peer-status--pending")) : node.classList.contains("cc-tool-call--pending"),
          resultCount: [...target.querySelectorAll(".cc-tool-call__section-label")].filter(label => label.textContent === "Result").length };
      });
      assert.equal(pendingState.pending, true, "the exact delayed read stays pending even beside a failed sibling");
      assert.equal(pendingState.resultCount, 0, "pending delayed read has no fabricated result body");
      assert.equal(await pending.evaluate(node => Boolean(node.closest(".cc-completed-tools"))), false, "running tool stays outside successful fold");
      await screenshot("pending");
      await fold.locator("summary").click();
      for (const tool of expected.slice(0, 2)) {
        const section = fold.locator("section.cc-tool-call").filter({ has: page.locator(`.cc-tool-call__name[title="${tool.name}"]`) });
        await section.waitFor();
        if (await section.locator(".cc-tool-call__header").getAttribute("aria-expanded") !== "true") await section.locator(".cc-tool-call__header").click();
        const bodies = await section.locator("pre").allTextContents();
        const inputSection = section.locator(".cc-tool-call__section").filter({ has: page.getByText("Input", { exact: true }) });
        const resultSection = section.locator(".cc-tool-call__section").filter({ has: page.getByText("Result", { exact: true }) });
        assert.equal(await resultSection.locator("pre").textContent(), tool.result, "disclosure retains exact source result including whitespace");
        if (await inputSection.count()) {
          assert.deepEqual(JSON.parse(await inputSection.locator("pre").textContent()), tool.args, "disclosure retains exact argument values");
        } else {
          assert.deepEqual(tool.args, {}, "only an actually empty argument object may omit its visual input row");
        }
        await section.getByRole("button", { name: "Copy", exact: true }).click();
        const copied = await page.evaluate(() => navigator.clipboard.readText());
        assert.equal(copied, `$ ${tool.name}\nInput: ${JSON.stringify(tool.args)}\nResult: ${tool.result}`, "real clipboard preserves exact tool body including empty arguments");
        result.checks.push({ tool: tool.id, bodies, copied });
      }
      await screenshot("expanded");
      const retained = viewport().getByRole("heading", { name: "Review checkpoint 10", exact: true });
      await retained.scrollIntoViewIfNeeded();
      anchor = await retained.evaluate(node => {
        const scroll = node.closest(".conv__body, .cc-conversation-pane__scroll");
        scroll.scrollTop += node.getBoundingClientRect().top - scroll.getBoundingClientRect().top - 110;
        scroll.dispatchEvent(new Event("scroll"));
        return { text: node.textContent, before: node.getBoundingClientRect().top - scroll.getBoundingClientRect().top };
      });
      await page.waitForTimeout(150);
      anchor.before = await retained.evaluate(node => node.getBoundingClientRect().top - node.closest(".conv__body, .cc-conversation-pane__scroll").getBoundingClientRect().top);
      assert(await viewport().evaluate(node => node.scrollHeight - node.clientHeight - node.scrollTop > 100), "reader is away from latest before late completion");
    }
    await fixture.control("routine-tools", { action: "release" });
    await eventually(() => { live.check(); return live.frames().some(frame => frame.kind === "interaction_complete" && frame.interaction_id === result.accepted.interaction_id); }, "real tool sequence completes without healing query");
    result.live = assertRoutineFrames(live.frames(), result.runId, "console_event");
    if (host) {
      await eventually(async () => await viewport().getByText(answer, { exact: true }).count() === 1, "final source rendered once");
      await page.waitForTimeout(150);
      const after = await viewport().getByRole("heading", { name: anchor.text, exact: true }).evaluate(node => node.getBoundingClientRect().top - node.closest(".conv__body, .cc-conversation-pane__scroll").getBoundingClientRect().top);
      result.geometry = { ...anchor, after, drift: Math.abs(after - anchor.before) };
      assert(result.geometry.drift <= 2, `late completion/new detail retained reader: ${JSON.stringify(result.geometry)}`);
      await screenshot("reader-retained");
    }
    result.requests = await verifyRequests(fixture, result.runId);
    result.ownerFrames = (await timeline(fixture)).frames;
    result.ownerLive = assertRoutineFrames(result.ownerFrames, result.runId, "console_event");
    assert.deepEqual(result.live, result.ownerLive, "owner query preserves original live source event identities");
    const started = live.frames().filter(frame => frame.kind === "run_started" && frame.interaction_id === result.accepted.interaction_id);
    assert.equal(started.length, 1, "one actual typed runtime start for the accepted routine review");
    assert.equal(typeof started[0].run_id, "string");
    result.canonicalRunId = started[0].run_id;
    // Console history may legitimately omit a persisted counterpart already
    // represented by live source events. Read the durable session owner itself.
    result.ownerHistory = await eventually(async () => {
      const response = await fetch(`${fixture.backendUrl}/__fixture/session-history?session_id=${encodeURIComponent(result.accepted.session_id)}`);
      assert.equal(response.status, 200, "actual session owner read succeeds");
      const page = await response.json();
      assertRoutineHistory(page, result.runId, result.accepted, result.canonicalRunId);
      return page;
    }, "durable session owner contains every actual routine call and result");
    result.history = assertRoutineHistory(result.ownerHistory, result.runId, result.accepted, result.canonicalRunId);
    result.replayFrames = (await snapshot(fixture.backendUrl, `?identity=router%3Amain&after=${encodeURIComponent(baseline.latest_cursor)}`)).flatMap(item => item.data.frame ? [item.data.frame] : []);
    assert.deepEqual(assertRoutineFrames(result.replayFrames, result.runId, "console_event"), result.live, "actual SSE replay retains live call/result IDs once");
    if (host) {
      await open(true);
      await eventually(async () => await viewport().getByText(answer, { exact: true }).count() === 1, "reloaded final answer");
      const folds = viewport().locator("details.cc-completed-tools");
      assert.equal(await folds.count(), 1); assert.match(await folds.textContent(), /2 completed tool calls/);
      assert.equal(await viewport().locator(".cc-completed-tools").filter({ hasText: "workgraph_ready" }).count(), 0, "domain tool never enters routine fold");
      const missing = viewport().locator("section.cc-tool-call").filter({ hasText: "Cannot read missing.txt: NotFound" }).last();
      assert(await missing.isVisible(), "durable failed result remains exposed after reload");
      await viewport().getByText(answer, { exact: true }).scrollIntoViewIfNeeded();
      await screenshot("reloaded");
      assert.deepEqual(result.errors, [], "no unexpected browser errors");
    }
  } catch (error) {
    result.failure = error.stack || String(error);
    if (page) { result.html = await page.content(); await screenshot("failure").catch(() => {}); }
    throw error;
  } finally {
    result.stream = live?.events; result.observations = fixture.observations; result.logs = fixture.logs();
    await fs.mkdir(evidence, { recursive: true }); await fs.writeFile(path.join(evidence, `${host || "api"}-real-routine-tools.json`), JSON.stringify(result, null, 2));
    await live?.close(); await browser?.close(); await fixture.close(); await fs.rm(stateDir, { recursive: true, force: true });
  }
}
const apiScenarios = [{ id: "api-routine-tool-owner", family: "routine-tools", backend: "real", run: () => routineScenario(null) }];
const browserScenarios = ["stock", "shared"].map(host => ({ id: `real-${host}-routine-tools`, family: "real-presentation", backend: "real", run: () => routineScenario(host) }));
module.exports = { apiScenarios, browserScenarios, assertRoutineFrames, assertRoutineHistory, expectedTools, capture };
const negative = require("./real-routine-negative.cjs");
apiScenarios.push(...negative.apiScenarios);
browserScenarios.push(...negative.browserScenarios);
if (require.main === module) require("../scenario-registry.cjs").runScenarios([...apiScenarios, ...browserScenarios]).catch(error => { process.stderr.write(`${error.stack || error}\n`); process.exitCode = 1; });
