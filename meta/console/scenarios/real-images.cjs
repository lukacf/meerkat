"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { createHash, randomUUID } = require("node:crypto");
const { chromium } = require("playwright");
const { startFixture, eventually, rpc } = require("../acceptance-runtime.cjs");
const evidence = path.join(__dirname, "../../output/playwright/console-acceptance");
const identity = "router:main";
const digest = bytes => createHash("sha256").update(bytes).digest("hex");

async function frames(fixture) {
  const response = await fetch(`${fixture.baseUrl}/console/timeline?identity=${encodeURIComponent(identity)}&mode=recent&limit=500`);
  assert.equal(response.status, 200);
  return (await response.json()).frames;
}
const pane = page => page.getByTestId(`chat-pane:${identity}`);
const transcript = page => pane(page).locator(".conv__body");
const textContent = content => typeof content === "string" ? content : (content || []).filter(block => block.type === "text").map(block => block.text).join("");
const interactionFrames = (current, accepted) => current.filter(frame => frame.interaction_id === accepted.interaction_id || frame.id === accepted.input_frame_id);

function browserErrors(page) {
  const errors = [], expected = [];
  let allowed = null;
  page.on("pageerror", error => errors.push(error.message));
  page.on("requestfailed", request => {
    const failure = { method: request.method(), url: request.url(), error: request.failure()?.errorText };
    if (allowed?.matches(request)) expected.push({ ...failure, reason: allowed.reason });
    else errors.push({ ...failure, payload: request.postData() });
  });
  return { errors, expected, async during(reason, action, matches) {
    assert.equal(allowed, null);
    allowed = { reason, matches };
    try { return await action(); } finally { allowed = null; }
  } };
}
function authorityCancellation(request) {
  if (!request.failure()?.errorText?.includes("ERR_ABORTED")) return false;
  if (new URL(request.url()).pathname.endsWith("/timeline/stream")) return true;
  try {
    const body = JSON.parse(request.postData() || "{}");
    return body.method === "meta/console/query_timeline" && body.params?.mode === "recent" && body.params?.limit === 200 && !body.params?.identity;
  } catch { return false; }
}
async function openStock(page, fixture, monitor, reload = false) {
  return monitor.during(reload ? "explicit image scenario reload" : "initial host authority initialization", async () => {
    if (reload) await page.reload(); else await page.goto(fixture.baseUrl + "/console");
    if (!await pane(page).count()) await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ has: page.getByText(identity, { exact: true }) }).first().click();
    await pane(page).getByTestId(`chat-composer:${identity}`).waitFor();
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
  }, authorityCancellation);
}
function requestSendPayload(request) {
  if (request.method() !== "POST") return null;
  let payload = request.postData() || "";
  try {
    if (new URL(request.url()).pathname.endsWith("/multipart") || request.headers()["content-type"]?.includes("multipart/form-data")) {
      payload = payload.match(/name="payload"\r\n(?:[^\r\n]+\r\n)*\r\n([^\r\n]+)/)?.[1] || "";
    }
    const body = JSON.parse(payload);
    return body.method === "meta/console/send" ? body.params : null;
  } catch { return null; }
}
async function send(page, fixture, instruction) {
  const composer = pane(page).getByTestId(`chat-composer:${identity}`);
  await composer.fill(instruction);
  const observed = page.waitForResponse(async response => {
    const payload = requestSendPayload(response.request());
    if (payload) return payload.identity === identity && textContent(payload.content) === instruction;
    // Chromium omits file-upload postData from its request object. Bind that
    // response through its exact canonical accepted input, not an arbitrary POST.
    if (response.request().method() !== "POST" || !new URL(response.url()).pathname.endsWith("/rpc/multipart") || response.status() !== 200) return false;
    const body = await response.json();
    if (body.result?.identity !== identity || !body.result?.input_frame_id) return false;
    const input = (await frames(fixture)).find(frame => frame.id === body.result.input_frame_id && frame.interaction_id === body.result.interaction_id);
    return input?.kind === "user_input" && textContent(input.payload?.content) === instruction;
  });
  await composer.press("Enter");
  const response = await observed;
  assert.equal(response.status(), 200);
  const body = await response.json();
  assert(!body.error, JSON.stringify(body));
  assert(body.result?.interaction_id && body.result?.input_frame_id, JSON.stringify(body));
  return body.result;
}
function interactionCompletion(current, accepted, expectedText = "") {
  const scoped = interactionFrames(current, accepted);
  const failure = scoped.find(frame => ["interaction_failed", "message_delivery_failed"].includes(frame.kind)
    || frame.status === "delivery_failed"
    || (["text_complete", "interaction_complete"].includes(frame.kind) && JSON.stringify(frame.payload).includes("Acceptance scenario stopped:")));
  assert(!failure, `accepted interaction failed or stopped: ${JSON.stringify(failure)}`);
  return scoped.find(frame => frame.kind === "interaction_complete" && typeof frame.payload?.result === "string"
    && frame.payload.result.includes(expectedText)) || null;
}
async function completed(fixture, accepted, expectedText, timeout = 180_000) {
  const outcome = await eventually(async () => {
    const current = await frames(fixture);
    try {
      const terminal = interactionCompletion(current, accepted, expectedText);
      return terminal ? { terminal } : null;
    } catch (error) { return { failure: error }; }
  }, "accepted interaction completes successfully", timeout);
  if (outcome.failure) throw outcome.failure;
  return outcome.terminal;
}
function modelImageIngress(requests, instruction, base64) {
  for (const request of requests) for (const message of request.messages || []) {
    if (message.role === "user" && Array.isArray(message.content)
      && message.content.some(block => block.type === "text" && block.text === instruction)
      && message.content.some(block => block.type === "image" && block.media_type === "image/png" && block.data === base64)) return { request, message };
  }
  return null;
}
function parseToolResult(result) {
  const structured = Array.isArray(result.content) && result.content.find(block => block.type === "structured");
  return structured ? structured.data : JSON.parse(textContent(result.content || result.result));
}
function imageToolEvidence(requests, callId, blobId) {
  const matches = [];
  for (const [requestIndex, request] of requests.entries()) for (const message of request.messages || []) {
    if (message.role !== "tool_results") continue;
    for (const result of message.results || []) if (result.tool_use_id === callId) {
      assert.equal(result.is_error, false, "image tool result must succeed");
      const value = parseToolResult(result);
      assert.equal(value.terminal?.terminal, "generated", "image operation reaches generated terminal state");
      const image = value.images?.find(image => image.blob_ref?.blob_id === blobId);
      assert(image?.image_id && /^image\//.test(image.blob_ref.media_type), "successful tool result contains the exact committed blob");
      matches.push({ requestIndex, model: request.model, result, image });
    }
  }
  assert(matches.length, "real image tool result reaches a following model request");
  return matches;
}
const schedulingFields = ["due_at", "not_before", "snoozed_until"];

function assertUnscheduledGraphCalls(calls) {
  for (const { payload } of calls) {
    if (!["workgraph_create", "workgraph_update"].includes(payload?.name)) continue;
    for (const field of schedulingFields) {
      assert(payload.args?.[field] == null, `unscheduled task must not receive ${field} in ${payload.name}: ${JSON.stringify(payload.args)}`);
    }
  }
}

function assertOpenGraph(graph, ready) {
  const titles = ["Review diagram", "Review badge", "Publish report"];
  assert.equal(graph.items?.length, 3, "only the three requested tasks exist");
  const items = titles.map(title => {
    const matches = graph.items.filter(item => item.title === title);
    assert.equal(matches.length, 1, `one exact task named ${title}`);
    assert.equal(matches[0].status, "open", `${title} remains open`);
    assert.equal(matches[0].machine_state?.claim_owner_key, null, `${title} remains unclaimed`);
    for (const field of schedulingFields) {
      assert(matches[0][field] == null && matches[0].machine_state?.[`${field}_utc_ms`] == null,
        `${title} remains unscheduled: ${field}=${JSON.stringify(matches[0][field])}, machine=${JSON.stringify(matches[0].machine_state?.[`${field}_utc_ms`])}`);
    }
    return matches[0];
  });
  assert.equal(graph.edges?.length, 2, "only the two requested dependencies exist");
  for (const prerequisite of items.slice(0, 2)) assert(graph.edges.some(edge => edge.kind === "blocks"
    && edge.from_id === prerequisite.id && edge.to_id === items[2].id), `directed blocking prerequisite ${prerequisite.id}`);
  assert.deepEqual(ready.items.map(item => item.id).sort(), items.slice(0, 2).map(item => item.id).sort(), "both reviews are ready and publication is blocked");
  return items;
}
async function recordedRequests(fixture) {
  const response = await fetch(`${fixture.backendUrl}/__fixture/requests`);
  assert.equal(response.status, 200);
  return response.json();
}
async function ownerRpc(fixture, method, params) {
  const response = await rpc(fixture.baseUrl, method, params);
  assert.equal(response.status, 200); assert(!response.body.error, JSON.stringify(response.body));
  return response.body.result;
}
async function exactReply(page, source, current, accepted) {
  const ids = interactionFrames(current, accepted).map(frame => frame.id);
  const candidates = transcript(page).locator(".msg--agent [data-quote-source]");
  const index = await eventually(async () => {
    const sources = await candidates.evaluateAll(nodes => nodes.map(node => ({ source: node.dataset.quoteSource, id: node.dataset.quoteMessageId })));
    const matches = sources.flatMap((value, index) => value.source === source && ids.includes(value.id) ? [index] : []);
    assert(matches.length <= 1, "one rendered reply preserves the exact completed source");
    return matches.length === 1 ? { value: matches[0] } : null;
  }, "exact accepted reply is rendered");
  return candidates.nth(index.value);
}
async function show(page, locator) {
  // Viewport resize first reflows rows, then the reading anchor is restored by
  // ResizeObserver on the next frame. Navigate only after that layout settles.
  const settleLayout = () => page.evaluate(() => new Promise(resolve =>
    requestAnimationFrame(() => requestAnimationFrame(() => requestAnimationFrame(resolve)))));
  await settleLayout();
  const jump = pane(page).getByRole("button", { name: "Jump to latest", exact: true });
  if (await jump.isVisible()) await jump.click();
  await locator.scrollIntoViewIfNeeded();
  await settleLayout();
  const rect = await locator.evaluate(node => {
    const visible = node.closest(".conv__body").getBoundingClientRect(), rect = node.getBoundingClientRect();
    return { width: Math.max(0, Math.min(visible.right, rect.right, innerWidth) - Math.max(visible.left, rect.left, 0)), height: Math.max(0, Math.min(visible.bottom, rect.bottom, innerHeight) - Math.max(visible.top, rect.top, 0)) };
  });
  assert(rect.width >= 80 && rect.height >= 40, `exact content must be visible in transcript viewport: ${JSON.stringify(rect)}`);
  return rect;
}
async function capture(page, name, locator) {
  const intersection = await show(page, locator);
  await fs.mkdir(evidence, { recursive: true });
  await page.screenshot({ path: path.join(evidence, `${name}.png`), fullPage: true });
  return { viewport: page.viewportSize(), intersection };
}
async function exactImage(page, blobId) {
  const preview = transcript(page).locator(`img[src$="/blobs/${encodeURIComponent(blobId)}"]`);
  await preview.waitFor();
  assert.equal(await preview.count(), 1, "one preview represents the exact owner blob");
  await eventually(() => preview.evaluate(node => node.complete && node.naturalWidth > 0 && node.naturalHeight > 0), "exact owner image decodes");
  return preview;
}

async function diagram(browser) {
  const page = await browser.newPage({ viewport: { width: 520, height: 280 }, deviceScaleFactor: 1 });
  try {
    await page.setContent(`<html><body style="margin:0"><svg xmlns="http://www.w3.org/2000/svg" width="520" height="280" viewBox="0 0 520 280"><rect width="520" height="280" fill="#f5f3ee"/><g font-family="Arial" text-anchor="middle"><text x="260" y="34" font-size="18" fill="#202020">Release review dependencies</text><g stroke="#73808a" stroke-width="3" fill="none"><path d="M190 104 L310 170 M190 218 L310 182"/></g><rect x="30" y="66" width="160" height="68" rx="12" fill="#d8e8f6" stroke="#417ba6"/><text x="110" y="105" font-size="16">Source review</text><rect x="30" y="176" width="160" height="68" rx="12" fill="#e1ead1" stroke="#66894a"/><text x="110" y="216" font-size="16">Badge review</text><rect x="310" y="136" width="180" height="76" rx="12" fill="#fff" stroke="#ba5b40"/><text x="400" y="169" font-size="16">Publish report</text><text x="400" y="191" font-size="12" fill="#666">after both reviews</text></g></svg></body></html>`);
    return await page.screenshot();
  } finally { await page.close(); }
}
async function saveFailure(name, fixture, page, error, monitor) {
  await fs.mkdir(evidence, { recursive: true });
  await page.screenshot({ path: path.join(evidence, `${name}-failure.png`), fullPage: true }).catch(() => {});
  const read = async action => { try { return await action(); } catch (error) { return { error: String(error) }; } };
  await fs.writeFile(path.join(evidence, `${name}-failure.json`), JSON.stringify({ error: error.stack || String(error),
    html: await read(() => page.content()), frames: await read(() => frames(fixture)), requests: await read(() => recordedRequests(fixture)),
    logs: fixture.logs(), observations: fixture.observations, errors: monitor.errors, expectedFailures: monitor.expected }, null, 2));
}

async function uploadImage() {
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  const monitor = browserErrors(page);
  try {
    const bytes = await diagram(browser);
    await fs.mkdir(evidence, { recursive: true });
    await fs.writeFile(path.join(evidence, "upload-release-diagram.png"), bytes);
    const source = "## Diagram received\n\nThe uploaded image reached the real runtime. Its bytes are verified against the recorded model request.\n\nThe source review and badge review are independent prerequisites for publication.";
    await fixture.control("model", { source, delay_ms: 0, chunk_chars: 32 });
    await openStock(page, fixture, monitor);
    const composer = pane(page).getByTestId(`chat-composer:${identity}`);
    await composer.evaluate((node, data) => {
      const transfer = new DataTransfer();
      transfer.items.add(new File([Uint8Array.from(data)], "release-diagram.png", { type: "image/png" }));
      node.dispatchEvent(new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: transfer }));
    }, Array.from(bytes));
    const staged = pane(page).locator(".composer__attachment img");
    await staged.waitFor();
    assert(await staged.evaluate(node => node.complete && node.naturalWidth === 520), "staged upload decodes");
    await page.screenshot({ path: path.join(evidence, "stock-upload-staged.png"), fullPage: true });
    const instruction = `Inspect the attached release dependency diagram. [upload:${randomUUID()}]`;
    const accepted = await send(page, fixture, instruction);
    const ingress = await eventually(async () => modelImageIngress(await recordedRequests(fixture), instruction, bytes.toString("base64")), "same actual user message carries instruction and exact image bytes");
    const terminal = await completed(fixture, accepted, source);
    const input = (await frames(fixture)).find(frame => frame.id === accepted.input_frame_id);
    assert.equal(input?.kind, "user_input");
    assert.equal(input?.interaction_id, accepted.interaction_id);
    assert.equal(textContent(input.payload.content), instruction, "accepted image input retains the exact authored instruction");
    const image = input.payload.content.find(block => block.type === "image");
    assert(image?.blob_id, JSON.stringify(input));
    const response = await fetch(`${fixture.baseUrl}/blobs/${encodeURIComponent(image.blob_id)}`);
    assert.equal(response.status, 200); assert.match(response.headers.get("content-type"), /^image\/png/);
    assert.equal(digest(Buffer.from(await response.arrayBuffer())), digest(bytes));
    const displayed = await exactImage(page, image.blob_id);
    const sourceRow = await displayed.evaluate(node => {
      const row = node.closest("[data-conversation-row-id]");
      const quote = node.closest("[data-quote-source]");
      return { id: row?.dataset.conversationRowId, sourceFrameId: quote?.dataset.quoteMessageId, source: quote?.dataset.quoteSource };
    });
    assert(sourceRow.id, "uploaded image has a visual row identity");
    assert.equal(sourceRow.sourceFrameId, input.id, "uploaded image quote identifies the exact accepted input frame");
    assert(sourceRow.source?.includes(instruction) && sourceRow.source.includes(image.blob_id), "uploaded image source retains the exact instruction and committed blob identity");
    const imageRow = displayed.locator('xpath=ancestor::*[@data-conversation-row-id][1]');
    assert.equal(await imageRow.getByText(instruction, { exact: true }).count(), 1, "uploaded image shares its row with the exact authored instruction");
    assert.equal(await transcript(page).locator("[data-conversation-row-id]").evaluateAll((nodes, id) => nodes.filter(node => node.dataset.conversationRowId === id).length, sourceRow.id), 1,
      "uploaded image visual row identity occurs exactly once");
    assert(await displayed.evaluate(node => node.naturalWidth === 520 && node.naturalHeight === 280));
    await exactReply(page, terminal.payload.result, await frames(fixture), accepted);
    const captures = [await capture(page, "stock-upload-committed", displayed)];
    await page.setViewportSize({ width: 1024, height: 768 });
    captures.push(await capture(page, "stock-upload-committed-1024", displayed));
    await openStock(page, fixture, monitor, true);
    const restored = await exactImage(page, image.blob_id);
    assert(await restored.evaluate(node => node.naturalWidth === 520 && node.naturalHeight === 280), "exact upload decodes after reload");
    const restoredRow = await restored.evaluate(node => {
      const row = node.closest("[data-conversation-row-id]");
      const quote = node.closest("[data-quote-source]");
      return { id: row?.dataset.conversationRowId, sourceFrameId: quote?.dataset.quoteMessageId, source: quote?.dataset.quoteSource };
    });
    assert.equal(restoredRow.id, sourceRow.id, "reloaded upload retains its initial visual row identity");
    assert.equal(restoredRow.sourceFrameId, input.id, "reloaded upload quote identifies the exact accepted input frame");
    assert.equal(restoredRow.source, sourceRow.source, "reloaded upload retains the exact initial instruction and image source");
    assert.equal(await restored.locator('xpath=ancestor::*[@data-conversation-row-id][1]').getByText(instruction, { exact: true }).count(), 1,
      "reloaded image shares its row with the exact authored instruction");
    assert.equal(await transcript(page).locator("[data-conversation-row-id]").evaluateAll((nodes, id) => nodes.filter(node => node.dataset.conversationRowId === id).length, sourceRow.id), 1,
      "reloaded upload retains its initial visual row identity exactly once");
    captures.push(await capture(page, "stock-upload-restored", restored));
    assert.deepEqual(monitor.errors, []);
    await fs.writeFile(path.join(evidence, "image-upload-evidence.json"), JSON.stringify({ sha256: digest(bytes), blobId: image.blob_id, accepted, inputFrameId: input.id, sourceRow, restoredRow, terminal,
      model: ingress.request.model, exactModelBytesInSameUserMessage: true, captures, errors: monitor.errors, expectedFailures: monitor.expected }, null, 2));
  } catch (error) { await saveFailure("image-upload", fixture, page, error, monitor); throw error; }
  finally { await browser.close(); await fixture.close(); }
}

async function liveImage() {
  assert(process.env.RKAT_OPENAI_API_KEY || process.env.OPENAI_API_KEY, "OpenAI image generation requires configured credentials");
  const fixture = await startFixture({ liveImages: true });
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  const monitor = browserErrors(page);
  try {
    await openStock(page, fixture, monitor);
    const runId = `badge-${randomUUID().slice(0, 8)}`;
    const callId = `fixture-${runId}-generate-image`;
    await fixture.control("model", { source: "Acknowledged.", scenario: { kind: "image", run_id: runId, image_provider: "openai", image_prompt: "Create a simple square release badge illustration: a blue shipping crate with a small green check on a warm cream background. Clean flat vector appearance, no text, no watermark." } });
    const accepted = await send(page, fixture, `[fixture:${runId}] Generate a release badge after the workgraph reviews.`);
    const terminal = await completed(fixture, accepted, "Image operation complete", 240_000);
    const current = await frames(fixture);
    const scoped = interactionFrames(current, accepted);
    const calls = scoped.filter(frame => frame.kind === "tool_call_requested" && frame.payload?.tool_call_id === callId);
    assert.equal(calls.length, 1, "the accepted interaction invokes generate_image exactly once");
    assert.equal(calls[0].payload.name, "generate_image");
    const committed = current.find(frame => ["assistant_image", "assistant_image_appended"].includes(frame.kind) && frame.payload?.tool_call_id === callId && frame.payload?.blob_id);
    assert(committed, "committed image is bound to the accepted interaction's exact tool call");
    const blobId = committed.payload.blob_id;
    const toolResults = imageToolEvidence(await recordedRequests(fixture), callId, blobId);
    const response = await fetch(`${fixture.baseUrl}/blobs/${encodeURIComponent(blobId)}`);
    assert.equal(response.status, 200); assert.match(response.headers.get("content-type"), /^image\//);
    const bytes = Buffer.from(await response.arrayBuffer());
    assert(bytes.length > 1000, "provider image is substantial binary content");
    await exactReply(page, terminal.payload.result, current, accepted);
    const preview = await exactImage(page, blobId);
    const dimensions = await preview.evaluate(node => ({ width: node.naturalWidth, height: node.naturalHeight }));
    const captures = [await capture(page, "stock-live-generated-image", preview)];
    await page.setViewportSize({ width: 1024, height: 768 });
    captures.push(await capture(page, "stock-live-generated-image-1024", preview));
    await openStock(page, fixture, monitor, true);
    const restored = await exactImage(page, blobId);
    assert.deepEqual(await restored.evaluate(node => ({ width: node.naturalWidth, height: node.naturalHeight })), dimensions, "exact generated blob decodes identically after reload");
    captures.push(await capture(page, "stock-live-generated-image-restored", restored));
    interactionCompletion(await frames(fixture), accepted, "Image operation complete");
    assert.deepEqual(monitor.errors, []);
    await fs.writeFile(path.join(evidence, "live-image-evidence.json"), JSON.stringify({ provider: "openai", modelChoice: "scripted", imageExecutor: "real provider API", runId, accepted,
      blobId, sha256: digest(bytes), byteLength: bytes.length, dimensions, frame: committed, completed: terminal, toolCall: calls[0], toolResults, captures, errors: monitor.errors, expectedFailures: monitor.expected }, null, 2));
  } catch (error) { await saveFailure("live-image", fixture, page, error, monitor); throw error; }
  finally { await browser.close(); await fixture.close(); }
}

async function liveModel() {
  assert(process.env.RKAT_OPENAI_API_KEY || process.env.OPENAI_API_KEY, "live model requires configured credentials");
  // Turn-driven identities have no bootstrap LLM conversation; this paid lane
  // measures the explicit operator instruction and its actual tool results.
  const fixture = await startFixture({ liveModel: true, mode: "identity" });
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  const monitor = browserErrors(page);
  try {
    await openStock(page, fixture, monitor);
    const prompt = 'Use the WorkGraph tools in the authorized realm mob.console-acceptance to create exactly two independent tasks called Review diagram and Review badge, plus a third called Publish report. These are immediately available, unscheduled tasks. For each create call send only realm_id and title, for example {"realm_id":"mob.console-acceptance","title":"Review diagram"}. Do not send due_at, not_before or snoozed_until; absent scheduling is distinct from an epoch date or a distant-future date. Do not call workgraph_update. Link each review as a blocking prerequisite of Publish report. Read the ready set after creating both links. End with one Markdown table containing exactly these columns: Task, ID, Ready. Include all three tasks with exact tool-returned IDs; the Ready cells must be Yes or No from the ready set. Leave the tasks open; do not claim or close them. No other work is needed.';
    const accepted = await send(page, fixture, prompt);
    const terminal = await completed(fixture, accepted, "Publish report", 180_000);
    const graph = await ownerRpc(fixture, "meta/workgraph/snapshot", { include_terminal: true });
    const ready = await ownerRpc(fixture, "meta/workgraph/ready", {});
    const items = assertOpenGraph(graph.snapshot || graph, ready);
    const current = await frames(fixture), scoped = interactionFrames(current, accepted);
    const calls = scoped.filter(frame => frame.kind === "tool_call_requested");
    assertUnscheduledGraphCalls(calls);
    assert(!calls.some(frame => /^workgraph_(claim|close|release|reopen|abandon)$/.test(frame.payload?.name)), "model must not claim or close requested tasks");
    const readyCalls = calls.filter(frame => frame.payload?.name === "workgraph_ready");
    assert(readyCalls.length, "live model actually reads ready set");
    // The ready set is the tool's output, which travels on its result event; the
    // execution fact for the same call carries no body. A session-history row
    // keeps the body under the execution fact's kind.
    const readyResults = scoped.filter(frame => ["tool_result_received", "tool_execution_completed"].includes(frame.kind)
      && (frame.payload?.content !== undefined || frame.payload?.result !== undefined)
      && readyCalls.some(call => call.payload.tool_call_id === frame.payload.tool_call_id));
    assert(readyResults.some(frame => {
      assert.equal(frame.payload.is_error, false, "ready query succeeded");
      const value = parseToolResult(frame.payload);
      return JSON.stringify(value.items?.map(item => item.id).sort()) === JSON.stringify(items.slice(0, 2).map(item => item.id).sort());
    }), "live model received authoritative ready set after dependencies");
    const reply = await exactReply(page, terminal.payload.result, current, accepted);
    const table = reply.locator("table");
    assert.equal(await table.count(), 1, "exact accepted reply contains its requested table");
    assert.deepEqual(await table.locator("thead th").allTextContents(), ["Task", "ID", "Ready"]);
    const rows = await table.locator("tbody tr").evaluateAll(rows => rows.map(row => [...row.querySelectorAll("td")].map(cell => cell.textContent.trim())));
    assert.equal(rows.length, 3);
    for (const [index, item] of items.entries()) assert(rows.some(row => row[0] === item.title && row[1] === item.id && row[2] === (index < 2 ? "Yes" : "No")), `exact table row for ${item.title}`);
    const captures = [await capture(page, "stock-live-model-workgraph", table)];
    await page.setViewportSize({ width: 1024, height: 768 });
    captures.push(await capture(page, "stock-live-model-workgraph-1024", table));
    await openStock(page, fixture, monitor, true);
    const restoredReply = await exactReply(page, terminal.payload.result, await frames(fixture), accepted);
    captures.push(await capture(page, "stock-live-model-workgraph-restored", restoredReply.locator("table")));
    assert.deepEqual(monitor.errors, []);
    await fs.writeFile(path.join(evidence, "live-model-workgraph.json"), JSON.stringify({ provider: "openai", model: "gpt-5.5", accepted, graph, ready, terminal, calls, readyResults, tableRows: rows, captures,
      frames: current, errors: monitor.errors, expectedFailures: monitor.expected }, null, 2));
  } catch (error) { await saveFailure("live-model", fixture, page, error, monitor); throw error; }
  finally { await browser.close(); await fixture.close(); }
}
module.exports = {
  _oracles: { interactionCompletion, modelImageIngress, imageToolEvidence, assertOpenGraph, assertUnscheduledGraphCalls },
  browserScenarios: [{ id: "real-stock-image-upload", family: "real-images", backend: "real", run: uploadImage }],
  liveScenarios: [
    { id: "live-stock-image-generation", family: "live-provider", backend: "real-provider", run: liveImage },
    { id: "live-stock-model-workgraph", family: "live-provider", backend: "real-provider", run: liveModel },
  ],
};
