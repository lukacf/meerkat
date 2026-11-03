import {
  parseExperimentalLiveChannelStatus,
  parseLivePlaybackOwnerReadiness,
  parsePendingLiveChannelHandle,
  type ActiveLiveChannelHandle,
  type LivePlaybackOwnerReadiness,
  type PendingLiveChannelHandle,
} from "../../../sdk/typescript/src/live";
import { callConsoleRpc } from "./network";
import { errorMessage, httpStatusCode, jsonRpcErrorCode } from "./errors";
import { CONSOLE_RPC_PATHS } from "./contract";
import { parseVoiceContextStatus, type VoiceContextPreparation } from "./voice-context";
import {
  parseVoiceCaptions, VOICE_CAPTIONS_METHOD, type VoiceCaption, type VoicePlaybackHint,
} from "./voice-captions";

export const VOICE_SILENCE_TIMEOUT_MS = 15 * 60 * 1000;
export const VOICE_CONNECT_TIMEOUT_MS = 30_000;
export const VOICE_RECOVERY_TIMEOUT_MS = 30_000;
export const VOICE_TEARDOWN_TIMEOUT_MS = 5_000;
/**
 * Backoff between gateway close attempts after a retryable failure. The owner retires a dead
 * remote within its own bound, so the schedule spans that bound; once it is exhausted the
 * closure is reported unconfirmed and the user must close again.
 */
export const VOICE_CLOSE_RETRY_DELAYS_MS: readonly number[] = [2_000, 4_000, 8_000];
/** Gateway-side failure of a live verb (JSON-RPC `-32000`); retried for close, unlike caller-side rejections. */
export const GATEWAY_INTERNAL_ERROR_CODE = -32000;
export const VOICE_REPLACEMENT_POLL_INTERVAL_MS = 1_000;
export const VOICE_ACTIVITY_REPORT_INTERVAL_MS = 5_000;
/** ICE `disconnected` is transient and usually self-heals; keep the peer alive this long first. */
export const VOICE_TRANSPORT_RECONNECT_GRACE_MS = 8_000;
/** Consecutive transient control-plane failures tolerated before a connected call is failed. */
export const VOICE_RPC_FAILURE_TOLERANCE_MS = 30_000;
export const VOICE_RPC_RETRY_BACKOFF_MAX_MS = 8_000;
/** A suspended or interrupted AudioContext gets this long to return to `running`. */
export const VOICE_AUDIO_RESUME_TIMEOUT_MS = 15_000;
/** How long one caption read asks the gateway to wait for a newer caption. */
export const VOICE_CAPTION_WAIT_MS = 10_000;
/** Pause before the next caption read after an empty batch or a failed read. */
export const VOICE_CAPTION_IDLE_DELAY_MS = 250;
export const VOICE_CAPTION_RETRY_DELAY_MS = 1_000;
/** Barge-in playback gate: silence while ducked, unity otherwise, 10 ms ramp. */
export const VOICE_PLAYBACK_DUCKED_GAIN = 0;
export const VOICE_PLAYBACK_UNITY_GAIN = 1;
export const VOICE_PLAYBACK_GAIN_TIME_CONSTANT_S = 0.01;
const CONTEXT_POLL_INTERVAL_MS = 1_000;
const CONTEXT_RETRY_INTERVAL_MS = 5_000;
const SAMPLE_INTERVAL_MS = 100;
const POLL_INTERVAL_MS = 100;
const RECOVERY_TIMEOUT_MESSAGE = "Voice recovery timed out. Check your network and voice access, then start again.";
const ACTIVITY_UNCONFIRMED_MESSAGE = "Voice activity could not be confirmed. Check your network and voice access, then start again.";
const REPLACEMENT_UNVERIFIED_MESSAGE = "Voice connection could not be verified. Check your network and voice access, then start again.";
const TRANSPORT_LOST_MESSAGE = "Voice connection was lost. Check your network and start voice again.";
const AUDIO_INTERRUPTED_MESSAGE = "Browser audio was interrupted. Check audio permissions and start voice again.";

export interface VoiceTarget {
  readonly identity: string;
  readonly label: string;
}

/** Upper bound on provisional live-speech rows kept per call. */
export const LIVE_SPEECH_MAX_ITEMS = 400;

export interface VoiceSessionSnapshot {
  readonly phase: "idle" | "requesting" | "connecting" | "active" | "closing" | "error";
  readonly target: VoiceTarget | null;
  readonly microphoneMuted: boolean;
  readonly speakerMuted: boolean;
  readonly error: string | null;
  readonly notice: string | null;
  readonly connectionStage?: "opening" | "transport" | "recovery";
  /** Active call whose WebRTC transport reported `disconnected` and is inside the reconnect grace. */
  readonly reconnecting?: boolean;
  readonly contextPreparation?: VoiceContextPreparation | null;
  readonly contextStatusError?: string | null;
  /**
   * Provisional speech for the active call, keyed by speaker and provider item id,
   * never persisted or sent anywhere, and dropped on every call end. User items come
   * from item-keyed input transcript deltas on the WebRTC data channel. Assistant
   * items come only from the gateway's provisional captions
   * (`meta/console/voice/captions`), keyed by the segment item id the committed
   * row later lists in `realtime_origin.provider_item_ids`; data-channel output
   * transcript deltas carry no such id and are not used. The console renders
   * unmatched items as distinct "live" rows. Canonical history replaces items only
   * through matching session, channel and provider item identities; call end
   * clears any remaining provisional rows.
   */
  readonly liveSpeech: readonly LiveSpeechItem[];
  /** Channel id of the active call, when one is active. */
  readonly activeChannelId?: string | null;
}

/** One provisional spoken item (user or assistant) accumulated from deltas. */
export interface LiveSpeechItem {
  readonly itemId: string;
  readonly speaker: "user" | "assistant";
  readonly text: string;
  readonly startedAt: number;
  readonly final: boolean;
}

/**
 * Readiness is tri-state: the gateway answered positively, the gateway answered negatively
 * (unavailable, unauthorized, unknown method), or the poll itself failed and nothing is known.
 */
export type VoiceAvailability = "available" | "unavailable" | "unknown";

/**
 * The gateway's readiness answer with its typed detail. `reason` is present only when voice is
 * unavailable because another owner holds the gateway's single live voice path; `holder` then
 * names that owner. Plain unavailability (no host, no credential, unauthorized) has no reason.
 */
export interface VoiceReadinessDetail {
  readonly availability: VoiceAvailability;
  readonly reason?: "external_live_active";
  readonly holder?: { readonly identity: string; readonly channelId?: string };
}

/** Browser seams are injectable so lifecycle tests require neither hardware nor credentials. */
export interface VoiceSessionEnvironment {
  voiceAvailable(identity: string): Promise<VoiceAvailability>;
  createAudioContext(): AudioContext;
  getUserMedia(): Promise<MediaStream>;
  createPeerConnection(): RTCPeerConnection;
  createMediaStream(tracks?: MediaStreamTrack[]): MediaStream;
  createAudioElement(): HTMLAudioElement;
  rpc(method: string, params: Record<string, unknown>, timeoutMs: number): Promise<unknown>;
  pagehideClose(params: Record<string, unknown>): void;
  onPagehide(listener: () => void): () => void;
  /** Fires on `pageshow` or when the document becomes visible again (bfcache restore). */
  onPageshow(listener: () => void): () => void;
  now(): number;
  randomId(): string;
  setTimeout(callback: () => void, milliseconds: number): ReturnType<typeof setTimeout>;
  clearTimeout(timer: ReturnType<typeof setTimeout>): void;
}

/**
 * Require a fresh positive readiness result fenced to the exact requested target.
 * A gateway answer (positive, negative, or a typed rejection) is definite; a transport
 * failure (network, timeout, 5xx, 429) is `unknown` so callers can keep the last known state.
 */
export async function queryVoiceAvailability(
  baseUrl: string,
  identity: string,
  signal?: AbortSignal,
): Promise<VoiceAvailability> {
  return (await queryVoiceReadiness(baseUrl, identity, signal)).availability;
}

/**
 * The gateway's typed "the check did not finish within its budget" answer. It is not a
 * readiness answer: callers keep what they knew and retry.
 */
export const VOICE_READINESS_TIMED_OUT_KIND = "voice_readiness_timed_out";

function isReadinessTimedOut(error: unknown): boolean {
  const rpcError = (error as { rpcError?: { data?: { kind?: unknown } } } | null)?.rpcError;
  return rpcError?.data?.kind === VOICE_READINESS_TIMED_OUT_KIND;
}

type ReadinessWire = {
  identity?: unknown;
  available?: unknown;
  reason?: unknown;
  holder?: { identity?: unknown; channel_id?: unknown } | null;
} | null;

/** Like {@link queryVoiceAvailability}, keeping the typed reason and holder when the gateway sends them. */
export async function queryVoiceReadiness(
  baseUrl: string,
  identity: string,
  signal?: AbortSignal,
): Promise<VoiceReadinessDetail> {
  if (!identity?.trim()) return { availability: "unavailable" };
  let readiness: ReadinessWire;
  try {
    readiness = await callConsoleRpc<ReadinessWire>(
      baseUrl, "meta/console/voice/readiness", { identity }, VOICE_TEARDOWN_TIMEOUT_MS, signal,
    );
  } catch (error) {
    return {
      availability: isReadinessTimedOut(error) || isTransientRpcFailure(error) ? "unknown" : "unavailable",
    };
  }
  if (readiness?.identity !== identity) return { availability: "unavailable" };
  if (readiness.available === true) return { availability: "available" };
  if (readiness.reason !== "external_live_active") return { availability: "unavailable" };
  const holderIdentity = readiness.holder?.identity;
  const channelId = readiness.holder?.channel_id;
  return {
    availability: "unavailable",
    reason: "external_live_active",
    ...(typeof holderIdentity === "string"
      ? { holder: { identity: holderIdentity, ...(typeof channelId === "string" ? { channelId } : {}) } }
      : {}),
  };
}

/** Message for a call the gateway closed because another live owner took the voice path. */
export const VOICE_SUPERSEDED_MESSAGE =
  "Voice moved to the external live channel. Start voice again to take it back.";
/** The same, when another console call took it. */
export const VOICE_SUPERSEDED_BY_CONSOLE_MESSAGE =
  "Voice moved to another console call. Start voice again to take it back.";

/** Pick the supersession message from the gateway's typed `data.reason`. */
export function voiceSupersededMessage(reason: unknown): string {
  return reason === "superseded_by_console_voice" || reason === "replaced_by_same_owner"
    ? VOICE_SUPERSEDED_BY_CONSOLE_MESSAGE
    : VOICE_SUPERSEDED_MESSAGE;
}

/**
 * Classify a failed control-plane call. Anything the gateway actually answered (a JSON-RPC
 * error, or a non-retryable HTTP status) is definite; network failures, timeouts, 5xx and 429
 * are transient and worth retrying while media keeps flowing.
 */
export function isTransientRpcFailure(error: unknown): boolean {
  if (error instanceof Cancelled) return false;
  if (error instanceof VoiceTimeout || error instanceof TransientRpcFailure) return true;
  if (error instanceof VoiceError) return false;
  if ((error as { rpcError?: unknown } | null)?.rpcError !== undefined) return false;
  const status = httpStatusCode(error);
  if (status !== null) return status === 429 || status >= 500;
  return true;
}

function browserEnvironment(baseUrl: string): VoiceSessionEnvironment {
  return {
    voiceAvailable: (identity) => queryVoiceAvailability(baseUrl, identity),
    createAudioContext: () => {
      if (typeof AudioContext === "undefined") {
        throw new VoiceError("Voice requires a browser with Web Audio and WebRTC support.");
      }
      return new AudioContext();
    },
    getUserMedia: () => {
      if (!globalThis.navigator?.mediaDevices?.getUserMedia) {
        throw new VoiceError("Microphone access requires HTTPS or localhost and a supported browser.");
      }
      return navigator.mediaDevices.getUserMedia({
        audio: { echoCancellation: true, noiseSuppression: true, autoGainControl: true },
        video: false,
      });
    },
    createPeerConnection: () => new RTCPeerConnection(),
    createMediaStream: (tracks) => new MediaStream(tracks ?? []),
    createAudioElement: () => document.createElement("audio"),
    rpc: (method, params, timeoutMs) => callConsoleRpc(baseUrl, method, params, timeoutMs),
    pagehideClose: (params) => {
      // Keepalive is needed specifically during document unloading, unlike ordinary console RPC.
      void fetch(`${baseUrl}${CONSOLE_RPC_PATHS.jsonRpc}`, {
        method: "POST",
        headers: { "content-type": "application/json" },
        body: JSON.stringify({
          jsonrpc: "2.0", id: "voice-pagehide", method: "meta/console/voice/close", params,
        }),
        keepalive: true,
      }).catch(() => {});
    },
    onPagehide: (listener) => {
      window.addEventListener("pagehide", listener);
      return () => window.removeEventListener("pagehide", listener);
    },
    onPageshow: (listener) => {
      const visible = () => {
        if (document.visibilityState === "visible") listener();
      };
      window.addEventListener("pageshow", listener);
      document.addEventListener("visibilitychange", visible);
      return () => {
        window.removeEventListener("pageshow", listener);
        document.removeEventListener("visibilitychange", visible);
      };
    },
    now: () => Date.now(),
    randomId: () => crypto.randomUUID(),
    setTimeout: (callback, milliseconds) => setTimeout(callback, milliseconds),
    clearTimeout: (timer) => clearTimeout(timer),
  };
}

class VoiceError extends Error {}
/** A locally bounded wait expired; distinct from a protocol violation so it can be retried. */
class VoiceTimeout extends VoiceError {}
/** Every scheduled gateway close attempt failed; media is already released and the request stays retained. */
export class VoiceCloseUnconfirmed extends VoiceError {
  constructor(readonly attempts: number, readonly cause: unknown) {
    super(`The gateway has not confirmed voice closure after ${attempts} attempts.`);
  }
}
/** Tagged at the RPC boundary only, so local parse and protocol failures stay definite. */
class TransientRpcFailure extends Error {}
class Cancelled extends Error {}

/** One loop's run of consecutive transient control-plane failures. */
interface FailureWindow {
  failures: number;
  timer: ReturnType<typeof setTimeout>;
}

function parseReplacement(raw: unknown): PendingLiveChannelHandle | null {
  if (!raw || typeof raw !== "object" || Array.isArray(raw)) {
    throw new VoiceError("The gateway returned an invalid voice recovery response.");
  }
  const result = raw as Record<string, unknown>;
  const keys = result.required === false
    ? ["required"]
    : ["required", "reason", "replacement", "canonical_seed_cursor"];
  if (Object.keys(result).some((key) => !keys.includes(key))) {
    throw new VoiceError("The gateway returned an invalid voice recovery response.");
  }
  if (result.required === false) return null;
  if (
    result.required !== true ||
    !["canonical_context", "delegation_result"].includes(String(result.reason)) ||
    typeof result.canonical_seed_cursor !== "number" ||
    !Number.isSafeInteger(result.canonical_seed_cursor) ||
    result.canonical_seed_cursor < 0
  ) throw new VoiceError("The gateway returned an invalid voice recovery response.");
  return parsePendingLiveChannelHandle(result.replacement);
}

function voiceError(error: unknown, stage?: VoiceSessionSnapshot["connectionStage"]): string {
  if (error instanceof VoiceError) return error.message;
  const name = error instanceof Error ? error.name : "";
  if (name === "NotAllowedError" || name === "PermissionDeniedError") {
    return "Microphone permission was denied. Allow microphone access in your browser, then start voice again.";
  }
  if (name === "NotFoundError" || name === "DevicesNotFoundError") {
    return "No microphone was found. Connect a microphone and start voice again.";
  }
  if (name === "NotReadableError") {
    return "The microphone is unavailable or in use. Check your device and browser settings.";
  }
  if (jsonRpcErrorCode(error) === -32030) {
    return "You do not have permission to start voice for this agent.";
  }
  if (jsonRpcErrorCode(error) === -32601 || /not configured|unavailable|not supported/i.test(errorMessage(error))) {
    return "Voice is unavailable. Ask your administrator to enable OpenAI GPT Live on this gateway.";
  }
  if (stage === "opening") {
    return "The gateway could not start voice for this agent. Check your voice access and the gateway log before trying again.";
  }
  // Never expose upstream response bodies, SDP, or bootstrap receipts in UI errors.
  return "Voice could not connect. Check your network and gateway configuration, then try again.";
}

interface Attempt {
  readonly target: VoiceTarget;
  readonly requestId: string;
  readonly abort: AbortController;
  deadline: number;
  connectionStage?: VoiceSessionSnapshot["connectionStage"];
  context?: AudioContext;
  stream?: MediaStream;
  remote?: MediaStream;
  peer?: RTCPeerConnection;
  channel?: RTCDataChannel;
  audio?: HTMLAudioElement;
  microphone?: AnalyserNode;
  speaker?: AnalyserNode;
  gain?: GainNode;
  /** Barge-in gate between the speaker analyser and `gain` (duck/restore hints). */
  playbackGate?: GainNode;
  nodes: AudioNode[];
  pending?: PendingLiveChannelHandle;
  readiness?: LivePlaybackOwnerReadiness;
  active?: ActiveLiveChannelHandle;
  openSent: boolean;
  localClosed: boolean;
  teardown?: Promise<void>;
  sampleTimer?: ReturnType<typeof setTimeout>;
  silenceTimer?: ReturnType<typeof setTimeout>;
  replacementTimer?: ReturnType<typeof setTimeout>;
  replacementPolling?: boolean;
  replacementFailure?: FailureWindow;
  reconnectTimer?: ReturnType<typeof setTimeout>;
  audioResumeTimer?: ReturnType<typeof setTimeout>;
  transportLoss?: string;
  recoveryDeadline?: number;
  recoveryTimer?: ReturnType<typeof setTimeout>;
  activityReportedAt?: number;
  activityReporting?: boolean;
  activityDirty?: boolean;
  activityTimer?: ReturnType<typeof setTimeout>;
  activityRetryAt?: number;
  activityFailure?: FailureWindow;
  contextObservation?: { abort: AbortController; timer?: ReturnType<typeof setTimeout> };
  captionObservation?: { abort: AbortController; timer?: ReturnType<typeof setTimeout> };
  /** Wakes the activation transport wait on peer or data channel state changes. */
  onTransportChange?: () => void;
  lastActivity: number;
}

export interface VoiceSession {
  subscribe(listener: () => void): () => void;
  getSnapshot(): VoiceSessionSnapshot;
  start(target: VoiceTarget): Promise<void>;
  close(): Promise<void>;
  toggleMicrophone(): void;
  toggleSpeaker(): void;
  dispose(): void;
  sampleWaveform(source: "microphone" | "speaker", target: Float32Array<ArrayBuffer>): void;
}

/**
 * Own one controller at the console root, not in an agent pane. Navigation is deliberately
 * absent from this API: only explicit start/close replaces the selected voice target.
 * The console/voice wrappers require a separately composed authenticated HTTP backend;
 * legacy strict stdio live capabilities alone do not make this controller usable.
 */
export function createVoiceSession(
  baseUrl: string,
  environment?: VoiceSessionEnvironment,
): VoiceSession {
  const env = environment ?? browserEnvironment(baseUrl);
  const listeners = new Set<() => void>();
  let snapshot: VoiceSessionSnapshot = {
    phase: "idle", target: null, microphoneMuted: false, speakerMuted: false,
    error: null, notice: null, liveSpeech: [], activeChannelId: null,
  };
  let current: Attempt | undefined;
  let disposed = false;
  let pageHidden = false;
  let removePagehide: (() => void) | undefined;
  let removePageshow: (() => void) | undefined;
  let serial: Promise<void> = Promise.resolve();
  let teardownBlock: Attempt | undefined;
  const retainedAttempts = new Set<Attempt>();

  const publish = (patch: Partial<VoiceSessionSnapshot>) => {
    snapshot = { ...snapshot, ...patch };
    for (const listener of listeners) listener();
  };
  const owns = (attempt: Attempt) => current === attempt && !attempt.abort.signal.aborted && !disposed;
  const assertOwns = (attempt: Attempt) => {
    if (!owns(attempt)) throw new Cancelled();
  };
  const enqueue = (operation: () => Promise<void>): Promise<void> => {
    const next = serial.then(operation, operation);
    serial = next.catch(() => {});
    return serial;
  };

  function bounded<T>(
    promise: Promise<T>,
    milliseconds: number,
    signal?: AbortSignal,
    timeoutMessage = "Voice connection timed out. Check microphone permissions and your network, then try again.",
  ): Promise<T> {
    return new Promise<T>((resolve, reject) => {
      let settled = false;
      const finish = (action: () => void) => {
        if (settled) return;
        settled = true;
        env.clearTimeout(timer);
        signal?.removeEventListener("abort", cancelled);
        action();
      };
      const cancelled = () => finish(() => reject(new Cancelled()));
      const timer = env.setTimeout(() => finish(() => reject(new VoiceTimeout(timeoutMessage))), Math.max(0, milliseconds));
      signal?.addEventListener("abort", cancelled, { once: true });
      promise.then((value) => finish(() => resolve(value)), (error) => finish(() => reject(error)));
      if (signal?.aborted) cancelled();
    });
  }

  /**
   * A steady-state control-plane call (heartbeat, replacement poll). Transport failures are
   * tagged here, at the boundary, so the loops can retry them without mistaking a malformed
   * gateway reply or a typed rejection for a network blip.
   */
  async function controlPlaneCall(
    attempt: Attempt,
    method: string,
    params: Record<string, unknown>,
  ): Promise<unknown> {
    try {
      return await bounded(
        env.rpc(method, params, VOICE_TEARDOWN_TIMEOUT_MS),
        VOICE_TEARDOWN_TIMEOUT_MS,
        attempt.abort.signal,
      );
    } catch (error) {
      if (isTransientRpcFailure(error)) throw new TransientRpcFailure(errorMessage(error));
      throw error;
    }
  }

  function connecting<T>(attempt: Attempt, promise: Promise<T>): Promise<T> {
    return bounded(
      promise,
      attempt.deadline - env.now(),
      attempt.abort.signal,
      attempt.connectionStage === "opening"
        ? "Starting voice timed out. Check your voice access and the gateway, then try again."
        : attempt.connectionStage === "transport"
          ? "The voice media connection timed out. Check the gateway and your network, then try again."
          : undefined,
    );
  }

  async function pollDelay(attempt: Attempt) {
    let timer: ReturnType<typeof setTimeout> | undefined;
    try {
      await connecting(attempt, new Promise<void>((resolve) => {
        timer = env.setTimeout(resolve, POLL_INTERVAL_MS);
      }));
    } finally {
      if (timer !== undefined) env.clearTimeout(timer);
    }
  }

  function cleanupPeer(attempt: Attempt) {
    stopContextObservation(attempt);
    stopCaptionObservation(attempt);
    clearReconnectGrace(attempt);
    const peer = attempt.peer;
    const channel = attempt.channel;
    attempt.peer = undefined;
    attempt.channel = undefined;
    attempt.onTransportChange = undefined;
    if (channel) {
      channel.onmessage = null;
      channel.onopen = null;
      channel.onclose = null;
      channel.onerror = null;
      channel.close();
    }
    if (peer) {
      peer.ontrack = null;
      peer.onconnectionstatechange = null;
      peer.oniceconnectionstatechange = null;
      peer.close();
    }
    for (const track of attempt.stream?.getAudioTracks() ?? []) track.enabled = false;
    for (const track of attempt.remote?.getTracks() ?? []) {
      track.onended = null;
      track.enabled = false;
      track.stop();
    }
    for (const node of attempt.nodes) node.disconnect();
    attempt.nodes = [];
    if (attempt.audio) {
      attempt.audio.pause();
      attempt.audio.srcObject = null;
    }
    attempt.audio = undefined;
    attempt.remote = undefined;
    attempt.microphone = undefined;
    attempt.speaker = undefined;
    attempt.gain = undefined;
    attempt.playbackGate = undefined;
  }

  function quiesceLocal(attempt: Attempt) {
    attempt.abort.abort();
    stopContextObservation(attempt);
    stopCaptionObservation(attempt);
    if (attempt.sampleTimer !== undefined) env.clearTimeout(attempt.sampleTimer);
    if (attempt.silenceTimer !== undefined) env.clearTimeout(attempt.silenceTimer);
    if (attempt.replacementTimer !== undefined) env.clearTimeout(attempt.replacementTimer);
    if (attempt.recoveryTimer !== undefined) env.clearTimeout(attempt.recoveryTimer);
    if (attempt.activityTimer !== undefined) env.clearTimeout(attempt.activityTimer);
    if (attempt.audioResumeTimer !== undefined) env.clearTimeout(attempt.audioResumeTimer);
    attempt.audioResumeTimer = undefined;
    clearReconnectGrace(attempt);
    clearFailureWindow(attempt, "replacementFailure");
    clearFailureWindow(attempt, "activityFailure");
    attempt.activityDirty = false;
    for (const track of attempt.stream?.getTracks() ?? []) track.enabled = false;
    if (attempt.gain) attempt.gain.gain.value = 0;
    attempt.audio?.pause();
  }

  function cleanupLocal(attempt: Attempt) {
    if (attempt.localClosed) return;
    quiesceLocal(attempt);
    attempt.localClosed = true;
    cleanupPeer(attempt);
    for (const track of attempt.stream?.getTracks() ?? []) {
      track.onended = null;
      track.enabled = false;
      track.stop();
    }
    if (attempt.context && attempt.context.state !== "closed") {
      attempt.context.onstatechange = null;
      void attempt.context.close().catch(() => {});
    }
  }

  function closeParams(attempt: Attempt): Record<string, unknown> {
    return { identity: attempt.target.identity, request_id: attempt.requestId };
  }

  /** A close failure the gateway can resolve on its own: transport blips and gateway-side faults. */
  function closeRetryable(error: unknown): boolean {
    return isTransientRpcFailure(error) || jsonRpcErrorCode(error) === GATEWAY_INTERNAL_ERROR_CODE;
  }

  function delay(milliseconds: number): Promise<void> {
    return new Promise((resolve) => { env.setTimeout(resolve, milliseconds); });
  }

  /**
   * One request-scoped close, retried on a fixed backoff schedule. The first attempt runs with
   * muted WebRTC still connected so provider acknowledgements can drain; every later attempt runs
   * with local media already released. Caller-side rejections fail at once; the schedule caps
   * gateway-side failures so a dead remote never turns into an unbounded retry loop.
   */
  async function confirmClose(attempt: Attempt): Promise<void> {
    let failure: unknown;
    for (let index = 0; index <= VOICE_CLOSE_RETRY_DELAYS_MS.length; index++) {
      if (index > 0) await delay(VOICE_CLOSE_RETRY_DELAYS_MS[index - 1]);
      try {
        const raw = await bounded(
          env.rpc("meta/console/voice/close", closeParams(attempt), VOICE_TEARDOWN_TIMEOUT_MS),
          VOICE_TEARDOWN_TIMEOUT_MS,
        );
        const result = parseExperimentalLiveChannelStatus(raw);
        if (result.phase !== "closed" && result.phase !== "revoked") {
          throw new VoiceError("The gateway did not confirm voice closure.");
        }
        return;
      } catch (error) {
        cleanupLocal(attempt);
        if (!closeRetryable(error)) throw error;
        failure = error;
      }
    }
    throw new VoiceCloseUnconfirmed(VOICE_CLOSE_RETRY_DELAYS_MS.length + 1, failure);
  }

  async function teardown(attempt: Attempt) {
    quiesceLocal(attempt);
    if (!attempt.openSent) {
      cleanupLocal(attempt);
      retainedAttempts.delete(attempt);
      return;
    }
    if (attempt.teardown) return attempt.teardown;
    attempt.teardown = (async () => {
      // The request-scoped close also fences an open whose response has not reached the browser.
      try {
        await confirmClose(attempt);
        if (teardownBlock === attempt) teardownBlock = undefined;
        retainedAttempts.delete(attempt);
      } finally {
        cleanupLocal(attempt);
      }
    })();
    try {
      await attempt.teardown;
    } catch (error) {
      attempt.teardown = undefined;
      teardownBlock = attempt;
      throw error;
    }
  }

  async function stop(attempt: Attempt, error: string | null = null, notice: string | null = null) {
    quiesceLocal(attempt);
    if (current === attempt) publish({ phase: "closing", error, notice });
    try {
      await teardown(attempt);
      if (current === attempt) {
        current = undefined;
        publish({
          phase: error ? "error" : "idle", target: error ? attempt.target : null, error, notice,
          reconnecting: false, contextPreparation: undefined, contextStatusError: null,
          liveSpeech: [], activeChannelId: null,
        });
      }
    } catch (failure) {
      if (current === attempt) {
        const attempts = failure instanceof VoiceCloseUnconfirmed ? ` after ${failure.attempts} attempts` : "";
        publish({
          phase: "error",
          error: `${error ? `${error} ` : ""}Your microphone and speaker are off, but the gateway has not confirmed voice closure${attempts}. Close again to retry before starting another session.`,
          notice: null,
        });
      }
    }
  }

  function fail(attempt: Attempt, message: string) {
    if (!owns(attempt)) return;
    quiesceLocal(attempt);
    void enqueue(() => stop(attempt, message));
  }

  function transportLost(attempt: Attempt, peer: RTCPeerConnection, message: string) {
    if (!owns(attempt) || attempt.peer !== peer) return;
    if (snapshot.phase !== "active") {
      fail(attempt, message);
      return;
    }
    attempt.transportLoss = message;
    attempt.recoveryDeadline = env.now() + VOICE_RECOVERY_TIMEOUT_MS;
    if (attempt.sampleTimer !== undefined) env.clearTimeout(attempt.sampleTimer);
    if (attempt.replacementTimer !== undefined) env.clearTimeout(attempt.replacementTimer);
    cleanupPeer(attempt);
    attempt.recoveryTimer = env.setTimeout(() => {
      fail(attempt, RECOVERY_TIMEOUT_MESSAGE);
    }, VOICE_RECOVERY_TIMEOUT_MS);
    attempt.connectionStage = "recovery";
    publish({ phase: "connecting", connectionStage: "recovery", reconnecting: false });
    // The owner closes transport before preparing replacement credentials and summary.
    // Keep polling while gated, with one fixed deadline for discovery and activation.
    requestReplacement(attempt);
  }

  function clearReconnectGrace(attempt: Attempt) {
    if (attempt.reconnectTimer === undefined) return;
    env.clearTimeout(attempt.reconnectTimer);
    attempt.reconnectTimer = undefined;
    if (current === attempt && snapshot.reconnecting) publish({ reconnecting: false });
  }

  // ICE `disconnected` self-heals often enough that destroying the peer would turn a
  // sub-second blip into a full owner-issued recovery. Hold the peer for a bounded grace.
  function transportInterrupted(attempt: Attempt, peer: RTCPeerConnection, message: string) {
    if (!owns(attempt) || attempt.peer !== peer || attempt.reconnectTimer !== undefined) return;
    attempt.reconnectTimer = env.setTimeout(() => {
      attempt.reconnectTimer = undefined;
      transportLost(attempt, peer, message);
    }, VOICE_TRANSPORT_RECONNECT_GRACE_MS);
    if (snapshot.phase === "active") publish({ reconnecting: true });
  }

  function transportRestored(attempt: Attempt, peer: RTCPeerConnection) {
    if (attempt.peer !== peer) return;
    clearReconnectGrace(attempt);
  }

  /**
   * Record one transient control-plane failure for a loop and return the retry delay.
   * The first failure in a run arms one tolerance deadline; a later success disarms it.
   */
  function recordTransientFailure(
    attempt: Attempt,
    key: "replacementFailure" | "activityFailure",
    message: string,
  ): number {
    const run = attempt[key] ?? (attempt[key] = {
      failures: 0,
      timer: env.setTimeout(() => {
        attempt[key] = undefined;
        fail(attempt, message);
      }, VOICE_RPC_FAILURE_TOLERANCE_MS),
    });
    run.failures += 1;
    const base = key === "activityFailure" ? VOICE_ACTIVITY_REPORT_INTERVAL_MS : VOICE_REPLACEMENT_POLL_INTERVAL_MS;
    return Math.min(base * 2 ** (run.failures - 1), VOICE_RPC_RETRY_BACKOFF_MAX_MS);
  }

  function clearFailureWindow(attempt: Attempt, key: "replacementFailure" | "activityFailure") {
    const run = attempt[key];
    if (!run) return;
    env.clearTimeout(run.timer);
    attempt[key] = undefined;
  }

  function activity(attempt: Attempt) {
    if (!owns(attempt) || snapshot.phase !== "active") return;
    attempt.lastActivity = env.now();
    scheduleSilence(attempt);
    attempt.activityDirty = true;
    flushActivity(attempt);
  }

  function flushActivity(attempt: Attempt) {
    if (!owns(attempt) || !attempt.activityDirty || attempt.activityReporting) return;
    const due = Math.max(
      attempt.activityReportedAt === undefined ? 0 : attempt.activityReportedAt + VOICE_ACTIVITY_REPORT_INTERVAL_MS,
      attempt.activityRetryAt ?? 0,
    );
    const delay = due - env.now();
    if (delay > 0) {
      if (attempt.activityTimer === undefined) {
        attempt.activityTimer = env.setTimeout(() => {
          attempt.activityTimer = undefined;
          flushActivity(attempt);
        }, delay);
      }
      return;
    }
    if (attempt.activityTimer !== undefined) env.clearTimeout(attempt.activityTimer);
    attempt.activityTimer = undefined;
    attempt.activityRetryAt = undefined;
    attempt.activityReportedAt = env.now();
    attempt.activityDirty = false;
    attempt.activityReporting = true;
    void reportActivity(attempt);
  }

  async function reportActivity(attempt: Attempt) {
    try {
      const result = await controlPlaneCall(attempt, "meta/console/voice/activity", closeParams(attempt));
      assertOwns(attempt);
      if (
        !result || typeof result !== "object" || Array.isArray(result) ||
        (result as Record<string, unknown>).accepted !== true
      ) throw new VoiceError("The gateway did not accept voice activity.");
      clearFailureWindow(attempt, "activityFailure");
    } catch (error) {
      if (error instanceof Cancelled || !owns(attempt)) return;
      if (error instanceof TransientRpcFailure) {
        // Audio keeps flowing; the heartbeat is re-sent with backoff inside one tolerance window.
        const delay = recordTransientFailure(attempt, "activityFailure", ACTIVITY_UNCONFIRMED_MESSAGE);
        attempt.activityDirty = true;
        attempt.activityRetryAt = env.now() + delay;
        return;
      }
      fail(attempt, ACTIVITY_UNCONFIRMED_MESSAGE);
    } finally {
      attempt.activityReporting = false;
      flushActivity(attempt);
    }
  }

  function scheduleSilence(attempt: Attempt) {
    if (attempt.silenceTimer !== undefined) env.clearTimeout(attempt.silenceTimer);
    attempt.silenceTimer = env.setTimeout(() => {
      if (!owns(attempt)) return;
      if (env.now() - attempt.lastActivity < VOICE_SILENCE_TIMEOUT_MS) {
        scheduleSilence(attempt);
        return;
      }
      quiesceLocal(attempt);
      void enqueue(() => stop(attempt, null, "Voice closed after 15 minutes of silence."));
    }, VOICE_SILENCE_TIMEOUT_MS - (env.now() - attempt.lastActivity));
  }

  function sampleActivity(attempt: Attempt) {
    const samples = new Float32Array(2048);
    const hasSignal = (analyser: AnalyserNode | undefined, threshold: number) => {
      if (!analyser) return false;
      analyser.getFloatTimeDomainData(samples);
      let power = 0;
      for (const value of samples) power += value * value;
      return power / samples.length > threshold * threshold;
    };
    const sample = () => {
      if (!owns(attempt) || snapshot.phase !== "active") return;
      if (
        (!snapshot.microphoneMuted && hasSignal(attempt.microphone, 0.015)) ||
        hasSignal(attempt.speaker, 0.005)
      ) activity(attempt);
      attempt.sampleTimer = env.setTimeout(sample, SAMPLE_INTERVAL_MS);
    };
    sample();
  }

  function gates(attempt: Attempt) {
    const active = owns(attempt) && snapshot.phase === "active" && !!attempt.active;
    for (const track of attempt.stream?.getAudioTracks() ?? []) {
      track.enabled = active && !snapshot.microphoneMuted;
    }
    if (attempt.gain) attempt.gain.gain.value = active && !snapshot.speakerMuted ? 1 : 0;
  }

  function stopContextObservation(attempt: Attempt) {
    const observation = attempt.contextObservation;
    attempt.contextObservation = undefined;
    observation?.abort.abort();
    if (observation?.timer !== undefined) env.clearTimeout(observation.timer);
  }

  function observeContext(attempt: Attempt) {
    stopContextObservation(attempt);
    if (!owns(attempt) || !attempt.active || snapshot.phase !== "active") return;
    const channelId = attempt.active.channelId;
    const observation: NonNullable<Attempt["contextObservation"]> = { abort: new AbortController() };
    attempt.contextObservation = observation;
    const isCurrent = () => owns(attempt) && snapshot.phase === "active" &&
      attempt.contextObservation === observation && attempt.active?.channelId === channelId;
    publish({ contextPreparation: null, contextStatusError: null });
    async function read() {
      if (!isCurrent()) return;
      let delay: number | undefined;
      try {
        const raw = await bounded(
          env.rpc("meta/console/voice/context_status", {
            ...closeParams(attempt), channel_id: channelId,
          }, VOICE_TEARDOWN_TIMEOUT_MS),
          VOICE_TEARDOWN_TIMEOUT_MS,
          observation.abort.signal,
        );
        if (!isCurrent()) return;
        const preparation = parseVoiceContextStatus(raw, {
          identity: attempt.target.identity, requestId: attempt.requestId, channelId,
        });
        if (snapshot.contextStatusError || JSON.stringify(snapshot.contextPreparation) !== JSON.stringify(preparation)) {
          publish({ contextPreparation: preparation, contextStatusError: null });
        }
        if (preparation.phase === "preparing") delay = CONTEXT_POLL_INTERVAL_MS;
      } catch (error) {
        if (error instanceof Cancelled || !isCurrent()) return;
        publish({
          contextStatusError: "Agent context status is unavailable. Voice remains connected; checking again automatically.",
        });
        delay = CONTEXT_RETRY_INTERVAL_MS;
      }
      if (isCurrent() && delay !== undefined) {
        observation.timer = env.setTimeout(() => {
          observation.timer = undefined;
          void read();
        }, delay);
      }
    }
    void read();
  }

  function consumeMessage(attempt: Attempt, data: unknown) {
    if (!owns(attempt) || typeof data !== "string") return;
    let event: Record<string, unknown>;
    try {
      const parsed: unknown = JSON.parse(data);
      if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return;
      event = parsed as Record<string, unknown>;
    } catch {
      return;
    }
    if (event.type === "error") {
      fail(attempt, "The voice provider reported an error. Check gateway configuration and start voice again.");
      return;
    }
    if (!attempt.active || snapshot.phase !== "active") return;
    const type = event.type;
    if (
      !snapshot.microphoneMuted &&
      (type === "input_audio_buffer.speech_started" || type === "input_audio_buffer.speech_stopped")
    ) activity(attempt);
    // Text and transcript deltas are not evidence of current audio activity.
    // Provider-managed/unmeasured playback is settled by the shared owner, never by
    // inferred browser completions or fabricated output IDs. Assistant speech is
    // shown from the gateway's item-keyed captions (`observeCaptions`), never from
    // `session.output_transcript.*`, whose events name no committed item.
    if (type === "session.input_transcript.delta") {
      const itemId = typeof event.item_id === "string" ? event.item_id : null;
      const delta = typeof event.delta === "string" ? event.delta
        : typeof event.text === "string" ? event.text : null;
      if (itemId && delta !== null) accumulateLiveSpeech(itemId, "user", delta);
      return;
    }
    if (type === "session.input_transcript.done") {
      const itemId = typeof event.item_id === "string" ? event.item_id : null;
      if (itemId) finalizeLiveSpeech(itemId, "user", typeof event.text === "string" ? event.text : null);
    }
  }

  /**
   * Fold one transcript delta into the provisional live-speech list. Items
   * are keyed by speaker and provider item id so an interrupted or resumed item never
   * duplicates; the list is bounded so a long call cannot grow it without end
   * (canonical rows carry the committed history during and after the call).
   */
  function accumulateLiveSpeech(itemId: string, speaker: "user", delta: string) {
    const existing = snapshot.liveSpeech.find((item) => item.itemId === itemId && item.speaker === speaker);
    let next: LiveSpeechItem[];
    if (existing) {
      next = snapshot.liveSpeech.map((item) =>
        item.itemId === itemId && item.speaker === speaker ? { ...item, text: item.text + delta } : item,
      );
    } else {
      next = [
        ...snapshot.liveSpeech,
        { itemId, speaker, text: delta, startedAt: env.now(), final: false },
      ];
      if (next.length > LIVE_SPEECH_MAX_ITEMS) next = next.slice(next.length - LIVE_SPEECH_MAX_ITEMS);
    }
    publish({ liveSpeech: next });
  }

  function finalizeLiveSpeech(itemId: string, speaker: "user", text: string | null) {
    if (!snapshot.liveSpeech.some((item) => item.itemId === itemId && item.speaker === speaker)) return;
    publish({
      liveSpeech: snapshot.liveSpeech.map((item) =>
        item.itemId === itemId && item.speaker === speaker
          ? { ...item, final: true, text: text !== null && text.length >= item.text.length ? text : item.text }
          : item,
      ),
    });
  }

  /**
   * Apply one caption batch: a caption replaces its assistant item's text with the
   * segment's whole text so far (so repeats and reordering are harmless), and a
   * retraction drops the item. The committed row retires a caption in the chat pane
   * by its `realtime_origin.provider_item_ids`, never by text.
   */
  /**
   * Apply Meerkat's barge-in hint to the playback gate: duck to silence while the user
   * speaks over the assistant, restore afterwards, with a short ramp (the policy the
   * Meerkat TypeScript SDK's applyLiveAssistantPlaybackHint applies).
   */
  function applyPlaybackHint(attempt: Attempt, hint: VoicePlaybackHint) {
    const gate = attempt.playbackGate;
    const context = attempt.context;
    if (!gate || !context) return;
    const target = hint === "duck" ? VOICE_PLAYBACK_DUCKED_GAIN : VOICE_PLAYBACK_UNITY_GAIN;
    gate.gain.cancelScheduledValues(context.currentTime);
    gate.gain.setTargetAtTime(target, context.currentTime, VOICE_PLAYBACK_GAIN_TIME_CONSTANT_S);
  }

  function applyCaptions(attempt: Attempt, captions: readonly VoiceCaption[]) {
    let next: LiveSpeechItem[] = [...snapshot.liveSpeech];
    let changed = false;
    for (const caption of captions) {
      if (caption.kind === "playback_hint") {
        applyPlaybackHint(attempt, caption.hint);
        continue;
      }
      const index = next.findIndex((item) => item.speaker === "assistant" && item.itemId === caption.itemId);
      if (caption.kind === "retracted") {
        if (index >= 0) {
          next.splice(index, 1);
          changed = true;
        }
        continue;
      }
      if (index >= 0) {
        if (next[index].text !== caption.text) {
          next[index] = { ...next[index], text: caption.text };
          changed = true;
        }
        continue;
      }
      next.push({ itemId: caption.itemId, speaker: "assistant", text: caption.text, startedAt: env.now(), final: false });
      if (next.length > LIVE_SPEECH_MAX_ITEMS) next = next.slice(next.length - LIVE_SPEECH_MAX_ITEMS);
      changed = true;
    }
    if (changed) publish({ liveSpeech: next });
  }

  function stopCaptionObservation(attempt: Attempt) {
    const observation = attempt.captionObservation;
    attempt.captionObservation = undefined;
    observation?.abort.abort();
    if (observation?.timer !== undefined) env.clearTimeout(observation.timer);
  }

  /**
   * Long-poll the active channel's provisional assistant captions. Captions are display
   * state only: a failed read never affects the call, it is retried after a pause, and a
   * closed or replaced channel ends this observer (replacement starts a new one).
   */
  function observeCaptions(attempt: Attempt) {
    stopCaptionObservation(attempt);
    if (!owns(attempt) || !attempt.active || snapshot.phase !== "active") return;
    const channelId = attempt.active.channelId;
    const observation: NonNullable<Attempt["captionObservation"]> = { abort: new AbortController() };
    attempt.captionObservation = observation;
    const isCurrent = () => owns(attempt) && snapshot.phase === "active" &&
      attempt.captionObservation === observation && attempt.active?.channelId === channelId;
    let after = 0;
    const schedule = (delay: number) => {
      if (!isCurrent()) return;
      observation.timer = env.setTimeout(() => {
        observation.timer = undefined;
        void read();
      }, delay);
    };
    async function read() {
      if (!isCurrent()) return;
      const timeout = VOICE_CAPTION_WAIT_MS + VOICE_TEARDOWN_TIMEOUT_MS;
      let delay: number;
      try {
        const raw = await bounded(
          env.rpc(VOICE_CAPTIONS_METHOD, {
            ...closeParams(attempt), channel_id: channelId, after, wait_ms: VOICE_CAPTION_WAIT_MS,
          }, timeout),
          timeout,
          observation.abort.signal,
        );
        if (!isCurrent()) return;
        const batch = parseVoiceCaptions(raw, {
          identity: attempt.target.identity, requestId: attempt.requestId, channelId,
        }, after);
        after = batch.cursor;
        applyCaptions(attempt, batch.captions);
        delay = batch.captions.length > 0 ? 0 : VOICE_CAPTION_IDLE_DELAY_MS;
      } catch (error) {
        if (error instanceof Cancelled || !isCurrent()) return;
        const kind = (error as { rpcError?: { data?: { kind?: string } } } | null)?.rpcError?.data?.kind;
        // The call's own control plane reports closure; captions just stop.
        if (kind === "voice_closed" || kind === "voice_superseded" || kind === "voice_unavailable") return;
        delay = VOICE_CAPTION_RETRY_DELAY_MS;
      }
      schedule(delay);
    }
    void read();
  }

  function preparePeer(attempt: Attempt) {
    const context = attempt.context!;
    const stream = attempt.stream!;
    const peer = env.createPeerConnection();
    attempt.peer = peer;
    attempt.remote = env.createMediaStream();
    attempt.audio = env.createAudioElement();
    attempt.audio.autoplay = true;
    attempt.audio.muted = true;
    attempt.audio.srcObject = attempt.remote;
    const gain = context.createGain();
    gain.gain.value = 0;
    gain.connect(context.destination);
    attempt.gain = gain;
    attempt.nodes.push(gain);
    const playbackGate = context.createGain();
    playbackGate.gain.value = VOICE_PLAYBACK_UNITY_GAIN;
    playbackGate.connect(gain);
    attempt.playbackGate = playbackGate;
    attempt.nodes.push(playbackGate);
    const microphone = context.createAnalyser();
    microphone.fftSize = 2048;
    const source = context.createMediaStreamSource(stream);
    source.connect(microphone);
    attempt.microphone = microphone;
    attempt.nodes.push(source, microphone);
    peer.ontrack = (event) => {
      if (!owns(attempt) || attempt.peer !== peer) {
        event.track.stop();
        return;
      }
      if (event.track.kind !== "audio") {
        event.track.stop();
        return;
      }
      attempt.remote!.addTrack(event.track);
      event.track.onended = () => {
        transportLost(attempt, peer, "The voice audio stream ended. Start voice again.");
      };
      if (!attempt.speaker) {
        const speaker = context.createAnalyser();
        speaker.fftSize = 2048;
        const remoteSource = context.createMediaStreamSource(attempt.remote!);
        remoteSource.connect(speaker);
        speaker.connect(playbackGate);
        attempt.speaker = speaker;
        attempt.nodes.push(remoteSource, speaker);
      }
      void attempt.audio!.play().catch(() => {
        if (attempt.peer === peer) {
          fail(attempt, "Audio playback was blocked. Allow audio in your browser and start voice again.");
        }
      });
    };
    const connectionChanged = () => {
      if (attempt.peer !== peer) return;
      attempt.onTransportChange?.();
      const states: string[] = [peer.connectionState, peer.iceConnectionState];
      if (states.includes("failed") || states.includes("closed")) {
        transportLost(attempt, peer, TRANSPORT_LOST_MESSAGE);
      } else if (states.includes("disconnected")) {
        transportInterrupted(attempt, peer, TRANSPORT_LOST_MESSAGE);
      } else if (
        peer.connectionState === "connected" &&
        ["connected", "completed"].includes(peer.iceConnectionState)
      ) {
        transportRestored(attempt, peer);
      }
    };
    peer.onconnectionstatechange = connectionChanged;
    peer.oniceconnectionstatechange = connectionChanged;
    const channel = peer.createDataChannel("oai-events");
    attempt.channel = channel;
    channel.onmessage = (event) => {
      if (attempt.peer === peer) consumeMessage(attempt, event.data);
    };
    channel.onopen = () => {
      if (attempt.peer === peer) attempt.onTransportChange?.();
    };
    channel.onclose = () => {
      transportLost(attempt, peer, "The voice data connection closed. Start voice again.");
    };
    channel.onerror = () => {
      transportLost(attempt, peer, "The voice data connection failed. Start voice again.");
    };
    for (const track of stream.getAudioTracks()) {
      track.enabled = false;
      track.onended = () => {
        if (attempt.peer === peer) {
          fail(attempt, "Microphone access was lost. Check browser permissions and start voice again.");
        }
      };
      peer.addTrack(track, stream);
    }
  }

  // Initial open and owner-issued replacement share receipt activation, not open admission.
  // This helper never opens a channel or changes the target, mute preferences, or silence clock.
  async function activatePending(
    attempt: Attempt,
    pending: PendingLiveChannelHandle,
  ): Promise<ActiveLiveChannelHandle> {
    assertOwns(attempt);
    attempt.pending = pending;
    if (
      pending.targetIdentity !== attempt.target.identity ||
      pending.executionMode !== "client_context" ||
      pending.transport.transport !== "webrtc" ||
      pending.transport.answerMethod !== "live/webrtc/answer" ||
      !pending.capabilities.audioIn || !pending.capabilities.audioOut
    ) throw new VoiceError("The gateway returned an incompatible GPT Live voice session.");
    attempt.connectionStage = "transport";
    publish({ connectionStage: "transport" });
    assertOwns(attempt);
    preparePeer(attempt);
    const offer = await connecting(attempt, attempt.peer!.createOffer());
    assertOwns(attempt);
    await connecting(attempt, attempt.peer!.setLocalDescription(offer));
    assertOwns(attempt);
    const sdp = attempt.peer!.localDescription?.sdp;
    if (!sdp?.trim()) throw new VoiceError("The browser could not create a voice connection offer.");
    const readiness = parseLivePlaybackOwnerReadiness(await connecting(attempt,
      env.rpc("meta/live/playback_owner/register", {
        identity: pending.targetIdentity,
        channel_id: pending.channelId,
        pending_receipt: pending.pendingReceipt,
      }, VOICE_CONNECT_TIMEOUT_MS)));
    assertOwns(attempt);
    if (readiness.channelId !== pending.channelId) throw new VoiceError("Voice playback authority did not match the session.");
    attempt.readiness = readiness;
    const answer = await connecting(attempt, env.rpc("live/webrtc/answer", {
      identity: pending.targetIdentity,
      channel_id: pending.channelId,
      pending_receipt: pending.pendingReceipt,
      readiness_receipt: readiness.readinessReceipt,
      token: pending.transport.token,
      offer_sdp: sdp,
    }, VOICE_CONNECT_TIMEOUT_MS));
    assertOwns(attempt);
    if (!answer || typeof answer !== "object" ||
      typeof (answer as Record<string, unknown>).answer_sdp !== "string" ||
      !(answer as { answer_sdp: string }).answer_sdp.trim()) {
      throw new VoiceError("The gateway returned an invalid voice connection answer.");
    }
    await connecting(attempt, attempt.peer!.setRemoteDescription({
      type: "answer", sdp: (answer as { answer_sdp: string }).answer_sdp,
    }));
    assertOwns(attempt);
    const received = await connecting(attempt, env.rpc("meta/console/voice/answer_received", {
      ...closeParams(attempt),
      channel_id: pending.channelId,
    }, VOICE_CONNECT_TIMEOUT_MS));
    assertOwns(attempt);
    if (
      !received || typeof received !== "object" || Array.isArray(received) ||
      (received as Record<string, unknown>).accepted !== true
    ) throw new VoiceError("The gateway did not acknowledge voice answer delivery. Start voice again.");
    // Activation needs two independent facts: the gateway's activation receipt
    // and a usable media transport. The transport wait is event-driven (peer
    // and data channel state changes) rather than an RPC per 100 ms; both are
    // bounded by the same connect deadline.
    const [handle] = await Promise.all([
      awaitActivationReceipt(attempt, pending),
      connecting(attempt, awaitTransportReady(attempt)),
    ]);
    assertOwns(attempt);
    return handle;
  }

  async function awaitActivationReceipt(
    attempt: Attempt,
    pending: PendingLiveChannelHandle,
  ): Promise<ActiveLiveChannelHandle> {
    while (owns(attempt)) {
      const status = parseExperimentalLiveChannelStatus(await connecting(attempt,
        env.rpc("meta/live/status", {
          identity: pending.targetIdentity,
          channel_id: pending.channelId,
          pending_receipt: pending.pendingReceipt,
        }, VOICE_CONNECT_TIMEOUT_MS)));
      assertOwns(attempt);
      if (status.phase === "closed" || status.phase === "revoked") {
        throw new VoiceError("The gateway closed voice before activation. Start voice again.");
      }
      if (status.phase === "active") {
        if (
          status.handle.channelId !== pending.channelId ||
          status.handle.targetIdentity !== pending.targetIdentity ||
          status.handle.executionMode !== pending.executionMode
        ) throw new VoiceError("Voice activation authority did not match the requested agent.");
        return status.handle;
      }
      await pollDelay(attempt);
    }
    throw new Cancelled();
  }

  function transportReady(attempt: Attempt): boolean {
    return attempt.peer?.connectionState === "connected" && attempt.channel?.readyState === "open";
  }

  /**
   * Resolve once the peer connection is connected and the data channel is open. The
   * current state is checked first (an answer may already have connected them); otherwise
   * the peer's connection-state and the channel's open events settle it. Failure and
   * cancellation are handled by the transport handlers and the caller's bound, so this
   * promise only ever resolves.
   */
  function awaitTransportReady(attempt: Attempt): Promise<void> {
    if (transportReady(attempt)) return Promise.resolve();
    return new Promise<void>((resolve) => {
      attempt.onTransportChange = () => {
        if (!transportReady(attempt)) return;
        attempt.onTransportChange = undefined;
        resolve();
      };
    });
  }

  function scheduleReplacement(attempt: Attempt, delay = VOICE_REPLACEMENT_POLL_INTERVAL_MS) {
    if (!owns(attempt) || (snapshot.phase !== "active" && !attempt.transportLoss)) return;
    if (attempt.replacementTimer !== undefined) env.clearTimeout(attempt.replacementTimer);
    attempt.replacementTimer = env.setTimeout(() => {
      attempt.replacementTimer = undefined;
      requestReplacement(attempt);
    }, delay);
  }

  function requestReplacement(attempt: Attempt) {
    if (!owns(attempt) || attempt.replacementPolling) return;
    attempt.replacementPolling = true;
    void pollReplacement(attempt).finally(() => { attempt.replacementPolling = false; });
  }

  async function pollReplacement(attempt: Attempt) {
    try {
      assertOwns(attempt);
      if (snapshot.phase !== "active" && !attempt.transportLoss) return;
      if (attempt.recoveryDeadline !== undefined && env.now() >= attempt.recoveryDeadline) {
        fail(attempt, RECOVERY_TIMEOUT_MESSAGE);
        return;
      }
      const raw = await controlPlaneCall(attempt, "meta/console/voice/replacement", closeParams(attempt));
      assertOwns(attempt);
      if (attempt.recoveryDeadline !== undefined && env.now() >= attempt.recoveryDeadline) {
        fail(attempt, RECOVERY_TIMEOUT_MESSAGE);
        return;
      }
      const pending = parseReplacement(raw);
      clearFailureWindow(attempt, "replacementFailure");
      if (pending) {
        await enqueue(async () => {
          try {
            assertOwns(attempt);
            if (pending.targetIdentity !== attempt.target.identity || pending.executionMode !== "client_context") {
              throw new VoiceError("Voice recovery authority did not match the current agent.");
            }
            if (pending.channelId === attempt.active?.channelId) {
              if (attempt.transportLoss) {
                throw new VoiceError("Voice connection was lost without new recovery authority. Start voice again.");
              }
              if (pending.pendingReceipt !== attempt.pending?.pendingReceipt) {
                throw new VoiceError("Voice recovery returned conflicting channel authority.");
              }
              return;
            }
            publish({ phase: "connecting" });
            assertOwns(attempt);
            if (attempt.sampleTimer !== undefined) env.clearTimeout(attempt.sampleTimer);
            cleanupPeer(attempt);
            attempt.active = undefined;
            attempt.readiness = undefined;
            attempt.deadline = attempt.recoveryDeadline ?? env.now() + VOICE_CONNECT_TIMEOUT_MS;
            attempt.active = await activatePending(attempt, pending);
            assertOwns(attempt);
            if (attempt.recoveryDeadline !== undefined && env.now() >= attempt.recoveryDeadline) {
              throw new VoiceError(RECOVERY_TIMEOUT_MESSAGE);
            }
            if (attempt.recoveryTimer !== undefined) env.clearTimeout(attempt.recoveryTimer);
            attempt.recoveryTimer = undefined;
            attempt.recoveryDeadline = undefined;
            attempt.transportLoss = undefined;
            if (env.now() - attempt.lastActivity >= VOICE_SILENCE_TIMEOUT_MS) {
              await stop(attempt, null, "Voice closed after 15 minutes of silence.");
              return;
            }
            // A replacement channel is a fresh provider session; its item ids
            // start over, so the provisional rows of the old channel are dropped.
            publish({ phase: "active", activeChannelId: attempt.active?.channelId ?? null, liveSpeech: [] });
            assertOwns(attempt);
            gates(attempt);
            scheduleSilence(attempt);
            sampleActivity(attempt);
            observeContext(attempt);
            observeCaptions(attempt);
          } catch (error) {
            if (!(error instanceof Cancelled)) await stop(attempt, voiceError(error, attempt.connectionStage));
          }
        });
      }
      scheduleReplacement(attempt);
    } catch (error) {
      if (error instanceof Cancelled || !owns(attempt)) return;
      if (error instanceof TransientRpcFailure) {
        // One unreachable poll must not end connected audio; retry with backoff. During
        // recovery the fixed recovery deadline still bounds the wait.
        scheduleReplacement(attempt, recordTransientFailure(attempt, "replacementFailure", REPLACEMENT_UNVERIFIED_MESSAGE));
        return;
      }
      const data = (error as { rpcError?: { data?: { kind?: string; reason?: string } } } | null)?.rpcError?.data;
      fail(attempt, data?.kind === "voice_superseded"
        ? voiceSupersededMessage(data.reason)
        : data?.kind === "voice_closed"
          ? "The gateway closed this voice session. Start voice again."
          : REPLACEMENT_UNVERIFIED_MESSAGE);
    }
  }

  // `suspended`/`interrupted` (phone call, headset switch, autoplay policy) usually return to
  // `running` on their own or on the next user gesture. Only a bounded stall is fatal.
  function observeAudioContext(attempt: Attempt) {
    const context = attempt.context!;
    context.onstatechange = () => {
      if (!owns(attempt) || attempt.context !== context) return;
      if (context.state === "running") {
        if (attempt.audioResumeTimer !== undefined) env.clearTimeout(attempt.audioResumeTimer);
        attempt.audioResumeTimer = undefined;
        return;
      }
      if (context.state === "closed") {
        fail(attempt, AUDIO_INTERRUPTED_MESSAGE);
        return;
      }
      attempt.audioResumeTimer ??= env.setTimeout(() => {
        attempt.audioResumeTimer = undefined;
        if (owns(attempt) && context.state !== "running") fail(attempt, AUDIO_INTERRUPTED_MESSAGE);
      }, VOICE_AUDIO_RESUME_TIMEOUT_MS);
      resumeAudio(attempt);
    };
  }

  function resumeAudio(attempt: Attempt) {
    const context = attempt.context;
    if (!owns(attempt) || !context || context.state === "running" || context.state === "closed") return;
    void context.resume().catch(() => {});
  }

  async function connect(attempt: Attempt, media: Promise<MediaStream>, resumed: Promise<void>) {
    try {
      assertOwns(attempt);
      if (teardownBlock) {
        throw new VoiceError("The previous voice session has not closed. Use Close to retry before starting another session.");
      }
      await connecting(attempt, resumed);
      assertOwns(attempt);
      if (attempt.context!.state !== "running") {
        throw new VoiceError("Browser audio is suspended. Allow audio playback and start voice again.");
      }
      observeAudioContext(attempt);
      attempt.stream = await connecting(attempt, media);
      assertOwns(attempt);
      if (attempt.stream.getAudioTracks().length === 0) {
        throw new VoiceError("No microphone audio track was available. Check your microphone and try again.");
      }
      attempt.connectionStage = "opening";
      publish({ phase: "connecting", connectionStage: "opening" });
      assertOwns(attempt);
      attempt.openSent = true;
      const raw = await connecting(attempt, env.rpc("meta/console/voice/open", {
        identity: attempt.target.identity, request_id: attempt.requestId,
      }, VOICE_CONNECT_TIMEOUT_MS));
      assertOwns(attempt);
      attempt.active = await activatePending(attempt, parsePendingLiveChannelHandle(raw));
      assertOwns(attempt);
      attempt.lastActivity = env.now();
      publish({ phase: "active", activeChannelId: attempt.active?.channelId ?? null, liveSpeech: [] });
      assertOwns(attempt);
      gates(attempt);
      scheduleSilence(attempt);
      sampleActivity(attempt);
      scheduleReplacement(attempt);
      observeContext(attempt);
      observeCaptions(attempt);
    } catch (error) {
      if (error instanceof Cancelled) {
        await teardown(attempt).catch(() => {});
      } else {
        await stop(attempt, voiceError(error, attempt.connectionStage));
      }
    }
  }

  const controller: VoiceSession = {
    subscribe(listener) {
      listeners.add(listener);
      return () => { listeners.delete(listener); };
    },
    getSnapshot: () => snapshot,
    start(target) {
      if (pageHidden) return Promise.resolve();
      // React StrictMode can replay an effect cleanup without replacing its memoized
      // controller. An explicit later click starts a fresh lifecycle, never old media.
      disposed = false;
      if (current && owns(current) && current.target.identity === target.identity) return serial;
      if (teardownBlock) {
        publish({ phase: "error", error: "The previous voice session has not closed. Use Close to retry before starting another session." });
        return Promise.resolve();
      }
      const previous = current;
      if (previous) quiesceLocal(previous);
      let requestId: string;
      try {
        requestId = env.randomId();
      } catch {
        publish({
          phase: "error",
          error: "Voice requires HTTPS or localhost and a browser with secure random identifiers.",
        });
        return previous ? enqueue(() => stop(previous, snapshot.error)) : Promise.resolve();
      }
      const attempt: Attempt = {
        target: { ...target },
        requestId,
        abort: new AbortController(),
        deadline: env.now() + VOICE_CONNECT_TIMEOUT_MS,
        nodes: [], openSent: false, localClosed: false,
        lastActivity: env.now(),
      };
      retainedAttempts.add(attempt);
      current = attempt;
      publish({
        phase: "requesting", target: attempt.target,
        microphoneMuted: false, speakerMuted: false, error: null, notice: null,
        connectionStage: undefined, reconnecting: false,
        contextPreparation: undefined, contextStatusError: null,
      });
      if (!owns(attempt)) {
        return enqueue(async () => {
          if (previous) await teardown(previous).catch(() => {});
          await teardown(attempt).catch(() => {});
        });
      }
      let media: Promise<MediaStream>;
      let resumed: Promise<void>;
      try {
        if (!target.identity.trim()) throw new VoiceError("Select an agent before starting voice.");
        // Unlock playback and request the microphone during the click, while the gateway
        // freshly confirms authenticated OpenAI voice availability. No channel opens until
        // readiness is positive; a negative or failed readiness releases the microphone.
        attempt.context = env.createAudioContext();
        resumed = attempt.context.resume();
        void resumed.catch(() => {});
        // Granted tracks are gated at once; nothing consumes them until activation.
        const microphone = env.getUserMedia().then((stream) => {
          for (const track of stream.getTracks()) track.enabled = false;
          return stream;
        });
        void microphone.catch(() => {});
        const releaseMicrophone = () => {
          void microphone.then((stream) => {
            for (const track of stream.getTracks()) track.stop();
          }).catch(() => {});
        };
        media = env.voiceAvailable(attempt.target.identity).then((available) => {
          if (!owns(attempt) || available !== "available") releaseMicrophone();
          assertOwns(attempt);
          if (available === "unknown") {
            throw new VoiceError("Voice readiness could not be checked. Check your network connection to the gateway, then start voice again.");
          }
          if (available !== "available") {
            throw new VoiceError("Voice is unavailable. Ask your administrator to authenticate OpenAI and enable GPT Live.");
          }
          return microphone;
        }, (error: unknown) => {
          releaseMicrophone();
          throw error;
        }).then((stream) => {
          for (const track of stream.getTracks()) track.enabled = false;
          if (!owns(attempt)) {
            for (const track of stream.getTracks()) track.stop();
          } else {
            attempt.stream = stream;
          }
          return stream;
        });
        void media.catch(() => {});
        removePagehide ??= env.onPagehide(() => {
          pageHidden = true;
          // A bfcache restore fires pageshow without re-running any effect; the one-shot
          // listener deliberately outlives dispose() so an explicit later start is admitted.
          removePageshow?.();
          removePageshow = env.onPageshow(() => {
            pageHidden = false;
            removePageshow?.();
            removePageshow = undefined;
          });
          for (const retained of retainedAttempts) {
            cleanupLocal(retained);
            if (retained.openSent) env.pagehideClose(closeParams(retained));
          }
          controller.dispose();
        });
      } catch (error) {
        cleanupLocal(attempt);
        return enqueue(async () => {
          if (previous) await stop(previous);
          await stop(attempt, voiceError(error));
        });
      }
      return enqueue(async () => {
        if (previous) {
          try { await teardown(previous); } catch {
            await stop(attempt, "The previous voice session has not closed. Use Close to retry before starting another session.");
            return;
          }
        }
        await connect(attempt, media, resumed);
      });
    },
    close() {
      const attempt = current ?? teardownBlock;
      if (!attempt) {
        publish({ phase: "idle", target: null, error: null, notice: null });
        return Promise.resolve();
      }
      current ??= attempt;
      quiesceLocal(attempt);
      publish({ phase: "closing" });
      return enqueue(async () => {
        await stop(attempt);
        if (teardownBlock && teardownBlock !== attempt) {
          const blocked = teardownBlock;
          current = blocked;
          await stop(blocked);
        }
      });
    },
    toggleMicrophone() {
      if (!current || !owns(current)) return;
      publish({ microphoneMuted: !snapshot.microphoneMuted });
      gates(current);
      // Mute toggles are user gestures, which is what a suspended AudioContext needs.
      resumeAudio(current);
    },
    toggleSpeaker() {
      if (!current || !owns(current)) return;
      publish({ speakerMuted: !snapshot.speakerMuted });
      gates(current);
      resumeAudio(current);
    },
    sampleWaveform(source, samples) {
      samples.fill(0);
      if (!current || !owns(current) || snapshot.phase !== "active") return;
      if (source === "microphone" && snapshot.microphoneMuted) return;
      const analyser = source === "microphone" ? current.microphone : current.speaker;
      analyser?.getFloatTimeDomainData(samples);
    },
    dispose() {
      if (disposed) return;
      disposed = true;
      removePagehide?.();
      removePagehide = undefined;
      listeners.clear();
      for (const attempt of retainedAttempts) cleanupLocal(attempt);
      void controller.close();
    },
  };
  return controller;
}
