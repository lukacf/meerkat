"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { createHash, randomUUID } = require("node:crypto");
const { chromium } = require("playwright");
const { startFixture, eventually, rpc, snapshot } = require("../acceptance-runtime.cjs");

const source = "## Release evidence\n\nKeep A\u030a, å and 🚀 exactly.\n\n| Check | Result |\n| --- | --- |\n| WorkGraph | Reviewed through the runtime |";
const identity = "router:main";
const assistantText = row => (row.blocks || []).filter(block => ["text", "transcript"].includes(block.block_type))
  .map(block => block.data?.text ?? "").join("");

function assertAssistantIdentity(history, frames, owner) {
  assert.equal(history.session_id, owner.session_id, "history session is the actual runtime session");
  assert.equal(history.has_more, false, "one complete canonical history read");
  const rows = history.messages.filter(row => row.role === "block_assistant"
    && row.identity?.run_id === owner.run_id && row.identity?.interaction_id === owner.interaction_id);
  assert.equal(rows.length, 3, "three assistant occurrences in the same real run");
  const ids = rows.map(row => row.assistant_message_id);
  assert(ids.every(id => typeof id === "string" && id.length > 0), "every canonical row has runtime identity");
  assert.equal(new Set(ids).size, 3, "byte-identical answers retain distinct occurrence identities");
  for (const row of rows) assert.equal(assistantText(row), source, "all three canonical answers preserve exact bytes");
  const live = frames.filter(frame => frame.source?.kind === "console_event" && frame.run_id === owner.run_id
    && frame.interaction_id === owner.interaction_id);
  assert.equal(new Set(live.map(frame => frame.id)).size, live.length, "live source events occur exactly once");
  for (const frame of live) assert.equal(frame.session_id, owner.session_id, "live session scope never aliases a fork");
  for (const id of ids) {
    const selected = live.filter(frame => frame.payload?.assistant_message_id === id);
    assert.equal(selected.filter(frame => frame.kind === "turn_started").length, 1, "one actual provider boundary per occurrence");
    assert.equal(selected.filter(frame => frame.kind === "text_delta").map(frame => frame.payload.delta).join(""), source,
      "live text joins exactly within its occurrence");
    assert.deepEqual(selected.filter(frame => frame.kind === "text_complete").map(frame => frame.payload.content), [source],
      "one exact text-complete candidate per occurrence");
  }
  const final = live.filter(frame => frame.kind === "interaction_complete" && frame.payload?.source_event_type === "run_completed");
  assert.equal(final.length, 1, "one real run completion");
  assert.equal(final[0].payload.assistant_message_id, ids[2], "final reference names the final occurrence, despite identical text");
  return ids;
}

function assertProvisionalPresentation(observation) {
  assert(observation?.streaming === true, "the actual Markdown document is streaming");
  assert(typeof observation.assistantMessageId === "string" && observation.assistantMessageId.length > 0,
    "the first live occurrence has an upstream identity");
  assert.equal(observation.firstTextComplete, false, "observed before the first occurrence finishes");
  assert(typeof observation.source === "string" && observation.source.length > 0
    && observation.source.length < source.length && source.startsWith(observation.source),
  "the visible document contains an exact partial prefix, not a completed history reply");
  assert(observation.documentId && observation.quoteId, "live Markdown has document and quote targets");
}

function assertProductionAssets(pageUrl, baseUrl, assets, expected) {
  assert.equal(new URL(pageUrl).origin, baseUrl, "production scenario stays on the fixture origin");
  assert.equal(new URL(pageUrl).pathname, "/console", "production scenario opens the shipping browser entry");
  for (const file of ["console-app.js", "console-app.css"]) {
    const actual = assets.filter(asset => new URL(asset.url).origin === baseUrl
      && new URL(asset.url).pathname === `/console/assets/${file}`);
    assert.equal(actual.length, 1, `browser loaded one production ${file}`);
    assert(!actual[0].error, JSON.stringify(actual[0]));
    assert.equal(actual[0].status, 200);
    assert.equal(actual[0].bytes, expected[file].bytes, `browser received the full production ${file}`);
    assert.equal(actual[0].sha256, expected[file].sha256, `browser received exact production ${file} bytes`);
    assert.equal(new URL(actual[0].url).searchParams.get("v"), expected[file].sha256.slice(0, 12),
      "production URL has the matching content version");
  }
}

async function captureLive(url) {
  const abort = new AbortController();
  const response = await fetch(url, { signal: abort.signal });
  assert.equal(response.status, 200);
  const events = [];
  let fault;
  const done = (async () => {
    const decoder = new TextDecoder();
    let buffer = "";
    for await (const chunk of response.body) {
      buffer += decoder.decode(chunk, { stream: true });
      let end;
      while ((end = buffer.indexOf("\n\n")) >= 0) {
        const block = buffer.slice(0, end); buffer = buffer.slice(end + 2);
        const data = block.split("\n").filter(line => line.startsWith("data:")).map(line => line.slice(5).trimStart()).join("\n");
        if (data) events.push(JSON.parse(data));
      }
    }
    if (!abort.signal.aborted) throw new Error("live identity stream ended unexpectedly");
  })().catch(error => { if (!abort.signal.aborted) fault = error; });
  return { events, check() { if (fault) throw fault; }, async close() { abort.abort(); await done; } };
}

function allowedNavigationAbort(request) {
  if (!request.failure()?.errorText?.includes("ERR_ABORTED")) return false;
  if (new URL(request.url()).pathname.endsWith("/timeline/stream")) return true;
  try {
    const body = JSON.parse(request.postData() || "{}");
    return body.method === "meta/console/query_timeline" && body.params?.mode === "recent" && !body.params?.identity;
  } catch { return false; }
}

async function assistantIdentity(host) {
  const hostRoute = { stock: "/scoped", shared: "/shared", embedded: "/console" }[host];
  assert(hostRoute, `Unknown assistant identity host: ${host}`);
  assert(process.env.META_EXAMPLE_BIN_DIR, "The identity scenario requires the coordinator's matching prebuilt fixture.");
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: process.env.META_HEADED !== "1" });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  const folder = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
  const result = { host, hostRoute, source, errors: [], expectedCancellations: [], navigations: [], rendered: {} };
  const assetResponses = [];
  const draft = "Next, compare these three observations. Preserve A\u030a and 🚀.";
  const marker = `identity-${randomUUID().slice(0, 8)}`;
  let allowance = "navigation", live;
  const pane = () => host === "shared" ? page.getByTestId("shared-pane-0") : page.getByTestId(`chat-pane:${identity}`).first();
  const viewport = () => pane().locator(host === "shared" ? ".cc-conversation-pane__scroll" : ".conv__body");
  const composer = () => host === "shared" ? pane().getByRole("textbox", { name: "Message", exact: true }) : pane().getByTestId(`chat-composer:${identity}`);
  page.on("pageerror", error => result.errors.push(error.message));
  page.on("requestfailed", request => {
    const detail = { url: request.url(), error: request.failure()?.errorText };
    if ((allowance === "navigation" && allowedNavigationAbort(request))
      || (allowance === "disconnect" && new URL(request.url()).pathname.endsWith("/timeline/stream"))) result.expectedCancellations.push(detail);
    else result.errors.push(detail);
  });
  page.on("response", response => { if (response.status() >= 400) result.errors.push({ url: response.url(), status: response.status() }); });
  if (host === "embedded") {
    // Observe the actual browser responses from the production route. A second
    // fetch would not establish which asset bytes rendered these occurrences.
    page.on("response", response => {
      const url = new URL(response.url());
      if (url.origin !== fixture.baseUrl || !["/console/assets/console-app.js", "/console/assets/console-app.css"].includes(url.pathname)) return;
      assetResponses.push(response.body().then(bytes => ({
        url: response.url(), status: response.status(), bytes: bytes.length,
        sha256: createHash("sha256").update(bytes).digest("hex"),
      })).catch(error => ({ url: response.url(), error: String(error) })));
    });
  }
  async function read(url) { const response = await fetch(url); assert.equal(response.status, 200); return response.json(); }
  const timeline = () => read(`${fixture.baseUrl}/console/timeline?identity=router%3Amain&mode=recent&limit=1000`);
  async function open(reload = false) {
    allowance = "navigation";
    const firstAsset = assetResponses.length;
    if (reload) await page.reload(); else await page.goto(fixture.baseUrl + hostRoute);
    result.navigations.push({ reload, requestedUrl: fixture.baseUrl + hostRoute, actualUrl: page.url() });
    assert.equal(new URL(page.url()).origin, fixture.baseUrl, "identity scenario stays on the fixture origin");
    assert.equal(new URL(page.url()).pathname, hostRoute, "identity scenario opens the selected host route");
    if (host !== "shared" && !await pane().count()) await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: /router:main/ }).first().click();
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    await composer().waitFor(); allowance = null;
    if (host === "embedded") {
      const loadedAssets = await Promise.all(assetResponses.slice(firstAsset));
      const expected = Object.fromEntries(await Promise.all(["console-app.js", "console-app.css"].map(async file => {
        const bytes = await fs.readFile(path.join(__dirname, "../dist", file));
        return [file, { sha256: createHash("sha256").update(bytes).digest("hex"), bytes: bytes.length }];
      })));
      (result.productionAssetChecks ||= []).push({ reload, pageUrl: page.url(), loadedAssets, expected });
      assertProductionAssets(page.url(), fixture.baseUrl, loadedAssets, expected);
    }
  }
  async function inspect(label) {
    const rows = await eventually(async () => {
      const docs = viewport().locator(".cc-markdown-document").filter({ has: page.getByRole("heading", { name: "Release evidence", exact: true }) });
      if (await docs.count() !== 3) return null;
      const value = await docs.evaluateAll(nodes => nodes.map(node => {
        const quote = node.closest("[data-quote-message-id]");
        return { documentId: node.dataset.markdownDocumentId, quoteId: quote?.dataset.quoteMessageId,
          source: quote?.dataset.quoteSource, rowId: node.closest("[data-conversation-row-id]")?.dataset.conversationRowId,
          tables: node.querySelectorAll("table").length };
      }));
      assert.equal(new Set(value.map(row => row.documentId)).size, 3, "three separate Markdown documents");
      assert(value.every(row => row.quoteId && row.rowId), "every occurrence has a quote and reading target");
      assert.equal(new Set(value.map(row => row.quoteId)).size, 3, "identical replies have distinct quote targets");
      for (const row of value) { assert.equal(row.source, source, "quote source remains byte exact"); assert.equal(row.tables, 1); }
      return value;
    }, `${host} ${label}: three exact separately addressable replies`);
    result.rendered[label] = rows;
    await viewport().getByRole("heading", { name: "Release evidence", exact: true }).last().scrollIntoViewIfNeeded();
    await fs.mkdir(folder, { recursive: true });
    await page.screenshot({ path: path.join(folder, `${host}-assistant-identity-${label}.png`), fullPage: true });
    return rows;
  }
  try {
    await open();
    const baseline = await timeline(); assert(baseline.latest_cursor);
    live = await captureLive(`${fixture.backendUrl}/console/timeline/stream?identity=router%3Amain&after=${encodeURIComponent(baseline.latest_cursor)}`);
    await eventually(() => { live.check(); return live.events.some(event => event.type === "snapshot_complete"); }, "live capture is ready before the operator send");
    await fixture.control("model", { source: "Unexpected fallback response.", delay_ms: 80, chunk_chars: 8,
      scenario: { kind: "assistant_identity", run_id: marker } });
    const sent = await rpc(fixture.baseUrl, "meta/console/send", { identity, origin: "console:identity-acceptance", origin_kind: "operator",
      idempotency_key: randomUUID(), content: `[fixture:${marker}] Review the release twice through the WorkGraph, then repeat your conclusion.` });
    assert.equal(sent.status, 200); assert(!sent.body.error, JSON.stringify(sent.body));
    result.accepted = sent.body.result; assert(result.accepted?.interaction_id && result.accepted?.input_frame_id);
    await composer().fill(draft); await composer().evaluate(node => node.setSelectionRange(6, 13));
    const firstOccurrence = () => live.events.find(event => event.frame?.interaction_id === result.accepted.interaction_id
      && event.frame.kind === "turn_started")?.frame.payload?.assistant_message_id;
    const firstTextComplete = () => live.events.some(event => event.frame?.interaction_id === result.accepted.interaction_id
      && event.frame.kind === "text_complete" && event.frame.payload?.assistant_message_id === firstOccurrence());
    await eventually(async () => {
      live.check();
      if (firstTextComplete()) return { tooLate: true };
      const documents = viewport().locator('.cc-markdown-document[data-streaming="true"]')
        .filter({ has: page.getByRole("heading", { name: "Release evidence", exact: true }) });
      if (await documents.count() !== 1 || !firstOccurrence()) return null;
      const visible = await documents.evaluateAll(nodes => nodes.map(node => ({
        streaming: node.dataset.streaming === "true", documentId: node.dataset.markdownDocumentId,
        source: node.closest("[data-quote-message-id]")?.dataset.quoteSource,
        quoteId: node.closest("[data-quote-message-id]")?.dataset.quoteMessageId,
      })));
      if (visible.length !== 1) return null;
      result.livePresentation = { ...visible[0], assistantMessageId: firstOccurrence(), firstTextComplete: firstTextComplete() };
      return result.livePresentation;
    }, "the real browser shows a partial first reply before its text-complete event");
    assertProvisionalPresentation(result.livePresentation);
    await fs.mkdir(folder, { recursive: true });
    await page.screenshot({ path: path.join(folder, `${host}-assistant-identity-streaming-1600.png`), fullPage: true });
    await eventually(() => { live.check(); return live.events.some(event => event.frame?.interaction_id === result.accepted.interaction_id
      && event.frame.kind === "interaction_complete" && event.frame.payload?.source_event_type === "run_completed"); }, "one real tool-loop run finishes on the live stream");
    result.liveFrames = live.events.flatMap(event => event.frame ? [event.frame] : []);
    result.owner = result.liveFrames.find(frame => frame.kind === "run_started" && frame.interaction_id === result.accepted.interaction_id);
    assert(result.owner?.session_id && result.owner?.run_id);
    result.history = await read(`${fixture.backendUrl}/__fixture/session-history?session_id=${encodeURIComponent(result.owner.session_id)}`);
    result.ids = assertAssistantIdentity(result.history, result.liveFrames, result.owner);
    const expectedToolIds = ["identity-first", "identity-second"].map(step => `fixture-${marker}-${step}`);
    const tools = result.history.messages.filter(row => row.role === "tool_results").flatMap(row => row.results)
      .filter(tool => expectedToolIds.includes(tool.tool_use_id));
    assert.deepEqual(tools.map(tool => tool.tool_use_id).sort(), [...expectedToolIds].sort(),
      "both distinct actual runtime tool calls commit exactly one result");
    for (const tool of tools) assert.equal(tool.is_error, false);
    for (const [index, id] of result.ids.entries()) {
      const row = result.history.messages.find(message => message.assistant_message_id === id);
      assert.deepEqual(row.blocks.filter(block => block.block_type === "tool_use").map(block => block.data.id),
        index < 2 ? [expectedToolIds[index]] : [], "each assistant occurrence retains its own tool call");
    }
    result.timeline = await eventually(async () => {
      const current = await timeline();
      const canonical = current.frames.filter(frame => frame.source?.kind === "session_history" && frame.session_id === result.owner.session_id
        && frame.payload?.message?.role === "block_assistant" && result.ids.includes(frame.payload.message.assistant_message_id));
      if (canonical.length !== 3) return null;
      for (const frame of canonical) assert.equal(frame.payload.assistant_message_id, frame.payload.message.assistant_message_id, "history projection retains exact containing occurrence");
      return current;
    }, "all three canonical occurrences reach the real console projection");
    const currentDraft = await composer().evaluate(node => ({ value: node.value, start: node.selectionStart, end: node.selectionEnd }));
    assert.deepEqual(currentDraft, { value: draft, start: 6, end: 13 }, "three streamed replies preserve the draft and selection");
    const before = await inspect("completed-1600");
    result.replay = await snapshot(fixture.backendUrl, "?identity=router%3Amain");
    const replayFrames = result.replay.flatMap(event => event.data?.frame ? [event.data.frame] : []);
    for (const id of result.ids) assert(replayFrames.some(frame => frame.payload?.assistant_message_id === id), "actual SSE replay carries each occurrence");
    allowance = "disconnect";
    const streams = () => fixture.observations.filter(item => item.path.includes("/timeline/stream") && item.status === 200).length;
    const prior = streams(); fixture.disconnectStreams();
    await eventually(() => streams() > prior, "browser reconnects through a real stream");
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor(); allowance = null;
    assert.deepEqual(await inspect("reconnected-1600"), before, "canonical document, quote and reading targets survive reconnect");
    await open(true);
    assert.deepEqual(await inspect("reloaded-1600"), before, "canonical occurrence targets survive reload");
    await page.setViewportSize({ width: 1440, height: 900 }); await inspect("reloaded-1440");
    assert.deepEqual(result.errors, [], "no unexpected browser or API failures");
  } catch (error) {
    result.failure = error.stack || String(error);
    await fs.mkdir(folder, { recursive: true });
    await page.screenshot({ path: path.join(folder, `${host}-assistant-identity-failure.png`), fullPage: true }).catch(() => {});
    throw error;
  } finally {
    result.observations = fixture.observations; result.logs = fixture.logs();
    result.liveEvents = live?.events;
    if (host === "embedded") result.loadedAssets = await Promise.all(assetResponses);
    await fs.mkdir(folder, { recursive: true });
    await fs.writeFile(path.join(folder, `${host}-assistant-identity.json`), JSON.stringify(result, null, 2));
    await live?.close(); await browser.close(); await fixture.close();
  }
}

const scenarios = ["stock", "shared", "embedded"].map(host => ({ id: `real-${host}-assistant-identity`, family: "real-presentation", backend: "real", run: () => assistantIdentity(host) }));
module.exports = { scenarios, assertAssistantIdentity, assertProvisionalPresentation, assertProductionAssets, source };
if (require.main === module) require("../scenario-registry.cjs").runScenarios(scenarios).catch(error => { process.stderr.write(`${error.stack || error}\n`); process.exitCode = 1; });
