//! Live WebRTC transport — browser WebRTC terminator over `LiveAdapterHost`.
//!
//! This module is behind the non-default `webrtc` feature because it owns the
//! media stack: SDP, ICE/DTLS/SRTP, RTP, Opus, and resampling. Surfaces only
//! compose this state and expose signaling; live channel semantics stay in
//! `LiveAdapterHost`.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::time::Duration;

use axum::Router;
use bytes::Bytes;
use meerkat_contracts::WireLiveAdapterObservation;
use meerkat_core::live_adapter::{LiveAdapterObservation, LiveInputChunk};
use opus::{Application, Channels, Decoder, Encoder};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use webrtc::api::APIBuilder;
use webrtc::api::media_engine::{MIME_TYPE_OPUS, MediaEngine};
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::sdp::session_description::RTCSessionDescription;
use webrtc::rtp_transceiver::rtp_codec::RTCRtpCodecCapability;
use webrtc::track::track_local::TrackLocal;
use webrtc::track::track_local::track_local_static_sample::TrackLocalStaticSample;
use webrtc::track::track_remote::TrackRemote;
use webrtc_media::Sample;

use crate::host::{
    LiveAdapterHost, LiveAdapterHostError, LiveChannelId, ObservationOutcome, ObservationRouting,
};
use crate::transport::{
    LiveChannelCloseFeedback, LiveChannelStatusFeedback, LiveTokenString, LiveWsState,
    live_ws_router,
};

/// Canonical JSON-RPC signaling method for browser-offer WebRTC.
pub const LIVE_WEBRTC_ANSWER_METHOD: &str = "live/webrtc/answer";
pub const LIVE_WEBRTC_ANSWER_PATH: &str = "/live/webrtc/answer";

/// Time-to-live for a minted but unconsumed WebRTC signaling token.
pub const WEBRTC_TOKEN_TTL: Duration = Duration::from_secs(60);

const BROWSER_OPUS_SAMPLE_RATE: u32 = 48_000;
const PROVIDER_PCM_SAMPLE_RATE: u32 = 24_000;
const MONO_CHANNELS: u16 = 1;
const OPUS_20MS_SAMPLES_48K: usize = 960;
const OPUS_MAX_PACKET_BYTES: usize = 4096;
const OPUS_MAX_DECODED_SAMPLES_48K: usize = 5760;
const WEBRTC_AUDIO_PACKET_MS: u64 = 20;
const WEBRTC_AUDIO_PACKET_DURATION: Duration = Duration::from_millis(WEBRTC_AUDIO_PACKET_MS);
const WEBRTC_AUDIO_QUEUE_CAPACITY: usize = 1_800;
const LOCAL_BARGE_IN_RMS_THRESHOLD: f32 = 0.035;

struct OutgoingAudioPacket {
    generation: u64,
    data: Vec<u8>,
}

#[derive(Default)]
struct OutgoingAudioControl {
    generation: AtomicU64,
    queued_packets: AtomicUsize,
}

impl OutgoingAudioControl {
    fn generation(&self) -> u64 {
        self.generation.load(Ordering::Relaxed)
    }

    fn queued_packets(&self) -> usize {
        self.queued_packets.load(Ordering::Relaxed)
    }

    fn note_queued(&self) {
        self.queued_packets.fetch_add(1, Ordering::Relaxed);
    }

    fn note_dequeued(&self) {
        let _ = self
            .queued_packets
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |current| {
                current.checked_sub(1)
            });
    }

    fn discard_queued(&self) -> u64 {
        self.queued_packets.store(0, Ordering::Relaxed);
        self.generation.fetch_add(1, Ordering::Relaxed) + 1
    }
}

/// Errors returned by the WebRTC transport/signaling layer.
#[derive(Debug, thiserror::Error)]
pub enum LiveWebrtcError {
    #[error("channel not found: {0}")]
    ChannelNotFound(String),
    #[error("WebRTC error: {0}")]
    Webrtc(String),
    #[error("audio bridge error: {0}")]
    Audio(String),
    #[error("JSON frame error: {0}")]
    Json(serde_json::Error),
    #[error("missing local WebRTC description after answer")]
    MissingLocalDescription,
}

impl From<serde_json::Error> for LiveWebrtcError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

struct LiveWebrtcPeer {
    peer: Arc<RTCPeerConnection>,
    outgoing_audio: Arc<OutgoingAudioControl>,
}

type LiveWebrtcPeerRegistry = Arc<Mutex<HashMap<LiveChannelId, LiveWebrtcPeer>>>;

fn observation_requires_generated_close(observation: &LiveAdapterObservation) -> bool {
    match observation {
        LiveAdapterObservation::Error { .. } => true,
        LiveAdapterObservation::StatusChanged { status } => status.is_terminal(),
        _ => false,
    }
}

/// Transport evidence that a WebRTC answer was materialized.
///
/// The SDP is transport payload. The sequence is monotonic evidence consumed
/// by MeerkatMachine before the RPC surface projects a public success result.
#[derive(Debug, Clone)]
pub struct LiveWebrtcAnswerAccepted {
    pub answer_sdp: String,
    pub answer_observation_sequence: u64,
}

/// Shared state for the live WebRTC transport.
///
/// This is composable transport state. It does not open provider sessions and
/// does not own Meerkat semantics; it binds an already-open live channel to a
/// browser peer after `live/open` has created the channel and provider adapter.
pub struct LiveWebrtcState {
    host: Arc<LiveAdapterHost>,
    close_feedback: Arc<dyn LiveChannelCloseFeedback>,
    status_feedback: Arc<dyn LiveChannelStatusFeedback>,
    peers: LiveWebrtcPeerRegistry,
    token_ttl: Duration,
    answer_observation_sequence: AtomicU64,
}

pub fn live_webrtc_router(_state: Arc<LiveWebrtcState>) -> Router {
    // HTTP signaling has no runtime machine authority handle here, so it must
    // not answer offers or classify errors. JSON-RPC `live/webrtc/answer` is
    // the authority-backed signaling surface.
    Router::new()
}

pub async fn serve_live_ws_and_webrtc_listener(
    listener: tokio::net::TcpListener,
    ws_state: Arc<LiveWsState>,
    webrtc_state: Arc<LiveWebrtcState>,
) -> Result<(), std::io::Error> {
    let app = live_ws_router(ws_state).merge(live_webrtc_router(webrtc_state));
    axum::serve(listener, app).await
}

impl LiveWebrtcState {
    pub fn new(
        host: Arc<LiveAdapterHost>,
        close_feedback: Arc<dyn LiveChannelCloseFeedback>,
        status_feedback: Arc<dyn LiveChannelStatusFeedback>,
    ) -> Self {
        Self::with_token_ttl(host, close_feedback, status_feedback, WEBRTC_TOKEN_TTL)
    }

    #[cfg(test)]
    pub(crate) fn new_for_test_with_generated_close_feedback(host: Arc<LiveAdapterHost>) -> Self {
        let close_feedback = Arc::new(
            crate::transport::GeneratedTestMachineLiveChannelCloseFeedback::new(Arc::clone(&host)),
        );
        let status_feedback = Arc::new(
            crate::transport::GeneratedTestMachineLiveChannelStatusFeedback::new(Arc::clone(&host)),
        );
        Self::new(host, close_feedback, status_feedback)
    }

    /// Construct with a custom token TTL — primarily for tests.
    pub fn with_token_ttl(
        host: Arc<LiveAdapterHost>,
        close_feedback: Arc<dyn LiveChannelCloseFeedback>,
        status_feedback: Arc<dyn LiveChannelStatusFeedback>,
        token_ttl: Duration,
    ) -> Self {
        Self {
            host,
            close_feedback,
            status_feedback,
            peers: Arc::new(Mutex::new(HashMap::new())),
            token_ttl,
            answer_observation_sequence: AtomicU64::new(0),
        }
    }

    pub fn host(&self) -> &Arc<LiveAdapterHost> {
        &self.host
    }

    pub fn token_ttl(&self) -> Duration {
        self.token_ttl
    }

    /// Mint random bearer material for a WebRTC signaling token.
    ///
    /// This transport does not store token binding, expiry, or consume state.
    /// `live/open` records those facts in MeerkatMachine before returning the
    /// token, and `live/webrtc/answer` must receive generated admission before
    /// calling [`Self::answer_offer`].
    pub async fn mint_token(&self, _channel_id: LiveChannelId) -> LiveTokenString {
        LiveTokenString::random()
    }

    /// Answer a browser-created SDP offer after generated token admission.
    pub async fn answer_offer(
        &self,
        channel_id: LiveChannelId,
        offer_sdp: String,
    ) -> Result<LiveWebrtcAnswerAccepted, LiveWebrtcError> {
        self.host
            .channel_session(&channel_id)
            .await
            .map_err(|_| LiveWebrtcError::ChannelNotFound(channel_id.to_string()))?;

        let mut media_engine = MediaEngine::default();
        media_engine
            .register_default_codecs()
            .map_err(|err| LiveWebrtcError::Webrtc(err.to_string()))?;
        let api = APIBuilder::new().with_media_engine(media_engine).build();
        let peer = Arc::new(
            api.new_peer_connection(RTCConfiguration::default())
                .await
                .map_err(|err| LiveWebrtcError::Webrtc(err.to_string()))?,
        );

        let outgoing_audio = Arc::new(TrackLocalStaticSample::new(
            RTCRtpCodecCapability {
                mime_type: MIME_TYPE_OPUS.to_owned(),
                clock_rate: BROWSER_OPUS_SAMPLE_RATE,
                channels: 1,
                ..Default::default()
            },
            "audio".to_owned(),
            "meerkat-live".to_owned(),
        ));
        let outgoing_audio_track: Arc<dyn TrackLocal + Send + Sync> = outgoing_audio.clone();
        peer.add_track(outgoing_audio_track)
            .await
            .map_err(|err| LiveWebrtcError::Webrtc(err.to_string()))?;
        let outgoing_audio_control = Arc::new(OutgoingAudioControl::default());
        let (outgoing_audio_tx, outgoing_audio_rx) =
            mpsc::channel::<OutgoingAudioPacket>(WEBRTC_AUDIO_QUEUE_CAPACITY);
        spawn_outgoing_audio_track_pump(
            channel_id.clone(),
            Arc::clone(&outgoing_audio),
            outgoing_audio_rx,
            Arc::clone(&outgoing_audio_control),
        );

        install_data_channel_handler(DataChannelPumpContext {
            host: Arc::clone(&self.host),
            channel_id: channel_id.clone(),
            peer: Arc::clone(&peer),
            peer_registry: Arc::clone(&self.peers),
            close_feedback: Arc::clone(&self.close_feedback),
            status_feedback: Arc::clone(&self.status_feedback),
            outgoing_audio_tx,
            outgoing_audio_control: Arc::clone(&outgoing_audio_control),
        });
        install_incoming_audio_handler(
            Arc::clone(&self.host),
            channel_id.clone(),
            Arc::clone(&peer),
            Arc::clone(&outgoing_audio_control),
        );

        let offer = RTCSessionDescription::offer(offer_sdp)
            .map_err(|err| LiveWebrtcError::Webrtc(err.to_string()))?;
        peer.set_remote_description(offer)
            .await
            .map_err(|err| LiveWebrtcError::Webrtc(err.to_string()))?;
        let answer = peer
            .create_answer(None)
            .await
            .map_err(|err| LiveWebrtcError::Webrtc(err.to_string()))?;
        let mut gathering_complete = peer.gathering_complete_promise().await;
        peer.set_local_description(answer)
            .await
            .map_err(|err| LiveWebrtcError::Webrtc(err.to_string()))?;
        let _ = gathering_complete.recv().await;
        let answer_sdp = peer
            .local_description()
            .await
            .ok_or(LiveWebrtcError::MissingLocalDescription)?
            .sdp;

        self.peers.lock().await.insert(
            channel_id,
            LiveWebrtcPeer {
                peer,
                outgoing_audio: outgoing_audio_control,
            },
        );

        Ok(LiveWebrtcAnswerAccepted {
            answer_sdp,
            answer_observation_sequence: self
                .answer_observation_sequence
                .fetch_add(1, Ordering::Relaxed)
                + 1,
        })
    }

    pub async fn close_peer(&self, channel_id: &LiveChannelId) {
        if let Some(peer) = self.peers.lock().await.remove(channel_id) {
            let _ = peer.peer.close().await;
        }
    }

    /// Drop any server-paced WebRTC output audio that has not reached the
    /// browser yet. Used by explicit interrupt/truncate RPC paths and by local
    /// VAD hints so the transport does not keep speaking stale provider audio
    /// while the semantic interrupt is still making its way through the
    /// provider event stream.
    pub async fn discard_output_audio(&self, channel_id: &LiveChannelId) {
        if let Some(peer) = self.peers.lock().await.get(channel_id) {
            let generation = peer.outgoing_audio.discard_queued();
            tracing::debug!(
                channel = %channel_id,
                generation,
                "discarding queued WebRTC output audio"
            );
        }
    }

    #[cfg(test)]
    async fn peer_count(&self) -> usize {
        self.peers.lock().await.len()
    }
}

#[derive(Clone)]
struct DataChannelPumpContext {
    host: Arc<LiveAdapterHost>,
    channel_id: LiveChannelId,
    peer: Arc<RTCPeerConnection>,
    peer_registry: LiveWebrtcPeerRegistry,
    close_feedback: Arc<dyn LiveChannelCloseFeedback>,
    status_feedback: Arc<dyn LiveChannelStatusFeedback>,
    outgoing_audio_tx: mpsc::Sender<OutgoingAudioPacket>,
    outgoing_audio_control: Arc<OutgoingAudioControl>,
}

fn install_data_channel_handler(context: DataChannelPumpContext) {
    let peer_for_handler = Arc::clone(&context.peer);
    let peer_for_callback = Arc::clone(&context.peer);
    peer_for_handler.on_data_channel(Box::new(move |channel: Arc<RTCDataChannel>| {
        let host_for_messages = Arc::clone(&context.host);
        let channel_for_messages = context.channel_id.clone();
        let channel_for_open = Arc::clone(&channel);
        let observation_context = DataChannelPumpContext {
            host: Arc::clone(&context.host),
            channel_id: context.channel_id.clone(),
            peer: Arc::clone(&peer_for_callback),
            peer_registry: Arc::clone(&context.peer_registry),
            close_feedback: Arc::clone(&context.close_feedback),
            status_feedback: Arc::clone(&context.status_feedback),
            outgoing_audio_tx: context.outgoing_audio_tx.clone(),
            outgoing_audio_control: Arc::clone(&context.outgoing_audio_control),
        };

        Box::pin(async move {
            channel.on_message(Box::new(move |message: DataChannelMessage| {
                let host = Arc::clone(&host_for_messages);
                let channel_id = channel_for_messages.clone();
                Box::pin(async move {
                    if message.is_string {
                        match serde_json::from_slice::<LiveInputChunk>(&message.data) {
                            Ok(chunk) => {
                                if let Err(err) = host.send_input(&channel_id, chunk).await {
                                    tracing::warn!(
                                        channel = %channel_id,
                                        error = %err,
                                        "WebRTC data-channel send_input failed"
                                    );
                                }
                            }
                            Err(err) => {
                                tracing::warn!(
                                    channel = %channel_id,
                                    error = %err,
                                    "invalid WebRTC data-channel JSON frame"
                                );
                            }
                        }
                    }
                })
            }));

            channel.on_open(Box::new(move || {
                let data_channel = Arc::clone(&channel_for_open);
                let context = observation_context.clone();
                Box::pin(async move {
                    tokio::spawn(async move {
                        pump_observations_to_data_channel(context, data_channel).await;
                    });
                })
            }));
        })
    }));
}

fn install_incoming_audio_handler(
    host: Arc<LiveAdapterHost>,
    channel_id: LiveChannelId,
    peer: Arc<RTCPeerConnection>,
    outgoing_audio_control: Arc<OutgoingAudioControl>,
) {
    peer.on_track(Box::new(move |track: Arc<TrackRemote>, _, _| {
        let host = Arc::clone(&host);
        let channel_id = channel_id.clone();
        let outgoing_audio_control = Arc::clone(&outgoing_audio_control);
        tokio::spawn(async move {
            let mut bridge = match WebrtcAudioBridge::new() {
                Ok(bridge) => bridge,
                Err(err) => {
                    tracing::warn!(channel = %channel_id, error = %err, "failed to initialize WebRTC audio bridge");
                    return;
                }
            };
            loop {
                let packet = match track.read_rtp().await {
                    Ok((packet, _)) => packet,
                    Err(err) => {
                        tracing::debug!(channel = %channel_id, error = %err, "WebRTC RTP track ended");
                        break;
                    }
                };
                let pcm_24k = match bridge.decode_browser_opus_payload(&packet.payload) {
                    Ok(pcm) => pcm,
                    Err(err) => {
                        tracing::warn!(channel = %channel_id, error = %err, "failed to decode browser Opus payload");
                        continue;
                    }
                };
                if pcm_24k.is_empty() {
                    continue;
                }
                if outgoing_audio_control.queued_packets() > 0 && pcm24k_le_has_speech(&pcm_24k) {
                    let generation = outgoing_audio_control.discard_queued();
                    tracing::debug!(
                        channel = %channel_id,
                        generation,
                        "discarding queued WebRTC output audio after local speech barge-in"
                    );
                }
                let chunk = LiveInputChunk::Audio {
                    data: pcm_24k,
                    sample_rate_hz: PROVIDER_PCM_SAMPLE_RATE,
                    channels: MONO_CHANNELS,
                };
                if let Err(err) = host.send_input(&channel_id, chunk).await {
                    tracing::warn!(
                        channel = %channel_id,
                        error = %err,
                        "WebRTC audio send_input failed"
                    );
                }
            }
        });
        Box::pin(async {})
    }));
}

async fn pump_observations_to_data_channel(
    context: DataChannelPumpContext,
    data_channel: Arc<RTCDataChannel>,
) {
    let DataChannelPumpContext {
        host,
        channel_id,
        peer,
        peer_registry,
        close_feedback,
        status_feedback,
        outgoing_audio_tx,
        outgoing_audio_control,
    } = context;
    let mut bridge = match WebrtcAudioBridge::new() {
        Ok(bridge) => bridge,
        Err(err) => {
            tracing::warn!(channel = %channel_id, error = %err, "failed to initialize WebRTC output audio bridge");
            close_channel_with_generated_feedback(
                host.as_ref(),
                close_feedback.as_ref(),
                &channel_id,
            )
            .await;
            close_and_deregister_peer(&peer_registry, &channel_id, &data_channel, &peer).await;
            return;
        }
    };
    let mut close_feedback_recorded = false;
    loop {
        let observation = match host.next_observation_raw(&channel_id).await {
            Ok(Some(obs)) => obs,
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(channel = %channel_id, error = %err, "WebRTC observation pump failed");
                break;
            }
        };

        let close_observation = observation_requires_generated_close(&observation);
        let publish_observation = !matches!(&observation, &LiveAdapterObservation::Error { .. });
        if close_observation {
            if !close_channel_with_generated_feedback(
                host.as_ref(),
                close_feedback.as_ref(),
                &channel_id,
            )
            .await
            {
                break;
            }
            close_feedback_recorded = true;
        } else if !commit_status_with_generated_feedback(
            host.as_ref(),
            status_feedback.as_ref(),
            &channel_id,
            &observation,
        )
        .await
        {
            break;
        }

        let outcome = match host.apply_observation(&channel_id, &observation).await {
            Ok(outcome) => outcome,
            Err(err) => {
                tracing::warn!(channel = %channel_id, error = %err, "apply_observation failed");
                break;
            }
        };

        if publish_observation
            && let Err(err) = forward_observation_json(&data_channel, &observation).await
        {
            tracing::warn!(channel = %channel_id, error = %err, "failed to send WebRTC data-channel observation");
            break;
        }

        if let LiveAdapterObservation::AssistantAudioChunk {
            data,
            sample_rate_hz,
            channels,
            ..
        } = &observation
            && *sample_rate_hz == PROVIDER_PCM_SAMPLE_RATE
            && *channels == MONO_CHANNELS
        {
            match bridge.encode_provider_pcm24k_chunk(data) {
                Ok(packets) => {
                    let generation = outgoing_audio_control.generation();
                    for packet in packets {
                        match outgoing_audio_tx.try_send(OutgoingAudioPacket {
                            generation,
                            data: packet,
                        }) {
                            Ok(()) => outgoing_audio_control.note_queued(),
                            Err(err) => {
                                tracing::warn!(
                                    channel = %channel_id,
                                    error = %err,
                                    "dropping WebRTC output audio packet because RTP pacing queue is full or closed"
                                );
                            }
                        }
                    }
                }
                Err(err) => {
                    tracing::warn!(channel = %channel_id, error = %err, "failed to encode provider PCM for WebRTC");
                }
            }
        }

        match outcome {
            ObservationOutcome::Terminal { code } => {
                tracing::info!(
                    channel = %channel_id,
                    ?code,
                    "WebRTC live channel reached terminal observation"
                );
                break;
            }
            ObservationOutcome::CommandRejected { code, message } => {
                tracing::info!(
                    channel = %channel_id,
                    ?code,
                    %message,
                    "live command rejected; WebRTC peer remains open"
                );
            }
            ObservationOutcome::InterruptSignalled | ObservationOutcome::TranscriptTruncated => {
                let generation = outgoing_audio_control.discard_queued();
                tracing::debug!(
                    channel = %channel_id,
                    generation,
                    "discarding queued WebRTC output audio after live interruption"
                );
            }
            _ => {}
        }
        if close_observation {
            break;
        }
    }

    if !close_feedback_recorded {
        close_channel_with_generated_feedback(host.as_ref(), close_feedback.as_ref(), &channel_id)
            .await;
    }
    close_and_deregister_peer(&peer_registry, &channel_id, &data_channel, &peer).await;
}

async fn commit_status_with_generated_feedback(
    host: &LiveAdapterHost,
    status_feedback: &dyn LiveChannelStatusFeedback,
    channel_id: &LiveChannelId,
    observation: &LiveAdapterObservation,
) -> bool {
    let status = match LiveAdapterHost::classify_observation(observation) {
        ObservationRouting::UpdateStatus(status) => status,
        _ => return true,
    };
    if status.is_terminal() {
        return true;
    }

    let status_observation = match host
        .reserve_channel_status_observation(channel_id, status)
        .await
    {
        Ok(observation) => observation,
        Err(LiveAdapterHostError::ChannelNotFound(_)) => return false,
        Err(err) => {
            tracing::warn!(
                channel = %channel_id,
                error = %err,
                "live WebRTC transport host status observation reservation failed"
            );
            return false;
        }
    };
    let authority = match status_feedback
        .record_live_channel_status(channel_id, &status_observation)
        .await
    {
        Ok(authority) => authority,
        Err(err) => {
            tracing::warn!(
                channel = %channel_id,
                error = %err,
                "generated live status feedback rejected WebRTC transport status"
            );
            return false;
        }
    };
    if let Err(err) = host
        .commit_channel_status_observation(&status_observation, &authority)
        .await
    {
        tracing::warn!(
            channel = %channel_id,
            error = %err,
            "live WebRTC transport host status commit failed after generated feedback"
        );
        return false;
    }
    true
}

async fn close_channel_with_generated_feedback(
    host: &LiveAdapterHost,
    close_feedback: &dyn LiveChannelCloseFeedback,
    channel_id: &LiveChannelId,
) -> bool {
    // Runtime-initiated terminal paths can commit generated close authority
    // before the WebRTC pump drains the staged terminal observation. In that
    // case the committed host status is the generated close handoff result;
    // asking close feedback for a second decision would route through an
    // already-cleared active binding.
    match host.generated_close_has_committed(channel_id).await {
        Ok(true) => return true,
        Ok(false) => {}
        Err(LiveAdapterHostError::ChannelNotFound(_)) => return false,
        Err(err) => {
            tracing::warn!(
                channel = %channel_id,
                error = %err,
                "live WebRTC transport generated close status check failed"
            );
            return false;
        }
    }

    let observation = match host.reserve_channel_close_observation(channel_id).await {
        Ok(observation) => observation,
        Err(LiveAdapterHostError::ChannelNotFound(_)) => return false,
        Err(err) => {
            tracing::warn!(
                channel = %channel_id,
                error = %err,
                "live WebRTC transport host close observation reservation failed"
            );
            return false;
        }
    };
    let authority = match close_feedback
        .record_live_channel_closed(channel_id, &observation)
        .await
    {
        Ok(authority) => authority,
        Err(err) => {
            tracing::warn!(
                channel = %channel_id,
                error = %err,
                "generated live close feedback rejected WebRTC transport close"
            );
            return false;
        }
    };
    if let Err(err) = host
        .commit_channel_close_observation(&observation, &authority)
        .await
    {
        tracing::warn!(
            channel = %channel_id,
            error = %err,
            "live WebRTC transport host close commit failed after generated feedback"
        );
        return false;
    }
    true
}

async fn close_and_deregister_peer(
    peer_registry: &LiveWebrtcPeerRegistry,
    channel_id: &LiveChannelId,
    data_channel: &RTCDataChannel,
    peer: &RTCPeerConnection,
) {
    let registered_peer = peer_registry.lock().await.remove(channel_id);
    let _ = data_channel.close().await;
    if let Some(registered_peer) = registered_peer {
        let _ = registered_peer.peer.close().await;
    } else {
        let _ = peer.close().await;
    }
}

fn spawn_outgoing_audio_track_pump(
    channel_id: LiveChannelId,
    outgoing_audio: Arc<TrackLocalStaticSample>,
    mut audio_rx: mpsc::Receiver<OutgoingAudioPacket>,
    outgoing_audio_control: Arc<OutgoingAudioControl>,
) {
    tokio::spawn(async move {
        while let Some(packet) = audio_rx.recv().await {
            outgoing_audio_control.note_dequeued();
            if packet.generation != outgoing_audio_control.generation() {
                continue;
            }
            if let Err(err) = outgoing_audio
                .write_sample(&Sample {
                    data: Bytes::from(packet.data),
                    duration: WEBRTC_AUDIO_PACKET_DURATION,
                    ..Default::default()
                })
                .await
            {
                tracing::warn!(channel = %channel_id, error = %err, "failed to write paced WebRTC audio sample");
                break;
            }
            tokio::time::sleep(WEBRTC_AUDIO_PACKET_DURATION).await;
        }
    });
}

async fn forward_observation_json(
    data_channel: &RTCDataChannel,
    observation: &LiveAdapterObservation,
) -> Result<(), LiveWebrtcError> {
    let wire = WireLiveAdapterObservation::from(observation.clone());
    let json = serde_json::to_string(&wire)?;
    data_channel
        .send_text(json)
        .await
        .map(|_| ())
        .map_err(|err| LiveWebrtcError::Webrtc(err.to_string()))
}

/// Stateful Opus/resampling bridge for browser WebRTC audio and provider PCM.
pub struct WebrtcAudioBridge {
    opus_decoder: Decoder,
    opus_encoder: Encoder,
    pending_output_48k: Vec<i16>,
}

impl WebrtcAudioBridge {
    pub fn new() -> Result<Self, LiveWebrtcError> {
        Ok(Self {
            opus_decoder: Decoder::new(BROWSER_OPUS_SAMPLE_RATE, Channels::Mono)
                .map_err(|err| LiveWebrtcError::Audio(err.to_string()))?,
            opus_encoder: Encoder::new(
                BROWSER_OPUS_SAMPLE_RATE,
                Channels::Mono,
                Application::Audio,
            )
            .map_err(|err| LiveWebrtcError::Audio(err.to_string()))?,
            pending_output_48k: Vec::new(),
        })
    }

    /// Decode one browser Opus RTP payload into provider PCM 24 kHz mono bytes.
    pub fn decode_browser_opus_payload(
        &mut self,
        payload: &[u8],
    ) -> Result<Vec<u8>, LiveWebrtcError> {
        let mut decoded = vec![0_i16; OPUS_MAX_DECODED_SAMPLES_48K];
        let frames = self
            .opus_decoder
            .decode(payload, &mut decoded, false)
            .map_err(|err| LiveWebrtcError::Audio(err.to_string()))?;
        decoded.truncate(frames);
        let pcm_24k = resample_i16_mono(
            &decoded,
            BROWSER_OPUS_SAMPLE_RATE as usize,
            PROVIDER_PCM_SAMPLE_RATE as usize,
        )?;
        Ok(i16_to_le_bytes(&pcm_24k))
    }

    /// Convert provider PCM 24 kHz mono bytes into 20 ms Opus packets for WebRTC.
    pub fn encode_provider_pcm24k_chunk(
        &mut self,
        pcm_24k_le: &[u8],
    ) -> Result<Vec<Vec<u8>>, LiveWebrtcError> {
        let pcm_24k = le_bytes_to_i16(pcm_24k_le)?;
        let pcm_48k = resample_i16_mono(
            &pcm_24k,
            PROVIDER_PCM_SAMPLE_RATE as usize,
            BROWSER_OPUS_SAMPLE_RATE as usize,
        )?;
        self.pending_output_48k.extend(pcm_48k);

        let mut packets = Vec::new();
        while self.pending_output_48k.len() >= OPUS_20MS_SAMPLES_48K {
            let frame: Vec<i16> = self
                .pending_output_48k
                .drain(..OPUS_20MS_SAMPLES_48K)
                .collect();
            let mut packet = vec![0_u8; OPUS_MAX_PACKET_BYTES];
            let len = self
                .opus_encoder
                .encode(&frame, &mut packet)
                .map_err(|err| LiveWebrtcError::Audio(err.to_string()))?;
            packet.truncate(len);
            packets.push(packet);
        }
        Ok(packets)
    }
}

fn resample_i16_mono(
    input: &[i16],
    input_rate: usize,
    output_rate: usize,
) -> Result<Vec<i16>, LiveWebrtcError> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    if input_rate == output_rate {
        return Ok(input.to_vec());
    }
    if input_rate == 48_000 && output_rate == 24_000 {
        return Ok(downsample_48k_to_24k(input));
    }
    if input_rate == 24_000 && output_rate == 48_000 {
        return Ok(upsample_24k_to_48k(input));
    }
    let frames = input.len();
    let input_f64 = input
        .iter()
        .map(|sample| f64::from(*sample) / f64::from(i16::MAX))
        .collect::<Vec<_>>();
    let input_adapter = InterleavedSlice::new(&input_f64, 1, frames)
        .map_err(|err| LiveWebrtcError::Audio(err.to_string()))?;
    let output_capacity = ((frames * output_rate).div_ceil(input_rate)).saturating_add(1024);
    let mut output = vec![0.0_f64; output_capacity];
    let mut output_adapter = InterleavedSlice::new_mut(&mut output, 1, output_capacity)
        .map_err(|err| LiveWebrtcError::Audio(err.to_string()))?;
    let mut resampler = Fft::<f64>::new(input_rate, output_rate, frames, 1, 1, FixedSync::Input)
        .map_err(|err| LiveWebrtcError::Audio(err.to_string()))?;
    let (_input_frames, output_frames) = resampler
        .process_all_into_buffer(&input_adapter, &mut output_adapter, frames, None)
        .map_err(|err| LiveWebrtcError::Audio(err.to_string()))?;
    Ok(output
        .into_iter()
        .take(output_frames)
        .map(|sample| {
            (sample.clamp(-1.0, 1.0) * f64::from(i16::MAX))
                .round()
                .clamp(f64::from(i16::MIN), f64::from(i16::MAX)) as i16
        })
        .collect())
}

fn downsample_48k_to_24k(input: &[i16]) -> Vec<i16> {
    let mut output = Vec::with_capacity(input.len() / 2 + input.len() % 2);
    let mut chunks = input.chunks_exact(2);
    for pair in &mut chunks {
        let mixed = (i32::from(pair[0]) + i32::from(pair[1])) / 2;
        output.push(mixed.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
    }
    if let Some(last) = chunks.remainder().first() {
        output.push(*last);
    }
    output
}

fn upsample_24k_to_48k(input: &[i16]) -> Vec<i16> {
    let mut output = Vec::with_capacity(input.len() * 2);
    for (idx, sample) in input.iter().enumerate() {
        output.push(*sample);
        let next = input.get(idx + 1).copied().unwrap_or(*sample);
        let interpolated = (i32::from(*sample) + i32::from(next)) / 2;
        output.push(interpolated.clamp(i32::from(i16::MIN), i32::from(i16::MAX)) as i16);
    }
    output
}

fn i16_to_le_bytes(samples: &[i16]) -> Vec<u8> {
    let mut out = Vec::with_capacity(samples.len() * 2);
    for sample in samples {
        out.extend_from_slice(&sample.to_le_bytes());
    }
    out
}

fn le_bytes_to_i16(bytes: &[u8]) -> Result<Vec<i16>, LiveWebrtcError> {
    if !bytes.len().is_multiple_of(2) {
        return Err(LiveWebrtcError::Audio(
            "PCM payload length must be an even number of bytes".to_string(),
        ));
    }
    Ok(bytes
        .chunks_exact(2)
        .map(|chunk| i16::from_le_bytes([chunk[0], chunk[1]]))
        .collect())
}

fn pcm24k_le_has_speech(bytes: &[u8]) -> bool {
    if bytes.len() < 2 || !bytes.len().is_multiple_of(2) {
        return false;
    }
    let mut sum_squares = 0.0_f64;
    let mut count = 0_u32;
    for chunk in bytes.chunks_exact(2) {
        let sample = f64::from(i16::from_le_bytes([chunk[0], chunk[1]])) / f64::from(i16::MAX);
        sum_squares += sample * sample;
        count += 1;
    }
    if count == 0 {
        return false;
    }
    let rms = (sum_squares / f64::from(count)).sqrt() as f32;
    rms >= LOCAL_BARGE_IN_RMS_THRESHOLD
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use crate::host::NoOpProjectionSink;
    use async_trait::async_trait;
    use meerkat_core::live_adapter::{
        LiveAdapter, LiveAdapterCommand, LiveAdapterError, LiveAdapterStatus,
    };
    use meerkat_core::types::SessionId;
    use tokio::sync::mpsc;

    /// rustls 0.23 requires a process-global default `CryptoProvider`. webrtc's
    /// DTLS handshake (driven at connect time by these tests) panics without one
    /// whenever more than one provider feature is active in the build graph —
    /// e.g. under a full `--workspace --all-features` union that pulls both
    /// `ring` and `aws-lc-rs` through other crates, which disables rustls's
    /// implicit single-provider default. Install `ring` explicitly, idempotently,
    /// so the webrtc tests are robust across feature combinations.
    fn ensure_test_crypto_provider() {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let _ = rustls::crypto::ring::default_provider().install_default();
        });
    }

    async fn new_browser_peer() -> RTCPeerConnection {
        ensure_test_crypto_provider();
        let mut media_engine = MediaEngine::default();
        media_engine.register_default_codecs().unwrap();
        APIBuilder::new()
            .with_media_engine(media_engine)
            .build()
            .new_peer_connection(RTCConfiguration::default())
            .await
            .unwrap()
    }

    async fn connect_browser_peer(
        browser_peer: &RTCPeerConnection,
        state: &LiveWebrtcState,
        channel_id: &LiveChannelId,
    ) {
        let offer = browser_peer.create_offer(None).await.unwrap();
        let mut gathering_complete = browser_peer.gathering_complete_promise().await;
        browser_peer.set_local_description(offer).await.unwrap();
        let _ = gathering_complete.recv().await;
        let offer_sdp = browser_peer.local_description().await.unwrap().sdp;
        let answer = state
            .answer_offer(channel_id.clone(), offer_sdp)
            .await
            .unwrap();
        browser_peer
            .set_remote_description(RTCSessionDescription::answer(answer.answer_sdp).unwrap())
            .await
            .unwrap();
    }

    #[tokio::test]
    async fn webrtc_token_ttl_is_transport_configuration_only() {
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let state = LiveWebrtcState::with_token_ttl(
            Arc::clone(&host),
            Arc::new(
                crate::transport::GeneratedTestMachineLiveChannelCloseFeedback::new(Arc::clone(
                    &host,
                )),
            ),
            Arc::new(
                crate::transport::GeneratedTestMachineLiveChannelStatusFeedback::new(Arc::clone(
                    &host,
                )),
            ),
            Duration::from_millis(20),
        );
        let token = state.mint_token(LiveChannelId::new("ch")).await;
        assert!(!token.as_str().is_empty());
        assert_eq!(state.token_ttl(), Duration::from_millis(20));
    }

    #[test]
    fn audio_bridge_round_trips_browser_opus_to_provider_pcm_and_back() {
        let mut bridge = WebrtcAudioBridge::new().unwrap();
        let pcm48: Vec<i16> = (0..OPUS_20MS_SAMPLES_48K)
            .map(|idx| {
                let phase = idx as f32 / OPUS_20MS_SAMPLES_48K as f32;
                (phase.sin() * i16::MAX as f32 * 0.2) as i16
            })
            .collect();
        let mut encoder =
            Encoder::new(BROWSER_OPUS_SAMPLE_RATE, Channels::Mono, Application::Audio).unwrap();
        let mut opus_packet = vec![0_u8; OPUS_MAX_PACKET_BYTES];
        let len = encoder.encode(&pcm48, &mut opus_packet).unwrap();
        opus_packet.truncate(len);

        let pcm24_bytes = bridge.decode_browser_opus_payload(&opus_packet).unwrap();
        assert_eq!(pcm24_bytes.len(), 480 * 2);
        assert!(pcm24_bytes.iter().any(|byte| *byte != 0));

        let packets = bridge.encode_provider_pcm24k_chunk(&pcm24_bytes).unwrap();
        assert!(!packets.is_empty());
        assert!(packets[0].iter().any(|byte| *byte != 0));
    }

    #[test]
    fn local_barge_in_speech_detector_ignores_silence_and_detects_voice_level_pcm() {
        let silence = i16_to_le_bytes(&vec![0_i16; 480]);
        assert!(!pcm24k_le_has_speech(&silence));

        let speech_like = i16_to_le_bytes(&vec![2_500_i16; 480]);
        assert!(pcm24k_le_has_speech(&speech_like));
    }

    #[test]
    fn outgoing_audio_control_invalidates_queued_packets_on_discard() {
        let control = OutgoingAudioControl::default();
        let generation = control.generation();
        control.note_queued();
        control.note_queued();
        assert_eq!(control.queued_packets(), 2);

        let next_generation = control.discard_queued();
        assert!(next_generation > generation);
        assert_eq!(control.queued_packets(), 0);
    }

    #[tokio::test]
    async fn webrtc_peer_data_channel_and_audio_reach_live_adapter_host() {
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(SessionId::new())
            .await
            .unwrap();
        let (adapter, mut command_rx, observation_tx) = RecordingAdapter::new();
        host.attach_adapter(&channel_id, adapter).await.unwrap();
        host.commit_status_with_generated_test_machine_authority(
            &channel_id,
            LiveAdapterStatus::Ready,
        )
        .await
        .unwrap();

        let state = LiveWebrtcState::new_for_test_with_generated_close_feedback(Arc::clone(&host));
        let browser_peer = new_browser_peer().await;

        let (data_tx, mut data_rx) = mpsc::channel::<String>(4);
        let data_channel = browser_peer
            .create_data_channel("meerkat.live", None)
            .await
            .unwrap();
        data_channel.on_message(Box::new(move |message: DataChannelMessage| {
            let data_tx = data_tx.clone();
            Box::pin(async move {
                if message.is_string
                    && let Ok(text) = String::from_utf8(message.data.to_vec())
                {
                    let _ = data_tx.send(text).await;
                }
            })
        }));
        let (open_tx, mut open_rx) = mpsc::channel::<()>(1);
        data_channel.on_open(Box::new(move || {
            let open_tx = open_tx.clone();
            Box::pin(async move {
                let _ = open_tx.send(()).await;
            })
        }));

        let (remote_audio_tx, mut remote_audio_rx) = mpsc::channel::<usize>(1);
        browser_peer.on_track(Box::new(move |track: Arc<TrackRemote>, _, _| {
            let remote_audio_tx = remote_audio_tx.clone();
            tokio::spawn(async move {
                if let Ok((packet, _)) = track.read_rtp().await {
                    let _ = remote_audio_tx.send(packet.payload.len()).await;
                }
            });
            Box::pin(async {})
        }));

        let outbound_audio: Arc<dyn TrackLocal + Send + Sync> =
            Arc::new(TrackLocalStaticSample::new(
                RTCRtpCodecCapability {
                    mime_type: MIME_TYPE_OPUS.to_owned(),
                    clock_rate: BROWSER_OPUS_SAMPLE_RATE,
                    channels: 1,
                    ..Default::default()
                },
                "audio".to_owned(),
                "browser-test".to_owned(),
            ));
        browser_peer
            .add_track(Arc::clone(&outbound_audio))
            .await
            .unwrap();

        connect_browser_peer(&browser_peer, &state, &channel_id).await;
        tokio::time::timeout(Duration::from_secs(5), open_rx.recv())
            .await
            .unwrap()
            .unwrap();

        data_channel
            .send_text(r#"{"kind":"text","text":"hello"}"#.to_owned())
            .await
            .unwrap();
        match tokio::time::timeout(Duration::from_secs(5), command_rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            LiveAdapterCommand::SendInput {
                chunk: LiveInputChunk::Text { text },
            } => assert_eq!(text, "hello"),
            other => panic!("expected text send_input, got {other:?}"),
        }

        let audio_track = outbound_audio
            .as_any()
            .downcast_ref::<TrackLocalStaticSample>()
            .unwrap();
        audio_track
            .write_sample(&Sample {
                data: Bytes::from(make_opus_packet()),
                duration: Duration::from_millis(20),
                ..Default::default()
            })
            .await
            .unwrap();
        match tokio::time::timeout(Duration::from_secs(5), command_rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            LiveAdapterCommand::SendInput {
                chunk:
                    LiveInputChunk::Audio {
                        data,
                        sample_rate_hz,
                        channels,
                    },
            } => {
                assert_eq!(sample_rate_hz, PROVIDER_PCM_SAMPLE_RATE);
                assert_eq!(channels, MONO_CHANNELS);
                assert_eq!(data.len(), 480 * 2);
            }
            other => panic!("expected audio send_input, got {other:?}"),
        }

        observation_tx
            .send(LiveAdapterObservation::AssistantAudioChunk {
                data: make_provider_pcm_24k(),
                sample_rate_hz: PROVIDER_PCM_SAMPLE_RATE,
                channels: MONO_CHANNELS,
                response_id: Some("resp_1".into()),
                item_id: Some("item_1".into()),
                content_index: Some(0),
            })
            .await
            .unwrap();
        let observation_json = tokio::time::timeout(Duration::from_secs(5), data_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(observation_json.contains(r#""observation":"assistant_audio_chunk""#));
        let payload_len = tokio::time::timeout(Duration::from_secs(5), remote_audio_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert!(payload_len > 0);

        state.close_peer(&channel_id).await;
        browser_peer.close().await.unwrap();
    }

    #[tokio::test]
    async fn webrtc_peer_deregisters_when_observation_stream_closes() {
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(SessionId::new())
            .await
            .unwrap();
        let (adapter, _command_rx, observation_tx) = RecordingAdapter::new();
        host.attach_adapter(&channel_id, adapter).await.unwrap();
        host.commit_status_with_generated_test_machine_authority(
            &channel_id,
            LiveAdapterStatus::Ready,
        )
        .await
        .unwrap();

        let state = LiveWebrtcState::new_for_test_with_generated_close_feedback(Arc::clone(&host));
        let browser_peer = new_browser_peer().await;

        let data_channel = browser_peer
            .create_data_channel("meerkat.live", None)
            .await
            .unwrap();
        let (open_tx, mut open_rx) = mpsc::channel::<()>(1);
        data_channel.on_open(Box::new(move || {
            let open_tx = open_tx.clone();
            Box::pin(async move {
                let _ = open_tx.send(()).await;
            })
        }));

        let outbound_audio: Arc<dyn TrackLocal + Send + Sync> =
            Arc::new(TrackLocalStaticSample::new(
                RTCRtpCodecCapability {
                    mime_type: MIME_TYPE_OPUS.to_owned(),
                    clock_rate: BROWSER_OPUS_SAMPLE_RATE,
                    channels: 1,
                    ..Default::default()
                },
                "audio".to_owned(),
                "browser-test".to_owned(),
            ));
        browser_peer
            .add_track(Arc::clone(&outbound_audio))
            .await
            .unwrap();

        connect_browser_peer(&browser_peer, &state, &channel_id).await;
        tokio::time::timeout(Duration::from_secs(5), open_rx.recv())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(state.peer_count().await, 1);

        drop(observation_tx);
        wait_for_peer_count(&state, 0).await;
        browser_peer.close().await.unwrap();
    }

    async fn wait_for_peer_count(state: &LiveWebrtcState, expected: usize) {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if state.peer_count().await == expected {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }

    fn make_opus_packet() -> Vec<u8> {
        let pcm48: Vec<i16> = (0..OPUS_20MS_SAMPLES_48K)
            .map(|idx| {
                let phase = idx as f32 / OPUS_20MS_SAMPLES_48K as f32;
                (phase.sin() * i16::MAX as f32 * 0.2) as i16
            })
            .collect();
        let mut encoder =
            Encoder::new(BROWSER_OPUS_SAMPLE_RATE, Channels::Mono, Application::Audio).unwrap();
        let mut packet = vec![0_u8; OPUS_MAX_PACKET_BYTES];
        let len = encoder.encode(&pcm48, &mut packet).unwrap();
        packet.truncate(len);
        packet
    }

    fn make_provider_pcm_24k() -> Vec<u8> {
        let pcm24: Vec<i16> = (0..480)
            .map(|idx| {
                let phase = idx as f32 / 480.0;
                (phase.sin() * i16::MAX as f32 * 0.2) as i16
            })
            .collect();
        i16_to_le_bytes(&pcm24)
    }

    struct RecordingAdapter {
        command_tx: mpsc::Sender<LiveAdapterCommand>,
        observation_rx: Mutex<mpsc::Receiver<LiveAdapterObservation>>,
    }

    impl RecordingAdapter {
        fn new() -> (
            Arc<Self>,
            mpsc::Receiver<LiveAdapterCommand>,
            mpsc::Sender<LiveAdapterObservation>,
        ) {
            let (command_tx, command_rx) = mpsc::channel(16);
            let (observation_tx, observation_rx) = mpsc::channel(16);
            (
                Arc::new(Self {
                    command_tx,
                    observation_rx: Mutex::new(observation_rx),
                }),
                command_rx,
                observation_tx,
            )
        }
    }

    #[async_trait]
    impl LiveAdapter for RecordingAdapter {
        async fn send_command(&self, command: LiveAdapterCommand) -> Result<(), LiveAdapterError> {
            self.command_tx
                .send(command)
                .await
                .map_err(|_| LiveAdapterError::Closed)
        }

        async fn next_observation(
            &self,
        ) -> Result<Option<LiveAdapterObservation>, LiveAdapterError> {
            Ok(self.observation_rx.lock().await.recv().await)
        }

        fn status(&self) -> LiveAdapterStatus {
            LiveAdapterStatus::Ready
        }

        async fn close(&self) -> Result<(), LiveAdapterError> {
            Ok(())
        }
    }
}
