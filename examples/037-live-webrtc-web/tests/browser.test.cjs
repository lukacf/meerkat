const { test, before, after } = require("node:test");
const assert = require("node:assert/strict");
const { createServer } = require("node:http");
const { readFile } = require("node:fs/promises");
const { join } = require("node:path");
const { chromium } = require("@playwright/test");
const { pageSmoke } = require("../.smoke/smoke.cjs");

let browser, server, url;
before(async () => {
  server = createServer(async (req, res) => {
    try {
      const name = req.url === "/" ? "index.html" : req.url.slice(1);
      res.setHeader("content-type", name.endsWith(".js") ? "text/javascript" : name.endsWith(".css") ? "text/css" : "text/html");
      res.end(await readFile(join(__dirname, "../public", name)));
    } catch { res.writeHead(404).end(); }
  });
  await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
  url = `http://127.0.0.1:${server.address().port}/`;
  // Playwright's bundled headless Chromium by default; set
  // PLAYWRIGHT_CHROMIUM_CHANNEL=chrome to run against an installed Google Chrome.
  browser = await chromium.launch({
    channel: process.env.PLAYWRIGHT_CHROMIUM_CHANNEL || undefined, headless: true,
    args: ["--use-fake-device-for-media-stream", "--use-fake-ui-for-media-stream"],
  });
});
after(async () => {
  await browser?.close();
  if (server) await new Promise((resolve) => server.close(resolve));
});

async function setup(config = {}) {
  const page = await browser.newPage();
  await page.clock.install();
  await page.addInitScript((config) => {
    const h = window.harness = {
      config, peers: [], tracks: [], contexts: [], closes: [], opens: 0,
      intervals: new Set(), errors: [],
    };
    window.addEventListener("unhandledrejection", (event) => h.errors.push(String(event.reason)));
    const interval = window.setInterval.bind(window), clear = window.clearInterval.bind(window);
    window.setInterval = (...args) => { const id = interval(...args); h.intervals.add(id); return id; };
    window.clearInterval = (id) => { h.intervals.delete(id); clear(id); };
    class Events extends EventTarget {
      emit(name) { this.dispatchEvent(new Event(name)); this[`on${name}`]?.({}); }
    }
    class Channel extends Events {
      readyState = "connecting";
      closes = 0;
      close() { this.closes++; this.readyState = "closed"; this.emit("close"); }
      send() {}
    }
    window.RTCPeerConnection = class extends Events {
      connectionState = "new";
      iceConnectionState = "new";
      signalingState = "stable";
      iceGatheringState = h.config.gathering ? "gathering" : "complete";
      closes = 0;
      constructor() { super(); h.peers.push(this); }
      createDataChannel() { return this.dc = new Channel(); }
      addTrack() {}
      async createOffer() { return { type: "offer", sdp: "synthetic-offer" }; }
      async setLocalDescription(offer) { this.localDescription = offer; }
      async setRemoteDescription() {
        if (h.config.remoteFailure) throw new Error("remote description failed");
        this.connectionState = this.iceConnectionState = "connected";
        this.emit("connectionstatechange"); this.emit("iceconnectionstatechange");
        this.dc.readyState = "open"; this.dc.emit("open");
      }
      close() {
        this.closes++; this.connectionState = this.signalingState = "closed";
        this.emit("connectionstatechange"); this.emit("signalingstatechange");
      }
    };
    const stream = () => {
      const track = { kind: "audio", readyState: "live", stops: 0, stop() { this.stops++; this.readyState = "ended"; } };
      h.tracks.push(track);
      return { getTracks: () => [track], getAudioTracks: () => [track] };
    };
    navigator.mediaDevices.getUserMedia = async () => {
      if (h.config.micDenied) throw new Error("microphone denied");
      if (h.config.micPending) return new Promise((resolve) => { h.resolveMic = () => resolve(stream()); });
      return stream();
    };
    window.AudioContext = class {
      closes = 0;
      constructor() { h.contexts.push(this); }
      createAnalyser() { return { frequencyBinCount: 1, getByteFrequencyData() {} }; }
      createMediaStreamSource() { return { connect() {} }; }
      async close() { this.closes++; }
    };
    const response = (body, ok = true) => ({ ok, status: ok ? 200 : 500, json: async () => body });
    window.fetch = async (url, options = {}) => {
      if (url === "/api/state") return response({});
      if (url === "/api/start") {
        const id = ++h.opens;
        const open = { channel_id: `channel-${id}`, session_id: `session-${id}`, transport: { token: "synthetic" }, tools: [] };
        if (h.config.openFailure) throw new Error("open refused");
        if (h.config.openPending) return new Promise((resolve) => { h.resolveOpen = () => resolve(response(open)); });
        return response(open);
      }
      if (url === "/api/webrtc/answer") {
        if (h.config.answerFailure) return response({ error: "answer refused" }, false);
        if (h.config.answerPending) return new Promise((_, reject) => options.signal.addEventListener("abort", () => reject(options.signal.reason), { once: true }));
        return response({ answer_sdp: "synthetic-answer" });
      }
      if (url.endsWith("/close")) { h.closes.push(url); return response({}); }
      return response({});
    };
    h.observe = (obs) => h.peers.at(-1).dc.onmessage?.({ data: JSON.stringify(obs) });
  }, config);
  await page.goto(url);
  await page.waitForFunction(() => document.querySelector("#status-text").textContent === "IDLE");
  return page;
}

async function start(page) {
  await page.evaluate(() => { window.startTask = document.querySelector("#start").onclick(); });
}
async function live(page) {
  await start(page);
  await page.waitForFunction(() => document.querySelector("#status-text").textContent === "LIVE");
}
async function stop(page) {
  await page.evaluate(() => document.querySelector("#stop").onclick());
}
async function snapshot(page) {
  return page.evaluate(() => ({
    status: document.querySelector("#status").dataset.state,
    startDisabled: document.querySelector("#start").disabled,
    stopDisabled: document.querySelector("#stop").disabled,
    controls: ["send-text", "commit-audio", "commit-text", "interrupt"].map((id) => document.getElementById(id).disabled),
    peers: harness.peers.map((p) => ({ closes: p.closes, dcCloses: p.dc.closes })),
    tracks: harness.tracks.map((t) => t.stops),
    contexts: harness.contexts.map((c) => c.closes),
    intervals: harness.intervals.size,
    opens: harness.opens, closes: harness.closes, errors: harness.errors,
  }));
}
async function released(page, expectedChannels = 1) {
  await page.waitForFunction(() => !document.querySelector("#start").disabled);
  const state = await snapshot(page);
  assert.equal(state.intervals, 0);
  assert.ok(state.tracks.every((count) => count === 1), JSON.stringify(state));
  assert.ok(state.peers.every((p) => p.closes === 1 && p.dcCloses === 1));
  assert.ok(state.contexts.every((count) => count === 1));
  assert.equal(state.closes.length, expectedChannels);
  assert.ok(state.controls.every(Boolean));
  assert.notEqual(state.status, "live");
  assert.deepEqual(state.errors, []);
}

test("Start/Start/Stop rejects duplicate starts and permits a clean restart", async () => {
  const page = await setup();
  try {
    await live(page);
    await start(page); // Direct invocation also respects the guard.
    assert.equal((await snapshot(page)).opens, 1);
    await stop(page);
    await released(page);
    await live(page);
    await stop(page);
    await released(page, 2);
  } finally { await page.close(); }
});

test("Stop during startup owns a late microphone and ignores a stale attempt", async () => {
  const page = await setup({ micPending: true });
  try {
    await start(page);
    await start(page);
    await page.waitForFunction(() => Boolean(harness.resolveMic));
    assert.equal((await snapshot(page)).stopDisabled, false);
    await stop(page);
    await released(page);
    await page.evaluate(() => { harness.config.micPending = false; });
    await live(page);
    await page.evaluate(() => harness.resolveMic());
    assert.equal((await snapshot(page)).status, "live");
    await stop(page);
    await released(page, 2);
  } finally { await page.close(); }
});

test("denied microphone still closes late live/open and observes early open rejection", async () => {
  for (const config of [{ micDenied: true, openPending: true }, { micPending: true, openFailure: true }]) {
    const page = await setup(config);
    try {
      await start(page);
      if (config.openPending) {
        await page.waitForFunction(() => !document.querySelector("#start").disabled);
        await page.evaluate(() => harness.resolveOpen());
        await page.waitForFunction(() => harness.closes.length === 1);
        await released(page);
      } else {
        await stop(page);
        await released(page, 0);
      }
    } finally { await page.close(); }
  }
});

test("answer/remote failures release all resources", async () => {
  for (const config of [{ answerFailure: true }, { remoteFailure: true }]) {
    const page = await setup(config);
    try { await start(page); await released(page); } finally { await page.close(); }
  }
});

test("partial meter allocation failures release the allocated AudioContext", async () => {
  for (const phase of ["analyser", "source", "connect"]) {
    const page = await setup();
    try {
      await page.evaluate((phase) => {
        const fail = () => { throw new DOMException("synthetic audio backend allocation failure", "OperationError"); };
        if (phase === "analyser") AudioContext.prototype.createAnalyser = fail;
        if (phase === "source") AudioContext.prototype.createMediaStreamSource = fail;
        if (phase === "connect") AudioContext.prototype.createMediaStreamSource = () => ({ connect: fail });
      }, phase);
      await start(page);
      await released(page);
      const state = await snapshot(page);
      assert.deepEqual(state.contexts, [1]);
      assert.equal(state.status, "error");
    } finally { await page.close(); }
  }
});

test("terminal callback at microphone settlement cannot allocate a late meter", async () => {
  const page = await setup();
  try {
    await page.evaluate(async () => {
      const { LiveAttempt } = await import("/live-session.js");
      const original = LiveAttempt.prototype.microphone;
      LiveAttempt.prototype.microphone = async function () {
        const stream = await original.call(this);
        queueMicrotask(() => {
          this.pc.connectionState = "failed";
          this.pc.emit("connectionstatechange");
        });
        return stream;
      };
    });
    await start(page);
    await released(page);
    const state = await snapshot(page);
    assert.deepEqual(state.contexts, []);
    assert.equal(state.status, "error");
  } finally { await page.close(); }
});

test("terminal observations and transports stop capture; nonterminal events do not", async () => {
  for (const terminal of ["dc", "pc", "closed", "error"]) {
    const page = await setup();
    try {
      await live(page);
      await page.evaluate((terminal) => {
        const pc = harness.peers[0];
        harness.observe({ observation: "command_rejected", message: "synthetic rejection" });
        harness.observe({ observation: "status_changed", status: { status: "degraded" } });
        pc.iceConnectionState = "disconnected"; pc.emit("iceconnectionstatechange");
        harness.oldMessage = pc.dc.onmessage;
      }, terminal);
      assert.equal((await snapshot(page)).status, "live");
      assert.equal((await snapshot(page)).tracks[0], 0);
      await page.evaluate((terminal) => {
        const pc = harness.peers[0];
        if (terminal === "dc") { pc.dc.readyState = "closed"; pc.dc.emit("close"); }
        if (terminal === "pc") { pc.connectionState = "failed"; pc.emit("connectionstatechange"); }
        if (terminal === "closed") harness.observe({ observation: "status_changed", status: { status: "closed" } });
        if (terminal === "error") harness.observe({ observation: "error", message: "terminal fault" });
      }, terminal);
      await released(page);
      await live(page);
      await page.evaluate(() => harness.oldMessage({ data: JSON.stringify({ observation: "error", message: "obsolete" }) }));
      assert.equal((await snapshot(page)).status, "live");
      await stop(page);
      await released(page, 2);
    } finally { await page.close(); }
  }
});

test("startup deadlines bound hung open, answer, and ICE; Stop cancels ICE", async () => {
  for (const config of [{ openPending: true }, { answerPending: true }, { gathering: true }]) {
    const page = await setup(config);
    try {
      await start(page);
      await page.waitForFunction(() => harness.tracks.length === 1);
      await page.clock.runFor(config.gathering ? 10_001 : 45_001);
      await released(page, config.openPending ? 0 : 1);
      if (config.openPending) {
        await page.evaluate(() => harness.resolveOpen());
        await page.waitForFunction(() => harness.closes.length === 1);
        await released(page);
      }
    } finally { await page.close(); }
  }
  const page = await setup({ gathering: true });
  try {
    await start(page);
    await page.waitForFunction(() => harness.tracks.length === 1);
    await stop(page);
    await released(page);
    await page.evaluate(() => {
      harness.peers[0].iceGatheringState = "complete";
      harness.peers[0].emit("icegatheringstatechange");
    });
    await released(page);
  } finally { await page.close(); }
});

test("real cockpit observation handlers correlate spoken and written rows", async () => {
  const page = await setup();
  try {
    await live(page);
    await page.evaluate(() => {
      const old = { response_id: "old", provider_item_id: "old-item", content_index: 0 };
      const fresh = { response_id: "fresh", provider_item_id: "new-item", content_index: 0 };
      harness.observe({ ...old, observation: "assistant_transcript_delta", delta: "obsolete prefix" });
      harness.observe({ ...old, provider_item_id: "written", observation: "assistant_text_delta", delta: "written survives" });
      harness.observe({ observation: "turn_interrupted", response_id: "old" });
      harness.observe({ ...old, observation: "assistant_transcript_truncated", text: "heard" });
      harness.observe({ observation: "user_transcript_final", text: "new question" });
      harness.observe({ ...fresh, observation: "assistant_transcript_delta", delta: "fresh" });
      harness.observe({ ...old, observation: "assistant_transcript_delta", delta: "late" });
      harness.observe({ ...old, observation: "assistant_transcript_final", text: "stale final" });
      harness.observe({ observation: "turn_completed", response_id: "old" });
      harness.observe({ observation: "assistant_transcript_delta", delta: "LATE ANONYMOUS" });
      harness.observe({ observation: "assistant_transcript_final", text: "STALE ANONYMOUS FINAL" });
    });
    const rows = await page.locator(".turn").allTextContents();
    assert.equal(rows.length, 4);
    assert.match(rows[0], /heard/);
    assert.match(rows[1], /written survives/);
    assert.match(rows[3], /fresh/);
    assert.ok(!rows.join().includes("obsolete") && !rows.join().includes("stale"));
    assert.ok(!rows.join().includes("ANONYMOUS"));
    assert.equal(await page.locator(".turn__body--draft").count(), 1);
    await page.evaluate(() => harness.observe({
      observation: "assistant_transcript_final", response_id: "fresh", provider_item_id: "new-item", content_index: 0, text: "fresh final",
    }));
    assert.equal(await page.locator(".turn__body--draft").count(), 0);
    await stop(page);
    await released(page);
  } finally { await page.close(); }
});

test("cockpit refuses response-only late transcript after item-scoped truncation", async () => {
  const page = await setup();
  try {
    await live(page);
    await page.evaluate(() => {
      const first = { response_id: "shared", provider_item_id: "one", content_index: 0 };
      const second = { response_id: "shared", provider_item_id: "two", content_index: 0 };
      harness.observe({ ...first, observation: "assistant_transcript_delta", delta: "one" });
      harness.observe({ ...second, observation: "assistant_transcript_delta", delta: "two" });
      harness.observe({ ...first, observation: "assistant_transcript_truncated", text: "heard" });
      harness.observe({ response_id: "shared", observation: "assistant_transcript_delta", delta: "LATE TRUNCATED ITEM" });
      harness.observe({ response_id: "shared", observation: "assistant_transcript_final", text: "LATE FINAL" });
      harness.observe({ ...second, observation: "assistant_transcript_delta", delta: " continued" });
      harness.observe({ response_id: "shared", observation: "assistant_text_delta", delta: "written survives" });
    });
    const rows = await page.locator(".turn").allTextContents();
    assert.equal(rows.length, 3);
    assert.match(rows[0], /heard/);
    assert.match(rows[1], /two continued/);
    assert.match(rows[2], /written survives/);
    assert.ok(!rows.join().includes("LATE"));
    await stop(page);
    await released(page);
  } finally { await page.close(); }
});

test("late item identity narrows interruption without suppressing a newer response", async () => {
  const page = await setup();
  try {
    await live(page);
    await page.evaluate(() => {
      harness.observe({ observation: "assistant_transcript_truncated", provider_item_id: "old-item", content_index: 0 });
      harness.observe({ observation: "assistant_transcript_delta", response_id: "old", provider_item_id: "old-item", content_index: 1, delta: "surviving old content" });
      harness.observe({ observation: "assistant_transcript_delta", response_id: "new", provider_item_id: "new-item", delta: "new" });
      harness.observe({ observation: "assistant_transcript_delta", response_id: "new", delta: " continued" });
      harness.observe({ observation: "assistant_transcript_delta", response_id: "old", content_index: 0, delta: "LATE" });
    });
    const rows = await page.locator(".turn").allTextContents();
    assert.equal(rows.length, 2);
    assert.match(rows[0], /surviving old content/);
    assert.match(rows[1], /new continued/);
    assert.ok(!rows.join().includes("LATE"));
    await stop(page);
    await released(page);
  } finally { await page.close(); }
});

test("no-row truncation identity binds a sibling before whole-response interruption", async () => {
  const page = await setup();
  try {
    await live(page);
    await page.evaluate(() => {
      harness.observe({ observation: "assistant_transcript_truncated", response_id: "known", provider_item_id: "shared-item", content_index: 0 });
      harness.observe({ observation: "assistant_transcript_delta", provider_item_id: "shared-item", content_index: 1, delta: "identified sibling" });
      harness.observe({ observation: "turn_interrupted", response_id: "known" });
      harness.observe({ observation: "assistant_transcript_delta", provider_item_id: "shared-item", content_index: 1, delta: "LATE" });
    });
    const rows = await page.locator(".turn").allTextContents();
    assert.equal(rows.length, 1);
    assert.match(rows[0], /identified sibling/);
    assert.ok(!rows[0].includes("LATE"));
    assert.equal(await page.locator(".turn__body--draft").count(), 0);
    await stop(page);
    await released(page);
  } finally { await page.close(); }
});

test("shared smoke negotiation releases resources on success and failures", async () => {
  for (const config of [{}, { micDenied: true }, { micDenied: true, openPending: true }, { answerFailure: true }, { gathering: true }]) {
    const page = await setup(config);
    try {
      const pending = page.evaluate(pageSmoke);
      await page.waitForFunction(() => harness.peers.length > 0);
      if (config.openPending) {
        await page.waitForFunction(() => harness.peers[0].closes === 1);
        await page.evaluate(() => harness.resolveOpen());
      }
      await page.clock.runFor(config.gathering ? 10_001 : 2500);
      const evidence = await pending;
      const state = await snapshot(page);
      assert.equal(state.closes.length, 1);
      assert.ok(state.peers.every((p) => p.closes === 1 && p.dcCloses === 1));
      assert.ok(state.tracks.every((count) => count === 1));
      if (Object.keys(config).length) assert.ok(evidence.error);
      else assert.equal(evidence.final.dc, "open");
    } finally { await page.close(); }
  }
});
