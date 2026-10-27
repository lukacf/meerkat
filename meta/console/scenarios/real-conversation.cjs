"use strict";
const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { chromium } = require("playwright");
const { randomUUID } = require("node:crypto");
const { startFixture, eventually, rpc } = require("../acceptance-runtime.cjs");

const evidence = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
const prose = index => `## Investigation ${index}\n\nThe reviewer checks the **candidate workgraph** and records the exact evidence. Repeated phrase, repeated phrase. Unicode: A\u030A, \u00e5 and \ud83d\ude80.\n\n- [x] Source inspected\n- [ ] Peer review pending\n\n| Check | Result |\n| --- | --- |\n| Runtime | Healthy |\n| Delivery | Confirmed |\n\n\`\`\`ts\nconst check = ${index};\n\`\`\`\n\n[Reference](https://example.com) and ![remote tracking image](https://example.com/tracker.png).\n\n`;

function browserErrors(page) {
  const errors = [];
  const expected = [];
  let allowed = null;
  page.on("pageerror", error => errors.push(error.message));
  page.on("requestfailed", request => {
    const failure = { method: request.method(), url: request.url(), error: request.failure()?.errorText };
    if (allowed?.matches(request)) expected.push({ ...failure, reason: allowed.reason });
    else errors.push(`${failure.method} ${failure.url}: ${failure.error} ${request.postData() || ""}`);
  });
  return {
    errors, expected,
    async during(reason, action, matches = request => new URL(request.url()).pathname.endsWith("/timeline/stream")) {
      assert.equal(allowed, null, "failure allowances must not overlap");
      allowed = { reason, matches };
      try { return await action(); } finally { allowed = null; }
    },
  };
}

async function settle(page) {
  await page.evaluate(() => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve))));
}

async function timeline(fixture) {
  const response = await fetch(`${fixture.baseUrl}/console/timeline?identity=router%3Amain&mode=recent&limit=1000`);
  assert.equal(response.status, 200);
  return response.json();
}

async function completed(fixture, source) {
  return eventually(async () => (await timeline(fixture)).frames?.find(frame =>
    frame.kind === "text_complete" && JSON.stringify(frame.payload).includes(source)), `completed text ${source.slice(0, 50)}`, 40_000);
}

async function anchorAt(viewport, row, offset = 100) {
  const id = await row.getAttribute("data-conversation-row-id");
  assert(id, "runtime transcript row has a stable id");
  await viewport.evaluate((node, { id, offset }) => {
    const row = [...node.querySelectorAll("[data-conversation-row-id]")].find(item => item.dataset.conversationRowId === id);
    node.scrollTop += row.getBoundingClientRect().top - node.getBoundingClientRect().top - offset;
    node.dispatchEvent(new Event("scroll"));
  }, { id, offset });
  await settle(viewport.page());
  return viewport.evaluate((node, id) => {
    const row = [...node.querySelectorAll("[data-conversation-row-id]")].find(item => item.dataset.conversationRowId === id);
    return { id, offset: row.getBoundingClientRect().top - node.getBoundingClientRect().top };
  }, id);
}

/// Hold a row at the reading position, 10 px above the viewport top, as the
/// anchor to measure. Through a prepend, a layout change or a return to the
/// conversation the console keeps the first row the viewport still shows in
/// place; a row further down moves whenever that row, or one between them,
/// grows. So the measured row has to be the held one: `row` once placed
/// there, or else the first later row that can be, since with too little
/// transcript above `row` the view reaches its top first. The anchor records
/// the index of its row among the mounted rows and, as `from`, that of `row`;
/// they differ when a later row is held.
async function holdReadingRow(viewport, row) {
  const rows = viewport.locator("[data-conversation-row-id]");
  const id = await row.getAttribute("data-conversation-row-id");
  const start = await rows.evaluateAll((items, id) => items.findIndex(item => item.dataset.conversationRowId === id), id);
  assert(start >= 0, `row ${id} is mounted in this transcript`);
  for (let index = start; index < start + 10 && index < await rows.count(); index += 1) {
    const anchor = await anchorAt(viewport, rows.nth(index), -10);
    if (Math.abs(anchor.offset + 10) <= 2) return { ...anchor, index, from: start };
  }
  assert.fail(`no row from ${id} on can be held at the reading position`);
}

const isOlderHistoryPage = (response) => {
  if (!response.url().endsWith("/console/rpc")) return false;
  const request = response.request().postDataJSON();
  return request?.method === "meta/console/query_timeline" && Boolean(request.params?.before);
};

/// Scroll the transcript to its start once, as a reader does. That scroll is
/// itself the stock pane's paging action (ChatPane onBodyScroll reveals
/// earlier turns or requests the next older page), so this waits for what it
/// started instead of racing it: history idle first, and when a page is due
/// (every revealed turn shown, older history left) the response to exactly
/// that request, then idle again. Returns the page response, or null.
async function scrollToStart(viewport) {
  const page = viewport.page();
  const loading = viewport.getByRole("button", { name: "Loading history", exact: true });
  const idle = () => eventually(async () => !(await loading.count()), "transcript history is idle");
  await idle();
  const loadOlder = viewport.getByRole("button", { name: "Load older history", exact: true });
  const reveal = viewport.getByRole("button", { name: "Show earlier messages", exact: true });
  const pageDue = !(await reveal.count()) && await loadOlder.count() > 0 && await loadOlder.first().isEnabled();
  const responsePromise = pageDue ? page.waitForResponse(isOlderHistoryPage) : null;
  await viewport.evaluate((node) => { node.scrollTop = 0; node.dispatchEvent(new Event("scroll")); });
  const response = responsePromise ? await responsePromise : null;
  if (response) await idle();
  await waitForMountedView(viewport);
  return response;
}

/// The stock transcript mounts only the turns near the viewport, so an early
/// row exists only once a reader scrolls back to it. Jump to the start in one
/// step, as Home does, and wait for the row itself to mount there.
async function mountEarlyRow(viewport, row) {
  // A parked turn (hidden until found) keeps its rows in the DOM, unseen.
  if (!await row.isVisible()) await scrollToStart(viewport);
  await row.waitFor();
}

/// Wait until no spacer or parked turn (hidden until found) overlaps the
/// viewport: the window has mounted every turn a reader can see here.
async function waitForMountedView(viewport) {
  await viewport.page().waitForFunction((node) => {
    const view = node.getBoundingClientRect();
    return [...node.querySelectorAll(":scope > [data-conversation-spacer], :scope > [data-conversation-turn-id][hidden]")].every((slot) => {
      const rect = slot.getBoundingClientRect();
      return rect.bottom <= view.top || rect.top >= view.bottom;
    });
  }, await viewport.elementHandle());
}

/// Read the whole transcript top to bottom, a viewport at a time, as a reader
/// scrolling through it does, and return `collect(node, arg)` for each
/// position. The stock transcript mounts only the turns near the viewport, so
/// an inventory of every row has to read it through; callers dedupe by row id
/// because neighbouring positions share rows, and skip rows of parked turns
/// (hidden until found), which are in the DOM but not rendered. The position
/// is restored after.
async function readThrough(viewport, collect, arg) {
  const start = await viewport.evaluate((node) => node.scrollTop);
  const scrollTo = async (top) => {
    const geometry = await viewport.evaluate((node, top) => {
      node.scrollTop = top;
      node.dispatchEvent(new Event("scroll"));
      return { end: Math.max(0, node.scrollHeight - node.clientHeight), step: node.clientHeight };
    }, top);
    await waitForMountedView(viewport);
    return geometry;
  };
  // Reaching the start may page older history; read only once nothing more
  // arrives above, so no row is prepended behind the read.
  while (await scrollToStart(viewport));
  const results = [];
  for (let top = 0; ;) {
    const { end, step } = await scrollTo(top);
    results.push(await viewport.evaluate(collect, arg));
    if (top >= end) break;
    top = Math.min(top + step, end);
  }
  await scrollTo(start);
  return results;
}

/// Wait until the controller has finished restoring ("Restoring earlier
/// position" gone) and the row's offset has held for `stableMs` of frames.
/// Returning to an identity reloads its history and the host may page older
/// history before the remembered row exists; rows can also reflow after the
/// row first appears. A fixed two-frame settle could measure mid-restore. A
/// real drift still settles at the wrong offset and fails.
async function settleRow(viewport, id, { stableMs = 500, timeoutMs = 8_000 } = {}) {
  await viewport.evaluate((node, { id, stableMs, timeoutMs }) => new Promise((resolve) => {
    const started = performance.now();
    let last = null;
    let since = started;
    const restoring = () => [...document.querySelectorAll('[role="status"]')]
      .some(status => status.textContent.includes("Restoring earlier position"));
    const tick = () => {
      const row = [...node.querySelectorAll("[data-conversation-row-id]")].find(item => item.dataset.conversationRowId === id);
      const top = row && !restoring() ? row.getBoundingClientRect().top - node.getBoundingClientRect().top : null;
      const now = performance.now();
      if (top !== last) { last = top; since = now; }
      if ((top !== null && now - since >= stableMs) || now - started >= timeoutMs) resolve();
      else requestAnimationFrame(tick);
    };
    requestAnimationFrame(tick);
  }), { id, stableMs, timeoutMs });
}



async function measureAnchor(viewport, anchor, action, label) {
  await settleRow(viewport, anchor.id);
  const before = await viewport.evaluate((node, id) => {
    const row = [...node.querySelectorAll("[data-conversation-row-id]")].find(item => item.dataset.conversationRowId === id);
    return { top: row.getBoundingClientRect().top - node.getBoundingClientRect().top, height: node.clientHeight, rows: node.querySelectorAll("[data-conversation-row-id]").length, text: row.textContent.slice(0, 150) };
  }, anchor.id);
  await action(); await settle(viewport.page());
  await settleRow(viewport, anchor.id);
  const after = await viewport.evaluate((node, id) => {
    const row = [...node.querySelectorAll("[data-conversation-row-id]")].find(item => item.dataset.conversationRowId === id);
    return { top: row?.getBoundingClientRect().top - node.getBoundingClientRect().top, height: node.clientHeight, rows: node.querySelectorAll("[data-conversation-row-id]").length,
      statuses: [...document.querySelectorAll('[role="status"]')].map(status => status.textContent.trim()).filter(text => /earlier position/i.test(text)),
      ...(row ? {} : { retainedRows: [...node.querySelectorAll("[data-conversation-row-id]")].slice(0, 12).map(item => ({ id: item.dataset.conversationRowId, text: item.textContent.slice(0, 150) })) }) };
  }, anchor.id);
  const drift = Math.abs(after.top - before.top);
  assert(Number.isFinite(drift) && drift <= 2, `${label} anchor drift: ${JSON.stringify({ before, after, drift })}`);
  return { label, anchor: anchor.id, before, after, drift };
}

function initializationCancellation(request) {
  if (!request.failure()?.errorText?.includes("ERR_ABORTED")) return false;
  if (new URL(request.url()).pathname.endsWith("/timeline/stream")) return true;
  try {
    const body = JSON.parse(request.postData() || "{}");
    return body.method === "meta/console/query_timeline" && body.params?.mode === "recent" && body.params?.limit === 200 && !body.params?.identity;
  } catch { return false; }
}

async function open(page, fixture, host, monitor) {
  if (monitor) return monitor.during("initial host authority initialization", () => open(page, fixture, host), initializationCancellation);
  await page.goto(fixture.baseUrl + (host === "shared" ? "/shared" : "/console"));
  if (host === "stock") {
    await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: /router/i }).first().click();
  }
  await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
}

const transcript = (page, host) => page.locator(host === "shared" ? '[data-testid="shared-pane-0"] .cc-conversation-pane__scroll' : ".conv__body").first();
const conversationPane = (page, host) => page.locator(host === "shared" ? '[data-testid="shared-pane-0"]' : ".conv").first();

async function inspectJumpTextClearance(viewport, jump, label) {
  const button = await jump.boundingBox();
  assert(button, `${label} latest control has rendered bounds`);
  const result = await viewport.evaluate((node, button) => {
    const viewport = node.getBoundingClientRect();
    const control = { left: button.x, right: button.x + button.width, top: button.y, bottom: button.y + button.height };
    const overlaps = [];
    let visibleTextRects = 0;
    const walker = document.createTreeWalker(node, NodeFilter.SHOW_TEXT);
    for (let text = walker.nextNode(); text; text = walker.nextNode()) {
      if (!text.textContent.trim()) continue;
      const element = text.parentElement;
      if (!element || element.closest("script, style, [aria-hidden=true]")) continue;
      const style = getComputedStyle(element);
      if (style.visibility === "hidden" || style.display === "none" || Number(style.opacity) === 0) continue;
      const range = document.createRange(); range.selectNodeContents(text);
      for (const rect of range.getClientRects()) {
        const left = Math.max(rect.left, viewport.left), right = Math.min(rect.right, viewport.right);
        const top = Math.max(rect.top, viewport.top), bottom = Math.min(rect.bottom, viewport.bottom);
        if (right <= left || bottom <= top) continue;
        visibleTextRects += 1;
        if (Math.min(right, control.right) > Math.max(left, control.left) + .5
          && Math.min(bottom, control.bottom) > Math.max(top, control.top) + .5) {
          overlaps.push({ text: text.textContent.slice(0, 100), rect: { left, right, top, bottom } });
        }
      }
    }
    const rail = node.parentElement.querySelector(".conv-turn-rail, .cc-conversation-turn-rail");
    const railBounds = rail && getComputedStyle(rail).visibility !== "hidden" ? rail.getBoundingClientRect().toJSON() : null;
    return { control, visibleTextRects, overlaps, railBounds };
  }, button);
  assert(result.visibleTextRects > 0, `${label} checks actual visible transcript text`);
  assert.deepEqual(result.overlaps, [], `${label} latest control covers readable text: ${JSON.stringify(result)}`);
  if (result.railBounds) assert(result.railBounds.bottom <= result.control.top,
    `${label} turn navigation leaves a separate slot for the latest control: ${JSON.stringify(result)}`);
  return result;
}

async function transcriptGeometry(viewport) {
  return viewport.evaluate(node => ({ width: node.clientWidth, height: node.clientHeight, scrollHeight: node.scrollHeight }));
}

async function inspectStockHeader(pane, label) {
  const result = await pane.locator(".conv__head").evaluate(head => {
    const box = node => node.getBoundingClientRect().toJSON();
    const title = head.querySelector(".conv__title");
    return { agentLabel: title.textContent, identity: title.title, head: box(head), title: box(title), actions: box(head.querySelector(".conv__actions")),
      buttons: [...head.querySelectorAll(".conv__action")].map(button => {
        const bounds = box(button), hit = document.elementFromPoint(bounds.x + bounds.width / 2, bounds.y + bounds.height / 2);
        return { name: button.getAttribute("aria-label") || button.textContent.trim(), tooltip: button.title, bounds,
          visible: getComputedStyle(button).visibility !== "hidden", unobstructed: button === hit || button.contains(hit),
          icon: button.querySelector("use")?.getAttribute("href"), textDisplay: button.querySelector(".conv__action-label") && getComputedStyle(button.querySelector(".conv__action-label")).display };
      }) };
  });
  assert(result.agentLabel && result.identity && result.title.width > 24, `${label} retains the agent target: ${JSON.stringify(result)}`);
  assert(result.title.right <= result.actions.left - 4, `${label} target does not overlap actions: ${JSON.stringify(result)}`);
  assert.equal(result.buttons.length, 3, `${label} retains all three permitted actions`);
  for (const button of result.buttons) {
    assert(button.visible && button.unobstructed, `${label} ${button.name} is unobstructed: ${JSON.stringify(result)}`);
    assert(button.bounds.left >= result.head.left + 2 && button.bounds.right <= result.head.right - 2
      && button.bounds.top >= result.head.top + 2 && button.bounds.bottom <= result.head.bottom - 2,
    `${label} ${button.name} stays inside the header: ${JSON.stringify(result)}`);
    assert(button.bounds.width >= 28 && button.bounds.height >= 28, `${label} ${button.name} remains a usable control`);
    assert(button.icon && button.tooltip.includes(result.identity), `${label} ${button.name} has an icon and explicit target tooltip`);
    if (result.head.width <= 440) assert.equal(button.textDisplay, "none", `${label} switches to compact icon actions`);
  }
  const buttons = pane.locator(".conv__head .conv__action");
  await buttons.first().focus();
  for (let index = 0; index < result.buttons.length; index += 1) {
    assert(await buttons.nth(index).evaluate(button => document.activeElement === button), `${label} keyboard reaches ${result.buttons[index].name} in order`);
    if (index + 1 < result.buttons.length) await pane.page().keyboard.press("Tab");
  }
  return { label, ...result };
}

async function inspectJump(page, host, working) {
  const viewport = transcript(page, host);
  const jump = conversationPane(page, host).getByRole("button", { name: "Jump to latest", exact: true });
  await jump.waitFor();
  await eventually(() => jump.getAttribute("data-working").then(value => value === String(working)), `${host} jump follows owner working=${working}`);
  const result = await jump.evaluate(async button => {
    const activity = button.querySelector(".cc-conversation-jump-latest__activity");
    const animation = activity.getAnimations()[0];
    const before = animation?.currentTime;
    await new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
    const rect = button.getBoundingClientRect();
    return {
      text: button.textContent.trim(), svgCount: button.querySelectorAll("svg").length,
      position: getComputedStyle(button).position, width: rect.width, height: rect.height,
      top: rect.top, bottom: rect.bottom, left: rect.left, right: rect.right,
      anchorHeight: button.parentElement.getBoundingClientRect().height,
      animations: activity.getAnimations().length, playState: animation?.playState,
      advanced: typeof before === "number" && animation.currentTime > before,
      stroke: getComputedStyle(activity).stroke,
    };
  });
  const bounds = await viewport.boundingBox();
  assert.equal(result.text, "", `${host} jump uses an icon without visible text`);
  assert(result.svgCount >= 1);
  assert.equal(result.position, "absolute");
  assert.equal(result.anchorHeight, 0, `${host} jump adds no normal-flow row`);
  assert(result.width >= 32 && result.width <= 44 && Math.abs(result.width - result.height) <= 1, `${host} jump remains a compact circle`);
  assert(result.left >= bounds.x && result.right <= bounds.x + bounds.width && result.top >= bounds.y && result.bottom <= bounds.y + bounds.height + 2,
    `${host} jump is inside the transcript above its footer: ${JSON.stringify({ result, bounds })}`);
  result.textClearance = await inspectJumpTextClearance(viewport, jump, `${host} working=${working}`);
  if (working) {
    assert(result.animations > 0 && result.playState === "running" && result.advanced, `${host} actual active perimeter animation advances`);
    await page.emulateMedia({ reducedMotion: "reduce" });
    const reduced = await jump.locator(".cc-conversation-jump-latest__activity").evaluate(node => ({
      animations: node.getAnimations().length, name: getComputedStyle(node).animationName,
      stroke: getComputedStyle(node).stroke,
    }));
    assert.equal(reduced.animations, 0, `${host} reduced motion stops the activity animation`);
    assert.equal(reduced.name, "none");
    assert.notEqual(reduced.stroke, "transparent");
    assert.notEqual(reduced.stroke, "rgba(0, 0, 0, 0)", `${host} reduced motion retains a static active accent`);
    result.reducedMotion = reduced;
    await page.emulateMedia({ reducedMotion: "no-preference" });
  } else {
    assert.equal(result.animations, 0, `${host} idle jump has no running animation`);
  }
  return result;
}

async function inspectQuoteAction(page, host) {
  const viewport = transcript(page, host);
  const quote = conversationPane(page, host).getByRole("button", { name: "Add to message", exact: true });
  assert.equal(await quote.count(), 0, `${host} has no permanent quote action`);
  const before = await viewport.evaluate(node => ({ height: node.clientHeight, scrollTop: node.scrollTop }));
  const selectedText = await viewport.evaluate(node => {
    const bounds = node.getBoundingClientRect();
    const paragraph = [...node.querySelectorAll("[data-quote-source] p")].find(item => {
      const rect = item.getBoundingClientRect();
      return item.textContent.trim() && rect.top >= bounds.top + 50 && rect.bottom <= bounds.bottom - 10;
    });
    if (!paragraph) throw new Error("Need an actual visible reply paragraph for selection");
    const range = document.createRange(); range.selectNodeContents(paragraph);
    const selection = window.getSelection(); selection.removeAllRanges(); selection.addRange(range);
    return selection.toString();
  });
  await quote.waitFor();
  const visible = await quote.evaluate(button => {
    const rect = button.getBoundingClientRect();
    return { text: button.textContent.trim(), position: getComputedStyle(button).position,
      anchorHeight: button.parentElement.getBoundingClientRect().height,
      top: rect.top, bottom: rect.bottom, left: rect.left, right: rect.right,
      svgCount: button.querySelectorAll("svg").length };
  });
  const bounds = await viewport.boundingBox();
  assert.equal(visible.text, "", `${host} quote action has no visible text`);
  assert.equal(visible.position, "absolute"); assert.equal(visible.anchorHeight, 0);
  assert.equal(visible.svgCount, 1);
  assert(visible.left >= bounds.x && visible.right <= bounds.x + bounds.width && visible.top >= bounds.y && visible.bottom <= bounds.y + bounds.height,
    `${host} quote floats beside the visible selection: ${JSON.stringify({ visible, bounds })}`);
  const after = await viewport.evaluate(node => ({ height: node.clientHeight, scrollTop: node.scrollTop }));
  assert.deepEqual(after, before, `${host} quote appearance does not consume transcript height or move its reading position`);
  await capture(page, `${host}-floating-quote-selection`);
  await page.keyboard.press("Escape");
  await quote.waitFor({ state: "detached" });
  assert.equal(await page.evaluate(() => window.getSelection()?.toString()), selectedText, "Escape dismisses the action without destroying selection");
  await page.evaluate(() => window.getSelection()?.removeAllRanges());
  await settle(page);
  assert.equal(await quote.count(), 0, `${host} quote stays absent after selection clears`);
  return { selectedText, visible, before, after };
}

async function capture(page, name) {
  await fs.mkdir(evidence, { recursive: true });
  await page.screenshot({ path: path.join(evidence, `${name}.png`), fullPage: true });
}

async function sendApi(fixture, text, key = text) {
  const accepted = await rpc(fixture.baseUrl, "meta/console/send", {
    identity: "router:main", content: text, origin: "console:visual-acceptance",
    origin_kind: "operator", idempotency_key: key,
  });
  assert(accepted.body.result?.input_frame_id, JSON.stringify(accepted.body));
  return accepted.body.result;
}

async function observePresentationStream(page) {
  const session = await page.context().newCDPSession(page);
  const streams = new Map();
  session.on("Network.responseReceived", event => {
    if (!new URL(event.response.url).pathname.endsWith("/timeline/stream") || streams.size >= 16) return;
    streams.set(event.requestId, { url: event.response.url, status: event.response.status, headersAt: event.timestamp,
      headersAtMs: Date.now(), chunks: 0, bytes: 0, first: [], last: [] });
  });
  session.on("Network.dataReceived", event => {
    const stream = streams.get(event.requestId);
    if (!stream) return;
    stream.chunks += 1; stream.bytes += event.dataLength;
    const sample = { at: event.timestamp, atMs: Date.now(), bytes: event.dataLength };
    if (stream.first.length < 32) stream.first.push(sample);
    stream.last.push(sample); if (stream.last.length > 32) stream.last.shift();
  });
  session.on("Network.loadingFailed", event => {
    const stream = streams.get(event.requestId);
    if (stream) stream.failure = { at: event.timestamp, error: event.errorText, canceled: event.canceled };
  });
  await session.send("Network.enable");
  return { snapshot: () => [...streams.values()], close: () => session.detach() };
}

async function observePresentationDocument(viewport, selectedText) {
  await viewport.evaluate((node, text) => {
    const data = { startedAtMs: Date.now(), timeOrigin: performance.timeOrigin, mutations: 0, samples: 0, streamingSamples: 0, paragraphStreamingSamples: 0, first: [], last: [] };
    let firstDocument; let firstParagraph; let previous;
    const sample = reason => {
      const documents = [...node.querySelectorAll(".cc-markdown-document")];
      const paragraph = documents.flatMap(item => [...item.querySelectorAll("p")]).find(item => item.textContent === text);
      const documentNode = paragraph?.closest(".cc-markdown-document");
      if (documentNode && !firstDocument) { firstDocument = documentNode; firstParagraph = paragraph; }
      const candidate = documentNode || documents.findLast(item => item.dataset.streaming === "true");
      const box = paragraph?.getBoundingClientRect();
      const state = { documentId: candidate?.dataset.markdownDocumentId, streaming: candidate?.dataset.streaming,
        sourceChars: candidate?.textContent.length, paragraphFound: Boolean(paragraph),
        sameDocument: Boolean(documentNode && documentNode === firstDocument), firstParagraphConnected: firstParagraph?.isConnected,
        paragraphBox: box ? { x: box.x, y: box.y, width: box.width, height: box.height } : undefined,
        paragraphVisibility: paragraph ? getComputedStyle(paragraph).visibility : undefined,
        visibilityState: document.visibilityState, focused: document.hasFocus(),
        streamingDocuments: documents.filter(item => item.dataset.streaming === "true").map(item => item.dataset.markdownDocumentId) };
      const signature = JSON.stringify(state);
      if (signature === previous && reason === "mutation") return;
      previous = signature;
      const item = { at: performance.now(), atMs: Date.now(), reason, ...state };
      data.samples += 1;
      if (state.streamingDocuments.length) data.streamingSamples += 1;
      if (state.paragraphFound && state.streaming === "true") data.paragraphStreamingSamples += 1;
      if (data.first.length < 64) data.first.push(item);
      data.last.push(item); if (data.last.length > 64) data.last.shift();
    };
    const observer = new MutationObserver(() => { data.mutations += 1; sample("mutation"); });
    observer.observe(node, { childList: true, subtree: true, characterData: true, attributes: true, attributeFilter: ["data-streaming", "data-markdown-document-id"] });
    const onVisibility = () => sample("visibility");
    document.addEventListener("visibilitychange", onVisibility);
    window.__presentationStreamDiagnostics = { data, stop() { sample("stop"); observer.disconnect(); document.removeEventListener("visibilitychange", onVisibility); return data; } };
    sample("armed");
  }, selectedText);
}

async function presentation(host) {
  const fixture = await startFixture({ diagnoseStreams: true });
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 }, permissions: ["clipboard-read", "clipboard-write"] });
  const monitor = browserErrors(page);
  const network = await observePresentationStream(page);
  const diagnostics = {};
  try {
    await fixture.control("model", { source: Array.from({ length: 12 }, (_, index) => prose(index)).join(""), delay_ms: 0, chunk_chars: 512 });
    const originalInstruction = "Review the workgraph and prepare peer-review evidence.";
    const first = await sendApi(fixture, originalInstruction);
    await completed(fixture, "Investigation 11");
    await open(page, fixture, host, monitor);
    const viewport = transcript(page, host);
    // Read the reply to the accepted input, not whatever renders first: the
    // mob's kickoff reply above it renders Markdown (a table and a link) too,
    // and the stock transcript parks turns outside its window (hidden until
    // found), so the first table can belong to a turn that never shows again.
    // The stock turn carrying the accepted input frame holds its reply.
    const reply = host === "stock"
      ? viewport.locator("[data-conversation-turn-id]").filter({ has: page.locator(`[data-quote-message-id="${first.input_frame_id}"]`) })
      : viewport;
    await reply.locator("table").first().waitFor();
    assert.equal(await reply.locator('img[src*="example.com/tracker"]').count(), 0, "default Markdown cannot fetch external images");
    assert(await reply.locator('a[href="https://example.com"]').count() > 0);
    const initialPrompt = viewport.getByText(originalInstruction, { exact: true });
    await initialPrompt.waitFor();
    assert.equal(await initialPrompt.count(), 1, `${host} the original authored input is rendered exactly once`);
    const initialInput = await initialPrompt.evaluate(node => {
      const row = node.closest("[data-conversation-row-id]");
      const quote = node.closest("[data-quote-source]");
      return { id: row?.dataset.conversationRowId, sourceFrameId: quote?.dataset.quoteMessageId, source: quote?.dataset.quoteSource };
    });
    assert(initialInput.id, `${host} the original input has a visual row identity before streaming`);
    assert.equal(initialInput.sourceFrameId, first.input_frame_id, `${host} the initial quote identifies the exact accepted input frame`);
    assert.equal(initialInput.source, originalInstruction, `${host} the initial input preserves the exact authored source`);
    assert.equal(await viewport.locator("[data-conversation-row-id]").evaluateAll((nodes, id) => nodes.filter(node => node.dataset.conversationRowId === id).length, initialInput.id), 1,
      `${host} the initial visual row identity occurs exactly once`);
    diagnostics.initialRenderedInput = initialInput;
    await capture(page, `${host}-1600-initial`);

    // Reading intent must survive a new, long stream through the real runtime.
    const beforeLatestAppears = await transcriptGeometry(viewport);
    await viewport.evaluate(node => { node.scrollTop = Math.min(350, node.scrollHeight / 3); node.dispatchEvent(new Event("scroll")); });
    const controls = { idle: await inspectJump(page, host, false), quote: await inspectQuoteAction(page, host) };
    controls.appearanceGeometry = { before: beforeLatestAppears, after: await transcriptGeometry(viewport) };
    assert.deepEqual(controls.appearanceGeometry.after, controls.appearanceGeometry.before,
      `${host} latest appearance changes neither transcript size nor text wrapping`);
    const initialJump = conversationPane(page, host).getByRole("button", { name: "Jump to latest", exact: true });
    const beforeLatestDisappears = await transcriptGeometry(viewport);
    await initialJump.click();
    await initialJump.waitFor({ state: "detached" });
    controls.disappearanceGeometry = { before: beforeLatestDisappears, after: await transcriptGeometry(viewport) };
    assert.deepEqual(controls.disappearanceGeometry.after, controls.disappearanceGeometry.before,
      `${host} latest disappearance changes neither transcript size nor text wrapping`);
    await viewport.evaluate(node => { node.scrollTop = Math.min(350, node.scrollHeight / 3); node.dispatchEvent(new Event("scroll")); });
    await initialJump.waitFor();
    const anchor = await viewport.evaluate(node => {
      const top = node.getBoundingClientRect().top;
      const row = [...node.querySelectorAll("[data-conversation-row-id]")].find(item => item.getBoundingClientRect().bottom > top + 20);
      return { id: row.dataset.conversationRowId, top: row.getBoundingClientRect().top - top };
    });
    const selectedText = "Preserve this exact selection: A\u030A, \u00e5 and \ud83d\ude80.";
    const streamSource = `${selectedText}\n\n${prose(99).repeat(8)}Stream finished successfully.\n`;
    await fixture.control("model", { source: streamSource, delay_ms: 16, chunk_chars: 8 });
    await observePresentationDocument(viewport, selectedText);
    diagnostics.sendStartedAtMs = Date.now();
    diagnostics.accepted = await sendApi(fixture, "Continue in the background while I inspect the earlier evidence.", "stream-anchor");
    diagnostics.sendReturnedAtMs = Date.now();
    const streamingDocument = viewport.locator('.cc-markdown-document[data-streaming="true"]').last();
    await streamingDocument.getByText(selectedText, { exact: true }).waitFor();
    controls.working = await inspectJump(page, host, true);
    await capture(page, `${host}-floating-jump-working`);
    await streamingDocument.getByText(selectedText, { exact: true }).evaluate(node => {
      const range = document.createRange(); range.selectNodeContents(node);
      const selection = window.getSelection(); selection.removeAllRanges(); selection.addRange(range);
      window.__presentationSelection = { node, document: node.closest(".cc-markdown-document") };
    });
    const geometry = await viewport.evaluate(async (node, anchor) => {
      let maxDrift = 0; let updates = 0;
      const row = () => [...node.querySelectorAll("[data-conversation-row-id]")].find(item => item.dataset.conversationRowId === anchor.id);
      const observer = new MutationObserver(() => { updates += 1; const item = row(); if (item) maxDrift = Math.max(maxDrift, Math.abs(item.getBoundingClientRect().top - node.getBoundingClientRect().top - anchor.top)); });
      observer.observe(node, { childList: true, subtree: true, characterData: true });
      const start = performance.now();
      while (updates < 100 && performance.now() - start < 20_000) await new Promise(requestAnimationFrame);
      observer.disconnect();
      return { maxDrift, updates };
    }, anchor);
    assert(geometry.updates >= 100, `must observe 100 real streamed updates: ${JSON.stringify(geometry)}`);
    assert(geometry.maxDrift <= 2, `${host} stream anchor drift: ${JSON.stringify(geometry)}`);
    await completed(fixture, "Stream finished successfully.");
    await eventually(() => streamingDocument.count().then(count => count === 0), "stream document completed");
    const selection = await page.evaluate(() => ({
      text: window.getSelection()?.toString(), connected: window.__presentationSelection.node.isConnected,
      sameDocument: window.__presentationSelection.node.closest(".cc-markdown-document") === window.__presentationSelection.document,
      streaming: window.__presentationSelection.document.dataset.streaming,
      completedSource: window.__presentationSelection.document.closest("[data-quote-source]")?.dataset.quoteSource,
    }));
    assert.equal(selection.text, selectedText, `${host} selection survives stream completion`);
    assert(selection.connected && selection.sameDocument, `${host} selected paragraph and document keep DOM identity`);
    assert.equal(selection.streaming, "false");
    assert.equal(selection.completedSource, streamSource, `${host} the selected document becomes the exact complete reply`);
    assert.equal(await viewport.locator(".cc-markdown-document").filter({ hasText: selectedText }).count(), 1, `${host} history/live reconciliation does not duplicate the reply`);
    diagnostics.document = await page.evaluate(() => window.__presentationStreamDiagnostics.stop());
    geometry.selection = selection;
    controls.completed = await inspectJump(page, host, false);
    geometry.controls = controls;
    geometry.resize = await measureAnchor(viewport, anchor, () => page.setViewportSize({ width: 1440, height: 900 }), `${host} viewport resize while reading`);
    controls.reading1440 = await inspectJump(page, host, false);
    await capture(page, `${host}-1440-reading`);
    const completedDocument = viewport.locator(".cc-markdown-document").filter({ hasText: selectedText }).last();
    const copy = host === "stock"
      ? completedDocument.locator('xpath=ancestor::*[contains(concat(" ",normalize-space(@class)," ")," msg ")][1]').getByRole("button", { name: "Copy reply", exact: true })
      : completedDocument.locator('xpath=ancestor::*[contains(concat(" ",normalize-space(@class)," ")," cc-message-group ")][1]').getByRole("button", { name: "Copy response", exact: true });
    await page.evaluate(() => {
      window.__clipboardEvidence = { writes: [], clicks: [] };
      const original = navigator.clipboard.writeText.bind(navigator.clipboard);
      navigator.clipboard.writeText = async text => {
        const call = { length: text.length, tail: text.slice(-80) }; window.__clipboardEvidence.writes.push(call);
        try { const result = await original(text); call.ok = true; return result; } catch (error) { call.error = String(error); throw error; }
      };
      for (const type of ["pointerdown", "pointerup", "mousedown", "mouseup", "click"]) document.addEventListener(type, event => {
        const button = event.target.closest("button"); window.__clipboardEvidence.clicks.push({ type, button: button?.outerHTML, target: event.target.outerHTML?.slice(0, 300), x: event.clientX, y: event.clientY });
      }, { capture: true });
    });
    await copy.click();
    await eventually(async () => {
      const actual = await page.evaluate(() => navigator.clipboard.readText());
      assert.equal(actual, streamSource, `${host} exact Markdown source clipboard differs (actual ${actual.length}, expected ${streamSource.length})`);
      return true;
    }, `${host} exact Markdown source clipboard`);
    geometry.clipboardExact = true;
    const jumpAfterCopy = conversationPane(page, host).getByRole("button", { name: "Jump to latest", exact: true });
    if (await jumpAfterCopy.count()) await jumpAfterCopy.click();
    await settle(page);
    const endDistance = await viewport.evaluate(node => node.scrollHeight - node.scrollTop - node.clientHeight);
    assert(endDistance <= 48, `${host} copy target reveal or jump reaches the transcript end, remaining ${endDistance}px`);
    await page.setViewportSize({ width: 1024, height: 768 });
    await viewport.evaluate(node => { node.scrollTop = Math.min(350, node.scrollHeight / 3); node.dispatchEvent(new Event("scroll")); });
    controls.reading1024 = await inspectJump(page, host, false);
    await capture(page, `${host}-1024-containment`);
    if (host === "shared") {
      await page.getByRole("button", { name: "Toggle second pane" }).click();
      await page.setViewportSize({ width: 1600, height: 1000 });
      await settle(page);
      controls.twoPanes = await inspectJump(page, host, false);
      await capture(page, `${host}-two-panes`);
    }
    await monitor.during("explicit presentation reload", async () => {
      await page.reload();
      if (host === "stock" && !await page.locator(".conv__body").count()) await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: /router/i }).first().click();
      await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    }, initializationCancellation);
    const reloadedViewport = transcript(page, host);
    const ownerInput = (await timeline(fixture)).frames.find(frame => frame.id === first.input_frame_id);
    assert(ownerInput, `${host} owner retains the original accepted input frame`);
    assert.equal(ownerInput.kind, "user_input");
    assert.equal(ownerInput.interaction_id, first.interaction_id);
    assert.equal(ownerInput.payload.content, originalInstruction);
    geometry.reloadOwnerInput = ownerInput;
    const originalPrompt = reloadedViewport.getByText(originalInstruction, { exact: true });
    geometry.reloadPages = [];
    // The long stream can push the first turn outside the recent 200-frame
    // seed. Recover it through the host's own history action, as a reader
    // does. Stock pages by scrolling to the start (where its windowed
    // transcript also mounts the first turn); the shared host has an explicit
    // Load older history control and does not page on scroll.
    const reveal = reloadedViewport.getByRole("button", { name: "Show earlier messages", exact: true });
    for (let index = 0; index < 16; index += 1) {
      let response = null;
      if (host === "shared") {
        if (await originalPrompt.count()) break;
        const older = page.getByRole("button", { name: "Load older history", exact: true }).first();
        await eventually(() => older.isEnabled(), "reload history action is ready");
        const responsePromise = page.waitForResponse(isOlderHistoryPage);
        await older.dispatchEvent("click");
        response = await responsePromise;
      } else {
        response = await scrollToStart(reloadedViewport);
      }
      if (response) {
        const body = await response.json();
        assert.equal(response.status(), 200, "reload older-history request succeeds");
        assert(Array.isArray(body.result?.frames), "reload receives an owner history page");
        geometry.reloadPages.push({ request: response.request().postDataJSON().params, frameCount: body.result.frames.length, exhausted: body.result.exhausted === true });
        if (host === "shared") await settle(page);
      }
      if (await originalPrompt.isVisible()) break;
      if (host !== "shared" && !response && !(await reveal.count())) break;
    }
    await originalPrompt.waitFor();
    assert.equal(await originalPrompt.count(), 1, `${host} the original authored input remains rendered exactly once after history paging`);
    const originalRow = originalPrompt.locator('xpath=ancestor::*[@data-conversation-row-id][1]');
    assert.equal(await reloadedViewport.locator("[data-conversation-row-id]").evaluateAll((nodes, id) => nodes.filter(node => node.dataset.conversationRowId === id).length, initialInput.id), 1,
      `${host} accepted input retains its initial visual row identity exactly once after history paging`);
    assert.equal(await originalRow.getByText(originalInstruction, { exact: true }).count(), 1);
    geometry.reloadRenderedInput = await originalRow.evaluate(node => {
      const quote = node.matches("[data-quote-source]") ? node : node.querySelector("[data-quote-source]");
      return { id: node.dataset.conversationRowId, sourceFrameId: quote?.dataset.quoteMessageId, source: quote?.dataset.quoteSource };
    });
    assert.equal(geometry.reloadRenderedInput.id, initialInput.id, `${host} the exact original input retains its visual row identity after reload`);
    assert.equal(geometry.reloadRenderedInput.sourceFrameId, first.input_frame_id, `${host} the reloaded quote identifies the exact accepted input frame`);
    assert.equal(geometry.reloadRenderedInput.source, originalInstruction, `${host} reloaded input preserves the exact authored source`);
    await originalPrompt.scrollIntoViewIfNeeded();
    await settle(page);
    await capture(page, `${host}-presentation-reloaded-original`);
    assert.deepEqual(monitor.errors, []);
    await fs.writeFile(path.join(evidence, `${host}-geometry.json`), JSON.stringify({ geometry, diagnostics: { ...diagnostics, network: network.snapshot(), proxy: fixture.streamDiagnostics }, errors: monitor.errors, expectedFailures: monitor.expected, observations: fixture.observations }, null, 2));
  } catch (error) {
    diagnostics.document ??= await page.evaluate(() => window.__presentationStreamDiagnostics?.stop()).catch(() => undefined);
    await capture(page, `${host}-presentation-failure`).catch(() => {});
    await fs.writeFile(path.join(evidence, `${host}-presentation-failure.json`), JSON.stringify({ error: error.stack, html: await page.content(), clipboard: await page.evaluate(() => ({ ...window.__clipboardEvidence, focused: document.hasFocus() })), diagnostics: { ...diagnostics, network: network.snapshot(), proxy: fixture.streamDiagnostics }, errors: monitor.errors, expectedFailures: monitor.expected, frames: (await timeline(fixture)).frames, observations: fixture.observations, logs: fixture.logs() }, null, 2));
    throw error;
  } finally { await network.close().catch(() => {}); await browser.close(); await fixture.close(); }
}

async function seedReadingHistory(fixture) {
  const turns = [];
  for (let index = 0; index < 6; index += 1) {
    const instruction = `Review checkpoint ${index}: preserve this release evidence.`;
    const source = `## Reading checkpoint ${index}\n\n${Array.from({ length: 6 }, (_, paragraph) => `Evidence ${index}.${paragraph}: the release reviewer checks the dependency graph, verifies the previous agent reply, and keeps the exact decision available while new work arrives.`).join("\n\n")}\n`;
    await fixture.control("model", { source, delay_ms: 0, chunk_chars: 4096 });
    const accepted = await sendApi(fixture, instruction, `reading-checkpoint-${index}`);
    const terminal = await completed(fixture, `Reading checkpoint ${index}`);
    assert.equal(terminal.interaction_id, accepted.interaction_id, "reading history belongs to its actual accepted turn");
    turns.push({ instruction, source, accepted, terminal });
  }
  return turns;
}

function identitySwitchCancellation(request) {
  if (!request.failure()?.errorText?.includes("ERR_ABORTED")) return false;
  const url = new URL(request.url());
  if (url.pathname.endsWith("/timeline/stream")) return true;
  try {
    const body = JSON.parse(request.postData() || "{}");
    return body.method === "meta/console/query_timeline" && body.params?.mode === "recent"
      && ["router:main", "domain:delivery"].includes(body.params?.identity);
  } catch { return false; }
}

async function retargetReadingPane(page, host, identity, pane) {
  if (host === "shared") await page.getByRole("combobox", { name: "Agent", exact: true }).selectOption(identity);
  else {
    await pane.getByTestId(/^pane-title:/).click();
    await pane.getByTestId(/^pane-menu-agent:/).filter({ hasText: identity }).click();
    await pane.getByTestId(`chat-composer:${identity}`).waitFor();
  }
  await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
  await settle(page);
}

async function readingIntent(host) {
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  const monitor = browserErrors(page);
  const result = { host, checks: [], errors: monitor.errors, expectedFailures: monitor.expected };
  try {
    result.turns = await seedReadingHistory(fixture);
    const toolSeed = { id: `reading-tools-${host}`, instruction: "Check the real WorkGraph before the next streaming review.",
      source: "Ready for the next streaming review." };
    await fixture.control("model-barrier", { action: "arm", plan: { id: toolSeed.id, match_text: toolSeed.instruction, source: toolSeed.source } });
    await fixture.control("model-barrier", { action: "release", id: toolSeed.id });
    toolSeed.accepted = await sendApi(fixture, toolSeed.instruction, toolSeed.id);
    toolSeed.terminal = await completed(fixture, toolSeed.source);
    assert.equal(toolSeed.terminal.interaction_id, toolSeed.accepted.interaction_id);
    const seededFrames = (await timeline(fixture)).frames;
    toolSeed.tool = seededFrames.find(frame => frame.kind === "tool_result_received"
      && frame.payload?.id === `fixture-${toolSeed.id}-peer-ready` && frame.payload?.is_error === false);
    assert(toolSeed.tool, "reading controls exercise an actual completed WorkGraph tool");
    result.toolSeed = toolSeed;
    const awaySource = "Delivery reviewer is checking the independent image branch.";
    await fixture.control("model", { source: awaySource, delay_ms: 0, chunk_chars: 4096 });
    const away = await rpc(fixture.baseUrl, "meta/console/send", { identity: "domain:delivery", content: "Review the image branch independently.",
      origin: "console:reading-intent", origin_kind: "operator", idempotency_key: "reading-away" });
    assert(away.body.result?.interaction_id, JSON.stringify(away));
    await eventually(async () => {
      const response = await fetch(`${fixture.baseUrl}/console/timeline?identity=domain%3Adelivery&mode=recent&limit=200`);
      const body = await response.json();
      return body.frames?.some(frame => frame.kind === "text_complete" && frame.interaction_id === away.body.result.interaction_id);
    }, "real alternate identity completion");
    await open(page, fixture, host, monitor);
    const firstPane = host === "shared" ? page.getByTestId("shared-pane-0") : page.getByTestId(/^pane:panel-/).first();
    const viewport = transcript(page, host);
    const anchorRow = viewport.locator("[data-conversation-row-id]").filter({ hasText: result.turns[1].instruction }).last();
    await mountEarlyRow(viewport, anchorRow);
    if (host === "stock") {
      result.headers = [];
      for (const size of [{ width: 1600, height: 1000 }, { width: 1440, height: 900 }, { width: 1024, height: 768 }]) {
        await page.setViewportSize(size); await settle(page);
        result.headers.push(await inspectStockHeader(firstPane, `single pane ${size.width}`));
        await capture(page, `stock-header-single-${size.width}`);
      }
      await page.setViewportSize({ width: 1600, height: 1000 }); await settle(page);
    }
    const anchor = await holdReadingRow(viewport, anchorRow);
    result.anchors = { identity: anchor };
    try {
      result.checks.push(await measureAnchor(viewport, anchor, () => monitor.during("deliberate identity away and return", async () => {
        await retargetReadingPane(page, host, "domain:delivery", firstPane);
        await transcript(page, host).getByText(awaySource, { exact: true }).waitFor();
        await retargetReadingPane(page, host, "router:main", firstPane);
        await anchorRow.waitFor();
      }, identitySwitchCancellation), `${host} identity return restores its reading row`));
    } catch (error) {
      // Retain this failure while collecting independent pane evidence. The
      // scenario still fails at its end; later success cannot conceal it.
      result.identityFailure = error.stack || String(error);
    }
    await capture(page, `${host}-reading-identity-${result.identityFailure ? "failure" : "restored"}-1600`);

    if (host === "shared") await page.getByRole("button", { name: "Toggle second pane", exact: true }).click();
    else await firstPane.getByTestId(/^pane-split-right:/).click();
    const secondPane = host === "shared" ? page.getByTestId("shared-pane-1") : page.getByTestId(/^pane:panel-/).nth(1);
    await secondPane.waitFor();
    if (host === "stock" && !await secondPane.getByTestId("chat-composer:router:main").count()) {
      await retargetReadingPane(page, host, "router:main", secondPane);
    }
    const secondViewport = secondPane.locator(host === "shared" ? ".cc-conversation-pane__scroll" : ".conv__body");
    await secondViewport.locator("[data-conversation-row-id]").filter({ hasText: result.turns[5].instruction }).last().waitFor();
    // Each pane receives its own explicit scroll intent before background work.
    const firstReading = await holdReadingRow(viewport, anchorRow);
    const secondAnchorRow = secondViewport.locator("[data-conversation-row-id]").filter({ hasText: result.turns[3].instruction }).last();
    const secondReading = await holdReadingRow(secondViewport, secondAnchorRow);
    Object.assign(result.anchors, { firstReading, secondReading });
    assert.notEqual(firstReading.id, secondReading.id, "same-agent panes intentionally read different retained rows");
    result.clearance = [
      await inspectJumpTextClearance(viewport, firstPane.getByRole("button", { name: "Jump to latest", exact: true }), `${host} first split pane 1600`),
      await inspectJumpTextClearance(secondViewport, secondPane.getByRole("button", { name: "Jump to latest", exact: true }), `${host} second split pane 1600`),
    ];
    result.checks.push(await measureAnchor(viewport, firstReading, async () => {
      await secondPane.getByRole("button", { name: "Jump to latest", exact: true }).click();
      await eventually(() => secondViewport.evaluate(node => node.scrollHeight - node.scrollTop - node.clientHeight <= 48), "second pane reaches actual live edge");
    }, `${host} second-pane latest action leaves first reading anchor`));

    await browser.contexts()[0].grantPermissions(["clipboard-read", "clipboard-write"], { origin: fixture.baseUrl });
    const liveGeometry = () => secondViewport.evaluate(node => ({ top: node.scrollTop, height: node.scrollHeight,
      remaining: node.scrollHeight - node.scrollTop - node.clientHeight }));
    result.liveEdgeGestures = { before: await liveGeometry(), actions: [] };
    await secondViewport.hover({ position: { x: 20, y: 100 } });
    await page.mouse.wheel(0, 350); await settle(page);
    result.liveEdgeGestures.actions.push({ action: "down-wheel", geometry: await liveGeometry() });
    await secondViewport.focus(); await page.keyboard.press("End"); await settle(page);
    result.liveEdgeGestures.actions.push({ action: "End", geometry: await liveGeometry() });
    await secondViewport.getByRole("button", { name: host === "stock" ? "Copy reply" : "Copy response", exact: true }).last().click();
    result.liveEdgeGestures.copied = await page.evaluate(() => navigator.clipboard.readText());
    assert.equal(result.liveEdgeGestures.copied, toolSeed.source, "actual copy control preserves the exact latest reply");
    result.liveEdgeGestures.actions.push({ action: "copy", geometry: await liveGeometry() });
    const completedTool = secondViewport.locator(".cc-tool-call").filter({
      has: page.locator('.cc-tool-call__name[title="workgraph_ready"]'),
    }).last();
    await completedTool.locator(".cc-tool-call__header").click();
    await completedTool.locator(".cc-tool-call__section").first().waitFor();
    await eventually(async () => (await liveGeometry()).remaining <= 48, "opening the actual tool at live edge remains following");
    result.liveEdgeGestures.actions.push({ action: "tool-open", geometry: await liveGeometry() });
    await completedTool.locator(".cc-tool-call__header").click(); await settle(page);
    result.liveEdgeGestures.actions.push({ action: "tool-close", geometry: await liveGeometry() });
    for (const action of result.liveEdgeGestures.actions) assert(action.geometry.remaining <= 48,
      `${host} ${action.action} stays at the live edge: ${JSON.stringify(action)}`);
    await capture(page, `${host}-live-edge-controls-1600`);

    const streamSource = `## New review arrived\n\n${Array.from({ length: 10 }, (_, index) => `Progress ${index}: a new peer review finishes while the first pane retains its older checkpoint.`).join("\n\n")}\n\nCompleted independent reading check.`;
    await fixture.control("model", { source: streamSource, delay_ms: 12, chunk_chars: 64 });
    const beforeSecond = await secondViewport.evaluate(node => ({ top: node.scrollTop, height: node.scrollHeight }));
    result.checks.push(await measureAnchor(viewport, firstReading, async () => {
      result.streamAccepted = await sendApi(fixture, "Continue the release review while I read an older checkpoint.", "reading-two-pane-stream");
      result.streamTerminal = await completed(fixture, "Completed independent reading check.");
      await secondViewport.getByText("Completed independent reading check.", { exact: true }).waitFor();
      await eventually(() => secondViewport.evaluate(node => node.scrollHeight - node.scrollTop - node.clientHeight <= 48), "following pane tracks new live text");
    }, `${host} background stream preserves independent reading pane`));
    const afterSecond = await secondViewport.evaluate(node => ({ top: node.scrollTop, height: node.scrollHeight, remaining: node.scrollHeight - node.scrollTop - node.clientHeight }));
    assert(afterSecond.top > beforeSecond.top + 200, "following pane actually moved with new content");
    assert.equal(result.streamTerminal.interaction_id, result.streamAccepted.interaction_id);
    result.panes = { firstReading, secondReading, beforeSecond, afterSecond };
    if (host === "stock") result.headers.push(await inspectStockHeader(firstPane, "first split pane 1600"), await inspectStockHeader(secondPane, "second split pane 1600"));
    await capture(page, `${host}-independent-reading-panes-1600`);

    await secondViewport.hover({ position: { x: 20, y: 100 } });
    await page.mouse.wheel(0, -450);
    await eventually(async () => (await liveGeometry()).remaining > 120, "actual upward wheel leaves the live edge");
    await settle(page);
    const wheelAnchor = await secondViewport.evaluate(node => {
      const viewport = node.getBoundingClientRect();
      const row = [...node.querySelectorAll("[data-conversation-row-id]")].find(item => {
        const rect = item.getBoundingClientRect(); return rect.bottom > viewport.top + 30 && rect.top < viewport.bottom;
      });
      if (!row) throw new Error("upward reading needs an actual visible transcript row");
      return { id: row.dataset.conversationRowId, offset: row.getBoundingClientRect().top - viewport.top };
    });
    const laterSource = `## Final reviewer update\n\n${Array.from({ length: 8 }, (_, index) => `Update ${index}: both readers keep their chosen evidence while another result streams.`).join("\n\n")}\n\nCompleted upward intent check.`;
    await fixture.control("model", { source: laterSource, delay_ms: 12, chunk_chars: 64 });
    result.checks.push(await measureAnchor(viewport, firstReading, async () => {
      result.checks.push(await measureAnchor(secondViewport, wheelAnchor, async () => {
        result.upwardAccepted = await sendApi(fixture, "Finish the review while both panes read earlier evidence.", "reading-upward-stream");
        result.upwardTerminal = await completed(fixture, "Completed upward intent check.");
        await secondViewport.getByText("Completed upward intent check.", { exact: true }).waitFor();
      }, `${host} actual upward wheel preserves the chosen reading row during real streaming`));
    }, `${host} both reading panes remain independent during the next stream`));
    assert.equal(result.upwardTerminal.interaction_id, result.upwardAccepted.interaction_id);
    result.liveEdgeGestures.upwardAnchor = wheelAnchor;
    await capture(page, `${host}-upward-intent-reading-1600`);
    await page.setViewportSize({ width: 1440, height: 900 }); await settle(page);
    result.clearance.push(await inspectJumpTextClearance(viewport, firstPane.getByRole("button", { name: "Jump to latest", exact: true }), `${host} split pane 1440`));
    if (host === "stock") result.headers.push(await inspectStockHeader(firstPane, "first split pane 1440"), await inspectStockHeader(secondPane, "second split pane 1440"));
    await capture(page, `${host}-independent-reading-panes-1440`);
    await page.setViewportSize({ width: 1024, height: 768 }); await settle(page);
    result.clearance.push(await inspectJumpTextClearance(viewport, firstPane.getByRole("button", { name: "Jump to latest", exact: true }), `${host} split pane 1024`));
    if (host === "stock") result.headers.push(await inspectStockHeader(firstPane, "first split pane 1024"), await inspectStockHeader(secondPane, "second split pane 1024"));
    await capture(page, `${host}-independent-reading-panes-1024`);
    assert.deepEqual(monitor.errors, []);
    assert(!result.identityFailure, result.identityFailure);
  } catch (error) {
    result.failure = error.stack || String(error); await capture(page, `${host}-reading-intent-failure`).catch(() => {}); throw error;
  } finally {
    result.html = result.failure ? await page.content() : undefined;
    result.observations = fixture.observations; result.logs = fixture.logs();
    await fs.mkdir(evidence, { recursive: true });
    await fs.writeFile(path.join(evidence, `${host}-reading-intent.json`), JSON.stringify(result, null, 2));
    await browser.close(); await fixture.close();
  }
}

async function recovery(host) {
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  const monitor = browserErrors(page);
  try {
    await fixture.control("model", { source: prose(1), delay_ms: 0, chunk_chars: 500 });
    await sendApi(fixture, "Keep this history through recovery.");
    await completed(fixture, "Investigation 1");
    await open(page, fixture, host, monitor);
    await transcript(page, host).getByText("Keep this history through recovery.", { exact: true }).waitFor();
    const baselineRecent = () => fixture.observations.filter(item => item.request.includes('"mode":"recent"')).length;
    // The initial replay schedules per-identity terminal refreshes after
    // 200 ms. Let those finish before measuring reconnect-only requests.
    let recentCount = baselineRecent();
    let quietSince = Date.now();
    await eventually(() => {
      const count = baselineRecent();
      const pending = fixture.observations.some(item => item.request.includes('"mode":"recent"') && item.status === null);
      if (count !== recentCount || pending) {
        recentCount = count;
        quietSince = Date.now();
      }
      return !pending && Date.now() - quietSince >= 500;
    }, "initial timeline and terminal refresh requests settle");
    const before = baselineRecent();
    await monitor.during("deliberate SSE disconnect and one 503", async () => {
      fixture.rejectNextStreams(1); fixture.disconnectStreams();
      await page.locator('[data-testid="console-transport-status"][data-phase="retrying"]').waitFor();
      await capture(page, `${host}-reconnecting`);
      await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    });
    assert.equal(baselineRecent(), before, "503/EOF does not require a history repair");
    for (let gap = 0; gap < 2; gap += 1) {
      const beforeGaps = fixture.observations.filter(item => item.path.includes("timeline/stream") && item.status === 409).length;
      await monitor.during(`deliberate replay expiry ${gap + 1}`, async () => {
        await fixture.control("fault", { fault: "expired" }); fixture.disconnectStreams();
        await eventually(() => fixture.observations.filter(item => item.path.includes("timeline/stream") && item.status === 409).length > beforeGaps, `${host} real replay gap`);
        await fixture.control("fault", { fault: "none" });
        await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
      });
    }
    await transcript(page, host).getByText("Keep this history through recovery.", { exact: true }).waitFor();
    assert.deepEqual(monitor.errors, []);
    await capture(page, `${host}-recovered`);
    await fs.writeFile(path.join(evidence, `${host}-recovery.json`), JSON.stringify({ errors: monitor.errors, expectedFailures: monitor.expected, observations: fixture.observations }, null, 2));
  } catch (error) {
    await capture(page, `${host}-recovery-failure`).catch(() => {});
    await fs.writeFile(path.join(evidence, `${host}-recovery-failure.json`), JSON.stringify({ error: error.stack, html: await page.content(), observations: fixture.observations, errors: monitor.errors, expectedFailures: monitor.expected }, null, 2));
    throw error;
  } finally { await browser.close(); await fixture.close(); }
}

async function uploadDiagram(fixture, bytes, instruction) {
  const uploadId = `presentation-${randomUUID()}`;
  const form = new FormData();
  form.append(`file:${uploadId}`, new Blob([bytes], { type: "image/png" }), "presentation-diagram.png");
  form.append("payload", JSON.stringify({ jsonrpc: "2.0", id: uploadId, method: "meta/console/send", params: {
    identity: "router:main", origin: "console:presentation-geometry", origin_kind: "operator", idempotency_key: uploadId,
    content: [{ type: "text", text: instruction }, { type: "image_upload", upload_id: uploadId, media_type: "image/png", alt: "Presentation dependency diagram" }],
  } }));
  const response = await fetch(`${fixture.baseUrl}/console/rpc/multipart`, { method: "POST", body: form });
  assert.equal(response.status, 200); const payload = await response.json();
  assert(payload.result?.input_frame_id, JSON.stringify(payload));
  const imageFrame = await eventually(async () => (await timeline(fixture)).frames.find(frame => frame.kind === "user_input"
    && JSON.stringify(frame.payload).includes(instruction)), "real uploaded image input");
  const image = imageFrame.payload.content.find(block => block.type === "image");
  assert(image?.blob_id, JSON.stringify(imageFrame));
  return { inputFrameId: imageFrame.id, blobId: image.blob_id };
}

async function layoutMutations(host) {
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 }, permissions: ["clipboard-read", "clipboard-write"] });
  const monitor = browserErrors(page);
  const result = { host, checks: [], errors: monitor.errors, expectedFailures: monitor.expected };
  let releaseImage;
  try {
    // Emit real WorkGraph tool calls so the disclosure contains owner records.
    const peer = (await rpc(fixture.baseUrl, "meta/cross_mob/peer_info", { member_id: "router:main" })).body.result;
    const parts = peer?.comms_name?.split("/");
    assert(parts?.length === 3 && parts[0] === peer.mob_id, JSON.stringify(peer));
    const runId = `layout-${randomUUID().slice(0, 8)}`;
    await fixture.control("model", { source: "Acknowledged.", chunk_chars: 4096, scenario: {
      kind: "workgraph", run_id: runId, owner_id: `mob/${peer.mob_id}/agent/${parts[2]}`,
    } });
    await sendApi(fixture, `[fixture:${runId}] Complete the two release prerequisites and inspect their real tool results.`);
    await completed(fixture, "WorkGraph scenario complete");

    const diagramPage = await browser.newPage({ viewport: { width: 640, height: 360 } });
    await diagramPage.setContent('<body style="margin:0;background:#f5f0e7;font:24px system-ui;color:#213547"><div style="padding:40px"><h2>Release review</h2><p>Source and image review feed the publication task.</p><div style="display:flex;gap:25px"><span style="padding:20px;background:#c5d8ee">Source</span><span style="padding:20px;background:#cbe0c2">Image</span><span style="padding:20px;background:#fff">Publish</span></div></div></body>');
    const bytes = await diagramPage.screenshot(); await diagramPage.close();
    await fixture.control("model", { source: "The uploaded diagram is available for the release review.", delay_ms: 0, chunk_chars: 4096 });
    result.image = await uploadDiagram(fixture, bytes, "Keep the dependency image above the reading anchor.");
    await completed(fixture, "The uploaded diagram is available");
    const anchorText = "Reading anchor: inspect this retained decision while earlier content changes.";
    await fixture.control("model", { source: prose(201).repeat(8), delay_ms: 0, chunk_chars: 4096 });
    await sendApi(fixture, anchorText);
    await completed(fixture, "Investigation 201");

    // Delay only the actual blob's network response. Do not fabricate a frame or image.
    const imageGate = new Promise(resolve => { releaseImage = resolve; });
    const imageUrl = `${fixture.baseUrl}/blobs/${encodeURIComponent(result.image.blobId)}`;
    let requestedImage = false;
    await page.route(imageUrl, async route => { requestedImage = true; await imageGate; await route.continue(); });
    await open(page, fixture, host, monitor);
    const viewport = transcript(page, host);
    const anchorRow = viewport.locator("[data-conversation-row-id]").filter({ hasText: anchorText }).last();
    await anchorRow.waitFor();
    // The image belongs to the turn just above the long anchor turn. The
    // windowed stock transcript mounts it once the reader is near it.
    await holdReadingRow(viewport, anchorRow);
    const delayedImage = viewport.locator(`img[src*="${encodeURIComponent(result.image.blobId)}"]`).first();
    await delayedImage.waitFor({ state: "attached" });
    await delayedImage.scrollIntoViewIfNeeded();
    await eventually(() => requestedImage, "browser requests actual delayed blob");
    assert.equal(await delayedImage.evaluate(node => node.complete), false, "blob remains pending until release");
    const anchor = await holdReadingRow(viewport, anchorRow);
    result.anchor = anchor;
    result.checks.push(await measureAnchor(viewport, anchor, async () => {
      releaseImage();
      await eventually(() => delayedImage.evaluate(node => node.complete && node.naturalWidth === 640), "delayed real image decodes");
    }, `${host} delayed blob load`));

    // Expanding a genuine completed tool above the reader cannot move the anchor.
    const closedHeader = viewport.locator('[data-testid^="workgraph-item:"][aria-expanded="false"], .cc-tool-call__header[role="button"][aria-expanded="false"]').first();
    assert(await closedHeader.count(), "real completed WorkGraph tool has a disclosure");
    const toolHeaderId = await closedHeader.getAttribute("data-testid");
    const headerIndex = toolHeaderId ? -1 : await closedHeader.evaluate(node => [...node.closest('[aria-label="Conversation transcript"]').querySelectorAll('.cc-tool-call__header[role="button"]')].indexOf(node));
    const toolHeader = toolHeaderId ? viewport.getByTestId(toolHeaderId) : viewport.locator('.cc-tool-call__header[role="button"]').nth(headerIndex);
    const toolTitle = await toolHeader.innerText();
    result.checks.push(await measureAnchor(viewport, anchor, async () => {
      // Activate the disclosure without Playwright scrolling the earlier control into view.
      await toolHeader.dispatchEvent("click");
      await eventually(async () => await toolHeader.getAttribute("aria-expanded") === "true", "tool disclosure expands");
    }, `${host} earlier completed tool disclosure`));
    result.disclosure = toolTitle;

    // Stock grows its composer through the supported image staging flow. Shared
    // host exposes a native textarea resize grip, exercised with a real drag.
    if (host === "stock") {
      const composer = page.getByTestId("chat-composer:router:main");
      result.checks.push(await measureAnchor(viewport, anchor, async () => {
        await composer.evaluate((node, data) => {
          const transfer = new DataTransfer(); transfer.items.add(new File([Uint8Array.from(data)], "staged-diagram.png", { type: "image/png" }));
          node.dispatchEvent(new DragEvent("drop", { bubbles: true, cancelable: true, dataTransfer: transfer }));
        }, Array.from(bytes));
        await page.locator(".composer__attachment img").waitFor();
      }, "stock composer attachment growth"));
    } else {
      const composer = page.getByRole("textbox", { name: "Message", exact: true });
      const box = await composer.boundingBox();
      result.checks.push(await measureAnchor(viewport, anchor, async () => {
        await page.mouse.move(box.x + box.width - 4, box.y + box.height - 4); await page.mouse.down();
        await page.mouse.move(box.x + box.width - 4, box.y + box.height + 90, { steps: 8 }); await page.mouse.up();
        assert((await composer.boundingBox()).height > box.height + 40, "native shared composer actually grows");
      }, "shared native composer resize"));
    }
    assert(result.checks.at(-1).after.height < result.checks.at(-1).before.height, "composer growth reduces actual transcript height");
    await capture(page, `${host}-layout-1600`);
    result.checks.push(await measureAnchor(viewport, anchor, () => page.setViewportSize({ width: 1440, height: 900 }), `${host} width and height resize`));
    await capture(page, `${host}-layout-1440`);
    result.checks.push(await measureAnchor(viewport, anchor, () => page.setViewportSize({ width: 1024, height: 768 }), `${host} compact viewport resize`));
    await capture(page, `${host}-layout-1024`);
    if (host === "shared") {
      await page.setViewportSize({ width: 1600, height: 1000 }); await settle(page);
      result.checks.push(await measureAnchor(viewport, anchor, () => page.getByRole("button", { name: "Toggle second pane", exact: true }).click(), "shared split-pane width change"));
      await capture(page, "shared-layout-split");
      assert.equal(await page.locator('[data-testid^="shared-pane-"]').count(), 2);
    }
    assert.deepEqual(monitor.errors, []);
  } catch (error) {
    result.failure = error.stack || String(error); await capture(page, `${host}-layout-failure`).catch(() => {}); throw error;
  } finally {
    releaseImage?.();
    result.html = result.failure ? await page.content() : undefined;
    result.failedRequests = fixture.observations.filter(item => item.status !== 200);
    await fs.mkdir(evidence, { recursive: true });
    await fs.writeFile(path.join(evidence, `${host}-layout-geometry.json`), JSON.stringify(result, null, 2));
    await browser.close(); await fixture.close();
  }
}

async function olderHistory(host) {
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: true });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  const monitor = browserErrors(page);
  const result = { host, turns: 0, pages: [], prepends: [], errors: monitor.errors, expectedFailures: monitor.expected };
  try {
    // Cross the owner's 1000-frame raw scan window as well as each host's
    // 200-frame recent page, then prove every actual turn remains reachable.
    const expected = [];
    for (let index = 0; index < 127; index += 1) {
      const input = `Review history checkpoint ${index}.`;
      const source = `History checkpoint ${index}: source, peer receipt and release decision retained.`;
      await fixture.control("model", { source, delay_ms: 0, chunk_chars: 4096 });
      const accepted = await sendApi(fixture, input);
      const complete = await completed(fixture, source);
      expected.push({ input, source, inputFrameId: accepted.input_frame_id, completeFrameId: complete.id });
      result.turns = index + 1;
    }
    const history = await timeline(fixture);
    assert(history.frames.length >= 1000 && Number(history.latest_cursor.split(":").at(-1)) > 1000,
      "actual owner history crosses both the recent-page and raw-scan boundaries");
    await open(page, fixture, host, monitor);
    const viewport = transcript(page, host);
    const ownerPages = () => fixture.observations.flatMap(item => {
      if (!item.request || !item.response) return [];
      try {
        const request = JSON.parse(item.request);
        if (request.method !== "meta/console/query_timeline" || request.params?.identity !== "router:main"
          || request.params?.mode !== "recent" || request.params?.limit !== 200) return [];
        return [{ observation: item, request, response: JSON.parse(item.response) }];
      } catch { return []; }
    });
    const seed = await eventually(() => {
      const item = ownerPages().filter(item => !item.request.params.before).at(-1);
      return item?.observation.status === 200 && item.response.result?.frames.length > 0 ? item : null;
    }, "host completes its actual authorized recent page after stream readiness");
    const cursorSeq = cursor => Number(cursor.split(":").at(-1));
    // The host may apply more than one recent page before its first older
    // request (a second load can follow stream readiness), and the owner
    // timeline can grow in between (committed-history backfill appends after
    // the last run). The host pages from the oldest cursor of every recent
    // page it applied, so the expected boundary is derived from those pages.
    const recentPages = () => ownerPages().filter(item => !item.request.params.before
      && item.observation.status === 200 && Array.isArray(item.response.result?.frames));
    const ownerFrames = new Map(seed.response.result.frames.map(frame => [frame.id, frame]));
    const adoptRecentPages = current => {
      let oldest = current;
      for (const item of recentPages()) {
        for (const frame of item.response.result.frames) {
          ownerFrames.set(frame.id, frame);
          if (cursorSeq(frame.cursor) < cursorSeq(oldest)) oldest = frame.cursor;
        }
      }
      return oldest;
    };
    const recordPage = (item, precedingBoundary) => {
      assert.equal(item.observation.status, 200);
      assert(Array.isArray(item.response.result?.frames), "older history request succeeds");
      const frames = item.response.result.frames;
      const before = item.request.params.before;
      const beforeSeq = Number(before.split(":").at(-1));
      assert(Number.isFinite(beforeSeq), "history request uses an owner cursor");
      assert.equal(before, precedingBoundary, "next page starts at the oldest retained owner cursor");
      assert(frames.every(frame => Number(frame.cursor.split(":").at(-1)) < beforeSeq),
        "every older frame is strictly before the requested owner boundary");
      for (const frame of frames) {
        assert(!ownerFrames.has(frame.id), `older pages do not repeat owner frame ${frame.id}`);
        ownerFrames.set(frame.id, frame);
      }
      result.pages.push({ request: item.request.params, exhausted: item.response.result.exhausted === true,
        frames: frames.map(frame => ({ id: frame.id, cursor: frame.cursor, kind: frame.kind })) });
      return frames[0]?.cursor ?? before;
    };
    // Frames the host had before any older page; a turn outside them is
    // reachable only through prepended history. (Counting rows here would
    // mean reading through the transcript, and reaching its top pages.)
    const recentFrameIds = new Set(recentPages().flatMap((item) => item.response.result.frames.map((frame) => frame.id)));
    // Hold the third row, or a later one when the recent page opens with a
    // turn so short that the view reaches its top first. What grows here is
    // the reply of a turn the recent page cut: it gains its "Worked for" line
    // once the older page brings the run's start.
    const anchor = await holdReadingRow(viewport, viewport.locator("[data-conversation-row-id]").nth(2));
    result.anchor = anchor;
    let boundary = seed.response.result.frames[0].cursor;
    let exhausted = seed.response.result.exhausted === true;
    for (let index = 0; !exhausted && index < 16; index += 1) {
      const beforePages = ownerPages().filter(item => item.request.params.before).length;
      result.prepends.push(await measureAnchor(viewport, anchor, async () => {
        // Stock may first need to reveal its already-loaded virtual window.
        const reveal = viewport.getByRole("button", { name: "Show earlier messages", exact: true });
        if (await reveal.count()) await reveal.dispatchEvent("click");
        const older = page.getByRole("button", { name: "Load older history", exact: true }).first();
        await older.waitFor({ state: "attached", timeout: 5000 });
        await eventually(() => older.isEnabled(), "history action leaves its previous loading state");
        await older.dispatchEvent("click");
        await eventually(() => ownerPages().filter(item => item.request.params.before).length > beforePages,
          "actual owner older-history request completes");
        await eventually(async () => !(await page.getByRole("button", { name: "Loading history", exact: true }).count()),
          "host applies the completed older page");
      }, `${host} actual history prepend ${index + 1}`));
      const newPages = ownerPages().filter(item => item.request.params.before).slice(beforePages);
      assert(newPages.length > 0);
      if (!result.pages.length) boundary = adoptRecentPages(boundary);
      for (const item of newPages) {
        boundary = recordPage(item, boundary);
        exhausted = item.response.result.exhausted === true;
      }
    }
    assert(exhausted, "repeated older-page loads reach the owner's history beginning");
    assert(result.pages.length > 1, "acceptance crosses multiple actual owner pages");
    const finalReveal = viewport.getByRole("button", { name: "Show earlier messages", exact: true });
    if (await finalReveal.count()) {
      result.prepends.push(await measureAnchor(viewport, anchor, () => finalReveal.dispatchEvent("click"),
        `${host} reveal retained first messages`));
    }
    await settle(page);
    // Reachable means a reader scrolling through finds it: the windowed stock
    // transcript mounts only the turns near the viewport.
    const positions = await readThrough(viewport, (node) => [...node.querySelectorAll("[data-conversation-row-id]")].filter((row) => !row.closest("[hidden]")).map((row) => ({
      id: row.dataset.conversationRowId,
      sources: [...(row.matches("[data-quote-source]") ? [row] : []), ...row.querySelectorAll("[data-quote-source]")]
        .map((quote) => quote.getAttribute("data-quote-source")),
    })));
    for (const position of positions) {
      const ids = position.map((row) => row.id);
      assert.equal(new Set(ids).size, ids.length, "retained transcript has no duplicate canonical rows");
    }
    const renderedRows = new Map(positions.flat().map((row) => [row.id, row.sources]));
    const quotedSources = [...renderedRows.values()].flat();
    for (let index = 0; index < expected.length; index += 1) {
      const turn = expected[index];
      assert.equal(ownerFrames.get(turn.inputFrameId)?.payload.content, turn.input,
        `${host} all older pages retain exact operator checkpoint ${index}`);
      assert(JSON.stringify(ownerFrames.get(turn.completeFrameId)?.payload).includes(turn.source),
        `${host} all older pages retain exact actual reply checkpoint ${index}`);
      assert.equal(quotedSources.filter(source => source === turn.input).length, 1,
        `${host} operator checkpoint ${index} is reachable exactly once in the actual transcript`);
      assert.equal(quotedSources.filter(source => source === turn.source).length, 1,
        `${host} reply checkpoint ${index} is reachable exactly once in the actual transcript`);
    }
    const rowIds = [...renderedRows.keys()];
    result.completeHistory = { operatorMessages: expected.length, modelReplies: expected.length,
      ownerFrames: ownerFrames.size, renderedRows: rowIds.length, pages: result.pages.length };
    // Each of these turns is reachable exactly once (above), so its rows were
    // prepended into the transcript.
    const prependedTurns = expected.filter((turn) => !recentFrameIds.has(turn.inputFrameId)).length;
    assert(prependedTurns * 2 >= 10, `multiple actual older messages prepend: ${prependedTurns} turns only in older pages`);
    if (host === "shared") {
      const rail = page.getByRole("navigation", { name: "Conversation turns" });
      const railIndexes = async () => rail.locator('[data-testid^="conversation-turn-rail:"]').evaluateAll(nodes =>
        nodes.map(node => Number(node.dataset.testid.split(":").at(-1))));
      const railGeometry = async () => rail.evaluate(node => {
        const viewport = document.querySelector('.cc-conversation-pane__scroll').getBoundingClientRect();
        const buttons = [...node.querySelectorAll('button')].map(button => button.getBoundingClientRect());
        return { top: Math.min(...buttons.map(rect => rect.top)), bottom: Math.max(...buttons.map(rect => rect.bottom)),
          viewportTop: viewport.top, viewportBottom: viewport.bottom,
          documentHeight: document.documentElement.scrollHeight, windowHeight: innerHeight, buttons: buttons.length };
      });
      const assertRailFits = async () => {
        const geometry = await railGeometry();
        assert(geometry.top >= geometry.viewportTop && geometry.bottom <= geometry.viewportBottom,
          `shared turn controls stay in the transcript band: ${JSON.stringify(geometry)}`);
        assert(geometry.documentHeight <= geometry.windowHeight + 1,
          `shared history rail does not expand document height: ${JSON.stringify(geometry)}`);
        assert(geometry.buttons <= 48);
        return geometry;
      };
      result.rail = { geometry: await assertRailFits() };
      const expectedTurns = await viewport.locator('[data-cc-conversation-turn-index]').count();
      const reached = new Set(await railIndexes());
      let railPages = 0;
      result.rail.anchor = await measureAnchor(viewport, anchor, async () => {
        while (await rail.getByRole("button", { name: "Show earlier turns", exact: true }).count()) {
          await rail.getByRole("button", { name: "Show earlier turns", exact: true }).click();
          (await railIndexes()).forEach(index => reached.add(index));
          await assertRailFits();
          assert(++railPages < 30, "rail earlier controls make progress");
        }
        while (await rail.getByRole("button", { name: "Show later turns", exact: true }).count()) {
          await rail.getByRole("button", { name: "Show later turns", exact: true }).click();
          (await railIndexes()).forEach(index => reached.add(index));
          await assertRailFits();
          assert(++railPages < 60, "rail later controls make progress");
        }
      }, "shared turn rail pages preserve reading position");
      assert.deepEqual([...reached].sort((a, b) => a - b), Array.from({ length: expectedTurns }, (_, index) => index),
        "every retained turn remains individually reachable through bounded rail controls");
      result.rail.reachableTurns = reached.size;
      result.rail.pages = railPages;
    }
    await capture(page, `${host}-real-history-prepend`);
    // Also inspect the recovered beginning, instead of only the retained recent anchor.
    await viewport.evaluate(node => { node.scrollTop = 0; node.dispatchEvent(new Event("scroll")); });
    await settle(page);
    await capture(page, `${host}-real-history-complete-start`);
    assert.deepEqual(monitor.errors, []);
  } catch (error) {
    result.failure = error.stack || String(error); await capture(page, `${host}-history-prepend-failure`).catch(() => {}); throw error;
  } finally {
    await fs.mkdir(evidence, { recursive: true });
    await fs.writeFile(path.join(evidence, `${host}-history-prepend.json`), JSON.stringify(result, null, 2));
    await browser.close(); await fixture.close();
  }
}

const scenarios = [
  ...["stock", "shared"].map(host => ({ id: `real-${host}-reading-intent`, family: "real-presentation", backend: "real", run: () => readingIntent(host) })),
  ...["stock", "shared"].map(host => ({ id: `real-${host}-presentation`, family: "real-presentation", backend: "real", run: () => presentation(host) })),
  ...["stock", "shared"].map(host => ({ id: `real-${host}-layout-mutations`, family: "real-presentation", backend: "real", run: () => layoutMutations(host) })),
  ...["stock", "shared"].map(host => ({ id: `real-${host}-history-prepend`, family: "real-presentation", backend: "real", run: () => olderHistory(host) })),
  ...["stock", "shared"].map(host => ({ id: `real-${host}-recovery`, family: "real-transport", backend: "real", run: () => recovery(host) })),
];
module.exports = { scenarios, geometry: { browserErrors, initializationCancellation, settle, timeline, completed, anchorAt, measureAnchor, mountEarlyRow, readThrough, open, transcript, conversationPane, seedReadingHistory } };

if (require.main === module) {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Use the coordinator prebuilt fixture; this lane must not run Cargo.");
  require("../scenario-registry.cjs").runScenarios(scenarios).catch(error => { process.stderr.write(`${error.stack || error}\n`); process.exitCode = 1; });
}
