import type {
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
 * `duck` silences assistant audio at the user's speech onset (a barge-in),
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
 * and any level measurement. The user then stops hearing the assistant at
 * their speech onset instead of when the provider yields and the audio
 * already in flight drains.
 *
 * `currentTime` is the audio context's clock (`AudioContext.currentTime`).
 */
export function applyLiveAssistantPlaybackHint(
  node: LiveGainNodeLike,
  hint: LiveAssistantPlaybackHint,
  currentTime: number,
): void {
  const target =
    hint === "duck" ? LIVE_ASSISTANT_PLAYBACK_DUCKED_GAIN : LIVE_ASSISTANT_PLAYBACK_UNITY_GAIN;
  node.gain.cancelScheduledValues?.(currentTime);
  node.gain.setTargetAtTime(target, currentTime, LIVE_ASSISTANT_PLAYBACK_GAIN_TIME_CONSTANT_S);
}
