import { test, vi } from "vitest";
import assert from "node:assert/strict";
import voiceContract from "../../../crates/meerkat-meta/tests/fixtures/console_voice_v1.json";
import { parseVoiceContextStatus, voiceContextFailureMessage } from "./voice-context";
import { parseVoiceCaptions, VOICE_CAPTIONS_METHOD } from "./voice-captions";
import {
  createVoiceSession,
  queryVoiceAvailability,
  queryVoiceReadiness,
  VOICE_CONNECT_TIMEOUT_MS,
  VOICE_RECOVERY_TIMEOUT_MS,
  VOICE_SILENCE_TIMEOUT_MS,
  VOICE_TEARDOWN_TIMEOUT_MS,
  VOICE_CLOSE_RETRY_DELAYS_MS,
  VOICE_REPLACEMENT_POLL_INTERVAL_MS,
  VOICE_ACTIVITY_REPORT_INTERVAL_MS,
  VOICE_TRANSPORT_RECONNECT_GRACE_MS,
  VOICE_RPC_FAILURE_TOLERANCE_MS,
  VOICE_AUDIO_RESUME_TIMEOUT_MS,
  VOICE_CAPTION_WAIT_MS,
  VOICE_CAPTION_RETRY_DELAY_MS,
  isTransientRpcFailure,
  type VoiceAvailability,
  type VoiceSessionEnvironment,
} from "./voice-session";

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((yes, no) => { resolve = yes; reject = no; });
  return { promise, resolve, reject };
}

async function flush() {
  for (let i = 0; i < 40; i++) await Promise.resolve();
}

class Clock {
  now = 0;
  id = 0;
  timers = new Map<number, { at: number; callback: () => void }>();
  setTimeout = (callback: () => void, milliseconds: number) => {
    const id = ++this.id;
    this.timers.set(id, { at: this.now + milliseconds, callback });
    return id as unknown as ReturnType<typeof setTimeout>;
  };
  clearTimeout = (id: ReturnType<typeof setTimeout>) => {
    this.timers.delete(id as unknown as number);
  };
  async advance(milliseconds: number) {
    await flush();
    const until = this.now + milliseconds;
    for (;;) {
      const next = [...this.timers].filter(([, timer]) => timer.at <= until)
        .sort((a, b) => a[1].at - b[1].at || a[0] - b[0])[0];
      if (!next) break;
      this.now = next[1].at;
      this.timers.delete(next[0]);
      next[1].callback();
      await flush();
    }
    this.now = until;
    await flush();
  }
}

class Track {
  kind = "audio";
  enabled = true;
  stopped = false;
  onended: (() => void) | null = null;
  stop() { this.stopped = true; }
  end() { this.onended?.(); }
}

class Stream {
  tracks: Track[];
  constructor(tracks = [new Track()]) { this.tracks = tracks; }
  getTracks() { return this.tracks; }
  getAudioTracks() { return this.tracks; }
  addTrack(track: Track) { this.tracks.push(track); }
}

class Node {
  connections: Node[] = [];
  disconnected = false;
  connect(node: Node) { this.connections.push(node); return node; }
  disconnect() { this.disconnected = true; }
}

class Analyser extends Node {
  fftSize = 2048;
  signal = 0;
  getFloatTimeDomainData(samples: Float32Array) { samples.fill(this.signal); }
}

class Gain extends Node {
  /** Ramp targets as [target, startTime, timeConstant]; a ramp settles to its target. */
  targets: [number, number, number][] = [];
  gain = {
    value: 1,
    cancelScheduledValues: (_time: number) => {},
    setTargetAtTime: (target: number, startTime: number, timeConstant: number) => {
      this.targets.push([target, startTime, timeConstant]);
      this.gain.value = target;
    },
  };
}

/**
 * The master (speaker) gain of the `attempt`-th peer preparation on `context`. Each
 * preparation creates its master gain and then its barge-in playback gate.
 */
function masterGain(context: { gains: Gain[] }, attempt: number): Gain {
  return context.gains[attempt * 2];
}

class Context {
  state = "suspended";
  currentTime = 0;
  onstatechange: (() => void) | null = null;
  destination = new Node();
  analysers: Analyser[] = [];
  gains: Gain[] = [];
  sources: Node[] = [];
  resumed = false;
  resume() { this.resumed = true; this.state = "running"; return Promise.resolve(); }
  close() { this.state = "closed"; return Promise.resolve(); }
  createAnalyser() { const node = new Analyser(); this.analysers.push(node); return node; }
  createGain() { const node = new Gain(); this.gains.push(node); return node; }
  createMediaStreamSource() { const node = new Node(); this.sources.push(node); return node; }
}

class Channel {
  readyState = "connecting";
  onmessage: ((event: { data: unknown }) => void) | null = null;
  onclose: (() => void) | null = null;
  onerror: (() => void) | null = null;
  close() { this.readyState = "closed"; }
  emit(value: unknown) { this.onmessage?.({ data: JSON.stringify(value) }); }
}

class Peer {
  connectionState = "new";
  iceConnectionState = "new";
  channel = new Channel();
  ontrack: ((event: { track: Track }) => void) | null = null;
  onconnectionstatechange: (() => void) | null = null;
  oniceconnectionstatechange: (() => void) | null = null;
  localDescription: { type: string; sdp: string } | null = null;
  tracks: Track[] = [];
  offerPrepared = false;
  remoteTrack = new Track();
  onOffer: (() => void) | undefined;
  createDataChannel() { return this.channel; }
  addTrack(track: Track) { this.tracks.push(track); }
  createOffer() {
    assert.ok(this.ontrack, "output consumer installed before offer");
    assert.ok(this.channel.onmessage, "data consumer installed before offer");
    assert.ok(this.tracks.every((track) => !track.enabled), "microphone gated before offer");
    this.onOffer?.();
    this.offerPrepared = true;
    return Promise.resolve({ type: "offer", sdp: "offer-sdp" });
  }
  setLocalDescription(offer: { type: string; sdp: string }) {
    this.localDescription = offer;
    return Promise.resolve();
  }
  setRemoteDescription() {
    assert.ok(this.tracks.every((track) => !track.enabled), "mic remains gated while applying answer");
    this.ontrack?.({ track: this.remoteTrack });
    this.connectionState = "connected";
    this.iceConnectionState = "connected";
    this.channel.readyState = "open";
    return Promise.resolve();
  }
  close() { this.connectionState = "closed"; this.iceConnectionState = "closed"; }
  /** Hard transport failure: ICE gave up. Terminal for this peer. */
  fail() { this.connectionState = "failed"; this.onconnectionstatechange?.(); }
  /** Transient ICE `disconnected`: the browser keeps probing and usually recovers. */
  disconnect() { this.connectionState = "disconnected"; this.onconnectionstatechange?.(); }
  reconnect() { this.connectionState = "connected"; this.onconnectionstatechange?.(); }
}

const target = { identity: "agent-a", label: "Agent A" };
const other = { identity: "agent-b", label: "Agent B" };

function pending(identity = target.identity, id = "channel-a") {
  return {
    channel_id: id,
    target_identity: identity,
    execution_mode: "client_context",
    pending_receipt: "pending-receipt",
    transport: { transport: "webrtc", token: "opaque-bootstrap", answer_method: "live/webrtc/answer" },
    capabilities: {
      audio_in: true, audio_out: true, text_in: false, text_out: false, image_in: false,
      video_in: false, transcript_supported: true, barge_in_supported: true, provider_native_resume: false,
    },
    continuity: { mode: "fresh" },
  };
}

function replacement(reason = "canonical_context", handle: unknown = pending(target.identity, "recovery-channel")) {
  return { required: true, reason, replacement: handle, canonical_seed_cursor: 17 };
}

function contextStatus(params: Record<string, unknown>, preparation: Record<string, unknown> = { phase: "provider_acknowledged" }) {
  return {
    identity: params.identity,
    request_id: params.request_id,
    channel_id: params.channel_id,
    context_preparation: preparation,
  };
}

test("context status accepts every shared Rust phase and failure projection without changing their meaning", () => {
  const request = voiceContract.context_status_request;
  const scope = { identity: request.identity, requestId: request.request_id, channelId: request.channel_id };
  for (const response of Object.values(voiceContract.context_status_responses)) {
    assert.deepEqual(parseVoiceContextStatus(response, scope), response.context_preparation);
  }
  for (const reason of voiceContract.context_status_failure_reasons) {
    const response = {
      ...request,
      context_preparation: { phase: "failed", reason },
    };
    const preparation = parseVoiceContextStatus(response, scope);
    assert.deepEqual(preparation, response.context_preparation);
    assert.equal(preparation.phase, "failed");
    if (preparation.phase === "failed") {
      const message = voiceContextFailureMessage(preparation.reason);
      assert.match(message, /Voice remains connected/);
      assert.doesNotMatch(message, /undefined/);
    }
  }
});

function harness() {
  const clock = new Clock();
  const streams: Stream[] = [];
  const peers: Peer[] = [];
  const contexts: Context[] = [];
  const calls: { method: string; params: Record<string, unknown> }[] = [];
  const pagehideCalls: Record<string, unknown>[] = [];
  let pagehide: (() => void) | undefined;
  let pageshow: (() => void) | undefined;
  let requestId = 0;
  let getMedia: (() => Promise<Stream>) | undefined;
  let onRpc: ((method: string, params: Record<string, unknown>) => Promise<unknown> | undefined) | undefined;
  let onContext: ((context: Context) => void) | undefined;
  let onPeer: ((peer: Peer) => void) | undefined;
  const env: VoiceSessionEnvironment = {
    voiceAvailable: () => Promise.resolve("available" as const),
    createAudioContext: () => {
      const context = new Context();
      contexts.push(context);
      onContext?.(context);
      return context as unknown as AudioContext;
    },
    getUserMedia: () => {
      if (getMedia) return getMedia() as unknown as Promise<MediaStream>;
      const stream = new Stream();
      streams.push(stream);
      return Promise.resolve(stream as unknown as MediaStream);
    },
    createPeerConnection: () => {
      const peer = new Peer();
      peers.push(peer);
      onPeer?.(peer);
      return peer as unknown as RTCPeerConnection;
    },
    createMediaStream: (tracks) => new Stream((tracks ?? []) as unknown as Track[]) as unknown as MediaStream,
    createAudioElement: () => ({
      autoplay: false, muted: false, srcObject: null, play: () => Promise.resolve(), pause() {},
    }) as unknown as HTMLAudioElement,
    rpc: async (method, params) => {
      calls.push({ method, params });
      const overridden = onRpc?.(method, params);
      if (overridden) return overridden;
      if (method === "meta/console/voice/readiness") return { identity: params.identity, available: true };
      if (method === "meta/console/voice/replacement") return { required: false };
      if (method === "meta/console/voice/answer_received") return { accepted: true };
      if (method === "meta/console/voice/activity") return { accepted: true };
      if (method === "meta/console/voice/context_status") return contextStatus(params);
      // The caption long poll waits; tests that need captions script it.
      if (method === "meta/console/voice/captions") return new Promise(() => {});
      if (method === "meta/console/voice/open") {
        return pending(params.identity as string, `channel-${params.identity}`);
      }
      if (method === "meta/console/voice/close") return { phase: "closed" };
      if (method === "meta/live/playback_owner/register") {
        return { channel_id: params.channel_id, readiness_receipt: "readiness-receipt" };
      }
      if (method === "live/webrtc/answer") return { answer_sdp: "answer-sdp" };
      if (method === "meta/live/status") {
        return {
          phase: "active",
          handle: {
            channel_id: params.channel_id,
            target_identity: params.identity,
            execution_mode: "client_context",
            activation_receipt: "activation-receipt",
          },
        };
      }
      throw new Error(`Unexpected RPC: ${method}`);
    },
    now: () => clock.now,
    randomId: () => `request-${++requestId}`,
    setTimeout: clock.setTimeout,
    clearTimeout: clock.clearTimeout,
    onPagehide: (listener) => { pagehide = listener; return () => { pagehide = undefined; }; },
    onPageshow: (listener) => { pageshow = listener; return () => { pageshow = undefined; }; },
    pagehideClose: (params) => { pagehideCalls.push(params); },
  };
  const controller = createVoiceSession("", env);
  return {
    controller, env, clock, calls, contexts, peers, streams, pagehideCalls,
    setMedia: (fn: typeof getMedia) => { getMedia = fn; },
    setRpc: (fn: typeof onRpc) => { onRpc = fn; },
    setContext: (fn: typeof onContext) => { onContext = fn; },
    setPeer: (fn: typeof onPeer) => { onPeer = fn; },
    pagehide: () => pagehide?.(),
    pageshow: () => pageshow?.(),
    hasPageshowListener: () => pageshow !== undefined,
  };
}

test("the context observer sends the exact shared Rust method and request shape", async () => {
  const h = harness();
  const request = voiceContract.context_status_request;
  h.env.randomId = () => request.request_id;
  h.setRpc((method) => {
    if (method === "meta/console/voice/open") return Promise.resolve(pending(request.identity, request.channel_id));
    if (method === voiceContract.context_status_method) return Promise.resolve(voiceContract.context_status_responses.generating);
    return undefined;
  });
  await h.controller.start({ identity: request.identity, label: "Golden agent" });
  await flush();
  assert.deepEqual(
    h.calls.find((call) => call.method === voiceContract.context_status_method)?.params,
    request,
  );
  assert.deepEqual(h.controller.getSnapshot().contextPreparation,
    voiceContract.context_status_responses.generating.context_preparation);
  await h.controller.close();
});

test("voice activates before a 20-second context job and context acknowledgement stops polling", async () => {
  const h = harness();
  h.setRpc((method, params) => {
    if (method === "meta/console/send") return Promise.resolve({ accepted: true });
    if (method !== "meta/console/voice/context_status") return undefined;
    return Promise.resolve(contextStatus(params, h.clock.now < 20_000
      ? { phase: "preparing", stage: "generating" }
      : { phase: "provider_acknowledged" }));
  });
  await h.controller.start(target);
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.streams[0].tracks[0].enabled, true);
  assert.equal(h.contexts[0].gains[0].gain.value, 1);
  assert.deepEqual(h.controller.getSnapshot().contextPreparation, { phase: "preparing", stage: "generating" });
  assert.deepEqual(await h.env.rpc("meta/console/send", { identity: target.identity, message: "Newer facts" }, 5000), { accepted: true });
  h.controller.toggleMicrophone();
  h.controller.toggleSpeaker();
  await h.clock.advance(19_999);
  assert.deepEqual(h.controller.getSnapshot().contextPreparation, { phase: "preparing", stage: "generating" });
  await h.clock.advance(1);
  assert.deepEqual(h.controller.getSnapshot().contextPreparation, { phase: "provider_acknowledged" });
  assert.equal(h.controller.getSnapshot().microphoneMuted, true);
  assert.equal(h.controller.getSnapshot().speakerMuted, true);
  const reads = h.calls.filter((call) => call.method.endsWith("/context_status")).length;
  await h.clock.advance(30_000);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/context_status")).length, reads);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/activity")).length, 0);
  await h.controller.close();
  assert.equal(h.clock.timers.size, 0);
});

test("a hanging context status read never gates audio and retries with a visible sanitized error", async () => {
  const h = harness();
  const read = deferred<unknown>();
  h.setRpc((method) => method.endsWith("/context_status") ? read.promise : undefined);
  await h.controller.start(target);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.controller.getSnapshot().contextPreparation, null);
  await h.clock.advance(VOICE_TEARDOWN_TIMEOUT_MS);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.match(h.controller.getSnapshot().contextStatusError!, /context status/i);
  assert.equal(h.streams[0].tracks[0].enabled, true);
  h.setRpc(undefined);
  await h.clock.advance(5000);
  assert.deepEqual(h.controller.getSnapshot().contextPreparation, { phase: "provider_acknowledged" });
  assert.equal(h.controller.getSnapshot().contextStatusError, null);
  read.reject(new Error("late secret=sk-context-body"));
  await flush();
  assert.equal(h.controller.getSnapshot().contextStatusError, null);
  await h.controller.close();
  assert.equal(h.clock.timers.size, 0);
});

test.each(["not_requested", "failed"])("terminal context %s remains explicit without closing audio", async (phase) => {
  const h = harness();
  const preparation = phase === "failed" ? { phase, reason: "timed_out" } : { phase };
  h.setRpc((method, params) => method.endsWith("/context_status")
    ? Promise.resolve(contextStatus(params, preparation)) : undefined);
  await h.controller.start(target);
  await flush();
  assert.deepEqual(h.controller.getSnapshot().contextPreparation, preparation);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.controller.getSnapshot().error, null);
  await h.clock.advance(10_000);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/context_status")).length, 1);
  await h.controller.close();
});

test.each([
  null,
  {},
  { phase: "provider_acknowledged", reason: "timed_out" },
  { phase: "preparing", stage: "invented" },
  { phase: "failed", reason: "sk-secret-provider-body" },
])("invalid context status %j cannot imply supplied context or interrupt audio", async (preparation) => {
  const h = harness();
  h.setRpc((method, params) => method.endsWith("/context_status")
    ? Promise.resolve({ ...contextStatus(params), context_preparation: preparation }) : undefined);
  await h.controller.start(target);
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.controller.getSnapshot().contextPreparation, null);
  assert.match(h.controller.getSnapshot().contextStatusError!, /context status/i);
  assert.doesNotMatch(JSON.stringify(h.controller.getSnapshot()), /sk-secret/);
  await h.controller.close();
});

test.each(["identity", "request_id", "channel_id", "unexpected_field"])("context status rejects a mismatched %s", async (field) => {
  const h = harness();
  h.setRpc((method, params) => method.endsWith("/context_status")
    ? Promise.resolve({ ...contextStatus(params), [field]: "different" }) : undefined);
  await h.controller.start(target);
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.controller.getSnapshot().contextPreparation, null);
  assert.match(h.controller.getSnapshot().contextStatusError!, /context status/i);
  await h.controller.close();
});

test.each(["close", "dispose", "pagehide"])("%s fences late context status and releases its timers", async (operation) => {
  const h = harness();
  const read = deferred<unknown>();
  h.setRpc((method) => method.endsWith("/context_status") ? read.promise : undefined);
  await h.controller.start(target);
  const request = h.calls.find((call) => call.method.endsWith("/context_status"))!;
  assert.ok(request);
  if (operation === "close") await h.controller.close();
  else if (operation === "dispose") h.controller.dispose();
  else h.pagehide();
  await flush();
  const snapshot = h.controller.getSnapshot();
  read.resolve(contextStatus(request.params));
  await flush();
  assert.equal(h.controller.getSnapshot(), snapshot);
  assert.equal(h.clock.timers.size, 0);
});

test("a previous agent's context reply cannot overwrite the new voice agent", async () => {
  const h = harness();
  const old = deferred<unknown>();
  h.setRpc((method, params) => {
    if (!method.endsWith("/context_status")) return undefined;
    return params.identity === target.identity ? old.promise
      : Promise.resolve(contextStatus(params, { phase: "preparing", stage: "generating" }));
  });
  await h.controller.start(target);
  const request = h.calls.find((call) => call.method.endsWith("/context_status"))!;
  assert.ok(request);
  await h.controller.start(other);
  await flush();
  old.resolve(contextStatus(request.params));
  await flush();
  assert.deepEqual(h.controller.getSnapshot().target, other);
  assert.deepEqual(h.controller.getSnapshot().contextPreparation, { phase: "preparing", stage: "generating" });
  await h.controller.close();
});

test("recovery replaces the context observer even though its request and agent are unchanged", async () => {
  const h = harness();
  const old = deferred<unknown>();
  let replace = false;
  h.setRpc((method, params) => {
    if (method.endsWith("/context_status")) return params.channel_id === "recovery-channel"
      ? Promise.resolve(contextStatus(params, { phase: "preparing", stage: "delivering" })) : old.promise;
    if (replace && method.endsWith("/replacement")) return Promise.resolve(replacement());
    return undefined;
  });
  await h.controller.start(target);
  const request = h.calls.find((call) => call.method.endsWith("/context_status"))!;
  assert.ok(request);
  replace = true;
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
  old.resolve(contextStatus(request.params));
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.deepEqual(h.controller.getSnapshot().contextPreparation, { phase: "preparing", stage: "delivering" });
  const reads = h.calls.filter((call) => call.method.endsWith("/context_status"));
  assert.equal(reads.at(-1)?.params.request_id, request.params.request_id);
  assert.equal(reads.at(-1)?.params.channel_id, "recovery-channel");
  await h.controller.close();
  assert.equal(h.clock.timers.size, 0);
});

test("context preparation and acknowledgement never extend the 15-minute audio silence deadline", async () => {
  const h = harness();
  h.setRpc((method, params) => method.endsWith("/context_status")
    ? Promise.resolve(contextStatus(params, h.clock.now < 800_000
      ? { phase: "preparing", stage: "generating" } : { phase: "provider_acknowledged" })) : undefined);
  await h.controller.start(target);
  await h.clock.advance(800_000);
  assert.deepEqual(h.controller.getSnapshot().contextPreparation, { phase: "provider_acknowledged" });
  await h.clock.advance(VOICE_SILENCE_TIMEOUT_MS - 800_000);
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.match(h.controller.getSnapshot().notice!, /15 minutes of silence/);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/activity")).length, 0);
  assert.equal(h.clock.timers.size, 0);
});

test.each([null, "provider_acknowledged"])("observer close during context publication %j cannot leave polling behind", async (phase) => {
  const h = harness();
  let closing: Promise<void> | undefined;
  h.controller.subscribe(() => {
    const snapshot = h.controller.getSnapshot();
    if (snapshot.phase === "active" && (phase === null
      ? snapshot.contextPreparation === null : snapshot.contextPreparation?.phase === phase)) {
      closing = h.controller.close();
    }
  });
  await h.controller.start(target);
  await flush();
  await closing;
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.equal(h.clock.timers.size, 0);
  assert.equal(h.streams[0].tracks[0].stopped, true);
});

test("strict handshake installs output before offer, gates media until typed authority, and hides credentials", async () => {
  const h = harness();
  const status = deferred<unknown>();
  h.setRpc((method) => method === "meta/live/status" ? status.promise : undefined);
  const start = h.controller.start(target);
  assert.equal(h.contexts[0].resumed, true, "AudioContext resumed synchronously in click gesture");
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  assert.equal(h.streams[0].tracks[0].enabled, false);
  assert.equal(h.contexts[0].gains[0].gain.value, 0);
  const statusCall = h.calls.find((call) => call.method === "meta/live/status")!;
  status.resolve({
    phase: "active", handle: {
      channel_id: statusCall.params.channel_id, target_identity: target.identity,
      execution_mode: "client_context", activation_receipt: "activation-secret",
    },
  });
  await start;
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.streams[0].tracks[0].enabled, true);
  assert.equal(h.contexts[0].gains[0].gain.value, 1);
  assert.deepEqual(h.calls.map((call) => call.method), [
    "meta/console/voice/open", "meta/live/playback_owner/register",
    "live/webrtc/answer", "meta/console/voice/answer_received", "meta/live/status",
    "meta/console/voice/context_status", "meta/console/voice/captions",
  ]);
  assert.doesNotMatch(JSON.stringify(h.controller.getSnapshot()), /secret|receipt|opaque-bootstrap|sdp/);
  await h.controller.close();
  assert.equal(h.clock.timers.size, 0);
});

test("pending-handle activation preserves target and mute choices without opening another channel", async () => {
  const h = harness();
  const status = deferred<unknown>();
  h.setRpc((method) => method === "meta/live/status" ? status.promise : undefined);
  const start = h.controller.start(target);
  await flush();
  h.controller.toggleMicrophone();
  h.controller.toggleSpeaker();
  status.resolve({
    phase: "active",
    handle: {
      channel_id: `channel-${target.identity}`, target_identity: target.identity,
      execution_mode: "client_context", activation_receipt: "active-receipt",
    },
  });
  await start;
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.deepEqual(h.controller.getSnapshot().target, target);
  assert.equal(h.controller.getSnapshot().microphoneMuted, true);
  assert.equal(h.controller.getSnapshot().speakerMuted, true);
  assert.equal(h.streams[0].tracks[0].enabled, false);
  assert.equal(h.contexts[0].gains[0].gain.value, 0);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
  await h.controller.close();
});

test("stable snapshot and voice target survive independent background text", async () => {
  const h = harness();
  h.setRpc((method) => method === "meta/console/send" ? Promise.resolve({ accepted: true }) : undefined);
  let changes = 0;
  const unsubscribe = h.controller.subscribe(() => { changes++; });
  await h.controller.start(target);
  const snapshot = h.controller.getSnapshot();
  const before = changes;
  await h.env.rpc("meta/console/send", { identity: other.identity, message: "Independent text" }, 5000);
  assert.equal(h.controller.getSnapshot(), snapshot);
  assert.equal(changes, before);
  assert.deepEqual(snapshot.target, target);
  unsubscribe();
  await h.controller.close();
  assert.equal(changes, before);
});

/**
 * Script the gateway's caption long poll: each read waits until the test pushes a
 * batch, and every read's params are kept so the cursor can be asserted.
 */
function captionFeed(h: ReturnType<typeof harness>) {
  const reads: { params: Record<string, unknown>; answer: ReturnType<typeof deferred<unknown>> }[] = [];
  let cursor = 0;
  h.setRpc((method, params) => {
    if (method !== VOICE_CAPTIONS_METHOD) return undefined;
    const answer = deferred<unknown>();
    reads.push({ params, answer });
    return answer.promise;
  });
  return {
    reads,
    async push(captions: unknown[]) {
      await flush();
      const read = reads.at(-1);
      assert.ok(read, "a caption read must be waiting");
      cursor += captions.length;
      read.answer.resolve({
        identity: read.params.identity, request_id: read.params.request_id, channel_id: read.params.channel_id,
        cursor, captions,
      });
      await h.clock.advance(0);
    },
  };
}

test("the caption observer reads the exact owned channel scope and advances its cursor", async () => {
  const h = harness();
  const feed = captionFeed(h);
  await h.controller.start(target);
  await flush();
  const channelId = h.controller.getSnapshot().activeChannelId;
  assert.deepEqual(feed.reads[0]?.params, {
    identity: target.identity, request_id: "request-1", channel_id: channelId, after: 0, wait_ms: VOICE_CAPTION_WAIT_MS,
  });
  await feed.push([{ kind: "caption", item_id: "segment-1", text: "The vault" }]);
  assert.equal(feed.reads[1]?.params.after, 1, "the next read starts after the applied batch");
  await h.controller.close();
});

test("assistant captions upsert live speech by item id with the segment's whole text", async () => {
  const h = harness();
  const feed = captionFeed(h);
  await h.controller.start(target);
  assert.deepEqual(h.controller.getSnapshot().liveSpeech, []);
  const items = () => h.controller.getSnapshot().liveSpeech.map((item) => [item.itemId, item.speaker, item.text, item.final]);
  await feed.push([{ kind: "caption", item_id: "segment-1", text: "The vault" }]);
  await feed.push([
    { kind: "caption", item_id: "segment-1", text: "The vault phrase is" },
    { kind: "caption", item_id: "segment-2", text: "Next" },
  ]);
  assert.deepEqual(items(), [
    ["segment-1", "assistant", "The vault phrase is", false],
    ["segment-2", "assistant", "Next", false],
  ]);
  // A repeated or older caption carries whole text; it replaces, never appends.
  await feed.push([{ kind: "caption", item_id: "segment-1", text: "The vault phrase is amber." }]);
  await feed.push([{ kind: "caption", item_id: "segment-1", text: "The vault phrase is amber." }]);
  assert.deepEqual(items(), [
    ["segment-1", "assistant", "The vault phrase is amber.", false],
    ["segment-2", "assistant", "Next", false],
  ]);
  // A retraction drops exactly its item.
  await feed.push([{ kind: "retracted", item_id: "segment-2" }]);
  assert.deepEqual(items(), [["segment-1", "assistant", "The vault phrase is amber.", false]]);
  await h.controller.close();
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.deepEqual(h.controller.getSnapshot().liveSpeech, [], "a normal close drops every provisional row");
  assert.equal(h.controller.getSnapshot().activeChannelId, null);
});

test("barge-in playback hints duck and restore the gate between the speaker analyser and the master gain", async () => {
  const h = harness();
  const feed = captionFeed(h);
  await h.controller.start(target);
  await flush();
  const context = h.contexts[0];
  const [master, gate] = context.gains;
  assert.ok(gate, "the playback gate is its own node");
  assert.ok(gate.connections.includes(master), "gate feeds the master gain (mute and speaker state stay there)");
  assert.equal(gate.gain.value, 1, "assistant audio plays at unity until a hint arrives");
  await feed.push([{ kind: "playback_hint", hint: "duck" }]);
  assert.deepEqual(gate.targets.at(-1), [0, 0, 0.01], "duck ramps the gate to silence");
  assert.equal(master.gain.value, 1, "the master gain (speaker on) is untouched");
  await feed.push([
    { kind: "caption", item_id: "segment-1", text: "Here is the" },
    { kind: "playback_hint", hint: "restore" },
  ]);
  assert.deepEqual(gate.targets.at(-1), [1, 0, 0.01], "restore ramps back to unity");
  assert.deepEqual(
    h.controller.getSnapshot().liveSpeech.map((item) => item.text), ["Here is the"],
    "a hint never becomes a caption row",
  );
  await h.controller.close();
});

test("data-channel output transcripts never create assistant captions; item-keyed user deltas still do", async () => {
  const h = harness();
  await h.controller.start(target);
  assert.equal(h.controller.getSnapshot().phase, "active");
  const channel = h.peers[0].channel;
  channel.emit({ type: "session.input_transcript.delta", item_id: "u1", delta: "What is " });
  channel.emit({ type: "session.input_transcript.delta", item_id: "u1", delta: "the vault phrase" });
  channel.emit({ type: "session.output_transcript.delta", item_id: "a1", delta: "The vault " });
  channel.emit({ type: "session.output_transcript.delta", delta: "phrase is amber" });
  channel.emit({ type: "session.output_transcript.done", item_id: "a1", text: "The vault phrase is amber." });
  assert.deepEqual(h.controller.getSnapshot().liveSpeech.map((item) => [item.itemId, item.speaker, item.text, item.final]), [
    ["u1", "user", "What is the vault phrase", false],
  ]);
  channel.emit({ type: "session.input_transcript.done", item_id: "u1", text: "What is the vault phrase?" });
  assert.deepEqual(h.controller.getSnapshot().liveSpeech.map((item) => [item.speaker, item.text, item.final]), [
    ["user", "What is the vault phrase?", true],
  ]);
  // Deltas without an item id or a string payload are ignored, never crash.
  channel.emit({ type: "session.input_transcript.delta", delta: "orphan" });
  channel.emit({ type: "session.input_transcript.delta", item_id: "u2", delta: 42 });
  assert.equal(h.controller.getSnapshot().liveSpeech.length, 1);
  await h.controller.close();
});

test("a user item and an assistant caption with the same item id stay separate", async () => {
  const h = harness();
  const feed = captionFeed(h);
  await h.controller.start(target);
  try {
    const channel = h.peers[0].channel;
    channel.emit({ type: "session.input_transcript.delta", item_id: "shared-item", delta: "User " });
    await feed.push([{ kind: "caption", item_id: "shared-item", text: "Assistant answer" }]);
    channel.emit({ type: "session.input_transcript.delta", item_id: "shared-item", delta: "question" });
    assert.deepEqual(h.controller.getSnapshot().liveSpeech.map((item) => [item.itemId, item.speaker, item.text, item.final]), [
      ["shared-item", "user", "User question", false],
      ["shared-item", "assistant", "Assistant answer", false],
    ]);
    await feed.push([{ kind: "retracted", item_id: "shared-item" }]);
    channel.emit({ type: "session.input_transcript.done", item_id: "shared-item", text: "User question?" });
    assert.deepEqual(h.controller.getSnapshot().liveSpeech.map((item) => [item.speaker, item.text, item.final]), [
      ["user", "User question?", true],
    ], "a retraction and a user final each touch only their own speaker");
  } finally {
    await h.controller.close();
  }
});

test("a replacement channel drops old captions and reads the new channel from the start", async () => {
  const h = harness();
  const feed = captionFeed(h);
  await h.controller.start(target);
  await feed.push([{ kind: "caption", item_id: "old-segment", text: "Old channel speech" }]);
  assert.equal(h.controller.getSnapshot().liveSpeech.length, 1);
  let required = true;
  h.setRpc((method, params) => {
    if (method === VOICE_CAPTIONS_METHOD) {
      const answer = deferred<unknown>();
      feed.reads.push({ params, answer });
      return answer.promise;
    }
    if (method !== "meta/console/voice/replacement") return undefined;
    const result = required ? replacement() : { required: false };
    required = false;
    return Promise.resolve(result);
  });
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.controller.getSnapshot().activeChannelId, "recovery-channel");
  assert.deepEqual(h.controller.getSnapshot().liveSpeech, [], "a replacement channel's item ids start over");
  const latest = feed.reads.at(-1)!;
  assert.equal(latest.params.channel_id, "recovery-channel");
  assert.equal(latest.params.after, 0);
  // A late answer to the old channel's read is fenced.
  const stale = feed.reads.find((read) => read.params.channel_id !== "recovery-channel" && read.params.after === 1)!;
  stale.answer.resolve({ identity: stale.params.identity, request_id: stale.params.request_id,
    channel_id: stale.params.channel_id, cursor: 9, captions: [{ kind: "caption", item_id: "late", text: "Late" }] });
  await flush();
  assert.deepEqual(h.controller.getSnapshot().liveSpeech, []);
  await feed.push([{ kind: "caption", item_id: "new-segment", text: "New channel speech" }]);
  assert.deepEqual(h.controller.getSnapshot().liveSpeech.map((item) => item.itemId), ["new-segment"]);
  await h.controller.close();
  assert.equal(h.clock.timers.size, 0);
});

test("a failed caption read never affects the call and is retried; a closed call stops reading", async () => {
  const h = harness();
  let reads = 0;
  h.setRpc((method, params) => {
    if (method !== VOICE_CAPTIONS_METHOD) return undefined;
    reads += 1;
    if (reads === 1) return Promise.reject(new Error("network down"));
    if (reads === 2) {
      return Promise.resolve({ identity: params.identity, request_id: params.request_id, channel_id: params.channel_id,
        cursor: 4, captions: [{ kind: "caption", item_id: "segment-1", text: "Recovered" }] });
    }
    if (reads === 3) return Promise.reject(Object.assign(new Error("closed"), { rpcError: { code: -32000, data: { kind: "voice_closed" } } }));
    return new Promise(() => {});
  });
  await h.controller.start(target);
  await flush();
  assert.equal(reads, 1);
  assert.equal(h.controller.getSnapshot().phase, "active", "a caption failure is display state only");
  await h.clock.advance(VOICE_CAPTION_RETRY_DELAY_MS);
  assert.equal(reads, 3);
  assert.deepEqual(h.controller.getSnapshot().liveSpeech.map((item) => item.text), ["Recovered"]);
  await h.clock.advance(60_000);
  assert.equal(reads, 3, "voice_closed ends the caption observer; the control plane reports closure");
  assert.equal(h.controller.getSnapshot().phase, "active");
  await h.controller.close();
});

test("a caption batch for another call scope is rejected, never applied", async () => {
  const h = harness();
  h.setRpc((method, params) => method === VOICE_CAPTIONS_METHOD
    ? Promise.resolve({ identity: params.identity, request_id: "other-request", channel_id: params.channel_id,
      cursor: 1, captions: [{ kind: "caption", item_id: "segment-1", text: "Foreign" }] })
    : undefined);
  await h.controller.start(target);
  await flush();
  assert.deepEqual(h.controller.getSnapshot().liveSpeech, []);
  await h.controller.close();
});

test("the caption observer and parser match the shared Rust captions contract", async () => {
  assert.equal(VOICE_CAPTIONS_METHOD, voiceContract.captions_method);
  const scope = { identity: voiceContract.captions_response.identity, requestId: voiceContract.captions_response.request_id,
    channelId: voiceContract.captions_response.channel_id };
  assert.deepEqual(parseVoiceCaptions(voiceContract.captions_response, scope, voiceContract.captions_request.after), {
    cursor: 7,
    captions: [
      { kind: "caption", itemId: "segment-a", text: "The vault phrase is amber." },
      { kind: "retracted", itemId: "segment-b" },
      { kind: "playback_hint", hint: "duck" },
    ],
  });
  const h = harness();
  const request = voiceContract.captions_request;
  h.env.randomId = () => request.request_id;
  const reads: Record<string, unknown>[] = [];
  let answered = false;
  h.setRpc((method, params) => {
    if (method === "meta/console/voice/open") return Promise.resolve(pending(request.identity, request.channel_id));
    if (method !== VOICE_CAPTIONS_METHOD) return undefined;
    reads.push(params);
    if (answered) return new Promise(() => {});
    answered = true;
    return Promise.resolve({ ...voiceContract.captions_response, cursor: request.after });
  });
  await h.controller.start({ identity: request.identity, label: "Golden agent" });
  await h.clock.advance(1_000);
  assert.deepEqual(Object.keys(reads[0]).sort(), Object.keys(request).sort());
  assert.deepEqual(reads[1], request, "the second read carries the cursor the first returned");
  await h.controller.close();
});

test("caption parsing accepts only the exact typed shape", () => {
  const scope = { identity: "agent", requestId: "request", channelId: "channel" };
  const wire = (extra: Record<string, unknown> = {}) => ({
    identity: "agent", request_id: "request", channel_id: "channel", cursor: 3,
    captions: [{ kind: "caption", item_id: "a", text: "Hi" }, { kind: "retracted", item_id: "b" }], ...extra,
  });
  assert.deepEqual(parseVoiceCaptions(wire(), scope, 0), {
    cursor: 3,
    captions: [{ kind: "caption", itemId: "a", text: "Hi" }, { kind: "retracted", itemId: "b" }],
  });
  for (const bad of [
    wire({ cursor: 2.5 }), wire({ cursor: -1 }), wire({ extra: true }), wire({ captions: {} }),
    wire({ captions: [{ kind: "caption", item_id: "", text: "Hi" }] }),
    wire({ captions: [{ kind: "caption", item_id: "a" }] }),
    wire({ captions: [{ kind: "caption", item_id: "a", text: "Hi", delta: "Hi" }] }),
    wire({ captions: [{ kind: "retracted", item_id: "b", text: "x" }] }),
    wire({ captions: [{ kind: "final", item_id: "a" }] }),
    wire({ captions: [{ kind: "playback_hint", hint: "louder" }] }),
    wire({ captions: [{ kind: "playback_hint" }] }),
    wire({ captions: [{ kind: "playback_hint", hint: "duck", item_id: "a" }] }),
  ]) assert.throws(() => parseVoiceCaptions(bad, scope, 0), JSON.stringify(bad));
  assert.deepEqual(
    parseVoiceCaptions(wire({ captions: [{ kind: "playback_hint", hint: "restore" }] }), scope, 0).captions,
    [{ kind: "playback_hint", hint: "restore" }],
  );
  assert.throws(() => parseVoiceCaptions(wire(), scope, 4), "a cursor never moves backwards");
  assert.throws(() => parseVoiceCaptions(wire({ channel_id: "old" }), scope, 0));
});

test("provisional live speech is dropped when the call is superseded or the provider is lost", async () => {
  const h = harness();
  await h.controller.start(target);
  h.peers[0].channel.emit({ type: "session.input_transcript.delta", item_id: "u1", delta: "hello" });
  assert.equal(h.controller.getSnapshot().liveSpeech.length, 1);
  const error = Object.assign(new Error("internal detail"), { rpcError: voiceContract.superseded_error });
  h.setRpc((method) => method === "meta/console/voice/replacement" ? Promise.reject(error) : undefined);
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.deepEqual(h.controller.getSnapshot().liveSpeech, [], "supersession drops every provisional row");

  const g = harness();
  const feed = captionFeed(g);
  await g.controller.start(target);
  await feed.push([{ kind: "caption", item_id: "a1", text: "partial" }]);
  assert.equal(g.controller.getSnapshot().liveSpeech.length, 1);
  g.peers[0].channel.emit({ type: "error" });
  await g.clock.advance(1);
  assert.notEqual(g.controller.getSnapshot().phase, "active");
  assert.deepEqual(g.controller.getSnapshot().liveSpeech, [], "a provider error drops every provisional row");
});

test("microphone and speaker mute independently, output waveform and activity continue while speaker-muted", async () => {
  const h = harness();
  await h.controller.start(target);
  const context = h.contexts[0];
  context.analysers[0].signal = 0.25;
  context.analysers[1].signal = 0.5;
  h.controller.toggleMicrophone();
  assert.equal(h.streams[0].tracks[0].enabled, false);
  assert.equal(context.gains[0].gain.value, 1);
  h.controller.toggleSpeaker();
  assert.equal(context.gains[0].gain.value, 0);
  assert.equal(h.peers[0].remoteTrack.enabled, true);
  const samples = new Float32Array(32);
  h.controller.sampleWaveform("speaker", samples);
  assert.equal(samples[0], 0.5);
  h.controller.sampleWaveform("microphone", samples);
  assert.equal(samples[0], 0);
  await h.clock.advance(VOICE_SILENCE_TIMEOUT_MS + 1000);
  assert.equal(h.controller.getSnapshot().phase, "active", "model sound refreshes silence even when muted");
  h.controller.toggleMicrophone();
  assert.equal(h.streams[0].tracks[0].enabled, true);
  h.controller.toggleSpeaker();
  assert.equal(context.gains[0].gain.value, 1);
  await h.controller.close();
});

test("closes at exactly 900000ms of silence despite silent PCM and keepalives", async () => {
  const h = harness();
  await h.controller.start(target);
  await h.clock.advance(VOICE_SILENCE_TIMEOUT_MS - 1);
  h.peers[0].channel.emit({ type: "session.updated" });
  h.peers[0].channel.emit({ type: "response.output_audio.delta", delta: "AAAA" });
  h.peers[0].channel.emit({ type: "response.audio_transcript.delta", delta: "   " });
  assert.equal(h.controller.getSnapshot().phase, "active");
  await h.clock.advance(1);
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.match(h.controller.getSnapshot().notice!, /15 minutes of silence/);
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.peers[0].connectionState, "closed");
  assert.equal(h.contexts[0].state, "closed");
  assert.equal(h.clock.timers.size, 0);
});

test("independent text to either agent and model text or transcripts never refresh audio silence", async () => {
  const h = harness();
  h.setRpc((method) => method === "meta/console/send" ? Promise.resolve({ accepted: true }) : undefined);
  await h.controller.start(target);
  await h.clock.advance(800_000);
  for (const identity of [target.identity, other.identity]) {
    const result = await h.env.rpc("meta/console/send", { identity, message: "Still independently texting" }, 5000);
    assert.deepEqual(result, { accepted: true });
  }
  for (const type of [
    "response.text.delta", "response.output_text.delta",
    "response.audio_transcript.delta", "response.output_audio_transcript.delta",
  ]) h.peers[0].channel.emit({ type, delta: "Text without current audio" });
  h.peers[0].channel.emit({
    type: "conversation.item.input_audio_transcription.completed",
    transcript: "Delayed transcription does not mean current speech",
  });
  await h.clock.advance(VOICE_SILENCE_TIMEOUT_MS - 800_000 - 1);
  assert.equal(h.controller.getSnapshot().phase, "active");
  await h.clock.advance(1);
  assert.equal(h.controller.getSnapshot().phase, "idle");
});

test("authoritative microphone speech events refresh silence only for an unmuted microphone", async () => {
  const h = harness();
  await h.controller.start(target);
  await h.clock.advance(800_000);
  h.peers[0].channel.emit({ type: "input_audio_buffer.speech_started" });
  await h.clock.advance(800_000);
  assert.equal(h.controller.getSnapshot().phase, "active");
  h.peers[0].channel.emit({ type: "input_audio_buffer.speech_stopped" });
  h.controller.toggleMicrophone();
  await h.clock.advance(VOICE_SILENCE_TIMEOUT_MS - 1);
  h.peers[0].channel.emit({ type: "input_audio_buffer.speech_started" });
  assert.equal(h.controller.getSnapshot().phase, "active");
  await h.clock.advance(1);
  assert.equal(h.controller.getSnapshot().phase, "idle");
});

test("actual microphone activity refreshes timeout; muted input does not", async () => {
  const h = harness();
  await h.controller.start(target);
  h.contexts[0].analysers[0].signal = 0.2;
  await h.clock.advance(1000);
  h.controller.toggleMicrophone();
  await h.clock.advance(VOICE_SILENCE_TIMEOUT_MS - 1);
  h.peers[0].channel.emit({ type: "input_audio_buffer.speech_started" });
  assert.equal(h.controller.getSnapshot().phase, "active");
  await h.clock.advance(1);
  assert.equal(h.controller.getSnapshot().phase, "idle");
});

test("replacement mutes immediately and drains the old transport before opening the next agent", async () => {
  const h = harness();
  await h.controller.start(target);
  const closing = deferred<unknown>();
  h.setRpc((method) => method === "meta/console/voice/close" ? closing.promise : undefined);
  const replacement = h.controller.start(other);
  assert.equal(h.streams[0].tracks[0].enabled, false);
  assert.equal(h.contexts[0].gains[0].gain.value, 0);
  assert.equal(h.streams[0].tracks[0].stopped, false);
  assert.equal(h.peers[0].connectionState, "connected");
  await flush();
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
  closing.resolve({ phase: "closed" });
  await replacement;
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.peers[0].connectionState, "closed");
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.controller.getSnapshot().target?.identity, other.identity);
  const closeIndex = h.calls.findIndex((call) => call.method.endsWith("/close"));
  const nextIndex = h.calls.findIndex((call) => call.method.endsWith("/open") && call.params.identity === other.identity);
  assert.ok(closeIndex < nextIndex);
  await h.controller.close();
});

test("fast start/start/close never opens stale requests and late microphone grants stop tracks", async () => {
  const h = harness();
  const media = deferred<Stream>();
  h.setMedia(() => media.promise);
  const first = h.controller.start(target);
  await flush();
  const second = h.controller.start(other);
  await flush();
  const close = h.controller.close();
  await Promise.all([first, second, close]);
  assert.equal(h.controller.getSnapshot().phase, "idle");
  const stream = new Stream();
  media.resolve(stream);
  await flush();
  assert.equal(stream.tracks[0].stopped, true);
  assert.equal(stream.tracks[0].enabled, false);
  assert.equal(h.calls.length, 0);
  assert.ok(h.contexts.every((context) => context.state === "closed"));
});

test("close fences an in-flight open, and a late response cannot activate or overwrite a replacement", async () => {
  const h = harness();
  const opened = deferred<unknown>();
  h.setRpc((method, params) =>
    method.endsWith("/open") && params.identity === target.identity ? opened.promise : undefined);
  const first = h.controller.start(target);
  await flush();
  assert.equal(h.calls[0].method, "meta/console/voice/open");
  const second = h.controller.start(other);
  await Promise.all([first, second]);
  assert.equal(h.controller.getSnapshot().target?.identity, other.identity);
  assert.equal(h.controller.getSnapshot().phase, "active");
  const close = h.calls.find((call) => call.method.endsWith("/close"))!;
  assert.equal(close.params.request_id, h.calls[0].params.request_id);
  opened.resolve(pending());
  await flush();
  assert.equal(h.controller.getSnapshot().target?.identity, other.identity);
  assert.equal(h.peers.length, 1);
  await h.controller.close();
});

test("pending activation does not enable media and has a bounded deadline", async () => {
  const h = harness();
  h.setRpc((method) => method === "meta/live/status" ? Promise.resolve({ phase: "pending" }) : undefined);
  const start = h.controller.start(target);
  await flush();
  h.controller.toggleMicrophone();
  h.controller.toggleMicrophone();
  assert.equal(h.streams[0].tracks[0].enabled, false);
  await h.clock.advance(VOICE_CONNECT_TIMEOUT_MS);
  await start;
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.match(h.controller.getSnapshot().error!, /timed out/);
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.contexts[0].state, "closed");
  assert.equal(h.clock.timers.size, 0);
});

test("open admission is distinct from media connection and its timeout does not blame microphone permission", async () => {
  const h = harness();
  h.setRpc(method => method === "meta/console/voice/open" ? new Promise(() => {}) : undefined);
  const start = h.controller.start(target);
  await flush();
  assert.equal(h.controller.getSnapshot().connectionStage, "opening");
  assert.equal(h.peers.length, 0);
  await h.clock.advance(VOICE_CONNECT_TIMEOUT_MS);
  await start;
  assert.match(h.controller.getSnapshot().error!, /Starting voice timed out/);
  assert.doesNotMatch(h.controller.getSnapshot().error!, /microphone permissions|your network/);
  assert.equal(h.streams[0].tracks[0].stopped, true);
});

test("open failures identify gateway admission without exposing its upstream response", async () => {
  const h = harness();
  h.setRpc(method => method === "meta/console/voice/open"
    ? Promise.reject(new Error("admission rejected: private-source-data"))
    : undefined);
  await h.controller.start(target);
  assert.match(h.controller.getSnapshot().error!, /could not start voice/);
  assert.doesNotMatch(h.controller.getSnapshot().error!, /private-source-data/);
});

test("permission denial, permission timeout, and late permission grants clean up without opening remote voice", async () => {
  const denied = harness();
  denied.setMedia(() => Promise.reject(Object.assign(new Error("denied"), { name: "NotAllowedError" })));
  await denied.controller.start(target);
  assert.equal(denied.controller.getSnapshot().phase, "error");
  assert.match(denied.controller.getSnapshot().error!, /permission was denied/);
  assert.equal(denied.calls.length, 0);
  assert.equal(denied.contexts[0].state, "closed");
  const h = harness();
  const media = deferred<Stream>();
  h.setMedia(() => media.promise);
  const start = h.controller.start(target);
  await flush();
  await h.clock.advance(VOICE_CONNECT_TIMEOUT_MS);
  await start;
  const stream = new Stream();
  media.resolve(stream);
  await flush();
  assert.equal(stream.tracks[0].stopped, true);
  assert.equal(h.calls.length, 0);
});

test("close keeps muted WebRTC alive until the gateway confirms finalization", async () => {
  const h = harness();
  await h.controller.start(target);
  const closing = deferred<unknown>();
  h.setRpc((method) => method.endsWith("/close") ? closing.promise : undefined);
  const close = h.controller.close();
  assert.equal(h.streams[0].tracks[0].enabled, false);
  assert.equal(h.contexts[0].gains[0].gain.value, 0);
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "closing");
  assert.equal(h.peers[0].connectionState, "connected");
  assert.equal(h.peers[0].channel.readyState, "open");
  assert.equal(h.streams[0].tracks[0].stopped, false);
  assert.equal(h.contexts[0].state, "running");
  closing.resolve({ phase: "closed" });
  await close;
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.equal(h.peers[0].connectionState, "closed");
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.contexts[0].state, "closed");
});

test.each(["dispose", "pagehide"])("%s immediately releases an older draining attempt during agent replacement", async (operation) => {
  const h = harness();
  await h.controller.start(target);
  const closing = deferred<unknown>();
  h.setRpc((method) => method.endsWith("/close") ? closing.promise : undefined);
  const replacement = h.controller.start(other);
  await flush();
  assert.equal(h.peers[0].connectionState, "connected");
  assert.equal(h.streams[0].tracks[0].stopped, false);
  if (operation === "pagehide") h.pagehide();
  else h.controller.dispose();
  assert.equal(h.peers[0].connectionState, "closed");
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.ok(h.contexts.every(context => context.state === "closed"));
  if (operation === "pagehide") {
    assert.deepEqual(h.pagehideCalls, [{ identity: target.identity, request_id: "request-1" }]);
  }
  closing.resolve({ phase: "closed" });
  await replacement;
  await flush();
  assert.ok(h.streams.every(stream => stream.tracks.every(track => track.stopped)));
  assert.equal(h.calls.filter(call => call.method.endsWith("/open")).length, 1);
});

test("unconfirmed teardown releases muted WebRTC at the first deadline, retries on the backoff schedule, and remains retryable", async () => {
  const h = harness();
  await h.controller.start(target);
  h.setRpc((method) => method.endsWith("/close") ? new Promise(() => {}) : undefined);
  const closeCalls = () => h.calls.filter((call) => call.method.endsWith("/close")).length;
  const close = h.controller.close();
  await flush();
  await h.clock.advance(VOICE_TEARDOWN_TIMEOUT_MS - 1);
  assert.equal(h.streams[0].tracks[0].enabled, false);
  assert.equal(h.contexts[0].gains[0].gain.value, 0);
  assert.equal(h.peers[0].connectionState, "connected");
  await h.clock.advance(1);
  // Media is released at the first deadline; the gateway close keeps retrying with media off.
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.peers[0].connectionState, "closed");
  assert.equal(h.controller.getSnapshot().phase, "closing");
  assert.equal(closeCalls(), 1);
  for (const [index, delay] of VOICE_CLOSE_RETRY_DELAYS_MS.entries()) {
    await h.clock.advance(delay - 1);
    assert.equal(closeCalls(), index + 1, "no retry before its backoff elapses");
    await h.clock.advance(1);
    assert.equal(closeCalls(), index + 2, "one retry per backoff step");
    await h.clock.advance(VOICE_TEARDOWN_TIMEOUT_MS);
  }
  await close;
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.match(h.controller.getSnapshot().error!, /not confirmed voice closure after 4 attempts/);
  assert.equal(closeCalls(), VOICE_CLOSE_RETRY_DELAYS_MS.length + 1, "the retry schedule is capped");
  assert.equal(h.clock.timers.size, 0, "no retry timer outlives the capped schedule");
  await h.controller.start(other);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
  h.setRpc(undefined);
  await h.controller.close();
  await h.controller.start(other);
  assert.equal(h.controller.getSnapshot().phase, "active");
  await h.controller.close();
});

test("gateway-side close failures are retried on the backoff schedule; caller-side rejections fail at once", async () => {
  const annotated = (props: Record<string, unknown>) => Object.assign(new Error("gateway"), props);
  // A gateway fault (-32000) while the owner retires a dead remote resolves on a later attempt.
  let h = harness();
  await h.controller.start(target);
  let faults = 0;
  h.setRpc((method) => {
    if (!method.endsWith("/close") || faults >= 2) return undefined;
    faults++;
    return Promise.reject(annotated({ rpcError: { code: -32000, message: "experimental live channel binding failed" } }));
  });
  const close = h.controller.close();
  await flush();
  assert.equal(h.streams[0].tracks[0].stopped, true, "media is released after the first failed attempt");
  assert.equal(h.controller.getSnapshot().phase, "closing");
  await h.clock.advance(VOICE_CLOSE_RETRY_DELAYS_MS[0] + VOICE_CLOSE_RETRY_DELAYS_MS[1]);
  await close;
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.equal(h.calls.filter((call) => call.method.endsWith("/close")).length, 3);
  assert.equal(h.clock.timers.size, 0);
  await h.controller.start(other);
  assert.equal(h.controller.getSnapshot().phase, "active", "a confirmed close admits the next session");
  await h.controller.close();

  // A caller-side rejection is definite: one attempt, no schedule, no attempt count in the message.
  h = harness();
  await h.controller.start(target);
  h.setRpc((method) => method.endsWith("/close")
    ? Promise.reject(annotated({ rpcError: { code: -32602, message: "live/close requires exactly one phase receipt" } }))
    : undefined);
  await h.controller.close();
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.match(h.controller.getSnapshot().error!, /not confirmed voice closure/);
  assert.doesNotMatch(h.controller.getSnapshot().error!, /attempts/);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/close")).length, 1);
  assert.equal(h.clock.timers.size, 0);
});

test("RTC, data channel, input track, and remote track loss close local and remote voice", async () => {
  for (const lose of [
    (h: ReturnType<typeof harness>) => h.peers[0].fail(),
    (h: ReturnType<typeof harness>) => h.peers[0].channel.onclose?.(),
    (h: ReturnType<typeof harness>) => h.streams[0].tracks[0].end(),
    (h: ReturnType<typeof harness>) => h.peers[0].remoteTrack.end(),
  ]) {
    const h = harness();
    await h.controller.start(target);
    lose(h);
    assert.equal(h.streams[0].tracks[0].enabled, false);
    await h.clock.advance(VOICE_RECOVERY_TIMEOUT_MS);
    assert.equal(h.controller.getSnapshot().phase, "error");
    assert.equal(h.streams[0].tracks[0].stopped, true);
    assert.equal(h.contexts[0].state, "closed");
    assert.equal(h.calls.at(-1)?.method, "meta/console/voice/close");
    assert.equal(h.clock.timers.size, 0);
  }
});

test("ICE disconnected enters a reconnect grace, keeps the peer alive, and heals without transport loss", async () => {
  for (const [interrupt, restore] of [
    [(peer: Peer) => peer.disconnect(), (peer: Peer) => peer.reconnect()],
    [
      (peer: Peer) => { peer.iceConnectionState = "disconnected"; peer.oniceconnectionstatechange?.(); },
      (peer: Peer) => { peer.iceConnectionState = "completed"; peer.oniceconnectionstatechange?.(); },
    ],
  ] as const) {
    const h = harness();
    await h.controller.start(target);
    const peer = h.peers[0];
    interrupt(peer);
    assert.equal(h.controller.getSnapshot().phase, "active");
    assert.equal(h.controller.getSnapshot().reconnecting, true);
    assert.notEqual(peer.connectionState, "closed", "the peer is not destroyed during the grace");
    assert.equal(peer.remoteTrack.stopped, false);
    assert.equal(h.streams[0].tracks[0].enabled, true, "microphone keeps flowing during the grace");
    assert.equal(h.contexts[0].gains[0].gain.value, 1, "speaker keeps flowing during the grace");
    await h.clock.advance(VOICE_TRANSPORT_RECONNECT_GRACE_MS - 1);
    assert.equal(h.controller.getSnapshot().reconnecting, true);
    restore(peer);
    assert.equal(h.controller.getSnapshot().reconnecting, false);
    assert.equal(h.controller.getSnapshot().phase, "active");
    await h.clock.advance(VOICE_TRANSPORT_RECONNECT_GRACE_MS);
    assert.equal(h.controller.getSnapshot().phase, "active");
    assert.equal(h.peers.length, 1, "no recovery peer was opened");
    assert.equal(h.calls.filter((call) => call.method.endsWith("/close")).length, 0);
    await h.controller.close();
    assert.equal(h.clock.timers.size, 0);
  }
});

test("ICE disconnected that never heals becomes transport loss only when the grace expires", async () => {
  const h = harness();
  await h.controller.start(target);
  const peer = h.peers[0];
  // The owner only issues replacement authority once the browser has actually lost transport.
  h.setRpc((method) => method !== "meta/console/voice/replacement" ? undefined
    : Promise.resolve(h.clock.now >= VOICE_TRANSPORT_RECONNECT_GRACE_MS ? replacement() : { required: false }));
  peer.disconnect();
  await h.clock.advance(VOICE_TRANSPORT_RECONNECT_GRACE_MS - 1);
  peer.disconnect();
  assert.equal(h.controller.getSnapshot().phase, "active", "repeated disconnected events do not restart the grace");
  assert.equal(h.controller.getSnapshot().reconnecting, true);
  assert.equal(h.peers.length, 1);
  assert.equal(h.streams[0].tracks[0].enabled, true);
  await h.clock.advance(1);
  // Grace expiry is the transport loss: the old peer closes and owner recovery takes over.
  assert.equal(peer.connectionState, "closed");
  assert.equal(h.controller.getSnapshot().reconnecting, false);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.peers.length, 2);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
  await h.controller.close();
  assert.equal(h.clock.timers.size, 0);
});

test("ICE failed and closed are terminal immediately, without a reconnect grace", async () => {
  for (const lose of [
    (peer: Peer) => peer.fail(),
    (peer: Peer) => { peer.iceConnectionState = "failed"; peer.oniceconnectionstatechange?.(); },
    (peer: Peer) => { peer.connectionState = "closed"; peer.onconnectionstatechange?.(); },
  ]) {
    const h = harness();
    await h.controller.start(target);
    lose(h.peers[0]);
    assert.equal(h.controller.getSnapshot().phase, "connecting");
    assert.equal(h.controller.getSnapshot().connectionStage, "recovery");
    assert.equal(h.controller.getSnapshot().reconnecting, false);
    assert.equal(h.streams[0].tracks[0].enabled, false);
    assert.equal(h.peers[0].connectionState, "closed");
    await h.controller.close();
    assert.equal(h.clock.timers.size, 0);
  }
});

test("mismatched active authority fails closed without ungating media", async () => {
  const h = harness();
  h.setRpc((method) => method === "meta/live/status" ? Promise.resolve({
    phase: "active", handle: {
      channel_id: "wrong-channel", target_identity: other.identity,
      execution_mode: "client_context", activation_receipt: "wrong-receipt",
    },
  }) : undefined);
  await h.controller.start(target);
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.match(h.controller.getSnapshot().error!, /authority did not match/);
  assert.equal(h.streams[0].tracks[0].enabled, false);
  assert.equal(h.contexts[0].gains[0].gain.value, 0);
  assert.equal(h.calls.at(-1)?.method, "meta/console/voice/close");
});

test("pagehide uses keepalive teardown, dispose is idempotent, and no start is admitted while hidden", async () => {
  const h = harness();
  await h.controller.start(target);
  h.pagehide();
  h.controller.dispose();
  await flush();
  assert.equal(h.pagehideCalls.length, 1);
  assert.equal(h.pagehideCalls[0].request_id, "request-1");
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.clock.timers.size, 0);
  await h.controller.start(other);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
  assert.equal(h.controller.getSnapshot().phase, "idle");
});

test("pageshow after a bfcache restore re-admits an explicit start on the same controller", async () => {
  const h = harness();
  await h.controller.start(target);
  h.pagehide();
  await flush();
  assert.ok(h.hasPageshowListener(), "pagehide arms a one-shot pageshow listener");
  await h.controller.start(other);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1, "still hidden");
  h.pageshow();
  assert.equal(h.hasPageshowListener(), false, "the pageshow listener is released once used");
  await h.controller.start(other);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.controller.getSnapshot().target?.identity, other.identity);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 2);
  assert.equal(h.streams.length, 2, "the restored page captures a fresh microphone");
  assert.equal(h.streams[0].tracks[0].stopped, true, "the pre-hide microphone stays released");
  // The restored lifecycle is fully armed again: a second pagehide still tears down by keepalive.
  h.pagehide();
  await flush();
  assert.equal(h.pagehideCalls.length, 2);
  assert.equal(h.pagehideCalls[1].request_id, "request-2", "the hidden start never consumed a request id");
  assert.equal(h.clock.timers.size, 0);
});

test("server errors never surface raw credentials or transport data", async () => {
  const h = harness();
  h.setRpc((method) => method.endsWith("/open") ?
    Promise.reject(new Error("provider secret sk-real-secret token=credential SDP full-body")) : undefined);
  await h.controller.start(target);
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.doesNotMatch(h.controller.getSnapshot().error!, /sk-real|credential|full-body/);
  assert.match(h.controller.getSnapshot().error!, /could not start voice/);
});

test("every post-open handshake failure fences the remote request and stops media", async () => {
  for (const stage of [
    "meta/live/playback_owner/register", "live/webrtc/answer",
    "meta/console/voice/answer_received", "meta/live/status",
  ]) {
    const h = harness();
    h.setRpc((method) => method === stage ? Promise.reject(new Error("handshake failure")) : undefined);
    await h.controller.start(target);
    assert.equal(h.controller.getSnapshot().phase, "error", stage);
    assert.equal(h.streams[0].tracks[0].stopped, true, stage);
    assert.equal(h.peers[0].connectionState, "closed", stage);
    assert.equal(h.contexts[0].state, "closed", stage);
    assert.equal(h.calls.at(-1)?.method, "meta/console/voice/close", stage);
  }
});

test("an unconnected peer cannot activate merely because the server returns active authority", async () => {
  const h = harness();
  h.setPeer((peer) => {
    peer.setRemoteDescription = () => Promise.resolve();
  });
  const start = h.controller.start(target);
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  assert.equal(h.streams[0].tracks[0].enabled, false);
  await h.clock.advance(VOICE_CONNECT_TIMEOUT_MS);
  await start;
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.equal(h.streams[0].tracks[0].stopped, true);
});

test("remote output arriving before typed activation remains muted and does not count activity", async () => {
  const h = harness();
  const status = deferred<unknown>();
  h.setRpc((method) => method === "meta/live/status" ? status.promise : undefined);
  const start = h.controller.start(target);
  await flush();
  assert.equal(h.contexts[0].gains[0].gain.value, 0);
  h.controller.toggleSpeaker();
  h.controller.toggleSpeaker();
  h.peers[0].channel.emit({ type: "response.output_text.delta", delta: "Not yet active" });
  assert.equal(h.contexts[0].gains[0].gain.value, 0);
  const samples = new Float32Array(16);
  h.contexts[0].analysers[1].signal = 0.2;
  h.controller.sampleWaveform("speaker", samples);
  assert.equal(samples[0], 0);
  await h.controller.close();
  await start;
  status.reject(new Error("late rejection after close"));
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "idle");
});

test("failed replacement teardown blocks further opens until explicit retry closes the original request", async () => {
  const h = harness();
  await h.controller.start(target);
  h.setRpc((method) => method.endsWith("/close") ? Promise.reject(new Error("offline")) : undefined);
  const start = h.controller.start(other);
  // A network failure is retried on the full backoff schedule before the closure is reported unconfirmed.
  await h.clock.advance(VOICE_CLOSE_RETRY_DELAYS_MS.reduce((total, delay) => total + delay, 0));
  await start;
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.equal(h.calls.filter((call) => call.method.endsWith("/close")).length, VOICE_CLOSE_RETRY_DELAYS_MS.length + 1);
  assert.ok(h.streams.every((stream) => stream.tracks[0].stopped));
  await h.controller.start({ identity: "agent-c", label: "Agent C" });
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
  h.setRpc(undefined);
  await h.controller.close();
  assert.equal(h.calls.at(-1)?.params.request_id, "request-1");
  assert.equal(h.controller.getSnapshot().phase, "idle");
  await h.controller.start(other);
  assert.equal(h.controller.getSnapshot().phase, "active");
  await h.controller.close();
});

test("browser audio interruption is recoverable: resume is retried and running clears the bounded wait", async () => {
  const h = harness();
  await h.controller.start(target);
  const context = h.contexts[0];
  let resumes = 0;
  // Unlike the default fake, resume() does not flip the state by itself.
  context.resume = () => { resumes++; return Promise.resolve(); };
  context.state = "interrupted";
  context.onstatechange?.();
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(resumes, 1, "resume is attempted on the state change");
  assert.equal(h.streams[0].tracks[0].stopped, false);
  await h.clock.advance(VOICE_AUDIO_RESUME_TIMEOUT_MS - 1);
  assert.equal(h.controller.getSnapshot().phase, "active");
  // A user gesture (mute toggle) retries resume, and the context comes back.
  h.controller.toggleMicrophone();
  assert.equal(resumes, 2);
  context.state = "running";
  context.onstatechange?.();
  await h.clock.advance(VOICE_AUDIO_RESUME_TIMEOUT_MS);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.controller.getSnapshot().microphoneMuted, true);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/close")).length, 0);
  await h.controller.close();
  assert.equal(h.clock.timers.size, 0);
});

test("browser audio that stays suspended past the bounded wait closes media-owner authority", async () => {
  for (const state of ["suspended", "interrupted"]) {
    const h = harness();
    await h.controller.start(target);
    const context = h.contexts[0];
    context.resume = () => Promise.reject(new Error("resume blocked"));
    context.state = state;
    context.onstatechange?.();
    await flush();
    assert.equal(h.controller.getSnapshot().phase, "active", state);
    await h.clock.advance(VOICE_AUDIO_RESUME_TIMEOUT_MS - 1);
    context.onstatechange?.();
    assert.equal(h.controller.getSnapshot().phase, "active", `${state}: repeated notifications do not extend the wait`);
    await h.clock.advance(1);
    assert.equal(h.controller.getSnapshot().phase, "error", state);
    assert.match(h.controller.getSnapshot().error!, /audio was interrupted/);
    assert.equal(h.streams[0].tracks[0].stopped, true);
    assert.equal(h.calls.at(-1)?.method, "meta/console/voice/close");
    assert.equal(h.clock.timers.size, 0);
  }
});

test("malformed data and transport keepalives are ignored; provider errors are sanitized and close voice", async () => {
  const h = harness();
  await h.controller.start(target);
  h.peers[0].channel.onmessage?.({ data: "not-json" });
  h.peers[0].channel.onmessage?.({ data: new ArrayBuffer(4) });
  h.peers[0].channel.emit(null);
  h.peers[0].channel.emit({ type: "rate_limits.updated" });
  assert.equal(h.controller.getSnapshot().phase, "active");
  h.peers[0].channel.emit({ type: "error", error: { message: "sk-provider-secret" } });
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.doesNotMatch(h.controller.getSnapshot().error!, /sk-provider-secret/);
});

test("synchronous observer close during requesting or activation cannot leak resources or enable audio", async () => {
  for (const phase of ["requesting", "active"]) {
    const h = harness();
    let close: Promise<void> | undefined;
    h.controller.subscribe(() => {
      if (h.controller.getSnapshot().phase === phase) close = h.controller.close();
    });
    await h.controller.start(target);
    await close;
    assert.equal(h.controller.getSnapshot().phase, "idle");
    assert.ok(h.streams.every((stream) => stream.tracks[0].stopped));
    assert.ok(h.contexts.every((context) => context.state === "closed"));
    assert.equal(h.clock.timers.size, 0);
  }
});

test("unavailable, unauthenticated, missing, and failed gateway availability release the microphone and never open voice", async () => {
  // The microphone is requested during the click, alongside readiness. A negative or
  // failed readiness stops the granted tracks; no channel is opened and no RPC is made.
  const released = (h: ReturnType<typeof harness>) =>
    h.streams.length === 1 && h.streams[0].tracks.every((track) => track.stopped);
  for (const available of [false, undefined, null, "true", 1]) {
    const h = harness();
    h.env.voiceAvailable = async () => available as VoiceAvailability;
    await h.controller.start(target);
    await flush();
    assert.equal(h.controller.getSnapshot().phase, "error");
    assert.match(h.controller.getSnapshot().error!, /authenticate OpenAI/);
    assert.ok(released(h), "granted microphone tracks are stopped");
    assert.equal(h.peers.length, 0);
    assert.equal(h.calls.length, 0);
    assert.ok(h.contexts.every((context) => context.state === "closed"));
  }
  const h = harness();
  h.env.voiceAvailable = () => Promise.reject(new Error("offline"));
  await h.controller.start(target);
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.ok(released(h));
  assert.equal(h.calls.length, 0);
  // An unknown readiness (the poll failed) is not "unavailable": it names the network, not auth.
  const unknown = harness();
  unknown.env.voiceAvailable = () => Promise.resolve("unknown");
  await unknown.controller.start(target);
  await flush();
  assert.equal(unknown.controller.getSnapshot().phase, "error");
  assert.match(unknown.controller.getSnapshot().error!, /could not be checked/);
  assert.doesNotMatch(unknown.controller.getSnapshot().error!, /authenticate OpenAI/);
  assert.ok(released(unknown));
  assert.equal(unknown.calls.length, 0);
  // A microphone denial with positive readiness is reported as the microphone problem.
  const denied = harness();
  denied.setMedia(() => Promise.reject(Object.assign(new Error("denied"), { name: "NotAllowedError" })));
  await denied.controller.start(target);
  assert.equal(denied.controller.getSnapshot().phase, "error");
  assert.match(denied.controller.getSnapshot().error!, /Microphone permission was denied/);
  assert.equal(denied.calls.length, 0);
});

test("the microphone is requested concurrently with readiness, and a fresh check gates every start", async () => {
  const h = harness();
  const availability = deferred<VoiceAvailability>();
  h.env.voiceAvailable = () => availability.promise;
  const start = h.controller.start(target);
  await flush();
  // Requested during the click, before the gateway answered; still gated (disabled).
  assert.equal(h.streams.length, 1);
  assert.ok(h.streams[0].tracks.every((track) => !track.enabled && !track.stopped));
  assert.equal(h.calls.length, 0, "no channel opens before readiness");
  await h.controller.close();
  availability.resolve("available");
  await start;
  await flush();
  assert.ok(h.streams[0].tracks.every((track) => track.stopped), "a closed attempt releases its late grant");
  assert.equal(h.calls.length, 0);
  h.env.voiceAvailable = () => Promise.resolve("available");
  await h.controller.start(target);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.streams.length, 2);
  assert.ok(h.streams[1].tracks.every((track) => track.enabled));
  await h.controller.close();
  const before = h.calls.length;
  h.env.voiceAvailable = () => Promise.resolve("unavailable");
  await h.controller.start(target);
  await flush();
  assert.equal(h.streams.length, 3, "readiness is queried freshly and the microphone requested again");
  assert.ok(h.streams[2].tracks.every((track) => track.stopped));
  assert.equal(h.calls.length, before);
});

test("activation waits for the transport by event, with one status poll and no 100 ms polling", async () => {
  const h = harness();
  let peer: Peer | undefined;
  h.setPeer((created) => {
    peer = created;
    // The answer applies, but the ICE connection and data channel settle later.
    created.setRemoteDescription = function (this: Peer) {
      this.ontrack?.({ track: this.remoteTrack });
      return Promise.resolve();
    };
  });
  const start = h.controller.start(target);
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  const statusPolls = () => h.calls.filter((call) => call.method === "meta/live/status").length;
  assert.equal(statusPolls(), 1, "the activation receipt was polled once");
  // Time passes without transport events: no further status polls are issued.
  await h.clock.advance(1_000);
  assert.equal(statusPolls(), 1);
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  assert.ok(h.streams[0].tracks.every((track) => !track.enabled), "microphone stays gated");
  // The peer connects but the data channel is not open yet: still connecting.
  peer!.connectionState = "connected";
  peer!.iceConnectionState = "connected";
  peer!.onconnectionstatechange?.();
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  // The data channel opens: activation completes from the event alone.
  peer!.channel.readyState = "open";
  (peer!.channel as unknown as { onopen?: () => void }).onopen?.();
  await start;
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(statusPolls(), 1);
  assert.ok(h.streams[0].tracks.every((track) => track.enabled), "microphone opens on activation");
  await h.controller.close();
});

test("a transport that never connects fails activation at the connect deadline without extra polls", async () => {
  const h = harness();
  h.setPeer((created) => {
    created.setRemoteDescription = function (this: Peer) {
      this.ontrack?.({ track: this.remoteTrack });
      return Promise.resolve();
    };
  });
  const start = h.controller.start(target);
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  await h.clock.advance(VOICE_CONNECT_TIMEOUT_MS);
  await start;
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.match(h.controller.getSnapshot().error!, /media connection timed out/);
  assert.equal(h.calls.filter((call) => call.method === "meta/live/status").length, 1);
  assert.ok(h.streams[0].tracks.every((track) => track.stopped));
});

test("StrictMode disposal can be followed by a fresh explicit start on the memoized controller", async () => {
  const h = harness();
  h.controller.dispose();
  let changes = 0;
  h.controller.subscribe(() => { changes++; });
  await h.controller.start(target);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.ok(changes > 0);
  h.controller.dispose();
  const oldStream = h.streams[0];
  await h.controller.start(other);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.controller.getSnapshot().target?.identity, other.identity);
  assert.equal(oldStream.tracks[0].stopped, true);
  assert.equal(h.peers[0].connectionState, "closed");
  await h.controller.close();
  assert.equal(h.clock.timers.size, 0);
});

test("availability helper queries exact target readiness, accepts only explicit true, and separates definite from unknown", async () => {
  const originalFetch = globalThis.fetch;
  try {
    for (const body of [
      {}, { available: false }, { available: "true" }, { available: true },
      { identity: target.identity, available: false },
      { identity: target.identity, available: "true" },
      { identity: other.identity, available: true },
      { identity: target.identity.toUpperCase(), available: true },
      { identity: target.identity, available: true },
    ]) {
      globalThis.fetch = (async (input, init) => {
        assert.equal(input, "https://gateway.example/console/rpc");
        const request = JSON.parse(String(init?.body));
        assert.equal(request.method, "meta/console/voice/readiness");
        assert.deepEqual(request.params, { identity: target.identity });
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: body }), { status: 200 });
      }) as typeof fetch;
      assert.equal(
        await queryVoiceAvailability("https://gateway.example", target.identity),
        body.identity === target.identity && body.available === true ? "available" : "unavailable",
      );
    }
    // The gateway answered: unauthorized, forbidden, and typed JSON-RPC rejections are definite.
    for (const status of [401, 403, 404]) {
      globalThis.fetch = (async () => new Response("denied", { status })) as typeof fetch;
      assert.equal(await queryVoiceAvailability("", target.identity), "unavailable", String(status));
    }
    globalThis.fetch = (async (_input, init) => {
      const request = JSON.parse(String(init?.body));
      return new Response(JSON.stringify({
        jsonrpc: "2.0", id: request.id, error: { code: -32601, message: "method not found" },
      }), { status: 200 });
    }) as typeof fetch;
    assert.equal(await queryVoiceAvailability("", target.identity), "unavailable");
    // The poll itself failed: nothing is known, so callers keep the last definite answer.
    for (const status of [500, 502, 503, 429]) {
      globalThis.fetch = (async () => new Response("busy", { status })) as typeof fetch;
      assert.equal(await queryVoiceAvailability("", target.identity), "unknown", String(status));
    }
    // The gateway's typed budget expiry is not an answer either: retry, keep what was known.
    globalThis.fetch = (async (_input, init) => {
      const request = JSON.parse(String(init?.body));
      return new Response(JSON.stringify({
        jsonrpc: "2.0", id: request.id, error: voiceContract.readiness_timed_out_error,
      }), { status: 200 });
    }) as typeof fetch;
    assert.equal(await queryVoiceAvailability("", target.identity), "unknown");
    globalThis.fetch = (async () => { throw new Error("offline"); }) as typeof fetch;
    assert.equal(await queryVoiceAvailability("", target.identity), "unknown");
    globalThis.fetch = (async () => { throw new TypeError("Failed to fetch"); }) as typeof fetch;
    assert.equal(await queryVoiceAvailability("", target.identity), "unknown");
    globalThis.fetch = (async () => { assert.fail("Missing identity must not query readiness"); }) as typeof fetch;
    assert.equal(await queryVoiceAvailability("", ""), "unavailable");
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("readiness helper accepts the shared Rust HTTP golden contract for both availability states", async () => {
  const originalFetch = globalThis.fetch;
  let calls = 0;
  try {
    for (const [result, expected] of [
      [voiceContract.readiness_available, "available"],
      [voiceContract.readiness_unavailable, "unavailable"],
      [voiceContract.readiness_external_live_active, "unavailable"],
    ] as const) {
      globalThis.fetch = (async (input, init) => {
        calls++;
        assert.equal(input, "https://gateway.example/console/rpc");
        const request = JSON.parse(String(init?.body));
        assert.equal(request.method, "meta/console/voice/readiness");
        assert.deepEqual(request.params, voiceContract.readiness_request);
        return new Response(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }));
      }) as typeof fetch;
      assert.equal(
        await queryVoiceAvailability("https://gateway.example", voiceContract.readiness_request.identity),
        expected,
      );
    }
    assert.equal(calls, 3);
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("readiness detail carries the typed holder when the external live channel owns the voice path", async () => {
  const originalFetch = globalThis.fetch;
  try {
    globalThis.fetch = (async (_input, init) => {
      const request = JSON.parse(String(init?.body));
      return new Response(JSON.stringify({
        jsonrpc: "2.0", id: request.id, result: voiceContract.readiness_external_live_active,
      }));
    }) as typeof fetch;
    const detail = await queryVoiceReadiness("https://gateway.example", voiceContract.readiness_request.identity);
    assert.deepEqual(detail, {
      availability: "unavailable",
      reason: "external_live_active",
      holder: {
        identity: voiceContract.readiness_external_live_active.holder.identity,
        channelId: voiceContract.readiness_external_live_active.holder.channel_id,
      },
    });
    globalThis.fetch = (async (_input, init) => {
      const request = JSON.parse(String(init?.body));
      return new Response(JSON.stringify({ jsonrpc: "2.0", id: request.id, result: voiceContract.readiness_unavailable }));
    }) as typeof fetch;
    assert.deepEqual(
      await queryVoiceReadiness("https://gateway.example", voiceContract.readiness_request.identity),
      { availability: "unavailable" },
    );
  } finally {
    globalThis.fetch = originalFetch;
  }
});

test("transient classification: network, timeout, 5xx and 429 retry; typed RPC errors and other HTTP rejections are definite", () => {
  const annotated = (props: Record<string, unknown>) => Object.assign(new Error("gateway"), props);
  assert.equal(isTransientRpcFailure(new Error("console rpc timeout after 5s")), true);
  assert.equal(isTransientRpcFailure(new TypeError("Failed to fetch")), true);
  for (const httpStatus of [500, 502, 503, 504, 429]) {
    assert.equal(isTransientRpcFailure(annotated({ httpStatus })), true, String(httpStatus));
  }
  for (const httpStatus of [400, 401, 403, 404, 409]) {
    assert.equal(isTransientRpcFailure(annotated({ httpStatus })), false, String(httpStatus));
  }
  assert.equal(isTransientRpcFailure(annotated({ rpcError: { code: -32030 } })), false);
  assert.equal(isTransientRpcFailure(annotated({ rpcError: { data: { kind: "voice_closed" } } })), false);
  assert.equal(isTransientRpcFailure(annotated({ rpcError: { code: -32603, message: "internal" } })), false,
    "a JSON-RPC internal error is an answer from the gateway, not a lost poll");
});

test("production browser environment constructs an empty MediaStream with a valid overload", async () => {
  const h = harness();
  const stream = new Stream();
  const controller = createVoiceSession("");
  const originalFetch = globalThis.fetch;
  const play = vi.spyOn(HTMLMediaElement.prototype, "play").mockResolvedValue();
  const pause = vi.spyOn(HTMLMediaElement.prototype, "pause").mockImplementation(() => {});
  class BrowserStream extends Stream {
    constructor(tracks?: Track[]) {
      assert.ok(arguments.length === 0 || Array.isArray(tracks), "MediaStream(undefined) is not a valid browser overload");
      super(tracks ?? []);
    }
  }
  vi.stubGlobal("AudioContext", Context);
  vi.stubGlobal("RTCPeerConnection", Peer);
  vi.stubGlobal("MediaStream", BrowserStream);
  vi.stubGlobal("navigator", { mediaDevices: { getUserMedia: () => Promise.resolve(stream) } });
  vi.stubGlobal("crypto", { randomUUID: () => "browser-request" });
  globalThis.fetch = (async (input, init) => {
    const request = JSON.parse(String(init?.body));
    const result = await h.env.rpc(request.method, request.params, 30_000);
    return new Response(JSON.stringify({ jsonrpc: "2.0", id: request.id, result }));
  }) as typeof fetch;
  try {
    await controller.start(target);
    assert.equal(controller.getSnapshot().phase, "active", controller.getSnapshot().error ?? "");
    await controller.close();
    assert.equal(stream.tracks[0].stopped, true);
  } finally {
    await controller.close();
    controller.dispose();
    globalThis.fetch = originalFetch;
    play.mockRestore();
    pause.mockRestore();
    vi.unstubAllGlobals();
  }
});

test("fresh readiness receives the exact selected voice identity before any microphone capture", async () => {
  const h = harness();
  const checked: string[] = [];
  h.env.voiceAvailable = async (identity) => {
    checked.push(identity);
    assert.equal(h.streams.length, 0);
    return "unavailable";
  };
  await h.controller.start(other);
  assert.deepEqual(checked, [other.identity]);
  assert.equal(h.calls.length, 0);
});

test("owner-issued replacement reuses request and mic, preserves target/mutes and original silence deadline", async () => {
  for (const reason of ["canonical_context", "delegation_result"]) {
    const h = harness();
    await h.controller.start(target);
    h.controller.toggleMicrophone();
    h.controller.toggleSpeaker();
    const oldPeer = h.peers[0];
    const oldNodes = [...h.contexts[0].sources, ...h.contexts[0].analysers, ...h.contexts[0].gains];
    let required = true;
    h.setRpc((method) => {
      if (method !== "meta/console/voice/replacement") return undefined;
      const result = required ? replacement(reason) : { required: false };
      required = false;
      return Promise.resolve(result);
    });
    await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
    assert.equal(h.controller.getSnapshot().phase, "active");
    assert.deepEqual(h.controller.getSnapshot().target, target);
    assert.equal(h.controller.getSnapshot().microphoneMuted, true);
    assert.equal(h.controller.getSnapshot().speakerMuted, true);
    assert.equal(h.streams.length, 1, "recovery must not request microphone permission again");
    assert.equal(h.contexts.length, 1);
    assert.equal(h.streams[0].tracks[0].stopped, false, "existing microphone remains reusable");
    assert.equal(h.streams[0].tracks[0].enabled, false);
    assert.equal(oldPeer.remoteTrack.stopped, true);
    assert.equal(oldPeer.connectionState, "closed");
    assert.ok(oldNodes.every((node) => node.disconnected));
    assert.equal(h.peers.length, 2);
    assert.equal(masterGain(h.contexts[0], 1).gain.value, 0);
    assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
    const register = h.calls.filter((call) => call.method === "meta/live/playback_owner/register").at(-1)!;
    assert.equal(register.params.channel_id, "recovery-channel");
    const poll = h.calls.find((call) => call.method === "meta/console/voice/replacement")!;
    assert.deepEqual(poll.params, { identity: target.identity, request_id: "request-1" });
    await h.clock.advance(VOICE_SILENCE_TIMEOUT_MS - VOICE_REPLACEMENT_POLL_INTERVAL_MS - 1);
    assert.equal(h.controller.getSnapshot().phase, "active");
    await h.clock.advance(1);
    assert.equal(h.controller.getSnapshot().phase, "idle");
    assert.equal(h.streams[0].tracks[0].stopped, true);
    assert.deepEqual(h.calls.at(-1)?.params, { identity: target.identity, request_id: "request-1" });
    assert.equal(h.clock.timers.size, 0);
  }
});

test("recovery keeps media gated until new typed activation and can be cancelled without late resurrection", async () => {
  const h = harness();
  await h.controller.start(target);
  const status = deferred<unknown>();
  h.setRpc((method, params) => {
    if (method === "meta/console/voice/replacement") return Promise.resolve(replacement());
    if (method === "meta/live/status" && params.channel_id === "recovery-channel") return status.promise;
    return undefined;
  });
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  assert.equal(h.streams[0].tracks[0].enabled, false);
  assert.equal(masterGain(h.contexts[0], 1).gain.value, 0);
  assert.equal(h.peers[0].remoteTrack.stopped, true);
  h.controller.toggleMicrophone();
  h.controller.toggleMicrophone();
  h.controller.toggleSpeaker();
  h.controller.toggleSpeaker();
  assert.equal(h.streams[0].tracks[0].enabled, false);
  assert.equal(masterGain(h.contexts[0], 1).gain.value, 0);
  await h.controller.close();
  status.resolve({
    phase: "active", handle: {
      channel_id: "recovery-channel", target_identity: target.identity,
      execution_mode: "client_context", activation_receipt: "late-receipt",
    },
  });
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.ok(h.peers.every((peer) => peer.connectionState === "closed"));
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
  assert.equal(h.clock.timers.size, 0);
});

test("late old-peer events and playback promises cannot kill or ungate the replacement", async () => {
  const h = harness();
  const playback = deferred<void>();
  let firstAudio = true;
  h.env.createAudioElement = () => {
    const first = firstAudio;
    firstAudio = false;
    return {
      play: () => first ? playback.promise : Promise.resolve(), pause() {},
      autoplay: false, muted: false, srcObject: null,
    } as unknown as HTMLAudioElement;
  };
  await h.controller.start(target);
  const old = h.peers[0];
  const lateTrack = old.ontrack!;
  const lateConnection = old.onconnectionstatechange!;
  const lateMessage = old.channel.onmessage!;
  const lateClose = old.channel.onclose!;
  const lateError = old.channel.onerror!;
  const lateRemoteEnd = old.remoteTrack.onended!;
  h.setRpc((method) => method === "meta/console/voice/replacement" ? Promise.resolve(replacement()) : undefined);
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
  const snapshot = h.controller.getSnapshot();
  const late = new Track();
  lateTrack({ track: late });
  lateConnection();
  lateMessage({ data: JSON.stringify({ type: "error" }) });
  lateClose();
  lateError();
  lateRemoteEnd();
  playback.reject(new Error("old audio player rejected after replacement"));
  await flush();
  assert.equal(late.stopped, true);
  assert.equal(h.controller.getSnapshot(), snapshot);
  assert.equal(snapshot.phase, "active");
  assert.equal(h.streams[0].tracks[0].enabled, true);
  await h.controller.close();
});

test("retained replacement response already bound does not create repeated peers", async () => {
  const h = harness();
  await h.controller.start(target);
  h.setRpc((method) => method === "meta/console/voice/replacement" ? Promise.resolve(replacement()) : undefined);
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS * 3);
  assert.equal(h.peers.length, 2);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
  assert.equal(h.controller.getSnapshot().phase, "active");
  await h.controller.close();
});

test("malformed, legacy, conflicting or cross-target replacement authority fails closed", async () => {
  const legacy = pending();
  delete (legacy as Partial<typeof legacy>).pending_receipt;
  for (const raw of [
    { required: "false" },
    replacement("unknown"),
    { ...replacement(), canonical_seed_cursor: -1 },
    replacement("canonical_context", legacy),
    replacement("canonical_context", pending(other.identity)),
    replacement("canonical_context", { ...pending(target.identity, `channel-${target.identity}`), pending_receipt: "conflict" }),
  ]) {
    const h = harness();
    await h.controller.start(target);
    h.setRpc((method) => method === "meta/console/voice/replacement" ? Promise.resolve(raw) : undefined);
    await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
    assert.equal(h.controller.getSnapshot().phase, "error");
    assert.equal(h.streams[0].tracks[0].stopped, true);
    assert.equal(h.calls.at(-1)?.method, "meta/console/voice/close");
    assert.equal(h.clock.timers.size, 0);
  }
});

test("closed, superseded, denied or lost replacement polling fails closed and never impersonates no replacement", async () => {
  for (const kind of ["voice_closed", "voice_superseded", "access_denied", "transport_lost"]) {
    const h = harness();
    await h.controller.start(target);
    const error = Object.assign(new Error("internal detail"), {
      rpcError: kind === "voice_superseded" ? voiceContract.superseded_error : { data: { kind } },
    });
    h.setRpc((method) => method === "meta/console/voice/replacement" ? Promise.reject(error) : undefined);
    await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
    assert.equal(h.controller.getSnapshot().phase, "error");
    if (kind === "voice_closed") assert.match(h.controller.getSnapshot().error!, /gateway closed/);
    if (kind === "voice_superseded") assert.match(h.controller.getSnapshot().error!, /moved to the external live channel/);
  }
  // The reason selects the wording: another console call is not "the external live channel".
  {
    const h = harness();
    await h.controller.start(target);
    const error = Object.assign(new Error("internal detail"), {
      rpcError: { data: { kind: "voice_superseded", reason: "superseded_by_console_voice" } },
    });
    h.setRpc((method) => method === "meta/console/voice/replacement" ? Promise.reject(error) : undefined);
    await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
    assert.equal(h.controller.getSnapshot().phase, "error");
    assert.match(h.controller.getSnapshot().error!, /moved to another console call/);
    assert.equal(h.streams[0].tracks[0].stopped, true);
    assert.equal(h.calls.at(-1)?.method, "meta/console/voice/close");
  }
});

test("a stalled replacement poll keeps connected audio and only sustained failure closes voice at the tolerance", async () => {
  const h = harness();
  await h.controller.start(target);
  h.setRpc((method) => method === "meta/console/voice/replacement" ? new Promise(() => {}) : undefined);
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS + VOICE_TEARDOWN_TIMEOUT_MS);
  assert.equal(h.controller.getSnapshot().phase, "active", "one stalled poll does not end connected audio");
  assert.equal(h.streams[0].tracks[0].enabled, true);
  assert.equal(h.contexts[0].gains[0].gain.value, 1);
  await h.clock.advance(VOICE_RPC_FAILURE_TOLERANCE_MS - 1);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.ok(h.calls.filter((call) => call.method.endsWith("/replacement")).length > 1, "polling retried with backoff");
  await h.clock.advance(1);
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.match(h.controller.getSnapshot().error!, /could not be verified/);
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.calls.at(-1)?.method, "meta/console/voice/close");
  assert.equal(h.clock.timers.size, 0);
});

test("one failed replacement poll is retried with backoff and a later success disarms the tolerance", async () => {
  const h = harness();
  await h.controller.start(target);
  let polls = 0;
  h.setRpc((method) => {
    if (method !== "meta/console/voice/replacement") return undefined;
    polls++;
    return polls <= 2 ? Promise.reject(new Error("network failed")) : Promise.resolve({ required: false });
  });
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
  assert.equal(polls, 1);
  assert.equal(h.controller.getSnapshot().phase, "active");
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
  assert.equal(polls, 2, "first retry after the base interval");
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
  assert.equal(polls, 2, "second retry backs off");
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
  assert.equal(polls, 3);
  await h.clock.advance(VOICE_RPC_FAILURE_TOLERANCE_MS);
  assert.equal(h.controller.getSnapshot().phase, "active", "the success disarmed the tolerance window");
  assert.equal(h.streams[0].tracks[0].enabled, true);
  await h.controller.close();
  assert.equal(h.clock.timers.size, 0);
});

test("pending replacement polling is cancelled on close and its late response cannot start a new peer", async () => {
  const h = harness();
  await h.controller.start(target);
  const result = deferred<unknown>();
  h.setRpc((method) => method === "meta/console/voice/replacement" ? result.promise : undefined);
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
  await h.controller.close();
  result.resolve(replacement());
  await flush();
  assert.equal(h.peers.length, 1);
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.equal(h.clock.timers.size, 0);
});

test("silence deadline can expire during recovery without waiting for connect timeout", async () => {
  const h = harness();
  await h.controller.start(target);
  await h.clock.advance(VOICE_SILENCE_TIMEOUT_MS - 2000);
  h.setRpc((method, params) => {
    if (method === "meta/console/voice/replacement") return Promise.resolve(replacement());
    if (method === "meta/live/status" && params.channel_id === "recovery-channel") return new Promise(() => {});
    return undefined;
  });
  await h.clock.advance(1000);
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  await h.clock.advance(999);
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  await h.clock.advance(1);
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.match(h.controller.getSnapshot().notice!, /15 minutes of silence/);
  assert.equal(h.clock.timers.size, 0);
});

test("new target start during recovery tears down the entire old request before opening the new target", async () => {
  const h = harness();
  await h.controller.start(target);
  const answer = deferred<unknown>();
  h.setRpc((method, params) => {
    if (method === "meta/console/voice/replacement") return Promise.resolve(replacement());
    if (method === "live/webrtc/answer" && params.channel_id === "recovery-channel") return answer.promise;
    return undefined;
  });
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
  await h.controller.start(other);
  answer.resolve({ answer_sdp: "late-answer" });
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.controller.getSnapshot().target?.identity, other.identity);
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.peers[0].connectionState, "closed");
  assert.equal(h.peers[1].connectionState, "closed");
  const newOpen = h.calls.findIndex((call) => call.method.endsWith("/open") && call.params.identity === other.identity);
  const oldClose = h.calls.findIndex((call) => call.method.endsWith("/close"));
  assert.ok(oldClose < newOpen);
  await h.controller.close();
});

test("old peer closing before the next poll discovers owner recovery instead of cancelling its request", async () => {
  for (const lose of [
    (peer: Peer) => { peer.connectionState = "closed"; peer.onconnectionstatechange?.(); },
    (peer: Peer) => { peer.channel.onclose?.(); },
    (peer: Peer) => { peer.remoteTrack.end(); },
  ]) {
    const h = harness();
    await h.controller.start(target);
    const oldPeer = h.peers[0];
    const lateState = oldPeer.onconnectionstatechange!;
    h.setRpc((method) => method === "meta/console/voice/replacement" ? Promise.resolve(replacement()) : undefined);
    lose(oldPeer);
    assert.equal(h.controller.getSnapshot().phase, "connecting");
    assert.equal(h.streams[0].tracks[0].enabled, false);
    assert.equal(h.streams[0].tracks[0].stopped, false);
    await flush();
    assert.equal(h.controller.getSnapshot().phase, "active");
    assert.deepEqual(h.controller.getSnapshot().target, target);
    assert.equal(h.peers.length, 2);
    assert.equal(h.streams.length, 1);
    assert.equal(h.streams[0].tracks[0].enabled, true);
    lateState();
    assert.equal(h.controller.getSnapshot().phase, "active");
    assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
    assert.equal(h.calls.filter((call) => call.method.endsWith("/close")).length, 0);
    await h.controller.close();
  }
});

test("transport loss joins an in-flight request-owned replacement check instead of racing a second poll", async () => {
  const h = harness();
  await h.controller.start(target);
  const result = deferred<unknown>();
  h.setRpc((method) => method === "meta/console/voice/replacement" ? result.promise : undefined);
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
  const lateConnection = h.peers[0].onconnectionstatechange!;
  h.peers[0].fail();
  lateConnection();
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  assert.equal(h.calls.filter((call) => call.method.endsWith("/replacement")).length, 1);
  result.resolve({ required: false });
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  h.setRpc((method) => method === "meta/console/voice/replacement" ? Promise.resolve(replacement()) : undefined);
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.peers.length, 2);
  await h.controller.close();
});

test("lost transport fails closed on consumed authority without opening fresh", async () => {
  const h = harness();
  await h.controller.start(target);
  h.setRpc((method) => method === "meta/console/voice/replacement"
    ? Promise.resolve(replacement("canonical_context", pending(target.identity, `channel-${target.identity}`)))
    : undefined);
  h.peers[0].fail();
  assert.equal(h.streams[0].tracks[0].enabled, false);
  await h.clock.advance(VOICE_TEARDOWN_TIMEOUT_MS);
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/replacement")).length, 1);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
  assert.equal(h.calls.at(-1)?.method, "meta/console/voice/close");
  assert.equal(h.clock.timers.size, 0);
});

test("lost transport with a stalled replacement poll keeps retrying until the fixed recovery deadline", async () => {
  const h = harness();
  await h.controller.start(target);
  h.setRpc((method) => method === "meta/console/voice/replacement" ? new Promise<unknown>(() => {}) : undefined);
  h.peers[0].fail();
  assert.equal(h.streams[0].tracks[0].enabled, false);
  await h.clock.advance(VOICE_TEARDOWN_TIMEOUT_MS);
  assert.equal(h.controller.getSnapshot().phase, "connecting", "one stalled poll does not abandon recovery");
  await h.clock.advance(VOICE_RECOVERY_TIMEOUT_MS - VOICE_TEARDOWN_TIMEOUT_MS - 1);
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  await h.clock.advance(1);
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.match(h.controller.getSnapshot().error!, /recovery timed out/);
  assert.ok(h.calls.filter((call) => call.method.endsWith("/replacement")).length > 1, "polling retried with backoff");
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.calls.at(-1)?.method, "meta/console/voice/close");
  assert.equal(h.clock.timers.size, 0);
});

test("transport loss waits for delayed owner replacement while preserving target, mutes and silence", async () => {
  const h = harness();
  await h.controller.start(target);
  h.controller.toggleMicrophone();
  h.controller.toggleSpeaker();
  await h.clock.advance(10_000);
  let polls = 0;
  h.setRpc((method) => {
    if (method !== "meta/console/voice/replacement") return undefined;
    polls++;
    return Promise.resolve(polls <= 3 ? { required: false } : replacement());
  });
  h.peers[0].fail();
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  assert.equal(h.streams[0].tracks[0].enabled, false);
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS * 3 - 1);
  assert.equal(polls, 3);
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  assert.equal(h.calls.filter((call) => call.method.endsWith("/close")).length, 0);
  await h.clock.advance(1);
  assert.equal(polls, 4);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.deepEqual(h.controller.getSnapshot().target, target);
  assert.equal(h.controller.getSnapshot().microphoneMuted, true);
  assert.equal(h.controller.getSnapshot().speakerMuted, true);
  assert.equal(masterGain(h.contexts[0], 1).gain.value, 0);
  assert.equal(h.streams.length, 1);
  assert.equal(h.peers.length, 2);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
  h.setRpc(undefined);
  await h.clock.advance(VOICE_SILENCE_TIMEOUT_MS - h.clock.now - 1);
  assert.equal(h.controller.getSnapshot().phase, "active");
  await h.clock.advance(1);
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.match(h.controller.getSnapshot().notice!, /15 minutes of silence/);
  assert.equal(h.clock.timers.size, 0);
});

test("false replacement replies and repeated old-peer loss cannot extend the fixed recovery deadline", async () => {
  const h = harness();
  await h.controller.start(target);
  const old = h.peers[0];
  const lateState = old.onconnectionstatechange!;
  const lateClose = old.channel.onclose!;
  let polls = 0;
  h.setRpc((method) => {
    if (method !== "meta/console/voice/replacement") return undefined;
    polls++;
    lateState();
    lateClose();
    return Promise.resolve({ required: false });
  });
  old.fail();
  await h.clock.advance(VOICE_RECOVERY_TIMEOUT_MS - 1);
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  assert.equal(h.streams[0].tracks[0].enabled, false);
  assert.equal(h.streams[0].tracks[0].stopped, false);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/close")).length, 0);
  await h.clock.advance(1);
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.match(h.controller.getSnapshot().error!, /recovery timed out/);
  assert.equal(polls, VOICE_RECOVERY_TIMEOUT_MS / VOICE_REPLACEMENT_POLL_INTERVAL_MS);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/close")).length, 1);
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.clock.timers.size, 0);
});

test("late pending discovery cannot extend the recovery deadline with a new activation timeout", async () => {
  const h = harness();
  await h.controller.start(target);
  h.setRpc((method, params) => {
    if (method === "meta/console/voice/replacement") {
      return Promise.resolve(h.clock.now < VOICE_RECOVERY_TIMEOUT_MS - 1000 ? { required: false } : replacement());
    }
    if (method === "meta/live/status" && params.channel_id === "recovery-channel") return new Promise(() => {});
    return undefined;
  });
  h.peers[0].fail();
  await h.clock.advance(VOICE_RECOVERY_TIMEOUT_MS - 1);
  assert.equal(h.peers.length, 2);
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  await h.clock.advance(1);
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.match(h.controller.getSnapshot().error!, /recovery timed out/);
  assert.ok(h.peers.every((peer) => peer.connectionState === "closed"));
  assert.equal(h.clock.timers.size, 0);
});

test("explicit cancel during delayed replacement discovery fences late pending without a new peer", async () => {
  const h = harness();
  await h.controller.start(target);
  const late = deferred<unknown>();
  let polls = 0;
  h.setRpc((method) => {
    if (method !== "meta/console/voice/replacement") return undefined;
    polls++;
    return polls <= 2 ? Promise.resolve({ required: false }) : late.promise;
  });
  h.peers[0].fail();
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS * 2);
  assert.equal(polls, 3);
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  await h.controller.close();
  late.resolve(replacement());
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.equal(h.peers.length, 1);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.clock.timers.size, 0);
});

test("original audio silence deadline expires during false recovery replies before the recovery deadline", async () => {
  const h = harness();
  await h.controller.start(target);
  await h.clock.advance(VOICE_SILENCE_TIMEOUT_MS - 2000);
  h.peers[0].fail();
  await h.clock.advance(1999);
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  assert.equal(h.streams[0].tracks[0].enabled, false);
  await h.clock.advance(1);
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.match(h.controller.getSnapshot().notice!, /15 minutes of silence/);
  assert.equal(h.peers.length, 1);
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.clock.timers.size, 0);
});

test("terminal authorization errors stop recovery waiting immediately", async () => {
  const h = harness();
  await h.controller.start(target);
  let polls = 0;
  h.setRpc((method) => {
    if (method !== "meta/console/voice/replacement") return undefined;
    polls++;
    return polls < 3
      ? Promise.resolve({ required: false })
      : Promise.reject(Object.assign(new Error("access revoked"), {
        rpcError: { code: -32030, data: { kind: "access_denied" } },
      }));
  });
  h.peers[0].fail();
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS * 2);
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/open")).length, 1);
  assert.equal(h.clock.timers.size, 0);
});

test("explicit close wins over a transport-loss recovery check and fences its late pending handle", async () => {
  const h = harness();
  await h.controller.start(target);
  const result = deferred<unknown>();
  h.setRpc((method) => method === "meta/console/voice/replacement" ? result.promise : undefined);
  h.peers[0].fail();
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  await h.controller.close();
  result.resolve(replacement());
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.equal(h.peers.length, 1);
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.clock.timers.size, 0);
});

test("answer delivery ACK follows remote description and precedes status or media release", async () => {
  const h = harness();
  const receipt = deferred<unknown>();
  h.setRpc((method, params) => {
    if (method !== "meta/console/voice/answer_received") return undefined;
    assert.equal(h.peers[0].connectionState, "connected", "answer must have been applied locally");
    assert.equal(h.streams[0].tracks[0].enabled, false);
    assert.equal(h.contexts[0].gains[0].gain.value, 0);
    assert.deepEqual(params, {
      identity: target.identity, request_id: "request-1", channel_id: `channel-${target.identity}`,
    });
    return receipt.promise;
  });
  const start = h.controller.start(target);
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  assert.equal(h.calls.some((call) => call.method === "meta/live/status"), false);
  receipt.resolve({ accepted: true });
  await start;
  assert.equal(h.controller.getSnapshot().phase, "active");
  await h.controller.close();
});

test("missing or denied answer delivery ACK fails closed without querying activation", async () => {
  for (const result of [null, {}, [], { accepted: false }, { accepted: "true" }]) {
    const h = harness();
    h.setRpc((method) => method === "meta/console/voice/answer_received" ? Promise.resolve(result) : undefined);
    await h.controller.start(target);
    assert.equal(h.controller.getSnapshot().phase, "error");
    assert.match(h.controller.getSnapshot().error!, /acknowledge voice answer delivery/);
    assert.equal(h.calls.some((call) => call.method === "meta/live/status"), false);
    assert.equal(h.streams[0].tracks[0].stopped, true);
    assert.equal(h.calls.at(-1)?.method, "meta/console/voice/close");
  }
});

test("each recovery answer has its own channel-scoped delivery ACK, cancelled late ACK cannot activate", async () => {
  const h = harness();
  await h.controller.start(target);
  const receipt = deferred<unknown>();
  h.setRpc((method, params) => {
    if (method === "meta/console/voice/replacement") return Promise.resolve(replacement());
    if (method === "meta/console/voice/answer_received" && params.channel_id === "recovery-channel") {
      assert.deepEqual(params, {
        identity: target.identity, request_id: "request-1", channel_id: "recovery-channel",
      });
      return receipt.promise;
    }
    return undefined;
  });
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS);
  assert.equal(h.controller.getSnapshot().phase, "connecting");
  assert.equal(h.calls.filter((call) => call.method === "meta/console/voice/answer_received").length, 2);
  assert.equal(h.calls.some((call) => call.method === "meta/live/status" && call.params.channel_id === "recovery-channel"), false);
  await h.controller.close();
  receipt.resolve({ accepted: true });
  await flush();
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.clock.timers.size, 0);
});

test("actual audio reports activity immediately, throttles to five seconds, and silence never sends heartbeats", async () => {
  const h = harness();
  const times: number[] = [];
  h.setRpc((method, params) => {
    if (method !== "meta/console/voice/activity") return undefined;
    times.push(h.clock.now);
    assert.deepEqual(params, { identity: target.identity, request_id: "request-1" });
    return Promise.resolve({ accepted: true });
  });
  await h.controller.start(target);
  assert.deepEqual(times, []);
  h.contexts[0].analysers[0].signal = 0.25;
  await h.clock.advance(100);
  assert.deepEqual(times, [100], "first observed audio reports without debounce delay");
  await h.clock.advance(VOICE_ACTIVITY_REPORT_INTERVAL_MS - 1);
  assert.deepEqual(times, [100]);
  h.contexts[0].analysers[0].signal = 0;
  await h.clock.advance(1);
  assert.deepEqual(times, [100, 5100]);
  await h.clock.advance(VOICE_ACTIVITY_REPORT_INTERVAL_MS * 3);
  assert.deepEqual(times, [100, 5100], "elapsed timers alone must never report activity");
  await h.controller.close();
});

test("muted mic, text and quiet frames never report activity; incoming model audio counts with muted speaker", async () => {
  const h = harness();
  await h.controller.start(target);
  h.controller.toggleMicrophone();
  h.controller.toggleSpeaker();
  h.contexts[0].analysers[0].signal = 1;
  h.peers[0].channel.emit({ type: "input_audio_buffer.speech_started" });
  h.peers[0].channel.emit({ type: "response.output_text.delta", delta: "Only text" });
  h.peers[0].channel.emit({ type: "response.output_audio_transcript.delta", delta: "Only transcript" });
  h.peers[0].channel.emit({ type: "response.output_audio.delta", delta: "AAAA" });
  await h.clock.advance(VOICE_ACTIVITY_REPORT_INTERVAL_MS * 2);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/activity")).length, 0);
  h.contexts[0].analysers[1].signal = 0.5;
  await h.clock.advance(100);
  assert.equal(h.contexts[0].gains[0].gain.value, 0);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/activity")).length, 1);
  await h.controller.close();
});

test("activity reports never overlap; a stalled confirmation retries and only sustained failure closes voice", async () => {
  const h = harness();
  const reports = () => h.calls.filter((call) => call.method.endsWith("/activity")).length;
  h.setRpc((method) => method === "meta/console/voice/activity" ? new Promise(() => {}) : undefined);
  await h.controller.start(target);
  h.contexts[0].analysers[0].signal = 0.5;
  await h.clock.advance(100);
  assert.equal(reports(), 1);
  await h.clock.advance(VOICE_TEARDOWN_TIMEOUT_MS - 1);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(reports(), 1, "no overlapping report while one is in flight");
  await h.clock.advance(1);
  assert.equal(h.controller.getSnapshot().phase, "active", "one timed-out heartbeat does not end connected audio");
  assert.equal(h.streams[0].tracks[0].enabled, true);
  await h.clock.advance(VOICE_RPC_FAILURE_TOLERANCE_MS - 1);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.ok(reports() > 1, "the heartbeat was retried");
  await h.clock.advance(1);
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.match(h.controller.getSnapshot().error!, /activity could not be confirmed/);
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.calls.at(-1)?.method, "meta/console/voice/close");
  assert.equal(h.clock.timers.size, 0);
});

test("one failed activity heartbeat keeps audio flowing and a later success disarms the tolerance", async () => {
  const h = harness();
  let failing = true;
  let reports = 0;
  h.setRpc((method) => {
    if (method !== "meta/console/voice/activity") return undefined;
    reports++;
    return failing ? Promise.reject(new Error("network failed")) : Promise.resolve({ accepted: true });
  });
  await h.controller.start(target);
  h.contexts[0].analysers[1].signal = 0.5;
  await h.clock.advance(100);
  assert.equal(reports, 1);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.contexts[0].gains[0].gain.value, 1, "speaker output keeps flowing");
  assert.equal(h.streams[0].tracks[0].enabled, true);
  failing = false;
  await h.clock.advance(VOICE_ACTIVITY_REPORT_INTERVAL_MS);
  assert.equal(reports, 2, "retried after the base interval");
  await h.clock.advance(VOICE_RPC_FAILURE_TOLERANCE_MS);
  assert.equal(h.controller.getSnapshot().phase, "active", "the success disarmed the tolerance window");
  await h.controller.close();
  assert.equal(h.clock.timers.size, 0);
});

test("sustained activity heartbeat failure closes voice at the tolerance deadline after bounded backoff", async () => {
  const h = harness();
  let reports = 0;
  h.setRpc((method) => {
    if (method !== "meta/console/voice/activity") return undefined;
    reports++;
    return Promise.reject(new Error("network failed"));
  });
  await h.controller.start(target);
  h.contexts[0].analysers[1].signal = 0.5;
  await h.clock.advance(100);
  assert.equal(reports, 1);
  await h.clock.advance(VOICE_RPC_FAILURE_TOLERANCE_MS - 1);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.ok(reports >= 4 && reports <= 6, `retries are backed off, not hammered: ${reports}`);
  await h.clock.advance(1);
  assert.equal(h.controller.getSnapshot().phase, "error");
  assert.match(h.controller.getSnapshot().error!, /activity could not be confirmed/);
  assert.equal(h.streams[0].tracks[0].stopped, true);
  assert.equal(h.calls.at(-1)?.method, "meta/console/voice/close");
  assert.equal(h.clock.timers.size, 0);
});

test("HTTP 5xx and 429 heartbeat rejections are transient; other HTTP statuses and typed RPC errors are definite", async () => {
  for (const [annotation, transient] of [
    [{ httpStatus: 503 }, true],
    [{ httpStatus: 429 }, true],
    [{ httpStatus: 403 }, false],
    [{ rpcError: { code: -32030, data: { kind: "access_denied" } } }, false],
  ] as const) {
    const h = harness();
    h.setRpc((method) => method === "meta/console/voice/activity"
      ? Promise.reject(Object.assign(new Error("gateway"), annotation))
      : undefined);
    await h.controller.start(target);
    h.contexts[0].analysers[0].signal = 0.5;
    await h.clock.advance(100);
    assert.equal(h.controller.getSnapshot().phase, transient ? "active" : "error", JSON.stringify(annotation));
    await h.controller.close();
    assert.equal(h.clock.timers.size, 0);
  }
});

test("invalid activity acceptance is a definite gateway answer: it closes voice and clears its silence deadline", async () => {
  for (const response of [
    () => Promise.resolve({ accepted: false }),
    () => Promise.resolve({ accepted: "true" }),
  ]) {
    const h = harness();
    h.setRpc((method) => method === "meta/console/voice/activity" ? response() : undefined);
    await h.controller.start(target);
    h.contexts[0].analysers[1].signal = 0.5;
    await h.clock.advance(100);
    assert.equal(h.controller.getSnapshot().phase, "error");
    assert.equal(h.streams[0].tracks[0].stopped, true);
    assert.equal(h.calls.at(-1)?.method, "meta/console/voice/close");
    assert.equal(h.clock.timers.size, 0);
  }
});

test("late activity acknowledgement after close is inert", async () => {
  const h = harness();
  const accepted = deferred<unknown>();
  h.setRpc((method) => method === "meta/console/voice/activity" ? accepted.promise : undefined);
  await h.controller.start(target);
  h.contexts[0].analysers[0].signal = 0.5;
  await h.clock.advance(100);
  await h.controller.close();
  const snapshot = h.controller.getSnapshot();
  accepted.resolve({ accepted: true });
  await flush();
  assert.equal(h.controller.getSnapshot(), snapshot);
  assert.equal(snapshot.phase, "idle");
  assert.equal(h.clock.timers.size, 0);
});

test("activity uses the same request scope across owner-issued recovery, never old channel authority", async () => {
  const h = harness();
  await h.controller.start(target);
  h.peers[0].channel.emit({ type: "input_audio_buffer.speech_started" });
  await flush();
  h.setRpc((method) => method === "meta/console/voice/replacement" ? Promise.resolve(replacement()) : undefined);
  await h.clock.advance(VOICE_ACTIVITY_REPORT_INTERVAL_MS);
  assert.equal(h.peers.length, 2);
  h.peers[1].channel.emit({ type: "input_audio_buffer.speech_started" });
  await flush();
  const reports = h.calls.filter((call) => call.method === "meta/console/voice/activity");
  assert.equal(reports.length, 2);
  assert.ok(reports.every((call) => Object.keys(call.params).length === 2));
  assert.deepEqual(reports.map((call) => call.params), [
    { identity: target.identity, request_id: "request-1" },
    { identity: target.identity, request_id: "request-1" },
  ]);
  await h.controller.close();
});

test("unreported real audio during cooldown flushes once after quiet without changing the local silence deadline", async () => {
  const h = harness();
  const reports: number[] = [];
  h.setRpc((method) => {
    if (method !== "meta/console/voice/activity") return undefined;
    reports.push(h.clock.now);
    return Promise.resolve({ accepted: true });
  });
  await h.controller.start(target);
  h.contexts[0].analysers[0].signal = 0.5;
  await h.clock.advance(200);
  h.contexts[0].analysers[0].signal = 0;
  assert.deepEqual(reports, [100]);
  await h.clock.advance(VOICE_ACTIVITY_REPORT_INTERVAL_MS - 100);
  assert.deepEqual(reports, [100, 5100], "the last real sample at 200ms must reach the server");
  await h.clock.advance(VOICE_SILENCE_TIMEOUT_MS + 200 - h.clock.now - 1);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.deepEqual(reports, [100, 5100], "quiet must not perpetually re-arm trailing reports");
  await h.clock.advance(1);
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.match(h.controller.getSnapshot().notice!, /15 minutes of silence/);
  assert.equal(h.clock.timers.size, 0);
});

test("audio received during an in-flight report remains dirty for one trailing flush, without overlap", async () => {
  const h = harness();
  const accepted = deferred<unknown>();
  let reports = 0;
  h.setRpc((method) => {
    if (method !== "meta/console/voice/activity") return undefined;
    reports++;
    return reports === 1 ? accepted.promise : Promise.resolve({ accepted: true });
  });
  await h.controller.start(target);
  h.contexts[0].analysers[0].signal = 0.5;
  await h.clock.advance(300);
  h.contexts[0].analysers[0].signal = 0;
  assert.equal(reports, 1);
  accepted.resolve({ accepted: true });
  await flush();
  await h.clock.advance(VOICE_ACTIVITY_REPORT_INTERVAL_MS - 200);
  assert.equal(reports, 2);
  await h.clock.advance(VOICE_ACTIVITY_REPORT_INTERVAL_MS * 2);
  assert.equal(reports, 2);
  await h.controller.close();
});

test("close cancels a dirty trailing activity flush", async () => {
  const h = harness();
  await h.controller.start(target);
  h.contexts[0].analysers[0].signal = 0.5;
  await h.clock.advance(200);
  h.contexts[0].analysers[0].signal = 0;
  await h.controller.close();
  await h.clock.advance(VOICE_ACTIVITY_REPORT_INTERVAL_MS * 2);
  assert.equal(h.calls.filter((call) => call.method.endsWith("/activity")).length, 1);
  assert.equal(h.controller.getSnapshot().phase, "idle");
  assert.equal(h.clock.timers.size, 0);
});

test("provider-managed unmeasured voice needs no output observer or browser playback settlement", async () => {
  const h = harness();
  await h.controller.start(target);
  h.peers[0].channel.emit({ type: "response.done" });
  h.peers[0].channel.emit({ type: "output_audio_buffer.stopped" });
  h.peers[0].channel.emit({
    type: "live.assistant_output",
    output: { channel_id: `channel-${target.identity}`, output_id: "untrusted", content_index: 0 },
  });
  await h.clock.advance(VOICE_REPLACEMENT_POLL_INTERVAL_MS * 2);
  assert.equal(h.controller.getSnapshot().phase, "active");
  assert.equal(h.calls.some((call) => /outputs\/|playback_complete|truncate/.test(call.method)), false);
  assert.equal(h.calls.some((call) => call.method.endsWith("/activity")), false);
  await h.controller.close();
});
