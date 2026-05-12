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
