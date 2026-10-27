"use strict";

const assert = require("node:assert/strict");
const fs = require("node:fs/promises");
const path = require("node:path");
const { randomUUID } = require("node:crypto");
const { chromium } = require("playwright");
const { startFixture, eventually, rpc } = require("../acceptance-runtime.cjs");
const { geometry } = require("./real-conversation.cjs");

const identity = "router:main";
const evidenceDir = process.env.META_BROWSER_EVIDENCE || path.join(__dirname, "../../output/playwright/console-acceptance");
const seedSource = "# Review context\n\n" + Array.from({ length: 18 }, (_, index) =>
  `### Evidence ${index + 1}\n\nThe release review includes the workgraph, agent communications and uploaded images. Keep this evidence available while a background review completes.\n\n`).join("");
const notice = "Background review complete: preserve A\u030A, \u00e5 and \ud83d\ude80 exactly; check the WorkGraph prerequisites before publishing.";
const finalSource = "## Background review incorporated\n\nThe current run checked the real WorkGraph ready set and received the durable review notice before its next model request. The instruction remains part of this conversation.\n\n| Check | Result |\n| --- | --- |\n| WorkGraph prerequisites | Reviewed through the runtime tool |\n| Background instruction | Kept once in the current run |\n| Operator draft | Preserved while the review completed |\n\nThe next step is to finish the release review. No publication was requested by this acceptance scenario.";
const draft = "My next instruction is still a draft. Keep A\u030A and \ud83d\ude80 intact.";
const textContent = content => typeof content === "string" ? content : (content || []).filter(block => block.type === "text").map(block => block.text).join("");
const assistantText = message => (message.blocks || []).filter(block => block.block_type === "text").map(block => block.data?.text ?? block.text ?? "").join("\n\n");
const liveEvent = frame => frame.source?.kind === "console_event";

function assertPeerSetupHistory(history, expected) {
  assert.equal(history.session_id, expected.completion.session_id, "actual recipient session");
  assert.equal(history.has_more, false, "complete recipient history");
  assert.equal(history.message_count, history.messages.length, "complete recipient history count");
  const incoming = history.messages.filter(message => message.role === "system_notice")
    .flatMap(message => (message.blocks || []).filter(block => block.type === "comms" && block.direction === "incoming"));
  const delivered = incoming.filter(block => block.kind === "message"
    && textContent(block.content) === `Peer message from ${expected.displayName}:\n${expected.body}`);
  assert.equal(delivered.length, 1, "one explicit peer message with exact delivered bytes");
  assert.deepEqual(delivered[0].peer, { id: expected.senderId, display_name: expected.displayName }, "canonical sender identity and display metadata");
  const replies = history.messages.filter(message => message.role === "block_assistant"
    && message.identity?.run_id === expected.completion.run_id
    && message.identity?.interaction_id === expected.completion.interaction_id);
  assert.equal(replies.length, 1, "one committed acknowledgement from the matching peer run");
  assert.equal(assistantText(replies[0]), expected.acknowledgement, "exact peer acknowledgement");
  const fromSender = incoming.filter(block => block.peer?.id === expected.senderId);
  for (const block of fromSender) assert.equal(block.peer.display_name, expected.displayName, "canonical sender display metadata");
  // A member-kickoff status (meerkat #1608, typed lifecycle or the older
  // request form) renders as a kickoff card, not an incoming peer row.
  const kickoffPhase = block => /^mob\.kickoff_([a-z_]+)$/.exec(block.intent || "")?.[1] || null;
  const peerRows = fromSender.filter(block => kickoffPhase(block) === null);
  const kickoffs = incoming.map(kickoffPhase).filter(Boolean).sort();
  return [{ id: expected.senderId, displayName: expected.displayName, label: "domain:delivery", count: peerRows.length, kickoffs }];
}

function expectedNavigationCancellation(request) {
  if (!request.failure()?.errorText?.includes("ERR_ABORTED")) return false;
  if (new URL(request.url()).pathname.endsWith("/timeline/stream")) return true;
  try {
    const body = JSON.parse(request.postData() || "{}");
    return body.method === "meta/console/query_timeline" && body.params?.mode === "recent"
      && body.params?.limit === 200 && !body.params?.identity;
  } catch { return false; }
}

async function durableSteer(host, persistedBackgroundJob = false) {
  assert(process.env.META_EXAMPLE_BIN_DIR, "Use the coordinator's prebuilt durable-steer fixture; never invoke Cargo from this browser case.");
  const fixture = await startFixture();
  const browser = await chromium.launch({ headless: process.env.META_HEADED !== "1" });
  const page = await browser.newPage({ viewport: { width: 1600, height: 1000 } });
  page.setDefaultTimeout(20_000);
  const barrierId = `durable-${host}-${randomUUID().slice(0, 8)}`;
  const backgroundJob = persistedBackgroundJob ? { job_id: barrierId, display_name: "release review" } : null;
  const expectedBody = backgroundJob
    ? `Background ${backgroundJob.display_name} job ${backgroundJob.job_id} finished (completed):` : notice;
  const expectedBlocks = backgroundJob
    ? [{ type: "background_job", ...backgroundJob, status: "completed", detail: notice, persisted: true }] : [];
  const modelProjection = backgroundJob ? `${expectedBody}\n${notice}` : notice;
  const evidenceName = `${host}-${backgroundJob ? "persisted-background-job" : "durable-steer"}`;
  const result = { host, backgroundJob, notice, finalSource, draft, errors: [], expectedCancellations: [], views: {} };
  const instruction = `Review this release while the background check runs. Exact turn marker ${barrierId}.`;
  let allowance = "navigation";
  let armed = false;
  const pane = () => host === "shared" ? page.getByTestId("shared-pane-0") : page.getByTestId(`chat-pane:${identity}`).first();
  const viewport = () => pane().locator(host === "shared" ? ".cc-conversation-pane__scroll" : ".conv__body");
  const composer = () => host === "shared" ? pane().getByRole("textbox", { name: "Message", exact: true }) : pane().getByTestId(`chat-composer:${identity}`);
  page.on("pageerror", error => result.errors.push(error.message));
  page.on("requestfailed", request => {
    const detail = { url: request.url(), error: request.failure()?.errorText };
    if ((allowance === "navigation" && expectedNavigationCancellation(request))
      || (allowance === "disconnect" && new URL(request.url()).pathname.endsWith("/timeline/stream"))) {
      result.expectedCancellations.push({ ...detail, reason: allowance });
    } else result.errors.push(detail);
  });
  page.on("response", response => {
    if (response.status() >= 400) result.errors.push({ url: response.url(), status: response.status() });
  });
  async function read(url) {
    const response = await fetch(url);
    assert.equal(response.status, 200, `${response.status}: ${await response.clone().text()}`);
    return response.json();
  }
  const frames = async (target = identity) => (await read(`${fixture.baseUrl}/console/timeline?identity=${encodeURIComponent(target)}&mode=recent&limit=1000`)).frames;
  const requests = () => read(`${fixture.backendUrl}/__fixture/requests`);
  const state = () => read(`${fixture.backendUrl}/__fixture/durable-steer?session_id=${encodeURIComponent(result.accepted.session_id)}&input_id=${encodeURIComponent(result.accepted.input_id)}`);
  const history = () => read(`${fixture.backendUrl}/__fixture/session-history?session_id=${encodeURIComponent(result.accepted.session_id)}`);
  async function send(content, key, target = identity) {
    const response = await rpc(fixture.baseUrl, "meta/console/send", {
      identity: target, content, origin: "console:durable-steer-acceptance", origin_kind: "operator",
      handling_mode: "queue", idempotency_key: key,
    });
    assert.equal(response.status, 200);
    assert(!response.body.error, JSON.stringify(response.body));
    assert(response.body.result?.input_frame_id && response.body.result.interaction_id);
    return response.body.result;
  }
  async function open(reload = false) {
    allowance = "navigation";
    if (reload) await page.reload(); else await page.goto(fixture.baseUrl + (host === "shared" ? "/shared" : "/scoped"));
    if (host === "stock" && !await pane().count()) await page.locator('.agent[role="button"], .cc-sidebar-row').filter({ hasText: /router:main/ }).first().click();
    if (host === "shared") await page.getByRole("combobox", { name: "Agent", exact: true }).selectOption(identity);
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    await composer().waitFor();
    allowance = null;
  }
  async function capture(label) {
    await fs.mkdir(evidenceDir, { recursive: true });
    await page.screenshot({ path: path.join(evidenceDir, `${evidenceName}-${label}.png`), fullPage: true });
  }
  const matchesNotice = message => message.role === "system_notice" && (backgroundJob
    ? (message.blocks || []).some(block => block.type === "background_job" && block.job_id === backgroundJob.job_id)
    : message.body === notice);
  function assertTypedNotice(message, label) {
    assert.equal(message.kind, backgroundJob ? "background_job" : "generic", `${label}: exact typed notice kind`);
    assert.equal(message.body, expectedBody, `${label}: exact notice body`);
    assert.deepEqual(message.blocks || [], expectedBlocks, `${label}: exact persisted job block and result bytes`);
    assert.deepEqual(message.runtime_origin, {
      session_id: result.runStart.session_id, run_id: result.runStart.run_id,
      input_id: result.accepted.input_id, append_ordinal: 0,
    }, `${label}: the real application retains its exact session, run, input and ordinal`);
    assert(Number.isFinite(Date.parse(message.created_at)), `${label}: owner timestamp is present`);
    if (result.noticeRecord) {
      assert.deepEqual(message, { role: "system_notice", ...result.noticeRecord },
        `${label}: every typed notice field equals the actual boundary event`);
    }
  }
  const inputStillRunning = current => current.current_run_id === result.runStart.run_id
    && current.terminal_outcome === null && current.completion === null;
  async function inspectNotice(label, requireRunning = false) {
    const rendered = await eventually(async () => {
      const before = requireRunning ? await state() : null;
      // Return this failure out of the polling helper immediately. A late
      // history row must never satisfy the live-rendering assertion.
      if (before && !inputStillRunning(before)) return { endedBeforeLiveObservation: before };
      const view = await viewport().evaluate((root, expected) => {
        const rows = [...root.querySelectorAll("[data-conversation-row-id]")]
          .filter(row => row.textContent.includes(expected));
        const workGraphTools = [...root.querySelectorAll(".cc-tool-call")]
          .filter(tool => tool.querySelector(".cc-tool-call__name")?.textContent === "workgraph_ready")
          .map(tool => ({
            rowId: tool.closest("[data-conversation-row-id]")?.dataset.conversationRowId,
            status: tool.querySelector(".cc-tool-call__status")?.textContent,
            displayed: tool.getBoundingClientRect().height > 0,
            beforeNotice: Boolean(rows[0] && (tool.compareDocumentPosition(rows[0]) & Node.DOCUMENT_POSITION_FOLLOWING)),
          }));
        return {
          text: root.textContent,
          matches: root.textContent.split(expected).length - 1,
          rows: rows.map(row => ({
            id: row.dataset.conversationRowId,
            text: row.textContent,
            workFooters: row.querySelectorAll('.msg__worked, [aria-label="Copy work time"]').length,
          })),
          backgroundJobs: [...root.querySelectorAll(".cc-background-job")].map(card => {
            const bounds = card.getBoundingClientRect(), viewportBounds = root.getBoundingClientRect();
            return { id: card.getAttribute("data-job-id"),
              name: card.querySelector(".cc-background-job__name")?.textContent,
              status: card.querySelector(".cc-background-job__status")?.textContent,
              rawStatus: card.querySelector(".cc-background-job__status")?.getAttribute("data-status"),
              detail: card.querySelector(".cc-background-job__detail")?.textContent,
              width: bounds.width, fits: bounds.left >= viewportBounds.left && bounds.right <= viewportBounds.right,
            };
          }),
          incomingPeers: [...root.querySelectorAll(".cc-tool-call--incoming .cc-tool-call__name")]
            .map(peer => ({ text: peer.textContent, title: peer.getAttribute("title"), displayed: peer.getBoundingClientRect().height > 0 })),
          workGraphTools,
        };
      }, notice);
      if (!requireRunning) {
        // The peer rows belong to earlier turns, which the windowed stock
        // transcript mounts only near the viewport. Once the run settled,
        // read the transcript through as a reader scrolling it does.
        const positions = await geometry.readThrough(viewport(), (root, expected) => [...root.querySelectorAll("[data-conversation-row-id]")].filter((row) => !row.closest("[hidden]")).map((row) => ({
          id: row.dataset.conversationRowId,
          notice: row.textContent.includes(expected),
          peers: [...row.querySelectorAll(".cc-tool-call--incoming .cc-tool-call__name")]
            .map((peer) => ({ text: peer.textContent, title: peer.getAttribute("title"), displayed: peer.getBoundingClientRect().height > 0 })),
          kickoffs: [...row.querySelectorAll(".cc-member-kickoff")].map((card) => card.getAttribute("data-phase")),
        })), notice);
        const rows = new Map(positions.flat().map((row) => [row.id, row]));
        view.noticeRowIds = [...rows.values()].filter((row) => row.notice).map((row) => row.id);
        view.incomingPeers = [...rows.values()].flatMap((row) => row.peers);
        view.kickoffs = [...rows.values()].flatMap((row) => row.kickoffs).sort();
      }
      result.lastNoticeProbe = { label, view, before };
      const after = requireRunning ? await state() : null;
      if (after && !inputStillRunning(after)) return { endedBeforeLiveObservation: after };
      try {
        assert.equal(view.matches, 1, "the durable instruction appears exactly once in the real transcript");
        assert(!view.text.includes("boundary_append_applied") && !view.text.includes('"input_id"') && !view.text.includes('"append_count"'),
          "the transcript renders the instruction without raw transport envelopes");
        assert.equal(view.rows.length, 1, "one stable rendered row owns the durable notice");
        if (view.noticeRowIds) assert.deepEqual(view.noticeRowIds, [result.noticeRowId], "no other row in the whole transcript repeats the durable notice");
        if (view.kickoffs) {
          assert.deepEqual(view.kickoffs, result.expectedPeers.flatMap(peer => peer.kickoffs),
            "each canonical member-kickoff notice renders as one kickoff card with its phase");
        }
        assert.equal(view.rows[0].id, result.noticeRowId, "the rendered row belongs to the exact runtime notice source");
        assert.equal(view.rows[0].workFooters, 0, "the typed System notice has no assistant work-duration footer");
        assert.doesNotMatch(view.rows[0].text, /Worked for/);
        if (backgroundJob) {
          assert.equal(view.backgroundJobs.length, 1, "one typed job card remains visible");
          const card = view.backgroundJobs[0];
          assert.equal(card.id, backgroundJob.job_id);
          assert.equal(card.name, backgroundJob.display_name);
          assert.equal(card.status, "Completed");
          assert.equal(card.rawStatus, "completed");
          assert.equal(card.detail, notice, "job detail retains exact original bytes");
          assert(card.width > 0 && card.fits, "job card fits the conversation viewport");
        }
        for (const peer of result.expectedPeers) {
          const matching = view.incomingPeers.filter(rendered => rendered.title === peer.id);
          assert.equal(matching.length, peer.count, "each canonical incoming peer row retains its exact peer identity in the header");
          for (const rendered of matching) {
            assert.equal(rendered.text, `Received from ${peer.label}`, "canonical display metadata supplies the readable peer label");
            assert.equal(rendered.displayed, true);
          }
        }
        if (host === "stock") {
          assert.equal(view.workGraphTools.length, 1, "the empty WorkGraph query remains visible as one tool result");
          assert.equal(view.workGraphTools[0].displayed, true);
          assert.match(view.workGraphTools[0].status || "", /Success/);
          assert.equal(view.workGraphTools[0].beforeNotice, true, "the durable notice follows the real WorkGraph result in the rendered transcript");
        }
      } catch (error) {
        result.lastNoticeProbe.assertion = error.message;
        throw error;
      }
      if (requireRunning) view.liveObservation = { before, after };
      return view;
    }, `${host} ${label}: one readable durable instruction`);
    assert(!rendered.endedBeforeLiveObservation,
      `the original run completed before its notice was rendered live: ${JSON.stringify(rendered.endedBeforeLiveObservation)}`);
    result.views[label] = rendered;
    return rendered;
  }
  // The reading anchor's position once it has come to rest. Layout passes
  // after a programmatic scroll (the transcript window mounting the turns
  // around it, the scroll controller's anchor restore) run in the frames
  // after it lands, so a reading is taken only once the heading has held its
  // exact offset for `stableMs` of frames. Main CI run 38511905113 read its
  // baseline 98 px from where the view came to rest, before any delivery: the
  // controller had anchored on a row of the parked turn above, which had
  // outgrown its placeholder, and unparking moved the heading instead (fixed
  // in the scroll controller). An anchor that keeps moving fails here; one
  // that comes to rest elsewhere fails the comparison. The evidence JSON
  // keeps, for every reading, the box read at once (what a single read would
  // have compared), each offset change while it settled and the resting box,
  // so a failure can be classified from the artifact alone.
  async function restingReading(label, { stableMs = 500, timeoutMs = 5_000 } = {}) {
    const immediate = await viewport().getByRole("heading", { name: "Evidence 3", exact: true })
      .boundingBox({ timeout: 1_000 }).catch(error => ({ error: String(error?.message || error) }));
    const reading = await viewport().evaluate((root, { stableMs, timeoutMs }) => new Promise((resolve) => {
      const started = performance.now();
      const moves = [];
      let lastY;
      let since = started;
      const anchor = () => {
        const heading = [...root.querySelectorAll('h1, h2, h3, h4, h5, h6, [role="heading"]')]
          .find((node) => node.textContent.trim() === "Evidence 3" && !node.closest("[hidden]"));
        if (!heading) return null;
        const { x, y, width, height } = heading.getBoundingClientRect();
        return { x, y, width, height };
      };
      const tick = () => {
        const rect = anchor();
        const y = rect ? rect.y : null;
        const now = performance.now();
        if (y !== lastY) {
          moves.push({ atMs: Math.round(now - started), y });
          lastY = y;
          since = now;
        }
        if (rect && now - since >= stableMs) resolve({ rested: true, rect, waitedMs: Math.round(now - started), moves });
        else if (now - started >= timeoutMs) resolve({ rested: false, rect, waitedMs: Math.round(now - started), moves });
        else requestAnimationFrame(tick);
      };
      requestAnimationFrame(tick);
    }), { stableMs, timeoutMs });
    (result.readings ||= {})[label] = { immediate, ...reading };
    assert(reading.rested, `${label}: the reading anchor comes to rest within ${timeoutMs} ms`);
    return reading.rect;
  }
  async function assertDraftAndReading(label) {
    const current = await composer().evaluate(node => ({ value: node.value, start: node.selectionStart, end: node.selectionEnd, focused: document.activeElement === node }));
    assert.deepEqual(current, result.draftBefore, "streaming the notice preserves draft, selection and keyboard focus");
    const after = await restingReading(label);
    (result.readingAfter ||= {})[label] = after;
    assert(after && Math.abs(after.y - result.readingBefore.y) <= 2, "background delivery preserves the exact reading anchor");
  }
  async function inspectAssistantDuration(label) {
    if (host !== "stock") return null;
    const view = await eventually(async () => {
      const rows = await viewport().evaluate(root => [...root.querySelectorAll("[data-conversation-row-id]")]
        .filter(row => [...row.querySelectorAll("h2")].some(heading => heading.textContent === "Background review incorporated"))
        .map(row => ({
          id: row.dataset.conversationRowId,
          sourceKind: row.dataset.sourceKind,
          footers: [...row.querySelectorAll(".msg__worked")].map(footer => footer.textContent),
          copyButtons: row.querySelectorAll('[aria-label="Copy work time"]').length,
        })));
      result.lastAssistantDurationProbe = { label, rows, expected: result.workDuration };
      try {
        assert.equal(rows.length, 1, "one final assistant row owns the completed review");
        assert.equal(rows[0].sourceKind, "assistant");
        assert.deepEqual(rows[0].footers, [result.workDuration.text], "the assistant footer matches the exact run's actual start-to-completion duration");
        assert.equal(rows[0].copyButtons, 0, "the proven duration is a plain footer without a separate copy action");
      } catch (error) {
        result.lastAssistantDurationProbe.assertion = error.message;
        throw error;
      }
      return rows[0];
    }, `${host} ${label}: the final assistant footer uses actual matching run timestamps`);
    result.durationViews ||= {};
    result.durationViews[label] = view;
    return view;
  }
  async function inspectApproval(label) {
    const card = viewport().getByTestId(`gating-pending:${result.approval.pending_id}`);
    await eventually(async () => await card.count() === 1, `${host} ${label}: one exact approval in this transcript`);
    const view = await card.evaluate(node => ({ pendingId: node.dataset.testid,
      turnId: node.closest("[data-conversation-turn-id]")?.dataset.conversationTurnId,
      text: node.textContent }));
    assert(view.turnId, "correlated approval belongs to a concrete visible conversation turn");
    assert(view.text.includes("Review this exact background-check interaction"));
    const pending = await rpc(fixture.baseUrl, "meta/gating/pending");
    const owner = pending.body.result?.pending.find(item => item.pending_id === result.approval.pending_id);
    assert.equal(owner?.origin?.interaction_id, result.started.interaction_id,
      "the runtime approval owner retains the exact active interaction");
    assert.equal(fixture.observations.filter(item => item.request.includes('"method":"meta/gating/decide"')).length, 0,
      "reading the correlated approval never decides it");
    result.approvalViews ||= {};
    result.approvalViews[label] = view;
    return view;
  }
  try {
    // Kickoff can finish before the fixture wires members. Establish an
    // explicit delivery after wiring instead of relying on lifecycle timing.
    const peers = await read(`${fixture.backendUrl}/__fixture/peers`);
    const sender = "domain:delivery";
    const peerRunId = `notice-peer-${randomUUID().slice(0, 8)}`;
    const peer = {
      senderId: peers[sender]?.[0], displayName: peers[sender]?.[1],
      recipientId: peers[identity]?.[0],
      body: `Review the release prerequisites before the background check. Exact peer marker ${peerRunId}.`,
      acknowledgement: `Release peer context acknowledged for ${peerRunId}.`,
    };
    assert.equal(typeof peer.senderId, "string");
    assert.equal(typeof peer.recipientId, "string");
    assert.equal(peer.displayName, "console-acceptance/lead/mk--domain_cdelivery");
    result.peerSetup = peer;
    await fixture.control("model", { source: peer.acknowledgement, delay_ms: 0, chunk_chars: 4096,
      scenario: { kind: "peer", run_id: peerRunId, peer_id: peer.recipientId, peer_body: peer.body },
    });
    peer.accepted = await send(`[fixture:${peerRunId}] Send the release review context to the router.`, `${peerRunId}-send`, sender);
    peer.senderCompletion = await eventually(async () => (await frames(sender)).find(frame => liveEvent(frame)
      && frame.kind === "interaction_complete" && frame.payload?.source_event_type === "run_completed"
      && frame.interaction_id === peer.accepted.interaction_id), "explicit peer sender completes after its real send receipt");
    peer.senderFrames = await frames(sender);
    const callId = `fixture-${peerRunId}-send-peer`;
    const calls = peer.senderFrames.filter(frame => liveEvent(frame) && frame.kind === "tool_call_requested" && frame.payload?.id === callId);
    const results = peer.senderFrames.filter(frame => liveEvent(frame) && frame.kind === "tool_result_received" && frame.payload?.id === callId);
    assert.equal(calls.length, 1, "one actual peer send tool call");
    assert.equal(calls[0].payload.name, "send_message");
    assert.deepEqual(calls[0].payload.args, { peer_id: peer.recipientId, body: peer.body, handling_mode: "queue" });
    assert.equal(results.length, 1, "one actual peer send tool result");
    assert.equal(results[0].payload.is_error, false);
    peer.receipt = JSON.parse(textContent(results[0].payload.content));
    assert.equal(peer.receipt.status, "sent");
    assert.equal(peer.receipt.receipt?.kind, "peer_message_sent");
    assert.equal(typeof peer.receipt.receipt.envelope_id, "string");
    peer.completion = await eventually(async () => (await frames()).find(frame => liveEvent(frame)
      && frame.kind === "interaction_complete" && frame.payload?.source_event_type === "run_completed"
      && frame.interaction_id === peer.receipt.receipt.envelope_id), "explicit peer recipient completes its matching envelope");
    assert.equal(peer.completion.payload.result, peer.acknowledgement);
    assert(peer.completion.session_id && peer.completion.run_id);
    await fixture.control("model", { source: seedSource, delay_ms: 0, chunk_chars: 4096 });
    const seeded = await send("Read the release evidence before the background review.", `${barrierId}-seed`);
    const seedCompletion = await eventually(async () => (await frames()).find(frame => liveEvent(frame) && frame.kind === "interaction_complete" && frame.interaction_id === seeded.interaction_id), "seed review completes through the real runtime");
    assert(seedCompletion.session_id);
    // The owner history read waits behind an active model turn. Read the seed
    // metadata before holding the next turn at its explicit model barrier.
    assert.equal(seedCompletion.session_id, peer.completion.session_id, "review and explicit peer share the recipient session");
    result.expectedPeers = await eventually(async () => {
      result.seededHistory = await read(`${fixture.backendUrl}/__fixture/session-history?session_id=${encodeURIComponent(seedCompletion.session_id)}`);
      return assertPeerSetupHistory(result.seededHistory, peer);
    }, "explicit peer delivery and matching acknowledgement are in canonical history before the review barrier");
    await open();
    await viewport().getByRole("heading", { name: "Evidence 3", exact: true }).waitFor();
    // The streamed reply remains visibly active long enough to inspect the
    // boundary event before completion. Progress is synchronized by events.
    await fixture.control("model", { source: "Unexpected fallback response.", delay_ms: 80, chunk_chars: 8 });
    result.armed = await fixture.control("model-barrier", { action: "arm", plan: { id: barrierId, match_text: instruction, source: finalSource } });
    armed = true;
    result.started = await send(instruction, `${barrierId}-turn`);
    result.entered = await eventually(async () => {
      const current = await fixture.control("model-barrier", { action: "status", id: barrierId });
      return current.entered && !current.released ? current : null;
    }, "the intended model request reaches its explicit barrier");
    result.runStart = await eventually(async () => (await frames()).find(frame => liveEvent(frame) && frame.kind === "run_started" && frame.interaction_id === result.started.interaction_id), "the running turn has canonical run and session ownership");
    assert(result.runStart.run_id && result.runStart.session_id);
    result.approval = await fixture.control("approval", { identity, interaction_id: result.started.interaction_id,
      action: "Review this exact background-check interaction" });
    assert(result.approval.pending_id);
    await inspectApproval("waiting");
    await composer().fill(draft);
    await composer().evaluate(node => node.setSelectionRange(3, 17));
    await viewport().getByRole("heading", { name: "Evidence 3", exact: true }).scrollIntoViewIfNeeded();
    result.draftBefore = await composer().evaluate(node => ({ value: node.value, start: node.selectionStart, end: node.selectionEnd, focused: document.activeElement === node }));
    assert.equal(result.draftBefore.focused, true);
    result.readingBefore = await restingReading("before");
    assert(result.readingBefore);
    await capture("waiting-with-draft");

    result.accepted = await fixture.control("durable-steer", {
      identity, session_id: result.runStart.session_id, content: notice,
      ...(backgroundJob ? { background_job: backgroundJob } : {}),
    });
    assert.equal(result.accepted.accepted, true);
    assert.equal(result.accepted.session_id, result.runStart.session_id);
    assert(result.accepted.input_id);
    result.admitted = await state();
    assert.equal(result.admitted.current_run_id, result.runStart.run_id, "durable admission preserves the currently running turn");
    assert.equal(result.admitted.terminal_outcome, null, "admission alone does not consume the durable input");
    assert.equal(result.admitted.completion, null, "a completed background job does not complete the receiving run");
    result.released = await fixture.control("model-barrier", { action: "release", id: barrierId });
    result.applied = await eventually(async () => (await frames()).find(frame => liveEvent(frame) && frame.kind === "boundary_append_applied" && frame.payload?.input_id === result.accepted.input_id), "the actual runtime applies the exact durable input at its next boundary");
    assert.equal(result.applied.run_id, result.runStart.run_id);
    assert.equal(result.applied.payload.run_id, result.runStart.run_id);
    assert.equal(result.applied.payload.append_count, 1);
    assert.equal(textContent(result.applied.payload.content), modelProjection, "the model projection includes the outcome exactly once");
    assert.equal(result.applied.session_id, result.runStart.session_id);
    assert.equal(result.applied.runtime_key, result.runStart.runtime_key);
    assert.equal(result.applied.identity, identity);
    assert.equal(result.applied.source_event_id, result.applied.id, "the boundary event retains its canonical source identity");
    assert.equal(result.applied.payload.notices.length, 1, "one actual typed notice was applied");
    assertTypedNotice(result.applied.payload.notices[0], "boundary");
    result.noticeRecord = result.applied.payload.notices[0];
    result.noticeRowId = `runtime-notice:${JSON.stringify([
      result.applied.runtime_key, result.applied.session_id, result.noticeRecord.runtime_origin.session_id,
      result.noticeRecord.runtime_origin.input_id, result.noticeRecord.runtime_origin.append_ordinal,
    ])}`;
    result.duringRun = await state();
    assert.equal(result.duringRun.current_run_id, result.runStart.run_id, "the append is visible before the same run completes");
    const duringView = await inspectNotice("during-run", true);
    await inspectApproval("during-run");
    await assertDraftAndReading("during-run");
    result.afterLiveReading = await state();
    assert(inputStillRunning(result.afterLiveReading), "live notice and preserved typing/reading were observed before the original run completed");
    await capture("notice-preserves-reading");

    result.completed = await eventually(async () => {
      const current = await state();
      return current.terminal_outcome?.outcome_type === "consumed" && current.completion
        && current.current_run_id === null && current.queue.length === 0 && current.steer_queue.length === 0 ? current : null;
    }, "the durable input and the original run converge without a follow-up", 30_000);
    assert.equal(result.completed.run_id, result.runStart.run_id, "the durable input is consumed by the original run");
    assert.equal(result.completed.completion.completion_type, "completed", "the retained input shares its run's successful result receipt");
    const isOwnerCompletion = frame => liveEvent(frame)
      && frame.kind === "interaction_complete" && frame.payload?.source_event_type === "run_completed"
      && ["runtime_key", "identity", "session_id", "run_id", "interaction_id"].every(key => frame[key] === result.runStart[key]);
    // The owner's durable receipt can precede asynchronous timeline publication.
    result.finalFrames = await eventually(async () => {
      const current = await frames();
      return current.some(isOwnerCompletion) ? current : null;
    }, "the exact original-run completion reaches the console timeline");
    assert.equal(result.finalFrames.filter(frame => liveEvent(frame) && frame.kind === "boundary_append_applied" && frame.payload?.input_id === result.accepted.input_id).length, 1);
    const completions = result.finalFrames.filter(isOwnerCompletion);
    assert.equal(completions.length, 1, "the original run has one completion with exact runtime, agent, session, run and interaction ownership");
    result.runCompletion = completions[0];
    if (host === "stock") {
      assert(Number.isSafeInteger(result.runStart.timestamp_ms) && Number.isSafeInteger(result.runCompletion.timestamp_ms));
      const elapsedMs = result.runCompletion.timestamp_ms - result.runStart.timestamp_ms;
      assert(elapsedMs >= 1000 && elapsedMs < 60_000, "this paced fixture provides a measurable run duration within one minute");
      result.workDuration = { elapsedMs, text: `Worked for ${Math.round(elapsedMs / 1000)}s` };
    }
    const turnRequests = (await requests()).filter(request => request.messages.some(message => message.role === "user" && textContent(message.content) === instruction));
    assert.equal(turnRequests.length, 2, "only the blocked request and one post-tool request execute; no follow-up turn");
    assert.equal(turnRequests[0].messages.filter(matchesNotice).length, 0);
    const modelNotices = turnRequests[1].messages.filter(matchesNotice);
    assert.equal(modelNotices.length, 1, "the next actual model request carries the durable notice once");
    assertTypedNotice(modelNotices[0], "next model request");
    assert.equal(JSON.stringify(turnRequests[1].messages).split(JSON.stringify(notice).slice(1, -1)).length - 1, 1,
      "the actual model request contains the exact result bytes once across all message roles");
    assert(turnRequests[1].messages.some(message => message.role === "tool_results" && message.results.some(tool => tool.tool_use_id === `fixture-${barrierId}-peer-ready` && !tool.is_error)), "a real WorkGraph tool result establishes the cooperative boundary");
    result.turnRequests = turnRequests;
    result.history = await history();
    assert.equal(result.history.has_more, false);
    const positions = result.history.messages.flatMap((message, index) => matchesNotice(message) ? [index] : []);
    assert.equal(positions.length, 1, "the committed session has exactly one durable instruction");
    const at = positions[0];
    assertTypedNotice(result.history.messages[at], "committed history");
    assert.equal(result.history.messages[at - 1].role, "tool_results");
    const answer = result.history.messages[at + 1];
    assert.equal(answer.role, "block_assistant");
    assert.equal(answer.identity.run_id, result.runStart.run_id);
    assert.equal(assistantText(answer), finalSource);
    await assertDraftAndReading("completed");
    const completedView = await inspectNotice("completed");
    await inspectApproval("completed");
    assert.deepEqual(completedView.rows, duringView.rows, "the exact live notice row and content survive the committed history reconciliation");
    const completedDuration = await inspectAssistantDuration("completed");
    await viewport().locator(`[data-conversation-row-id=${JSON.stringify(result.noticeRowId)}]`).scrollIntoViewIfNeeded();
    await capture("completed");
    if (backgroundJob) {
      await page.setViewportSize({ width: 1024, height: 900 });
      await viewport().locator(`[data-conversation-row-id=${JSON.stringify(result.noticeRowId)}]`).scrollIntoViewIfNeeded();
      await inspectNotice("completed-1024");
      await capture("completed-1024");
      await page.setViewportSize({ width: 1600, height: 1000 });
    }

    allowance = "disconnect";
    const streams = () => fixture.observations.filter(item => item.path.includes("/timeline/stream") && item.status === 200).length;
    const before = streams(); fixture.disconnectStreams();
    await eventually(() => streams() > before, "the browser reconnects to an actual successful stream");
    await page.locator('[data-testid="console-transport-status"][data-phase="live"]').waitFor();
    allowance = null;
    const reconnected = await inspectNotice("reconnected");
    await inspectApproval("reconnected");
    assert.deepEqual(reconnected.rows, completedView.rows, "reconnect preserves the exact notice row");
    assert.equal(await composer().inputValue(), draft);
    await open(true);
    const reloaded = await inspectNotice("reloaded");
    await inspectApproval("reloaded");
    assert.deepEqual(reloaded.rows, completedView.rows, "history reconciliation and reload preserve the exact notice row");
    assert.deepEqual(await inspectAssistantDuration("reloaded"), completedDuration, "reload preserves the final assistant row and proven duration exactly");
    assert.deepEqual((await history()).messages, result.history.messages, "reload does not alter committed history");
    // Stock owns draft persistence. The reusable test host keeps draft state
    // in React only, so reload is a transcript check there.
    if (host === "stock") assert.equal(await composer().inputValue(), draft, "reload restores the unsent operator draft");
    await viewport().locator(`[data-conversation-row-id=${JSON.stringify(result.noticeRowId)}]`).scrollIntoViewIfNeeded();
    await capture("reloaded");
    const peerLabel = viewport().locator(".cc-tool-call--incoming .cc-tool-call__name")
      .filter({ hasText: "Received from domain:delivery" }).first();
    // The peer delivery is in an early turn, mounted only near the start.
    await geometry.mountEarlyRow(viewport(), peerLabel);
    await peerLabel.scrollIntoViewIfNeeded();
    await capture("peer-label");
    const apiFailures = fixture.observations.filter(item => item.status >= 400 || (item.response && (() => { try { return Boolean(JSON.parse(item.response).error); } catch { return false; } })()));
    assert.deepEqual(apiFailures, [], "no unexpected actual API failures");
    assert.deepEqual(result.errors, [], "no unexpected browser or network failures");
  } catch (error) {
    result.failure = error.stack || String(error);
    await capture("failure").catch(() => {});
    throw error;
  } finally {
    if (armed) await fixture.control("model-barrier", { action: "release", id: barrierId }).catch(() => {});
    result.requests = await requests().catch(() => []);
    result.frames = await frames().catch(() => []);
    if (result.accepted) {
      result.finalState = await state().catch(error => ({ error: String(error) }));
      result.finalHistory = await history().catch(error => ({ error: String(error) }));
    }
    result.observations = fixture.observations;
    result.logs = fixture.logs();
    await fs.mkdir(evidenceDir, { recursive: true });
    await fs.writeFile(path.join(evidenceDir, `${evidenceName}.json`), JSON.stringify(result, null, 2));
    await browser.close(); await fixture.close();
  }
}

const scenarios = ["stock", "shared"].flatMap(host => [
  { id: `real-${host}-durable-steer`, family: "real-runtime", backend: "real", run: () => durableSteer(host) },
  { id: `real-${host}-persisted-background-job`, family: "real-runtime", backend: "real", run: () => durableSteer(host, true) },
]);
module.exports = { scenarios, assertPeerSetupHistory };
if (require.main === module) require("../scenario-registry.cjs").runScenarios(scenarios).catch(error => { process.stderr.write(`${error.stack || error}\n`); process.exitCode = 1; });
