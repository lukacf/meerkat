import type {
  LiveAssistantOutputAvailableParams,
  LiveAssistantPlaybackHintParams,
  LiveMediaHealthRequestedParams,
  LiveWebrtcAnswerParams,
  LiveWebrtcAnswerResult,
  WireLiveTransportBootstrap,
  WireLiveTransportBootstrapWebrtc,
} from "./generated/types.js";

export interface LiveWebrtcAnswerClient {
  liveWebrtcAnswer(params: LiveWebrtcAnswerParams): Promise<LiveWebrtcAnswerResult>;
}

export interface LiveWebrtcOfferDescription {
  readonly type?: string;
  readonly sdp?: string | null;
}

export interface LiveWebrtcPeerConnectionLike {
  createOffer(): Promise<LiveWebrtcOfferDescription>;
  setLocalDescription(description: LiveWebrtcOfferDescription): Promise<void>;
  setRemoteDescription(description: { type: "answer"; sdp: string }): Promise<void>;
}

export interface LiveWebrtcAudioConstraintOptions {
  readonly echoCancellation?: boolean;
  readonly noiseSuppression?: boolean;
  readonly autoGainControl?: boolean;
  readonly extraAudio?: Record<string, unknown>;
}

export function liveWebrtcAudioConstraints(
  options: LiveWebrtcAudioConstraintOptions = {},
): Record<string, unknown> {
  return {
    echoCancellation: options.echoCancellation ?? true,
    noiseSuppression: options.noiseSuppression ?? true,
    autoGainControl: options.autoGainControl ?? true,
    ...(options.extraAudio ?? {}),
  };
}

export function liveWebrtcMediaConstraints(
  options: LiveWebrtcAudioConstraintOptions = {},
): Record<string, unknown> {
  return {
    audio: liveWebrtcAudioConstraints(options),
    video: false,
  };
}

export function isLiveWebrtcBootstrap(
  transport: WireLiveTransportBootstrap,
): transport is WireLiveTransportBootstrapWebrtc {
  return transport.transport === "webrtc";
}

export async function answerLiveWebrtcOffer(
  client: LiveWebrtcAnswerClient,
  channelId: string,
  transport: WireLiveTransportBootstrapWebrtc,
  peerConnection: LiveWebrtcPeerConnectionLike,
): Promise<string> {
  const offer = await peerConnection.createOffer();
  if (offer.sdp == null || offer.sdp.length === 0) {
    throw new Error("RTCPeerConnection.createOffer() did not produce SDP");
  }
  await peerConnection.setLocalDescription(offer);
  const result = await client.liveWebrtcAnswer({
    channel_id: channelId,
    token: transport.token,
    offer_sdp: offer.sdp,
  });
  await peerConnection.setRemoteDescription({
    type: "answer",
    sdp: result.answer_sdp,
  });
  return result.answer_sdp;
}

/**
 * The playback a `live/assistant_playback_hint` notification asks for:
 * `duck` silences assistant audio that overlaps the user's speech (a barge-in),
 * `restore` plays it normally again.
 */
export type LiveAssistantPlaybackHint = "duck" | "restore";

/** The audio-param surface the playback gate drives (a Web Audio `AudioParam`). */
export interface LiveAudioParamLike {
  setTargetAtTime(target: number, startTime: number, timeConstant: number): unknown;
  cancelScheduledValues?(cancelTime: number): unknown;
}

/** A gain stage on the assistant's remote audio (a Web Audio `GainNode`). */
export interface LiveGainNodeLike {
  readonly gain: LiveAudioParamLike;
}

/** Assistant playback gain while ducked: silence. */
export const LIVE_ASSISTANT_PLAYBACK_DUCKED_GAIN = 0;
/** Assistant playback gain when restored. */
export const LIVE_ASSISTANT_PLAYBACK_UNITY_GAIN = 1;
/** Time constant of the gain change, in seconds (an audio-clock ramp, not a timer). */
export const LIVE_ASSISTANT_PLAYBACK_GAIN_TIME_CONSTANT_S = 0.01;

/**
 * Apply one `live/assistant_playback_hint` to the gain stage the client
 * places on the assistant's remote audio, before both its playback output
 * and any level measurement. The user then stops hearing the assistant as
 * soon as it overlaps their speech instead of when the provider yields and
 * the audio already in flight drains.
 *
 * `currentTime` is the audio context's clock (`AudioContext.currentTime`).
 */
export interface LiveAssistantPlaybackGateOptions {
  /**
   * Gain while ducked, 0 (silence, the default) to 1. A client that would
   * rather keep backchannel-time audio than enforce strict talk-over can
   * attenuate partially, for example 0.2.
   */
  readonly duckedGain?: number;
}

export function applyLiveAssistantPlaybackHint(
  node: LiveGainNodeLike,
  hint: LiveAssistantPlaybackHint,
  currentTime: number,
  options: LiveAssistantPlaybackGateOptions = {},
): void {
  const duckedGain = Math.min(
    LIVE_ASSISTANT_PLAYBACK_UNITY_GAIN,
    Math.max(0, options.duckedGain ?? LIVE_ASSISTANT_PLAYBACK_DUCKED_GAIN),
  );
  const target = hint === "duck" ? duckedGain : LIVE_ASSISTANT_PLAYBACK_UNITY_GAIN;
  node.gain.cancelScheduledValues?.(currentTime);
  node.gain.setTargetAtTime(target, currentTime, LIVE_ASSISTANT_PLAYBACK_GAIN_TIME_CONSTANT_S);
}

/**
 * A server-to-client `live/*` notification, as delivered to
 * {@link MeerkatClient.onLiveNotification} listeners.
 *
 * - `live/assistant_output_available`: an actionable playback handle.
 * - `live/media_health_requested`: answer with `liveMediaHealth`.
 * - `live/assistant_playback_hint`: barge-in duck/restore; apply it with
 *   {@link applyLiveAssistantPlaybackHint}. Purely advisory.
 */
export type LiveNotification =
  | { method: "live/assistant_output_available"; params: LiveAssistantOutputAvailableParams }
  | { method: "live/media_health_requested"; params: LiveMediaHealthRequestedParams }
  | { method: "live/assistant_playback_hint"; params: LiveAssistantPlaybackHintParams };

export type LiveNotificationListener = (notification: LiveNotification) => void;

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

/**
 * Narrow one server notification to a {@link LiveNotification}. Returns
 * `undefined` for a method this SDK build does not know or a malformed
 * payload, so a newer server's notifications are ignored, never misread.
 */
export function parseLiveNotification(
  method: string,
  params: unknown,
): LiveNotification | undefined {
  if (!isRecord(params) || typeof params.channel_id !== "string") {
    return undefined;
  }
  const channel_id = params.channel_id;
  switch (method) {
    case "live/assistant_output_available":
      if (typeof params.output_id !== "string" || typeof params.content_index !== "number") {
        return undefined;
      }
      return {
        method,
        params: { channel_id, output_id: params.output_id, content_index: params.content_index },
      };
    case "live/media_health_requested":
      if (typeof params.output_id !== "string") {
        return undefined;
      }
      return { method, params: { channel_id, output_id: params.output_id } };
    case "live/assistant_playback_hint":
      if (params.hint !== "duck" && params.hint !== "restore") {
        return undefined;
      }
      return { method, params: { channel_id, hint: params.hint } };
    default:
      return undefined;
  }
}
