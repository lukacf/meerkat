#!/usr/bin/env node

const assert = require("node:assert/strict");
const crypto = require("node:crypto");
const fs = require("node:fs/promises");
const http = require("node:http");
const path = require("node:path");
const { chromium } = require("playwright");

const agents = ["Alpha", "Beta"].map((label) => ({
  identity: `identity:${label.toLowerCase()}`,
  member_id: `identity:${label.toLowerCase()}`,
  agent_id: `identity:${label.toLowerCase()}`,
  session_id: `session-${label.toLowerCase()}`,
  label,
  kind: "identity",
  state: "active",
  addressable: true,
  affordances: { addressable: true, can_send_message: true },
}));

function pendingHandle(channelId, identity) {
  return {
    channel_id: channelId,
    target_identity: identity,
    execution_mode: "client_context",
    pending_receipt: `pending-${channelId}`,
    transport: { transport: "webrtc", token: `token-${channelId}`, answer_method: "live/webrtc/answer" },
    capabilities: {
      audio_in: true, audio_out: true, text_in: false, text_out: false,
      image_in: false, video_in: false, transcript_supported: true,
      barge_in_supported: true, provider_native_resume: false,
    },
    continuity: { mode: "transcript_only" },
  };
}

async function startServer() {
  const timelineStreams = new Set();
  let noteTimelineConnected;
  const timelineConnected = new Promise(resolve => { noteTimelineConnected = resolve; });
  const files = new Map(await Promise.all([
    ["/console", "index.html", "text/html"],
    ["/console/assets/console-app.js", "console-app.js", "application/javascript"],
    ["/console/assets/console-app.css", "console-app.css", "text/css"],
  ].map(async ([url, file, type]) => [url, { bytes: await fs.readFile(path.join(__dirname, "dist", file)), type }])));
  const server = http.createServer((req, res) => {
    const url = new URL(req.url, "http://localhost");
    if (url.pathname === "/console/timeline/stream") {
      res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
      res.write(": connected\n\n");
      res.write('id: console:0\nevent: snapshot_complete\ndata: {"type":"snapshot_complete","cursor":"console:0"}\n\n');
      timelineStreams.add(res);
      res.on("close", () => timelineStreams.delete(res));
      noteTimelineConnected();
      return;
    }
    const file = files.get(url.pathname);
    if (file) {
      res.writeHead(200, { "content-type": file.type });
      res.end(file.bytes);
    } else {
      res.writeHead(404);
      res.end("Not found");
    }
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", resolve);
  });
  return {
    server, url: `http://127.0.0.1:${server.address().port}`,
    async publishTimeline(frames) {
      await timelineConnected;
      assert(timelineStreams.size > 0, "canonical rows need an active HTTP timeline stream");
      for (const frame of frames) {
        for (const stream of timelineStreams) {
          stream.write(`id: ${frame.cursor}\nevent: ${frame.kind}\ndata: ${JSON.stringify({ type: "console_frame", frame })}\n\n`);
        }
      }
    },
  };
}

async function installConsoleFixture(page, { available = true, publishTimeline = () => {} } = {}) {
  const requests = [];
  let timeline = [];
  let sequence = 0;
  const channels = new Map();
  const replacements = new Map();
  const preparing = new Set();
  const cancelled = new Set();
  // Provisional assistant captions per channel, coalesced per item exactly
  // like the gateway: the newest caption or retraction of an item replaces
  // its entry and moves it to the end of the cursor order.
  let captionSequence = 0;
  const captionFeeds = new Map();
  const captionFeed = (channelId) => {
    if (!captionFeeds.has(channelId)) captionFeeds.set(channelId, { items: new Map(), wake: new Set() });
    return captionFeeds.get(channelId);
  };
  const captionBatch = (channelId, after) => {
    const entries = [...captionFeed(channelId).items.values()].filter(entry => entry.sequence > after)
      .sort((a, b) => a.sequence - b.sequence);
    return { cursor: entries.reduce((cursor, entry) => Math.max(cursor, entry.sequence), after),
      captions: entries.map(entry => entry.caption) };
  };
  const timelinePage = (identity) => {
    const frames = identity ? timeline.filter(frame => frame.identity === identity) : timeline;
    return { frames, available: true, exhausted: true, next_cursor: frames.at(-1)?.cursor ?? null,
      latest_cursor: timeline.at(-1)?.cursor ?? null };
  };
  await page.route("**/console/experience", (route) => route.fulfill({ json: {
    contract_version: "0.5.0",
    runtime_id: "voice-browser-fixture",
    voice: { available: false, readiness_method: "meta/console/voice/readiness" },
    runtime_capabilities: { can_send_messages: true },
    console_config: {
      title: "Voice fixture",
      appearance: { default_theme: "dark", default_variant: "graphite" },
      layout: { initial_agent: "identity:alpha", initial_preset: "single", sidebar_collapsed: false },
      rail: { visible: false },
    },
    agent_sidebar: { live_snapshot: { agents } },
    identity_status: { rows: agents.map((agent) => ({ ...agent, display_name: agent.label, addressability: "addressable" })) },
  } }));
  await page.route("**/console/modules", (route) => route.fulfill({ json: { modules: [] } }));
  await page.route("**/console/identities", (route) => route.fulfill({ json: { rows: agents } }));
  await page.route("**/console/timeline?*", route => route.fulfill({
    json: timelinePage(new URL(route.request().url()).searchParams.get("identity")),
  }));
  await page.route("**/console/rpc", async (route) => {
    const { id, method, params = {} } = route.request().postDataJSON();
    requests.push({ method, params });
    const respond = (result) => route.fulfill({ json: { jsonrpc: "2.0", id, result } });
    if (method === "meta/capabilities") {
      return respond({ methods: ["meta/console/send"], feature_capabilities: available ? ["live.execution_identity.v1", "live.execution.client_context.v1"] : [] });
    }
    if (method === "meta/console/voice/readiness") {
      return respond({ identity: params.identity, available: available && agents.some((agent) => agent.identity === params.identity) });
    }
    if (method === "meta/console/voice/replacement") {
      const channel = [...channels.values()].find((channel) => channel.requestId === params.request_id && channel.identity === params.identity && !channel.closed);
      if (cancelled.has(params.request_id) || (!channel && !preparing.has(params.request_id))) {
        return route.fulfill({ json: { jsonrpc: "2.0", id, error: { code: -32000, message: "Voice closed", data: { kind: "voice_closed" } } } });
      }
      return respond(replacements.get(params.request_id) ?? { required: false });
    }
    if (method === "meta/console/voice/context_status") {
      const channel = channels.get(params.channel_id);
      assert.ok(channel && !channel.closed && channel.identity === params.identity &&
        channel.requestId === params.request_id && !cancelled.has(params.request_id),
      "context status must address the current owned channel");
      return respond({ ...params, context_preparation: channel.contextPreparation });
    }
    if (method === "meta/console/voice/captions") {
      const channel = channels.get(params.channel_id);
      assert.ok(channel && channel.identity === params.identity && channel.requestId === params.request_id,
        "captions must address an owned channel");
      assert.deepEqual(Object.keys(params).sort(), ["after", "channel_id", "identity", "request_id", "wait_ms"]);
      if (channel.closed || cancelled.has(params.request_id)) {
        return route.fulfill({ json: { jsonrpc: "2.0", id, error: { code: -32000, message: "Voice closed", data: { kind: "voice_closed" } } } });
      }
      let batch = captionBatch(params.channel_id, params.after);
      if (batch.captions.length === 0) {
        const feed = captionFeed(params.channel_id);
        await new Promise(resolve => {
          const timer = setTimeout(done, Math.min(params.wait_ms, 1000));
          function done() { clearTimeout(timer); feed.wake.delete(done); resolve(); }
          feed.wake.add(done);
        });
        batch = captionBatch(params.channel_id, params.after);
      }
      return respond({ identity: params.identity, request_id: params.request_id, channel_id: params.channel_id, ...batch })
        .catch(() => {});
    }
    if (method === "meta/console/voice/answer_received" || method === "meta/console/voice/activity") {
      const channel = [...channels.entries()].find(([channelId, candidate]) =>
        candidate.identity === params.identity && candidate.requestId === params.request_id &&
        !cancelled.has(params.request_id) &&
        (method.endsWith("/activity") || (!candidate.closed && channelId === params.channel_id)));
      assert.ok(channel, `${method} must address the current request`);
      if (method.endsWith("/answer_received")) channel[1].answerAccepted = true;
      else assert.equal(channel[1].answerAccepted, true, "activity must follow activation");
      return respond({ accepted: true });
    }
    if (method === "meta/console/voice/open") {
      assert.equal(available, true, "voice must not open without OpenAI auth");
      assert.equal(typeof params.request_id, "string");
      const channelId = `voice-${++sequence}`;
      channels.set(channelId, {
        identity: params.identity, requestId: params.request_id, closed: false, answerAccepted: false,
        contextPreparation: { phase: "preparing", stage: "generating" },
      });
      return respond(pendingHandle(channelId, params.identity));
    }
    if (method === "meta/live/playback_owner/register") {
      return respond({ channel_id: params.channel_id, readiness_receipt: `ready-${params.channel_id}` });
    }
    if (method === "live/webrtc/answer") {
      const answerSdp = await page.evaluate(async ({ offer, channelId }) => {
        const peer = new RTCPeerConnection();
        const context = new AudioContext();
        await context.resume();
        const oscillator = context.createOscillator();
        oscillator.frequency.value = 180;
        const volume = context.createGain();
        volume.gain.value = 0.09;
        const output = context.createMediaStreamDestination();
        oscillator.connect(volume).connect(output);
        oscillator.start();
        for (const track of output.stream.getTracks()) peer.addTrack(track, output.stream);
        peer.addEventListener("datachannel", (event) => {
          window.voiceFixture.channels.push(event.channel);
        });
        window.voiceFixture.providers.set(channelId, { peer, context, oscillator });
        await peer.setRemoteDescription({ type: "offer", sdp: offer });
        await peer.setLocalDescription(await peer.createAnswer());
        if (peer.iceGatheringState !== "complete") {
          await new Promise((resolve, reject) => {
            const timeout = setTimeout(() => reject(new Error("Fixture ICE gathering timed out")), 5000);
            peer.addEventListener("icegatheringstatechange", () => {
              if (peer.iceGatheringState === "complete") {
                clearTimeout(timeout);
                resolve();
              }
            });
          });
        }
        return peer.localDescription.sdp;
      }, { offer: params.offer_sdp, channelId: params.channel_id });
      for (const [requestId, replacement] of replacements) {
        if (replacement.replacement.channel_id === params.channel_id) replacements.delete(requestId);
      }
      return respond({ answer_sdp: answerSdp });
    }
    if (method === "meta/live/status") {
      const channel = channels.get(params.channel_id);
      assert.ok(channel, "status must address an opened channel");
      if (!channel.closed && !channel.answerAccepted) return respond({ phase: "pending" });
      return respond(channel.closed ? { phase: "closed" } : {
        phase: "active",
        handle: {
          channel_id: params.channel_id, target_identity: channel.identity,
          execution_mode: "client_context", activation_receipt: `active-${params.channel_id}`,
        },
      });
    }
    if (method === "meta/console/voice/close") {
      const owned = [...channels.values()].filter((channel) => channel.requestId === params.request_id && channel.identity === params.identity);
      assert.ok(owned.length, "close must address an opened request");
      owned.forEach((channel) => { channel.closed = true; });
      replacements.delete(params.request_id);
      preparing.delete(params.request_id);
      cancelled.add(params.request_id);
      return respond({ phase: "closed" });
    }
    if (method === "meta/live/replacement_required") return respond({ required: false });
    if (method === "meta/live/playback_complete") return respond({ status: "completed" });
    if (method === "meta/console/send") {
      return respond({ interaction_id: `text-${++sequence}`, identity: params.identity, accepted: true });
    }
    if (method === "meta/console/query_timeline") return respond(timelinePage(params.identity));
    return route.fulfill({ json: { jsonrpc: "2.0", id, error: { code: -32601, message: `Fixture does not implement ${method}` } } });
  });
  await page.addInitScript(() => {
    window.voiceFixture = { microphoneTracks: [], providers: new Map(), channels: [] };
    const getUserMedia = navigator.mediaDevices.getUserMedia.bind(navigator.mediaDevices);
    navigator.mediaDevices.getUserMedia = async (constraints) => {
      const stream = await getUserMedia(constraints);
      window.voiceFixture.microphoneTracks.push(...stream.getTracks());
      return stream;
    };
  });
  return {
    requests,
    channels,
    async setTimeline(frames) { timeline = frames; await publishTimeline(frames); },
    caption(channelId, captions) {
      const feed = captionFeed(channelId);
      for (const caption of captions) {
        feed.items.set(caption.item_id, { sequence: ++captionSequence, caption });
      }
      for (const wake of [...feed.wake]) wake();
    },
    setAvailable(value) { available = value; },
    setContext(identity, preparation) {
      const channel = [...channels.values()].find((candidate) => candidate.identity === identity && !candidate.closed);
      assert.ok(channel, "context transition requires an active fixture channel");
      channel.contextPreparation = preparation;
    },
    prepareReplacement(identity) {
      const current = [...channels.entries()].find(([, channel]) => channel.identity === identity && !channel.closed);
      assert.ok(current);
      const [previousId, previous] = current;
      previous.closed = true;
      preparing.add(previous.requestId);
      return {
        previousId,
        publish() {
          assert.equal(cancelled.has(previous.requestId), false, "recovery must not cancel a request while its replacement is preparing");
          const channelId = `voice-${++sequence}`;
          channels.set(channelId, {
            identity, requestId: previous.requestId, closed: false, answerAccepted: false,
            contextPreparation: { phase: "preparing", stage: "generating" },
          });
          replacements.set(previous.requestId, {
            required: true,
            reason: "canonical_context",
            replacement: pendingHandle(channelId, identity),
            canonical_seed_cursor: 12,
          });
          preparing.delete(previous.requestId);
        },
      };
    },
  };
}

async function openAgent(page, label) {
  await page.locator('.agent[role="button"]').filter({ hasText: label }).first().click();
  await page.getByTestId(`chat-composer:identity:${label.toLowerCase()}`).waitFor();
}

async function sendText(page, identity, text) {
  await page.getByTestId(`chat-composer:${identity}`).fill(text);
  const sent = page.waitForResponse((response) => response.url().endsWith("/console/rpc") &&
    response.request().postDataJSON()?.method === "meta/console/send");
  await page.getByTestId(`chat-send:${identity}`).click();
  await sent;
}

async function waitForVoice(page, requests) {
  try {
    await page.locator('[data-testid="voice-bar"][data-phase="active"]').waitFor({ timeout: 20000 });
  } catch (error) {
    throw new Error(`Voice did not activate: ${await page.getByTestId("voice-bar").textContent()}\nRPCs: ${requests.map((request) => request.method).join(", ")}`, { cause: error });
  }
}

async function assertVoiceLayout(page) {
  const layout = await page.getByTestId("voice-bar").evaluate((bar) => {
    const bounds = bar.getBoundingClientRect();
    const controls = bar.querySelector(".voice-bar__controls").getBoundingClientRect();
    return {
      width: bounds.width,
      right: bounds.right,
      viewport: window.innerWidth,
      overflowingControls: [...bar.querySelectorAll("button")].filter((button) => {
        const rect = button.getBoundingClientRect();
        return rect.left < bounds.left || rect.right > bounds.right;
      }).map((button) => button.getAttribute("aria-label")),
      overflowingText: [...bar.querySelectorAll(".voice-bar__status, .voice-bar__message")].filter((element) => {
        const rect = element.getBoundingClientRect();
        return rect.left < bounds.left || rect.right > bounds.right || element.scrollWidth > element.clientWidth + 1;
      }).map((element) => element.textContent),
      overlappingHeaderText: [...bar.querySelectorAll(".voice-bar__name, .voice-bar__status > span")].filter((element) => {
        const rect = element.getBoundingClientRect();
        return rect.right > controls.left && rect.left < controls.right &&
          rect.bottom > controls.top && rect.top < controls.bottom;
      }).map((element) => element.textContent),
    };
  });
  assert.ok(layout.width > 0 && layout.right <= layout.viewport, JSON.stringify(layout));
  assert.deepEqual(layout.overflowingControls, [], JSON.stringify(layout));
  assert.deepEqual(layout.overflowingText, [], JSON.stringify(layout));
  assert.deepEqual(layout.overlappingHeaderText, [], JSON.stringify(layout));
}

async function verifyVoiceTranscriptHandoff(page, fixture) {
  const identity = "identity:alpha";
  const channelId = [...fixture.channels.entries()].find(([, channel]) => channel.identity === identity && !channel.closed)?.[0];
  assert(channelId, "an active fixture channel is required");
  const source = "## Spoken review\n\nKeep A\u030a, å and 🚀 exactly.\n\n| Check | Result |\n| --- | --- |\n| WorkGraph | Ready |";
  const now = Date.now();
  const row = (id, role, text, realtimeOrigin) => ({
    id, cursor: `console:${id}`, kind: role === "user" ? "user_input" : "text_complete",
    identity, runtime_key: "voice-browser-fixture", session_id: "session-alpha", timestamp_ms: now + id,
    source: { kind: "session_history", source_cursor: `session-alpha:${id}` }, status: "completed",
    payload: {
      text, result: text, content: text,
      message: role === "user" ? { role, content: text } : {
        role: "block_assistant", blocks: [{ block_type: "transcript", data: { text, source: "spoken" } }],
        ...(realtimeOrigin ? { identity: { realtime_origin: realtimeOrigin } } : {}),
      },
    },
  });
  const origin = { session_id: "session-alpha", channel_id: channelId, canonical_row_sequence: 3,
    provider_item_ids: ["spoken-first", "spoken-second"] };
  await page.waitForFunction(() => window.voiceFixture.channels.at(-1)?.readyState === "open");
  const speak = events => page.evaluate(events => {
    const channel = window.voiceFixture.channels.at(-1);
    for (const event of events) channel.send(JSON.stringify(event));
  }, events);
  // User speech comes from item-keyed input transcripts on the data channel;
  // assistant speech only from the gateway's provisional captions, keyed by
  // the segment item id the committed row later lists.
  await speak([
    { type: "session.input_transcript.delta", item_id: "spoken-first", delta: "Keep my spoken instruction separate." },
  ]);
  fixture.caption(channelId, [
    { kind: "caption", item_id: "spoken-first", text: "The first" },
    { kind: "caption", item_id: "spoken-first", text: "The first provisional fragment" },
    { kind: "caption", item_id: "spoken-second", text: "The second provisional fragment" },
    { kind: "caption", item_id: "still-speaking", text: "Checking one more prerequisite..." },
  ]);
  const live = (item, speaker = "assistant") => page.locator(`.msg--live-${speaker}[data-testid="chat-live-row:${identity}:${item}"]`);
  await live("spoken-first").waitFor();
  await live("spoken-first", "user").waitFor();
  await live("spoken-second").waitFor();
  assert.equal(await live("spoken-first").locator(".msg__text").textContent(), "The first provisional fragment",
    "a caption replaces its item's text with the segment's whole text");
  // Data-channel output transcripts name no committed item and never render.
  await speak([
    { type: "session.output_transcript.delta", item_id: "data-channel-only", delta: "Unkeyed provider transcript" },
    { type: "session.input_transcript.delta", item_id: "after-data-channel", delta: "Ordered after the output delta." },
  ]);
  await live("after-data-channel", "user").waitFor();
  assert.equal(await live("data-channel-only").count(), 0, "output transcript deltas are not assistant captions");
  const composer = page.getByTestId(`chat-composer:${identity}`);
  const draft = { text: "Keep this draft while the voice result arrives.", start: 5, end: 15 };
  const assertDraft = async () => assert.deepEqual(
    await composer.evaluate(node => ({ text: node.value, start: node.selectionStart, end: node.selectionEnd })), draft,
  );
  await composer.fill(draft.text);
  await composer.evaluate((node, draft) => node.setSelectionRange(draft.start, draft.end), draft);
  // Canonical history arrives over the actual HTTP event stream while the
  // loopback WebRTC call and provider item buffers remain active.
  await fixture.setTimeline([
    row(1, "user", "Check the release graph while we talk."),
    row(2, "assistant", "The typed WorkGraph check is ready."),
    row(3, "assistant", source, origin),
  ]);
  await page.getByRole("heading", { name: "Spoken review", exact: true }).waitFor();
  await live("spoken-first").waitFor({ state: "detached" });
  await live("spoken-second").waitFor({ state: "detached" });
  await live("still-speaking").waitFor();
  assert.equal(await live("spoken-first", "user").locator(".msg__text").textContent(), "Keep my spoken instruction separate.",
    "an assistant's canonical provider item cannot consume a user's item with the same ID");
  await assertDraft();
  const pane = page.getByTestId(`chat-pane:${identity}`);
  const messageSources = pane.locator("[data-quote-source]");
  assert(await messageSources.getByText("Check the release graph while we talk.", { exact: true }).isVisible());
  assert(await messageSources.getByText("The typed WorkGraph check is ready.", { exact: true }).isVisible());
  const canonical = pane.locator('[data-quote-source]').filter({ has: page.getByRole("heading", { name: "Spoken review", exact: true }) });
  assert.equal(await canonical.getAttribute("data-quote-source"), source);
  assert.equal(await canonical.locator("table").count(), 1);
  fixture.caption(channelId, [
    { kind: "caption", item_id: "spoken-first", text: "The first provisional fragment Late stale speech must stay hidden." },
    { kind: "caption", item_id: "after-late", text: "The caption feed delivered the late fragment." },
  ]);
  await live("after-late").waitFor();
  assert.equal(await live("spoken-first").count(), 0, "a late caption cannot replace its canonical row");
  // A retracted segment will never commit; its caption disappears.
  fixture.caption(channelId, [{ kind: "retracted", item_id: "still-speaking" }]);
  await live("still-speaking").waitFor({ state: "detached" });
  await live("after-late").waitFor();
  await assertDraft();
  assert.equal(await pane.getByRole("heading", { name: "Spoken review", exact: true }).count(), 1);
  if (process.env.VOICE_SCREENSHOT_DIR) {
    await page.setViewportSize({ width: 1600, height: 1000 });
    await page.screenshot({ path: path.join(process.env.VOICE_SCREENSHOT_DIR, "voice-canonical-handoff-1600.png") });
    await page.setViewportSize({ width: 1440, height: 900 });
    await page.screenshot({ path: path.join(process.env.VOICE_SCREENSHOT_DIR, "voice-canonical-handoff-1440.png") });
  }
  await composer.fill("");
}

async function main() {
  const { server, url, publishTimeline } = await startServer();
  let browser;
  try {
    browser = await chromium.launch({
      headless: true,
      args: ["--use-fake-device-for-media-stream", "--use-fake-ui-for-media-stream"],
    });
    const context = await browser.newContext({ viewport: { width: 1280, height: 900 }, permissions: ["microphone"] });
    const page = await context.newPage();
    if (process.env.VOICE_DEBUG) {
      const debuggerSession = await context.newCDPSession(page);
      await debuggerSession.send("Debugger.enable");
      await debuggerSession.send("Debugger.setPauseOnExceptions", { state: "all" });
      debuggerSession.on("Debugger.paused", async (event) => {
        console.error("Browser exception:", event.data?.description);
        await debuggerSession.send("Debugger.resume");
      });
    }
    const errors = [];
    page.on("pageerror", (error) => errors.push(error.message));
    const assetResponses = [];
    page.on("response", response => {
      const pathname = new URL(response.url()).pathname;
      if (!["/console/assets/console-app.js", "/console/assets/console-app.css"].includes(pathname)) return;
      assetResponses.push(response.body().then(bytes => ({
        url: response.url(), status: response.status(), bytes: bytes.length,
        sha256: crypto.createHash("sha256").update(bytes).digest("hex"),
      })));
    });
    const fixture = await installConsoleFixture(page, { publishTimeline });
    await page.goto(`${url}/console`);
    await openAgent(page, "Alpha");
    if (process.env.VOICE_SCREENSHOT_DIR) {
      await fs.mkdir(process.env.VOICE_SCREENSHOT_DIR, { recursive: true });
      await page.setViewportSize({ width: 1600, height: 1000 });
      await page.screenshot({ path: path.join(process.env.VOICE_SCREENSHOT_DIR, "voice-idle-desktop.png") });
    }
    await page.getByRole("button", { name: "Start voice with Alpha" }).click();
    await waitForVoice(page, fixture.requests);
    await page.getByRole("region", { name: "Voice with Alpha" }).waitFor();
    // Context is still being prepared, yet the primary status already says the user can talk.
    await page.locator('.voice-bar__context[data-context="preparing"][data-context-stage="generating"]', { hasText: "Context arriving" }).waitFor();
    await page.locator('[data-testid="voice-status"][data-talk-ready="true"]', { hasText: /you can talk/i }).waitFor();
    assert.equal(await page.locator(".voice-waveform").count(), 2);
    await sendText(page, "identity:alpha", "Keep working while we talk");
    await page.locator('[data-testid="voice-bar"][data-phase="active"]').waitFor();
    assert.ok(fixture.requests.some((request) => request.method === "meta/console/send" && request.params.identity === "identity:alpha"));
    await verifyVoiceTranscriptHandoff(page, fixture);
    await page.getByRole("button", { name: "Mute microphone" }).click();
    assert.equal(await page.evaluate(() => window.voiceFixture.microphoneTracks.at(-1).enabled), false);
    await page.getByRole("button", { name: "Mute speakers" }).click();
    await page.getByRole("button", { name: "Unmute speakers" }).waitFor();
    await page.waitForFunction(() => {
      const canvas = document.querySelector(".voice-waveform--speaker");
      const context = canvas.getContext("2d");
      const pixels = context.getImageData(0, 0, canvas.width, canvas.height).data;
      // A muted speaker must still display real incoming audio, not a flat idle line.
      for (let y = 0; y < canvas.height / 2 - 4; y += 1) {
        for (let x = 0; x < canvas.width; x += 1) {
          if (pixels[(y * canvas.width + x) * 4 + 3] > 100) return true;
        }
      }
      return false;
    });
    assert.equal(await page.locator('.voice-bar__context[data-context-stage="generating"]', { hasText: "Context arriving" }).count(), 1,
      "native audio, text and mute controls must work before the context gate is released");
    fixture.setContext("identity:alpha", { phase: "provider_acknowledged" });
    await page.getByText("Context supplied", { exact: true }).waitFor();
    const capturesBeforeRecovery = await page.evaluate(() => window.voiceFixture.microphoneTracks.length);
    const replacement = fixture.prepareReplacement("identity:alpha");
    await page.evaluate(async (channelId) => {
      const provider = window.voiceFixture.providers.get(channelId);
      provider.peer.close();
      provider.oscillator.stop();
      await provider.context.close();
    }, replacement.previousId);
    await page.locator('[data-testid="voice-bar"][data-phase="connecting"]').waitFor();
    for (let attempt = 0; attempt < 2; attempt += 1) {
      const preparingResponse = await page.waitForResponse((response) =>
        response.url().endsWith("/console/rpc") &&
        response.request().postDataJSON()?.method === "meta/console/voice/replacement");
      assert.deepEqual((await preparingResponse.json()).result, { required: false });
    }
    replacement.publish();
    await page.waitForFunction(() => window.voiceFixture.providers.size === 2);
    await waitForVoice(page, fixture.requests);
    await page.getByRole("region", { name: "Voice with Alpha" }).waitFor();
    await page.getByRole("button", { name: "Unmute microphone" }).waitFor();
    await page.getByRole("button", { name: "Unmute speakers" }).waitFor();
    assert.equal(await page.evaluate(() => window.voiceFixture.microphoneTracks.length), capturesBeforeRecovery);
    assert.equal(fixture.requests.filter((request) => request.method === "meta/console/voice/open").length, 1);
    assert.equal(await page.getByRole("heading", { name: "Spoken review", exact: true }).count(), 1,
      "saved speech remains visible after the channel replacement clears provisional items");

    await openAgent(page, "Beta");
    await page.getByRole("region", { name: "Voice with Alpha" }).waitFor();
    assert.equal(fixture.requests.filter((request) => request.method === "meta/console/voice/open").length, 1);
    await page.getByTestId("nav:roster").click();
    await page.locator('.main > [data-testid="voice-bar"]').waitFor();
    await page.getByRole("region", { name: "Voice with Alpha" }).waitFor();
    await openAgent(page, "Beta");
    await sendText(page, "identity:beta", "A separate text task");
    await page.waitForFunction(() => document.querySelector('[data-testid="chat-composer:identity:beta"]').value === "");
    assert.ok(fixture.requests.some((request) => request.method === "meta/console/send" && request.params.identity === "identity:beta"));
    assert.equal(await page.getByRole("heading", { name: "Spoken review", exact: true }).count(), 0,
      "the separate agent has no canonical speech from Alpha's history");

    await assertVoiceLayout(page);
    if (process.env.VOICE_SCREENSHOT_DIR) {
      await fs.mkdir(process.env.VOICE_SCREENSHOT_DIR, { recursive: true });
      await page.screenshot({ path: path.join(process.env.VOICE_SCREENSHOT_DIR, "voice-desktop.png") });
    }
    await page.setViewportSize({ width: 390, height: 844 });
    await assertVoiceLayout(page);
    if (process.env.VOICE_SCREENSHOT_DIR) {
      await page.screenshot({ path: path.join(process.env.VOICE_SCREENSHOT_DIR, "voice-mobile.png") });
    }
    await page.setViewportSize({ width: 1280, height: 900 });

    fixture.setContext("identity:alpha", { phase: "failed", reason: "timed_out" });
    await page.getByRole("alert").filter({ hasText: "Context preparation timed out." }).waitFor();
    await waitForVoice(page, fixture.requests);
    assert.equal(await page.getByRole("button", { name: "Unmute microphone" }).isEnabled(), true);
    for (const [name, viewport] of [
      ["desktop", { width: 1280, height: 900 }],
      ["mobile", { width: 390, height: 844 }],
    ]) {
      await page.setViewportSize(viewport);
      await assertVoiceLayout(page);
      if (process.env.VOICE_SCREENSHOT_DIR) {
        await page.screenshot({ path: path.join(process.env.VOICE_SCREENSHOT_DIR, `voice-context-error-${name}.png`) });
      }
    }
    await page.setViewportSize({ width: 1280, height: 900 });

    await page.getByRole("button", { name: "Start voice with Beta" }).click();
    await page.getByRole("region", { name: "Voice with Beta" }).waitFor();
    await waitForVoice(page, fixture.requests);
    const firstClose = fixture.requests.findIndex((request) => request.method === "meta/console/voice/close");
    const secondOpen = fixture.requests.findIndex((request) => request.method === "meta/console/voice/open" && request.params.identity === "identity:beta");
    assert.ok(firstClose >= 0 && firstClose < secondOpen, "old voice closes before the new voice opens");
    await page.getByRole("button", { name: "End voice conversation" }).click();
    await page.waitForFunction(() => window.voiceFixture.microphoneTracks.every((track) => track.readyState === "ended"));
    assert.ok([...fixture.channels.values()].every((channel) => channel.closed));
    assert.deepEqual(errors, []);
    const openedBeforeAuthLoss = fixture.channels.size;
    const capturesBeforeAuthLoss = await page.evaluate(() => window.voiceFixture.microphoneTracks.length);
    await page.getByRole("button", { name: "Start voice with Beta" }).waitFor();
    fixture.setAvailable(false);
    await page.getByRole("button", { name: "Start voice with Beta" }).click();
    await page.getByRole("alert").filter({ hasText: "authenticate OpenAI" }).waitFor();
    // The microphone is requested with the click, concurrently with readiness; a
    // negative readiness releases that capture and opens no channel.
    // The browser may grant the pending capture after the fast readiness error.
    await page.waitForFunction((previous) => window.voiceFixture.microphoneTracks.length === previous + 1, capturesBeforeAuthLoss);
    assert.equal(await page.evaluate(() => window.voiceFixture.microphoneTracks.length), capturesBeforeAuthLoss + 1);
    await page.waitForFunction(() => window.voiceFixture.microphoneTracks.every((track) => track.readyState === "ended"));
    assert.equal(fixture.channels.size, openedBeforeAuthLoss);
    assert.ok(!fixture.requests.slice(secondOpen + 1).some((request) => request.method === "meta/console/voice/open"),
      "no open follows a negative readiness");

    const unauthenticated = await context.newPage();
    const noAuth = await installConsoleFixture(unauthenticated, { available: false });
    // The button shows a checking state until the first readiness answer; the definite
    // negative answer for the opened agent then removes it.
    const alphaReadinessAnswered = unauthenticated.waitForResponse((response) => {
      const body = response.request().postDataJSON?.();
      return body?.method === "meta/console/voice/readiness" && body?.params?.identity === "identity:alpha";
    });
    await unauthenticated.goto(`${url}/console`);
    await openAgent(unauthenticated, "Alpha");
    await alphaReadinessAnswered;
    await unauthenticated.getByTestId("voice-start").waitFor({ state: "detached" });
    assert.equal(await unauthenticated.getByTestId("voice-start").count(), 0);
    assert.equal(await unauthenticated.evaluate(() => window.voiceFixture.microphoneTracks.length), 0);
    assert.ok(!noAuth.requests.some((request) => request.method === "meta/console/voice/open"));
    const assets = await Promise.all(assetResponses);
    assert.equal(assets.length, 2, "the browser fetched both production console assets");
    assert(assets.every(asset => asset.status === 200));
    if (process.env.VOICE_SCREENSHOT_DIR) {
      await fs.writeFile(path.join(process.env.VOICE_SCREENSHOT_DIR, "voice-assets.json"), JSON.stringify(assets, null, 2) + "\n");
    }
    await context.close();
    console.log("Voice browser E2E passed: real WebRTC audio before context release, context acknowledgement/failure, mute, navigation, concurrent text, provisional captions, canonical transcript handoff, replacement, cleanup and auth gate.");
  } finally {
    await browser?.close();
    server.closeAllConnections();
    await new Promise((resolve) => server.close(resolve));
  }
}

module.exports = { verifyVoiceTranscriptHandoff };
if (require.main === module) {
  main().catch((error) => {
    console.error(error);
    process.exitCode = 1;
  });
}
