//! Live WebRTC transport — browser WebRTC terminator over `LiveAdapterHost`.
//!
//! This module is behind the non-default `webrtc` feature because it owns the
//! media stack: SDP, ICE/DTLS/SRTP, RTP, Opus, and resampling. Surfaces only
//! compose this state and expose signaling; live channel semantics stay in
//! `LiveAdapterHost`.
//!
//! Data-channel observations and RTP audio are independently transported.
//! Because rust-webrtc's callback reader is limited to 65,535-byte messages,
//! image input for a WebRTC channel goes through JSON-RPC `live/send_input`,
//! not the data channel. An image envelope delivered whole within that ceiling
//! receives the scoped `image_input_transport_unsupported` observation; a
//! larger envelope can fail in the browser/SCTP transport before Meerkat can
//! classify it and therefore cannot be promised a server-side rejection. A
//! client must then wait for the redacted `user_content_committed` data-channel
//! observation before starting RTP audio that depends on the image.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex, Weak};
use std::time::Duration;

use axum::Router;
use bytes::Bytes;
use meerkat_contracts::{LiveInputChunkWire, WireLiveAdapterObservation};
use meerkat_core::live_adapter::{
    LiveAdapterErrorCode, LiveAdapterObservation, LiveConfigRejectionReason, LiveInputChunk,
};
use opus::{Application, Channels, Decoder, Encoder};
use rubato::audioadapter_buffers::direct::InterleavedSlice;
use rubato::{Fft, FixedSync, Resampler};
use tokio::sync::Mutex;
use tokio::sync::mpsc;
use tokio::sync::{Notify, watch};
use webrtc::api::APIBuilder;
use webrtc::api::media_engine::{MIME_TYPE_OPUS, MediaEngine};
use webrtc::data_channel::RTCDataChannel;
use webrtc::data_channel::data_channel_message::DataChannelMessage;
use webrtc::peer_connection::RTCPeerConnection;
use webrtc::peer_connection::configuration::RTCConfiguration;
use webrtc::peer_connection::peer_connection_state::RTCPeerConnectionState;
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
    live_ws_router, should_publish_observation,
};
use crate::wire_input::{live_input_chunk_decode_rejection, live_input_chunk_from_wire};

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

fn webrtc_data_channel_image_rejection() -> LiveAdapterObservation {
    LiveAdapterObservation::CommandRejected {
        code: LiveAdapterErrorCode::ConfigRejected {
            reason: LiveConfigRejectionReason::ImageInputTransportUnsupported {
                transport: "webrtc_data_channel_use_live_send_input_rpc".to_string(),
            },
        },
        message: "WebRTC data-channel image frames are unsupported; send the image with JSON-RPC live/send_input and wait for user_content_committed before dependent RTP audio".to_string(),
    }
}

struct OutgoingAudioPacket {
    generation: u64,
    data: Vec<u8>,
}

#[derive(Default)]
struct OutgoingAudioControl {
    generation: AtomicU64,
    queued_packets: AtomicUsize,
    /// D223/K16: count of output-audio packets dropped because the RTP pacing
    /// queue was full or closed. The cumulative count is lowered into the
    /// live-host signal seam (`LiveAdapterHost::signal_output_audio_degraded`)
    /// once per degraded chunk, so the session observes delivery degradation
    /// as a typed fact — this counter is the running total feeding that
    /// signal, not a transport-local truth with no live reader.
    dropped_packets: AtomicU64,
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

    /// D223: record a dropped output-audio packet (RTP pacing queue full or
    /// closed) and return the new cumulative drop count.
    fn note_dropped(&self) -> u64 {
        self.dropped_packets.fetch_add(1, Ordering::Relaxed) + 1
    }

    #[cfg(test)]
    fn dropped_packets(&self) -> u64 {
        self.dropped_packets.load(Ordering::Relaxed)
    }
}

/// Errors returned by the WebRTC transport/signaling layer.
///
/// D124: the signaling/audio failure classes are typed per phase so transports
/// and the RPC answer-admission path route on a typed variant rather than
/// reparsing a single `Webrtc(String)` / `Audio(String)` blob. Each variant
/// carries the underlying webrtc-crate / opus / resampler prose in a `detail`
/// field — that detail is a leaf string we do not own — but the *phase* (which
/// signaling or audio step failed) is the typed discriminant. `ChannelNotFound`
/// / `MissingLocalDescription` / `Json` are the existing typed precedents this
/// extends.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum LiveWebrtcError {
    #[error("channel not found: {0}")]
    ChannelNotFound(String),
    /// Default-codec registration on the media engine failed.
    #[error("WebRTC codec registration failed: {detail}")]
    CodecRegistration { detail: String },
    /// Creating the peer connection (or adding its output track) failed.
    #[error("WebRTC peer creation failed: {detail}")]
    PeerCreation { detail: String },
    /// Parsing or applying the remote (browser) SDP offer failed.
    #[error("WebRTC set-remote-description failed: {detail}")]
    SetRemoteDescription { detail: String },
    /// Creating or applying the local SDP answer failed.
    #[error("WebRTC create-answer failed: {detail}")]
    CreateAnswer { detail: String },
    /// Sending an observation frame over the data channel failed.
    #[error("WebRTC data-channel send failed: {detail}")]
    DataChannelSend { detail: String },
    /// An Opus encode/decode or PCM resampling step failed.
    #[error("audio bridge error: {detail}")]
    Audio { detail: String },
    #[error("JSON frame error: {0}")]
    Json(serde_json::Error),
    #[error("missing local WebRTC description after answer")]
    MissingLocalDescription,
    #[error("WebRTC peer close failed: {detail}")]
    PeerClose { detail: String },
    /// A fallible answer-construction step failed and the mandatory physical
    /// cleanup also failed. The original phase remains visible in `operation`;
    /// the close failure is the reason code because physical custody could not
    /// be retired cleanly.
    #[error("{operation}; WebRTC construction cleanup failed: {close_detail}")]
    ConstructionCleanup {
        operation: String,
        close_detail: String,
    },
}

impl LiveWebrtcError {
    /// Stable typed reason code for this WebRTC error (D124).
    ///
    /// The RPC answer-admission path maps each code to a distinct generated
    /// rejection reason. Exhaustive over the `#[non_exhaustive]` enum from
    /// inside the crate so a new variant must add its code here.
    #[must_use]
    pub fn reason_code(&self) -> &'static str {
        match self {
            Self::ChannelNotFound(_) => "channel_not_found",
            Self::CodecRegistration { .. } => "codec_registration",
            Self::PeerCreation { .. } => "peer_creation",
            Self::SetRemoteDescription { .. } => "set_remote_description",
            Self::CreateAnswer { .. } => "create_answer",
            Self::DataChannelSend { .. } => "data_channel_send",
            Self::Audio { .. } => "audio",
            Self::Json(_) => "json_frame",
            Self::MissingLocalDescription => "missing_local_description",
            Self::PeerClose { .. } => "peer_close",
            Self::ConstructionCleanup { .. } => "peer_close",
        }
    }
}

impl From<serde_json::Error> for LiveWebrtcError {
    fn from(value: serde_json::Error) -> Self {
        Self::Json(value)
    }
}

#[derive(serde::Serialize)]
struct WebrtcErrorFrame<'a> {
    error: String,
    reason: &'a str,
}

struct LiveWebrtcPeer {
    peer: Arc<RTCPeerConnection>,
    outgoing_audio: Arc<OutgoingAudioControl>,
    peer_tasks: Arc<WebrtcPeerTaskControl>,
    answer_observation_sequence: u64,
}

type LiveWebrtcPeerRegistry = Arc<Mutex<HashMap<LiveChannelId, LiveWebrtcPeer>>>;
type LiveWebrtcPendingAnswerRegistry = Arc<Mutex<HashMap<u64, LiveWebrtcPendingAnswer>>>;
type WeakLiveWebrtcPeerRegistry = Weak<Mutex<HashMap<LiveChannelId, LiveWebrtcPeer>>>;
type WeakLiveWebrtcPendingAnswerRegistry = Weak<Mutex<HashMap<u64, LiveWebrtcPendingAnswer>>>;

struct LiveWebrtcPendingAnswer {
    channel_id: LiveChannelId,
    peer: Arc<RTCPeerConnection>,
    peer_tasks: Arc<WebrtcPeerTaskControl>,
}

struct PublishedPeerTeardown {
    channel_id: LiveChannelId,
    peer: Weak<RTCPeerConnection>,
    peer_tasks: Weak<WebrtcPeerTaskControl>,
}

type PublishedPeerCloseFuture = std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<(), LiveWebrtcError>> + Send + 'static>,
>;
type PublishedPeerCloseOperation =
    Box<dyn FnOnce(Arc<RTCPeerConnection>) -> PublishedPeerCloseFuture + Send + 'static>;

#[derive(Clone, Debug)]
struct PublishedPeerCleanupOutcome {
    removed_current: bool,
    error_detail: Option<Arc<str>>,
}

impl PublishedPeerCleanupOutcome {
    fn into_result(self) -> Result<bool, LiveWebrtcError> {
        match self.error_detail {
            Some(detail) => Err(LiveWebrtcError::PeerClose {
                detail: detail.to_string(),
            }),
            None => Ok(self.removed_current),
        }
    }
}

struct PublishedPeerCleanupContext {
    channel_id: LiveChannelId,
    answer_observation_sequence: u64,
    peer: Weak<RTCPeerConnection>,
    peer_registry: WeakLiveWebrtcPeerRegistry,
    pending_answers: WeakLiveWebrtcPendingAnswerRegistry,
    published_peer_lifecycle: Weak<PublishedPeerLifecycleOwner>,
    runtime: tokio::runtime::Handle,
}

/// The one physical-close owner for an answer-published peer.
///
/// Callers only subscribe to the result. The first request moves the one-shot
/// rust-webrtc close into a runtime-owned task, so cancelling a checked close
/// can never cancel the physical effect after the cleanup claim is sticky.
/// Every later request joins the same outcome instead of racing a second
/// `RTCPeerConnection::close` call.
struct PublishedPeerCleanupCoordinator {
    context: PublishedPeerCleanupContext,
    started: AtomicBool,
    outcome_tx: watch::Sender<Option<PublishedPeerCleanupOutcome>>,
}

impl PublishedPeerCleanupCoordinator {
    fn new(context: PublishedPeerCleanupContext) -> Arc<Self> {
        let (outcome_tx, _outcome_rx) = watch::channel(None);
        Arc::new(Self {
            context,
            started: AtomicBool::new(false),
            outcome_tx,
        })
    }

    fn start(self: &Arc<Self>) -> watch::Receiver<Option<PublishedPeerCleanupOutcome>> {
        self.start_with(Box::new(|peer| {
            Box::pin(async move {
                peer.close()
                    .await
                    .map_err(|error| LiveWebrtcError::PeerClose {
                        detail: error.to_string(),
                    })
            })
        }))
    }

    fn start_with(
        self: &Arc<Self>,
        close: PublishedPeerCloseOperation,
    ) -> watch::Receiver<Option<PublishedPeerCleanupOutcome>> {
        let outcome_rx = self.outcome_tx.subscribe();
        if self
            .started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return outcome_rx;
        }

        let coordinator = Arc::clone(self);
        // Capture physical custody before yielding to the runtime. In
        // particular, `LiveWebrtcState::drop` starts this coordinator and then
        // immediately releases the registry roots; upgrading inside the task
        // would race those drops and could skip the required peer close.
        let peer = self.context.peer.upgrade();
        let runtime = self.context.runtime.clone();
        runtime.spawn(async move {
            let outcome = coordinator.run(close, peer).await;
            if let Some(detail) = &outcome.error_detail {
                tracing::warn!(
                    channel = %coordinator.context.channel_id,
                    answer_observation_sequence = coordinator.context.answer_observation_sequence,
                    error = %detail,
                    "owned WebRTC peer cleanup did not complete cleanly"
                );
            }
            coordinator.outcome_tx.send_replace(Some(outcome));
        });
        outcome_rx
    }

    async fn run(
        &self,
        close: PublishedPeerCloseOperation,
        peer: Option<Arc<RTCPeerConnection>>,
    ) -> PublishedPeerCleanupOutcome {
        let Some(peer) = peer else {
            if let Some(owner) = self.context.published_peer_lifecycle.upgrade() {
                owner.remove_dead(self.context.answer_observation_sequence);
            }
            return PublishedPeerCleanupOutcome {
                removed_current: false,
                error_detail: None,
            };
        };

        let close_result = close(Arc::clone(&peer)).await;
        let reached_closed = peer.connection_state() == RTCPeerConnectionState::Closed;
        let removed_current = if reached_closed {
            retire_closed_peer_custody_from_registries(
                &self.context.peer_registry,
                &self.context.pending_answers,
                &self.context.published_peer_lifecycle,
                &self.context.channel_id,
                self.context.answer_observation_sequence,
                peer.as_ref(),
            )
            .await
        } else {
            false
        };
        let error_detail = match close_result {
            Err(error) => Some(Arc::<str>::from(error.to_string())),
            Ok(()) if !reached_closed => Some(Arc::<str>::from(
                "peer close returned success without reaching closed state",
            )),
            Ok(()) => None,
        };
        PublishedPeerCleanupOutcome {
            removed_current,
            error_detail,
        }
    }

    async fn wait_for_outcome(
        mut outcome_rx: watch::Receiver<Option<PublishedPeerCleanupOutcome>>,
    ) -> PublishedPeerCleanupOutcome {
        loop {
            if let Some(outcome) = outcome_rx.borrow_and_update().clone() {
                return outcome;
            }
            if outcome_rx.changed().await.is_err() {
                return PublishedPeerCleanupOutcome {
                    removed_current: false,
                    error_detail: Some(Arc::<str>::from(
                        "owned WebRTC peer cleanup ended without an outcome",
                    )),
                };
            }
        }
    }
}

/// The synchronous owner for every task set and physical peer that crossed
/// the answer-publication boundary.
///
/// rust-webrtc callbacks live inside the peer they describe, so callback/task
/// mechanics may retain only a `Weak` reference to this owner and to the peer
/// registries. The state is therefore the sole lifecycle root. Its `Drop`
/// path can signal parked tasks immediately and move async peer close onto the
/// runtime captured at publication without needing to await in `Drop`.
#[derive(Default)]
struct PublishedPeerLifecycleOwner {
    peers: StdMutex<HashMap<u64, PublishedPeerTeardown>>,
}

impl PublishedPeerLifecycleOwner {
    fn insert(
        &self,
        answer_observation_sequence: u64,
        channel_id: LiveChannelId,
        peer: &Arc<RTCPeerConnection>,
        peer_tasks: &Arc<WebrtcPeerTaskControl>,
    ) {
        self.peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(
                answer_observation_sequence,
                PublishedPeerTeardown {
                    channel_id,
                    peer: Arc::downgrade(peer),
                    peer_tasks: Arc::downgrade(peer_tasks),
                },
            );
    }

    fn remove_exact(
        &self,
        answer_observation_sequence: u64,
        exact_peer: &RTCPeerConnection,
    ) -> bool {
        let mut peers = self
            .peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remove = peers
            .get(&answer_observation_sequence)
            .and_then(|entry| entry.peer.upgrade())
            .is_none_or(|peer| std::ptr::eq(peer.as_ref(), exact_peer));
        if remove {
            peers.remove(&answer_observation_sequence);
        }
        remove
    }

    fn remove_dead(&self, answer_observation_sequence: u64) -> bool {
        let mut peers = self
            .peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remove = peers
            .get(&answer_observation_sequence)
            .is_some_and(|entry| entry.peer.upgrade().is_none());
        if remove {
            peers.remove(&answer_observation_sequence);
        }
        remove
    }

    fn shutdown_all(&self) {
        let peers = std::mem::take(
            &mut *self
                .peers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner),
        );
        for (answer_observation_sequence, teardown) in peers {
            let Some(peer_tasks) = teardown.peer_tasks.upgrade() else {
                tracing::warn!(
                    channel = %teardown.channel_id,
                    answer_observation_sequence,
                    "published WebRTC cleanup coordinator disappeared before owner teardown"
                );
                continue;
            };
            peer_tasks.shutdown();
            if peer_tasks.start_published_cleanup().is_none() {
                tracing::warn!(
                    channel = %teardown.channel_id,
                    answer_observation_sequence,
                    "published WebRTC peer had no installed cleanup coordinator"
                );
            }
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

impl Drop for PublishedPeerLifecycleOwner {
    fn drop(&mut self) {
        self.shutdown_all();
    }
}

struct LiveWebrtcConstructionPeer {
    channel_id: LiveChannelId,
    peer: Arc<RTCPeerConnection>,
}

#[derive(Default)]
struct LiveWebrtcConstructionRegistry {
    peers: StdMutex<HashMap<u64, LiveWebrtcConstructionPeer>>,
    changed: Notify,
}

impl LiveWebrtcConstructionRegistry {
    fn insert(&self, sequence: u64, channel_id: LiveChannelId, peer: Arc<RTCPeerConnection>) {
        self.peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .insert(sequence, LiveWebrtcConstructionPeer { channel_id, peer });
        self.changed.notify_waiters();
    }

    fn remove_exact(&self, sequence: u64, peer: &Arc<RTCPeerConnection>) -> bool {
        let mut peers = self
            .peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let remove = peers
            .get(&sequence)
            .is_some_and(|entry| Arc::ptr_eq(&entry.peer, peer));
        if remove {
            peers.remove(&sequence);
        }
        drop(peers);
        if remove {
            self.changed.notify_waiters();
        }
        remove
    }

    async fn wait_for_channel_absent(&self, channel_id: &LiveChannelId) {
        loop {
            let changed = self.changed.notified();
            tokio::pin!(changed);
            // `notify_waiters` does not retain a permit. Register this waiter
            // before inspecting the registry so removal cannot land between
            // the check and the first poll and strand a lifecycle lease.
            changed.as_mut().enable();
            let present = self
                .peers
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .values()
                .any(|entry| entry.channel_id == *channel_id);
            if !present {
                return;
            }
            changed.as_mut().await;
        }
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.peers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .len()
    }
}

struct WebrtcPeerTaskControl {
    shutdown_tx: watch::Sender<bool>,
    terminal_cleanup_started: AtomicBool,
    answer_observation_sequence: AtomicU64,
    published_cleanup: StdMutex<Option<Arc<PublishedPeerCleanupCoordinator>>>,
}

impl WebrtcPeerTaskControl {
    fn new() -> Self {
        let (shutdown_tx, _shutdown_rx) = watch::channel(false);
        Self {
            shutdown_tx,
            terminal_cleanup_started: AtomicBool::new(false),
            answer_observation_sequence: AtomicU64::new(0),
            published_cleanup: StdMutex::new(None),
        }
    }

    fn subscribe(&self) -> watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    fn shutdown(&self) {
        self.shutdown_tx.send_replace(true);
    }

    fn publish(&self, answer_observation_sequence: u64) {
        self.answer_observation_sequence
            .store(answer_observation_sequence, Ordering::Release);
    }

    fn answer_observation_sequence(&self) -> Option<u64> {
        let sequence = self.answer_observation_sequence.load(Ordering::Acquire);
        (sequence != 0).then_some(sequence)
    }

    fn claim_terminal_cleanup(&self) -> bool {
        self.terminal_cleanup_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_ok()
    }

    fn install_published_cleanup(
        &self,
        context: PublishedPeerCleanupContext,
    ) -> Arc<PublishedPeerCleanupCoordinator> {
        let mut cleanup = self
            .published_cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(cleanup) = cleanup.as_ref() {
            debug_assert_eq!(
                cleanup.context.answer_observation_sequence,
                context.answer_observation_sequence
            );
            return Arc::clone(cleanup);
        }
        let coordinator = PublishedPeerCleanupCoordinator::new(context);
        *cleanup = Some(Arc::clone(&coordinator));
        coordinator
    }

    fn start_published_cleanup(
        &self,
    ) -> Option<watch::Receiver<Option<PublishedPeerCleanupOutcome>>> {
        // A coordinator-started close must not be reclassified as a fresh
        // remote disconnect when rust-webrtc reports its own Closed state.
        // Remote/error paths claim this bit before their owned semantic-close
        // task; checked/rejected/owner paths claim it here to suppress that
        // callback while preserving their existing authority ordering.
        let _ = self.claim_terminal_cleanup();
        self.shutdown();
        self.published_cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(PublishedPeerCleanupCoordinator::start)
    }

    fn start_published_cleanup_with(
        &self,
        close: PublishedPeerCloseOperation,
    ) -> Option<watch::Receiver<Option<PublishedPeerCleanupOutcome>>> {
        let _ = self.claim_terminal_cleanup();
        self.shutdown();
        self.published_cleanup
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .as_ref()
            .map(|coordinator| coordinator.start_with(close))
    }
}

struct AnswerConstructionCustody {
    sequence: u64,
    channel_id: LiveChannelId,
    peer: Option<Arc<RTCPeerConnection>>,
    peer_tasks: Option<Arc<WebrtcPeerTaskControl>>,
    registry: Arc<LiveWebrtcConstructionRegistry>,
}

impl AnswerConstructionCustody {
    fn new(
        sequence: u64,
        channel_id: LiveChannelId,
        peer: Arc<RTCPeerConnection>,
        registry: Arc<LiveWebrtcConstructionRegistry>,
    ) -> Self {
        // This is intentionally synchronous: after peer creation returns there
        // is no cancellation point before exact cleanup custody exists.
        registry.insert(sequence, channel_id.clone(), Arc::clone(&peer));
        Self {
            sequence,
            channel_id,
            peer: Some(peer),
            peer_tasks: None,
            registry,
        }
    }

    fn attach_peer_tasks(&mut self, peer_tasks: Arc<WebrtcPeerTaskControl>) {
        self.peer_tasks = Some(peer_tasks);
    }

    fn disarm_after_publication(mut self) {
        if let Some(peer) = self.peer.take() {
            self.registry.remove_exact(self.sequence, &peer);
        }
        self.peer_tasks.take();
    }

    async fn close_after(mut self, operation: LiveWebrtcError) -> LiveWebrtcError {
        let operation_detail = operation.to_string();
        let Some(cleanup) = self.spawn_cleanup_task() else {
            return LiveWebrtcError::ConstructionCleanup {
                operation: operation_detail,
                close_detail: "construction cleanup custody was already transferred".to_string(),
            };
        };
        match cleanup.await {
            Ok(Ok(())) => operation,
            Ok(Err(close_error)) => LiveWebrtcError::ConstructionCleanup {
                operation: operation_detail,
                close_detail: close_error.to_string(),
            },
            Err(join_error) => LiveWebrtcError::ConstructionCleanup {
                operation: operation_detail,
                close_detail: format!("cleanup task failed: {join_error}"),
            },
        }
    }

    fn spawn_cleanup_task(
        &mut self,
    ) -> Option<tokio::task::JoinHandle<Result<(), LiveWebrtcError>>> {
        let peer = self.peer.take()?;
        let peer_tasks = self.peer_tasks.take();
        let registry = Arc::clone(&self.registry);
        let sequence = self.sequence;
        let channel_id = self.channel_id.clone();
        Some(tokio::spawn(async move {
            let result = close_construction_peer(peer, peer_tasks, registry, sequence).await;
            if let Err(error) = &result {
                tracing::error!(
                    channel = %channel_id,
                    construction_sequence = sequence,
                    error = %error,
                    "WebRTC answer construction cleanup failed"
                );
            }
            result
        }))
    }
}

impl Drop for AnswerConstructionCustody {
    fn drop(&mut self) {
        if self.peer.is_some() {
            // Dropping the answer future means caller cancellation. Move the
            // physical close into an owned task so cancellation cannot cancel
            // cleanup too. The RPC coordinator waits on the registry before
            // releasing its lifecycle lease.
            let _cleanup = self.spawn_cleanup_task();
        }
    }
}

async fn close_construction_peer(
    peer: Arc<RTCPeerConnection>,
    peer_tasks: Option<Arc<WebrtcPeerTaskControl>>,
    registry: Arc<LiveWebrtcConstructionRegistry>,
    sequence: u64,
) -> Result<(), LiveWebrtcError> {
    if let Some(peer_tasks) = &peer_tasks {
        peer_tasks.shutdown();
    }
    let close_result = peer
        .close()
        .await
        .map_err(|error| LiveWebrtcError::PeerClose {
            detail: error.to_string(),
        });
    if peer.connection_state() != RTCPeerConnectionState::Closed {
        return close_result.and_then(|()| {
            Err(LiveWebrtcError::PeerClose {
                detail: "peer close returned success without reaching closed state".to_string(),
            })
        });
    }
    registry.remove_exact(sequence, &peer);
    close_result
}

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

#[cfg(test)]
#[derive(Default)]
struct AnswerConstructionTestState {
    last_peer: StdMutex<Option<Arc<RTCPeerConnection>>>,
    last_data_channel: StdMutex<Option<Weak<RTCDataChannel>>>,
    capture_peer: AtomicBool,
    capture_data_channel: AtomicBool,
    pause: StdMutex<Option<Arc<AnswerConstructionTestPause>>>,
    active_audio_pumps: Arc<AtomicUsize>,
    active_observation_pumps: Arc<AtomicUsize>,
    active_incoming_audio_tasks: Arc<AtomicUsize>,
}

#[cfg(test)]
impl AnswerConstructionTestState {
    fn observe_peer(&self, peer: Arc<RTCPeerConnection>) {
        if !self.capture_peer.load(Ordering::SeqCst) {
            return;
        }
        *self
            .last_peer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(peer);
    }

    fn observe_data_channel(&self, data_channel: &Arc<RTCDataChannel>) {
        if !self.capture_data_channel.load(Ordering::SeqCst) {
            return;
        }
        *self
            .last_data_channel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) =
            Some(Arc::downgrade(data_channel));
    }

    async fn pause_before_negotiation(&self) {
        let pause = self
            .pause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone();
        if let Some(pause) = pause {
            pause.reached.notify_one();
            pause.release.notified().await;
        }
    }
}

#[cfg(test)]
#[derive(Default)]
struct AnswerConstructionTestPause {
    reached: Notify,
    release: Notify,
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
    published_peer_lifecycle: Arc<PublishedPeerLifecycleOwner>,
    peers: LiveWebrtcPeerRegistry,
    pending_answers: LiveWebrtcPendingAnswerRegistry,
    construction_peers: Arc<LiveWebrtcConstructionRegistry>,
    token_ttl: Duration,
    answer_observation_sequence: AtomicU64,
    construction_sequence: AtomicU64,
    #[cfg(test)]
    construction_test_state: Arc<AnswerConstructionTestState>,
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
            published_peer_lifecycle: Arc::new(PublishedPeerLifecycleOwner::default()),
            peers: Arc::new(Mutex::new(HashMap::new())),
            pending_answers: Arc::new(Mutex::new(HashMap::new())),
            construction_peers: Arc::new(LiveWebrtcConstructionRegistry::default()),
            token_ttl,
            answer_observation_sequence: AtomicU64::new(0),
            construction_sequence: AtomicU64::new(0),
            #[cfg(test)]
            construction_test_state: Arc::new(AnswerConstructionTestState::default()),
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
        media_engine.register_default_codecs().map_err(|err| {
            LiveWebrtcError::CodecRegistration {
                detail: err.to_string(),
            }
        })?;
        let api = APIBuilder::new().with_media_engine(media_engine).build();
        let peer = Arc::new(
            api.new_peer_connection(RTCConfiguration::default())
                .await
                .map_err(|err| LiveWebrtcError::PeerCreation {
                    detail: err.to_string(),
                })?,
        );
        let construction_sequence = self.construction_sequence.fetch_add(1, Ordering::Relaxed) + 1;
        let mut construction = AnswerConstructionCustody::new(
            construction_sequence,
            channel_id.clone(),
            Arc::clone(&peer),
            Arc::clone(&self.construction_peers),
        );
        #[cfg(test)]
        self.construction_test_state.observe_peer(Arc::clone(&peer));

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
        if let Err(err) = peer.add_track(outgoing_audio_track).await {
            let error = LiveWebrtcError::PeerCreation {
                detail: err.to_string(),
            };
            return Err(construction.close_after(error).await);
        }
        let outgoing_audio_control = Arc::new(OutgoingAudioControl::default());
        let (outgoing_audio_tx, outgoing_audio_rx) =
            mpsc::channel::<OutgoingAudioPacket>(WEBRTC_AUDIO_QUEUE_CAPACITY);
        let peer_tasks = spawn_outgoing_audio_track_pump(
            channel_id.clone(),
            Arc::clone(&outgoing_audio),
            outgoing_audio_rx,
            Arc::clone(&outgoing_audio_control),
            #[cfg(test)]
            Arc::clone(&self.construction_test_state.active_audio_pumps),
        );
        construction.attach_peer_tasks(Arc::clone(&peer_tasks));

        install_data_channel_handler(DataChannelPumpContext {
            host: Arc::clone(&self.host),
            channel_id: channel_id.clone(),
            peer: Arc::downgrade(&peer),
            peer_registry: Arc::downgrade(&self.peers),
            close_feedback: Arc::clone(&self.close_feedback),
            status_feedback: Arc::clone(&self.status_feedback),
            outgoing_audio_tx,
            outgoing_audio_control: Arc::clone(&outgoing_audio_control),
            pending_answers: Arc::downgrade(&self.pending_answers),
            published_peer_lifecycle: Arc::downgrade(&self.published_peer_lifecycle),
            peer_tasks: Arc::clone(&peer_tasks),
            #[cfg(test)]
            construction_test_state: Arc::clone(&self.construction_test_state),
        });
        install_incoming_audio_handler(
            PeerDisconnectContext {
                host: Arc::clone(&self.host),
                channel_id: channel_id.clone(),
                peer: Arc::downgrade(&peer),
                peer_registry: Arc::downgrade(&self.peers),
                pending_answers: Arc::downgrade(&self.pending_answers),
                published_peer_lifecycle: Arc::downgrade(&self.published_peer_lifecycle),
                close_feedback: Arc::clone(&self.close_feedback),
                peer_tasks: Arc::clone(&peer_tasks),
            },
            Arc::clone(&outgoing_audio_control),
            #[cfg(test)]
            Arc::clone(&self.construction_test_state.active_incoming_audio_tasks),
        );

        #[cfg(test)]
        self.construction_test_state
            .pause_before_negotiation()
            .await;

        let offer = match RTCSessionDescription::offer(offer_sdp) {
            Ok(offer) => offer,
            Err(err) => {
                let error = LiveWebrtcError::SetRemoteDescription {
                    detail: err.to_string(),
                };
                return Err(construction.close_after(error).await);
            }
        };
        if let Err(err) = peer.set_remote_description(offer).await {
            let error = LiveWebrtcError::SetRemoteDescription {
                detail: err.to_string(),
            };
            return Err(construction.close_after(error).await);
        }
        let answer = match peer.create_answer(None).await {
            Ok(answer) => answer,
            Err(err) => {
                let error = LiveWebrtcError::CreateAnswer {
                    detail: err.to_string(),
                };
                return Err(construction.close_after(error).await);
            }
        };
        let mut gathering_complete = peer.gathering_complete_promise().await;
        if let Err(err) = peer.set_local_description(answer).await {
            let error = LiveWebrtcError::CreateAnswer {
                detail: err.to_string(),
            };
            return Err(construction.close_after(error).await);
        }
        let _ = gathering_complete.recv().await;
        let answer_sdp = match peer.local_description().await {
            Some(description) => description.sdp,
            None => {
                return Err(construction
                    .close_after(LiveWebrtcError::MissingLocalDescription)
                    .await);
            }
        };

        let answer_observation_sequence = self
            .answer_observation_sequence
            .fetch_add(1, Ordering::Relaxed)
            + 1;

        self.publish_answer_peer_with_cleanup_custody(
            channel_id,
            construction,
            peer,
            outgoing_audio_control,
            peer_tasks,
            answer_observation_sequence,
        )
        .await;

        Ok(LiveWebrtcAnswerAccepted {
            answer_sdp,
            answer_observation_sequence,
        })
    }

    /// Publish transport and rejection-cleanup custody as one cancellation-safe
    /// mechanical step. There is no await between the two map mutations.
    async fn publish_answer_peer_with_cleanup_custody(
        &self,
        channel_id: LiveChannelId,
        construction: AnswerConstructionCustody,
        peer: Arc<RTCPeerConnection>,
        outgoing_audio: Arc<OutgoingAudioControl>,
        peer_tasks: Arc<WebrtcPeerTaskControl>,
        answer_observation_sequence: u64,
    ) {
        // Acquire both maps before publishing either entry. Cancellation while
        // waiting for the second lock drops the first guard without exposing a
        // peer that has no rejection-cleanup owner.
        let mut peers = self.peers.lock().await;
        let mut pending_answers = self.pending_answers.lock().await;
        peers.insert(
            channel_id.clone(),
            LiveWebrtcPeer {
                peer: Arc::clone(&peer),
                outgoing_audio,
                peer_tasks: Arc::clone(&peer_tasks),
                answer_observation_sequence,
            },
        );
        pending_answers.insert(
            answer_observation_sequence,
            LiveWebrtcPendingAnswer {
                channel_id: channel_id.clone(),
                peer: Arc::clone(&peer),
                peer_tasks: Arc::clone(&peer_tasks),
            },
        );
        self.published_peer_lifecycle.insert(
            answer_observation_sequence,
            channel_id.clone(),
            &peer,
            &peer_tasks,
        );
        // Map publication and construction disarm occur in the same poll with
        // both destination locks held. Cancellation can observe either the
        // construction owner or both published owners, never neither.
        peer_tasks.publish(answer_observation_sequence);
        self.install_published_peer_cleanup(
            &channel_id,
            answer_observation_sequence,
            &peer,
            &peer_tasks,
        );
        construction.disarm_after_publication();
        drop(pending_answers);
        drop(peers);
        install_published_peer_disconnect_handler(PeerDisconnectContext {
            host: Arc::clone(&self.host),
            channel_id,
            peer: Arc::downgrade(&peer),
            peer_registry: Arc::downgrade(&self.peers),
            pending_answers: Arc::downgrade(&self.pending_answers),
            published_peer_lifecycle: Arc::downgrade(&self.published_peer_lifecycle),
            close_feedback: Arc::clone(&self.close_feedback),
            peer_tasks,
        });
    }

    /// Wait until a cancelled answer future's detached construction cleanup
    /// has either retired the exact physical peer or retained truthful custody
    /// because the one-shot close did not reach typed `Closed`.
    pub async fn wait_for_answer_construction_cleanup(&self, channel_id: &LiveChannelId) {
        self.construction_peers
            .wait_for_channel_absent(channel_id)
            .await;
    }

    pub async fn close_peer(&self, channel_id: &LiveChannelId) {
        if let Err(error) = self.close_peer_checked(channel_id).await {
            tracing::warn!(
                channel = %channel_id,
                error = %error,
                "best-effort WebRTC peer close failed"
            );
        }
    }

    /// Release the private cleanup custody retained while the generated
    /// answer result is being projected and serialized by the RPC surface.
    /// The public accepted-answer value stays source-compatible; the monotonic
    /// sequence identifies its exact private peer obligation.
    pub async fn release_answer_cleanup_obligation(
        &self,
        channel_id: &LiveChannelId,
        answer_observation_sequence: u64,
    ) {
        let mut pending_answers = self.pending_answers.lock().await;
        if pending_answers
            .get(&answer_observation_sequence)
            .is_some_and(|pending| pending.channel_id == *channel_id)
        {
            pending_answers.remove(&answer_observation_sequence);
        }
    }

    /// Close the exact peer materialized for a rejected answer result.
    ///
    /// rust-webrtc close is one-shot: it marks the peer closed before
    /// attempting every transport shutdown, and a later call returns `Ok`
    /// without retrying failed sub-closes. We therefore call it exactly once,
    /// retire custody only after the typed peer state is `Closed`, and preserve
    /// any shutdown error for the caller. A registry replacement is never
    /// closed or removed on behalf of the rejected sequence.
    pub async fn close_rejected_answer_peer(
        &self,
        channel_id: &LiveChannelId,
        answer_observation_sequence: u64,
    ) -> Result<(), LiveWebrtcError> {
        self.close_rejected_answer_peer_with(
            channel_id,
            answer_observation_sequence,
            |peer| async move {
                peer.close()
                    .await
                    .map_err(|error| LiveWebrtcError::PeerClose {
                        detail: error.to_string(),
                    })
            },
        )
        .await
    }

    async fn close_rejected_answer_peer_with<C, F>(
        &self,
        channel_id: &LiveChannelId,
        answer_observation_sequence: u64,
        close: C,
    ) -> Result<(), LiveWebrtcError>
    where
        C: FnOnce(Arc<RTCPeerConnection>) -> F + Send + 'static,
        F: std::future::Future<Output = Result<(), LiveWebrtcError>> + Send + 'static,
    {
        let exact_peer = {
            let pending_answers = self.pending_answers.lock().await;
            pending_answers
                .get(&answer_observation_sequence)
                .filter(|pending| pending.channel_id == *channel_id)
                .map(|pending| (Arc::clone(&pending.peer), Arc::clone(&pending.peer_tasks)))
        };
        let Some((exact_peer, peer_tasks)) = exact_peer else {
            return Ok(());
        };

        self.install_published_peer_cleanup(
            channel_id,
            answer_observation_sequence,
            &exact_peer,
            &peer_tasks,
        );
        let outcome_rx = peer_tasks
            .start_published_cleanup_with(Box::new(move |peer| Box::pin(close(peer))))
            .expect("published cleanup was installed before start");
        PublishedPeerCleanupCoordinator::wait_for_outcome(outcome_rx)
            .await
            .into_result()
            .map(|_| ())
    }

    /// Install or recover the one cleanup coordinator for an exact published
    /// peer. Tests that build registry fixtures by hand route through this
    /// same seam; production installs it atomically during publication.
    fn install_published_peer_cleanup(
        &self,
        channel_id: &LiveChannelId,
        answer_observation_sequence: u64,
        peer: &Arc<RTCPeerConnection>,
        peer_tasks: &Arc<WebrtcPeerTaskControl>,
    ) -> Arc<PublishedPeerCleanupCoordinator> {
        peer_tasks.install_published_cleanup(PublishedPeerCleanupContext {
            channel_id: channel_id.clone(),
            answer_observation_sequence,
            peer: Arc::downgrade(peer),
            peer_registry: Arc::downgrade(&self.peers),
            pending_answers: Arc::downgrade(&self.pending_answers),
            published_peer_lifecycle: Arc::downgrade(&self.published_peer_lifecycle),
            runtime: tokio::runtime::Handle::current(),
        })
    }

    /// Invoke the transport's one-shot close and retire only an exact peer that
    /// reached its typed `Closed` state. Any shutdown error remains visible to
    /// lifecycle authority before machine terminality.
    pub async fn close_peer_checked(
        &self,
        channel_id: &LiveChannelId,
    ) -> Result<(), LiveWebrtcError> {
        self.close_peer_checked_with_operation(channel_id, None)
            .await
    }

    async fn close_peer_checked_with_operation(
        &self,
        channel_id: &LiveChannelId,
        mut close: Option<PublishedPeerCloseOperation>,
    ) -> Result<(), LiveWebrtcError> {
        loop {
            let peer = self.peers.lock().await.get(channel_id).map(|entry| {
                (
                    Arc::clone(&entry.peer),
                    entry.answer_observation_sequence,
                    Arc::clone(&entry.peer_tasks),
                )
            });
            let Some((peer, answer_observation_sequence, peer_tasks)) = peer else {
                return Ok(());
            };
            self.install_published_peer_cleanup(
                channel_id,
                answer_observation_sequence,
                &peer,
                &peer_tasks,
            );
            let outcome_rx = match close.take() {
                Some(close) => peer_tasks.start_published_cleanup_with(close),
                None => peer_tasks.start_published_cleanup(),
            }
            .expect("published cleanup was installed before start");
            let removed_current = PublishedPeerCleanupCoordinator::wait_for_outcome(outcome_rx)
                .await
                .into_result()?;
            if removed_current {
                return Ok(());
            }
            // A concurrent replacement is still physical custody for the
            // same channel. Close that exact peer too; never report absence
            // from closing only a stale Arc.
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

    #[cfg(test)]
    async fn pending_answer_count(&self) -> usize {
        self.pending_answers.lock().await.len()
    }

    #[cfg(test)]
    fn construction_peer_count(&self) -> usize {
        self.construction_peers.len()
    }

    #[cfg(test)]
    fn published_peer_lifecycle_count(&self) -> usize {
        self.published_peer_lifecycle.len()
    }
}

impl Drop for LiveWebrtcState {
    fn drop(&mut self) {
        // `Drop` cannot await. Task shutdown is synchronous; physical close is
        // transferred to the Tokio runtime captured when each answer was
        // published. Callback/task contexts hold only weak registry/owner
        // handles, so this teardown cannot be kept alive by the peer it owns.
        self.published_peer_lifecycle.shutdown_all();
    }
}

#[derive(Clone)]
struct PeerDisconnectContext {
    host: Arc<LiveAdapterHost>,
    channel_id: LiveChannelId,
    peer: Weak<RTCPeerConnection>,
    peer_registry: WeakLiveWebrtcPeerRegistry,
    pending_answers: WeakLiveWebrtcPendingAnswerRegistry,
    published_peer_lifecycle: Weak<PublishedPeerLifecycleOwner>,
    close_feedback: Arc<dyn LiveChannelCloseFeedback>,
    peer_tasks: Arc<WebrtcPeerTaskControl>,
}

#[derive(Clone)]
struct DataChannelPumpContext {
    host: Arc<LiveAdapterHost>,
    channel_id: LiveChannelId,
    peer: Weak<RTCPeerConnection>,
    peer_registry: WeakLiveWebrtcPeerRegistry,
    close_feedback: Arc<dyn LiveChannelCloseFeedback>,
    status_feedback: Arc<dyn LiveChannelStatusFeedback>,
    outgoing_audio_tx: mpsc::Sender<OutgoingAudioPacket>,
    outgoing_audio_control: Arc<OutgoingAudioControl>,
    pending_answers: WeakLiveWebrtcPendingAnswerRegistry,
    published_peer_lifecycle: Weak<PublishedPeerLifecycleOwner>,
    peer_tasks: Arc<WebrtcPeerTaskControl>,
    #[cfg(test)]
    construction_test_state: Arc<AnswerConstructionTestState>,
}

fn peer_state_requires_disconnect_cleanup(state: RTCPeerConnectionState) -> bool {
    matches!(
        state,
        RTCPeerConnectionState::Disconnected
            | RTCPeerConnectionState::Failed
            | RTCPeerConnectionState::Closed
    )
}

fn install_published_peer_disconnect_handler(context: PeerDisconnectContext) {
    let Some(peer) = context.peer.upgrade() else {
        return;
    };
    let callback_context = context.clone();
    peer.on_peer_connection_state_change(Box::new(move |state| {
        if peer_state_requires_disconnect_cleanup(state) {
            spawn_peer_disconnect_cleanup(callback_context.clone());
        }
        Box::pin(async {})
    }));

    // The peer can become terminal between map publication and callback
    // installation. Inspect once after installing so that edge is not lost.
    if peer_state_requires_disconnect_cleanup(peer.connection_state()) {
        spawn_peer_disconnect_cleanup(context);
    }
}

fn spawn_peer_disconnect_cleanup(context: PeerDisconnectContext) {
    // A zero sequence is pre-publication construction custody. Its Drop owner
    // performs physical cleanup without generating a semantic channel close.
    if context.peer_tasks.answer_observation_sequence().is_none()
        || !context.peer_tasks.claim_terminal_cleanup()
    {
        return;
    }
    context.peer_tasks.shutdown();
    tokio::spawn(async move {
        cleanup_peer_after_disconnect(context).await;
    });
}

async fn cleanup_peer_after_disconnect(context: PeerDisconnectContext) {
    let Some(answer_observation_sequence) = context.peer_tasks.answer_observation_sequence() else {
        return;
    };
    context.peer_tasks.shutdown();
    let Some(peer) = context.peer.upgrade() else {
        return;
    };

    let is_current = if let Some(peer_registry) = context.peer_registry.upgrade() {
        peer_registry
            .lock()
            .await
            .get(&context.channel_id)
            .is_some_and(|current| {
                current.answer_observation_sequence == answer_observation_sequence
                    && Arc::ptr_eq(&current.peer, &peer)
            })
    } else {
        false
    };
    // A callback from a stale replaced peer owns only that physical peer. It
    // must not terminalize the newer semantic channel binding.
    if is_current {
        let _ = close_channel_with_generated_feedback(
            context.host.as_ref(),
            context.close_feedback.as_ref(),
            &context.channel_id,
        )
        .await;
    }

    install_cleanup_from_disconnect_context(&context, &peer, answer_observation_sequence);
    if let Some(outcome_rx) = context.peer_tasks.start_published_cleanup() {
        let _ = PublishedPeerCleanupCoordinator::wait_for_outcome(outcome_rx).await;
    }
}

fn install_cleanup_from_disconnect_context(
    context: &PeerDisconnectContext,
    peer: &Arc<RTCPeerConnection>,
    answer_observation_sequence: u64,
) -> Arc<PublishedPeerCleanupCoordinator> {
    context
        .peer_tasks
        .install_published_cleanup(PublishedPeerCleanupContext {
            channel_id: context.channel_id.clone(),
            answer_observation_sequence,
            peer: Arc::downgrade(peer),
            peer_registry: context.peer_registry.clone(),
            pending_answers: context.pending_answers.clone(),
            published_peer_lifecycle: context.published_peer_lifecycle.clone(),
            runtime: tokio::runtime::Handle::current(),
        })
}

fn start_physical_cleanup(context: &PeerDisconnectContext) {
    context.peer_tasks.shutdown();
    let Some(answer_observation_sequence) = context.peer_tasks.answer_observation_sequence() else {
        return;
    };
    let Some(peer) = context.peer.upgrade() else {
        return;
    };
    install_cleanup_from_disconnect_context(context, &peer, answer_observation_sequence);
    let _ = context.peer_tasks.start_published_cleanup();
}

fn install_data_channel_handler(context: DataChannelPumpContext) {
    let Some(peer_for_handler) = context.peer.upgrade() else {
        return;
    };
    let peer_for_callback = context.peer.clone();
    peer_for_handler.on_data_channel(Box::new(move |channel: Arc<RTCDataChannel>| {
        #[cfg(test)]
        context
            .construction_test_state
            .observe_data_channel(&channel);
        let host_for_messages = Arc::clone(&context.host);
        let channel_for_messages = context.channel_id.clone();
        let channel_for_messages_handle = Arc::downgrade(&channel);
        let channel_for_open = Arc::downgrade(&channel);
        let observation_context = DataChannelPumpContext {
            host: Arc::clone(&context.host),
            channel_id: context.channel_id.clone(),
            peer: peer_for_callback.clone(),
            peer_registry: context.peer_registry.clone(),
            close_feedback: Arc::clone(&context.close_feedback),
            status_feedback: Arc::clone(&context.status_feedback),
            outgoing_audio_tx: context.outgoing_audio_tx.clone(),
            outgoing_audio_control: Arc::clone(&context.outgoing_audio_control),
            pending_answers: context.pending_answers.clone(),
            published_peer_lifecycle: context.published_peer_lifecycle.clone(),
            peer_tasks: Arc::clone(&context.peer_tasks),
            #[cfg(test)]
            construction_test_state: Arc::clone(&context.construction_test_state),
        };
        let disconnect_context = PeerDisconnectContext {
            host: Arc::clone(&context.host),
            channel_id: context.channel_id.clone(),
            peer: peer_for_callback.clone(),
            peer_registry: context.peer_registry.clone(),
            pending_answers: context.pending_answers.clone(),
            published_peer_lifecycle: context.published_peer_lifecycle.clone(),
            close_feedback: Arc::clone(&context.close_feedback),
            peer_tasks: Arc::clone(&context.peer_tasks),
        };
        let message_cleanup_context = disconnect_context.clone();

        // D329: capture the close machinery + peer handles into the message
        // handler so malformed JSON/base64 terminalizes the channel identically
        // to the WS path. Bounded image size/MIME failures and the explicitly
        // unsupported data-channel image route are scoped command rejections,
        // leaving the peer usable for RPC image ingress plus RTP audio.
        Box::pin(async move {
            channel.on_message(Box::new(move |message: DataChannelMessage| {
                let host = Arc::clone(&host_for_messages);
                let channel_id = channel_for_messages.clone();
                let cleanup_context = message_cleanup_context.clone();
                let data_channel = channel_for_messages_handle.clone();
                Box::pin(async move {
                    let Some(data_channel) = data_channel.upgrade() else {
                        return;
                    };
                    if message.is_string {
                        // D243: the parse boundary is the generated
                        // `LiveInputChunkWire` + the shared
                        // `live_input_chunk_from_wire` conversion owner
                        // (`crate::wire_input`) that the RPC `live/send_input`
                        // path also routes through, so the WebRTC data channel
                        // runs identical wire validation. A payload that fails
                        // wire validation is rejected here exactly as on the
                        // RPC path. Malformed envelopes/base64 are terminal;
                        // resource-policy failures remain typed and scoped.
                        let chunk = match serde_json::from_slice::<LiveInputChunkWire>(
                            &message.data,
                        ) {
                            Ok(LiveInputChunkWire::Image { .. }) => {
                                // rust-webrtc's callback API reads into a fixed
                                // 65,535-byte buffer. This typed rejection is
                                // therefore guaranteed only for an envelope
                                // delivered whole and decoded here; larger
                                // messages may fail below this callback. Images
                                // use the JSON-RPC `live/send_input` control
                                // plane, while the data channel remains the
                                // redacted receipt lane and RTP ordering barrier.
                                let observation = webrtc_data_channel_image_rejection();
                                let _ = forward_observation_json(&data_channel, &observation).await;
                                return;
                            }
                            Ok(wire) => match live_input_chunk_from_wire(wire) {
                                Ok(chunk) => Some(chunk),
                                Err(err) => {
                                    if let Some(code) = live_input_chunk_decode_rejection(&err) {
                                        let observation =
                                            LiveAdapterObservation::CommandRejected {
                                                code,
                                                message: err.to_string(),
                                            };
                                        let _ = forward_observation_json(
                                            &data_channel,
                                            &observation,
                                        )
                                        .await;
                                        return;
                                    }
                                    tracing::warn!(
                                        channel = %channel_id,
                                        error = %err,
                                        "WebRTC data-channel input chunk failed wire validation; closing"
                                    );
                                    None
                                }
                            },
                            Err(err) => {
                                tracing::warn!(
                                    channel = %channel_id,
                                    error = %err,
                                    "invalid WebRTC data-channel input frame; closing"
                                );
                                None
                            }
                        };
                        match chunk {
                            Some(chunk) => {
                                if let Err(err) = host.send_input(&channel_id, chunk).await {
                                    if let Some(observation) =
                                        crate::transport::scoped_command_rejection_from_host_error(
                                            &err,
                                        )
                                    {
                                        let _ = forward_observation_json(
                                            &data_channel,
                                            &observation,
                                        )
                                        .await;
                                        return;
                                    }
                                    tracing::warn!(
                                        channel = %channel_id,
                                        error = %err,
                                        "WebRTC data-channel send_input failed; closing"
                                    );
                                    send_webrtc_input_error_frame(&data_channel, &err).await;
                                    reject_failed_input_handoff(
                                        &cleanup_context,
                                        &data_channel,
                                    );
                                }
                            }
                            None => {
                                reject_invalid_live_frame(&cleanup_context, &data_channel);
                            }
                        }
                    }
                })
            }));

            channel.on_close(Box::new(move || {
                spawn_peer_disconnect_cleanup(disconnect_context.clone());
                Box::pin(async {})
            }));

            channel.on_open(Box::new(move || {
                let data_channel = channel_for_open.clone();
                let context = observation_context.clone();
                Box::pin(async move {
                    let Some(data_channel) = data_channel.upgrade() else {
                        return;
                    };
                    let shutdown_rx = context.peer_tasks.subscribe();
                    #[cfg(test)]
                    context
                        .construction_test_state
                        .active_observation_pumps
                        .fetch_add(1, Ordering::SeqCst);
                    tokio::spawn(async move {
                        #[cfg(test)]
                        let _active_guard = ActiveObservationPumpGuard(Arc::clone(
                            &context
                                .construction_test_state
                                .active_observation_pumps,
                        ));
                        pump_observations_to_data_channel(
                            context,
                            data_channel,
                            shutdown_rx,
                        )
                        .await;
                    });
                })
            }));
        })
    }));
}

fn install_incoming_audio_handler(
    cleanup_context: PeerDisconnectContext,
    outgoing_audio_control: Arc<OutgoingAudioControl>,
    #[cfg(test)] active_incoming_audio_tasks: Arc<AtomicUsize>,
) {
    let Some(peer_for_handler) = cleanup_context.peer.upgrade() else {
        return;
    };
    peer_for_handler.on_track(Box::new(move |track: Arc<TrackRemote>, _, _| {
        let cleanup_context = cleanup_context.clone();
        let outgoing_audio_control = Arc::clone(&outgoing_audio_control);
        let mut shutdown_rx = cleanup_context.peer_tasks.subscribe();
        #[cfg(test)]
        active_incoming_audio_tasks.fetch_add(1, Ordering::SeqCst);
        #[cfg(test)]
        let active_incoming_audio_tasks = Arc::clone(&active_incoming_audio_tasks);
        tokio::spawn(async move {
            #[cfg(test)]
            let _active_guard = ActiveIncomingAudioTaskGuard(active_incoming_audio_tasks);
            let mut bridge = match WebrtcAudioBridge::new() {
                Ok(bridge) => bridge,
                Err(err) => {
                    tracing::warn!(channel = %cleanup_context.channel_id, error = %err, "failed to initialize WebRTC audio bridge");
                    return;
                }
            };
            loop {
                let packet = match tokio::select! {
                    () = wait_for_peer_task_shutdown(&mut shutdown_rx) => return,
                    packet = track.read_rtp() => packet,
                } {
                    Ok((packet, _)) => packet,
                    Err(err) => {
                        tracing::debug!(channel = %cleanup_context.channel_id, error = %err, "WebRTC RTP track ended");
                        break;
                    }
                };
                let pcm_24k = match bridge.decode_browser_opus_payload(&packet.payload) {
                    Ok(pcm) => pcm,
                    Err(err) => {
                        tracing::warn!(channel = %cleanup_context.channel_id, error = %err, "failed to decode browser Opus payload");
                        continue;
                    }
                };
                if pcm_24k.is_empty() {
                    continue;
                }
                if outgoing_audio_control.queued_packets() > 0 && pcm24k_le_has_speech(&pcm_24k) {
                    // D223: lower the transport-local barge-in into the
                    // canonical interrupt seam (the same `signal_turn_interrupt`
                    // path adapter-observed barge-ins use) before discarding
                    // queued output audio. If the typed seam rejects the
                    // signal, there is no compensation path for already-lost
                    // output, so fail closed through generated close feedback.
                    let generation = match signal_barge_in_then_discard_output_audio(
                        cleanup_context.host.as_ref(),
                        &cleanup_context.channel_id,
                        outgoing_audio_control.as_ref(),
                    )
                    .await
                    {
                        Ok(generation) => generation,
                        Err(err) => {
                            tracing::warn!(
                                channel = %cleanup_context.channel_id,
                                error = %err,
                                "failed to lower WebRTC barge-in into the interrupt seam; closing before discarding output audio"
                            );
                            reject_failed_audio_input_handoff(&cleanup_context);
                            break;
                        }
                    };
                    tracing::debug!(
                        channel = %cleanup_context.channel_id,
                        generation,
                        "discarding queued WebRTC output audio after local speech barge-in"
                    );
                }
                let chunk = LiveInputChunk::Audio {
                    data: pcm_24k,
                    sample_rate_hz: PROVIDER_PCM_SAMPLE_RATE,
                    channels: MONO_CHANNELS,
                };
                if let Err(err) = cleanup_context
                    .host
                    .send_input(&cleanup_context.channel_id, chunk)
                    .await
                {
                    if crate::transport::scoped_command_rejection_from_host_error(&err).is_some() {
                        // RTP has no request/response lane on which to return a
                        // scoped rejection. Drop this packet and keep the peer
                        // alive: input backpressure/config rejection describes
                        // the packet, not a terminal channel failure.
                        tracing::debug!(
                            channel = %cleanup_context.channel_id,
                            error = %err,
                            "dropping WebRTC RTP input after scoped adapter rejection"
                        );
                        continue;
                    }
                    tracing::warn!(
                        channel = %cleanup_context.channel_id,
                        error = %err,
                        "WebRTC audio send_input failed; closing"
                    );
                    reject_failed_audio_input_handoff(&cleanup_context);
                    break;
                }
            }
        });
        Box::pin(async {})
    }));
}

async fn signal_barge_in_then_discard_output_audio(
    host: &LiveAdapterHost,
    channel_id: &LiveChannelId,
    outgoing_audio_control: &OutgoingAudioControl,
) -> Result<u64, LiveAdapterHostError> {
    host.signal_transport_barge_in(channel_id).await?;
    Ok(outgoing_audio_control.discard_queued())
}

async fn pump_observations_to_data_channel(
    context: DataChannelPumpContext,
    data_channel: Arc<RTCDataChannel>,
    mut shutdown_rx: watch::Receiver<bool>,
) {
    let DataChannelPumpContext {
        host,
        channel_id,
        peer,
        peer_registry,
        pending_answers,
        published_peer_lifecycle,
        close_feedback,
        status_feedback,
        outgoing_audio_tx,
        outgoing_audio_control,
        peer_tasks,
        #[cfg(test)]
            construction_test_state: _,
    } = context;
    let cleanup_context = PeerDisconnectContext {
        host: Arc::clone(&host),
        channel_id: channel_id.clone(),
        peer: peer.clone(),
        peer_registry: peer_registry.clone(),
        pending_answers: pending_answers.clone(),
        published_peer_lifecycle: published_peer_lifecycle.clone(),
        close_feedback: Arc::clone(&close_feedback),
        peer_tasks: Arc::clone(&peer_tasks),
    };
    let mut bridge = match WebrtcAudioBridge::new() {
        Ok(bridge) => bridge,
        Err(err) => {
            tracing::warn!(channel = %channel_id, error = %err, "failed to initialize WebRTC output audio bridge");
            spawn_peer_disconnect_cleanup(cleanup_context);
            return;
        }
    };
    let mut close_feedback_recorded = false;
    loop {
        let observation = match tokio::select! {
            () = wait_for_peer_task_shutdown(&mut shutdown_rx) => return,
            observation = host.next_observation_raw(&channel_id) => observation,
        } {
            Ok(Some(obs)) => obs,
            Ok(None) => break,
            Err(err) => {
                tracing::warn!(
                    channel = %channel_id,
                    error = %err,
                    "WebRTC observation pump failed"
                );
                break;
            }
        };

        let close_observation = observation_requires_generated_close(&observation);
        let publish_observation = should_publish_observation(&observation);
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

        // Forward only after `apply_observation` has completed. In
        // particular, an image's internal byte-bearing transcript event is
        // filtered and its following `user_content_committed` receipt becomes
        // visible only after canonical persistence. WebRTC clients must wait
        // for that receipt before sending RTP audio that relies on the image.
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
                    let mut chunk_cumulative_dropped = None;
                    for packet in packets {
                        match outgoing_audio_tx.try_send(OutgoingAudioPacket {
                            generation,
                            data: packet,
                        }) {
                            Ok(()) => outgoing_audio_control.note_queued(),
                            Err(err) => {
                                // D223: surface the drop as a typed
                                // delivery-degraded fact (cumulative drop
                                // count), not just a log line, so the
                                // transport/session knows output audio was not
                                // fully delivered.
                                let dropped = outgoing_audio_control.note_dropped();
                                chunk_cumulative_dropped = Some(dropped);
                                tracing::warn!(
                                    channel = %channel_id,
                                    error = %err,
                                    dropped_output_audio_packets = dropped,
                                    "dropping WebRTC output audio packet because RTP pacing queue is full or closed"
                                );
                            }
                        }
                    }
                    // K16: lower the delivery degradation into the canonical
                    // host signal seam (the same `LiveProjectionSink` path
                    // `signal_transport_barge_in` uses) so the dropped output
                    // audio is a typed fact the session observes — not a
                    // transport-local counter with no live reader. One signal
                    // per degraded chunk, carrying the cumulative drop count.
                    if let Some(dropped) = chunk_cumulative_dropped
                        && !signal_output_audio_degraded_or_close(
                            host.as_ref(),
                            close_feedback.as_ref(),
                            &channel_id,
                            dropped,
                        )
                        .await
                    {
                        start_physical_cleanup(&cleanup_context);
                        return;
                    }
                }
                Err(err) => {
                    tracing::warn!(channel = %channel_id, error = %err, "failed to encode provider PCM for WebRTC");
                }
            }
        }

        match outcome {
            ObservationOutcome::UserContentCommitted { observation } => {
                if let Err(err) = forward_observation_json(&data_channel, &observation).await {
                    tracing::warn!(channel = %channel_id, error = %err, "failed to send durable WebRTC user-content receipt");
                    break;
                }
            }
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

    if close_feedback_recorded {
        start_physical_cleanup(&cleanup_context);
    } else {
        spawn_peer_disconnect_cleanup(cleanup_context);
    }
}

async fn wait_for_peer_task_shutdown(shutdown_rx: &mut watch::Receiver<bool>) {
    if *shutdown_rx.borrow_and_update() {
        return;
    }
    loop {
        if shutdown_rx.changed().await.is_err() || *shutdown_rx.borrow_and_update() {
            return;
        }
    }
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
    if let Err(err) = host.prepare_channel_physical_close(&observation).await {
        tracing::warn!(
            channel = %channel_id,
            error = %err,
            "live WebRTC physical adapter close failed before generated feedback"
        );
        return false;
    }
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

async fn signal_output_audio_degraded_or_close(
    host: &LiveAdapterHost,
    close_feedback: &dyn LiveChannelCloseFeedback,
    channel_id: &LiveChannelId,
    dropped: u64,
) -> bool {
    if let Err(err) = host.signal_output_audio_degraded(channel_id, dropped).await {
        tracing::warn!(
            channel = %channel_id,
            error = %err,
            dropped_output_audio_packets = dropped,
            "failed to lower WebRTC output-audio degradation into the host signal seam; closing channel"
        );
        let _ = close_channel_with_generated_feedback(host, close_feedback, channel_id).await;
        return false;
    }
    true
}

async fn retire_closed_peer_custody_from_registries(
    peer_registry: &WeakLiveWebrtcPeerRegistry,
    pending_answers: &WeakLiveWebrtcPendingAnswerRegistry,
    published_peer_lifecycle: &Weak<PublishedPeerLifecycleOwner>,
    channel_id: &LiveChannelId,
    answer_observation_sequence: u64,
    exact_peer: &RTCPeerConnection,
) -> bool {
    let removed_current = if let Some(peer_registry) = peer_registry.upgrade() {
        let mut peers = peer_registry.lock().await;
        let removed_current = peers.get(channel_id).is_some_and(|current| {
            current.answer_observation_sequence == answer_observation_sequence
                && std::ptr::eq(current.peer.as_ref(), exact_peer)
        });
        if removed_current {
            peers.remove(channel_id);
        }
        removed_current
    } else {
        false
    };

    if let Some(pending_answers) = pending_answers.upgrade() {
        let mut pending_answers = pending_answers.lock().await;
        if pending_answers
            .get(&answer_observation_sequence)
            .is_some_and(|pending| {
                pending.channel_id == *channel_id && std::ptr::eq(pending.peer.as_ref(), exact_peer)
            })
        {
            pending_answers.remove(&answer_observation_sequence);
        }
    }
    if let Some(published_peer_lifecycle) = published_peer_lifecycle.upgrade() {
        published_peer_lifecycle.remove_exact(answer_observation_sequence, exact_peer);
    }
    removed_current
}

fn webrtc_host_error_frame_json(err: &LiveAdapterHostError) -> String {
    serde_json::to_string(&WebrtcErrorFrame {
        error: err.to_string(),
        reason: err.reason_code(),
    })
    .unwrap_or_default()
}

async fn send_webrtc_input_error_frame(data_channel: &RTCDataChannel, err: &LiveAdapterHostError) {
    let frame = webrtc_host_error_frame_json(err);
    if frame.is_empty() {
        return;
    }
    if let Err(send_err) = data_channel.send_text(frame).await {
        tracing::warn!(
            error = %send_err,
            "failed to send WebRTC input error frame"
        );
    }
}

/// D329: terminalize a live channel after an invalid input frame, uniformly
/// across transports.
///
/// The WS path (`transport.rs`) closes the channel through generated close
/// feedback when an input frame fails to parse; the WebRTC data channel
/// previously only `tracing::warn`-ed and kept the peer alive, so the same
/// malformed condition terminalized differently by transport. This helper
/// routes the WebRTC reject through the same generated close authority and then
/// tears down the peer/data-channel so the outcome matches WS: invalid frame =>
/// generated close.
fn reject_invalid_live_frame(
    cleanup_context: &PeerDisconnectContext,
    data_channel: &RTCDataChannel,
) {
    reject_failed_data_channel(cleanup_context, data_channel);
}

/// Terminalize a live channel after a parsed WebRTC input cannot be handed to
/// the live host. This mirrors the malformed-frame path with one extra client
/// error frame when the data channel is available.
fn reject_failed_input_handoff(
    cleanup_context: &PeerDisconnectContext,
    data_channel: &RTCDataChannel,
) {
    reject_failed_data_channel(cleanup_context, data_channel);
}

fn reject_failed_data_channel(
    cleanup_context: &PeerDisconnectContext,
    _data_channel: &RTCDataChannel,
) {
    spawn_peer_disconnect_cleanup(cleanup_context.clone());
}

fn reject_failed_audio_input_handoff(cleanup_context: &PeerDisconnectContext) {
    spawn_peer_disconnect_cleanup(cleanup_context.clone());
}

fn spawn_outgoing_audio_track_pump(
    channel_id: LiveChannelId,
    outgoing_audio: Arc<TrackLocalStaticSample>,
    mut audio_rx: mpsc::Receiver<OutgoingAudioPacket>,
    outgoing_audio_control: Arc<OutgoingAudioControl>,
    #[cfg(test)] active_audio_pumps: Arc<AtomicUsize>,
) -> Arc<WebrtcPeerTaskControl> {
    let control = Arc::new(WebrtcPeerTaskControl::new());
    let mut shutdown_rx = control.subscribe();
    #[cfg(test)]
    active_audio_pumps.fetch_add(1, Ordering::SeqCst);
    tokio::spawn(async move {
        #[cfg(test)]
        let _active_guard = ActiveAudioPumpGuard(active_audio_pumps);
        loop {
            let packet = tokio::select! {
                () = wait_for_peer_task_shutdown(&mut shutdown_rx) => break,
                packet = audio_rx.recv() => match packet {
                    Some(packet) => packet,
                    None => break,
                },
            };
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
            tokio::select! {
                () = wait_for_peer_task_shutdown(&mut shutdown_rx) => break,
                _ = tokio::time::sleep(WEBRTC_AUDIO_PACKET_DURATION) => {}
            }
        }
    });
    control
}

#[cfg(test)]
struct ActiveAudioPumpGuard(Arc<AtomicUsize>);

#[cfg(test)]
impl Drop for ActiveAudioPumpGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
struct ActiveObservationPumpGuard(Arc<AtomicUsize>);

#[cfg(test)]
impl Drop for ActiveObservationPumpGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[cfg(test)]
struct ActiveIncomingAudioTaskGuard(Arc<AtomicUsize>);

#[cfg(test)]
impl Drop for ActiveIncomingAudioTaskGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
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
        .map_err(|err| LiveWebrtcError::DataChannelSend {
            detail: err.to_string(),
        })
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
            opus_decoder: Decoder::new(BROWSER_OPUS_SAMPLE_RATE, Channels::Mono).map_err(
                |err| LiveWebrtcError::Audio {
                    detail: err.to_string(),
                },
            )?,
            opus_encoder: Encoder::new(
                BROWSER_OPUS_SAMPLE_RATE,
                Channels::Mono,
                Application::Audio,
            )
            .map_err(|err| LiveWebrtcError::Audio {
                detail: err.to_string(),
            })?,
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
            .map_err(|err| LiveWebrtcError::Audio {
                detail: err.to_string(),
            })?;
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
                .map_err(|err| LiveWebrtcError::Audio {
                    detail: err.to_string(),
                })?;
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
    let input_adapter =
        InterleavedSlice::new(&input_f64, 1, frames).map_err(|err| LiveWebrtcError::Audio {
            detail: err.to_string(),
        })?;
    let output_capacity = ((frames * output_rate).div_ceil(input_rate)).saturating_add(1024);
    let mut output = vec![0.0_f64; output_capacity];
    let mut output_adapter =
        InterleavedSlice::new_mut(&mut output, 1, output_capacity).map_err(|err| {
            LiveWebrtcError::Audio {
                detail: err.to_string(),
            }
        })?;
    let mut resampler = Fft::<f64>::new(input_rate, output_rate, frames, 1, 1, FixedSync::Input)
        .map_err(|err| LiveWebrtcError::Audio {
            detail: err.to_string(),
        })?;
    let (_input_frames, output_frames) = resampler
        .process_all_into_buffer(&input_adapter, &mut output_adapter, frames, None)
        .map_err(|err| LiveWebrtcError::Audio {
            detail: err.to_string(),
        })?;
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
        return Err(LiveWebrtcError::Audio {
            detail: "PCM payload length must be an even number of bytes".to_string(),
        });
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
    use crate::host::{
        LiveProjectionError, LiveProjectionSink, LiveTranscriptIdentity, NoOpProjectionSink,
    };
    use async_trait::async_trait;
    use meerkat_core::live_adapter::{
        LiveAdapter, LiveAdapterCommand, LiveAdapterError, LiveAdapterErrorCode, LiveAdapterStatus,
    };
    use meerkat_core::types::{SessionId, StopReason, Usage};
    use tokio::sync::mpsc;

    #[test]
    fn webrtc_filters_raw_adapter_receipt_until_host_commit_outcome() {
        use meerkat_core::types::{ContentBlock, ImageData};

        let internal = LiveAdapterObservation::RealtimeTranscript {
            event: meerkat_core::RealtimeTranscriptEvent::UserContentFinal {
                idempotency_key: "image-request-1".into(),
                item_id: "item_image".into(),
                previous_item_id: None,
                content_index: 0,
                content: vec![ContentBlock::Image {
                    media_type: "image/jpeg".into(),
                    data: ImageData::Inline {
                        data: "private-image-data".into(),
                    },
                }],
            },
        };
        let receipt = LiveAdapterObservation::UserContentCommitted {
            idempotency_key: "image-request-1".into(),
            item_id: "item_image".into(),
            previous_item_id: None,
            content_index: 0,
            media_type: "image/jpeg".into(),
        };

        assert!(!should_publish_observation(&internal));
        assert!(!should_publish_observation(&receipt));

        assert!(matches!(
            webrtc_data_channel_image_rejection(),
            LiveAdapterObservation::CommandRejected {
                code: LiveAdapterErrorCode::ConfigRejected {
                    reason: LiveConfigRejectionReason::ImageInputTransportUnsupported {
                        transport,
                    },
                },
                message,
            } if transport == "webrtc_data_channel_use_live_send_input_rpc"
                && message.contains("JSON-RPC live/send_input")
                && message.contains("user_content_committed")
        ));
    }

    #[test]
    fn webrtc_scoped_backpressure_is_nonterminal_for_data_and_rtp_ingress() {
        let reason = LiveConfigRejectionReason::InputBackpressured {
            max_pending_bytes: 64 * 1024 * 1024,
        };
        let error = LiveAdapterHostError::AdapterError(LiveAdapterError::ProviderError {
            code: LiveAdapterErrorCode::ConfigRejected {
                reason: reason.clone(),
            },
            message: reason.to_string(),
        });

        assert!(matches!(
            crate::transport::scoped_command_rejection_from_host_error(&error),
            Some(LiveAdapterObservation::CommandRejected {
                code: LiveAdapterErrorCode::ConfigRejected {
                    reason: LiveConfigRejectionReason::InputBackpressured {
                        max_pending_bytes,
                    },
                },
                ..
            }) if max_pending_bytes == 64 * 1024 * 1024
        ));
        assert!(
            crate::transport::scoped_command_rejection_from_host_error(
                &LiveAdapterHostError::AdapterError(LiveAdapterError::Closed)
            )
            .is_none(),
            "closed adapters remain terminal"
        );
    }

    struct FailingLiveProjectionSink;

    #[async_trait]
    impl LiveProjectionSink for FailingLiveProjectionSink {
        async fn append_user_transcript(
            &self,
            _session_id: &SessionId,
            _text: &str,
            _identity: LiveTranscriptIdentity<'_>,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn append_assistant_text_delta(
            &self,
            _session_id: &SessionId,
            _delta: &str,
            _identity: LiveTranscriptIdentity<'_>,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn append_assistant_transcript_delta(
            &self,
            _session_id: &SessionId,
            _delta: &str,
            _identity: LiveTranscriptIdentity<'_>,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn append_assistant_text_final(
            &self,
            _session_id: &SessionId,
            _text: &str,
            _identity: LiveTranscriptIdentity<'_>,
            _stop_reason: StopReason,
            _usage: Usage,
            _response_id: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn append_assistant_transcript_final(
            &self,
            _session_id: &SessionId,
            _text: &str,
            _identity: LiveTranscriptIdentity<'_>,
            _stop_reason: StopReason,
            _usage: Usage,
            _response_id: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn truncate_assistant_transcript(
            &self,
            _session_id: &SessionId,
            _provider_item_id: Option<&str>,
            _previous_item_id: Option<&str>,
            _content_index: Option<u32>,
            _response_id: Option<&str>,
            _text: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn signal_turn_interrupt(
            &self,
            _session_id: &SessionId,
            _response_id: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            Err(LiveProjectionError::Rejected(
                "interrupt sink rejected".to_string(),
            ))
        }

        async fn signal_output_audio_degraded(
            &self,
            _session_id: &SessionId,
            _dropped: u64,
        ) -> Result<(), LiveProjectionError> {
            Err(LiveProjectionError::Rejected(
                "output degradation sink rejected".to_string(),
            ))
        }

        async fn signal_turn_completed(
            &self,
            _session_id: &SessionId,
            _stop_reason: StopReason,
            _usage: Usage,
            _response_id: Option<&str>,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn signal_terminal_error(
            &self,
            _session_id: &SessionId,
            _code: LiveAdapterErrorCode,
            _message: &str,
        ) -> Result<(), LiveProjectionError> {
            Ok(())
        }

        async fn append_realtime_transcript(
            &self,
            _session_id: &SessionId,
            _event: &meerkat_core::RealtimeTranscriptEvent,
        ) -> Result<meerkat_core::RealtimeTranscriptApplyOutcome, LiveProjectionError> {
            Ok(meerkat_core::RealtimeTranscriptApplyOutcome::default())
        }
    }

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

    fn dormant_peer_tasks() -> Arc<WebrtcPeerTaskControl> {
        Arc::new(WebrtcPeerTaskControl::new())
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

    #[test]
    fn outgoing_audio_control_tracks_dropped_packets_as_typed_signal() {
        // D223: an RTP-queue-full drop is a typed delivery-degraded fact
        // (cumulative count), not just a log line. (Fails-old: there was no
        // drop counter — the drop was `tracing::warn`-and-forget.)
        let control = OutgoingAudioControl::default();
        assert_eq!(control.dropped_packets(), 0);
        assert_eq!(control.note_dropped(), 1);
        assert_eq!(control.note_dropped(), 2);
        assert_eq!(control.dropped_packets(), 2);
        // Discarding the queue (barge-in) does not erase the delivery-degraded
        // record — the two facts are independent.
        let _ = control.discard_queued();
        assert_eq!(control.dropped_packets(), 2);
    }

    #[tokio::test]
    async fn barge_in_signal_rejection_keeps_queued_output_audio() {
        // The interrupt/truncate authority must be accepted before WebRTC
        // discards queued output audio. If the interrupt signal is rejected,
        // the queue remains intact so the transport has not silently lost
        // output before command authority.
        let host = LiveAdapterHost::new(Arc::new(FailingLiveProjectionSink));
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(SessionId::new())
            .await
            .unwrap();
        let control = OutgoingAudioControl::default();
        control.note_queued();
        control.note_queued();

        let result = signal_barge_in_then_discard_output_audio(&host, &channel_id, &control).await;

        assert!(
            matches!(result, Err(LiveAdapterHostError::ProjectionError(_))),
            "barge-in rejection must surface instead of discarding output audio"
        );
        assert_eq!(
            control.queued_packets(),
            2,
            "queued output audio must not be discarded before accepted interrupt authority"
        );
    }

    #[tokio::test]
    async fn output_audio_degradation_signal_failure_closes_channel() {
        // Once an output-audio packet is dropped, warning-only failure to
        // record the degradation would lose truth. The WebRTC helper must
        // force a typed/generated close outcome when the degradation signal
        // cannot be projected.
        let host = Arc::new(LiveAdapterHost::new(Arc::new(FailingLiveProjectionSink)));
        let session_id = SessionId::new();
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(session_id)
            .await
            .unwrap();
        let close_feedback =
            crate::transport::GeneratedTestMachineLiveChannelCloseFeedback::new(Arc::clone(&host));

        let continued =
            signal_output_audio_degraded_or_close(host.as_ref(), &close_feedback, &channel_id, 7)
                .await;

        assert!(!continued, "projection failure must stop the WebRTC pump");
        assert_eq!(
            host.channel_status(&channel_id).await.unwrap(),
            LiveAdapterStatus::Closed
        );
    }

    // -----------------------------------------------------------------
    // D124: WebRTC signaling errors are typed per phase
    // -----------------------------------------------------------------

    #[test]
    fn webrtc_error_reason_codes_are_distinct_per_phase() {
        // Each typed signaling/audio variant exposes a distinct stable reason
        // code so the RPC answer-admission path routes on the typed class, not
        // a single `Webrtc(String)` blob. (Fails-old: `Webrtc`/`Audio` tuple
        // variants no longer exist.)
        let cases = [
            (
                LiveWebrtcError::CodecRegistration { detail: "x".into() },
                "codec_registration",
            ),
            (
                LiveWebrtcError::PeerCreation { detail: "x".into() },
                "peer_creation",
            ),
            (
                LiveWebrtcError::SetRemoteDescription { detail: "x".into() },
                "set_remote_description",
            ),
            (
                LiveWebrtcError::CreateAnswer { detail: "x".into() },
                "create_answer",
            ),
            (
                LiveWebrtcError::DataChannelSend { detail: "x".into() },
                "data_channel_send",
            ),
            (LiveWebrtcError::Audio { detail: "x".into() }, "audio"),
            (
                LiveWebrtcError::ChannelNotFound("c".into()),
                "channel_not_found",
            ),
            (
                LiveWebrtcError::MissingLocalDescription,
                "missing_local_description",
            ),
            (
                LiveWebrtcError::PeerClose { detail: "x".into() },
                "peer_close",
            ),
        ];
        let mut seen = std::collections::HashSet::new();
        for (err, expected) in cases {
            assert_eq!(err.reason_code(), expected, "reason_code for {err:?}");
            assert!(
                seen.insert(err.reason_code()),
                "reason codes must be distinct: {expected}"
            );
        }
    }

    #[test]
    fn webrtc_host_error_frame_carries_stable_reason_code() {
        let err = LiveAdapterHostError::NoAdapter(LiveChannelId::new("ch-1"));
        let frame: serde_json::Value =
            serde_json::from_str(&webrtc_host_error_frame_json(&err)).expect("error frame JSON");

        assert_eq!(frame["reason"], "no_adapter");
        assert!(
            frame["error"]
                .as_str()
                .is_some_and(|message| message.contains("no adapter attached"))
        );
    }

    // -----------------------------------------------------------------
    // D346: a generated answer-result rejection after answer_offer leaves
    // zero entries in the peer registry (fail-closed cleanup).
    // -----------------------------------------------------------------

    async fn wait_for_peer_tasks_to_stop(state: &LiveWebrtcState) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while state
                .construction_test_state
                .active_audio_pumps
                .load(Ordering::SeqCst)
                != 0
                || state
                    .construction_test_state
                    .active_observation_pumps
                    .load(Ordering::SeqCst)
                    != 0
                || state
                    .construction_test_state
                    .active_incoming_audio_tasks
                    .load(Ordering::SeqCst)
                    != 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("all WebRTC peer tasks must terminate with peer custody");
    }

    async fn wait_for_observation_pump_to_start(state: &LiveWebrtcState) {
        tokio::time::timeout(Duration::from_secs(2), async {
            while state
                .construction_test_state
                .active_observation_pumps
                .load(Ordering::SeqCst)
                == 0
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("WebRTC observation pump must start after data-channel open");
    }

    #[tokio::test]
    async fn malformed_offer_closes_construction_peer_without_self_cycle_or_audio_task() {
        ensure_test_crypto_provider();
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(SessionId::new())
            .await
            .unwrap();
        let state = LiveWebrtcState::new_for_test_with_generated_close_feedback(host);
        state
            .construction_test_state
            .capture_peer
            .store(true, Ordering::SeqCst);

        let error = state
            .answer_offer(channel_id, "not valid SDP".to_string())
            .await
            .expect_err("malformed SDP must fail answer construction");
        assert!(matches!(
            error,
            LiveWebrtcError::SetRemoteDescription { .. }
                | LiveWebrtcError::ConstructionCleanup { .. }
        ));
        assert_eq!(state.construction_peer_count(), 0);
        assert_eq!(state.peer_count().await, 0);
        assert_eq!(state.pending_answer_count().await, 0);

        let peer = state
            .construction_test_state
            .last_peer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("test captures the exact constructed peer");
        assert_eq!(
            peer.connection_state(),
            RTCPeerConnectionState::Closed,
            "pre-publication failure must reach typed Closed"
        );
        wait_for_peer_tasks_to_stop(&state).await;
        let weak_peer = Arc::downgrade(&peer);
        drop(peer);
        assert!(
            weak_peer.upgrade().is_none(),
            "stored WebRTC callbacks must not strongly retain their peer"
        );
    }

    #[tokio::test]
    async fn cancelled_negotiation_detaches_close_and_retires_exact_construction_custody() {
        ensure_test_crypto_provider();
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(SessionId::new())
            .await
            .unwrap();
        let state = Arc::new(LiveWebrtcState::new_for_test_with_generated_close_feedback(
            host,
        ));
        state
            .construction_test_state
            .capture_peer
            .store(true, Ordering::SeqCst);
        let pause = Arc::new(AnswerConstructionTestPause::default());
        *state
            .construction_test_state
            .pause
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = Some(Arc::clone(&pause));
        let reached = pause.reached.notified();
        let answer_state = Arc::clone(&state);
        let answer_channel_id = channel_id.clone();
        let answer = tokio::spawn(async move {
            answer_state
                .answer_offer(answer_channel_id, "not valid SDP".to_string())
                .await
        });
        reached.await;
        assert_eq!(state.construction_peer_count(), 1);

        answer.abort();
        assert!(answer.await.unwrap_err().is_cancelled());
        state
            .wait_for_answer_construction_cleanup(&channel_id)
            .await;
        assert_eq!(state.construction_peer_count(), 0);
        assert_eq!(state.peer_count().await, 0);
        assert_eq!(state.pending_answer_count().await, 0);

        let peer = state
            .construction_test_state
            .last_peer
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("test captures the exact cancelled peer");
        assert_eq!(
            peer.connection_state(),
            RTCPeerConnectionState::Closed,
            "detached cancellation cleanup must reach typed Closed"
        );
        wait_for_peer_tasks_to_stop(&state).await;
        let weak_peer = Arc::downgrade(&peer);
        drop(peer);
        assert!(
            weak_peer.upgrade().is_none(),
            "cancelled construction must leave no callback self-cycle"
        );
    }

    #[tokio::test]
    async fn answer_peer_publication_is_atomic_under_cancellation() {
        ensure_test_crypto_provider();
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let state = Arc::new(LiveWebrtcState::new_for_test_with_generated_close_feedback(
            host,
        ));
        let channel_id = LiveChannelId::new("atomic-answer-publication");
        let peer = Arc::new(new_browser_peer().await);
        let cleanup_peer = Arc::clone(&peer);
        let peer_tasks = dormant_peer_tasks();
        let mut construction = AnswerConstructionCustody::new(
            99,
            channel_id.clone(),
            Arc::clone(&peer),
            Arc::clone(&state.construction_peers),
        );
        construction.attach_peer_tasks(Arc::clone(&peer_tasks));

        // Hold the second custody map so publication parks after acquiring the
        // first guard but before either insertion. Aborting at that await must
        // publish neither half.
        let pending_guard = state.pending_answers.lock().await;
        let publish_state = Arc::clone(&state);
        let publish = tokio::spawn(async move {
            publish_state
                .publish_answer_peer_with_cleanup_custody(
                    channel_id,
                    construction,
                    peer,
                    Arc::new(OutgoingAudioControl::default()),
                    peer_tasks,
                    10,
                )
                .await;
        });

        let mut parked_on_second_map = false;
        for _ in 0..1_000 {
            if state.peers.try_lock().is_err() {
                parked_on_second_map = true;
                break;
            }
            tokio::task::yield_now().await;
        }
        assert!(
            parked_on_second_map,
            "publication should hold the peer-map guard while awaiting pending custody"
        );
        publish.abort();
        assert!(publish.await.unwrap_err().is_cancelled());
        drop(pending_guard);

        assert_eq!(state.peer_count().await, 0);
        assert_eq!(state.pending_answer_count().await, 0);
        state
            .wait_for_answer_construction_cleanup(&LiveChannelId::new("atomic-answer-publication"))
            .await;
        assert_eq!(
            cleanup_peer.connection_state(),
            RTCPeerConnectionState::Closed
        );
    }

    #[tokio::test]
    async fn answer_result_rejection_after_answer_offer_leaves_no_orphaned_peer() {
        ensure_test_crypto_provider();
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(SessionId::new())
            .await
            .unwrap();
        let (adapter, _command_rx, _observation_tx) = RecordingAdapter::new();
        host.attach_adapter(&channel_id, adapter).await.unwrap();
        host.commit_status_with_generated_test_machine_authority(
            &channel_id,
            LiveAdapterStatus::Ready,
        )
        .await
        .unwrap();

        let state = LiveWebrtcState::new_for_test_with_generated_close_feedback(Arc::clone(&host));
        let browser_peer = new_browser_peer().await;
        // A WebRTC offer needs at least one m-line to carry ICE credentials;
        // an offer with no data channel / track yields an SDP with no
        // ice-ufrag, which the answer side's set_remote_description rejects.
        // Open the canonical "meerkat.live" data channel before offering,
        // mirroring the other webrtc tests in this module.
        let _dc = browser_peer
            .create_data_channel("meerkat.live", None)
            .await
            .unwrap();
        let offer = browser_peer.create_offer(None).await.unwrap();
        let mut gathering_complete = browser_peer.gathering_complete_promise().await;
        browser_peer.set_local_description(offer).await.unwrap();
        let _ = gathering_complete.recv().await;
        let offer_sdp = browser_peer.local_description().await.unwrap().sdp;

        // answer_offer materializes the transport peer into the registry.
        let answer = state
            .answer_offer(channel_id.clone(), offer_sdp)
            .await
            .unwrap();
        assert_eq!(
            state.peer_count().await,
            1,
            "answer_offer must install the peer"
        );

        // Simulate rust-webrtc's one-shot close contract: the peer reaches its
        // typed Closed state, but a subcomponent shutdown error is still
        // returned. The error must remain visible while exact closed custody is
        // retired; calling close again would falsely return Ok without retrying
        // the failed sub-close.
        let close_attempts = Arc::new(AtomicUsize::new(0));
        let observed_attempts = Arc::clone(&close_attempts);
        let error = state
            .close_rejected_answer_peer_with(
                &channel_id,
                answer.answer_observation_sequence,
                move |peer| {
                    let observed_attempts = Arc::clone(&observed_attempts);
                    async move {
                        observed_attempts.fetch_add(1, Ordering::SeqCst);
                        peer.close()
                            .await
                            .map_err(|error| LiveWebrtcError::PeerClose {
                                detail: error.to_string(),
                            })?;
                        Err(LiveWebrtcError::PeerClose {
                            detail: "injected subcomponent shutdown failure".to_string(),
                        })
                    }
                },
            )
            .await
            .expect_err("shutdown error must remain visible");
        assert!(error.to_string().contains("injected subcomponent"));
        assert_eq!(close_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            state.peer_count().await,
            0,
            "a rejected answer-result must leave zero peers in the registry"
        );
        assert_eq!(
            state.pending_answer_count().await,
            0,
            "the exact rejected answer cleanup obligation must be retired"
        );
    }

    #[tokio::test]
    async fn rejected_answer_cleanup_retains_custody_without_racing_one_shot_retry() {
        ensure_test_crypto_provider();
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let state = LiveWebrtcState::new_for_test_with_generated_close_feedback(host);
        let channel_id = LiveChannelId::new("unclosed-custody");
        let sequence = 9;
        let peer = Arc::new(new_browser_peer().await);
        let peer_tasks = dormant_peer_tasks();
        peer_tasks.publish(sequence);
        state.pending_answers.lock().await.insert(
            sequence,
            LiveWebrtcPendingAnswer {
                channel_id: channel_id.clone(),
                peer: Arc::clone(&peer),
                peer_tasks: Arc::clone(&peer_tasks),
            },
        );
        state.peers.lock().await.insert(
            channel_id.clone(),
            LiveWebrtcPeer {
                peer: Arc::clone(&peer),
                outgoing_audio: Arc::new(OutgoingAudioControl::default()),
                peer_tasks,
                answer_observation_sequence: sequence,
            },
        );

        let close_attempts = Arc::new(AtomicUsize::new(0));
        let observed_attempts = Arc::clone(&close_attempts);
        state
            .close_rejected_answer_peer_with(&channel_id, sequence, move |_peer| async move {
                observed_attempts.fetch_add(1, Ordering::SeqCst);
                Err(LiveWebrtcError::PeerClose {
                    detail: "injected failure before closed state".to_string(),
                })
            })
            .await
            .expect_err("unclosed peer must preserve the cleanup error");

        assert_eq!(state.peer_count().await, 1);
        assert_eq!(state.pending_answer_count().await, 1);
        let repeated_error = state
            .close_rejected_answer_peer(&channel_id, sequence)
            .await
            .expect_err("a consumed one-shot close must return its recorded failure");
        assert!(repeated_error.to_string().contains("injected failure"));
        assert_eq!(
            close_attempts.load(Ordering::SeqCst),
            1,
            "later callers must join the exact outcome, not race another peer close"
        );
        assert_eq!(state.peer_count().await, 1);
        assert_eq!(state.pending_answer_count().await, 1);

        // Test-only fixture cleanup. Production preserves this custody because
        // rust-webrtc's one-shot close cannot be retried truthfully after a
        // non-Closed failure.
        peer.close().await.unwrap();
    }

    #[tokio::test]
    async fn rejected_answer_cleanup_leaves_a_replacement_peer_untouched() {
        ensure_test_crypto_provider();
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let state = LiveWebrtcState::new_for_test_with_generated_close_feedback(host);
        let channel_id = LiveChannelId::new("replacement-custody");
        let rejected_sequence = 7;
        let replacement_sequence = 8;
        let rejected_peer = Arc::new(new_browser_peer().await);
        let replacement_peer = Arc::new(new_browser_peer().await);
        let rejected_peer_tasks = dormant_peer_tasks();
        rejected_peer_tasks.publish(rejected_sequence);

        state.pending_answers.lock().await.insert(
            rejected_sequence,
            LiveWebrtcPendingAnswer {
                channel_id: channel_id.clone(),
                peer: Arc::clone(&rejected_peer),
                peer_tasks: rejected_peer_tasks,
            },
        );
        state.peers.lock().await.insert(
            channel_id.clone(),
            LiveWebrtcPeer {
                peer: Arc::clone(&replacement_peer),
                outgoing_audio: Arc::new(OutgoingAudioControl::default()),
                peer_tasks: {
                    let peer_tasks = dormant_peer_tasks();
                    peer_tasks.publish(replacement_sequence);
                    peer_tasks
                },
                answer_observation_sequence: replacement_sequence,
            },
        );

        state
            .close_rejected_answer_peer(&channel_id, rejected_sequence)
            .await
            .unwrap();

        let peers = state.peers.lock().await;
        let current = peers
            .get(&channel_id)
            .expect("replacement must remain registered");
        assert_eq!(current.answer_observation_sequence, replacement_sequence);
        assert!(Arc::ptr_eq(&current.peer, &replacement_peer));
        drop(peers);
        assert_eq!(state.pending_answer_count().await, 0);

        replacement_peer.close().await.unwrap();
    }

    #[tokio::test]
    async fn stale_disconnect_cleanup_retires_only_its_exact_peer_and_tasks() {
        ensure_test_crypto_provider();
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(SessionId::new())
            .await
            .unwrap();
        host.commit_status_with_generated_test_machine_authority(
            &channel_id,
            LiveAdapterStatus::Ready,
        )
        .await
        .unwrap();
        let state = LiveWebrtcState::new_for_test_with_generated_close_feedback(Arc::clone(&host));
        let stale_sequence = 17;
        let replacement_sequence = 18;
        let stale_peer = Arc::new(new_browser_peer().await);
        let replacement_peer = Arc::new(new_browser_peer().await);
        let stale_tasks = dormant_peer_tasks();
        stale_tasks.publish(stale_sequence);
        let replacement_tasks = dormant_peer_tasks();
        replacement_tasks.publish(replacement_sequence);
        let stale_shutdown = stale_tasks.subscribe();
        let replacement_shutdown = replacement_tasks.subscribe();

        state.pending_answers.lock().await.insert(
            stale_sequence,
            LiveWebrtcPendingAnswer {
                channel_id: channel_id.clone(),
                peer: Arc::clone(&stale_peer),
                peer_tasks: Arc::clone(&stale_tasks),
            },
        );
        state.peers.lock().await.insert(
            channel_id.clone(),
            LiveWebrtcPeer {
                peer: Arc::clone(&replacement_peer),
                outgoing_audio: Arc::new(OutgoingAudioControl::default()),
                peer_tasks: Arc::clone(&replacement_tasks),
                answer_observation_sequence: replacement_sequence,
            },
        );
        cleanup_peer_after_disconnect(PeerDisconnectContext {
            host: Arc::clone(&host),
            channel_id: channel_id.clone(),
            peer: Arc::downgrade(&stale_peer),
            peer_registry: Arc::downgrade(&state.peers),
            pending_answers: Arc::downgrade(&state.pending_answers),
            published_peer_lifecycle: Arc::downgrade(&state.published_peer_lifecycle),
            close_feedback: Arc::clone(&state.close_feedback),
            peer_tasks: stale_tasks,
        })
        .await;

        assert_eq!(
            host.channel_status(&channel_id).await.unwrap(),
            LiveAdapterStatus::Ready
        );
        assert!(*stale_shutdown.borrow());
        assert!(!*replacement_shutdown.borrow());
        assert_eq!(state.pending_answer_count().await, 0);
        let peers = state.peers.lock().await;
        let current = peers
            .get(&channel_id)
            .expect("stale disconnect must leave replacement registered");
        assert_eq!(current.answer_observation_sequence, replacement_sequence);
        assert!(Arc::ptr_eq(&current.peer, &replacement_peer));
        drop(peers);
        assert_eq!(
            stale_peer.connection_state(),
            RTCPeerConnectionState::Closed
        );

        replacement_peer.close().await.unwrap();
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
        state
            .construction_test_state
            .capture_data_channel
            .store(true, Ordering::SeqCst);
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
        let server_peer = {
            let peers = state.peers.lock().await;
            Arc::downgrade(
                &peers
                    .get(&channel_id)
                    .expect("connected server peer must be registered")
                    .peer,
            )
        };
        wait_for_observation_pump_to_start(&state).await;
        let server_data_channel = state
            .construction_test_state
            .last_data_channel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("server data channel must be captured after open");

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

        state.close_peer_checked(&channel_id).await.unwrap();
        wait_for_peer_tasks_to_stop(&state).await;
        assert_eq!(state.peer_count().await, 0);
        assert_eq!(state.pending_answer_count().await, 0);
        assert_eq!(state.published_peer_lifecycle_count(), 0);
        tokio::time::timeout(Duration::from_secs(2), async {
            while server_peer.upgrade().is_some() || server_data_channel.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("normal close must reclaim the exact server peer and data-channel callback graph");
        browser_peer.close().await.unwrap();
    }

    #[tokio::test]
    async fn timed_out_checked_close_keeps_owned_cleanup_running_to_exact_retirement() {
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

        let state = Arc::new(LiveWebrtcState::new_for_test_with_generated_close_feedback(
            host,
        ));
        state
            .construction_test_state
            .capture_data_channel
            .store(true, Ordering::SeqCst);
        let browser_peer = new_browser_peer().await;
        let browser_data_channel = browser_peer
            .create_data_channel("meerkat.live", None)
            .await
            .unwrap();
        let (open_tx, mut open_rx) = mpsc::channel::<()>(1);
        browser_data_channel.on_open(Box::new(move || {
            let open_tx = open_tx.clone();
            Box::pin(async move {
                let _ = open_tx.send(()).await;
            })
        }));

        connect_browser_peer(&browser_peer, &state, &channel_id).await;
        tokio::time::timeout(Duration::from_secs(5), open_rx.recv())
            .await
            .unwrap()
            .unwrap();
        wait_for_observation_pump_to_start(&state).await;
        let server_peer = {
            let peers = state.peers.lock().await;
            Arc::downgrade(
                &peers
                    .get(&channel_id)
                    .expect("connected server peer must be registered")
                    .peer,
            )
        };
        let server_data_channel = state
            .construction_test_state
            .last_data_channel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("server data channel must be captured after open");

        let close_started = Arc::new(Notify::new());
        let release_close = Arc::new(Notify::new());
        let close_attempts = Arc::new(AtomicUsize::new(0));
        let close_state = Arc::clone(&state);
        let close_channel_id = channel_id.clone();
        let observed_started = Arc::clone(&close_started);
        let observed_release = Arc::clone(&release_close);
        let observed_attempts = Arc::clone(&close_attempts);
        let timed_close = tokio::spawn(async move {
            tokio::time::timeout(
                Duration::from_millis(50),
                close_state.close_peer_checked_with_operation(
                    &close_channel_id,
                    Some(Box::new(move |peer| {
                        Box::pin(async move {
                            observed_attempts.fetch_add(1, Ordering::SeqCst);
                            observed_started.notify_one();
                            observed_release.notified().await;
                            peer.close()
                                .await
                                .map_err(|error| LiveWebrtcError::PeerClose {
                                    detail: error.to_string(),
                                })
                        })
                    })),
                ),
            )
            .await
        });

        tokio::time::timeout(Duration::from_secs(5), close_started.notified())
            .await
            .expect("owned close task must start before the caller timeout");
        assert!(
            timed_close.await.unwrap().is_err(),
            "the checked-close waiter must be cancelled while physical close is parked"
        );
        assert_eq!(close_attempts.load(Ordering::SeqCst), 1);

        // The timeout dropped only the subscriber. The exact runtime-owned
        // coordinator still owns the one physical close and its retirement.
        release_close.notify_one();
        wait_for_peer_count(&state, 0).await;
        wait_for_peer_tasks_to_stop(&state).await;
        assert_eq!(state.pending_answer_count().await, 0);
        assert_eq!(state.published_peer_lifecycle_count(), 0);
        assert_eq!(close_attempts.load(Ordering::SeqCst), 1);
        assert_eq!(
            state.host.channel_status(&channel_id).await.unwrap(),
            LiveAdapterStatus::Ready,
            "checked physical close must not mint remote-disconnect semantic authority"
        );
        tokio::time::timeout(Duration::from_secs(2), async {
            while server_peer.upgrade().is_some() || server_data_channel.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("cancelled checked close must still reclaim the exact peer and channel graph");

        drop(observation_tx);
        browser_peer.close().await.unwrap();
    }

    #[tokio::test]
    async fn remote_data_channel_disconnect_closes_host_and_reclaims_exact_peer_tasks() {
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
        state
            .construction_test_state
            .capture_data_channel
            .store(true, Ordering::SeqCst);
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

        connect_browser_peer(&browser_peer, &state, &channel_id).await;
        tokio::time::timeout(Duration::from_secs(5), open_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let server_peer = {
            let peers = state.peers.lock().await;
            Arc::downgrade(
                &peers
                    .get(&channel_id)
                    .expect("connected server peer must be registered")
                    .peer,
            )
        };
        wait_for_observation_pump_to_start(&state).await;
        let server_data_channel = state
            .construction_test_state
            .last_data_channel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("server data channel must be captured after open");
        // Keep the adapter's observation sender alive and idle. The only way
        // for this pump to exit is the exact peer lifecycle shutdown signal.
        data_channel.close().await.unwrap();
        wait_for_peer_count(&state, 0).await;
        wait_for_peer_tasks_to_stop(&state).await;
        assert_eq!(state.pending_answer_count().await, 0);
        assert_eq!(state.published_peer_lifecycle_count(), 0);
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if host.channel_status(&channel_id).await.unwrap() == LiveAdapterStatus::Closed {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("remote WebRTC disconnect must commit generated semantic close feedback");
        tokio::time::timeout(Duration::from_secs(2), async {
            while server_peer.upgrade().is_some() || server_data_channel.upgrade().is_some() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("remote close must reclaim the exact server peer and data channel");

        drop(observation_tx);
        browser_peer.close().await.unwrap();
    }

    #[tokio::test]
    async fn dropping_webrtc_state_reclaims_active_published_peer_channel_and_idle_tasks() {
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

        let state = LiveWebrtcState::new_for_test_with_generated_close_feedback(host);
        state
            .construction_test_state
            .capture_data_channel
            .store(true, Ordering::SeqCst);
        let browser_peer = new_browser_peer().await;
        let browser_data_channel = browser_peer
            .create_data_channel("meerkat.live", None)
            .await
            .unwrap();
        let (open_tx, mut open_rx) = mpsc::channel::<()>(1);
        browser_data_channel.on_open(Box::new(move || {
            let open_tx = open_tx.clone();
            Box::pin(async move {
                let _ = open_tx.send(()).await;
            })
        }));

        connect_browser_peer(&browser_peer, &state, &channel_id).await;
        tokio::time::timeout(Duration::from_secs(5), open_rx.recv())
            .await
            .unwrap()
            .unwrap();
        wait_for_observation_pump_to_start(&state).await;

        let server_peer = {
            let peers = state.peers.lock().await;
            Arc::downgrade(
                &peers
                    .get(&channel_id)
                    .expect("connected server peer must be registered")
                    .peer,
            )
        };
        let server_data_channel = state
            .construction_test_state
            .last_data_channel
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
            .expect("server data channel must be captured after open");
        let peer_registry = Arc::downgrade(&state.peers);
        let pending_answers = Arc::downgrade(&state.pending_answers);
        let published_peer_lifecycle = Arc::downgrade(&state.published_peer_lifecycle);
        let active_audio_pumps = Arc::clone(&state.construction_test_state.active_audio_pumps);
        let active_observation_pumps =
            Arc::clone(&state.construction_test_state.active_observation_pumps);
        let active_incoming_audio_tasks =
            Arc::clone(&state.construction_test_state.active_incoming_audio_tasks);
        assert_eq!(state.peer_count().await, 1);
        assert_eq!(state.pending_answer_count().await, 1);
        assert_eq!(state.published_peer_lifecycle_count(), 1);

        // Keep the adapter observation stream alive and idle. Without owner
        // teardown the observation pump parks forever and retains the server
        // data channel, while the peer callback graph retains both registries.
        drop(state);

        let reclaimed = tokio::time::timeout(Duration::from_secs(5), async {
            while active_audio_pumps.load(Ordering::SeqCst) != 0
                || active_observation_pumps.load(Ordering::SeqCst) != 0
                || active_incoming_audio_tasks.load(Ordering::SeqCst) != 0
                || server_peer.upgrade().is_some()
                || server_data_channel.upgrade().is_some()
                || peer_registry.upgrade().is_some()
                || pending_answers.upgrade().is_some()
                || published_peer_lifecycle.upgrade().is_some()
            {
                tokio::task::yield_now().await;
            }
        })
        .await;
        assert!(
            reclaimed.is_ok(),
            "owner drop must reclaim every published WebRTC registry, peer, channel, and task: audio={}, observations={}, incoming={}, peer={}, channel={}, peers={}, pending={}, lifecycle={}",
            active_audio_pumps.load(Ordering::SeqCst),
            active_observation_pumps.load(Ordering::SeqCst),
            active_incoming_audio_tasks.load(Ordering::SeqCst),
            server_peer.upgrade().is_some(),
            server_data_channel.upgrade().is_some(),
            peer_registry.upgrade().is_some(),
            pending_answers.upgrade().is_some(),
            published_peer_lifecycle.upgrade().is_some(),
        );

        drop(observation_tx);
        browser_peer.close().await.unwrap();
    }

    #[tokio::test]
    async fn webrtc_peer_rejects_small_image_and_keeps_same_data_channel_usable() {
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(SessionId::new())
            .await
            .unwrap();
        let (adapter, mut command_rx, _observation_tx) = RecordingAdapter::new();
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

        connect_browser_peer(&browser_peer, &state, &channel_id).await;
        tokio::time::timeout(Duration::from_secs(5), open_rx.recv())
            .await
            .unwrap()
            .unwrap();

        let image_envelope = r#"{"kind":"image","idempotency_key":"image-request-1","mime":"image/png","data":"iVBORw0KGgo="}"#.to_owned();
        assert!(
            image_envelope.len() < 65_535,
            "fixture must remain within the effective data-channel ceiling"
        );
        data_channel.send_text(image_envelope).await.unwrap();

        let rejection_json = tokio::time::timeout(Duration::from_secs(5), data_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let rejection: serde_json::Value = serde_json::from_str(&rejection_json).unwrap();
        assert_eq!(rejection["observation"], "command_rejected");
        assert_eq!(rejection["code"]["code"], "config_rejected");
        assert_eq!(
            rejection["code"]["reason"]["kind"],
            "image_input_transport_unsupported"
        );
        assert_eq!(
            rejection["code"]["reason"]["transport"],
            "webrtc_data_channel_use_live_send_input_rpc"
        );
        assert!(matches!(
            command_rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
        assert_eq!(state.peer_count().await, 1);

        data_channel
            .send_text(r#"{"kind":"text","text":"still-open"}"#.to_owned())
            .await
            .unwrap();
        match tokio::time::timeout(Duration::from_secs(5), command_rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            LiveAdapterCommand::SendInput {
                chunk: LiveInputChunk::Text { text },
            } => assert_eq!(text, "still-open"),
            other => panic!("expected text input on the surviving data channel, got {other:?}"),
        }
        assert_eq!(state.peer_count().await, 1);

        state.close_peer(&channel_id).await;
        browser_peer.close().await.unwrap();
    }

    #[tokio::test]
    async fn webrtc_data_channel_scoped_backpressure_keeps_peer_usable() {
        let host = Arc::new(LiveAdapterHost::new(Arc::new(NoOpProjectionSink)));
        let channel_id = host
            .open_channel_with_generated_test_machine_authority(SessionId::new())
            .await
            .unwrap();
        let (adapter, mut command_rx, _observation_tx) = RejectFirstInputAdapter::new();
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

        connect_browser_peer(&browser_peer, &state, &channel_id).await;
        tokio::time::timeout(Duration::from_secs(5), open_rx.recv())
            .await
            .unwrap()
            .unwrap();

        data_channel
            .send_text(r#"{"kind":"text","text":"backpressure-me"}"#.to_owned())
            .await
            .unwrap();
        let rejection_json = tokio::time::timeout(Duration::from_secs(5), data_rx.recv())
            .await
            .unwrap()
            .unwrap();
        let rejection: serde_json::Value = serde_json::from_str(&rejection_json).unwrap();
        assert_eq!(rejection["observation"], "command_rejected");
        assert_eq!(rejection["code"]["code"], "config_rejected");
        assert_eq!(rejection["code"]["reason"]["kind"], "input_backpressured");
        assert_eq!(state.peer_count().await, 1);

        data_channel
            .send_text(r#"{"kind":"text","text":"accepted-after-retry"}"#.to_owned())
            .await
            .unwrap();
        match tokio::time::timeout(Duration::from_secs(5), command_rx.recv())
            .await
            .unwrap()
            .unwrap()
        {
            LiveAdapterCommand::SendInput {
                chunk: LiveInputChunk::Text { text },
            } => assert_eq!(text, "accepted-after-retry"),
            other => panic!("expected text input after scoped retry, got {other:?}"),
        }
        assert_eq!(state.peer_count().await, 1);

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

    struct RejectFirstInputAdapter {
        command_tx: mpsc::Sender<LiveAdapterCommand>,
        observation_rx: Mutex<mpsc::Receiver<LiveAdapterObservation>>,
        rejected: std::sync::atomic::AtomicBool,
    }

    impl RejectFirstInputAdapter {
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
                    rejected: std::sync::atomic::AtomicBool::new(false),
                }),
                command_rx,
                observation_tx,
            )
        }
    }

    #[async_trait]
    impl LiveAdapter for RejectFirstInputAdapter {
        async fn send_command(&self, command: LiveAdapterCommand) -> Result<(), LiveAdapterError> {
            if matches!(&command, LiveAdapterCommand::SendInput { .. })
                && !self
                    .rejected
                    .swap(true, std::sync::atomic::Ordering::SeqCst)
            {
                let reason = LiveConfigRejectionReason::InputBackpressured {
                    max_pending_bytes: 64 * 1024 * 1024,
                };
                return Err(LiveAdapterError::ProviderError {
                    code: LiveAdapterErrorCode::ConfigRejected {
                        reason: reason.clone(),
                    },
                    message: reason.to_string(),
                });
            }
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
