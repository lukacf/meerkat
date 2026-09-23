const fs = require("node:fs");

const TEST_TIMEOUT_MS = 90_000;
const OPERATION_TIMEOUT_MS = 80_000;

async function pageSmoke() {
  const { LiveAttempt, CLOSE_TIMEOUT_MS } = await import("/live-session.js");
  const result = { startedAt: new Date().toISOString(), states: [], observations: [] };
  let owner;
  try {
    owner = new LiveAttempt((error) => { result.close_error = error.message; });
    const { pc, dc } = owner;
    const record = (kind, value) => result.states.push({ t: new Date().toISOString(), kind, value });
    pc.onconnectionstatechange = () => record("pc", pc.connectionState);
    pc.oniceconnectionstatechange = () => record("ice", pc.iceConnectionState);
    pc.onsignalingstatechange = () => record("signaling", pc.signalingState);
    pc.ontrack = (event) => {
      owner.ownStream(event.streams[0] || new MediaStream([event.track]));
      record("remote-track", `${event.track.kind}:${event.track.readyState}`);
    };
    dc.onopen = () => record("data", "open");
    dc.onclose = () => record("data", "closed");
    dc.onmessage = (message) => {
      try { result.observations.push(JSON.parse(message.data)); }
      catch { result.observations.push({ raw: String(message.data).slice(0, 120) }); }
    };
    owner.open({ turningMode: "explicit_commit" });
    const stream = await owner.microphone();
    record("mic", stream.getAudioTracks().map((t) => `${t.kind}:${t.readyState}`).join(","));
    pc.addTrack(stream.getAudioTracks()[0], stream);
    const { open, offerSdp, answerSdp } = await owner.negotiate();
    Object.assign(result, {
      session_id: open.session_id, channel_id: open.channel_id,
      capabilities: open.capabilities, continuity: open.continuity,
      tool_count: Array.isArray(open.tools) ? open.tools.length : null,
      offer_sdp_len: offerSdp.length, answer_sdp_len: answerSdp.length,
    });
    await owner.connected();
    await new Promise((resolve) => setTimeout(resolve, 2000));
    result.final = {
      pc: pc.connectionState, ice: pc.iceConnectionState,
      signaling: pc.signalingState, dc: dc.readyState,
    };
  } catch (error) {
    result.error = error.message;
  } finally {
    if (owner) {
      await owner.dispose();
      // Give a late open its own bounded close opportunity before destroying
      // the page. A still-pending HTTP open is closed by the host on disconnect.
      if (owner.openPromise) {
        let timer;
        await Promise.race([
          owner.openPromise.catch(() => {}),
          new Promise((resolve) => { timer = setTimeout(resolve, CLOSE_TIMEOUT_MS); }),
        ]);
        clearTimeout(timer);
        await owner.closeChannel();
      }
    }
    result.endedAt = new Date().toISOString();
    result.observation_count = result.observations.length;
    result.observations = result.observations.slice(0, 10);
  }
  return result;
}

async function runSmoke(chromium, {
  url = "http://127.0.0.1:4173/",
  evidencePath = ".smoke/webrtc-evidence.json",
} = {}) {
  let browser;
  let timer;
  let evidence = { startedAt: new Date().toISOString() };
  try {
    const work = async () => {
      browser = await chromium.launch({
        channel: "chrome", headless: true, timeout: 10_000,
        args: ["--use-fake-device-for-media-stream", "--use-fake-ui-for-media-stream", "--no-sandbox"],
      });
      const context = await browser.newContext({ permissions: ["microphone"] });
      const page = await context.newPage();
      page.on("console", (msg) => console.log(`[browser:${msg.type()}] ${msg.text()}`));
      await page.goto(url, { waitUntil: "domcontentloaded", timeout: 10_000 });
      return page.evaluate(pageSmoke);
    };
    evidence = await Promise.race([
      work(),
      new Promise((_, reject) => {
        timer = setTimeout(() => reject(new Error("Whole smoke operation timed out")), OPERATION_TIMEOUT_MS);
      }),
    ]);
    if (evidence.error) throw new Error(evidence.error);
    if (evidence.close_error) throw new Error(evidence.close_error);
    if (evidence.final.dc !== "open" || !["connected", "completed"].includes(evidence.final.ice)) {
      throw new Error(`Transport not connected: ${JSON.stringify(evidence.final)}`);
    }
    return evidence;
  } catch (error) {
    evidence.error = error.message;
    throw error;
  } finally {
    clearTimeout(timer);
    evidence.endedAt ??= new Date().toISOString();
    try {
      // Failure evidence is durable before browser cleanup or the runner's outer deadline.
      fs.writeFileSync(evidencePath, JSON.stringify(evidence, null, 2));
    } finally {
      if (browser) await browser.close();
    }
  }
}

module.exports = { runSmoke, pageSmoke, TEST_TIMEOUT_MS, OPERATION_TIMEOUT_MS };
