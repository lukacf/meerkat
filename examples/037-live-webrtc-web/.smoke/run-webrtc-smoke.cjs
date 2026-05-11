const { chromium } = require('/Users/luka/.npm/_npx/e41f203b7505f1fb/node_modules/playwright');
const fs = require('node:fs');
(async () => {
  const browser = await chromium.launch({
    channel: 'chrome',
    headless: true,
    args: [
      '--use-fake-device-for-media-stream',
      '--use-fake-ui-for-media-stream',
      '--autoplay-policy=no-user-gesture-required',
      '--no-sandbox',
    ],
  });
  try {
    const context = await browser.newContext({ permissions: ['microphone'] });
    const page = await context.newPage();
    page.on('console', msg => console.log(`[browser:${msg.type()}] ${msg.text()}`));
    await page.goto('http://127.0.0.1:4173/', { waitUntil: 'domcontentloaded' });
    const evidence = await page.evaluate(async () => {
      const wait = (ms) => new Promise((resolve) => setTimeout(resolve, ms));
      const fetchJson = async (url, body) => {
        const res = await fetch(url, {
          method: body === undefined ? 'GET' : 'POST',
          headers: body === undefined ? undefined : { 'content-type': 'application/json' },
          body: body === undefined ? undefined : JSON.stringify(body),
        });
        const json = await res.json().catch(() => ({}));
        if (!res.ok) throw new Error(`${url} ${res.status}: ${json.error || res.statusText}`);
        return json;
      };
      const startedAt = new Date().toISOString();
      const pc = new RTCPeerConnection();
      const states = [];
      const observations = [];
      const record = (kind, value) => states.push({ t: new Date().toISOString(), kind, value });
      pc.onconnectionstatechange = () => record('pc', pc.connectionState);
      pc.oniceconnectionstatechange = () => record('ice', pc.iceConnectionState);
      pc.onsignalingstatechange = () => record('signaling', pc.signalingState);
      pc.ontrack = (event) => record('remote-track', `${event.track.kind}:${event.track.readyState}`);
      const dc = pc.createDataChannel('meerkat-live');
      dc.onopen = () => record('data', 'open');
      dc.onclose = () => record('data', 'closed');
      dc.onmessage = (message) => {
        try { observations.push(JSON.parse(message.data)); }
        catch { observations.push({ raw: String(message.data).slice(0, 120) }); }
      };
      const stream = await navigator.mediaDevices.getUserMedia({
        audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
        video: false,
      });
      record('mic', stream.getAudioTracks().map((t) => `${t.kind}:${t.readyState}`).join(','));
      pc.addTrack(stream.getAudioTracks()[0], stream);
      const openPromise = fetchJson('/api/start', { turningMode: 'explicit_commit' });
      const offer = await pc.createOffer();
      await pc.setLocalDescription(offer);
      const open = await openPromise;
      const answer = await fetchJson('/api/webrtc/answer', {
        channel_id: open.channel_id,
        token: open.transport.token,
        offer_sdp: offer.sdp,
      });
      await pc.setRemoteDescription({ type: 'answer', sdp: answer.answer_sdp });
      const deadline = Date.now() + 30000;
      while (Date.now() < deadline) {
        if (dc.readyState === 'open' && ['connected', 'completed'].includes(pc.iceConnectionState)) break;
        await wait(100);
      }
      await wait(2000);
      const result = {
        startedAt,
        endedAt: new Date().toISOString(),
        session_id: open.session_id,
        channel_id: open.channel_id,
        capabilities: open.capabilities,
        continuity: open.continuity,
        tool_count: Array.isArray(open.tools) ? open.tools.length : null,
        offer_sdp_len: offer.sdp.length,
        answer_sdp_len: answer.answer_sdp.length,
        final: { pc: pc.connectionState, ice: pc.iceConnectionState, signaling: pc.signalingState, dc: dc.readyState },
        states,
        observations: observations.slice(0, 10),
        observation_count: observations.length,
      };
      await fetchJson(`/api/live/${encodeURIComponent(open.channel_id)}/close`, {}).catch((e) => { result.close_error = e.message; });
      dc.close(); pc.close(); stream.getTracks().forEach((t) => t.stop());
      return result;
    });
    fs.writeFileSync('.smoke/webrtc-evidence.json', JSON.stringify(evidence, null, 2));
    console.log(JSON.stringify(evidence, null, 2));
    if (evidence.final.dc !== 'open') throw new Error(`data channel not open: ${JSON.stringify(evidence.final)}`);
    if (!['connected', 'completed'].includes(evidence.final.ice)) throw new Error(`ICE not connected: ${JSON.stringify(evidence.final)}`);
  } finally {
    await browser.close();
  }
})().catch((err) => { console.error(err); process.exit(1); });
