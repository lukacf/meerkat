export const STARTUP_TIMEOUT_MS = 45_000;
export const ICE_TIMEOUT_MS = 10_000;
export const CLOSE_TIMEOUT_MS = 5_000;

export function abortable(promise, signal) {
  return new Promise((resolve, reject) => {
    const abort = () => {
      signal.removeEventListener("abort", abort);
      reject(signal.reason || new Error("Stopped"));
    };
    signal.addEventListener("abort", abort, { once: true });
    Promise.resolve(promise).then(resolve, reject).finally(() => {
      signal.removeEventListener("abort", abort);
    });
    if (signal.aborted) abort();
  });
}

export function waitForIceGatheringComplete(pc, signal, timeout = ICE_TIMEOUT_MS) {
  return new Promise((resolve, reject) => {
    const finish = (error) => {
      clearTimeout(timer);
      pc.removeEventListener("icegatheringstatechange", check);
      pc.removeEventListener("connectionstatechange", check);
      pc.removeEventListener("signalingstatechange", check);
      signal.removeEventListener("abort", abort);
      if (error) reject(error);
      else resolve();
    };
    const abort = () => finish(signal.reason || new Error("Stopped"));
    const check = () => {
      if (["failed", "closed"].includes(pc.connectionState) || pc.signalingState === "closed") {
        finish(new Error("Peer closed during ICE gathering"));
      } else if (pc.iceGatheringState === "complete") {
        finish();
      }
    };
    const timer = setTimeout(() => finish(new Error("ICE gathering timed out")), timeout);
    pc.addEventListener("icegatheringstatechange", check);
    pc.addEventListener("connectionstatechange", check);
    pc.addEventListener("signalingstatechange", check);
    signal.addEventListener("abort", abort, { once: true });
    if (signal.aborted) abort();
    else check();
  });
}

export async function fetchJson(url, options = {}) {
  const response = await fetch(url, {
    ...options,
    headers: { "content-type": "application/json", ...options.headers },
  });
  const body = await response.json().catch(() => ({}));
  if (!response.ok) throw new Error(body.error || `${response.status} ${response.statusText}`);
  return body;
}

// Each acquisition is observed even after cancellation: neither getUserMedia nor
// a server-side live/open is undone by abandoning its JavaScript await.
export class LiveAttempt {
  constructor(onCloseError = () => {}, timeout = STARTUP_TIMEOUT_MS) {
    this.controller = new AbortController();
    this.signal = this.controller.signal;
    this.streams = new Set();
    this.cleanups = [];
    this.onCloseError = onCloseError;
    this.deadline = setTimeout(() => this.controller.abort(new Error("Startup timed out")), timeout);
    try {
      this.pc = new RTCPeerConnection();
      this.dc = this.pc.createDataChannel("meerkat.live");
    } catch (error) {
      clearTimeout(this.deadline);
      this.pc?.close();
      throw error;
    }
  }

  wait(promise) {
    return abortable(promise, this.signal);
  }

  open(body) {
    this.openPromise = fetchJson("/api/start", {
      method: "POST", body: JSON.stringify(body),
    }).then((open) => {
      this.openResult = open;
      if (this.disposed) void this.closeChannel();
      return open;
    });
    // Install the rejection handler before microphone acquisition can fail.
    this.openPromise.catch(() => {});
    return this.openPromise;
  }

  ownStream(stream) {
    if (this.disposed) stream.getTracks().forEach((track) => track.stop());
    else this.streams.add(stream);
    return stream;
  }

  microphone() {
    return this.wait(navigator.mediaDevices.getUserMedia({
      audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
      video: false,
    }).then((stream) => this.ownStream(stream)));
  }

  async negotiate() {
    const offer = await this.wait(this.pc.createOffer());
    await this.wait(this.pc.setLocalDescription(offer));
    await waitForIceGatheringComplete(this.pc, this.signal);
    const open = await this.wait(this.openPromise);
    const offerSdp = this.pc.localDescription.sdp;
    const answer = await this.wait(fetchJson("/api/webrtc/answer", {
      method: "POST",
      signal: this.signal,
      body: JSON.stringify({
        channel_id: open.channel_id, token: open.transport.token, offer_sdp: offerSdp,
      }),
    }));
    await this.wait(this.pc.setRemoteDescription({ type: "answer", sdp: answer.answer_sdp }));
    return { open, offerSdp, answerSdp: answer.answer_sdp };
  }

  async connected() {
    const { pc, dc, signal } = this;
    await new Promise((resolve, reject) => {
      const finish = (error) => {
        pc.removeEventListener("connectionstatechange", check);
        pc.removeEventListener("iceconnectionstatechange", check);
        dc.removeEventListener("open", check);
        dc.removeEventListener("close", check);
        signal.removeEventListener("abort", abort);
        if (error) reject(error);
        else resolve();
      };
      const abort = () => finish(signal.reason || new Error("Stopped"));
      const check = () => {
        if (["closed", "failed"].includes(pc.connectionState) || dc.readyState === "closed") {
          finish(new Error("Transport closed during startup"));
        } else if (dc.readyState === "open" && ["connected", "completed"].includes(pc.iceConnectionState)) {
          finish();
        }
      };
      pc.addEventListener("connectionstatechange", check);
      pc.addEventListener("iceconnectionstatechange", check);
      dc.addEventListener("open", check);
      dc.addEventListener("close", check);
      signal.addEventListener("abort", abort, { once: true });
      if (signal.aborted) abort();
      else check();
    });
    clearTimeout(this.deadline);
  }

  closeChannel() {
    if (!this.openResult) return Promise.resolve();
    if (this.closePromise) return this.closePromise;
    this.closePromise = (async () => {
      const controller = new AbortController();
      const timer = setTimeout(() => controller.abort(new Error("Channel close timed out")), CLOSE_TIMEOUT_MS);
      try {
        await abortable(fetchJson(`/api/live/${encodeURIComponent(this.openResult.channel_id)}/close`, {
          method: "POST", body: "{}", signal: controller.signal, keepalive: true,
        }), controller.signal);
      } catch (error) {
        this.onCloseError(error);
      } finally {
        clearTimeout(timer);
      }
    })();
    return this.closePromise;
  }

  dispose() {
    if (this.disposePromise) return this.disposePromise;
    this.disposed = true;
    clearTimeout(this.deadline);
    this.controller.abort(new Error("Stopped"));
    this.pc.onconnectionstatechange = this.pc.oniceconnectionstatechange =
      this.pc.onsignalingstatechange = this.pc.ontrack = null;
    this.dc.onopen = this.dc.onclose = this.dc.onmessage = null;
    for (const cleanup of this.cleanups.splice(0)) cleanup();
    const tracks = new Set([...this.streams].flatMap((stream) => stream.getTracks()));
    for (const track of tracks) track.stop();
    this.streams.clear();
    this.dc.close();
    this.pc.close();
    this.disposePromise = this.closeChannel();
    return this.disposePromise;
  }
}
